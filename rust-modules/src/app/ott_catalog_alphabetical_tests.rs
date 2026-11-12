use super::*;
use plx_data::ott::CatalogEntry;
use serde_json::json;

fn fixture(kind: ProviderKind, titles: &[&str]) -> (ProviderProfile, Catalog) {
    let source = match kind {
        ProviderKind::M3u => "https://playlist.invalid/list.m3u",
        ProviderKind::Edem | ProviderKind::Operator(_) => r#"{"key":"fixture"}"#,
        ProviderKind::Xtream => r#"{"server":"https://tv.invalid","username":"u","password":"p"}"#,
        ProviderKind::Stalker => r#"{"portal":"https://tv.invalid","mac":"00:1A:79:00:00:01"}"#,
        ProviderKind::VPortal => "portal::[key:private]https://portal.invalid/api",
        ProviderKind::Local => "/fixture/videos",
        ProviderKind::Plex => "fixture-machine",
    };
    let profile = ProviderProfile { id: "alphabetical-source".into(), name: "Source".into(), kind,
        source: source.into(), source_alias: String::new(), enabled: true };
    let entries = titles.iter().enumerate().map(|(index, title)| {
        let key = format!("{:016x}", index + 1);
        let media = match kind {
            ProviderKind::M3u | ProviderKind::Edem | ProviderKind::Operator(_) => MediaLocator::Http { url: format!("https://media.invalid/{index}") },
            ProviderKind::Xtream => MediaLocator::Xtream { stream_id: key.clone(), stream_kind: "live".into(),
                extension: "ts".into(), series_id: None, has_direct_source: false },
            ProviderKind::Stalker => MediaLocator::Stalker { cmd: format!("ffmpeg http://localhost/ch/{index}"),
                stream_kind: "itv".into(), series: "0".into() },
            ProviderKind::VPortal => MediaLocator::VPortal { request: json!({"id":index + 1}) },
            ProviderKind::Local => MediaLocator::Local { relative_path: format!("video-{index}.mp4") },
            ProviderKind::Plex => MediaLocator::Plex { server_id: profile.source.clone(), rating_key: key.clone() },
        };
        CatalogEntry { protection: None, synopsis: None, id: format!("{}:{key}", profile.id), title: (*title).into(),
            category: if index < 2 { "Z first" } else { "A second" }.into(),
            description: String::new(), poster: String::new(), media, duration_ms: None,
            provider_id: Some(key), provider_id_field: None, live: None }
    }).collect();
    (profile, Catalog { entries, hierarchy: None })
}
fn kinds() -> [ProviderKind; 6] {
    [ProviderKind::M3u, ProviderKind::Xtream, ProviderKind::Stalker,
        ProviderKind::VPortal, ProviderKind::Local, ProviderKind::Plex]
}
fn titles(ids: &[String], catalog: &Catalog) -> Vec<String> {
    ids.iter().map(|id| catalog.entries.iter().find(|entry| entry.id == *id).unwrap().title.clone()).collect()
}
fn set(doc: &mut Document, source: &Source, catalog: &Catalog, enabled: bool) -> Change {
    doc.apply(source, catalog, Operation::SetAlphabetical { enabled }).unwrap()
}

#[test]
fn alphabetical_only_lifecycle_is_idempotent_for_every_provider_and_restores_as_is() {
    for kind in kinds() {
        let (profile, raw) = fixture(kind, &["C", "A", "B"]);
        let source = Source::new(&profile).unwrap();
        let before = raw.clone();
        let mut doc = Document::default();
        assert!(!doc.alphabetical(&source));
        assert!(!set(&mut doc, &source, &raw, false).changed);
        assert_eq!(doc, Document::default());
        assert!(set(&mut doc, &source, &raw, true).changed);
        assert!(doc.alphabetical(&source));
        assert!(!set(&mut doc, &source, &raw, true).changed);
        let projection = doc.project(&source, &raw).unwrap();
        assert_eq!(titles(&projection.all, &raw), ["A", "B", "C"]);
        assert_eq!(titles(&projection.available, &raw), ["A", "B", "C"]);
        assert_eq!(projection.categories.iter().map(|group| group.label.as_str()).collect::<Vec<_>>(),
            ["Z first", "A second"], "alphabetical never sorts the category list itself");
        assert_eq!(titles(&projection.categories[0].members, &raw), ["A", "C"]);
        let mut settings = BTreeMap::new();
        doc.write_into(&mut settings).unwrap();
        assert_eq!(settings[KEY]["version"], 1);
        assert_eq!(settings[KEY]["profiles"][&profile.id]["alphabetical"], true);
        let mut restored = Document::read(&settings).unwrap();
        assert!(restored.alphabetical(&source));
        assert!(set(&mut restored, &source, &raw, false).changed);
        assert_eq!(restored, Document::default(), "an emptied matching alpha-only overlay is removed");
        assert!(!set(&mut restored, &source, &raw, false).changed);
        assert_eq!(titles(&restored.project(&source, &raw).unwrap().all, &raw), ["C", "A", "B"]);
        assert_eq!(raw, before);
    }
}

