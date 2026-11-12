use super::*;
use plx_data::ott::{parse_m3u, CatalogEntry};
use serde_json::json;

fn profile() -> ProviderProfile {
    ProviderProfile { id: "m3u-1".into(), name: "Home".into(), kind: ProviderKind::M3u,
        source: "https://playlist.invalid/one.m3u?key=private".into(),
        source_alias: String::new(), enabled: true }
}
fn catalog(profile: &ProviderProfile, items: &[(&str, &str)]) -> Catalog {
    let mut input = String::from("#EXTM3U\n");
    for (title, group) in items {
        input.push_str(&format!("#EXTINF:-1 tvg-id=\"{title}\" group-title=\"{group}\",{title}\nhttps://media.invalid/{title}.m3u8\n"));
    }
    parse_m3u(&input, profile).unwrap()
}
fn fixture() -> (Source, Catalog, Document) {
    let profile = profile();
    (Source::new(&profile).unwrap(), catalog(&profile, &[("One", "News"),
        ("Two", "News"), ("Three", "News"), ("Four", "Cinema")]), Document::default())
}
fn apply(document: &mut Document, source: &Source, catalog: &Catalog, op: Operation) -> Change {
    document.apply(source, catalog, op).unwrap()
}
fn group(document: &Document, source: &Source, catalog: &Catalog, label: &str) -> Category {
    document.project(source, catalog).unwrap().categories.into_iter()
        .find(|group| group.label == label).unwrap()
}
fn item(catalog: &Catalog, title: &str) -> String {
    catalog.entries.iter().find(|entry| entry.title == title).unwrap().id.clone()
}

#[test]
fn empty_document_projects_raw_order_and_unclassified_channels_without_writing() {
    let (source, mut catalog, document) = fixture();
    let mut unclassified = catalog.entries[0].clone();
    unclassified.id = "m3u-1:ffffffffffffffff".into();
    unclassified.category.clear();
    catalog.entries.push(unclassified);
    let projection = document.project(&source, &catalog).unwrap();
    assert_eq!(projection.all, catalog.entries.iter().map(|entry| entry.id.clone()).collect::<Vec<_>>());
    assert_eq!(projection.categories.iter().map(|group| group.label.as_str()).collect::<Vec<_>>(), ["News", "Cinema"]);
    assert_eq!(document, Document::default());
    assert_eq!(projection.ordered_ids(""), &projection.all);
}

#[test]
fn create_empty_copy_rename_delete_and_reset_preserve_raw_catalog() {
    let (source, catalog, mut document) = fixture();
    let before = catalog.clone();
    let news = group(&document, &source, &catalog, "News");
    let empty = apply(&mut document, &source, &catalog, Operation::Create { label: " Empty ".into() }).category.unwrap();
    assert!(document.project(&source, &catalog).unwrap().category(&empty).unwrap().members.is_empty());
    let copy = apply(&mut document, &source, &catalog, Operation::Copy { category: news.id.clone(), label: "Copy".into() }).category.unwrap();
    assert_eq!(document.project(&source, &catalog).unwrap().category(&copy).unwrap().members, news.members);
    apply(&mut document, &source, &catalog, Operation::Rename { category: copy.clone(), label: "Evening".into() });
    assert_eq!(document.project(&source, &catalog).unwrap().category(&copy).unwrap().label, "Evening");
    apply(&mut document, &source, &catalog, Operation::Delete { category: news.id.clone() });
    assert!(document.project(&source, &catalog).unwrap().category(&news.id).is_none());
    assert_eq!(document.project(&source, &catalog).unwrap().all, before.entries.iter().map(|entry| entry.id.clone()).collect::<Vec<_>>());
    apply(&mut document, &source, &catalog, Operation::Reset);
    assert_eq!(group(&document, &source, &catalog, "News").id, news.id);
    assert!(document.project(&source, &catalog).unwrap().category(&copy).is_none());
    assert_eq!(catalog, before);
}

#[test]
fn renamed_provider_inherits_new_channels_copy_remains_a_snapshot() {
    let (source, catalog, mut document) = fixture();
    let news = group(&document, &source, &catalog, "News");
    let copy = apply(&mut document, &source, &catalog, Operation::Copy { category: news.id.clone(), label: "Copy".into() }).category.unwrap();
    apply(&mut document, &source, &catalog, Operation::Rename { category: news.id.clone(), label: "My news".into() });
    let mut updated = catalog.clone();
    let extra = catalog_entry("m3u-1:eeeeeeeeeeeeeeee", "Five", "News");
    updated.entries.push(extra.clone());
    let projection = document.project(&source, &updated).unwrap();
    assert_eq!(projection.category(&news.id).unwrap().members.len(), 4);
    assert!(projection.category(&news.id).unwrap().members.contains(&extra.id));
    assert_eq!(projection.category(&copy).unwrap().members, news.members);
    assert_eq!(projection.selection_id("News"), Some(news.id.as_str()));
    assert_eq!(projection.selection_id("My news"), None);
    assert_eq!(projection.categories[0].id, news.id, "rename does not reorder categories");
}

