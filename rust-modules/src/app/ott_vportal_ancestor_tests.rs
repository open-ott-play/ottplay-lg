//! Injected transport exercises the actual typed catalogue visitor; no socket or worker.
use super::*;
use serde_json::json;
use std::cell::{Cell, RefCell};

fn profile() -> ProviderProfile {
    ProviderProfile {
        id: "ancestor-adapter".into(), name: "Ancestor adapter".into(),
        kind: ProviderKind::VPortal,
        source: "portal::[key:fixture]https://portal.invalid/api".into(),
        source_alias: String::new(), enabled: true,
    }
}

#[test]
fn typed_vportal_empty_folder_reaches_catalogue_without_media() {
    let calls = Cell::new(0);
    let mut fetch = |_: &str, body: &Value, _: &AtomicBool| {
        calls.set(calls.get() + 1);
        assert_eq!(body["key"], "fixture");
        Ok(json!({"type":"videoportal","items":[
            {"type":"category","id":"empty","title":"Empty","items":[]}
        ]}).to_string())
    };
    let mut catalog = Catalog::default();
    let mut events = 0;
    visit_catalog_with_deadline(&OffFrame::for_test(), &profile(), &AtomicBool::new(false),
        &mut |batch| {
            events += 1;
            assert!(batch.entries.is_empty());
            catalog.merge(batch).expect("valid typed batch");
            Ok(())
        }, None, &mut fetch).unwrap();
    assert_eq!(calls.get(), 1);
    assert!(events > 0, "a real empty folder must not be dropped with empty rows");
    assert!(catalog.entries.is_empty());
    assert_eq!(catalog.categories(), vec!["Empty"]);
    let choice = &catalog.category_choices()[0];
    assert!(catalog.category_indices(&choice.id).is_empty());
}

#[test]
fn typed_vportal_first_media_precedes_later_fetch_and_keeps_ancestor_membership() {
    let events = RefCell::new(Vec::new());
    let mut fetch = |_: &str, body: &Value, _: &AtomicBool| {
        let response = match body.get("folder").and_then(Value::as_str) {
            None => {
                events.borrow_mut().push("root");
                json!({"type":"videoportal","items":[
                    {"type":"category","id":"a","title":"A","items":[
                        {"type":"category","id":"b","title":"B","request":{"folder":"first"}}
                    ]},
                    {"type":"category","id":"later","title":"Later","request":{"folder":"later"}}
                ]})
            },
            Some("first") => {
                events.borrow_mut().push("first-fetch");
                json!({"type":"category","items":[
                    {"type":"stream","id":1,"title":"One","url":"/one"}
                ]})
            },
            Some("later") => {
                events.borrow_mut().push("later-fetch");
                json!({"type":"category","items":[]})
            },
            _ => panic!("unexpected folder request"),
        };
        Ok(response.to_string())
    };
    let mut catalog = Catalog::default();
    visit_catalog_with_deadline(&OffFrame::for_test(), &profile(), &AtomicBool::new(false),
        &mut |batch| {
            if !batch.entries.is_empty() { events.borrow_mut().push("media"); }
            catalog.merge(batch).expect("valid typed batch");
            Ok(())
        }, None, &mut fetch).unwrap();
    let events = events.borrow();
    assert!(events.iter().position(|event| *event == "media").unwrap()
        < events.iter().position(|event| *event == "later-fetch").unwrap());
    assert_eq!(catalog.entries.len(), 1);
    for label in ["A", "A / B"] {
        let choice = catalog.category_choices().into_iter().find(|choice| choice.label == label).unwrap();
        assert_eq!(catalog.category_indices(&choice.id), vec![0]);
    }
}

