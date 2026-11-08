//! Actual mounted editor + remote command admission. Writers use the parent's
//! revoked permit; the real pre-IO refusal is consumed before modeled Saved(Ok).
//! No storage success, provider IO or native playback is claimed by these tests.
use super::*;

fn editor_visible(app: &App) -> bool {
    app.pages.nav.top_page().unwrap().inst.as_ref().unwrap().screen
        .as_any().unwrap().downcast_ref::<plx_screens::ott::OttScreen>()
        .unwrap().handles_private_text()
}
fn setup_app() -> App {
    let mut app = mounted_ready();
    seed_saved(&mut app);
    ui(&mut app, OttAction::SelectProvider("m3u".into()));
    app
}
fn open(app: &mut App, key: &str) -> EditorSession {
    ui(app, OttAction::EditSetting(key.into()));
    assert!(editor_visible(app));
    app.ott.as_ref().unwrap().edit_session.clone().unwrap()
}
fn admit(app: &mut App, action: &str, params: Value) -> (Option<Invocation>, bool, Value) {
    let mut runtime = app.ott.take().unwrap();
    let mut request = runtime.remote.request_for_test(action, params);
    request.deadline_ms = runtime.now_ms() + 10_000;
    assert!(commands::command(&mut runtime, app, &request).is_none(), "writer admitted");
    assert!(runtime.save_ui_owner.is_none());
    app.ott = Some(runtime);
    capture_refused_write(app)
}
fn activated(app: &App) -> Saved {
    let mut saved = app.ott.as_ref().unwrap().saved.clone();
    saved.profiles.active_profile = Some("m3u".into());
    saved.settings.insert("selectedProvider".into(), json!("m3u"));
    saved
}
fn assert_editor(app: &App, session: &EditorSession) {
    assert!(app.ott.as_ref().unwrap().edit_session.as_ref() == Some(session));
    assert!(editor_visible(app));
}

#[test]
fn successful_remote_profile_activation_retires_only_its_captured_source_editor() {
    let _serial = plx_base::testlock::serial();
    for key in ["profile.source", "profile.add"] {
        let mut app = setup_app();
        let _session = open(&mut app, key);
        let pending = admit(&mut app, "profile", json!({"number":1}));
        let saved = activated(&app);
        modeled_saved(&mut app, saved, pending);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.saved.profiles.active_profile.as_deref(), Some("m3u"));
        assert_eq!(runtime.page, "channels");
        assert!(runtime.edit_session.is_none(), "superseded source owner must retire");
        assert!(runtime.edit_profile.is_none());
        assert!(!editor_visible(&app), "mounted unsubmitted editor must close too");
    }
}

#[test]
fn remote_activation_retires_local_source_and_m3u_media_editors() {
    let _serial = plx_base::testlock::serial();
    for media in [false, true] {
        let mut app = setup_app();
        if media {
            let mut runtime = app.ott.take().unwrap();
            let parent = runtime.saved.profiles.profiles.iter().find(|p| p.id == "m3u").unwrap().clone();
            // Existing owner used by EditM3uMediaLink after its context validation.
            runtime.begin_m3u_media(&mut app, parent).unwrap();
            app.ott = Some(runtime);
            settle_pages(&mut app);
        } else {
            ui(&mut app, OttAction::SelectProvider("local".into()));
            open(&mut app, "profile.source");
        }
        assert!(editor_visible(&app));
        let pending = admit(&mut app, "provider", json!({"query":"m3u"}));
        let saved = activated(&app);
        modeled_saved(&mut app, saved, pending);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.edit_session.is_none());
        assert!(runtime.m3u_media_editor.is_none());
        assert!(!editor_visible(&app));
    }
}

#[test]
fn remote_activation_retires_structured_setup_and_its_optional_scalar_editor() {
    let _serial = plx_base::testlock::serial();
    for field in [false, true] {
        let mut app = setup_app();
        ui(&mut app, OttAction::SelectProvider("xtream".into()));
        ui(&mut app, OttAction::EditSetting("profile.add".into()));
        assert!(app.ott.as_ref().unwrap().setup.is_some());
        if field { open(&mut app, "profile.setup.server"); }
        let pending = admit(&mut app, "provider", json!({"query":"m3u"}));
        let saved = activated(&app);
        modeled_saved(&mut app, saved, pending);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.setup.is_none(), "no hidden stale setup after activation");
        assert!(runtime.edit_session.is_none());
        assert!(!editor_visible(&app));
        assert_eq!(runtime.page, "channels");
    }
}

