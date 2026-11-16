//! Source-owned category projection without navigation or playback effects.
use super::*;
use plx_data::ott::{CategoryHierarchy, CategoryMembership, CategoryNode};

fn fixture() -> Box<App> {
    let mut app = Box::new(ready_app_fixture());
    let runtime = app.ott.as_mut().unwrap();
    runtime.provider = ProviderKind::VPortal;
    runtime.saved.profiles.profiles = vec![synthetic_profile()];
    runtime.saved.profiles.active_profile = Some("portal-fixture".into());
    runtime.saved.settings.insert("interfaceMode".into(), json!(1));
    runtime.catalog = plx_data::ott::parse_vportal_response(&json!({
        "type":"videoportal", "items":[
            {"type":"stream","id":"one","title":"One","request":{"id":"one"}},
            {"type":"stream","id":"two","title":"Two","request":{"id":"two"}}
        ]
    }).to_string(), &synthetic_profile(), "").unwrap().catalog;
    let ids: Vec<_> = runtime.catalog.entries.iter().map(|entry| entry.id.clone()).collect();
    runtime.catalog.hierarchy = Some(CategoryHierarchy {
        nodes: [("films", "Films"), ("3d", "3D"), ("empty", "Empty")].into_iter()
            .map(|(id, label)| CategoryNode { id:id.into(), parent:None, label:label.into(),
                anonymous_fingerprint:None, source_identity:None }).collect(),
        memberships: vec![
            CategoryMembership { entry_id:ids[0].clone(), category_id:"films".into() },
            CategoryMembership { entry_id:ids[1].clone(), category_id:"films".into() },
            CategoryMembership { entry_id:ids[1].clone(), category_id:"3d".into() },
        ],
    });
    runtime.page = "channels";
    runtime.catalog_loading = false;
    runtime.catalog_complete = true;
    runtime.sync_saved_context();
    runtime.changed();
    app
}

#[test]
fn modern_all_shelves_reference_one_canonical_catalogue() {
    let _lock = plx_base::testlock::serial();
    let app = fixture();
    let runtime = app.ott.as_ref().unwrap();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let view = runtime.view();
    assert_eq!(view.rows.len(), 2);
    assert_eq!(view.shelves.iter().map(|shelf| shelf.title.as_str()).collect::<Vec<_>>(), [plx_platform::i18n::msg::browse_library_all(), "Films", "3D"]);
    assert_eq!(view.shelves[0].rows, [0, 1]);
    assert_eq!(view.shelves[1].rows, [0, 1]);
    assert_eq!(view.shelves[2].rows, [1]);
    for row in &view.rows {
        assert_eq!(row.action, OttAction::Play(row.id.clone()));
        assert!(runtime.catalog.entries.iter().any(|entry| entry.id == row.id));
    }
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(runtime.playing.is_none() && runtime.resolver_next.is_none());
}

#[test]
fn shelf_memory_context_is_presentation_only_and_retires_with_the_source() {
    let _lock = plx_base::testlock::serial();
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    let context = runtime.view().shelf_context.unwrap();
    runtime.navigation_epoch += 1;
    runtime.revision += 1;
    assert_eq!(runtime.view().shelf_context.as_deref(), Some(context.as_str()));
    runtime.generation += 1;
    assert_ne!(runtime.view().shelf_context.as_deref(), Some(context.as_str()));
}

#[test]
fn category_membership_and_user_order_use_the_existing_projection() {
    let _lock = plx_base::testlock::serial();
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    let source = catalog_organization::Source::new(&synthetic_profile()).unwrap();
    let mut document = catalog_organization::Document::default();
    let first = runtime.organization_projection().unwrap().categories[0].id.clone();
    document.apply(&source, &runtime.catalog, catalog_organization::Operation::Rename {
        category:first.clone(), label:"Cinema".into(),
    }).unwrap();
    document.apply(&source, &runtime.catalog, catalog_organization::Operation::MoveMember {
        category:first, item:runtime.catalog.entries[1].id.clone(),
        direction:catalog_organization::Direction::Up,
    }).unwrap();
    document.write_into(&mut runtime.saved.settings).unwrap();
    let view = runtime.view();
    assert_eq!(view.shelves[0].rows, [0, 1]);
    assert_eq!(view.shelves[1].title, "Cinema");
    assert_eq!(view.shelves[1].rows, [1, 0]);
    assert_eq!(view.rows[0].id, runtime.catalog.entries[0].id);
}

#[test]
fn classic_filtered_and_favorites_keep_the_flat_view() {
    let _lock = plx_base::testlock::serial();
    for case in 0..3 {
        let mut app = fixture();
        let runtime = app.ott.as_mut().unwrap();
        match case {
            0 => { runtime.saved.settings.insert("interfaceMode".into(), json!(0)); },
            1 => { runtime.saved.category = "films".into(); },
            _ => { runtime.saved.favorites_selected = true; runtime.page = "favorites"; },
        }
        let view = runtime.view();
        assert!(view.shelves.is_empty(), "case {case}");
    }
}