fn catalog_entry(id: &str, title: &str, category: &str) -> CatalogEntry {
    CatalogEntry { protection: None, synopsis: None, live: None, id: id.into(), title: title.into(), category: category.into(),
        description: String::new(), poster: String::new(), duration_ms: None,
        media: MediaLocator::Http { url: "https://media.invalid/video.m3u8".into() },
        provider_id: None, provider_id_field: None }
}

#[test]
fn removed_known_channels_stay_removed_and_new_channels_append() {
    let (source, catalog, mut document) = fixture();
    let news = group(&document, &source, &catalog, "News");
    let removed = item(&catalog, "Two");
    apply(&mut document, &source, &catalog, Operation::RemoveMember { category: news.id.clone(), item: removed.clone() });
    let mut updated = catalog.clone();
    updated.entries.reverse();
    let extra = catalog_entry("m3u-1:eeeeeeeeeeeeeeee", "Five", "News");
    updated.entries.push(extra.clone());
    let members = group(&document, &source, &updated, "News").members;
    assert_eq!(members, [item(&catalog, "One"), item(&catalog, "Three"), extra.id]);
    assert!(document.project(&source, &updated).unwrap().all.contains(&removed));
}

#[test]
fn edited_snapshot_survives_provider_moving_every_member_to_another_category() {
    let mut p = profile();
    p.kind = ProviderKind::Xtream;
    p.source = r#"{"server":"https://tv.invalid","username":"u","password":"p"}"#.into();
    let config = plx_data::ott::xtream::Config::parse(&p.source).unwrap();
    let source = Source::new(&p).unwrap();
    let rows = json!([
        {"stream_id":1,"name":"One","category_id":"9"},
        {"stream_id":2,"name":"Two","category_id":"9"},
        {"stream_id":3,"name":"Three","category_id":"9"}
    ]);
    let original = plx_data::ott::xtream::streams(&rows, &p, &config, "live",
        &BTreeMap::from([("9".into(), "News".into())])).unwrap();
    let updated = plx_data::ott::xtream::streams(&rows, &p, &config, "live",
        &BTreeMap::from([("9".into(), "New provider grouping".into())])).unwrap();
    let mut document = Document::default();
    let news = group(&document, &source, &original, "News");
    apply(&mut document, &source, &original, Operation::RemoveMember {
        category: news.id.clone(), item: item(&original, "Two") });
    assert_eq!(group(&document, &source, &updated, "News").members,
        [item(&original, "One"), item(&original, "Three")]);
    assert_eq!(document.project(&source, &updated).unwrap().selection_id("News"), Some(news.id.as_str()));
}

#[test]
fn absent_members_stay_dormant_through_later_edit_copy_and_refresh() {
    let (source, catalog, mut document) = fixture();
    let news = group(&document, &source, &catalog, "News");
    apply(&mut document, &source, &catalog, Operation::RemoveMember { category: news.id.clone(), item: item(&catalog, "Two") });
    let mut partial = catalog.clone();
    partial.entries.retain(|entry| entry.title != "One");
    apply(&mut document, &source, &partial, Operation::AddMember { category: news.id.clone(), item: item(&catalog, "Four") });
    let copy = apply(&mut document, &source, &partial, Operation::Copy { category: news.id.clone(), label: "Snapshot".into() }).category.unwrap();
    assert_eq!(group(&document, &source, &partial, "News").members, [item(&catalog, "Three"), item(&catalog, "Four")]);
    let restored = document.project(&source, &catalog).unwrap();
    assert_eq!(restored.category(&news.id).unwrap().members,
        [item(&catalog, "One"), item(&catalog, "Three"), item(&catalog, "Four")]);
    assert_eq!(restored.category(&news.id).unwrap().members, restored.category(&copy).unwrap().members);
}

#[test]
fn add_remove_are_idempotent_and_do_not_change_other_memberships() {
    let (source, catalog, mut document) = fixture();
    let news = group(&document, &source, &catalog, "News");
    let four = item(&catalog, "Four");
    let op = Operation::AddMember { category: news.id.clone(), item: four.clone() };
    assert!(apply(&mut document, &source, &catalog, op.clone()).changed);
    let saved = document.clone();
    assert!(!apply(&mut document, &source, &catalog, op).changed);
    assert_eq!(document, saved);
    assert_eq!(group(&document, &source, &catalog, "Cinema").members, [four.clone()]);
    let op = Operation::RemoveMember { category: news.id, item: four };
    apply(&mut document, &source, &catalog, op.clone());
    assert!(!apply(&mut document, &source, &catalog, op).changed);
}

