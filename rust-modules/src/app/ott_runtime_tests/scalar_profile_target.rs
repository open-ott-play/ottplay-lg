//! Scalar profile drafts use the same target snapshot guard as credential setup.
//! Remote publication is injected through Job::Saved; no provider IO is admitted.
//! The one accepted-write case revokes WritePermit before the real writer starts.
use super::*;
use plx_machine::machine::{InputEvent, InputKind, Key, Source, TextEdit};

const CHANGED: &str = "This profile changed; reopen its setup before saving";

fn profile(id: &str) -> ProviderProfile {
    ProviderProfile {
        id: id.into(),
        name: format!("Fixture {id}"),
        kind: ProviderKind::M3u,
        source: format!("https://fixture.invalid/{id}.m3u"),
        source_alias: id.into(),
        enabled: true,
    }
}

fn fixture(profiles: Vec<ProviderProfile>) -> App {
    let mut app = app_fixture();
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.loaded = true;
        runtime.entropy = Some([3; 16]);
        runtime.write_permit.revoke();
        runtime.saved.profiles.active_profile = profiles.first().map(|profile| profile.id.clone());
        runtime.saved.profiles.profiles = profiles;
        // Hold every provider-work slot while exercising the real Saved consumer.
        runtime.catalog_done.store(false, Ordering::Release);
        runtime.metadata_done.store(false, Ordering::Release);
        runtime.resolver_busy = true;
        runtime.plex_refresh_pending = true;
    }
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    app
}

fn open(app: &mut App, key: &str) -> EditorSession {
    let mut runtime = app.ott.take().unwrap();
    runtime.reply = app.pages.top_page();
    handle_action(&mut runtime, app, OttAction::EditSetting(key.into())).unwrap();
    let request = runtime.edit_session.clone().unwrap();
    app.ott = Some(runtime);
    settle_pages(app);
    assert!(visible(app));
    request
}

fn visible(app: &App) -> bool {
    app.pages.nav.top_page().unwrap().inst.as_ref().unwrap().screen
        .as_any().unwrap().downcast_ref::<plx_screens::ott::OttScreen>()
        .unwrap().handles_private_text()
}

fn submit(app: &mut App, request: &EditorSession, value: &str) -> Result<(), &'static str> {
    let mut runtime = app.ott.take().unwrap();
    let result = handle_action(&mut runtime, app, OttAction::SubmitSetting {
        session: request.id,
        key: request.key.clone(),
        value: value.into(),
    });
    app.ott = Some(runtime);
    settle_pages(app);
    result
}

// Successful submissions must use the form so its pending state owns the reply.
// The direct submit helper remains useful for runtime admission failures above.
fn submit_via_editor(app: &mut App, value: &str) {
    let tick = next_tick(app);
    let input = [TextEdit::Clear, TextEdit::Commit(value.into())]
        .into_iter()
        .map(|edit| InputEvent {
            at: tick,
            source: Source::Script,
            kind: InputKind::Text(edit),
        })
        .collect();
    bridge::frame(&mut app.pages, &mut app.bridge, tick, input);
    runtime_tick(app);
    settle_pages(app);
    for key in [Key::Down, Key::Ok] {
        let tick = next_tick(app);
        bridge::frame(
            &mut app.pages,
            &mut app.bridge,
            tick,
            bridge::script_key(key, tick),
        );
        runtime_tick(app);
        settle_pages(app);
    }
}

fn remote_saved(app: &mut App, saved: Saved) {
    let runtime = app.ott.as_mut().unwrap();
    runtime.saving = true;
    runtime.save_navigation = SaveNavigation::Preserve;
    runtime.save_ui_owner = None;
    runtime.tx.send(Job::Saved(Ok(saved), None, false, json!({"status":"ok"})))
        .unwrap_or_else(|_| panic!("fixture receiver missing"));
    runtime_tick(app);
    settle_pages(app);
}

