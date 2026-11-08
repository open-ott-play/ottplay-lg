use super::*;
use plx_data::ott::{catalog_from_local_files, LocalFile};
use serde_json::json;

fn profile() -> ProviderProfile {
    ProviderProfile { id: "logical-local".into(), name: "Local".into(), kind: ProviderKind::Local,
        source: "/private/fixture/videos".into(), source_alias: "Drive".into(), enabled: true }
}
fn catalog() -> Catalog {
    catalog_from_local_files(&["A/a.mp4", "A/b.mp4", "B/c.mp4", "B/d.mp4"].into_iter()
        .map(|path| LocalFile { relative_path: path.into(), size: 1, duration_ms: None }).collect::<Vec<_>>(), &profile()).unwrap()
}
fn id(raw: &Catalog, path: &str) -> String {
    raw.entries.iter().find(|entry| matches!(&entry.media,
        MediaLocator::Local { relative_path } if relative_path == path)).unwrap().id.clone()
}
fn group<'a>(doc: &'a Document, category: &str) -> &'a Group {
    doc.profiles[&profile().id].groups.iter().find(|group| group.id == category).unwrap()
}
fn apply(doc: &mut Document, raw: &Catalog, op: Operation) -> Change {
    doc.apply(&Source::new(&profile()).unwrap(), raw, op).unwrap()
}
fn legacy() -> (Catalog, Document) {
    let raw = catalog();
    let mut doc = Document::default();
    for category in ["A", "B"] {
        apply(&mut doc, &raw, Operation::ShuffleOrder { category: provider_id(category), seed: 1 });
    }
    let a = doc.profiles.get_mut(&profile().id).unwrap().groups.iter_mut().find(|group| group.id == provider_id("A")).unwrap();
    let Members::Edited { members, .. } = &mut a.members else { panic!("legacy order"); };
    members.insert(0, id(&raw, "B/c.mp4"));
    members.push("logical-local:ffffffffffffffff".into());
    doc.validate().unwrap();
    (raw, doc)
}

#[test]
fn old_local_groups_keep_their_format_and_clamp_after_unrelated_create_and_copy() {
    let (raw, mut doc) = legacy();
    let source = Source::new(&profile()).unwrap();
    let old_a = group(&doc, &provider_id("A")).clone();
    let old_b = group(&doc, &provider_id("B")).clone();
    let value = serde_json::to_value(&doc).unwrap();
    assert!(value["profiles"][profile().id.as_str()]["groups"][0].get("local_logical").is_none());
    assert_eq!(Document::read(&BTreeMap::from([(KEY.into(), value.clone())])).unwrap(), doc);
    assert_eq!(serde_json::to_value(&doc).unwrap(), value);
    let visible_a = doc.project(&source, &raw).unwrap().ordered_ids("A").to_vec();
    assert_eq!(visible_a.len(), 2);
    let created = apply(&mut doc, &raw, Operation::Create { label: "Created".into() }).category.unwrap();
    let copy = apply(&mut doc, &raw, Operation::Copy { category: provider_id("A"), label: "Copy".into() }).category.unwrap();
    assert!(group(&doc, &created).local_logical);
    assert!(group(&doc, &copy).local_logical);
    assert_eq!(doc.project(&source, &raw).unwrap().ordered_ids(&copy), visible_a);
    assert_eq!(group(&doc, &provider_id("A")), &old_a);
    assert_eq!(group(&doc, &provider_id("B")), &old_b);
    let mut regrouped = raw.clone();
    for entry in &mut regrouped.entries { entry.category = "B".into(); }
    let projection = doc.project(&source, &regrouped).unwrap();
    assert!(projection.category(&provider_id("A")).is_none());
    assert_eq!(projection.selection_id("A"), None);
    assert_eq!(projection.ordered_ids(&copy), visible_a);
}

#[test]
fn renaming_legacy_group_promotes_only_sanitized_current_members_and_retains_baseline() {
    let (raw, mut doc) = legacy();
    let source = Source::new(&profile()).unwrap();
    let baseline = group(&doc, &provider_id("A")).shuffle_before.clone();
    let untouched = group(&doc, &provider_id("B")).clone();
    let members = doc.project(&source, &raw).unwrap().ordered_ids("A").to_vec();
    apply(&mut doc, &raw, Operation::Rename { category: provider_id("A"), label: "My A".into() });
    let renamed = group(&doc, &provider_id("A"));
    assert!(renamed.local_logical);
    assert_eq!(renamed.shuffle_before, baseline);
    assert!(matches!(&renamed.members, Members::Edited { members: saved, known }
        if saved == &members && known.len() == 2 && !known.contains(&id(&raw, "B/c.mp4"))));
    assert_eq!(group(&doc, &provider_id("B")), &untouched);
    let mut regrouped = raw.clone();
    for entry in &mut regrouped.entries { entry.category = "B".into(); }
    let projection = doc.project(&source, &regrouped).unwrap();
    assert_eq!(projection.selection_id("A"), Some(provider_id("A").as_str()));
    assert_eq!(projection.ordered_ids("A"), members);
    assert!(!projection.ordered_ids("A").contains(&id(&raw, "B/c.mp4")));
    apply(&mut doc, &regrouped, Operation::ResetOrder { category: provider_id("A") });
    assert!(!doc.project(&source, &regrouped).unwrap().ordered_ids("A").contains(&id(&raw, "B/c.mp4")));
}