#[test]
fn member_and_category_move_wrap_without_swapping_intermediate_rows() {
    let (source, catalog, mut document) = fixture();
    let news = group(&document, &source, &catalog, "News");
    apply(&mut document, &source, &catalog, Operation::MoveMember { category: news.id.clone(), item: item(&catalog, "One"), direction: Direction::Up });
    assert_eq!(group(&document, &source, &catalog, "News").members,
        [item(&catalog, "Two"), item(&catalog, "Three"), item(&catalog, "One")]);
    apply(&mut document, &source, &catalog, Operation::MoveMember { category: news.id.clone(), item: item(&catalog, "One"), direction: Direction::Down });
    assert_eq!(group(&document, &source, &catalog, "News").members, news.members);
    let custom = apply(&mut document, &source, &catalog, Operation::Create { label: "Custom".into() }).category.unwrap();
    apply(&mut document, &source, &catalog, Operation::MoveCategory { category: news.id.clone(), direction: Direction::Up });
    assert_eq!(document.project(&source, &catalog).unwrap().categories.iter().map(|group| group.id.clone()).collect::<Vec<_>>(),
        [provider_id("Cinema"), custom, news.id]);
}

#[test]
fn source_profile_and_account_scope_are_distinct_but_display_rename_is_not() {
    let (source, catalog, mut document) = fixture();
    apply(&mut document, &source, &catalog, Operation::Create { label: "Private group".into() });
    let mut renamed = profile();
    renamed.name = "Renamed".into();
    renamed.source_alias = "Label only".into();
    assert_eq!(Source::new(&renamed).unwrap(), source);
    let mut other = profile();
    other.source = "https://playlist.invalid/other.m3u".into();
    let other_source = Source::new(&other).unwrap();
    assert_ne!(other_source, source);
    assert_eq!(document.project(&other_source, &catalog).unwrap().categories.len(), 2);
    other.id = "m3u-2".into();
    let other_catalog = super::tests::catalog(&other, &[("One", "News")]);
    let other_source = Source::new(&other).unwrap();
    apply(&mut document, &other_source, &other_catalog, Operation::Create { label: "Other group".into() });
    apply(&mut document, &other_source, &other_catalog, Operation::Reset);
    assert_eq!(document.project(&source, &catalog).unwrap().categories.len(), 3);
}

#[test]
fn changed_source_replaces_one_slot_only_and_roundtrip_contains_no_private_urls() {
    let (source, catalog, mut document) = fixture();
    apply(&mut document, &source, &catalog, Operation::Create { label: "Old".into() });
    let mut changed = profile();
    changed.source = "https://playlist.invalid/new.m3u?key=another-secret".into();
    let replacement = Source::new(&changed).unwrap();
    apply(&mut document, &replacement, &catalog, Operation::Create { label: "New".into() });
    assert_eq!(document.profiles.len(), 1);
    assert_eq!(document.project(&source, &catalog).unwrap().categories.len(), 2);
    let mut settings = BTreeMap::from([("unrelated".into(), json!(true))]);
    document.write_into(&mut settings).unwrap();
    assert_eq!(Document::read(&settings).unwrap(), document);
    assert_eq!(settings["unrelated"], true);
    let encoded = serde_json::to_string(&settings[KEY]).unwrap();
    assert!(!encoded.contains("https://"));
    assert!(!encoded.contains("private"));
    assert!(!encoded.contains("another-secret"));
}

#[test]
fn raw_provider_reorder_and_rotated_token_keep_same_membership_identity() {
    let profile = profile();
    let input = "#EXTM3U\n#EXTINF:-1 tvg-id=\"one\" group-title=\"News\",One\nhttps://media.invalid/one.m3u8?token=first\n#EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Two\nhttps://media.invalid/two.m3u8\n";
    let original = parse_m3u(input, &profile).unwrap();
    let mut next = parse_m3u(&input.replace("token=first", "token=second"), &profile).unwrap();
    next.entries.reverse();
    let source = Source::new(&profile).unwrap();
    let mut document = Document::default();
    apply(&mut document, &source, &original, Operation::RemoveMember {
        category: provider_id("News"), item: item(&original, "One") });
    assert_eq!(group(&document, &source, &next, "News").members, [item(&original, "Two")]);
    assert_eq!(next.entries[1].id, original.entries[0].id);
}

#[test]
fn system_missing_foreign_and_ambiguous_targets_fail_atomically() {
    let (source, catalog, mut document) = fixture();
    let news = provider_id("News");
    for operation in [Operation::Delete { category: "".into() },
        Operation::Rename { category: "favorites".into(), label: "No".into() },
        Operation::Copy { category: "missing".into(), label: "No".into() },
        Operation::AddMember { category: news, item: "m3u-2:0000000000000001".into() }]
    {
        assert!(document.apply(&source, &catalog, operation).is_err());
        assert_eq!(document, Document::default());
    }
    let mut duplicate = catalog.clone();
    duplicate.entries.push(catalog.entries[0].clone());
    assert_eq!(document.project(&source, &duplicate), Err(Error::IdentityCollision));
    let mut foreign = catalog.clone();
    foreign.entries[0].id = "m3u-2:0000000000000001".into();
    assert_eq!(document.project(&source, &foreign), Err(Error::Invalid));
}

