use super::*;
use serde_json::json;
use std::cell::RefCell;

fn profile() -> ProviderProfile {
    ProviderProfile {
        id: "portal".into(),
        name: "Portal".into(),
        kind: ProviderKind::VPortal,
        source: "portal::[key:private]https://portal.invalid/api".into(),
        source_alias: String::new(),
        enabled: true,
    }
}
fn entry() -> CatalogEntry {
    CatalogEntry {
        synopsis: None,
        id: "x".into(),
        title: "X".into(),
        category: String::new(),
        description: String::new(),
        poster: String::new(),
        media: MediaLocator::VPortal {
            request: json!({"id":1}),
        },
        duration_ms: None,
        provider_id: Some("1".into()),
        provider_id_field: Some("id".into()),
        live: None,
    }
}
fn nas_profile() -> ProviderProfile {
    ProviderProfile {
        source: "portal::[key:private]https://nas.invalid/nas/api".into(),
        ..profile()
    }
}

#[test]
fn returned_auth_echo_is_stripped_before_traversal_and_first_leaf_publication() {
    let events = RefCell::new(Vec::new());
    let cancel = AtomicBool::new(false);
    let mut transport = |_: &str, body: &Value, _: &AtomicBool| {
        assert_eq!(body["app"], "ott-play");
        assert_eq!(body["key"], "private");
        assert_eq!(body["limit"], 300);
        let response = match (
            body.get("folder").and_then(Value::as_u64),
            body.get("offset").and_then(Value::as_u64),
        ) {
            (None, None) => {
                events.borrow_mut().push("root");
                // Observed root shape: the response request contains ONLY a
                // key echo, not the limit/app fields sent by the client.
                json!({"type":"videoportal","request":{"key":"echo-key"},"items":[
                    {"type":"category","title":"A","request":{"folder":1,"app":"echo-app","key":"echo-key"}},
                    {"type":"category","title":"B","request":{"folder":2,"app":"echo-app","key":"echo-key"}}]})
            }
            (Some(1), None) => {
                events.borrow_mut().push("folder_a");
                json!({"type":"category","request":{"folder":1,"limit":300,"app":"echo-app","key":"echo-key"},"items":[
                    {"type":"stream","title":"One","request":{"cmd":"play","id":7,"app":"echo-app","key":"echo-key"}},
                    {"type":"next","request":{"offset":1,"limit":300,"app":"echo-app","key":"echo-key"}}]})
            }
            (Some(1), Some(1)) => {
                events.borrow_mut().push("next_a");
                json!({"type":"category","items":[
                    {"type":"stream","title":"Two","request":{"cmd":"play","id":8,"app":"echo-app","key":"echo-key"}}]})
            }
            (Some(2), None) => {
                events.borrow_mut().push("folder_b");
                json!({"type":"category","items":[]})
            }
            _ => panic!("unexpected fixture request"),
        };
        Ok(response.to_string())
    };
    let mut rows = Vec::new();
    visit_with(&OffFrame::for_test(), &profile(), &cancel, &mut |batch| {
        events.borrow_mut().push("emit");
        rows.extend(batch);
        Ok(())
    }, &mut transport).unwrap();
    assert_eq!(*events.borrow(), ["root", "folder_a", "emit", "next_a", "emit", "folder_b"]);
    assert_eq!(rows.len(), 2);
    let link = ott::parse_vportal_link(&profile().source).unwrap();
    for (row, id) in rows.iter().zip([7, 8]) {
        assert_eq!(row.category, "A");
        let MediaLocator::VPortal { request } = &row.media else {
            panic!("expected retained portal locator");
        };
        assert_eq!(request, &json!({"cmd":"play","id":id}));
        let body = link.request_body(request).unwrap();
        assert_eq!(body["app"], "ott-play");
        assert_eq!(body["key"], "private");
    }
}