#[test]
fn typed_vportal_repeated_leaf_adds_membership_without_another_raw_row() {
    let calls = Cell::new(0);
    let mut fetch = |_: &str, body: &Value, _: &AtomicBool| {
        calls.set(calls.get() + 1);
        Ok(if body.get("folder").is_none() {
            json!({"type":"videoportal","items":[
                {"type":"category","id":"literal","title":"A / B","request":{"folder":"literal"}},
                {"type":"category","id":"a","title":"A","items":[
                    {"type":"category","id":"nested","title":"B","request":{"folder":"nested"}}
                ]}
            ]})
        } else {
            json!({"type":"category","items":[
                {"type":"stream","id":1,"title":"One","url":"/one"}
            ]})
        }.to_string())
    };
    let mut catalog = Catalog::default();
    let mut emitted_rows = 0;
    let mut membership_only = false;
    visit_catalog_with_deadline(&OffFrame::for_test(), &profile(), &AtomicBool::new(false),
        &mut |batch| {
            emitted_rows += batch.entries.len();
            membership_only |= batch.entries.is_empty() && batch.hierarchy.as_ref()
                .is_some_and(|graph| !graph.memberships.is_empty());
            catalog.merge(batch).expect("valid typed batch");
            Ok(())
        }, None, &mut fetch).unwrap();
    assert_eq!(calls.get(), 3);
    assert_eq!(emitted_rows, 1);
    assert_eq!(catalog.entries.len(), 1);
    assert!(membership_only, "the second structural owner must survive row deduplication");
    let choices: Vec<_> = catalog.category_choices().into_iter()
        .filter(|choice| choice.label == "A / B").collect();
    assert_eq!(choices.len(), 2);
    assert_ne!(choices[0].id, choices[1].id);
    for choice in choices { assert_eq!(catalog.category_indices(&choice.id), vec![0]); }
}

#[test]
fn typed_vportal_cancelled_fetch_emits_neither_nodes_nor_rows() {
    let cancel = AtomicBool::new(false);
    let calls = Cell::new(0);
    let mut fetch = |_: &str, _: &Value, flag: &AtomicBool| {
        calls.set(calls.get() + 1);
        flag.store(true, Ordering::Release);
        Ok(json!({"type":"videoportal","items":[
            {"type":"category","title":"A","items":[
                {"type":"stream","id":1,"title":"One","url":"/one"}
            ]}
        ]}).to_string())
    };
    let result = visit_catalog_with_deadline(&OffFrame::for_test(), &profile(), &cancel,
        &mut |_| panic!("cancelled completion must not publish a typed batch"), None, &mut fetch);
    assert_eq!(result, Err(CANCELLED));
    assert_eq!(calls.get(), 1);
}

#[test]
fn typed_vportal_cancel_from_node_callback_stops_rows_and_future_fetches() {
    let cancel = AtomicBool::new(false);
    let calls = Cell::new(0);
    let mut fetch = |_: &str, _: &Value, _: &AtomicBool| {
        calls.set(calls.get() + 1);
        Ok(json!({"type":"videoportal","items":[
            {"type":"category","title":"A","items":[
                {"type":"stream","id":1,"title":"One","url":"/one"}
            ]},
            {"type":"category","title":"Later","request":{"folder":"later"}}
        ]}).to_string())
    };
    let mut events = 0;
    let result = visit_catalog_with_deadline(&OffFrame::for_test(), &profile(), &cancel,
        &mut |batch| {
            events += 1;
            assert!(batch.entries.is_empty());
            assert!(batch.hierarchy.as_ref().is_some_and(|graph| !graph.nodes.is_empty()));
            cancel.store(true, Ordering::Release);
            Ok(())
        }, None, &mut fetch);
    assert_eq!(result, Err(CANCELLED));
    assert_eq!(events, 1);
    assert_eq!(calls.get(), 1);
}

#[test]
fn typed_vportal_conflicting_page_cannot_publish_new_folder_descriptors() {
    let calls = Cell::new(0);
    let mut fetch = |_: &str, body: &Value, _: &AtomicBool| {
        calls.set(calls.get() + 1);
        Ok(if body.get("offset").is_none() {
            json!({"type":"videoportal","items":[
                {"type":"stream","id":1,"title":"One","url":"/one"},
                {"type":"next","request":{"offset":1}}
            ]})
        } else {
            json!({"type":"videoportal","items":[
                {"type":"category","id":"new","title":"Must not publish","items":[]},
                {"type":"stream","id":1,"title":"One","url":"/conflicting"}
            ]})
        }.to_string())
    };
    let mut catalog = Catalog::default();
    let result = visit_catalog_with_deadline(&OffFrame::for_test(), &profile(), &AtomicBool::new(false),
        &mut |batch| { catalog.merge(batch).expect("valid earlier batch"); Ok(()) }, None, &mut fetch);
    assert_eq!(result, Err("VPortal catalogue contains conflicting rows"));
    assert_eq!(calls.get(), 2);
    assert_eq!(catalog.entries.len(), 1);
    assert!(matches!(&catalog.entries[0].media, MediaLocator::Http { url } if url.ends_with("/one")));
    assert!(catalog.categories().is_empty());
}
