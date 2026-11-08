//! Actual settings editor and Saved settlement with synthetic completion.
//! The only admitted real writer has a revoked permit; it cannot access storage.
use super::*;
use plx_machine::machine::{InputEvent, InputKind, Key, Source, TextEdit};

struct Reset(plx_machine::machine::FontScale);
impl Drop for Reset {
    fn drop(&mut self) { plx_gfx::text::set_ui_typography(self.0); }
}
fn mounted() -> App {
    let mut app = app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.entropy = Some([1; 16]);
    runtime.write_permit.revoke();
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.continuity.stop(runtime.generation);
    runtime.page = "settings";
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    app
}
fn open(app: &mut App) -> EditorSession {
    let mut runtime = app.ott.take().unwrap();
    runtime.reply = app.pages.top_page();
    handle_action(&mut runtime, app, OttAction::EditSetting("uiFontScale".into())).unwrap();
    let session = runtime.edit_session.clone().unwrap();
    app.ott = Some(runtime);
    settle_pages(app);
    session
}
fn submit(app: &mut App, editor: &EditorSession, value: &str) -> Result<(), &'static str> {
    let mut runtime = app.ott.take().unwrap();
    let result = handle_action(&mut runtime, app, OttAction::SubmitSetting {
        session: editor.id, key: editor.key.clone(), value: value.into(),
    });
    app.ott = Some(runtime);
    settle_pages(app);
    result
}
fn visible(app: &App) -> bool {
    app.pages.nav.top_page().unwrap().inst.as_ref().unwrap().screen
        .as_any().unwrap().downcast_ref::<plx_screens::ott::OttScreen>()
        .unwrap().handles_private_text()
}

// Drive the real editor into Pending before modeling the writer completion.
// Calling Runtime directly cannot set the screen's addressed pending state.
fn stage_editor_save(app: &mut App, editor: &EditorSession, value: &str) -> OttAction {
    let tick = next_tick(app);
    let edits = [TextEdit::Clear, TextEdit::Commit(value.into())].into_iter()
        .map(|edit| InputEvent { at: tick, source: Source::Script, kind: InputKind::Text(edit) })
        .collect();
    bridge::frame(&mut app.pages, &mut app.bridge, tick, edits);
    settle_pages(app);
    for key in [Key::Down, Key::Ok] {
        let tick = next_tick(app);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(key, tick));
        settle_pages(app);
    }
    let mut actions = app.bridge.take_ott_reqs().into_iter().filter_map(|out| match out {
        OttOut::Action { action: action @ OttAction::SubmitSetting { .. }, .. } => Some(action),
        _ => None,
    });
    let action = actions.next().expect("real editor Save must submit its draft");
    assert_eq!(action, OttAction::SubmitSetting {
        session: editor.id, key: editor.key.clone(), value: value.into(),
    });
    assert!(actions.next().is_none(), "one press must submit only once");
    action
}

fn query(app: &mut App, id: u64, params: Value) -> Value {
    let mut runtime = app.ott.take().unwrap();
    let now = runtime.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{id:032x}"),"action":"player_settings","params":params,"expires_at":1020
    }]}).to_string();
    let mut requests = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(requests.len(), 1);
    let result = commands::command(&mut runtime, app, &requests.pop().unwrap()).unwrap();
    app.ott = Some(runtime);
    result
}

#[test]
fn local_scale_row_defaults_to_100_and_cancel_never_changes_saved_state() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    let mut app = mounted();
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.font_scale_percent(), 100);
    let opener = runtime.view().rows.into_iter().find(|r| r.id == "settings.open:interface").unwrap();
    assert!(opener.enabled);
    assert_eq!(opener.action, OttAction::EditSetting("settings.open:interface".into()));
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let mut runtime = app.ott.take().unwrap();
    runtime.reply = app.pages.top_page();
    handle_action(&mut runtime, &mut app, opener.action).unwrap();
    let row = runtime.view().rows.into_iter().find(|r| matches!(&r.action,
        OttAction::EditSetting(key) if key.ends_with(":pick:uiFontScale"))).unwrap();
    assert!(row.enabled);
    assert_eq!(row.detail, "100%");
    handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    app.ott = Some(runtime);
    // The dedicated scalar editor remains supported independently of the group draft.
    let editor = open(&mut app);
    assert!(visible(&app));
    let mut runtime = app.ott.take().unwrap();
    handle_action(&mut runtime, &mut app, OttAction::CancelSetting { session: editor.id }).unwrap();
    assert!(runtime.edit_session.is_none());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(!runtime.saving);
    app.ott = Some(runtime);
}

#[test]
fn scale_editor_rejects_invalid_values_and_retains_the_draft() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    let mut app = mounted();
    let editor = open(&mut app);
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    for value in ["", "19", "201", "-20", "+20", "1e2", "20.0", "  ", "9999"] {
        assert_eq!(submit(&mut app, &editor, value), Err("Enter an integer from 20 to 200"));
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.edit_session.as_ref() == Some(&editor));
        assert!(runtime.pending_editor_save.is_none());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert_eq!(app.bridge.ui_font_scale_percent(), 100);
        assert!(visible(&app));
    }
}