#[test]
fn first_leaf_precedes_other_folders_and_paging_keeps_request_context() {
    let events = RefCell::new(Vec::new());
    let cancel = AtomicBool::new(false);
    let mut transport = |_: &str, body: &Value, _: &AtomicBool| {
        assert_eq!(body["app"], "ott-play");
        assert_eq!(body["key"], "private");
        assert_eq!(body["limit"], 300);
        let response = match (
            body.get("folder").and_then(Value::as_u64),
            body.get("offset").and_then(Value::as_u64),
        ) {
            (None, _) => {
                events.borrow_mut().push("root");
                json!({"type":"videoportal","items":[
                {"type":"category","title":"A","request":{"folder":1}},
                {"type":"category","title":"B","request":{"folder":2}}]})
            }
            (Some(1), None) => {
                events.borrow_mut().push("folder_a");
                json!({"type":"category","items":[
                {"type":"stream","id":1,"title":"One","url":"/one"},{"type":"next","request":{}}]})
            }
            (Some(1), Some(1)) => {
                events.borrow_mut().push("next_a");
                json!({"type":"category","items":[
                {"type":"stream","id":2,"title":"Two","url":"/two"}]})
            }
            (Some(2), _) => {
                events.borrow_mut().push("folder_b");
                json!({"type":"category","items":[]})
            }
            _ => panic!("unexpected fixture request"),
        };
        Ok(response.to_string())
    };
    let mut rows = Vec::new();
    visit_with(
        &OffFrame::for_test(),
        &profile(),
        &cancel,
        &mut |batch| {
            events.borrow_mut().push("emit");
            rows.extend(batch);
            Ok(())
        },
        &mut transport,
    )
    .unwrap();
    assert_eq!(
        *events.borrow(),
        ["root", "folder_a", "emit", "next_a", "emit", "folder_b"]
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].category, "A");
}

#[test]
fn cancellation_during_fetch_never_emits_or_fetches_another_page() {
    let cancel = AtomicBool::new(false);
    let mut called = 0;
    let mut transport = |_: &str, _: &Value, flag: &AtomicBool| {
        called += 1;
        flag.store(true, Ordering::Release);
        Ok(
            json!({"type":"category","items":[{"type":"stream","title":"One","url":"/one"}]})
                .to_string(),
        )
    };
    let result = visit_with(
        &OffFrame::for_test(),
        &profile(),
        &cancel,
        &mut |_| panic!("stale emission"),
        &mut transport,
    );
    assert_eq!(result, Err(CANCELLED));
    assert_eq!(called, 1);
}

#[test]
fn callback_cancellation_stops_within_first_page() {
    let cancel = AtomicBool::new(false);
    let mut transport = |_: &str, _: &Value, _: &AtomicBool| {
        Ok(json!({"type":"category","items":[
        {"type":"stream","id":1,"url":"/one"},{"type":"stream","id":2,"url":"/two"}]})
        .to_string())
    };
    let mut count = 0;
    let result = visit_with(
        &OffFrame::for_test(),
        &profile(),
        &cancel,
        &mut |rows| {
            count += rows.len();
            cancel.store(true, Ordering::Release);
            Ok(())
        },
        &mut transport,
    );
    assert_eq!(result, Err(CANCELLED));
    assert_eq!(count, 1);
}

#[test]
fn repeated_request_in_distinct_categories_is_preserved_but_cycle_stops() {
    let mut calls = 0;
    let mut transport = |_: &str, body: &Value, _: &AtomicBool| {
        calls += 1;
        Ok(if body.get("folder").is_none() {
            json!({"type":"videoportal","items":[
            {"type":"category","title":"A","request":{"folder":1}},
            {"type":"category","title":"B","request":{"folder":1}}]})
        } else {
            json!({"type":"category","items":[{"type":"stream","id":1,"url":"/one"}]})
        }
        .to_string())
    };
    let mut rows = Vec::new();
    visit_with(
        &OffFrame::for_test(),
        &profile(),
        &AtomicBool::new(false),
        &mut |batch| {
            rows.extend(batch);
            Ok(())
        },
        &mut transport,
    )
    .unwrap();
    assert_eq!(calls, 3);
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0].id, rows[1].id);
    let mut cycle = |_: &str, _: &Value, _: &AtomicBool| {
        Ok(
            json!({"type":"category","items":[{"type":"category","title":"Loop","request":{}}]})
                .to_string(),
        )
    };
    let result = visit_with(
        &OffFrame::for_test(),
        &profile(),
        &AtomicBool::new(false),
        &mut |_| Ok(()),
        &mut cycle,
    );
    assert_eq!(result, Err("VPortal catalogue contains a page loop"));
}