#[test]
fn local_logical_categories_allow_creation_but_disabled_profiles_and_mixed_rows_remain_restricted() {
    let mut p = profile();
    p.kind = ProviderKind::Local;
    p.source = "/fixture/videos".into();
    let local = Source::new(&p).unwrap();
    assert!(Document::default().apply(&local, &Catalog::default(),
        Operation::Create { label: "Logical category".into() }).is_ok());
    p.kind = ProviderKind::Plex;
    p.source = "fixture-server".into();
    assert!(Source::new(&p).is_ok());
    p = profile();
    p.enabled = false;
    assert_eq!(Source::new(&p), Err(Error::UnsupportedSource));
    let (source, mut catalog, mut document) = fixture();
    catalog.entries[0].media = MediaLocator::VPortal { request: json!({"id":1}) };
    assert_eq!(document.apply(&source, &catalog, Operation::Create { label: "No".into() }), Err(Error::UnsupportedSource));
    assert_eq!(document, Document::default());
}

#[test]
fn xtream_and_stalker_typed_rows_share_the_same_projection_without_http_assumptions() {
    for (kind, source_value, media) in [
        (ProviderKind::Xtream, r#"{"server":"https://tv.invalid","username":"u","password":"p"}"#,
            MediaLocator::Xtream { stream_id: "1".into(), stream_kind: "live".into(), extension: "ts".into(), series_id: None, has_direct_source: false }),
        (ProviderKind::Stalker, r#"{"portal":"https://tv.invalid","mac":"00:1A:79:00:00:01"}"#,
            MediaLocator::Stalker { cmd: "ffmpeg http://localhost/ch/1".into(), stream_kind: "itv".into(), series: "0".into() })]
    {
        let mut p = profile(); p.kind = kind; p.source = source_value.into();
        let source = Source::new(&p).unwrap();
        let mut catalog = Catalog { hierarchy: None, entries: vec![catalog_entry("m3u-1:0000000000000001", "One", "News")] };
        catalog.entries[0].media = media;
        let mut document = Document::default();
        let copy = apply(&mut document, &source, &catalog, Operation::Copy { category: provider_id("News"), label: "Mine".into() }).category.unwrap();
        assert_eq!(document.project(&source, &catalog).unwrap().category(&copy).unwrap().members, [catalog.entries[0].id.clone()]);
    }
}

#[test]
fn invalid_names_and_duplicate_visible_labels_do_not_partially_create_state() {
    let (source, catalog, mut document) = fixture();
    for label in ["".to_string(), "  ".into(), "A\nB".into(), "é".repeat(257), "News".into()] {
        assert!(document.apply(&source, &catalog, Operation::Create { label }).is_err());
        assert_eq!(document, Document::default());
    }
    let valid = "é".repeat(256);
    apply(&mut document, &source, &catalog, Operation::Create { label: valid.clone() });
    assert_eq!(group(&document, &source, &catalog, &valid).label, valid);
}

#[test]
fn malformed_unknown_version_foreign_member_and_duplicate_ids_are_not_silently_replaced() {
    let (source, catalog, mut document) = fixture();
    apply(&mut document, &source, &catalog, Operation::Copy { category: provider_id("News"), label: "Copy".into() });
    let original = serde_json::to_value(&document).unwrap();
    let mut variants = vec![json!(null), json!({"version":2,"profiles":{}}), json!({"version":1,"profiles":{},"extra":true})];
    let mut duplicate = original.clone();
    let group = duplicate["profiles"]["m3u-1"]["groups"][0].clone();
    duplicate["profiles"]["m3u-1"]["groups"].as_array_mut().unwrap().push(group);
    variants.push(duplicate);
    let mut foreign = original.clone();
    foreign["profiles"]["m3u-1"]["groups"][0]["members"]["members"][0] = json!("m3u-2:0000000000000001");
    variants.push(foreign);
    for value in variants {
        let settings = BTreeMap::from([(KEY.into(), value)]);
        let before = settings.clone();
        assert!(Document::read(&settings).is_err());
        assert_eq!(settings, before);
    }
}

#[test]
fn oversized_first_member_snapshot_rejects_without_partial_overlay() {
    let (source, _, mut document) = fixture();
    let catalog = Catalog { hierarchy: None, entries: (0..20000).map(|index|
        catalog_entry(&format!("m3u-1:{index:016x}"), "Channel", "Large")).collect() };
    let result = document.apply(&source, &catalog, Operation::RemoveMember {
        category: provider_id("Large"), item: catalog.entries[0].id.clone() });
    assert_eq!(result, Err(Error::TooLarge));
    assert_eq!(document, Document::default());
    assert_eq!(document.project(&source, &catalog).unwrap().all.len(), 20000, "storage refusal must not truncate the catalog");
}