fn assert_rejected(app: &mut App, request: &EditorSession, value: &str, error: &str) {
    let runtime = app.ott.as_ref().unwrap();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let draft = runtime.edit_profile.clone();
    let original = runtime.edit_profile_original.clone();
    assert_eq!(submit(app, request, value), Err(error));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert_eq!(runtime.edit_profile, draft, "failed submit retains its source draft");
    assert_eq!(runtime.edit_profile_original, original);
    assert!(runtime.edit_session.as_ref() == Some(request));
    assert!(runtime.pending_editor_save.is_none());
    assert!(!runtime.saving, "target refusal occurs before admitting a writer");
    assert!(runtime.rx.try_recv().is_err());
    assert!(visible(app), "the addressed failure keeps the editor open");
}

#[test]
fn scalar_profile_target_remote_change_cannot_be_overwritten() {
    let _serial = plx_base::testlock::serial();
    for (key, value) in [
        ("profile.name", "New local name"),
        ("profile.source", "https://fixture.invalid/local.m3u"),
    ] {
        let mut app = fixture(vec![profile("m3u-1")]);
        let request = open(&mut app, key);
        let mut saved = app.ott.as_ref().unwrap().saved.clone();
        saved.profiles.profiles[0].source = "https://fixture.invalid/remote.m3u".into();
        remote_saved(&mut app, saved);
        assert_rejected(&mut app, &request, value, CHANGED);
    }
}

#[test]
fn scalar_profile_removed_target_cannot_be_recreated() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture(vec![profile("m3u-1")]);
    let request = open(&mut app, "profile.name");
    let mut saved = app.ott.as_ref().unwrap().saved.clone();
    saved.profiles.profiles.clear();
    saved.profiles.active_profile = None;
    remote_saved(&mut app, saved);
    assert_rejected(&mut app, &request, "Do not resurrect", CHANGED);
}

#[test]
fn scalar_profile_new_id_collision_cannot_replace_the_remote_profile() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture(Vec::new());
    let request = open(&mut app, "profile.add");
    let runtime = app.ott.as_ref().unwrap();
    let id = runtime.edit_profile.as_ref().unwrap().id.clone();
    assert!(runtime.edit_profile_original.is_none());
    let mut saved = runtime.saved.clone();
    saved.profiles.profiles.push(profile(&id));
    remote_saved(&mut app, saved);
    assert_rejected(&mut app, &request, "https://fixture.invalid/local.m3u", CHANGED);
}

#[test]
fn scalar_profile_unrelated_change_is_preserved_in_an_admitted_candidate() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture(vec![profile("m3u-1"), profile("m3u-2")]);
    let request = open(&mut app, "profile.name");
    let mut saved = app.ott.as_ref().unwrap().saved.clone();
    saved.profiles.profiles[1].name = "Remote change to another profile".into();
    let other = saved.profiles.profiles[1].clone();
    remote_saved(&mut app, saved);
    let runtime = app.ott.as_mut().unwrap();
    let mut edited = runtime.edit_profile.clone().unwrap();
    edited.name = "Local name".into();
    let candidate = runtime.profile_candidate(edited.clone(), runtime.edit_profile_original.clone().as_ref()).unwrap();
    assert_eq!(candidate.profiles.profiles, vec![edited, other]);
    let before = serde_json::to_value(&runtime.saved).unwrap();
    assert_eq!(submit(&mut app, &request, "Local name"), Ok(()));
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.saving, "unrelated edits must not block admission");
    assert!(runtime.pending_editor_save.as_ref() == Some(&request));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before,
        "admitting a candidate is not early durable publication");
    assert!(matches!(runtime.rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        Job::Saved(Err(_), None, true, _)),
        "the real revoked writer refuses before filesystem IO");
}