#[test]
fn local_member_edits_promote_only_the_target_and_keep_exclusions_and_dormant_ids() {
    let (raw, mut doc) = legacy();
    let source = Source::new(&profile()).unwrap();
    let untouched = group(&doc, &provider_id("B")).clone();
    let foreign = id(&raw, "B/c.mp4");
    apply(&mut doc, &raw, Operation::AddMember { category: provider_id("A"), item: foreign.clone() });
    apply(&mut doc, &raw, Operation::RemoveMember { category: provider_id("A"), item: id(&raw, "A/a.mp4") });
    assert!(group(&doc, &provider_id("A")).local_logical);
    assert_eq!(group(&doc, &provider_id("B")), &untouched);
    let mut partial = raw.clone();
    partial.entries.retain(|entry| entry.id != foreign);
    apply(&mut doc, &partial, Operation::MoveMember { category: provider_id("A"),
        item: id(&raw, "A/b.mp4"), direction: Direction::Up });
    let restored = doc.project(&source, &raw).unwrap();
    assert!(restored.ordered_ids("A").contains(&foreign));
    assert!(!restored.ordered_ids("A").contains(&id(&raw, "A/a.mp4")));
    assert!(!restored.ordered_ids("A").contains(&"logical-local:ffffffffffffffff".to_owned()));
    let mut new = raw.clone();
    let mut entry = raw.entries[0].clone();
    entry.id = "logical-local:eeeeeeeeeeeeeeee".into();
    entry.category = "A".into();
    new.entries.push(entry.clone());
    assert_eq!(doc.project(&source, &new).unwrap().ordered_ids("A").last(), Some(&entry.id));
}

#[test]
fn metadata_moves_delete_and_non_edits_do_not_promote_legacy_memberships() {
    let (raw, mut doc) = legacy();
    let before_groups = doc.profiles[&profile().id].groups.clone();
    apply(&mut doc, &raw, Operation::MoveCategory { category: provider_id("A"), direction: Direction::Down });
    assert_eq!(doc.profiles[&profile().id].groups, before_groups);
    let before = doc.clone();
    for op in [Operation::Rename { category: provider_id("A"), label: "A".into() },
        Operation::AddMember { category: provider_id("A"), item: id(&raw, "A/a.mp4") },
        Operation::RemoveMember { category: provider_id("A"), item: id(&raw, "B/c.mp4") }] {
        assert!(!apply(&mut doc, &raw, op).changed);
        assert_eq!(doc, before);
    }
    let mut single = raw.clone();
    single.entries.retain(|entry| entry.id == id(&raw, "A/a.mp4"));
    assert!(!apply(&mut doc, &single, Operation::MoveMember { category: provider_id("A"),
        item: id(&raw, "A/a.mp4"), direction: Direction::Up }).changed);
    assert_eq!(doc, before);
    assert!(doc.apply(&Source::new(&profile()).unwrap(), &raw,
        Operation::AddMember { category: provider_id("A"), item: "foreign:0000000000000001".into() }).is_err());
    assert_eq!(doc, before);
    apply(&mut doc, &raw, Operation::Delete { category: provider_id("A") });
    assert_eq!(group(&doc, &provider_id("B")), &before_groups[1]);
    assert!(!group(&doc, &provider_id("B")).local_logical);
    assert!(doc.project(&Source::new(&profile()).unwrap(), &raw).unwrap().category(&provider_id("A")).is_none());
}

#[test]
fn local_sorting_never_promotes_and_logical_groups_keep_snapshot_and_exclusions() {
    let (raw, mut doc) = legacy();
    let source = Source::new(&profile()).unwrap();
    apply(&mut doc, &raw, Operation::SetAlphabetical { enabled: true });
    apply(&mut doc, &raw, Operation::ShuffleOrder { category: provider_id("A"), seed: 2 });
    apply(&mut doc, &raw, Operation::ResetOrder { category: provider_id("A") });
    assert!(!group(&doc, &provider_id("A")).local_logical);
    let copy = apply(&mut doc, &raw, Operation::Copy { category: provider_id("A"), label: "Logical".into() }).category.unwrap();
    apply(&mut doc, &raw, Operation::RemoveMember { category: provider_id("A"), item: id(&raw, "A/a.mp4") });
    let a = provider_id("A");
    for category in [&copy, &a] {
        let before = group(&doc, category).members.clone();
        apply(&mut doc, &raw, Operation::ShuffleOrder { category: category.clone(), seed: 1 });
        apply(&mut doc, &raw, Operation::SetAlphabetical { enabled: true });
        apply(&mut doc, &raw, Operation::ResetOrder { category: category.clone() });
        assert!(group(&doc, category).local_logical);
        match (&before, &group(&doc, category).members) {
            (Members::Snapshot { .. }, Members::Snapshot { .. }) => {}
            (Members::Edited { known: a, .. }, Members::Edited { known: b, .. }) => assert_eq!(a, b),
            _ => panic!("sort changed membership kind"),
        }
    }
    assert!(!doc.project(&source, &raw).unwrap().ordered_ids("A").contains(&id(&raw, "A/a.mp4")));
}

