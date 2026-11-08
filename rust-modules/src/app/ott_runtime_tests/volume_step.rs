//! Real settings admission/editor dispatch with synthetic Saved completion.
//! The existing revoked writer permit prevents access to protected storage.
use super::*;
use plx_machine::machine::{InputEvent, InputKind, Key, Source, TextEdit, UiMode};

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
    handle_action(&mut runtime, app, OttAction::EditSetting("volumeStep".into())).unwrap();
    let session = runtime.edit_session.clone().unwrap();
    app.ott = Some(runtime);
    settle_pages(app);
    session
}
fn visible(app: &App) -> bool {
    app.pages.nav.top_page().unwrap().inst.as_ref().unwrap().screen
        .as_any().unwrap().downcast_ref::<plx_screens::ott::OttScreen>()
        .unwrap().handles_private_text()
}
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
    assert!(actions.next().is_none());
    action
}
fn query(app: &mut App, id: u64, params: Value) -> Option<Value> {
    let mut runtime = app.ott.take().unwrap();
    let now = runtime.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{id:032x}"),"action":"player_settings","params":params,"expires_at":1020
    }]}).to_string();
    let mut requests = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(requests.len(), 1);
    let inv = requests.pop().unwrap();
    let result = match super::super::input_policy::remote(&mut runtime, &inv) {
        Ok(()) => commands::command(&mut runtime, app, &inv),
        Err(error) => Some(json!({"status":"rejected","data":{"error":error}})),
    };
    app.ott = Some(runtime);
    result
}
fn drain_refused_writer(app: &mut App) -> Job {
    let job = app.ott.as_ref().unwrap().rx.recv_timeout(Duration::from_secs(1))
        .expect("the revoked writer must settle");
    assert!(matches!(&job, Job::Saved(Err(_), _, false, _)));
    job
}
fn settle_saved(app: &mut App, job: Job) {
    app.ott.as_ref().unwrap().tx.send(job).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(app);
    settle_pages(app);
}

#[test]
fn volume_step_default_row_and_cancel_preserve_state_in_both_modes() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut app = mounted();
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved = runtime.interface_mode_candidate(mode);
        app.bridge.apply_ui_mode(mode);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.volume_step(), 5);
        let opener = runtime.view().rows.into_iter().find(|row| row.id == "settings.open:interface").unwrap();
        assert!(opener.enabled);
        assert_eq!(opener.action, OttAction::EditSetting("settings.open:interface".into()));
        let before = serde_json::to_value(&runtime.saved).unwrap();
        let mut runtime = app.ott.take().unwrap();
        runtime.reply = app.pages.top_page();
        handle_action(&mut runtime, &mut app, opener.action).unwrap();
        let row = runtime.view().rows.into_iter().find(|row| matches!(&row.action,
            OttAction::EditSetting(key) if key.ends_with(":pick:volumeStep"))).unwrap();
        assert!(row.enabled);
        assert_eq!(row.detail, "5%");
        handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        app.ott = Some(runtime);
        // Preserve the independent scalar-editor and cancellation contract.
        let editor = open(&mut app);
        assert!(visible(&app));
        let tick = next_tick(&mut app);
        let edits = [TextEdit::Clear, TextEdit::Commit("9".into())].into_iter()
            .map(|edit| InputEvent { at: tick, source: Source::Script, kind: InputKind::Text(edit) })
            .collect();
        bridge::frame(&mut app.pages, &mut app.bridge, tick, edits);
        let mut runtime = app.ott.take().unwrap();
        handle_action(&mut runtime, &mut app, OttAction::CancelSetting { session: editor.id }).unwrap();
        assert!(runtime.edit_session.is_none());
        assert!(!runtime.saving);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        app.ott = Some(runtime);
    }
}

#[test]
fn volume_step_local_save_waits_for_saved_failure_keeps_old_value_success_reopens() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    for success in [false, true] {
        let mut app = mounted();
        let editor = open(&mut app);
        let action = stage_editor_save(&mut app, &editor, "10");
        let mut runtime = app.ott.take().unwrap();
        let candidate = runtime.volume_step_candidate("10", editor.id).unwrap();
        let before_provider = runtime.provider;
        let before_playback = runtime.playback_epoch;
        handle_action(&mut runtime, &mut app, action.clone()).unwrap();
        assert_eq!(runtime.volume_step(), 5, "no optimistic update");
        assert!(runtime.saving);
        assert!(matches!(runtime.save_navigation, SaveNavigation::Preserve));
        assert!(runtime.pending_editor_save.as_ref() == Some(&editor));
        handle_action(&mut runtime, &mut app, action).unwrap();
        assert!(runtime.pending_editor_save.as_ref() == Some(&editor), "duplicate Submit must keep pending ownership");
        app.ott = Some(runtime);
        let failed = drain_refused_writer(&mut app);
        assert!(app.ott.as_ref().unwrap().rx.try_recv().is_err(), "no duplicate writer");
        let job = if success {
            let Job::Saved(_, invocation, reload, response) = failed else { unreachable!() };
            Job::Saved(Ok(candidate), invocation, reload, response)
        } else { failed };
        settle_saved(&mut app, job);
        let runtime = app.ott.as_ref().unwrap();
        assert!(!runtime.saving);
        assert!(runtime.pending_editor_save.is_none());
        assert_eq!(runtime.volume_step(), if success { 10 } else { 5 });
        assert_eq!(runtime.provider, before_provider);
        assert_eq!(runtime.playback_epoch, before_playback);
        assert_eq!(visible(&app), !success);
        if success {
            let saved = runtime.saved.clone();
            drop(app);
            let mut reopened = mounted();
            reopened.ott.as_mut().unwrap().loaded = false;
            settle_saved(&mut reopened, Job::Loaded(Ok((saved, [1; 16]))));
            assert_eq!(reopened.ott.as_ref().unwrap().volume_step(), 10);
            let editor = open(&mut reopened);
            assert_eq!(editor.key, "volumeStep");
            assert!(visible(&reopened));
        }
    }
}

