//! Real VPortal parser/traversal through its existing injected fetch seam.
use super::*;
use crate::app::ott_m3u_attachment::{Authorities, Registry, Receipt, SETTINGS_KEY};
use ott::favorites::{Favorite, Locator, Origin};
use serde_json::json;
use std::collections::BTreeMap;

fn receipt() -> Receipt {
    let mut profiles = ott::ProviderProfiles::default();
    profiles.profiles.push(ProviderProfile { id:"m3u-1".into(), name:"Home".into(),
        kind:ProviderKind::M3u, source:"https://tv.invalid/list.m3u".into(),
        source_alias:"Home".into(), enabled:true });
    let settings = BTreeMap::from([(SETTINGS_KEY.into(), json!({"version":1,"entries":[{
        "id":"attached","parent_profile_id":"m3u-1","kind":"vportal",
        "link":"portal::[key:fixture-key]https://portal.invalid/api"
    }]}))]);
    let registry = Registry::read(&settings, &profiles).unwrap();
    let mut authorities = Authorities::default(); authorities.observe(&registry, &profiles).unwrap();
    authorities.capture("m3u-1").unwrap()
}
fn favorite(category: Option<&str>) -> Favorite {
    Favorite { title:"Ignored title".into(), origin:Origin { provider:"m3u".into(), profile:"Home".into(),
        source_alias:"Home".into(), subprofile:category.map(str::to_owned) },
        locator:Locator::VPortal { id_field:"fid".into(), id:"7".into() } }
}
fn lookup(
    captured: &Receipt, favorite: &Favorite, cancel: &AtomicBool,
    fetch: &mut dyn FnMut(&str, &Value, &AtomicBool) -> Result<String, &'static str>,
) -> Result<CatalogEntry, &'static str> {
    let worker = OffFrame::for_test();
    find_attachment_favorite_with(&worker, captured, favorite, cancel, |effective, emit, deadline| {
        assert_eq!(effective.kind, ProviderKind::VPortal);
        assert_eq!(effective.id, "attached");
        assert_ne!(effective.source, captured.parent().source);
        vportal::visit_with_deadline(&worker, effective, cancel, emit, Some(deadline), fetch)
    })
}
fn response(body: &Value, second_category: &str) -> String {
    match body.get("folder").and_then(Value::as_u64) {
        None => json!({"type":"videoportal","items":[
            {"type":"category","title":"A","request":{"folder":1}},
            {"type":"category","title":second_category,"request":{"folder":2}}
        ]}),
        Some(folder) => json!({"type":"category","items":[{
            "type":"stream","title":"Film","fid":7,"url":format!("/movie{folder}.mp4"),
            "description":"Full description. Second sentence.","img":"/poster.jpg"
        }]}),
    }.to_string()
}

#[test]
fn lazy_attachment_uses_portal_only_and_exact_category_despite_duplicate_id_in_other_folder() {
    let captured = receipt(); let mut requests = Vec::new();
    assert!(requests.is_empty()); // Constructing/capturing the source does no IO.
    let found = lookup(&captured, &favorite(Some("A")), &AtomicBool::new(false), &mut |endpoint, body, _| {
        assert_eq!(endpoint, "https://portal.invalid:443/api");
        assert_eq!(body["key"], "fixture-key");
        assert_eq!(body["app"], "ott-play");
        requests.push(body.get("folder").and_then(Value::as_u64));
        Ok(response(body, "B"))
    }).unwrap();
    assert_eq!(requests, [None, Some(1), Some(2)], "must finish traversal after first match");
    assert_eq!(found.category, "A");
    assert_eq!(found.provider_id_field.as_deref(), Some("fid"));
    assert_eq!(found.provider_id.as_deref(), Some("7"));
    assert_eq!(found.description, "Full description. Second sentence.");
    assert!(found.poster.ends_with("/poster.jpg"));
}

#[test]
fn duplicate_ids_are_ambiguous_without_category_or_with_different_rows_in_same_category() {
    let captured = receipt();
    for (wanted, second, expected) in [
        (None, "B", "Favorite matches more than one item"),
        (Some("A"), "A", "VPortal catalogue contains conflicting rows"),
    ] {
        let mut calls = 0;
        let result = lookup(&captured, &favorite(wanted), &AtomicBool::new(false), &mut |_, body, _| {
            calls += 1; Ok(response(body, second))
        });
        assert_eq!(result, Err(expected), "failed traversal must not return a first-match row");
        assert_eq!(calls, 3);
    }
}

#[test]
fn lookup_error_after_first_match_and_cancellation_cannot_return_partial_success() {
    let captured = receipt();
    let mut calls = 0;
    let result = lookup(&captured, &favorite(Some("A")), &AtomicBool::new(false), &mut |_, body, _| {
        calls += 1;
        if body["folder"] == 2 { Err("fixture interrupted traversal") } else { Ok(response(body, "B")) }
    });
    assert_eq!(calls, 3);
    assert_eq!(result, Err("fixture interrupted traversal"));
    let canceled = AtomicBool::new(true);
    assert!(lookup(&captured, &favorite(None), &canceled, &mut |_, _, _| panic!("canceled work must not fetch")).is_err());
    let canceled = AtomicBool::new(false);
    let result = lookup(&captured, &favorite(Some("A")), &canceled, &mut |_, body, flag| {
        if body["folder"] == 2 { flag.store(true, Ordering::Release); }
        Ok(response(body, "B"))
    });
    assert!(result.is_err());
}

