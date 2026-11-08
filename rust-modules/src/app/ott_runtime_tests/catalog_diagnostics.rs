//! Catalogue diagnostics through real status ingress and accepted worker events.
//! No source worker, protected writer, network or decoder is started.
use super::*;

const PRIVATE_ERROR: &str = "failed https://private-user:private-password@catalog.invalid/path?token=private-token";

fn fixture() -> Box<App> {
    let mut app = Box::new(app_fixture());
    app.running = true;
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.page = "channels";
    runtime.catalog_loading = true;
    runtime.catalog_complete = false;
    runtime.continuity.suppress_unrequested_start();
    // An active profile is not needed to deliver an already-addressed result.
    // Keeping it absent prevents lifecycle rearming from starting source IO.
    assert!(runtime.catalog_next.is_none());
    app
}

fn finish(app: &mut App, generation: u64, result: Result<(), &'static str>) {
    app.ott.as_ref().unwrap().catalog_tx
        .send(CatalogEvent::Finished(generation, result)).unwrap();
    runtime_tick(app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.catalog_next.is_none());
    assert!(runtime.resolver_next.is_none());
    assert!(runtime.play_target.is_none());
}

fn read(app: &mut App, action: &str, serial: u64) -> Value {
    super::provider_context::read_status(app, action, serial)
}

#[test]
fn catalogue_failure_is_closed_and_distinct_from_empty_success() {
    let _lock = plx_base::testlock::serial();
    let mut app = fixture();
    let generation = app.ott.as_ref().unwrap().generation;
    finish(&mut app, generation, Err(PRIVATE_ERROR));
    assert_eq!(app.ott.as_ref().unwrap().status, PRIVATE_ERROR,
        "the diagnostic must not rewrite the existing UI error");
    let failed = read(&mut app, "status", 1);
    assert_eq!(failed["data"]["catalog"], json!({
        "version":1,"scope":"source","generation":generation,
        "loading":false,"complete":false,"count":0,"error_code":"catalog_failed"
    }));
    assert_eq!(failed["data"]["ready"], true, "legacy ready semantics stay unchanged");
    assert_eq!(failed["data"]["playback"]["state"], "idle");
    for secret in [PRIVATE_ERROR, "private-user", "private-password", "private-token", "catalog.invalid"] {
        assert!(!failed.to_string().contains(secret), "catalogue diagnostics leaked input");
    }
    finish(&mut app, generation, Ok(()));
    let complete = read(&mut app, "state", 2);
    assert_eq!(complete["data"]["catalog"], json!({
        "version":1,"scope":"source","generation":generation,
        "loading":false,"complete":true,"count":0,"error_code":null
    }));
    assert_eq!(complete["data"]["ready"], failed["data"]["ready"]);
    assert_eq!(complete["data"]["playback"], failed["data"]["playback"]);
}

#[test]
fn catalogue_failure_keeps_partial_source_count_and_ignores_ui_status() {
    let _lock = plx_base::testlock::serial();
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.catalog.entries.push(synthetic_entry());
    let generation = runtime.generation;
    finish(&mut app, generation, Err(PRIVATE_ERROR));
    let failed = read(&mut app, "status", 1)["data"]["catalog"].clone();
    assert_eq!(failed["count"], 1);
    assert_eq!(failed["complete"], false);
    assert_eq!(failed["error_code"], "catalog_failed");
    let runtime = app.ott.as_mut().unwrap();
    runtime.status = "A different UI notice".into();
    runtime.page = "favorites";
    runtime.saved.favorites_selected = true;
    assert_eq!(read(&mut app, "status", 2)["data"]["catalog"], failed,
        "scope=source must not mislabel the empty Favorites view as a failed catalogue");
}

