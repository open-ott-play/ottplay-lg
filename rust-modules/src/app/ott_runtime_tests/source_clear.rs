//! Source clearing uses the normal command/write completion path. Protected
//! storage success is modeled after the existing revoked-permit writer refusal.
use super::*;

const PORTAL: &str = "portal::[key:fixture]https://portal.fixture.invalid/api/";

fn setup(kind: ProviderKind, active: bool) -> App {
    let mut app = mounted_ready();
    seed_saved(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    let profile = &mut runtime.saved.profiles.profiles[1];
    profile.kind = kind;
    if kind == ProviderKind::VPortal { profile.source = PORTAL.into(); }
    runtime.saved.profiles.active_profile = active.then(|| "m3u".into());
    runtime.saved.settings.insert("selectedProvider".into(), json!(kind.canonical_id()));
    runtime.saved.settings.insert("slot:m3u".into(), json!(7));
    runtime.saved.settings.insert("history:m3u".into(), json!(72));
    runtime.sync_saved_context();
    runtime.page = "profiles";
    app
}

fn request(runtime: &mut Runtime, serial: u32, action: &str, params: Value) -> Invocation {
    let now = runtime.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{serial:032x}"),"action":action,"params":params,"expires_at":1020
    }]}).to_string();
    let mut received = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(received.len(), 1);
    received.pop().unwrap()
}

fn admit_clear(app: &mut App, kind: ProviderKind, dedicated: bool) -> (Option<Invocation>, bool, Value) {
    let mut runtime = app.ott.take().unwrap();
    let key = if kind == ProviderKind::VPortal { "vportal" } else { "playlist" };
    let mut settings = json!({});
    settings[key] = json!("");
    let (action, params) = if dedicated && kind == ProviderKind::VPortal {
        let params = json!({"provider":"vportal", "runtime":runtime.remote.runtime(),
            "number":7,"settings":settings});
        let query = request(&mut runtime, 1, "settings_compare", params);
        let reply = commands::command(&mut runtime, app, &query).unwrap();
        assert_eq!(reply["status"], "ok");
        settings["profile"] = json!(7);
        ("provider_settings", json!({"provider":"vportal","runtime":runtime.remote.runtime(),
            "context":reply["data"]["context"],"revision":reply["data"]["revision"],"settings":settings}))
    } else if dedicated {
        ("provider_settings", json!({"provider":"m3u","settings":settings}))
    } else {
        ("profile_settings", json!({"number":7,"settings":settings}))
    };
    let invocation = request(&mut runtime, 2, action, params);
    assert!(commands::command(&mut runtime, app, &invocation).is_none(), "clear must admit the existing writer");
    assert!(matches!(runtime.save_navigation, SaveNavigation::Preserve));
    app.ott = Some(runtime);
    let pending = capture_refused_write(app);
    assert!(!pending.1, "clearing a source is not a reload request");
    pending
}