#[test]
fn serialized_document_limit_is_atomic_even_when_each_individual_copy_fits() {
    let (source, _, mut document) = fixture();
    let catalog = Catalog { hierarchy: None, entries: (0..7000).map(|index|
        catalog_entry(&format!("m3u-1:{index:016x}"), "Channel", "Large")).collect() };
    apply(&mut document, &source, &catalog, Operation::Copy { category: provider_id("Large"), label: "First".into() });
    apply(&mut document, &source, &catalog, Operation::Copy { category: provider_id("Large"), label: "Second".into() });
    let before = document.clone();
    assert_eq!(document.apply(&source, &catalog, Operation::Copy { category: provider_id("Large"), label: "Third".into() }), Err(Error::TooLarge));
    assert_eq!(document, before);
}

#[test]
fn hidden_groups_and_deleted_custom_ids_do_not_reappear_or_get_reused() {
    let (source, catalog, mut document) = fixture();
    let first = apply(&mut document, &source, &catalog, Operation::Create { label: "Mine".into() }).category.unwrap();
    apply(&mut document, &source, &catalog, Operation::Delete { category: first.clone() });
    let second = apply(&mut document, &source, &catalog, Operation::Create { label: "Mine".into() }).category.unwrap();
    assert_ne!(first, second);
    apply(&mut document, &source, &catalog, Operation::Delete { category: provider_id("News") });
    let mut settings = BTreeMap::new(); document.write_into(&mut settings).unwrap();
    let loaded = Document::read(&settings).unwrap();
    assert!(loaded.project(&source, &catalog).unwrap().category(&provider_id("News")).is_none());
    assert!(loaded.project(&source, &catalog).unwrap().category(&second).is_some());
}

#[test]
fn category_names_that_look_like_system_or_private_ids_do_not_gain_authority() {
    let profile = profile();
    let source = Source::new(&profile).unwrap();
    let catalog = catalog(&profile, &[("One", "Favorites"), ("Two", "All"), ("Three", "organization:custom:1")]);
    let mut document = Document::default();
    let created = apply(&mut document, &source, &catalog, Operation::Create { label: "Mine".into() }).category.unwrap();
    let projection = document.project(&source, &catalog).unwrap();
    assert!(projection.category(&provider_id("Favorites")).is_some());
    assert!(projection.category(&provider_id("All")).is_some());
    assert_ne!(created, provider_id("organization:custom:1"));
    assert_eq!(projection.selection_id("organization:custom:1"), None, "ambiguous old-label/new-ID choice must not silently choose either");
}

#[test]
fn exhausted_custom_id_and_oversized_input_preserve_prior_state() {
    let (source, catalog, mut document) = fixture();
    apply(&mut document, &source, &catalog, Operation::Create { label: "First".into() });
    document.profiles.get_mut("m3u-1").unwrap().next_group = u64::MAX;
    let before = document.clone();
    assert_eq!(document.apply(&source, &catalog, Operation::Create { label: "Next".into() }), Err(Error::Exhausted));
    assert_eq!(document, before);
    let settings = BTreeMap::from([(KEY.into(), json!("x".repeat(ENCODED_BYTES)))]);
    assert_eq!(Document::read(&settings), Err(Error::TooLarge));
}

#[test]
fn no_op_in_new_source_does_not_erase_other_sources_dormant_organization() {
    let (source, catalog, mut document) = fixture();
    apply(&mut document, &source, &catalog, Operation::Create { label: "Original source".into() });
    let before = document.clone();
    let mut changed = profile();
    changed.source = "https://other.invalid/playlist.m3u".into();
    let replacement = Source::new(&changed).unwrap();
    let result = apply(&mut document, &replacement, &catalog, Operation::AddMember {
        category: provider_id("News"), item: item(&catalog, "One") });
    assert!(!result.changed);
    assert_eq!(document, before);
}

#[test]
fn deleted_profile_cleanup_requires_complete_validated_profiles_and_retains_disabled() {
    let (source, catalog, mut document) = fixture();
    apply(&mut document, &source, &catalog, Operation::Create { label: "Keep".into() });
    let before = document.clone();
    let invalid = ProviderProfiles { version: 2, active_profile: None, profiles: Vec::new() };
    assert_eq!(document.prune_deleted(&invalid), Err(Error::Invalid));
    assert_eq!(document, before);
    let mut disabled = profile(); disabled.enabled = false;
    let profiles = ProviderProfiles { version: 1, active_profile: None, profiles: vec![disabled] };
    assert_eq!(document.prune_deleted(&profiles).unwrap(), 0);
    assert_eq!(document, before);
    assert_eq!(document.prune_deleted(&ProviderProfiles::default()).unwrap(), 1);
    assert_eq!(document, Document::default());
}