#[test]
fn cancel_and_new_load_retire_failure_and_stale_completion_cannot_restore_it() {
    let _lock = plx_base::testlock::serial();
    let mut app = fixture();
    let old = app.ott.as_ref().unwrap().generation;
    finish(&mut app, old, Err(PRIVATE_ERROR));
    app.ott.as_mut().unwrap().cancel_catalog();
    let cancelled = read(&mut app, "status", 1)["data"]["catalog"].clone();
    assert_ne!(cancelled["generation"], json!(old));
    assert_eq!(cancelled["loading"], false);
    assert_eq!(cancelled["complete"], false);
    assert!(cancelled["error_code"].is_null());
    // Exercise the real new-load reset with no profile: no worker can start.
    app.ott.as_mut().unwrap().load_catalog();
    let current = app.ott.as_ref().unwrap().generation;
    assert_ne!(current, old);
    app.ott.as_mut().unwrap().catalog_loading = true;
    finish(&mut app, old, Err(PRIVATE_ERROR));
    let loading = read(&mut app, "status", 2)["data"]["catalog"].clone();
    assert_eq!(loading["generation"], current);
    assert_eq!(loading["loading"], true);
    assert_eq!(loading["complete"], false);
    assert!(loading["error_code"].is_null());
    finish(&mut app, current, Ok(()));
    finish(&mut app, old, Err(PRIVATE_ERROR));
    let complete = read(&mut app, "status", 3)["data"]["catalog"].clone();
    assert_eq!(complete["complete"], true);
    assert!(complete["error_code"].is_null());
}

#[test]
fn malformed_merge_is_attributed_to_new_generation_and_is_not_a_limit() {
    use plx_data::ott::{CatalogError, CategoryHierarchy, CategoryMembership};
    let _lock = plx_base::testlock::serial();
    let mut app = fixture();
    let old = app.ott.as_ref().unwrap().generation;
    let malformed = Catalog { entries: Vec::new(), hierarchy: Some(CategoryHierarchy {
        nodes: Vec::new(), memberships: vec![CategoryMembership {
            entry_id: "missing-entry".into(), category_id: "missing-category".into(),
        }],
    }) };
    assert_eq!(Catalog::default().merge(malformed.clone()), Err(CatalogError::InvalidFormat));
    app.ott.as_ref().unwrap().catalog_tx.send(CatalogEvent::Batch(old, malformed)).unwrap();
    runtime_tick(&mut app);
    let result = read(&mut app, "status", 1)["data"]["catalog"].clone();
    assert_ne!(result["generation"], json!(old));
    assert_eq!(result["error_code"], "catalog_failed");
    assert_eq!(result["loading"], false);
    assert_eq!(result["complete"], false);
    finish(&mut app, old, Ok(()));
    assert_eq!(read(&mut app, "status", 2)["data"]["catalog"], result,
        "retired producer success must not erase the current merge failure");
}

#[test]
fn typed_merge_limit_reports_on_current_generation() {
    use plx_data::ott::{CatalogError, CategoryHierarchy, CategoryNode};
    let _lock = plx_base::testlock::serial();
    let mut app = fixture();
    let old = app.ott.as_ref().unwrap().generation;
    // Each label is valid; the child's fully projected path exceeds 4096 bytes.
    // Two nodes exercise the real bound without a large synthetic catalogue.
    let over_limit = Catalog { entries: Vec::new(), hierarchy: Some(CategoryHierarchy {
        nodes: vec![
            CategoryNode { id: "root".into(), parent: None, label: "r".repeat(4096),
                anonymous_fingerprint: None, source_identity: None },
            CategoryNode { id: "child".into(), parent: Some("root".into()), label: "c".into(),
                anonymous_fingerprint: None, source_identity: None },
        ], memberships: Vec::new(),
    }) };
    assert_eq!(Catalog::default().merge(over_limit.clone()), Err(CatalogError::LimitExceeded));
    app.ott.as_ref().unwrap().catalog_tx.send(CatalogEvent::Batch(old, over_limit)).unwrap();
    runtime_tick(&mut app);
    let current = read(&mut app, "status", 1)["data"]["catalog"].clone();
    assert_ne!(current["generation"], json!(old));
    assert_eq!(current["error_code"], "catalog_limit");
    assert_eq!(current["complete"], false);
    app.ott.as_mut().unwrap().cancel_catalog();
    let retired = read(&mut app, "status", 2)["data"]["catalog"].clone();
    assert!(retired["error_code"].is_null());
}