#[test]
fn scalar_profile_retained_editor_can_retry_its_own_durable_change() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture(vec![profile("m3u-1")]);
    let request = open(&mut app, "profile.source");
    let runtime = app.ott.as_mut().unwrap();
    let mut edited = runtime.edit_profile.clone().unwrap();
    edited.source = "https://fixture.invalid/own-saved.m3u".into();
    let candidate = runtime.profile_candidate(edited.clone(), runtime.edit_profile_original.clone().as_ref()).unwrap();
    // The writer already published but applying it failed. This is the same
    // completion hook used before acknowledging the retained scalar editor.
    runtime.saved = candidate;
    runtime.complete_profile_editor(&request, true);
    assert_eq!(runtime.edit_profile.as_ref(), Some(&edited));
    assert_eq!(runtime.edit_profile_original.as_ref(), Some(&edited));
    edited.source = "https://fixture.invalid/corrected.m3u".into();
    assert!(runtime.profile_candidate(edited, runtime.edit_profile_original.clone().as_ref()).is_ok());
    assert!(runtime.edit_session.as_ref() == Some(&request));
}

#[test]
fn scalar_profile_old_completion_cannot_rebase_a_replacement_editor() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture(vec![profile("m3u-1")]);
    let old = open(&mut app, "profile.name");
    let current = open(&mut app, "profile.name");
    assert_ne!(old.id, current.id);
    let runtime = app.ott.as_mut().unwrap();
    let draft = runtime.edit_profile.clone();
    let original = runtime.edit_profile_original.clone();
    runtime.saved.profiles.profiles[0].name = "Older editor saved value".into();
    runtime.complete_profile_editor(&old, true);
    assert_eq!(runtime.edit_profile, draft);
    assert_eq!(runtime.edit_profile_original, original);
    assert!(runtime.edit_session.as_ref() == Some(&current));
    assert_rejected(&mut app, &current, "Newer editor must reopen", CHANGED);
}

#[test]
fn scalar_profile_late_capacity_fill_keeps_the_open_draft() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture((1..=14).map(|n| profile(&format!("m3u-{n}"))).collect());
    let request = open(&mut app, "profile.add");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.edit_profile.as_ref().unwrap().id, "m3u-15");
    let mut saved = runtime.saved.clone();
    saved.profiles.profiles.push(profile("m3u-16"));
    remote_saved(&mut app, saved);
    assert_rejected(&mut app, &request, "https://fixture.invalid/local.m3u",
        "This provider already has 15 profiles");
    assert_eq!(app.ott.as_ref().unwrap().saved.profiles.profiles.len(), 15);
}

#[test]
fn structured_profile_late_capacity_fill_keeps_all_entered_fields() {
    let _serial = plx_base::testlock::serial();
    let xtream = |id: &str| {
        let mut value = profile(id);
        value.kind = ProviderKind::Xtream;
        value.source = json!({"server":"https://fixture.invalid/",
            "username":"fixture-user", "password":"fixture-password"}).to_string();
        value
    };
    let mut app = fixture((1..=14).map(|n| xtream(&format!("xtream-{n}"))).collect());
    let mut runtime = app.ott.take().unwrap();
    runtime.provider = ProviderKind::Xtream;
    runtime.reply = app.pages.top_page();
    handle_action(&mut runtime, &mut app, OttAction::EditSetting("profile.add".into())).unwrap();
    let setup_id = runtime.setup.as_ref().unwrap().id;
    assert_eq!(runtime.setup.as_ref().unwrap().draft.profile_id(), "xtream-15");
    app.ott = Some(runtime);
    for (key, value) in [
        ("profile.setup.name", "Retained account"),
        ("profile.setup.server", "https://fixture.invalid/new/"),
        ("profile.setup.username", "entered-user"),
        ("profile.setup.password", "entered-password"),
    ] {
        open(&mut app, key);
        submit_via_editor(&mut app, value);
        assert!(!visible(&app), "field staging returns to the setup overview");
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.edit_session.is_none());
        assert!(!runtime.saving);
        assert_eq!(runtime.setup.as_ref().unwrap().draft.value(key), Ok(value));
    }
    let mut saved = app.ott.as_ref().unwrap().saved.clone();
    saved.profiles.profiles.push(xtream("xtream-16"));
    remote_saved(&mut app, saved);
    let mut runtime = app.ott.take().unwrap();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    assert_eq!(handle_action(&mut runtime, &mut app,
        OttAction::EditSetting("profile.setup.save".into())),
        Err("This provider already has 15 profiles"));
    let setup = runtime.setup.as_ref().unwrap();
    assert_eq!(setup.id, setup_id);
    assert_eq!(setup.draft.value("profile.setup.name"), Ok("Retained account"));
    assert_eq!(setup.draft.value("profile.setup.server"), Ok("https://fixture.invalid/new/"));
    assert_eq!(setup.draft.value("profile.setup.username"), Ok("entered-user"));
    assert_eq!(setup.draft.value("profile.setup.password"), Ok("entered-password"));
    assert_eq!(runtime.page, "profile-setup");
    assert!(runtime.pending_setup_save.is_none());
    assert!(!runtime.saving);
    assert!(runtime.rx.try_recv().is_err());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    app.ott = Some(runtime);
}