#[test]
fn flat_pagination_can_exceed_folder_depth_limit() {
    let mut calls = 0;
    let mut transport = |_: &str, body: &Value, _: &AtomicBool| {
        calls += 1;
        let offset = body.get("offset").and_then(Value::as_u64).unwrap_or(0);
        let mut items = vec![json!({"type":"stream","id":offset,"url":"/one"})];
        if offset < 140 {
            items.push(json!({"type":"next","request":{}}));
        }
        Ok(json!({"type":"category","items":items}).to_string())
    };
    let mut count = 0;
    visit_with(
        &OffFrame::for_test(),
        &profile(),
        &AtomicBool::new(false),
        &mut |rows| {
            count += rows.len();
            Ok(())
        },
        &mut transport,
    )
    .unwrap();
    assert_eq!(calls, 141);
    assert_eq!(count, 141);
}

#[test]
fn default_and_variant_resolution_validate_urls() {
    let cancel = AtomicBool::new(false);
    let mut transport = |_: &str, _: &Value, _: &AtomicBool| {
        Ok(
            json!({"url":"javascript:bad","variants":{"bad":"file:///x","good":"/movie.m3u8"}})
                .to_string(),
        )
    };
    let resolved = resolve_with(&profile(), &entry(), &cancel, &mut transport).unwrap();
    assert_eq!(resolved.url, "https://portal.invalid:443/movie.m3u8");
    assert!(resolved.session.is_none());
    let mut rejected = |_: &str, _: &Value, _: &AtomicBool| {
        Ok(json!({"type":"error","url":"/should-not-play"}).to_string())
    };
    assert!(resolve_with(&profile(), &entry(), &cancel, &mut rejected).is_err());
}

#[test]
fn stale_playback_returns_owned_session_for_cleanup_before_cancel_check() {
    let cancel = AtomicBool::new(false);
    let mut transport = |_: &str, _: &Value, flag: &AtomicBool| {
        flag.store(true, Ordering::Release);
        Ok(
            json!({"url":"/movie.m3u8","stop":"https://nas.invalid/nas/stream/one.key/media.stop",
            "heartbeat":"https://nas.invalid/nas/stream/one.key/media.ping"})
            .to_string(),
        )
    };
    let resolved = resolve_with(&nas_profile(), &entry(), &cancel, &mut transport).unwrap();
    let session = resolved.session.unwrap();
    assert!(session.stop.ends_with("media.stop"));
    assert!(session.heartbeat.unwrap().ends_with("media.ping"));
    assert!(cancel.load(Ordering::Acquire));
}

#[test]
fn failed_playback_preserves_only_top_level_cleanup_lease() {
    let mut transport = |_: &str, _: &Value, _: &AtomicBool| {
        Ok(json!({"type":"error",
        "stop":"https://nas.invalid/nas/stream/one.key/media.stop"})
        .to_string())
    };
    let resolved = resolve_with(
        &nas_profile(),
        &entry(),
        &AtomicBool::new(false),
        &mut transport,
    )
    .unwrap();
    assert!(resolved.url.is_empty());
    assert!(resolved.failure.is_some());
    assert!(resolved.session.is_some());
}

#[test]
fn session_hints_require_exact_api_origin_and_narrow_control_path() {
    let endpoint = "https://nas.invalid/nas/api";
    let valid = "https://nas.invalid/nas/stream/one.key/media.stop";
    assert!(session_url(endpoint, valid).is_some());
    assert!(session_url("https://NAS.invalid/nas/api", valid).is_some());
    for invalid in [
        "//nas.invalid/nas/stream/one.key/media.stop",
        "https://elsewhere.invalid/nas/stream/one.key/media.stop",
        "https://nas.invalid:444/nas/stream/one.key/media.stop",
        "https://nas.invalid/delete",
        "https://nas.invalid/nas/stream/one.key/media.stop?x=1",
        "https://nas.invalid/nas/stream/one.key/media.stop#x",
    ] {
        assert!(session_url(endpoint, invalid).is_none());
    }
    assert!(session_url("https://nas.invalid/prefix/nas/api", valid).is_none());
    assert!(session_url("https://nas.invalid/api", valid).is_none());
    assert!(session_url(
        endpoint,
        "https://nas.invalid/NAS/STREAM/ONE.KEY/MEDIA.STOP"
    )
    .is_some());
}