#[test]
fn uncategorised_all_preserves_canonical_rows_and_real_category_identity() {
    let _lock = plx_base::testlock::serial();
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    let before = runtime.view();
    let id = runtime.catalog.entries[0].id.clone();
    runtime.catalog.hierarchy.as_mut().unwrap().memberships
        .retain(|membership| membership.entry_id != id);
    runtime.catalog_revision += 1;
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let after = runtime.view();
    assert_eq!(after.rows, before.rows);
    assert_eq!(after.shelves[0], before.shelves[0]);
    assert_eq!(after.shelves.iter().skip(1).map(|shelf| (&shelf.id, &shelf.title)).collect::<Vec<_>>(),
        before.shelves.iter().skip(1).map(|shelf| (&shelf.id, &shelf.title)).collect::<Vec<_>>());
    assert_eq!(after.shelves[1].rows, [1]);
    assert_eq!(after.shelves[2].rows, [1]);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(runtime.playing.is_none() && runtime.resolver_next.is_none());
}

#[test]
fn all_occurrence_identity_survives_append_and_reorder_in_the_same_source() {
    let _lock = plx_base::testlock::serial();
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    let before = runtime.view();
    let selected = before.rows[1].id.clone();
    let identity = (before.shelf_context.clone(), before.shelves[0].id.clone(), selected.clone());
    let mut appended = runtime.catalog.entries[0].clone();
    appended.id = "unclassified-appended-item".into();
    appended.title = "Appended item".into();
    runtime.catalog.entries.push(appended);
    runtime.catalog.entries.reverse();
    runtime.catalog_revision += 1;
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let after = runtime.view();
    assert_eq!(after.shelves[0].rows, (0..after.rows.len()).collect::<Vec<_>>());
    assert!(after.shelves[0].rows.iter().any(|&index| after.rows[index].id == selected));
    assert_eq!((after.shelf_context, after.shelves[0].id.clone(), selected), identity);
    assert_eq!(after.rows.len(), 3);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
}

#[test]
fn m3u_and_local_parsers_share_all_without_replacing_playback_identity() {
    let _lock = plx_base::testlock::serial();
    for kind in [ProviderKind::M3u, ProviderKind::Local] {
        let mut app = fixture();
        let runtime = app.ott.as_mut().unwrap();
        let mut profile = synthetic_profile();
        profile.kind = kind;
        profile.source = if kind == ProviderKind::Local { "/fixture/videos".into() }
            else { "https://playlist.invalid/list.m3u".into() };
        runtime.catalog = if kind == ProviderKind::Local {
            plx_data::ott::catalog_from_local_files(&[
                plx_data::ott::LocalFile { relative_path:"Family/One.mp4".into(), size:1, duration_ms:None },
                plx_data::ott::LocalFile { relative_path:"Two.mkv".into(), size:1, duration_ms:None },
            ], &profile).unwrap()
        } else {
            plx_data::ott::parse_m3u(concat!("#EXTM3U\n",
                "#EXTINF:-1 group-title=\"Family\",One\nhttps://media.invalid/one.ts\n",
                "#EXTINF:-1,Two\nhttps://media.invalid/two.ts\n"), &profile).unwrap()
        };
        runtime.provider = kind;
        runtime.saved.profiles.profiles = vec![profile];
        runtime.catalog_revision += 1;
        let entries = runtime.catalog.entries.clone();
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let view = runtime.view();
        assert_eq!(view.shelves[0].rows, [0, 1]);
        assert_eq!(view.shelves[0].title, plx_platform::i18n::msg::browse_library_all());
        for (row, entry) in view.rows.iter().zip(&entries) {
            assert_eq!(row.id, entry.id);
            assert_eq!(row.action, OttAction::Play(entry.id.clone()));
            assert_eq!(row.content_title.as_deref(), Some(entry.title.as_str()));
            assert_eq!(row.description, entry.description);
        }
        assert_eq!(runtime.catalog.entries, entries);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(runtime.playing.is_none() && runtime.resolver_next.is_none());
    }
}


#[test]
fn all_plus_membership_overflow_keeps_every_canonical_row_in_the_flat_view() {
    let _lock = plx_base::testlock::serial();
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    let template = runtime.catalog.entries[0].clone();
    let count = plx_data::ott::MAX_CATALOG_ENTRIES / 2 + 1;
    runtime.catalog.entries = (0..count).map(|index| {
        let mut entry = template.clone();
        entry.id = format!("bounded-item-{index}");
        entry
    }).collect();
    runtime.catalog.hierarchy = Some(CategoryHierarchy {
        nodes: vec![CategoryNode { id:"films".into(), parent:None, label:"Films".into(),
            anonymous_fingerprint:None, source_identity:None }],
        memberships: runtime.catalog.entries.iter().map(|entry| CategoryMembership {
            entry_id:entry.id.clone(), category_id:"films".into(),
        }).collect(),
    });
    runtime.catalog_revision += 1;
    let view = runtime.view();
    assert_eq!(view.rows.len(), count);
    assert!(view.shelves.is_empty(), "All plus category occurrences must not raise the existing cap");
    for (row, entry) in view.rows.iter().zip(&runtime.catalog.entries) {
        assert_eq!(row.id, entry.id);
        assert_eq!(row.action, OttAction::Play(entry.id.clone()));
    }
}