// Archive hours must be reachable through the mounted settings UI, not only RPC.
#[test]
fn m3u_archive_hours_has_a_profile_scoped_ui_editor() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture(vec![profile("m3u-1")]);
    let runtime = app.ott.as_mut().unwrap();
    runtime.page = "settings";
    let row = runtime.raw_view().rows.into_iter()
        .find(|row| row.id == "profile.history_hours")
        .expect("Archive hours must be visible for an M3U television profile");
    assert_eq!(row.detail, "0");
    assert!(matches!(row.action, OttAction::EditSetting(ref key) if key == "profile.history_hours"));
    runtime.saved.settings.insert("history:m3u-1".into(), json!(48));
    assert_eq!(runtime.raw_view().rows.into_iter().find(|row| row.id == "profile.history_hours").unwrap().detail, "48");
    runtime.saved.profiles.profiles[0].kind = ProviderKind::VPortal;
    runtime.saved.profiles.profiles[0].source = "portal::[key:fixture]https://fixture.invalid/api".into();
    runtime.provider = ProviderKind::VPortal;
    assert!(runtime.raw_view().rows.iter().all(|row| row.id != "profile.history_hours"));
}

#[test]
fn mounted_archive_hours_open_and_save_survives_its_own_navigation_stamp() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture(vec![profile("m3u-1")]);
    let reply = app.pages.top_page().unwrap();
    let epoch = app.ott.as_ref().unwrap().navigation_epoch;
    app.pages.emit(MachineId::Instance(reply), Fx::App(plx_screens::registry::AppFx::Ott(OttOut::Action {
        reply, action: OttAction::EditSetting("profile.history_hours".into()),
    })));
    let tick = next_tick(&mut app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(&mut app);
    settle_pages(&mut app);
    let request = app.ott.as_ref().unwrap().edit_session.clone()
        .expect("Archive hours must open the normal editor");
    assert_eq!(request.key, "profile.history_hours");
    assert!(app.ott.as_ref().unwrap().navigation_epoch > epoch);
    assert!(visible(&app));
    submit_via_editor(&mut app, "48");
    let runtime = app.ott.as_mut().unwrap();
    assert!(runtime.saving, "opening the editor must not invalidate its own draft");
    assert!(runtime.pending_editor_save.as_ref() == Some(&request));
    assert!(!runtime.saved.settings.contains_key("history:m3u-1"), "wait for durable Saved");
    assert!(matches!(runtime.rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        Job::Saved(Err(_), None, false, _)), "real writer is revoked before any filesystem IO");
}


#[test]
fn archive_hours_validation_and_remote_change_preserve_the_target() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture(vec![profile("m3u-1"), profile("m3u-2")]);
    let request = open(&mut app, "profile.history_hours");
    for bad in ["", "-1", "+1", "1.5", "8761", "NaN"] {
        assert_rejected(&mut app, &request, bad, "Use whole archive hours from 0 to 8760");
    }
    let mut saved = app.ott.as_ref().unwrap().saved.clone();
    saved.settings.insert("history:m3u-2".into(), json!(24));
    remote_saved(&mut app, saved);
    let runtime = app.ott.as_mut().unwrap();
    let candidate = runtime.archive_hours_candidate("48").unwrap();
    assert_eq!(candidate.settings["history:m3u-1"], json!(48));
    assert_eq!(candidate.settings["history:m3u-2"], json!(24));
    let mut changed = runtime.saved.clone();
    changed.settings.insert("history:m3u-1".into(), json!(72));
    remote_saved(&mut app, changed);
    assert_rejected(&mut app, &request, "48", "Archive settings changed; reopen the editor");
}

