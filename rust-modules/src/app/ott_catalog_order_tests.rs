use super::*;
use plx_data::ott::{catalog_from_local_files, LocalFile};
fn profile() -> ProviderProfile {
    ProviderProfile {
        id: "local-test".into(),
        name: "Movies".into(),
        kind: ProviderKind::Local,
        source: "/fixture/movies".into(),
        source_alias: "Movies".into(),
        enabled: true,
    }
}
fn catalog(names: &[&str]) -> Catalog {
    catalog_from_local_files(
        &names
            .iter()
            .map(|name| LocalFile {
                relative_path: format!("{name}.mp4"),
                size: 1,
                duration_ms: None,
            })
            .collect::<Vec<_>>(),
        &profile(),
    )
    .unwrap()
}
#[test]
fn permanent_local_shuffle_roundtrip_reset_retains_first_baseline_and_new_members() {
    let source = Source::new(&profile()).unwrap();
    let original = catalog(&["a", "b", "c"]);
    let mut doc = Document::default();
    doc.apply(
        &source,
        &original,
        Operation::ShuffleOrder {
            category: "".into(),
            seed: 1,
        },
    )
    .unwrap();
    let expanded = catalog(&["a", "b", "c", "d", "e"]);
    doc.apply(
        &source,
        &expanded,
        Operation::ShuffleOrder {
            category: "".into(),
            seed: 2,
        },
    )
    .unwrap();
    let mut settings = BTreeMap::new();
    doc.write_into(&mut settings).unwrap();
    let mut reloaded = Document::read(&settings).unwrap();
    assert!(reloaded.can_reset_order(&source, ""));
    reloaded
        .apply(
            &source,
            &expanded,
            Operation::ResetOrder {
                category: "".into(),
            },
        )
        .unwrap();
    assert_eq!(
        reloaded.project(&source, &expanded).unwrap().all,
        expanded
            .entries
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>()
    );
    assert!(!reloaded.can_reset_order(&source, ""));
    assert_eq!(settings[KEY]["version"], 1);
}
#[test]
fn local_order_is_source_scoped_and_reset_cannot_erase_another_source() {
    let source = Source::new(&profile()).unwrap();
    let raw = catalog(&["a", "b", "c"]);
    let mut doc = Document::default();
    doc.apply(
        &source,
        &raw,
        Operation::ShuffleOrder {
            category: "".into(),
            seed: 1,
        },
    )
    .unwrap();
    let mut other = profile();
    other.source = "/other/movies".into();
    let other = Source::new(&other).unwrap();
    assert!(!doc.can_reset_order(&other, ""));
    let before = doc.clone();
    assert!(!doc.apply(&other, &raw, Operation::Reset).unwrap().changed);
    assert_eq!(doc, before);
    assert_eq!(
        doc.project(&other, &raw).unwrap().all,
        raw.entries
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>()
    );
}
#[test]
fn old_documents_omit_new_optional_baseline_and_large_new_order_rejects_atomically() {
    let source = Source::new(&profile()).unwrap();
    let raw = catalog(&["a", "b", "c"]);
    let mut doc = Document::default();
    doc.apply(
        &source,
        &raw,
        Operation::ShuffleOrder {
            category: "".into(),
            seed: 1,
        },
    )
    .unwrap();
    let mut value = serde_json::to_value(&doc).unwrap();
    value["profiles"]["local-test"]["groups"][0]
        .as_object_mut()
        .unwrap()
        .remove("shuffle_before");
    let old = Document::read(&BTreeMap::from([(KEY.into(), value)])).unwrap();
    assert!(!old.can_reset_order(&source, ""));
    let names: Vec<String> = (0..20_000).map(|n| n.to_string()).collect();
    let big = catalog(&names.iter().map(String::as_str).collect::<Vec<_>>());
    let before = doc.clone();
    assert_eq!(
        doc.apply(
            &source,
            &big,
            Operation::ShuffleOrder {
                category: "".into(),
                seed: 1
            }
        ),
        Err(Error::TooLarge)
    );
    assert_eq!(doc, before);
    assert_eq!(big.entries.len(), 20_000);
}
#[test]
fn category_shuffle_keeps_other_categories_and_reset_never_resurrects_deleted_rows() {
    let source = Source::new(&profile()).unwrap();
    let raw = catalog(&["A/a", "A/b", "A/c", "B/d", "B/e"]);
    let mut doc = Document::default();
    let original_b = doc
        .project(&source, &raw)
        .unwrap()
        .ordered_ids("B")
        .to_vec();
    doc.apply(
        &source,
        &raw,
        Operation::ShuffleOrder {
            category: provider_id("A"),
            seed: 1,
        },
    )
    .unwrap();
    assert_eq!(
        doc.project(&source, &raw).unwrap().ordered_ids("B"),
        original_b
    );
    let reduced = catalog(&["A/a", "A/c", "A/new", "B/d", "B/e"]);
    doc.apply(
        &source,
        &reduced,
        Operation::ResetOrder {
            category: provider_id("A"),
        },
    )
    .unwrap();
    assert_eq!(
        doc.project(&source, &reduced).unwrap().ordered_ids("A"),
        reduced
            .entries
            .iter()
            .filter(|entry| entry.category == "A")
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn local_serialized_order_cannot_hide_membership_with_known_ids() {
    let source = Source::new(&profile()).unwrap();
    let raw = catalog(&["a", "b", "c"]);
    let mut doc = Document::default();
    doc.apply(
        &source,
        &raw,
        Operation::ShuffleOrder {
            category: "".into(),
            seed: 1,
        },
    )
    .unwrap();
    let mut value = serde_json::to_value(doc).unwrap();
    value["profiles"]["local-test"]["groups"][0]["members"]["known"] =
        serde_json::json!([raw.entries[0].id]);
    assert_eq!(
        Document::read(&BTreeMap::from([(KEY.into(), value)])),
        Err(Error::Invalid)
    );
}

#[test]
fn local_order_cannot_move_members_between_folders_or_restore_absent_categories() {
    let source = Source::new(&profile()).unwrap();
    let raw = catalog(&["A/a", "A/b", "B/c"]);
    let mut doc = Document::default();
    doc.apply(
        &source,
        &raw,
        Operation::ShuffleOrder {
            category: provider_id("A"),
            seed: 1,
        },
    )
    .unwrap();
    let foreign = raw
        .entries
        .iter()
        .find(|entry| entry.category == "B")
        .unwrap()
        .id
        .clone();
    let group = &mut doc.profiles.get_mut("local-test").unwrap().groups[0];
    let Members::Edited { members, .. } = &mut group.members else {
        panic!("edited order")
    };
    members.insert(0, foreign.clone());
    doc.validate().unwrap();
    assert!(!doc
        .project(&source, &raw)
        .unwrap()
        .ordered_ids("A")
        .contains(&foreign));
    let mut moved = raw.clone();
    for entry in &mut moved.entries {
        entry.category = "B".into();
    }
    let projection = doc.project(&source, &moved).unwrap();
    assert!(projection
        .categories
        .iter()
        .all(|category| category.label != "A"));
    assert_eq!(projection.selection_id("A"), None);
    assert_eq!(projection.ordered_ids("B").len(), 3);
    assert_eq!(
        doc.apply(
            &source,
            &moved,
            Operation::ResetOrder {
                category: provider_id("A")
            }
        ),
        Err(Error::MissingCategory)
    );
}

#[test]
fn generic_shuffle_reset_preserves_exclusions_snapshots_and_first_baseline() {
    for kind in [ProviderKind::M3u, ProviderKind::Plex, ProviderKind::VPortal] {
        let (p, mut raw) = if kind == ProviderKind::M3u {
            let p = ProviderProfile { id: "m3u-order".into(), name: "Order".into(), kind,
                source: "https://playlist.invalid/list.m3u".into(), source_alias: String::new(), enabled: true };
            let raw = plx_data::ott::parse_m3u("#EXTM3U\n#EXTINF:-1 group-title=\"Group\",A\nhttps://media.invalid/a\n#EXTINF:-1 group-title=\"Group\",B\nhttps://media.invalid/b\n", &p).unwrap();
            (p, raw)
        } else { super::tests::hierarchical_fixture(kind) };
        // Use All so every provider shares the same membership contract.
        let source = Source::new(&p).unwrap();
        let mut extra = raw.entries[0].clone();
        extra.id = format!("{}:ffffffffffffffff", p.id);
        if let MediaLocator::Plex { rating_key, .. } = &mut extra.media { *rating_key = "ffffffffffffffff".into(); }
        raw.entries.push(extra);
        let baseline: Vec<_> = raw.entries.iter().map(|entry| entry.id.clone()).collect();
        let mut doc = Document::default();
        doc.apply(&source, &raw, Operation::RemoveMember { category: ALL_ID.into(), item: baseline[1].clone() }).unwrap();
        let copy = doc.apply(&source, &raw, Operation::Copy { category: ALL_ID.into(), label: "Snapshot".into() }).unwrap().category.unwrap();
        for category in [ALL_ID, copy.as_str()] {
            doc.apply(&source, &raw, Operation::ShuffleOrder { category: category.into(), seed: 1 }).unwrap();
        }
        let mut new = raw.entries[0].clone();
        new.id = format!("{}:eeeeeeeeeeeeeeee", p.id);
        if let MediaLocator::Plex { rating_key, .. } = &mut new.media { *rating_key = "eeeeeeeeeeeeeeee".into(); }
        let new_id = new.id.clone();
        raw.entries.push(new);
        // Further shuffle must not replace the first order, revive the excluded row,
        // or turn the custom copy into an inheriting provider group.
        for category in [ALL_ID, copy.as_str()] {
            doc.apply(&source, &raw, Operation::ShuffleOrder { category: category.into(), seed: 2 }).unwrap();
        }
        let mut settings = BTreeMap::new();
        doc.write_into(&mut settings).unwrap();
        let mut loaded = Document::read(&settings).unwrap();
        for category in [ALL_ID, copy.as_str()] {
            loaded.apply(&source, &raw, Operation::ResetOrder { category: category.into() }).unwrap();
            assert!(!loaded.can_reset_order(&source, category));
        }
        let projection = loaded.project(&source, &raw).unwrap();
        assert_eq!(projection.all, [baseline[0].clone(), baseline[2].clone(), new_id]);
        assert_eq!(projection.ordered_ids(&copy), [baseline[0].clone(), baseline[2].clone()]);
        let groups = &loaded.overlay(&source).unwrap().groups;
        assert!(matches!(&groups.iter().find(|group| group.id == copy).unwrap().members, Members::Snapshot { .. }));
        assert!(matches!(&groups.iter().find(|group| group.id == ALL_ID).unwrap().members,
            Members::Edited { known, .. } if known.contains(&baseline[1])));
        assert_eq!(raw.entries.len(), 4);
    }
}

#[test]
fn shuffled_custom_members_stay_dormant_and_later_removal_is_not_reset_back_in() {
    let (p, mut raw) = super::tests::hierarchical_fixture(ProviderKind::Plex);
    raw.hierarchy = None;
    let source = Source::new(&p).unwrap();
    let mut doc = Document::default();
    let copy = doc.apply(&source, &raw, Operation::Copy { category: ALL_ID.into(), label: "Saved copy".into() }).unwrap().category.unwrap();
    doc.apply(&source, &raw, Operation::ShuffleOrder { category: copy.clone(), seed: 1 }).unwrap();
    let mut partial = raw.clone();
    partial.entries.remove(0);
    doc.apply(&source, &partial, Operation::ResetOrder { category: copy.clone() }).unwrap();
    assert_eq!(doc.project(&source, &raw).unwrap().ordered_ids(&copy),
        raw.entries.iter().map(|entry| entry.id.clone()).collect::<Vec<_>>());
    doc.apply(&source, &raw, Operation::ShuffleOrder { category: copy.clone(), seed: 2 }).unwrap();
    doc.apply(&source, &raw, Operation::RemoveMember { category: copy.clone(), item: raw.entries[0].id.clone() }).unwrap();
    doc.apply(&source, &raw, Operation::ResetOrder { category: copy.clone() }).unwrap();
    assert_eq!(doc.project(&source, &raw).unwrap().ordered_ids(&copy), [raw.entries[1].id.clone()]);
}

#[test]
fn non_local_reset_keeps_first_baseline_then_new_items_in_their_current_order() {
    for kind in [ProviderKind::Plex, ProviderKind::VPortal] {
        let (p, mut raw) = super::tests::hierarchical_fixture(kind);
        raw.hierarchy = None;
        let source = Source::new(&p).unwrap();
        let initial: Vec<_> = raw.entries.iter().map(|entry| entry.id.clone()).collect();
        let mut doc = Document::default();
        doc.apply(&source, &raw, Operation::ShuffleOrder { category: ALL_ID.into(), seed: 1 }).unwrap();
        let mut later = Vec::new();
        for suffix in ["cccccccccccccccc", "dddddddddddddddd"] {
            let mut entry = raw.entries[0].clone();
            entry.id = format!("{}:{suffix}", p.id);
            if let MediaLocator::Plex { rating_key, .. } = &mut entry.media { *rating_key = suffix.into(); }
            later.push(entry.id.clone());
            raw.entries.push(entry);
        }
        doc.apply(&source, &raw, Operation::ShuffleOrder { category: ALL_ID.into(), seed: 2 }).unwrap();
        doc.apply(&source, &raw, Operation::MoveMember {
            category: ALL_ID.into(), item: later[1].clone(), direction: Direction::Up,
        }).unwrap();
        let reordered = doc.project(&source, &raw).unwrap();
        assert_eq!(reordered.all.iter().filter(|id| later.contains(id)).cloned().collect::<Vec<_>>(),
            [later[1].clone(), later[0].clone()], "the viewer has moved D before C");
        let mut settings = BTreeMap::new();
        doc.write_into(&mut settings).unwrap();
        let mut restored = Document::read(&settings).unwrap();
        restored.apply(&source, &raw, Operation::ResetOrder { category: ALL_ID.into() }).unwrap();
        assert_eq!(restored.project(&source, &raw).unwrap().all,
            [initial[0].clone(), initial[1].clone(), later[1].clone(), later[0].clone()]);
    }
}
