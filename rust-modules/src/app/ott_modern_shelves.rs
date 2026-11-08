//! Category shelves borrow the current All projection; they do not own another
//! catalogue, source selection or playback order.
use super::{OttAction, OttRow, Runtime};
use plx_data::ott::MAX_CATALOG_ENTRIES;
use plx_machine::machine::UiMode;
use plx_screens::ott::OttShelf;
use std::collections::HashMap;

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
    let mut occurrences = 0;
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

/// Never hide uncategorised All items or invent a provider category for them.
/// Until all current rows have a real category, retain the ordinary All grid.
fn complete(count: usize, mut shelves: Vec<OttShelf>) -> Vec<OttShelf> {
    shelves.retain(|shelf| !shelf.rows.is_empty());
    let mut covered = vec![false; count];
    for shelf in &shelves {
        for &index in &shelf.rows {
            let Some(value) = covered.get_mut(index) else { return Vec::new(); };
            *value = true;
        }
    }
    if covered.into_iter().all(|covered| covered) { shelves } else { Vec::new() }
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
        assert_eq!(complete(3, shelves), vec![shelf("films", &[2, 0]), shelf("3d", &[2, 1])]);
    }
    #[test]
    fn incomplete_or_invalid_membership_keeps_all_content_in_flat_grid() {
        assert!(complete(3, vec![shelf("films", &[0, 1])]).is_empty());
        assert!(complete(3, vec![shelf("films", &[0, 1, 2, 3])]).is_empty());
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