#[test]
fn archive_hours_source_change_cannot_retarget_an_open_editor() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture(vec![profile("m3u-1"), profile("m3u-2")]);
    let request = open(&mut app, "profile.history_hours");
    let mut saved = app.ott.as_ref().unwrap().saved.clone();
    saved.profiles.active_profile = Some("m3u-2".into());
    remote_saved(&mut app, saved);
    assert_rejected(&mut app, &request, "24", "Archive settings changed; reopen the editor");
}

#[test]
fn archive_hours_change_retires_only_pending_fallback_dependent_playback() {
    let _serial = plx_base::testlock::serial();
    for (declared, changed_profile, expected_retired) in [(0, "m3u-1", true), (0, "m3u-2", false), (86400, "m3u-1", false)] {
        let mut app = fixture(vec![profile("m3u-1"), profile("m3u-2")]);
        let runtime = app.ott.as_mut().unwrap();
        let profile = runtime.active().unwrap().clone();
        let playlist = format!("#EXTM3U\n#EXTINF:-1 catchup=\"shift\" {},News\nhttps://fixture.invalid/live.m3u8", if declared == 0 { String::new() } else { "catchup-days=\"1\"".into() });
        runtime.catalog = plx_data::ott::parse_m3u(&playlist, &profile).unwrap();
        runtime.catalog_complete = true;
        runtime.saved.settings.insert("history:m3u-1".into(), json!(48));
        let entry = runtime.catalog.entries[0].clone();
        let interval = plx_data::ott::archive::ArchiveInterval { start: 1_699_998_000, end: 1_699_999_000 };
        let intent = runtime.archive_intent(&profile, &entry, interval, 1_700_000_000).unwrap();
        let mut request = PlaybackRequest::ordinary(&entry.id, 0);
        request.archive = Some(intent);
        runtime.play_target = Some(request);
        let generation = runtime.play_generation;
        let mut saved = runtime.saved.clone();
        saved.settings.insert(format!("history:{changed_profile}"), json!(24));
        runtime.retire_changed_archive_fallback(&saved);
        assert_eq!(runtime.play_target.is_none(), expected_retired);
        assert_eq!(runtime.play_generation != generation, expected_retired);
    }
}


fn stalker_name_profile(name: &str) -> ProviderProfile {
    ProviderProfile {
        id: "stalker-name-target".into(),
        name: name.into(),
        kind: ProviderKind::Stalker,
        source: json!({"portal":"https://fixture.invalid/c/", "mac":"00:1a:79:00:00:01"}).to_string(),
        source_alias: String::new(),
        enabled: true,
    }
}

fn stalker_name_comparison(app: &mut App, serial: u64, name: &str) -> Value {
    let mut runtime = app.ott.take().unwrap();
    let source: Value = serde_json::from_str(&runtime.saved.profiles.profiles[0].source).unwrap();
    let body = json!({"commands":[], "server_time":1000, "requests":[{
        "id":format!("{serial:032x}"), "action":"settings_compare", "expires_at":1020,
        "params":{"provider":"stalker", "number":3, "settings":{
            "server":source["portal"], "mac":source["mac"], "name":name
        }}
    }]}).to_string();
    let now = runtime.now_ms();
    let mut requests = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(requests.len(), 1);
    let result = commands::command(&mut runtime, app, &requests.remove(0)).unwrap();
    app.ott = Some(runtime);
    assert_eq!(result["status"], "ok");
    result
}