#[test]
fn remote_source_clear_preserves_selection_retires_old_batches_and_never_autoplays() {
    let _serial = plx_base::testlock::serial();
    for kind in [ProviderKind::M3u, ProviderKind::VPortal] {
        for dedicated in [false, true] {
            let mut app = setup(kind, true);
            let old_generation = app.ott.as_ref().unwrap().generation;
            let old_epoch = app.ott.as_ref().unwrap().playback_epoch;
            let pending = admit_clear(&mut app, kind, dedicated);
            let mut cleared = app.ott.as_ref().unwrap().saved.clone();
            cleared.profiles.profiles[1].source.clear();
            cleared.validate().unwrap();
            modeled_saved(&mut app, cleared.clone(), pending);
            let runtime = app.ott.as_ref().unwrap();
            assert_eq!(runtime.saved.profiles, cleared.profiles);
            assert_eq!(runtime.saved.settings["selectedProvider"], cleared.settings["selectedProvider"]);
            assert_eq!(runtime.saved.settings["history:m3u"], 72);
            assert_eq!(runtime.playback_epoch, old_epoch, "clear must not claim decoder ownership");
            assert!(runtime.generation != old_generation);
            assert!(runtime.catalog_next.is_none() && !runtime.catalog_loading);
            assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
            runtime.catalog_tx.send(CatalogEvent::Batch(old_generation,
                Catalog { hierarchy: None, entries: vec![entry("stale")] }))
                .unwrap_or_else(|_| panic!("fixture catalogue receiver disappeared"));
            runtime_tick(&mut app);
            assert!(app.ott.as_ref().unwrap().catalog.entries.is_empty());
            assert!(app.ott.as_ref().unwrap().play_target.is_none());
            // The active slot persists across restart; it cannot start a loader.
            let mut restarted = mounted();
            deliver(&mut restarted, Job::Loaded(Ok((cleared, [1; 16]))));
            let runtime = restarted.ott.as_ref().unwrap();
            assert_eq!(runtime.saved.profiles.active_profile.as_deref(), Some("m3u"));
            assert!(runtime.catalog_next.is_none() && !runtime.catalog_loading);
            assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        }
    }
}

#[test]
fn remote_source_clear_keeps_load_disabled_and_edit_available() {
    let _serial = plx_base::testlock::serial();
    for kind in [ProviderKind::M3u, ProviderKind::VPortal] {
        let mut app = setup(kind, true);
        let pending = admit_clear(&mut app, kind, false);
        let mut cleared = app.ott.as_ref().unwrap().saved.clone();
        cleared.profiles.profiles[1].source.clear();
        modeled_saved(&mut app, cleared, pending);
        let runtime = app.ott.as_mut().unwrap();
        runtime.profile_edit_epoch = 1;
        let rows = profile_edit::rows(runtime, &runtime.saved.profiles.profiles[1]);
        assert!(rows.iter().any(|row| matches!(&row.action, OttAction::SelectProfile(_)) && !row.enabled));
        assert!(rows.iter().any(|row| matches!(&row.action, OttAction::EditProfile { .. }) && row.enabled));
        let mut runtime = app.ott.take().unwrap();
        let invocation = request(&mut runtime, 3, "profile", json!({"number":7}));
        let response = commands::command(&mut runtime, &mut app, &invocation).unwrap();
        assert_eq!(response["status"], "rejected");
        assert!(!runtime.saving);
    }
}

#[test]
fn remote_source_clear_with_no_active_profile_never_activates_a_slot() {
    let _serial = plx_base::testlock::serial();
    for kind in [ProviderKind::M3u, ProviderKind::VPortal] {
        for dedicated in [false, true] {
            let mut app = setup(kind, false);
            let before = app.ott.as_ref().unwrap().saved.clone();
            let _pending = admit_clear(&mut app, kind, dedicated);
            let runtime = app.ott.as_ref().unwrap();
            assert_eq!(runtime.saved.profiles, before.profiles);
            assert!(runtime.saved.profiles.active_profile.is_none());
            assert!(runtime.catalog_next.is_none() && runtime.resolver_next.is_none());
        }
    }
}

#[test]
fn cleared_source_restart_preserves_favorites_view_without_starting_media() {
    let _serial = plx_base::testlock::serial();
    for kind in [ProviderKind::M3u, ProviderKind::VPortal] {
        let app = setup(kind, true);
        let mut cleared = app.ott.as_ref().unwrap().saved.clone();
        cleared.profiles.profiles[1].source.clear();
        cleared.favorites_selected = true;
        let mut restarted = mounted();
        deliver(&mut restarted, Job::Loaded(Ok((cleared, [1; 16]))));
        let runtime = restarted.ott.as_ref().unwrap();
        assert_eq!(runtime.page, "favorites");
        assert!(runtime.saved.favorites_selected);
        assert!(runtime.catalog_next.is_none() && runtime.resolver_next.is_none());
        assert!(runtime.play_target.is_none());
    }
}
