//! Category shelves borrow the current All projection; they do not own another
//! catalogue, source selection or playback order.
use super::{OttAction, OttRow, Runtime};
use plx_data::ott::MAX_CATALOG_ENTRIES;
use plx_machine::machine::UiMode;
use plx_screens::ott::OttShelf;
use std::collections::{HashMap, HashSet};

/// Display-only identity, never a provider category or a command target.
const ALL_SHELF_ID: &str = "ott-presentation:all";

/// Presentation memory only, never accepted as a command or read authority.
/// Category navigation and user ordering do not replace the underlying source.
pub(super) fn context(runtime: &Runtime) -> String {
    let identity = serde_json::json!([
        runtime.remote.runtime(), runtime.generation, runtime.effective_profile(), runtime.provider,
    ]);
    plx_data::ott::favorites_wire::hash(identity.to_string().as_bytes())
}

pub(super) fn eligible(runtime: &Runtime) -> bool {
    runtime.interface_mode() == UiMode::Modern
        && runtime.page == "channels"
        && !runtime.view_selection().1
        && runtime.effective_category().is_empty()
        && runtime.search_view().is_some_and(|search| search.query.is_empty())
        && runtime.transient.accepted.is_none()
        && runtime.transient.candidate.is_none()
}

pub(super) fn project(runtime: &Runtime, rows: &[OttRow]) -> Vec<OttShelf> {
    if !eligible(runtime) || rows.is_empty()
        || rows.iter().any(|row| !row.enabled || row.media.is_none()
            || !matches!(&row.action, OttAction::Play(id) if id == &row.id))
    { return Vec::new(); }
    let positions: HashMap<&str, usize> = rows.iter().enumerate()
        .map(|(index, row)| (row.id.as_str(), index)).collect();
    if positions.len() != rows.len() { return Vec::new(); }
    let mut shelves = Vec::new();
    // All and every real category share the same total occurrence budget.
    let mut occurrences = rows.len();
    if occurrences > MAX_CATALOG_ENTRIES { return Vec::new(); }
    if let Some(projection) = runtime.organization_projection() {
        for category in &projection.categories {
            let indices = category.members.iter().filter_map(|id| positions.get(id.as_str()).copied());
            let Some(rows) = bounded_rows(indices, &mut occurrences) else { return Vec::new(); };
            shelves.push(OttShelf { id: category.id.clone(), title: category.label.clone(), rows });
        }
    } else {
        let Ok(groups) = runtime.catalog.checked_category_groups() else { return Vec::new(); };
        for category in groups {
            let indices = category.indices.into_iter().filter_map(|index| runtime.catalog.entries.get(index))
                .filter_map(|entry| positions.get(entry.id.as_str()).copied());
            let Some(rows) = bounded_rows(indices, &mut occurrences) else { return Vec::new(); };
            shelves.push(OttShelf { id: category.id, title: category.label, rows });
        }
    }
    complete(rows.len(), shelves)
}

/// Ancestor categories can repeat each canonical row many times. Admit the
/// whole presentation before allocating occurrence keys; never truncate All.
fn bounded_rows(indices: impl Iterator<Item = usize>, total: &mut usize) -> Option<Vec<usize>> {
    let mut rows = Vec::new();
    for index in indices {
        if *total >= MAX_CATALOG_ENTRIES { return None; }
        *total += 1;
        rows.push(index);
    }
    Some(rows)
}

/// All preserves every canonical row, including uncategorised content. Real
/// categories keep their source identities/order; malformed or oversized maps
/// retain the ordinary flat grid instead of truncating content or sharing keys.
fn complete(count: usize, mut shelves: Vec<OttShelf>) -> Vec<OttShelf> {
    if count == 0 || count > MAX_CATALOG_ENTRIES { return Vec::new(); }
    let mut identities = HashSet::new();
    let mut total = count;
    for shelf in &shelves {
        if shelf.id.is_empty() || shelf.id == ALL_SHELF_ID || !identities.insert(&shelf.id) {
            return Vec::new();
        }
        total = match total.checked_add(shelf.rows.len()).filter(|total| *total <= MAX_CATALOG_ENTRIES) {
            Some(total) => total,
            None => return Vec::new(),
        };
        let mut seen = HashSet::new();
        if shelf.rows.iter().any(|&index| index >= count || !seen.insert(index)) {
            return Vec::new();
        }
    }
    drop(identities);
    shelves.retain(|shelf| !shelf.rows.is_empty());
    shelves.insert(0, OttShelf { id: ALL_SHELF_ID.into(),
        title: plx_platform::i18n::msg::browse_library_all().into(), rows: (0..count).collect() });
    shelves
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shelf(id: &str, rows: &[usize]) -> OttShelf {
        OttShelf { id: id.into(), title: id.into(), rows: rows.to_vec() }
    }
    #[test]
    fn shared_membership_retains_category_order_without_duplicating_rows() {
        let shelves = vec![shelf("films", &[2, 0]), shelf("3d", &[2, 1]), shelf("empty", &[])];
        let result = complete(3, shelves);
        assert_eq!(result[0].id, ALL_SHELF_ID);
        assert_eq!(result[0].rows, [0, 1, 2]);
        assert_eq!(result[1..], [shelf("films", &[2, 0]), shelf("3d", &[2, 1])]);
    }
    #[test]
    fn all_keeps_uncategorised_items_without_changing_real_categories() {
        let result = complete(3, vec![shelf("films", &[0, 1])]);
        assert_eq!(result[0].rows, [0, 1, 2]);
        assert_eq!(result[1], shelf("films", &[0, 1]));
        assert_eq!(complete(3, vec![])[0].rows, [0, 1, 2]);
    }
    #[test]
    fn invalid_or_colliding_memberships_keep_the_complete_flat_fallback() {
        for shelves in [vec![shelf("films", &[0, 3])], vec![shelf("films", &[0, 0])],
            vec![shelf("", &[0])], vec![shelf(ALL_SHELF_ID, &[])],
            vec![shelf("films", &[0]), shelf("films", &[1])]] {
            assert!(complete(3, shelves).is_empty());
        }
    }
    #[test]
    fn all_and_categories_share_the_unchanged_total_occurrence_limit() {
        let count = MAX_CATALOG_ENTRIES / 2;
        let result = complete(count, vec![shelf("films", &(0..count).collect::<Vec<_>>())]);
        assert_eq!(result.iter().map(|shelf| shelf.rows.len()).sum::<usize>(), MAX_CATALOG_ENTRIES);
        assert!(complete(count, vec![shelf("films", &(0..count).collect::<Vec<_>>()), shelf("other", &[0])]).is_empty());
        assert_eq!(complete(MAX_CATALOG_ENTRIES, vec![])[0].rows.len(), MAX_CATALOG_ENTRIES);
        assert!(complete(MAX_CATALOG_ENTRIES + 1, vec![]).is_empty());
    }
    #[test]
    fn repeated_ancestor_memberships_share_one_bounded_occurrence_budget() {
        let mut total = 0;
        assert_eq!(bounded_rows(0..MAX_CATALOG_ENTRIES, &mut total).unwrap().len(), MAX_CATALOG_ENTRIES);
        assert!(bounded_rows(std::iter::empty(), &mut total).unwrap().is_empty());
        assert!(bounded_rows(std::iter::once(0), &mut total).is_none());
        assert_eq!(total, MAX_CATALOG_ENTRIES);
    }
}
