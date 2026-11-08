//! Addressed editor completion at the real screen/runtime boundary.
//! Persistence outcomes are injected; no protected state or network is opened.
use super::*;
use plx_machine::machine::Key;

fn mount_editor(app: &mut App) -> EditorSession {
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(app);
    runtime_tick(app);
    settle_pages(app);
    open_editor(app)
}

fn open_editor(app: &mut App) -> EditorSession {
    let mut runtime = app.ott.take().unwrap();
    runtime.reply = app.pages.top_page();
    handle_action(&mut runtime, app, OttAction::EditSetting("profile.add".into()))
        .expect("source editor opens without touching storage");
    let session = runtime.edit_session.clone().unwrap();
    app.ott = Some(runtime);
    settle_pages(app);
    assert!(editor_visible(app));
    session
}

fn editor_visible(app: &App) -> bool {
    app.pages.nav.top_page().unwrap().inst.as_ref().unwrap().screen
        .as_any().unwrap().downcast_ref::<plx_screens::ott::OttScreen>()
        .unwrap().handles_private_text()
}

fn submit(app: &mut App, session: u64, key: &str, value: &str) -> Result<(), &'static str> {
    let mut runtime = app.ott.take().unwrap();
    let result = handle_action(&mut runtime, app, OttAction::SubmitSetting {
        session, key: key.into(), value: value.into(),
    });
    app.ott = Some(runtime);
    settle_pages(app);
    result
}

fn press(app: &mut App, key: Key) {
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(key, tick));
    runtime_tick(app);
    settle_pages(app);
}

// Represent an already admitted save without invoking its real storage worker.
// The screen's Save sends a duplicate for that session, which must remain pending.
fn pending_save(app: &mut App, session: &EditorSession) -> Saved {
    let candidate = {
        let runtime = app.ott.as_mut().unwrap();
        runtime.loaded = true;
        runtime.saving = true;
        runtime.save_playback_epoch = runtime.playback_epoch;
        runtime.save_navigation = SaveNavigation::Activate(runtime.navigation_epoch);
        runtime.pending_editor_save = Some(session.clone());
        runtime.catalog_done.store(false, Ordering::Release);
        runtime.resolver_busy = true;
        runtime.plex_refresh_pending = true;
        let mut candidate = runtime.saved.clone();
        let mut profile = runtime.edit_profile.clone().unwrap();
        profile.source = "https://media.invalid/fixture.m3u".into();
        candidate.profiles.active_profile = Some(profile.id.clone());
        candidate.profiles.profiles.push(profile);
        candidate
    };
    press(app, Key::Down);
    press(app, Key::Ok);
    assert!(editor_visible(app), "Save cannot close before a durable outcome");
    assert!(app.ott.as_ref().unwrap().pending_editor_save.is_some());
    assert!(app.ott.as_ref().unwrap().saved.profiles.profiles.is_empty());
    candidate
}

#[test]
fn editor_invalid_and_unready_source_do_not_publish_profile() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready_app_fixture();
    let session = mount_editor(&mut app);
    // The editor was admitted while ready; loss of verified state must reject
    // both malformed and otherwise valid submissions without publishing them.
    app.ott.as_mut().unwrap().loaded = false;
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    for value in ["not-a-playlist-url", "https://media.invalid/fixture.m3u"] {
        assert!(submit(&mut app, session.id, &session.key, value).is_err());
        assert!(editor_visible(&app), "validation/unready failure retains the editor");
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.edit_session.as_ref() == Some(&session));
        assert!(runtime.pending_editor_save.is_none());
        assert!(!runtime.saving);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    }
}

#[test]
fn editor_submit_requires_current_positive_session_key_and_owner() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready_app_fixture();
    let first = mount_editor(&mut app);
    let current = open_editor(&mut app);
    assert!(current.id > first.id);
    for (session, key) in [(0, current.key.as_str()), (first.id, first.key.as_str()),
        (current.id, "profile.name")]
    {
        assert!(submit(&mut app, session, key, "https://media.invalid/a.m3u").is_err());
    }
    app.ott.as_mut().unwrap().reply = Some(InstanceId(current.owner.0.wrapping_add(1)));
    assert!(submit(&mut app, current.id, &current.key, "https://media.invalid/a.m3u").is_err());
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.edit_session.as_ref() == Some(&current));
    assert!(runtime.saved.profiles.profiles.is_empty());
    assert!(runtime.pending_editor_save.is_none());
    assert!(editor_visible(&app));
}