#[test]
fn alphabetical_is_a_view_over_edit_order_and_preserves_membership_and_baseline() {
    let (profile, raw) = fixture(ProviderKind::M3u, &["C", "A", "B"]);
    let source = Source::new(&profile).unwrap();
    let mut doc = Document::default();
    doc.apply(&source, &raw, Operation::ShuffleOrder { category: ALL_ID.into(), seed: 1 }).unwrap();
    doc.apply(&source, &raw, Operation::ResetOrder { category: ALL_ID.into() }).unwrap();
    doc.apply(&source, &raw, Operation::MoveMember { category: ALL_ID.into(),
        item: raw.entries[2].id.clone(), direction: Direction::Up }).unwrap();
    let copy = doc.apply(&source, &raw, Operation::Copy { category: ALL_ID.into(), label: "Copy".into() }).unwrap().category.unwrap();
    let groups = doc.overlay(&source).unwrap().groups.clone();
    assert_eq!(titles(&doc.project(&source, &raw).unwrap().all, &raw), ["C", "B", "A"]);
    set(&mut doc, &source, &raw, true);
    assert_eq!(titles(&doc.project(&source, &raw).unwrap().all, &raw), ["A", "B", "C"]);
    assert_eq!(titles(doc.project(&source, &raw).unwrap().ordered_ids(&copy), &raw), ["A", "B", "C"]);
    assert_eq!(doc.overlay(&source).unwrap().groups, groups);
    set(&mut doc, &source, &raw, false);
    assert_eq!(titles(&doc.project(&source, &raw).unwrap().all, &raw), ["C", "B", "A"]);
    assert_eq!(doc.overlay(&source).unwrap().groups, groups);
    set(&mut doc, &source, &raw, true);
    doc.apply(&source, &raw, Operation::MoveMember { category: ALL_ID.into(),
        item: raw.entries[0].id.clone(), direction: Direction::Down }).unwrap();
    assert!(doc.alphabetical(&source), "manual membership edits do not clear the view preference");
    assert_eq!(titles(&doc.project(&source, &raw).unwrap().all, &raw), ["A", "B", "C"]);
    set(&mut doc, &source, &raw, false);
    assert_eq!(titles(&doc.project(&source, &raw).unwrap().all, &raw), ["B", "C", "A"]);
}

#[test]
fn alphabetical_ties_follow_persisted_order_with_canonical_casefold_and_lexical_numbers() {
    let (profile, raw) = fixture(ProviderKind::M3u,
        &["Zulu", "channel 2", "Channel 10", "é", "e\u{301}", "É", "alpha", "ALPHA", "e", "Straße", "STRASSE", "ß", "ss"]);
    let source = Source::new(&profile).unwrap();
    let mut doc = Document::default();
    for index in [7, 5] {
        doc.apply(&source, &raw, Operation::MoveMember { category: ALL_ID.into(),
            item: raw.entries[index].id.clone(), direction: Direction::Up }).unwrap();
    }
    set(&mut doc, &source, &raw, true);
    assert_eq!(titles(&doc.project(&source, &raw).unwrap().all, &raw),
        ["ALPHA", "alpha", "Channel 10", "channel 2", "e", "ß", "ss", "Straße", "STRASSE", "Zulu", "é", "É", "e\u{301}"]);
}