#[test]
fn wrong_origin_id_field_and_category_never_fall_back_to_title() {
    let captured = receipt();
    let mut wrong = favorite(Some("A")); wrong.origin.profile = "Other".into();
    assert!(lookup(&captured, &wrong, &AtomicBool::new(false), &mut |_, _, _| panic!("wrong owner must not fetch")).is_err());
    let mut field = favorite(Some("A")); field.locator = Locator::VPortal { id_field:"id".into(), id:"7".into() };
    for missing in [field, favorite(Some("A / child"))] {
        let result = lookup(&captured, &missing, &AtomicBool::new(false), &mut |_, body, _| Ok(response(body, "B")));
        assert_eq!(result, Err("Favorite is not available from this profile"));
    }
}


/// Uses the standalone call mode from find_favorite and the same production
/// parser/traversal as its VPortal visitor, without opening a network endpoint.
fn standalone_lookup(
    category: Option<&str>, cancel: &AtomicBool,
    fetch: &mut dyn FnMut(&str, &Value, &AtomicBool) -> Result<String, &'static str>,
) -> Result<CatalogEntry, &'static str> {
    let worker = OffFrame::for_test();
    let profile = ProviderProfile {
        id: "standalone-portal".into(), name: "Home".into(), kind: ProviderKind::VPortal,
        source: "portal::[key:fixture-key]https://portal.invalid/api".into(),
        source_alias: "Home".into(), enabled: true,
    };
    let mut wanted = favorite(category);
    wanted.origin.provider = "vportal".into();
    find_catalog_favorite(&worker, &wanted, cancel, false, |emit, deadline| {
        vportal::visit_with_deadline(&worker, &profile, cancel, emit, Some(deadline), fetch)
    })
}

#[test]
fn standalone_favorite_finishes_traversal_and_keeps_exact_category() {
    let mut calls = Vec::new();
    let found = standalone_lookup(Some("A"), &AtomicBool::new(false), &mut |_, body, _| {
        calls.push(body.get("folder").and_then(Value::as_u64));
        Ok(response(body, "B"))
    }).unwrap();
    assert_eq!(calls, [None, Some(1), Some(2)]);
    assert_eq!(found.category, "A");
    assert_eq!(found.description, "Full description. Second sentence.");
    assert!(matches!(found.media, MediaLocator::Http { ref url } if url.ends_with("/movie1.mp4")));
}

#[test]
fn standalone_favorite_rejects_later_distinct_row_with_same_portable_locator() {
    for (wanted, second, error) in [
        (None, "B", "Favorite matches more than one item"),
        (Some("A"), "A", "VPortal catalogue contains conflicting rows"),
    ] {
        let mut calls = 0;
        let found = standalone_lookup(wanted, &AtomicBool::new(false), &mut |_, body, _| {
            calls += 1;
            Ok(response(body, second))
        });
        assert_eq!(calls, 3, "a first-page match must not finish lookup");
        assert_eq!(found, Err(error));
    }
}

#[test]
fn standalone_favorite_accepts_identical_duplicates_after_complete_traversal() {
    let mut calls = 0;
    let found = standalone_lookup(Some("A"), &AtomicBool::new(false), &mut |_, body, _| {
        calls += 1;
        // The two page requests remain distinct. Their media rows are identical,
        // so the production visitor removes the second occurrence.
        Ok(response(body, "A").replace("/movie2.mp4", "/movie1.mp4"))
    }).unwrap();
    assert_eq!(calls, 3);
    assert_eq!(found.provider_id.as_deref(), Some("7"));
    assert!(matches!(found.media, MediaLocator::Http { ref url } if url.ends_with("/movie1.mp4")));
}

#[test]
fn standalone_favorite_does_not_hide_late_failure_or_cancellation() {
    let mut calls = 0;
    let found = standalone_lookup(Some("A"), &AtomicBool::new(false), &mut |_, body, _| {
        calls += 1;
        if body["folder"] == 2 { Err("fixture interrupted traversal") }
        else { Ok(response(body, "B")) }
    });
    assert_eq!(calls, 3);
    assert_eq!(found, Err("fixture interrupted traversal"));

    let cancelled = AtomicBool::new(true);
    assert_eq!(standalone_lookup(Some("A"), &cancelled, &mut |_, _, _| {
        panic!("cancelled lookup must not fetch")
    }), Err("Playback request cancelled"));

    let cancelled = AtomicBool::new(false);
    let mut calls = 0;
    let found = standalone_lookup(Some("A"), &cancelled, &mut |_, body, flag| {
        calls += 1;
        if body["folder"] == 2 { flag.store(true, Ordering::Release); }
        Ok(response(body, "B"))
    });
    assert_eq!(calls, 3);
    assert!(found.is_err(), "cancellation after the first match must not return that match");
}