#[test]
fn scale_submit_uses_the_sole_writer_and_failure_never_applies_the_draft() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    let mut app = mounted();
    let editor = open(&mut app);
    submit(&mut app, &editor, "200").unwrap();
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.saving);
    assert!(runtime.pending_editor_save.as_ref() == Some(&editor));
    assert!(matches!(runtime.save_navigation, SaveNavigation::Preserve));
    assert_eq!(runtime.font_scale_percent(), 100);
    assert_eq!(app.bridge.ui_font_scale_percent(), 100);
    let job = runtime.rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(&job, Job::Saved(Err(_), None, false, _)));
    runtime.tx.send(job).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    settle_pages(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.saving);
    assert!(runtime.pending_editor_save.is_none());
    assert!(runtime.edit_session.as_ref() == Some(&editor));
    assert_eq!(runtime.font_scale_percent(), 100);
    assert_eq!(app.bridge.ui_font_scale_percent(), 100);
    assert!(visible(&app));
}

#[test]
fn saved_scale_boundaries_apply_without_changing_provider_or_waking_standby() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    for percent in [20, 200] {
        let mut app = mounted();
        let editor = open(&mut app);
        let _submitted = stage_editor_save(&mut app, &editor, &percent.to_string());
        let runtime = app.ott.as_mut().unwrap();
        let before_provider = runtime.provider;
        let before_playback = runtime.playback_epoch;
        let before_navigation = runtime.navigation_epoch;
        let saved = runtime.font_scale_candidate(&percent.to_string(), editor.id).unwrap();
        assert_eq!(runtime.font_scale_percent(), 100, "staging does not publish the draft");
        runtime.saving = true;
        runtime.save_navigation = SaveNavigation::Preserve;
        runtime.save_playback_epoch = runtime.playback_epoch;
        runtime.pending_editor_save = Some(editor.clone());
        crate::app::ott_standby::set(&mut app, true);
        app.ott.as_ref().unwrap().tx.send(Job::Saved(Ok(saved), None, false, json!({"status":"ok"})))
            .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
        runtime_tick(&mut app);
        settle_pages(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.font_scale_percent(), percent);
        assert_eq!(app.bridge.ui_font_scale_percent(), percent);
        assert_eq!(runtime.provider, before_provider);
        assert_eq!(runtime.playback_epoch, before_playback);
        assert_eq!(runtime.navigation_epoch, before_navigation);
        assert!(app.ott_standby);
        assert!(runtime.pending_editor_save.is_none());
        assert!(runtime.edit_session.is_none());
        assert!(!visible(&app));
    }
}

#[test]
fn local_scale_aba_invalidates_an_old_remote_revision() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    let mut app = mounted();
    let token = app.ott.as_ref().unwrap().remote.runtime().to_owned();
    let get = query(&mut app, 101, json!({"op":"get","runtime":token,"id":"uiFontScale"}));
    assert_eq!(get["status"], "ok");
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved = runtime.font_scale_candidate("200", 1).unwrap();
        runtime.saved = runtime.font_scale_candidate("100", 2).unwrap();
    }
    let result = query(&mut app, 102, json!({"op":"set","runtime":token,"id":"uiFontScale",
        "context":get["data"]["context"]["token"],"revision":get["data"]["setting"]["revision"],"value":20}));
    assert_eq!(result["status"], "rejected");
    assert_eq!(result["data"]["reason"], "revision_conflict");
    assert!(!app.ott.as_ref().unwrap().saving);
    assert_eq!(app.ott.as_ref().unwrap().font_scale_percent(), 100);
}

#[test]
fn saved_scale_restores_on_a_fresh_runtime_without_changing_the_default() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    for percent in [20, 200] {
        let mut app = mounted();
        let runtime = app.ott.as_mut().unwrap();
        assert_eq!(runtime.font_scale_percent(), 100);
        let saved = runtime.font_scale_candidate(&percent.to_string(), 1).unwrap();
        // Feed the real initial-load completion to a fresh fixture. This proves
        // restore application, not encrypted filesystem durability.
        drop(app);
        let mut reopened = mounted();
        let runtime = reopened.ott.as_mut().unwrap();
        runtime.loaded = false;
        runtime.tx.send(Job::Loaded(Ok((saved, [1; 16]))))
            .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
        runtime_tick(&mut reopened);
        assert_eq!(reopened.ott.as_ref().unwrap().font_scale_percent(), percent);
        assert_eq!(reopened.bridge.ui_font_scale_percent(), percent);
        assert_eq!(plx_machine::machine::FontScale::DEFAULT.percent(), 100);
    }
}