#[test]
fn rejected_or_unapplied_remote_activation_preserves_source_editor() {
    let _serial = plx_base::testlock::serial();
    for apply_failure in [false, true] {
        let mut app = setup_app();
        let session = open(&mut app, "profile.source");
        let (invocation, reload, response) = admit(&mut app, "profile", json!({"number":1}));
        if apply_failure {
            let mut saved = activated(&app);
            // Completion fault: the stored source list cannot be applied.
            saved.settings.insert("nativePlexSources".into(), json!("invalid source list"));
            deliver(&mut app, Job::Saved(Ok(saved), invocation, reload, response));
            assert!(!app.ott.as_ref().unwrap().status.is_empty());
        } else {
            deliver(&mut app, Job::Saved(Err("Injected save refusal"), invocation, reload, response));
        }
        assert_editor(&app, &session);
    }
}

#[test]
fn late_remote_completion_preserves_newer_same_key_editor_and_setup_field() {
    let _serial = plx_base::testlock::serial();
    for structured in [false, true] {
        let mut app = setup_app();
        let key = if structured {
            ui(&mut app, OttAction::SelectProvider("xtream".into()));
            ui(&mut app, OttAction::EditSetting("profile.add".into()));
            "profile.setup.server"
        } else { "profile.source" };
        let old = open(&mut app, key);
        let setup_id = app.ott.as_ref().unwrap().setup.as_ref().map(|s| s.id);
        let pending = admit(&mut app, "provider", json!({"query":"m3u"}));
        let newer = open(&mut app, key);
        assert_ne!(old.id, newer.id);
        let saved = activated(&app);
        modeled_saved(&mut app, saved, pending);
        assert_editor(&app, &newer);
        assert_eq!(app.ott.as_ref().unwrap().setup.as_ref().map(|s| s.id), setup_id);
    }
}

#[test]
fn metadata_only_save_and_snapshot_preserve_unsubmitted_source_draft() {
    let _serial = plx_base::testlock::serial();
    let mut app = setup_app();
    // profile_settings follows the effective Local adapter when Local is active.
    // Keep M3U slot 1 inactive, with its draft open under a supported provider context.
    let runtime = app.ott.as_mut().unwrap();
    let active = synthetic_profile();
    runtime.saved.profiles.active_profile = Some(active.id.clone());
    runtime.saved.profiles.profiles.push(active);
    runtime.saved.settings.insert("selectedProvider".into(), json!("vportal"));
    runtime.sync_saved_context();
    ui(&mut app, OttAction::SelectProvider("m3u".into()));
    let session = open(&mut app, "profile.source");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.edit_profile.as_ref().unwrap().id, "m3u");
    assert_ne!(runtime.saved.profiles.active_profile.as_deref(), Some("m3u"));
    assert_eq!(runtime.effective_provider(), ProviderKind::VPortal);
    app.ott.as_mut().unwrap().status = "New catalogue metadata".into();
    app.ott.as_mut().unwrap().changed();
    runtime_tick(&mut app);
    settle_pages(&mut app);
    assert_editor(&app, &session);
    let pending = admit(&mut app, "profile_settings", json!({"number":1,"settings":{"name":"Renamed"}}));
    assert!(!pending.1, "inactive profile metadata has no activation authority");
    let mut saved = app.ott.as_ref().unwrap().saved.clone();
    saved.profiles.profiles.iter_mut().find(|p| p.id == "m3u").unwrap().name = "Renamed".into();
    modeled_saved(&mut app, saved, pending);
    assert_editor(&app, &session);
}

#[test]
fn remote_activation_preserves_ordinary_setting_and_search_editors() {
    let _serial = plx_base::testlock::serial();
    for search_editor in [false, true] {
        let mut app = setup_app();
        if search_editor {
            let runtime = app.ott.as_mut().unwrap();
            runtime.page = "channels";
            runtime.catalog.entries = vec![entry("searchable")];
            runtime.changed();
            ui(&mut app, OttAction::BeginSearch);
        } else { open(&mut app, "volumeStep"); }
        let session = app.ott.as_ref().unwrap().edit_session.clone().unwrap();
        assert!(editor_visible(&app));
        let pending = admit(&mut app, "profile", json!({"number":1}));
        let saved = activated(&app);
        modeled_saved(&mut app, saved, pending);
        // Search already has its own source-reconciliation lifetime. This
        // retirement must not dismiss its mounted draft or change that policy.
        if search_editor { assert!(editor_visible(&app)); }
        else { assert_editor(&app, &session); }
    }
}

#[test]
fn old_owner_completion_cannot_retire_replacement_owner_source_editor() {
    let _serial = plx_base::testlock::serial();
    let mut app = setup_app();
    let old = open(&mut app, "profile.source");
    let pending = admit(&mut app, "profile", json!({"number":1}));
    bridge::nav_root(&mut app.pages, AppArg::Home);
    settle_pages(&mut app);
    bridge::nav_root(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    let current = open(&mut app, "profile.source");
    assert_ne!(old.owner, current.owner);
    let saved = activated(&app);
    modeled_saved(&mut app, saved, pending);
    assert_editor(&app, &current);
}
