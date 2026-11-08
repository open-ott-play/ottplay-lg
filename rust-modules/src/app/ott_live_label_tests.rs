use super::*;
use super::super::{MediaLocator, ProviderKind};

fn profile() -> ProviderProfile {
    ProviderProfile { id: "source-a".into(), name: "Source A".into(), kind: ProviderKind::M3u,
        source: "https://example.invalid/a.m3u".into(), source_alias: String::new(), enabled: true }
}
fn entry(id: &str) -> CatalogEntry {
    CatalogEntry { id: id.into(), title: format!("Title {id}"), category: "Group".into(),
        description: String::new(), poster: String::new(),
        media: MediaLocator::Http { url: format!("https://example.invalid/{id}.ts") },
        duration_ms: None, provider_id: None, provider_id_field: None, live: None }
}
fn playing() -> Playing {
    Playing { channel_capture: None, kiosk_selection: None, archive: None, transient_owner: None, local_selection: None,
        identity: None, profile: profile(), entry: entry("c"), selection_id: "c".into(), source: None,
        advance_on_failure: false, requires_server_lease: false }
}
fn source<'a>(profile: &'a ProviderProfile, entries: &'a [CatalogEntry]) -> CatalogSource<'a> {
    CatalogSource { profile, entries, transient_owner: None, source_current: true }
}

#[test]
fn absent_catalog_keeps_exact_captured_label_without_parsing_digits() {
    let mut playing = playing();
    playing.entry.title = "24  News HD".into();
    let value = project(&playing, None);
    assert_eq!(value.label, "24  News HD");
    assert_eq!(value.ordinal, None);
}

#[test]
fn ordinal_is_raw_catalog_index_not_category_position_or_display_text() {
    let playing = playing();
    let mut entries = vec![entry("a"), entry("b"), entry("c")];
    entries[0].category = "Other".into();
    entries[1].category = "Other".into();
    entries[2].title = "999  Changed display title".into();
    let value = project(&playing, Some(source(&playing.profile, &entries)));
    assert_eq!(value.ordinal, Some(3));
    assert_eq!(value.label, "Title c");
}

#[test]
fn metadata_enrichment_does_not_replace_label_or_remove_number() {
    let playing = playing();
    let mut entries = vec![entry("c")];
    entries[0].description = "A new synopsis".into();
    entries[0].poster = "https://example.invalid/poster.png".into();
    entries[0].duration_ms = Some(90_000);
    let value = project(&playing, Some(source(&playing.profile, &entries)));
    assert_eq!(value.ordinal, Some(1));
    assert_eq!(value.label, playing.entry.title);
}

#[test]
fn reused_entry_id_from_another_profile_or_changed_source_has_no_number() {
    let playing = playing();
    let entries = vec![entry("c")];
    let mut other = playing.profile.clone();
    other.id = "source-b".into();
    assert_eq!(project(&playing, Some(source(&other, &entries))).ordinal, None);
    other = playing.profile.clone();
    other.source = "https://example.invalid/replacement.m3u".into();
    assert_eq!(project(&playing, Some(source(&other, &entries))).ordinal, None);
    other = playing.profile.clone();
    other.source_alias = "replacement".into();
    assert_eq!(project(&playing, Some(source(&other, &entries))).ordinal, None);
}

#[test]
fn reused_id_with_another_media_locator_or_live_identity_has_no_number() {
    let playing = playing();
    let mut entries = vec![entry("c")];
    entries[0].media = entry("replacement").media;
    assert_eq!(project(&playing, Some(source(&playing.profile, &entries))).ordinal, None);
    entries[0] = entry("c");
    entries[0].live = Some(plx_data::ott::guide::ChannelMetadata::default());
    assert_eq!(project(&playing, Some(source(&playing.profile, &entries))).ordinal, None);
}

#[test]
fn duplicate_or_removed_id_does_not_guess_a_number() {
    let playing = playing();
    let entries = vec![entry("c"), entry("c")];
    assert_eq!(project(&playing, Some(source(&playing.profile, &entries))).ordinal, None);
    let entries = vec![entry("a")];
    assert_eq!(project(&playing, Some(source(&playing.profile, &entries))).ordinal, None);
}

#[test]
fn unavailable_or_disabled_source_cannot_supply_a_number() {
    let playing = playing();
    let entries = vec![entry("c")];
    let mut current = source(&playing.profile, &entries);
    current.source_current = false;
    assert_eq!(project(&playing, Some(current)).ordinal, None);
    let mut playing = playing;
    playing.profile.enabled = false;
    assert_eq!(project(&playing, Some(source(&playing.profile, &entries))).ordinal, None);
}

#[test]
fn transient_owner_must_match_even_when_profile_and_item_are_identical() {
    let mut playing = playing();
    let entries = vec![entry("c")];
    playing.transient_owner = Some(OwnerToken(10));
    assert_eq!(project(&playing, Some(source(&playing.profile, &entries))).ordinal, None);
    let mut current = source(&playing.profile, &entries);
    current.transient_owner = Some(OwnerToken(11));
    assert_eq!(project(&playing, Some(current)).ordinal, None);
    let mut current = source(&playing.profile, &entries);
    current.transient_owner = Some(OwnerToken(10));
    assert_eq!(project(&playing, Some(current)).ordinal, Some(1));
}

#[test]
fn new_transient_catalog_cannot_number_an_ordinary_playback() {
    let playing = playing();
    let entries = vec![entry("c")];
    let mut current = source(&playing.profile, &entries);
    current.transient_owner = Some(OwnerToken(10));
    assert_eq!(project(&playing, Some(current)).ordinal, None);
}

#[test]
fn favorite_gets_only_proven_underlying_catalog_number_never_favorite_rank() {
    let mut playing = playing();
    playing.selection_id = "favorite:opaque-locator".into();
    let entries = vec![entry("a"), entry("b"), entry("c")];
    assert_eq!(project(&playing, None).ordinal, None);
    let value = project(&playing, Some(source(&playing.profile, &entries)));
    assert_eq!(value.ordinal, Some(3));
    assert_eq!(value.label, "Title c");
    let mut other = profile(); other.id = "currently-browsed-other-profile".into();
    assert_eq!(project(&playing, Some(source(&other, &entries))).ordinal, None);
}

#[test]
fn revoked_local_authority_keeps_captured_label_without_lending_its_number() {
    let mut playing = playing();
    playing.profile.kind = ProviderKind::Local;
    playing.profile.source = "/media/films".into();
    playing.entry.media = MediaLocator::Local { relative_path: "movie.mp4".into() };
    playing.transient_owner = Some(OwnerToken(4));
    let entries = vec![playing.entry.clone()];
    let mut current = source(&playing.profile, &entries);
    current.transient_owner = Some(OwnerToken(4));
    current.source_current = false;
    let value = project(&playing, Some(current));
    assert_eq!(value.label, "Title c");
    assert_eq!(value.ordinal, None);
}

#[test]
fn one_based_number_is_checked_at_wire_width() {
    assert_eq!(one_based(0), Some(1));
    assert_eq!(one_based(u32::MAX as usize - 1), Some(u32::MAX));
    assert_eq!(one_based(u32::MAX as usize), None);
}
