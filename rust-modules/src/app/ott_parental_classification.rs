//! Protection membership is derived from the source-owned catalog category,
//! never a browse category rename, row number or guessed display title.
use super::*;

/// Matches the Classic provider category policy. An unavailable catalog entry
/// is not evidence that a target is ordinary and therefore remains protected.
pub(super) fn marked(entry: Option<&CatalogEntry>) -> bool {
    let Some(entry) = entry else { return true; };
    if entry.protection == Some(true) { return true; }
    let category = entry.category.to_lowercase();
    ["xxx", "ххх", "взрослые", "для взрослых", "эротика", "18+", "adults"]
        .iter().any(|marker| category.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(category: &str) -> CatalogEntry {
        CatalogEntry { protection: None, id: "item".into(), title: "Fixture".into(), category: category.into(),
            description: String::new(), synopsis: None, poster: String::new(),
            media: MediaLocator::Http { url: "https://example.invalid/live.m3u8".into() },
            duration_ms: None, provider_id: None, provider_id_field: None, live: None }
    }
    #[test]
    fn source_membership_is_independent_of_number_title_and_category_navigation() {
        let mut ordinary = entry("Documentary");
        ordinary.title = "Fixture 18+".into();
        assert!(!marked(Some(&ordinary)));
        let mut protected = entry("Catalog / 18+ / Subscription");
        assert!(marked(Some(&protected)));
        protected.id = "renumbered".into(); protected.title = "Renamed".into();
        assert!(marked(Some(&protected)));
        assert!(marked(None), "unknown ownership must not become an ordinary target");
        ordinary.protection = Some(true);
        assert!(marked(Some(&ordinary)), "a source flag must survive an ordinary category");
        protected.protection = Some(false);
        assert!(marked(Some(&protected)), "a false flag cannot remove a category's protection");
    }
}