#[test]
fn alphabetical_is_exact_source_scoped_but_display_rename_keeps_it() {
    let (profile, raw) = fixture(ProviderKind::M3u, &["C", "A", "B"]);
    let source = Source::new(&profile).unwrap();
    let mut doc = Document::default();
    set(&mut doc, &source, &raw, true);
    let before = doc.clone();
    let mut renamed = profile.clone();
    renamed.name = "Display rename".into();
    renamed.source_alias = "Alias rename".into();
    assert!(doc.alphabetical(&Source::new(&renamed).unwrap()));
    let mut other = profile.clone();
    other.source = "https://other.invalid/list.m3u".into();
    let other_source = Source::new(&other).unwrap();
    assert!(!doc.alphabetical(&other_source));
    assert!(!set(&mut doc, &other_source, &raw, false).changed);
    assert_eq!(doc, before, "As-Is on a mismatched source must preserve its dormant owner");
    other.id = "other-profile".into();
    assert!(!doc.alphabetical(&Source::new(&other).unwrap()));
    let (different_kind, _) = fixture(ProviderKind::VPortal, &[]);
    assert!(!doc.alphabetical(&Source::new(&different_kind).unwrap()));
}

#[test]
fn alphabetical_add_picker_keeps_removed_entries_and_does_not_rewrite_raw_or_other_settings() {
    let (profile, raw) = fixture(ProviderKind::Plex, &["C", "A", "B"]);
    let source = Source::new(&profile).unwrap();
    let before_raw = raw.clone();
    let mut doc = Document::default();
    doc.apply(&source, &raw, Operation::RemoveMember { category: ALL_ID.into(), item: raw.entries[1].id.clone() }).unwrap();
    let groups = doc.overlay(&source).unwrap().groups.clone();
    set(&mut doc, &source, &raw, true);
    let before_projection = doc.clone();
    let projection = doc.project(&source, &raw).unwrap();
    assert_eq!(titles(&projection.all, &raw), ["B", "C"]);
    assert_eq!(titles(&projection.available, &raw), ["A", "B", "C"]);
    assert_eq!(titles(&projection.categories[0].members, &raw), ["A", "C"]);
    assert_eq!(doc, before_projection);
    assert_eq!(doc.overlay(&source).unwrap().groups, groups);
    let mut settings = BTreeMap::from([("Favorites".into(), json!([{"id":"unchanged"}]))]);
    doc.write_into(&mut settings).unwrap();
    assert_eq!(settings["Favorites"], json!([{"id":"unchanged"}]));
    assert_eq!(raw, before_raw);
}

#[test]
fn alphabetical_refresh_sorts_new_rows_without_forgetting_dormant_edit_order() {
    let (profile, raw) = fixture(ProviderKind::M3u, &["C", "A", "B"]);
    let source = Source::new(&profile).unwrap();
    let mut doc = Document::default();
    doc.apply(&source, &raw, Operation::MoveMember { category: ALL_ID.into(),
        item: raw.entries[2].id.clone(), direction: Direction::Up }).unwrap();
    set(&mut doc, &source, &raw, true);
    let stored = doc.clone();
    let (_, expanded) = fixture(ProviderKind::M3u, &["C", "A", "B", "AA"]);
    let mut partial = expanded.clone();
    partial.entries.remove(1);
    assert_eq!(titles(&doc.project(&source, &partial).unwrap().all, &partial), ["AA", "B", "C"]);
    assert_eq!(titles(&doc.project(&source, &expanded).unwrap().all, &expanded), ["A", "AA", "B", "C"]);
    assert_eq!(doc, stored);
    set(&mut doc, &source, &expanded, false);
    assert_eq!(titles(&doc.project(&source, &expanded).unwrap().all, &expanded), ["C", "B", "A", "AA"]);
}

