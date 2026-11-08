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
    assert_eq!(view.shelves.iter().map(|shelf| shelf.title.as_str()).collect::<Vec<_>>(), ["Films", "3D"]);
    assert_eq!(view.shelves[0].rows, [0, 1]);
    assert_eq!(view.shelves[1].rows, [1]);
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
    assert_eq!(view.shelves[0].title, "Cinema");
    assert_eq!(view.shelves[0].rows, [1, 0]);
    assert_eq!(view.rows[0].id, runtime.catalog.entries[0].id);
}

#[test]
fn classic_filtered_favorites_and_uncategorised_all_keep_the_flat_view() {
    let _lock = plx_base::testlock::serial();
    for case in 0..4 {
        let mut app = fixture();
        let runtime = app.ott.as_mut().unwrap();
        match case {
            0 => { runtime.saved.settings.insert("interfaceMode".into(), json!(0)); },
            1 => { runtime.saved.category = "films".into(); },
            2 => { runtime.saved.favorites_selected = true; runtime.page = "favorites"; },
            _ => { runtime.catalog.hierarchy.as_mut().unwrap().memberships
                .retain(|membership| membership.entry_id != runtime.catalog.entries[0].id); },
        }
        let view = runtime.view();
        assert!(view.shelves.is_empty(), "case {case}");
        if case == 3 { assert_eq!(view.rows.len(), 2, "All must not hide unassigned content"); }
    }
}