#[test]
fn wrong_key_submission_cannot_acknowledge_an_inflight_editor_save() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready_app_fixture();
    let session = mount_editor(&mut app);
    // Model an admitted writer, then use real screen input to enter its pending state.
    // No storage worker or provider is started by this fixture.
    let saved = pending_save(&mut app, &session);
    let queued_before = app.pages.queued();
    let mut runtime = app.ott.take().unwrap();
    let result = handle_action(&mut runtime, &mut app, OttAction::SubmitSetting {
        session: session.id,
        key: "profile.name".into(),
        value: "Wrong field".into(),
    });
    assert_eq!(result, Err("Editor session is no longer active"));
    assert!(runtime.edit_session.as_ref() == Some(&session));
    assert!(runtime.pending_editor_save.as_ref() == Some(&session));
    assert!(runtime.saving);
    assert!(runtime.saved.profiles.profiles.is_empty());
    app.ott = Some(runtime);
    assert_eq!(app.pages.queued(), queued_before,
        "a wrong-key envelope must not enqueue a result that unlocks the valid pending editor");
    settle_pages(&mut app);
    assert!(editor_visible(&app));
    // The original writer still owns the only completion allowed to close this editor.
    app.ott.as_ref().unwrap().tx.send(Job::Saved(Ok(saved), None, true, json!({"status":"ok"})))
        .unwrap_or_else(|_| panic!("fixture reply receiver disappeared"));
    runtime_tick(&mut app);
    settle_pages(&mut app);
    assert!(app.ott.as_ref().unwrap().pending_editor_save.is_none());
    assert_eq!(app.ott.as_ref().unwrap().saved.profiles.profiles.len(), 1);
    assert!(!editor_visible(&app));
}

#[test]
fn editor_cancel_and_provider_navigation_reject_a_queued_old_submit() {
    let _serial = plx_base::testlock::serial();
    for navigate in [false, true] {
        let mut app = ready_app_fixture();
        let session = mount_editor(&mut app);
        let mut runtime = app.ott.take().unwrap();
        let action = if navigate { OttAction::SelectProvider("xtream".into()) }
            else { OttAction::CancelSetting { session: session.id } };
        handle_action(&mut runtime, &mut app, action).unwrap();
        assert!(runtime.edit_session.is_none());
        assert!(runtime.edit_profile.is_none());
        app.ott = Some(runtime);
        assert_eq!(submit(&mut app, session.id, &session.key, "https://media.invalid/queued.m3u"),
            Err("Editor session is no longer active"));
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.saved.profiles.profiles.is_empty());
        assert!(!runtime.saving);
        assert!(runtime.pending_editor_save.is_none());
    }
}

#[test]
fn stale_editor_cancel_cannot_invalidate_a_new_editor_session() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready_app_fixture();
    let old = mount_editor(&mut app);
    let current = open_editor(&mut app);
    let mut runtime = app.ott.take().unwrap();
    handle_action(&mut runtime, &mut app, OttAction::CancelSetting { session: old.id }).unwrap();
    assert!(runtime.edit_session.as_ref() == Some(&current));
    app.ott = Some(runtime);
    assert!(editor_visible(&app));
}

#[test]
fn editor_closes_only_after_its_durable_saved_result() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready_app_fixture();
    let session = mount_editor(&mut app);
    let saved = pending_save(&mut app, &session);
    app.ott.as_ref().unwrap().tx.send(Job::Saved(Ok(saved), None, true, json!({"status":"ok"})))
        .unwrap_or_else(|_| panic!("fixture reply receiver disappeared"));
    runtime_tick(&mut app);
    settle_pages(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.pending_editor_save.is_none());
    assert!(runtime.edit_session.is_none());
    assert_eq!(runtime.saved.profiles.profiles.len(), 1);
    assert!(!editor_visible(&app));
}

#[test]
fn editor_failed_and_uncertain_saved_results_keep_the_draft_open() {
    let _serial = plx_base::testlock::serial();
    for uncertain in [false, true] {
        let mut app = ready_app_fixture();
        let session = mount_editor(&mut app);
        let _candidate = pending_save(&mut app, &session);
        let job = if uncertain { Job::Uncertain(None, None) }
            else { Job::Saved(Err("Fixture write refused"), None, false, json!({"status":"ok"})) };
        app.ott.as_ref().unwrap().tx.send(job)
            .unwrap_or_else(|_| panic!("fixture reply receiver disappeared"));
        runtime_tick(&mut app);
        settle_pages(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.pending_editor_save.is_none());
        assert!(runtime.edit_session.as_ref() == Some(&session));
        assert!(!runtime.saving);
        assert_eq!(runtime.persistence_uncertain, uncertain);
        assert!(runtime.saved.profiles.profiles.is_empty());
        assert!(editor_visible(&app));
        press(&mut app, Key::Back);
        assert!(!editor_visible(&app), "failed save keeps Cancel/Back reachable");
    }
}

#[test]
fn older_editor_save_cannot_close_a_reopened_editor() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready_app_fixture();
    let old = mount_editor(&mut app);
    let saved = pending_save(&mut app, &old);
    press(&mut app, Key::Back);
    assert!(!editor_visible(&app));
    let current = open_editor(&mut app);
    assert_ne!(old.id, current.id);
    app.ott.as_ref().unwrap().tx.send(Job::Saved(Ok(saved), None, true, json!({"status":"ok"})))
        .unwrap_or_else(|_| panic!("fixture reply receiver disappeared"));
    runtime_tick(&mut app);
    settle_pages(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.pending_editor_save.is_none());
    assert!(runtime.edit_session.as_ref() == Some(&current));
    assert!(editor_visible(&app), "old success is addressed to the old editor session");
}