#[test]
fn stalker_ui_rename_retires_old_wire_name_for_scalar_and_structured_edits() {
    let _serial = plx_base::testlock::serial();
    for structured in [false, true] {
        for wire_name in ["Original name", ""] {
            let original = stalker_name_profile(if wire_name.is_empty() { "Stalker 3" } else { wire_name });
            let mut app = fixture(vec![original.clone()]);
            {
                let runtime = app.ott.as_mut().unwrap();
                runtime.provider = ProviderKind::Stalker;
                runtime.saved.settings.insert("slot:stalker-name-target".into(), json!(3));
                runtime.saved.settings.insert("wireName:stalker-name-target".into(), json!(wire_name));
                runtime.saved.settings.insert("wireName:unrelated".into(), json!("Keep this alias"));
            }
            assert_eq!(stalker_name_comparison(&mut app, 1, wire_name)["data"]["matched"], true);
            let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
            let candidate = if structured {
                let mut runtime = app.ott.take().unwrap();
                handle_action(&mut runtime, &mut app, OttAction::EditSetting("profile.source".into())).unwrap();
                runtime.setup.as_mut().unwrap().draft.stage("profile.setup.name", "Renamed locally").unwrap();
                let candidate = runtime.setup_candidate().unwrap().1;
                app.ott = Some(runtime);
                candidate
            } else {
                open(&mut app, "profile.name");
                let runtime = app.ott.as_mut().unwrap();
                let mut profile = runtime.edit_profile.clone().unwrap();
                profile.name = "Renamed locally".into();
                let original = runtime.edit_profile_original.clone();
                runtime.profile_candidate(profile, original.as_ref()).unwrap()
            };
            assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), before,
                "preparing a candidate does not publish it");
            assert!(!candidate.settings.contains_key("wireName:stalker-name-target"));
            assert_eq!(candidate.settings["wireName:unrelated"], "Keep this alias");
            assert_eq!(candidate.profiles.profiles[0].source, original.source);
            assert_eq!(candidate.profiles.profiles[0].name, "Renamed locally");
            assert!(!app.ott.as_ref().unwrap().saving, "no writer is started by candidate preparation");
            // Model durable publication; this test does not execute protected storage.
            app.ott.as_mut().unwrap().saved = candidate;
            assert_eq!(stalker_name_comparison(&mut app, 2, "Renamed locally")["data"]["matched"], true);
            assert_eq!(stalker_name_comparison(&mut app, 3, wire_name)["data"]["matched"], false);
        }
    }
}

#[test]
fn stalker_ui_source_or_noop_edit_keeps_the_empty_wire_name_contract() {
    let _serial = plx_base::testlock::serial();
    for change_source in [false, true] {
        let original = stalker_name_profile("Stalker 3");
        let mut app = fixture(vec![original.clone()]);
        let runtime = app.ott.as_mut().unwrap();
        runtime.provider = ProviderKind::Stalker;
        runtime.saved.settings.insert("slot:stalker-name-target".into(), json!(3));
        runtime.saved.settings.insert("wireName:stalker-name-target".into(), json!(""));
        runtime.open_setup(original, false).unwrap();
        runtime.setup.as_mut().unwrap().draft.stage("profile.setup.name", "Stalker 3").unwrap();
        if change_source {
            runtime.setup.as_mut().unwrap().draft.stage("profile.setup.portal", "https://other.fixture.invalid/c/").unwrap();
        }
        let candidate = runtime.setup_candidate().unwrap().1;
        assert_eq!(candidate.settings["wireName:stalker-name-target"], "");
        assert_eq!(candidate.profiles.profiles[0].name, "Stalker 3");
        runtime.saved = candidate;
        assert_eq!(stalker_name_comparison(&mut app, 1, "")["data"]["matched"], true);
        assert_eq!(stalker_name_comparison(&mut app, 2, "Stalker 3")["data"]["matched"], false);
    }
}

#[test]
fn stale_stalker_name_candidate_cannot_remove_a_newer_wire_alias() {
    let _serial = plx_base::testlock::serial();
    let original = stalker_name_profile("Original name");
    let mut app = fixture(vec![original.clone()]);
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.settings.insert("wireName:stalker-name-target".into(), json!("New remote name"));
    runtime.saved.profiles.profiles[0].name = "New remote name".into();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let mut edited = original.clone();
    edited.name = "Stale local name".into();
    assert_eq!(runtime.profile_candidate(edited, Some(&original)).err(), Some(CHANGED));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(!runtime.saving);
}