#[test]
fn local_logical_roundtrip_reset_and_source_isolation_do_not_touch_media_or_favorites() {
    let raw = catalog();
    let before_raw = raw.clone();
    let source = Source::new(&profile()).unwrap();
    let mut doc = Document::default();
    let category = apply(&mut doc, &raw, Operation::Create { label: "Logical".into() }).category.unwrap();
    apply(&mut doc, &raw, Operation::AddMember { category: category.clone(), item: id(&raw, "B/c.mp4") });
    let mut settings = BTreeMap::from([("Favorites".into(), json!(["unchanged"]))]);
    doc.write_into(&mut settings).unwrap();
    let mut restored = Document::read(&settings).unwrap();
    assert_eq!(restored.project(&source, &raw).unwrap().ordered_ids(&category), [id(&raw, "B/c.mp4")]);
    assert_eq!(settings["Favorites"], json!(["unchanged"]));
    assert!(!serde_json::to_string(&settings[KEY]).unwrap().contains("/private/fixture"));
    let mut other = profile(); other.source = "/other/root".into();
    assert!(!restored.apply(&Source::new(&other).unwrap(), &raw, Operation::Reset).unwrap().changed);
    assert_eq!(restored, doc);
    assert!(restored.apply(&source, &raw, Operation::Reset).unwrap().changed);
    assert_eq!(restored, Document::default());
    assert_eq!(raw, before_raw);
}

#[test]
fn local_marker_is_typed_and_forbidden_on_other_providers() {
    let (raw, mut doc) = legacy();
    let old = serde_json::to_value(&doc).unwrap();
    for invalid in [json!("true"), json!(1), json!(null), json!({})] {
        let mut value = old.clone();
        value["profiles"][profile().id.as_str()]["groups"][0]["local_logical"] = invalid;
        assert_eq!(Document::read(&BTreeMap::from([(KEY.into(), value)])), Err(Error::Invalid));
    }
    apply(&mut doc, &raw, Operation::Rename { category: provider_id("A"), label: "Logical".into() });
    let mut value = serde_json::to_value(&doc).unwrap();
    assert_eq!(value["profiles"][profile().id.as_str()]["groups"][0]["local_logical"], true);
    value["profiles"][profile().id.as_str()]["kind"] = json!("m3u");
    assert_eq!(Document::read(&BTreeMap::from([(KEY.into(), value)])), Err(Error::Invalid));
    let mut value = old;
    value["profiles"][profile().id.as_str()]["groups"][0]["members"]["known"] = json!([id(&raw, "A/a.mp4")]);
    assert_eq!(Document::read(&BTreeMap::from([(KEY.into(), value)])), Err(Error::Invalid));
}

#[test]
fn oversized_local_snapshot_is_rejected_without_promoting_or_replacing_state() {
    let source = Source::new(&profile()).unwrap();
    let raw = catalog();
    let template = raw.entries[0].clone();
    let large = Catalog { entries: (0..20000).map(|index| {
        let mut entry = template.clone(); entry.id = format!("logical-local:{index:016x}"); entry
    }).collect(), hierarchy: None };
    let (_, mut doc) = legacy();
    let before = doc.clone();
    assert_eq!(doc.apply(&source, &large, Operation::Copy { category: ALL_ID.into(), label: "Too large".into() }), Err(Error::TooLarge));
    assert_eq!(doc, before);
    assert_eq!(large.entries.len(), 20000);
}

#[test]
fn first_remove_or_real_move_promotes_sanitized_members_without_changing_neighbors() {
    for remove in [false, true] {
        let (raw, mut doc) = legacy();
        let source = Source::new(&profile()).unwrap();
        let before_b = group(&doc, &provider_id("B")).clone();
        let baseline = group(&doc, &provider_id("A")).shuffle_before.clone();
        let op = if remove {
            Operation::RemoveMember { category: provider_id("A"), item: id(&raw, "A/a.mp4") }
        } else {
            Operation::MoveMember { category: provider_id("A"), item: id(&raw, "A/a.mp4"), direction: Direction::Up }
        };
        assert!(apply(&mut doc, &raw, op).changed);
        let edited = group(&doc, &provider_id("A"));
        assert!(edited.local_logical);
        assert_eq!(edited.shuffle_before, baseline);
        assert!(matches!(&edited.members, Members::Edited { members, known }
            if !members.contains(&id(&raw, "B/c.mp4")) && !members.contains(&"logical-local:ffffffffffffffff".to_owned()) && known.len() == 2));
        assert_eq!(group(&doc, &provider_id("B")), &before_b);
        assert!(!doc.project(&source, &raw).unwrap().ordered_ids("A").contains(&id(&raw, "B/c.mp4")));
    }
}