#[test]
fn all_membership_is_editable_but_raw_add_picker_and_other_categories_remain_complete() {
    let (source, catalog, mut document) = fixture();
    let removed = item(&catalog, "One");
    apply(&mut document, &source, &catalog, Operation::RemoveMember { category: ALL_ID.into(), item: removed.clone() });
    let projection = document.project(&source, &catalog).unwrap();
    assert!(!projection.all.contains(&removed));
    assert!(projection.available.contains(&removed));
    assert!(projection.category(&provider_id("News")).unwrap().members.contains(&removed));
    assert_eq!(projection.ordered_ids(""), projection.all.as_slice());
    assert_eq!(projection.ordered_ids(ALL_ID), projection.all.as_slice());
    assert!(projection.category(ALL_ID).is_none());
    apply(&mut document, &source, &catalog, Operation::AddMember { category: "".into(), item: removed.clone() });
    assert_eq!(document.project(&source, &catalog).unwrap().all.last(), Some(&removed));
    let before = document.clone();
    assert!(!apply(&mut document, &source, &catalog, Operation::AddMember { category: ALL_ID.into(), item: removed }).changed);
    assert_eq!(document, before);
}

#[test]
fn all_order_wraps_and_edited_all_inherits_new_ids_without_resurrecting_removals() {
    let (source, catalog, mut document) = fixture();
    let first = item(&catalog, "One");
    apply(&mut document, &source, &catalog, Operation::MoveMember { category: ALL_ID.into(), item: first.clone(), direction: Direction::Up });
    assert_eq!(document.project(&source, &catalog).unwrap().all.last(), Some(&first));
    let removed = item(&catalog, "Two");
    apply(&mut document, &source, &catalog, Operation::RemoveMember { category: ALL_ID.into(), item: removed.clone() });
    let mut updated = catalog.clone();
    updated.entries.reverse();
    let extra = catalog_entry("m3u-1:eeeeeeeeeeeeeeee", "Five", "News");
    updated.entries.push(extra.clone());
    let projection = document.project(&source, &updated).unwrap();
    assert_eq!(projection.all, [item(&catalog, "Three"), item(&catalog, "Four"), first, extra.id]);
    assert!(projection.available.contains(&removed));
}

#[test]
fn all_metadata_is_protected_and_copy_all_is_an_independent_snapshot() {
    let (source, catalog, mut document) = fixture();
    for operation in [Operation::Rename { category: ALL_ID.into(), label: "Mine".into() },
        Operation::Delete { category: ALL_ID.into() },
        Operation::MoveCategory { category: ALL_ID.into(), direction: Direction::Down }]
    {
        assert!(document.apply(&source, &catalog, operation).is_err());
        assert_eq!(document, Document::default());
    }
    let copy = apply(&mut document, &source, &catalog, Operation::Copy { category: ALL_ID.into(), label: "Snapshot".into() }).category.unwrap();
    apply(&mut document, &source, &catalog, Operation::RemoveMember { category: ALL_ID.into(), item: item(&catalog, "One") });
    let projection = document.project(&source, &catalog).unwrap();
    assert_eq!(projection.category(&copy).unwrap().members, projection.available);
    assert_eq!(projection.all.len(), catalog.entries.len() - 1);
    apply(&mut document, &source, &catalog, Operation::Reset);
    let reset = document.project(&source, &catalog).unwrap();
    assert_eq!(reset.all, reset.available);
}

