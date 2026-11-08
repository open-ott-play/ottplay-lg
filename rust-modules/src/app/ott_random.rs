//! Internal random choice of an ordinary Play action from the current view.
//! No wire capability, provider classification, durable state or RNG IO.
use super::*;
use std::collections::BTreeSet;

const STALE: &str = "The list changed; select Random again";
const UNAVAILABLE: &str = "Random playback is unavailable for this list";

fn targets(rows: &[OttRow]) -> Vec<&str> {
    let mut seen = BTreeSet::new();
    rows.iter().filter(|row| row.enabled).filter_map(|row| match &row.action {
        OttAction::Play(id) if seen.insert(id.as_str()) => Some(id.as_str()),
        _ => None,
    }).collect()
}

/// Bind the action to a source and view, not an incidental repaint or discovery
/// batch. Dispatch rebuilds this same visible projection before choosing a key.
pub(super) fn context(runtime: &Runtime, rows: &[OttRow]) -> Option<String> {
    let (_, favorites) = runtime.view_selection();
    if !matches!((runtime.page, favorites), ("channels", false) | ("favorites", true))
        || !runtime.loaded || runtime.persistence_uncertain || runtime.saving
        || runtime.entropy.is_none() || runtime.edit_session.is_some()
        || !runtime.play_work_allowed()
        || !rows.iter().any(|row| row.enabled && matches!(row.action, OttAction::Play(_)))
    {
        return None;
    }
    let search = runtime.search_view()?;
    let value = json!([
        runtime.remote.runtime(), runtime.navigation_epoch, runtime.generation,
        runtime.effective_profile(), runtime.view_selection(),
        runtime.transient.accepted.as_ref().map(transient::TransientMedia::owner).map(|owner| owner.0),
        search.query,
        runtime.saved.settings.get(catalog_organization::KEY),
        favorites.then_some(&runtime.saved.favorites),
        favorites.then_some(&runtime.saved.profiles),
    ]);
    Some(plx_data::ott::favorites_wire::hash(&serde_json::to_vec(&value).ok()?))
}

// A bounded rejection sampler avoids modulo bias. SplitMix64 is a local stream
// derived from already loaded boot entropy, not a new security identity or RNG.
fn index(mut seed: u64, length: usize) -> Option<usize> {
    let bound = u64::try_from(length).ok().filter(|bound| *bound != 0)?;
    let threshold = bound.wrapping_neg() % bound;
    for _ in 0..8 {
        seed = seed.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = seed;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^= value >> 31;
        if value >= threshold { return usize::try_from(value % bound).ok(); }
    }
    None
}

/// This runs before ordinary policy dispatch and never changes runtime state.
/// The resulting Play action uses the same PIN, kiosk, lifecycle, source and
/// final native admission as a clicked row. Refusal never draws another row.
pub(super) fn resolve(runtime: &Runtime, action: OttAction) -> Result<OttAction, &'static str> {
    let OttAction::Random(expected) = action else { return Ok(action); };
    let view = runtime.raw_view();
    if view.random_context.as_deref() != Some(expected.as_str()) { return Err(STALE); }
    let ids = targets(&view.rows);
    let entropy = runtime.entropy.ok_or(UNAVAILABLE)?;
    let seed = u64::from_le_bytes(entropy[..8].try_into().map_err(|_| UNAVAILABLE)?)
        ^ u64::from_le_bytes(entropy[8..].try_into().map_err(|_| UNAVAILABLE)?)
        ^ runtime.playback_epoch.rotate_left(17)
        ^ runtime.navigation_epoch.rotate_left(31)
        ^ runtime.generation.rotate_left(47);
    let chosen = index(seed, ids.len()).and_then(|index| ids.get(index)).ok_or(UNAVAILABLE)?;
    Ok(OttAction::Play((*chosen).into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(id: &str, enabled: bool) -> OttRow {
        OttRow { current_programme: None, content_title: None, guide: None, guide_available: false, records_available: false, id: id.into(), title: id.into(), detail: String::new(), description: String::new(),
            poster: None, media: None, enabled, favorite: false, action: OttAction::Play(id.into()) }
    }
    #[test]
    fn targets_are_original_enabled_play_keys_without_duplicate_weight() {
        let mut other = row("navigation", true); other.action = OttAction::ShowSettings;
        let rows = [row("favorite:exact", true), row("other", false), row("favorite:exact", true), other, row("local:id", true)];
        assert_eq!(targets(&rows), ["favorite:exact", "local:id"]);
    }
    #[test]
    fn empty_is_unavailable_and_singleton_can_repeat_the_current_item() {
        assert_eq!(index(1, 0), None);
        for seed in 0..100 { assert_eq!(index(seed, 1), Some(0)); }
    }
    #[test]
    fn sampler_stays_in_bounds_and_is_deterministic_without_external_randomness() {
        for length in [2, 3, 1000, 50000] {
            for seed in 0..100 {
                let selected = index(seed, length).unwrap();
                assert!(selected < length);
                assert_eq!(index(seed, length), Some(selected));
            }
        }
    }
}