#[test]
fn selected_variant_adopts_its_session_and_does_not_borrow_unrelated_top_level_lease() {
    let data = json!({"url":"/default","stop":"https://nas.invalid/nas/stream/default.key/media.stop",
        "sessions":{"/other":{"stop":"https://nas.invalid/nas/stream/other.key/media.stop"}}});
    let chosen = session_hints(
        "https://nas.invalid/nas/api",
        &data,
        "/other",
        "https://nas.invalid:443/other",
    )
    .unwrap();
    assert!(chosen.stop.ends_with("other.key/media.stop"));
    assert!(session_hints(
        "https://nas.invalid/nas/api",
        &data,
        "/missing",
        "https://nas.invalid:443/missing"
    )
    .is_none());
}

#[test]
fn later_page_can_enrich_metadata_for_an_existing_row() {
    let mut transport = |_: &str, body: &Value, _: &AtomicBool| {
        Ok(if body.get("offset").is_none() {
            json!({"type":"category","items":[{"type":"stream","id":1,"url":"/old"},{"type":"next","request":{}}]})
        } else {
            json!({"type":"category","items":[{"type":"stream","id":1,"url":"/old","description":"Complete description","img":"/cover.jpg"}]})
        }.to_string())
    };
    let mut rows = Vec::new();
    let mut catalog = Catalog::default();
    visit_with(
        &OffFrame::for_test(),
        &profile(),
        &AtomicBool::new(false),
        &mut |batch| {
            catalog.merge(Catalog { entries: batch.clone(), hierarchy: None }).unwrap();
            rows.extend(batch);
            Ok(())
        },
        &mut transport,
    )
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, rows[1].id);
    assert_eq!(rows[1].description, "Complete description");
    assert_eq!(rows[1].poster, "https://portal.invalid:443/cover.jpg");
    assert_eq!(catalog.entries.len(), 1);
    assert_eq!(catalog.entries[0], rows[1]);
    assert_eq!(
        rows[1].media,
        MediaLocator::Http {
            url: "https://portal.invalid:443/old".into()
        }
    );
}

#[test]
fn later_page_cannot_replace_an_existing_row_locator() {
    let mut calls = 0;
    let mut transport = |_: &str, body: &Value, _: &AtomicBool| {
        calls += 1;
        Ok(if body.get("offset").is_none() {
            json!({"type":"category","items":[{"type":"stream","id":1,"url":"/old"},{"type":"next","request":{}}]})
        } else {
            json!({"type":"category","items":[{"type":"stream","id":1,"url":"/fresh","description":"Complete description","img":"/cover.jpg"}]})
        }.to_string())
    };
    let mut rows = Vec::new();
    let result = visit_with(
        &OffFrame::for_test(),
        &profile(),
        &AtomicBool::new(false),
        &mut |batch| { rows.extend(batch); Ok(()) },
        &mut transport,
    );
    assert_eq!(result, Err("VPortal catalogue contains conflicting rows"));
    assert_eq!(calls, 2);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].description.is_empty());
    assert!(rows[0].poster.is_empty());
    assert_eq!(rows[0].media, MediaLocator::Http {
        url: "https://portal.invalid:443/old".into()
    });
}

#[test]
fn expired_lookup_deadline_stops_before_empty_catalogue_traversal() {
    let worker = OffFrame::for_test();
    let result = visit_with_deadline(
        &worker,
        &profile(),
        &AtomicBool::new(false),
        &mut |_| panic!("expired lookup emitted"),
        Some(Deadline::after(&worker, std::time::Duration::ZERO)),
        &mut |_, _, _| panic!("expired lookup performed IO"),
    );
    assert_eq!(result, Err("VPortal catalogue lookup timed out"));
}