#[test]
fn only_successful_shuffle_or_reset_clears_alphabetical() {
    for kind in kinds() {
        let (profile, raw) = fixture(kind, &["C", "A", "B"]);
        let source = Source::new(&profile).unwrap();
        let mut doc = Document::default();
        set(&mut doc, &source, &raw, true);
        let before = doc.clone();
        assert!(!doc.apply(&source, &raw, Operation::ResetOrder { category: ALL_ID.into() }).unwrap().changed);
        let mut single = raw.clone();
        single.entries.truncate(1);
        assert!(!doc.apply(&source, &single, Operation::ShuffleOrder { category: ALL_ID.into(), seed: 1 }).unwrap().changed);
        assert_eq!(doc.apply(&source, &raw, Operation::ShuffleOrder { category: ALL_ID.into(), seed: 0 }), Err(Error::Invalid));
        assert_eq!(doc, before);
        doc.apply(&source, &raw, Operation::ShuffleOrder { category: ALL_ID.into(), seed: 1 }).unwrap();
        assert!(!doc.alphabetical(&source));
        assert!(doc.can_reset_order(&source, ALL_ID));
        let groups = doc.overlay(&source).unwrap().groups.clone();
        set(&mut doc, &source, &raw, true);
        assert_eq!(doc.overlay(&source).unwrap().groups, groups, "enabling the view keeps the reset baseline");
        doc.apply(&source, &raw, Operation::ResetOrder { category: ALL_ID.into() }).unwrap();
        assert!(!doc.alphabetical(&source));
        assert_eq!(titles(&doc.project(&source, &raw).unwrap().all, &raw), ["C", "A", "B"]);
        set(&mut doc, &source, &raw, true);
        assert!(!doc.apply(&source, &raw, Operation::ResetOrder { category: ALL_ID.into() }).unwrap().changed);
        assert!(doc.alphabetical(&source));
    }
}

#[test]
fn version_one_default_false_and_invalid_alphabetical_fields_are_not_migrated() {
    let (profile, raw) = fixture(ProviderKind::M3u, &["C", "A"]);
    let source = Source::new(&profile).unwrap();
    let mut doc = Document::default();
    doc.apply(&source, &raw, Operation::Create { label: "Existing".into() }).unwrap();
    let value = serde_json::to_value(&doc).unwrap();
    assert!(value["profiles"][&profile.id].get("alphabetical").is_none());
    let settings = BTreeMap::from([(KEY.into(), value.clone())]);
    assert_eq!(Document::read(&settings).unwrap(), doc);
    assert!(!Document::read(&settings).unwrap().alphabetical(&source));
    for invalid in [json!("true"), json!(1), json!(null), json!({})] {
        let mut bad = value.clone();
        bad["profiles"][&profile.id]["alphabetical"] = invalid;
        let settings = BTreeMap::from([(KEY.into(), bad)]);
        let before = settings.clone();
        assert_eq!(Document::read(&settings), Err(Error::Invalid));
        assert_eq!(settings, before);
    }
    set(&mut doc, &source, &raw, true);
    set(&mut doc, &source, &raw, false);
    assert_eq!(serde_json::to_value(doc).unwrap(), value);
}

#[test]
fn alphabetical_storage_limit_refusal_is_atomic_even_for_a_tiny_flag() {
    let (profile, raw) = fixture(ProviderKind::M3u, &["C", "A"]);
    let source = Source::new(&profile).unwrap();
    let mut overlay = Overlay::new(&source);
    overlay.next_group = 1001;
    for number in 1..1001 {
        overlay.groups.push(Group { id: format!("{CUSTOM_PREFIX}{number}"), label: Some(format!("Group {number}")),
            provider_label: None, provider_key: None, local_logical: false, members: Members::Snapshot { members: Vec::new() }, shuffle_before: None });
    }
    let mut doc = Document { version: 1, profiles: BTreeMap::from([(profile.id.clone(), overlay)]) };
    let mut remaining = ENCODED_BYTES - serde_json::to_vec(&doc).unwrap().len();
    for group in &mut doc.profiles.get_mut(&profile.id).unwrap().groups {
        let label = group.label.as_mut().unwrap();
        let count = remaining.min(LABEL_BYTES - label.len());
        label.push_str(&"x".repeat(count));
        remaining -= count;
    }
    assert_eq!(remaining, 0);
    doc.validate().unwrap();
    assert_eq!(serde_json::to_vec(&doc).unwrap().len(), ENCODED_BYTES);
    let before = doc.clone();
    assert_eq!(doc.apply(&source, &raw, Operation::SetAlphabetical { enabled: true }), Err(Error::TooLarge));
    assert_eq!(doc, before);
    assert!(!doc.alphabetical(&source));
}