/// Actual adapter identity shapes, including VPortal direct URLs and requests.
pub(super) fn hierarchical_fixture(kind: ProviderKind) -> (ProviderProfile, Catalog) {
    use plx_data::ott::{CategoryHierarchy, CategoryMembership, CategoryNode};
    let mut p = profile();
    p.id = "provider:one".into();
    p.kind = kind;
    p.source = match kind {
        ProviderKind::Plex => "fixture-machine".into(),
        ProviderKind::VPortal => "portal::[key:private-key]https://portal.invalid/api".into(),
        _ => panic!("hierarchical provider fixture"),
    };
    if kind == ProviderKind::VPortal {
        let catalog = plx_data::ott::parse_vportal_response(r#"{"type":"videoportal","items":[{"type":"category","id":"root","title":"Library / literal","items":[{"type":"category","id":"child","title":"Season","items":[{"type":"stream","id":"episode-1","title":"One","request":{"id":1}},{"type":"stream","id":"episode-2","title":"Two","url":"/two"}]}]},{"type":"category","id":"other","title":"Library / literal","items":[]},{"type":"category","id":"empty","title":"Empty","items":[]}]}"#, &p, "").unwrap().catalog;
        return (p, catalog);
    }
    let entries = ["episode-1", "episode-2"].into_iter().map(|key| {
        let mut entry = catalog_entry(&format!("{}:{key}", p.id), key, "Library / literal / Season");
        entry.provider_id = Some(key.into());
        entry.media = MediaLocator::Plex { server_id: p.source.clone(), rating_key: key.into() };
        entry
    }).collect::<Vec<_>>();
    let node = |id: &str, parent: Option<&str>, label: &str| CategoryNode {
        id: id.into(), parent: parent.map(str::to_owned), label: label.into(), anonymous_fingerprint: None,
        source_identity: None,
    };
    let memberships = entries.iter().map(|entry| CategoryMembership {
        entry_id: entry.id.clone(), category_id: "plex:season:child".into(),
    }).collect();
    (p, Catalog { entries, hierarchy: Some(CategoryHierarchy {
        nodes: vec![node("plex:section:root", None, "Library / literal"),
            node("plex:season:child", Some("plex:section:root"), "Season"),
            node("plex:section:other", None, "Library / literal"),
            node("plex:section:empty", None, "Empty")], memberships,
    }) })
}

#[test]
fn hierarchical_sources_flatten_ancestors_empty_folders_and_duplicate_labels_by_identity() {
    for kind in [ProviderKind::Plex, ProviderKind::VPortal] {
        let (p, mut raw) = hierarchical_fixture(kind);
        // Direct and descendant membership must not duplicate a row in its ancestor.
        let hierarchy = raw.hierarchy.as_mut().unwrap();
        hierarchy.memberships.push(plx_data::ott::CategoryMembership {
            entry_id: raw.entries[0].id.clone(), category_id: hierarchy.nodes[0].id.clone(),
        });
        let before = raw.clone();
        let source = Source::new(&p).unwrap();
        let projection = Document::default().project(&source, &raw).unwrap();
        let hierarchy = raw.hierarchy.as_ref().unwrap();
        assert_eq!(projection.categories.len(), 4);
        let root = projection.selection_id(&hierarchy.nodes[0].id).unwrap();
        let child = projection.selection_id(&hierarchy.nodes[1].id).unwrap();
        let other = projection.selection_id(&hierarchy.nodes[2].id).unwrap();
        assert_ne!(root, other);
        assert_eq!(projection.ordered_ids(root), projection.all);
        assert_eq!(projection.ordered_ids(child), projection.all);
        assert!(projection.ordered_ids(other).is_empty());
        assert!(projection.ordered_ids("Empty").is_empty());
        assert_eq!(projection.selection_id("Library / literal"), None);
        assert_eq!(projection.selection_id("Library / literal / Season"), Some(child));
        assert_eq!(projection.all, raw.entries.iter().map(|entry| entry.id.clone()).collect::<Vec<_>>());
        assert_eq!(raw, before);
    }
}

#[test]
fn structural_edits_roundtrip_survive_labels_and_remain_in_the_persisted_profile() {
    for kind in [ProviderKind::Plex, ProviderKind::VPortal] {
        let (p, raw) = hierarchical_fixture(kind);
        let source = Source::new(&p).unwrap();
        let key = raw.hierarchy.as_ref().unwrap().nodes[1].id.clone();
        let mut doc = Document::default();
        let id = doc.project(&source, &raw).unwrap().selection_id(&key).unwrap().to_owned();
        apply(&mut doc, &source, &raw, Operation::Rename { category: id.clone(), label: "Mine".into() });
        apply(&mut doc, &source, &raw, Operation::RemoveMember { category: id.clone(), item: raw.entries[0].id.clone() });
        let mut settings = BTreeMap::from([("portableFavorites".into(), json!(["untouched"]))]);
        doc.write_into(&mut settings).unwrap();
        assert_eq!(settings["portableFavorites"], json!(["untouched"]));
        assert_eq!(settings[KEY]["version"], 1);
        let loaded = Document::read(&settings).unwrap();
        let mut renamed = raw.clone();
        renamed.hierarchy.as_mut().unwrap().nodes[0].label = "Provider rename".into();
        renamed.hierarchy.as_mut().unwrap().nodes[1].label = "Different season label".into();
        renamed.entries.reverse();
        let projection = loaded.project(&source, &renamed).unwrap();
        assert_eq!(projection.selection_id(&key), Some(id.as_str()));
        assert_eq!(projection.category(&id).unwrap().label, "Mine");
        assert_eq!(projection.ordered_ids(&id), [raw.entries[1].id.clone()]);
        let mut renamed_profile = p.clone();
        renamed_profile.name = "Profile display rename".into();
        renamed_profile.source_alias = "New alias".into();
        assert_eq!(Source::new(&renamed_profile).unwrap(), source);
        let mut other_profile = p.clone();
        other_profile.id = "other-profile".into();
        assert!(loaded.overlay(&Source::new(&other_profile).unwrap()).is_none());
        other_profile = p.clone();
        other_profile.source = other_profile.source.replace("fixture-machine", "other-machine")
            .replace("private-key", "other-key");
        assert!(loaded.overlay(&Source::new(&other_profile).unwrap()).is_none());
        let encoded = serde_json::to_string(&settings[KEY]).unwrap();
        assert!(!encoded.contains("private-key"));
        assert!(!encoded.contains("portal.invalid"));
        assert_eq!(raw.entries.len(), 2, "organization does not create Favorites rows");
    }
}

#[test]
fn plex_long_rating_keys_are_preserved_but_foreign_or_mismatched_locators_are_rejected() {
    let (p, mut raw) = hierarchical_fixture(ProviderKind::Plex);
    let source = Source::new(&p).unwrap();
    raw.hierarchy = None;
    raw.entries.truncate(1);
    let key = "a".repeat(256);
    raw.entries[0].id = format!("{}:{key}", p.id);
    raw.entries[0].media = MediaLocator::Plex { server_id: p.source.clone(), rating_key: key.clone() };
    let mut doc = Document::default();
    apply(&mut doc, &source, &raw, Operation::Copy { category: provider_id(&raw.entries[0].category), label: "Long key".into() });
    let mut settings = BTreeMap::new();
    doc.write_into(&mut settings).unwrap();
    assert_eq!(Document::read(&settings).unwrap().project(&source, &raw).unwrap().all, [raw.entries[0].id.clone()]);
    for (server_id, rating_key) in [("other-machine".into(), key.clone()), (p.source.clone(), "different-key".into()),
        (p.source.clone(), format!("{key}a"))] {
        raw.entries[0].media = MediaLocator::Plex { server_id, rating_key };
        assert_eq!(doc.project(&source, &raw), Err(Error::Invalid));
    }
}

#[test]
fn malformed_hierarchy_does_not_silently_become_an_empty_organization() {
    let (p, raw) = hierarchical_fixture(ProviderKind::Plex);
    let source = Source::new(&p).unwrap();
    let mut doc = Document::default();
    for mutation in 0..3 {
        let mut invalid = raw.clone();
        let hierarchy = invalid.hierarchy.as_mut().unwrap();
        match mutation {
            0 => hierarchy.nodes[0].parent = Some(hierarchy.nodes[1].id.clone()),
            1 => hierarchy.memberships[0].entry_id = "foreign:entry".into(),
            _ => hierarchy.nodes.push(hierarchy.nodes[0].clone()),
        }
        assert!(doc.apply(&source, &invalid, Operation::Create { label: "No".into() }).is_err());
        assert_eq!(doc, Document::default());
    }
}

#[test]
fn version_one_legacy_groups_need_no_provider_key_and_structural_keys_are_validated() {
    let (source, raw, mut doc) = fixture();
    apply(&mut doc, &source, &raw, Operation::Rename { category: provider_id("News"), label: "Mine".into() });
    let value = serde_json::to_value(&doc).unwrap();
    assert!(value["profiles"]["m3u-1"]["groups"][0].get("provider_key").is_none());
    assert_eq!(Document::read(&BTreeMap::from([(KEY.into(), value)])).unwrap(), doc);

    let (p, raw) = hierarchical_fixture(ProviderKind::Plex);
    let source = Source::new(&p).unwrap();
    let mut doc = Document::default();
    let key = &raw.hierarchy.as_ref().unwrap().nodes[0].id;
    let id = doc.project(&source, &raw).unwrap().selection_id(key).unwrap().to_owned();
    apply(&mut doc, &source, &raw, Operation::Rename { category: id, label: "Library".into() });
    let mut value = serde_json::to_value(&doc).unwrap();
    assert_eq!(value["profiles"][&p.id]["groups"][0]["provider_key"], key.as_str());
    value["profiles"][&p.id]["groups"][0]["provider_key"] = json!("a different source key");
    assert_eq!(Document::read(&BTreeMap::from([(KEY.into(), value)])), Err(Error::Invalid));
}


#[test]
fn structural_projection_refuses_duplicate_memberships_after_a_valid_projection() {
    let (profile, mut catalog) = hierarchical_fixture(ProviderKind::VPortal);
    let source = Source::new(&profile).unwrap();
    let mut document = Document::default();
    assert!(document.project(&source, &catalog).is_ok());
    let graph = catalog.hierarchy.as_mut().unwrap();
    graph.memberships.push(graph.memberships[0].clone());
    let before = catalog.clone();
    assert_eq!(document.project(&source, &catalog), Err(Error::Invalid));
    assert_eq!(document.apply(&source, &catalog, Operation::Create { label: "No".into() }), Err(Error::Invalid));
    assert_eq!(document, Document::default());
    assert_eq!(catalog, before);
}

#[test]
fn structural_projection_preserves_portal_payload_budget_without_flat_policy_change() {
    let (profile, mut catalog) = hierarchical_fixture(ProviderKind::VPortal);
    let source = Source::new(&profile).unwrap();
    let document = Document::default();
    assert!(document.project(&source, &catalog).is_ok());
    catalog.entries[0].media = MediaLocator::VPortal {
        request: json!({"payload": "x".repeat(plx_data::ott::MAX_CATALOG_MODEL_BYTES)}),
    };
    assert_eq!(document.project(&source, &catalog), Err(Error::Invalid));
    // The pre-existing flat organization path only validates its row/source
    // identities; this change must not silently add a new flat-model policy.
    catalog.hierarchy = None;
    assert!(document.project(&source, &catalog).is_ok());
}