#[test]
fn volume_step_remote_validation_cas_saved_success_and_readback_use_existing_protocol() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    let mut app = mounted();
    let token = app.ott.as_ref().unwrap().remote.runtime().to_owned();
    let before = query(&mut app, 1, json!({"op":"get","runtime":token,"id":"volumeStep"})).unwrap();
    assert_eq!(before["status"], "ok");
    assert_eq!(before["data"]["setting"]["value"], 5);
    assert_eq!(before["data"]["setting"]["effective_value"], 5);
    for (i, value) in [json!(2), json!(11), json!(-1), json!("7"), json!(true), json!(7.0), Value::Null].into_iter().enumerate() {
        let result = query(&mut app, 2 + i as u64, json!({"op":"set","runtime":token,"id":"volumeStep",
            "context":before["data"]["context"]["token"],"revision":before["data"]["setting"]["revision"],"value":value}));
        if result.is_none() {
            let job = drain_refused_writer(&mut app);
            settle_saved(&mut app, job);
        }
        let result = result.expect("invalid values must not start Saved");
        assert_eq!(result["data"]["reason"], "invalid_value");
        assert_eq!(app.ott.as_ref().unwrap().volume_step(), 5);
    }
    assert!(query(&mut app, 20, json!({"op":"set","runtime":token,"id":"volumeStep",
        "context":before["data"]["context"]["token"],"revision":before["data"]["setting"]["revision"],"value":7})).is_none());
    assert_eq!(app.ott.as_ref().unwrap().volume_step(), 5);
    let Job::Saved(_, Some(invocation), false, response) = drain_refused_writer(&mut app) else {
        panic!("remote Saved must retain its invocation");
    };
    assert_eq!(response["data"]["setting"]["value"], 7);
    assert_eq!(response["data"]["setting"]["effective_value"], 7);
    assert_eq!(response["data"]["persisted"], true);
    let id = invocation.id.clone();
    assert!(app.ott.as_ref().unwrap().remote.response_for_id_for_test(&id).is_none(),
        "success must not escape before Saved");
    // Model the successful durable snapshot at the established Saved seam.
    // The only real writer in this test had a revoked permit and already drained.
    let mut candidate = app.ott.as_ref().unwrap().saved.clone();
    candidate.settings.insert("volumeStep".into(), json!(7));
    let digest = plx_base::sha256::sha256(json!(id).to_string().as_bytes());
    let revision: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    candidate.settings.insert("settingRevision:volumeStep".into(), json!(revision));
    settle_saved(&mut app, Job::Saved(Ok(candidate), Some(invocation), false, response));
    assert_eq!(app.ott.as_ref().unwrap().remote.response_for_id_for_test(&id).unwrap()["status"], "ok");
    let after = query(&mut app, 21, json!({"op":"get","runtime":token,"id":"volumeStep"})).unwrap();
    for field in ["value", "persisted_value", "effective_value"] {
        assert_eq!(after["data"]["setting"][field], 7, "{field}");
    }
    assert_eq!(after["data"]["setting"]["storage"], "stored");
    let replay = query(&mut app, 22, json!({"op":"set","runtime":token,"id":"volumeStep",
        "context":before["data"]["context"]["token"],"revision":before["data"]["setting"]["revision"],"value":3})).unwrap();
    assert_eq!(replay["data"]["reason"], "revision_conflict");
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn volume_step_local_aba_invalidates_a_remote_revision() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    let mut app = mounted();
    let token = app.ott.as_ref().unwrap().remote.runtime().to_owned();
    let before = query(&mut app, 41, json!({"op":"get","runtime":token,"id":"volumeStep"})).unwrap();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved = runtime.volume_step_candidate("10", 1).unwrap();
    runtime.saved = runtime.volume_step_candidate("5", 2).unwrap();
    let replay = query(&mut app, 42, json!({"op":"set","runtime":token,"id":"volumeStep",
        "context":before["data"]["context"]["token"],"revision":before["data"]["setting"]["revision"],"value":3})).unwrap();
    assert_eq!(replay["data"]["reason"], "revision_conflict");
    assert_eq!(app.ott.as_ref().unwrap().volume_step(), 5);
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn volume_step_editor_rejects_invalid_envelopes_without_starting_a_save() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    let mut app = mounted();
    let editor = open(&mut app);
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    for value in ["", "2", "11", "-3", "+5", "5.0", "true", "100"] {
        let mut runtime = app.ott.take().unwrap();
        assert_eq!(handle_action(&mut runtime, &mut app, OttAction::SubmitSetting {
            session: editor.id, key: editor.key.clone(), value: value.into(),
        }), Err("Enter an integer from 3 to 10"));
        assert!(!runtime.saving);
        assert!(runtime.pending_editor_save.is_none());
        assert!(runtime.edit_session.as_ref() == Some(&editor));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        app.ott = Some(runtime);
        settle_pages(&mut app);
        assert!(visible(&app));
    }
}
