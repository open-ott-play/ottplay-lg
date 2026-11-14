//! Real editor and public preference completion regressions without IO.
//! Writer acknowledgements are injected; isolated-process durable IO is qualified separately.
use super::*;
use plx_machine::machine::{InputEvent, InputKind, Key, Source, TextEdit};

fn open_remote_group(runtime: &mut Runtime) {
    let row = runtime.view().rows.into_iter().find(|row| row.id == "settings.open:remote")
        .expect("Remote control group must be present in Settings");
    assert!(row.enabled);
    assert_eq!(row.action, OttAction::EditSetting("settings.open:remote".into()));
    settings_groups::action(runtime, &row.action).expect("owned group action").unwrap();
}

fn edit_address(app: &mut App) -> EditorSession {
    let mut runtime = app.ott.take().unwrap();
    runtime.reply = app.pages.top_page();
    handle_action(
        &mut runtime,
        app,
        OttAction::EditSetting("remote.address".into()),
    )
    .unwrap();
    let session = runtime.edit_session.clone().unwrap();
    app.ott = Some(runtime);
    settle_pages(app);
    session
}

fn mounted() -> App {
    let mut app = app_fixture();
    app.ott.as_mut().unwrap().loaded = true;
    app.ott.as_ref().unwrap().tx.send(Job::PublicLoaded(Ok(None)))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    assert!(!app.ott.as_ref().unwrap().persistence_uncertain);
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    app
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

// The production branch builds this candidate and starts the sole settings writer.
// Inject its admission here; no test opens the user's protected store.
fn pending_address(app: &mut App, session: &EditorSession, value: &str) -> Saved {
    let action = stage_editor_save(app, session, value);
    let runtime = app.ott.as_mut().unwrap();
    let candidate = remote_settings::candidate(runtime, value).unwrap();
    runtime.loaded = true;
    runtime.saving = true;
    runtime.save_navigation = SaveNavigation::Preserve;
    runtime.save_playback_epoch = runtime.playback_epoch;
    runtime.pending_editor_save = Some(session.clone());
    // Duplicate Save is consumed without a second writer or early acknowledgement.
    let mut runtime = app.ott.take().unwrap();
    handle_action(&mut runtime, app, action).unwrap();
    assert_ne!(runtime.remote.snapshot()["state"], "discovering");
    assert_eq!(remote_settings::address_input(&runtime), "");
    app.ott = Some(runtime);
    settle_pages(app);
    assert!(visible(app));
    candidate
}

fn address_snapshot(candidate: &Saved) -> public_preferences::Snapshot {
    public_preferences::Snapshot {
        value: plx_platform::storage::public_preferences::Preferences {
            remote_address: Some(remote_settings::saved_address(candidate).unwrap().unwrap().into()),
            ..Default::default()
        }, revision: 1, epoch: 1,
    }
}

fn deliver(app: &mut App, job: Job) {
    app.ott.as_ref().unwrap().tx.send(job)
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(app);
    settle_pages(app);
}

// First-run Settings is a content action, so reaching Remote control does not
// require discovering Browse first. These fixtures use the real screen/key path.
fn first_run_key(app: &mut App, key: Key) {
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(key, tick));
    runtime_tick(app);
    settle_pages(app);
}

fn first_run_open_body_action(app: &mut App, action: OttAction) {
    let view = app.ott.as_ref().unwrap().view();
    // Count the complete body order. Without an explicit selection the real
    // screen starts on its first enabled metadata row, not a read-only heading.
    let start = view.selected_row.as_ref()
        .and_then(|id| view.rows.iter().position(|row| row.id == *id))
        .unwrap_or_else(|| view.rows.iter().position(|row| row.enabled).unwrap_or(0));
    let target = view.rows.iter().position(|row| row.enabled && row.action == action)
        .expect("first-run body must expose the requested enabled action");
    let direction = if target >= start { Key::Down } else { Key::Up };
    for _ in 0..target.abs_diff(start) { first_run_key(app, direction); }
    first_run_key(app, Key::Ok);
}

#[test]
fn first_run_settings_row_preserves_provider_and_setup_selection() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    runtime.loaded = true;
    let providers = [ProviderKind::M3u, ProviderKind::Xtream, ProviderKind::Stalker,
        ProviderKind::VPortal, ProviderKind::Local, ProviderKind::Plex];
    for provider in providers {
        runtime.provider = provider;
        runtime.page = "providers";
        let view = runtime.view();
        let choices=view.rows.iter().filter_map(|row|match &row.action{OttAction::SelectProvider(id)=>Some(id.as_str()),_=>None}).collect::<Vec<_>>();
        assert_eq!(&choices[..6], &["m3u","xtream","stalker","vportal","local","plex"]);
        assert_eq!(choices.len(),ProviderKind::all().len());
        assert!(choices.contains(&"edem"));
        for id in plx_data::ott::OperatorId::ALL {assert!(choices.contains(&id.canonical_id()));}
        assert_eq!(view.selected_row.as_deref(), Some(provider.canonical_id()));
        let settings = view.rows.last().unwrap();
        assert_eq!(settings.id, "action/settings");
        assert_eq!(settings.action, OttAction::ShowSettings);
        assert!(settings.enabled);
        assert_eq!(settings.title, plx_platform::i18n::msg::settings_title());
        assert_eq!(view.rows.iter().filter(|row| row.action == OttAction::ShowSettings).count(), 1);

        runtime.page = "profiles";
        let view = runtime.view();
        let first = match provider {
            ProviderKind::Plex => "action/plex.connect",
            ProviderKind::M3u => "m3u-slot/1",
            _ => "action/profile.add",
        };
        assert_eq!(view.rows[0].id, first);
        assert_eq!(view.selected_row.as_deref(), Some(first));
        assert_eq!(view.rows.last().unwrap().action, OttAction::ShowSettings,
            "Local USB/connect rows must remain before the appended Settings row");
    }
}

#[test]
fn first_run_settings_uses_global_enabled_profiles_and_never_pollutes_media_lists() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    runtime.loaded = true;
    runtime.saved.profiles.profiles.push(ProviderProfile {
        id: "disabled-home".into(), name: "Home".into(), kind: ProviderKind::M3u,
        source: "https://fixture.invalid/list.m3u".into(), source_alias: "Home".into(),
        enabled: false,
    });
    for page in ["providers", "profiles"] {
        runtime.page = page;
        runtime.provider = ProviderKind::Plex;
        assert!(runtime.view().rows.iter().any(|row| row.id == "action/settings"),
            "disabled profiles provide no first-run source");
    }
    runtime.saved.profiles.profiles[0].enabled = true;
    for provider in [ProviderKind::M3u, ProviderKind::Xtream, ProviderKind::Stalker,
        ProviderKind::VPortal, ProviderKind::Local, ProviderKind::Plex] {
        runtime.provider = provider;
        for page in ["providers", "profiles"] {
            runtime.page = page;
            assert!(!runtime.view().rows.iter().any(|row| row.id == "action/settings"),
                "any enabled profile ends first-run presentation, even another provider");
        }
    }
    runtime.saved.profiles.profiles.clear();
    for page in ["channels", "categories", "favorites", "settings"] {
        runtime.page = page;
        assert!(!runtime.view().rows.iter().any(|row| row.id == "action/settings"));
    }
}

#[test]
fn first_run_body_settings_reaches_remote_editor_in_classic_and_modern() {
    let _serial = plx_base::testlock::serial();
    for mode in [plx_machine::machine::UiMode::Classic, plx_machine::machine::UiMode::Modern] {
        for through_profiles in [false, true] {
            let mut app = mounted();
            {
                let runtime = app.ott.as_mut().unwrap();
                runtime.saved.settings.insert("interfaceMode".into(), json!(mode.index()));
                runtime.changed();
            }
            app.bridge.apply_ui_mode(mode);
            runtime_tick(&mut app);
            settle_pages(&mut app);
            assert_eq!(app.ott.as_ref().unwrap().page, "providers");
            assert_eq!(app.ott.as_ref().unwrap().view().selected_row.as_deref(), Some("m3u"));
            if through_profiles {
                first_run_key(&mut app, Key::Ok);
                assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
                assert_eq!(app.ott.as_ref().unwrap().view().selected_row.as_deref(),
                    Some(if mode == plx_machine::machine::UiMode::Classic { "m3u-slot/1" } else { "action/profile.add" }));
            }
            let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
            first_run_open_body_action(&mut app, OttAction::ShowSettings);
            assert_eq!(app.ott.as_ref().unwrap().page, "settings",
                "mode={mode:?} through_profiles={through_profiles}");
            first_run_open_body_action(&mut app, OttAction::EditSetting("settings.open:remote".into()));
            first_run_open_body_action(&mut app, OttAction::EditSetting("remote.address".into()));
            let runtime = app.ott.as_ref().unwrap();
            let editor = runtime.edit_session.as_ref().expect("real content action opened an editor");
            assert_eq!(editor.key, "remote.address");
            assert_eq!(Some(editor.owner), app.pages.top_page());
            assert!(visible(&app));
            assert!(runtime.saved.profiles.profiles.is_empty());
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
            assert!(!runtime.saving);
            assert!(runtime.pending_editor_save.is_none());
            assert_eq!(app.bridge.ui_mode(), mode);
        }
    }
}

#[test]
fn first_run_settings_action_preserves_unready_parental_and_kiosk_gates() {
    let _serial = plx_base::testlock::serial();
    for page in ["providers", "profiles"] {
        for gate in ["unloaded", "uncertain", "parental", "strict"] {
            let mut app = mounted();
            let mut runtime = app.ott.take().unwrap();
            runtime.page = page;
            runtime.reply = app.pages.top_page();
            runtime.entropy = Some([7; 16]);
            match gate {
                "unloaded" => runtime.loaded = false,
                "uncertain" => runtime.persistence_uncertain = true,
                "parental" => {
                    // Existing synthetic test PIN record; no derivation or verification runs.
                    runtime.saved.settings.insert("parentPin".into(), json!("pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
                    runtime.saved.settings.insert("psOptions".into(), json!(1));
                }
                "strict" => {
                    let origin = json!({"provider":"m3u","profile":"Home","subprofile":null,"sourceAlias":"Local"});
                    runtime.saved.settings.insert("nativeKiosk".into(), json!({
                        "version":1,"strict":true,"origin":origin,"target":{"kind":"item","entry":{
                            "title":"Fixture channel","origin":origin,
                            "locator":{"kind":"channel","channelId":"fixture-1"}
                        }}
                    }));
                    assert!(matches!(runtime.track_selection_kiosk(),
                        Ok(crate::app::ott_kiosk_policy::State::Locked { strict: true })));
                }
                _ => unreachable!(),
            }
            let action = runtime.view().rows.into_iter().find(|row| row.id == "action/settings")
                .expect("visibility does not bypass the action's admission gate").action;
            let before = serde_json::to_value(&runtime.saved).unwrap();
            let epochs = (runtime.generation, runtime.play_generation, runtime.playback_epoch);
            assert!(handle_action(&mut runtime, &mut app, action).is_err(), "{page} {gate}");
            assert_eq!(runtime.page, page);
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
            assert_eq!((runtime.generation, runtime.play_generation, runtime.playback_epoch), epochs);
            assert!(!runtime.saving);
            assert!(runtime.pending_editor_save.is_none());
            if gate == "parental" {
                assert_eq!(runtime.edit_session.as_ref().map(|session| session.key.as_str()), Some("parental.unlock"));
            } else {
                assert!(runtime.edit_session.is_none());
            }
            app.ott = Some(runtime);
        }
    }
}

#[test]
fn saved_remote_address_survives_new_runtime_and_loaded_reopen_before_pairing() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let first = edit_address(&mut app);
    let _held = app.ott.as_mut().unwrap().remote.hold_transport_for_test();
    let candidate = pending_address(&mut app, &first,
        " https://control.invalid/team/api/webhook/commands/ ");
    deliver(&mut app, Job::PublicSaved(Ok(address_snapshot(&candidate)), None, json!({"status":"ok"})));
    assert!(!visible(&app));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.remote.snapshot()["state"], "discovering");
    assert_eq!(remote_settings::address_input(runtime), "https://control.invalid:443/team");
    assert_eq!(runtime.remote.snapshot()["pairing_code"], "");
    let snapshot = address_snapshot(&runtime.saved);
    let mut saved = runtime.saved.clone();
    saved.settings.remove("nativeRemoteServerAddress");
    drop(app);

    // A fresh Runtime consumes the public load; a later protected load with
    // no address cannot erase it. Neither runtime retains an editor RAM draft.
    let mut restarted = mounted();
    deliver(&mut restarted, Job::PublicLoaded(Ok(Some(snapshot))));
    deliver(&mut restarted, Job::Loaded(Ok((saved, [7; 16]))));
    let session = edit_address(&mut restarted);
    let runtime = restarted.ott.as_ref().unwrap();
    assert_eq!(remote_settings::address_input(runtime), "https://control.invalid:443/team");
    assert_eq!(runtime.remote.address_input(), "");
    assert_ne!(runtime.remote.snapshot()["state"], "discovering");
    assert!(runtime.edit_session.as_ref() == Some(&session));
}

#[test]
fn failed_or_uncertain_remote_address_save_never_starts_discovery() {
    let _serial = plx_base::testlock::serial();
    for uncertain in [false, true] {
        let mut app = mounted();
        let session = edit_address(&mut app);
        let _candidate = pending_address(&mut app, &session, "https://control.invalid/new");
        let failure = if uncertain { public_preferences::WriteFailure::Uncertain }
            else { public_preferences::WriteFailure::Rejected(private::Error::Revoked) };
        let job = Job::PublicSaved(Err(failure), None, json!({"status":"ok"}));
        deliver(&mut app, job);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(remote_settings::address_input(runtime), "");
        assert_ne!(runtime.remote.snapshot()["state"], "discovering");
        assert!(runtime.edit_session.as_ref() == Some(&session));
        assert!(visible(&app));
        assert!(!runtime.persistence_uncertain, "public uncertainty does not forge private write evidence");
        assert_eq!(public_preferences::write_refusal(runtime).is_some(), uncertain,
            "an uncertain public write must close its write door until reconciliation");
    }
}

#[test]
fn retired_remote_editor_durable_result_cannot_start_pairing() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let session = edit_address(&mut app);
    let candidate = pending_address(&mut app, &session, "https://control.invalid/retired");
    let mut runtime = app.ott.take().unwrap();
    handle_action(&mut runtime, &mut app, OttAction::CancelSetting { session: session.id }).unwrap();
    app.ott = Some(runtime);
    deliver(&mut app, Job::PublicSaved(Ok(address_snapshot(&candidate)), None, json!({"status":"ok"})));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(remote_settings::address_input(runtime), "https://control.invalid:443/retired");
    assert_ne!(runtime.remote.snapshot()["state"], "discovering");
    assert!(runtime.edit_session.is_none());
}

#[test]
fn unready_remote_address_submit_retains_editor_and_does_not_discover() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let session = edit_address(&mut app);
    let mut runtime = app.ott.take().unwrap();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    // Readiness is lost after admission, not before opening the editor.
    runtime.loaded = false;
    assert!(!runtime.loaded);
    assert!(handle_action(&mut runtime, &mut app, OttAction::SubmitSetting {
        session: session.id, key: session.key.clone(), value: "https://control.invalid".into(),
    }).is_err());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert_ne!(runtime.remote.snapshot()["state"], "discovering");
    assert!(runtime.pending_editor_save.is_none());
    app.ott = Some(runtime);
    settle_pages(&mut app);
    assert!(visible(&app));
}

#[test]
fn invalid_remote_address_cannot_enter_the_durable_candidate() {
    let _serial = plx_base::testlock::serial();
    let runtime = runtime_fixture();
    for invalid in ["https://user:secret@control.invalid", "https://control.invalid?token=secret",
        "http://public.invalid", "https://control.invalid/../other"] {
        assert!(remote_settings::candidate(&runtime, invalid).is_err());
    }
    let mut corrupted = Saved::default();
    corrupted.settings.insert("nativeRemoteServerAddress".into(), json!(true));
    assert!(corrupted.validate().is_err());
}

#[test]
fn saved_address_remains_when_remote_bootstrap_cannot_begin_pairing() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let session = edit_address(&mut app);
    let candidate = pending_address(&mut app, &session, "https://control.invalid/saved");
    let runtime = app.ott.as_mut().unwrap();
    runtime.remote = Remote::from_install("dev_fixture", "fixture");
    let _held = runtime.remote.hold_transport_for_test();
    deliver(&mut app, Job::PublicSaved(Ok(address_snapshot(&candidate)), None, json!({"status":"ok"})));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(remote_settings::address_input(runtime), "https://control.invalid:443/saved");
    assert!(runtime.status.starts_with("Address saved."));
    assert!(runtime.edit_session.as_ref() == Some(&session));
    assert!(visible(&app));
    assert_ne!(runtime.remote.snapshot()["state"], "discovering");
}

#[test]
fn remote_address_reopens_with_last_submitted_address_after_pairing_cancel() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let address = format!(
        "https://control.invalid/{}/service",
        "long-folder-".repeat(24)
    );
    // The established control-origin contract includes the explicit HTTPS port.
    let canonical_address = format!(
        "https://control.invalid:443/{}/service",
        "long-folder-".repeat(24)
    );
    let first = edit_address(&mut app);
    let _held_transport = app.ott.as_mut().unwrap().remote.hold_transport_for_test();
    let candidate = pending_address(&mut app, &first, &address);
    deliver(&mut app, Job::PublicSaved(Ok(address_snapshot(&candidate)), None, json!({"status":"ok"})));
    let runtime = app.ott.as_mut().unwrap();
    assert_eq!(runtime.remote.snapshot()["state"], "discovering");
    runtime.remote.cancel_pairing();
    let second = edit_address(&mut app);
    assert_ne!(first.id, second.id);
    for key in [Key::Down, Key::Ok] {
        let tick = next_tick(&mut app);
        bridge::frame(
            &mut app.pages,
            &mut app.bridge,
            tick,
            bridge::script_key(key, tick),
        );
        settle_pages(&mut app);
    }
    // Read the actual scalar Save envelope, without dispatching it to transport.
    let value = app
        .bridge
        .take_ott_reqs()
        .into_iter()
        .find_map(|request| match request {
            OttOut::Action {
                action:
                    OttAction::SubmitSetting {
                        session,
                        key,
                        value,
                    },
                ..
            } if session == second.id && key == "remote.address" => Some(value),
            _ => None,
        })
        .expect("reopened editor emitted its current value");
    assert_eq!(
        value, canonical_address,
        "reopening must preserve the complete canonical server address"
    );
}

#[test]
fn current_remote_discovery_and_pairing_status_is_not_hidden_by_old_player_status() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    runtime.page = "settings";
    open_remote_group(&mut runtime);
    runtime.status = "No profile selected".into();
    for (state, message) in [
        ("loading", "Loading remote control"),
        ("discovering", "Finding command server"),
        ("choose", "Select command server"),
        ("pairing", "Requesting pairing approval"),
        ("waiting", "Approve the pairing code with ott"),
        ("saving", "Saving protected pairing"),
        ("error", "Pairing server is unavailable"),
    ] {
        runtime.remote_snapshot = json!({"state":state,"message":message,"pairing_code":""});
        assert_eq!(runtime.view().status, message, "remote state {state}");
    }
}

#[test]
fn pairing_code_and_protected_storage_refusal_keep_priority_over_remote_status() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    runtime.page = "settings";
    open_remote_group(&mut runtime);
    runtime.status = "Unrelated old status".into();
    runtime.remote_snapshot = json!({"state":"waiting","message":"Approve the pairing code with ott","pairing_code":"TEST1234"});
    assert_eq!(runtime.view().status, "Pairing code: TEST1234");
    runtime.state_load = StateLoad::Failed("Could not read protected OTT state");
    assert_eq!(runtime.view().status, "Could not read protected OTT state");
    runtime.persistence_uncertain = true;
    assert_eq!(
        runtime.view().status,
        plx_platform::i18n::msg::ott_profile_restart_in(plx_platform::i18n::current())
    );
}

#[test]
fn inactive_remote_status_does_not_hide_current_player_error() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    runtime.page = "settings";
    runtime.status = "Current player error".into();
    runtime.remote_snapshot =
        json!({"state":"disabled","message":"Remote control disabled","pairing_code":""});
    assert_eq!(runtime.view().status, "Current player error");
}


#[test]
fn discovered_command_servers_show_their_addresses_before_selection() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    runtime.page = "settings";
    open_remote_group(&mut runtime);
    // Real v1 descriptors do not carry a friendly name. The addresses are
    // admitted public endpoint metadata, not tokens or pairing receipts.
    let addresses = [
        "https://first-control.invalid:443/team",
        "https://second-control.invalid:443/home",
    ];
    runtime.remote_snapshot = json!({
        "state":"choose", "message":"Select command server", "pairing_code":"",
        "servers":[
            {"id":"first._ottplay-ctrl._tcp.test.","address":addresses[0]},
            {"id":"second._ottplay-ctrl._tcp.test.","address":addresses[1]}
        ]
    });
    let view = runtime.view();
    let rows: Vec<_> = view.rows.iter()
        .filter(|row| row.id.starts_with("remote.server:"))
        .collect();
    assert_eq!(rows.len(), 2);
    for (row, address) in rows.iter().zip(addresses) {
        assert!(row.enabled);
        // Ordinary settings rows render title/detail, not description.
        assert!(row.title.contains(address) || row.detail.contains(address),
            "a discovered server must show its own address in its row before selection");
    }
    assert_eq!(runtime.remote.snapshot()["enabled"], false);
    assert_eq!(runtime.remote.snapshot()["pairing_code"], "");
}


#[test]
fn command_server_find_entry_matches_profile_and_shows_unavailable_reason() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    runtime.page = "settings";
    runtime.remote_snapshot = json!({"find_configured":false,
        "find_unavailable":"Discovery is not configured. Enter a server address."});
    let view = runtime.raw_view();
    assert!(!view.rows.iter().any(|row| row.id == "remote.find"));
    let manual = view.rows.iter().find(|row| row.id == "remote.address").unwrap();
    assert!(manual.enabled);
    assert_eq!(manual.detail, "Discovery is not configured. Enter a server address.");
    runtime.remote_snapshot = json!({"find_configured":true, "find_unavailable":""});
    let view = runtime.raw_view();
    let find = view.rows.iter().find(|row| row.id == "remote.find").unwrap();
    assert!(find.enabled);
    assert_eq!(find.action, OttAction::EditSetting("remote.find".into()));
    assert!(view.rows.iter().find(|row| row.id == "remote.address").unwrap().detail.is_empty());
}

#[test]
fn command_server_choices_forward_captured_tokens_and_disclose_identity() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    runtime.page = "settings";
    runtime.remote_snapshot = json!({"find_configured":true, "servers":[
        {"id":"two._ottplay-ctrl._tcp.test.","domain":"test",
         "address":"https://two.invalid:443/control","choice":"91:1"},
        {"id":"one._ottplay-ctrl._tcp.test.","domain":"test",
         "address":"https://one.invalid:443/control","choice":"91:0"}
    ]});
    let view = runtime.raw_view();
    let choices: Vec<_> = view.rows.iter().filter(|row| row.id.starts_with("remote.server:")).collect();
    assert_eq!(choices.len(), 2);
    assert_eq!(choices[0].action, OttAction::EditSetting("remote.choice:91:1".into()));
    assert_eq!(choices[1].action, OttAction::EditSetting("remote.choice:91:0".into()));
    assert_eq!(choices[0].title, "https://two.invalid:443/control");
    assert!(choices[0].detail.contains("two._ottplay-ctrl._tcp.test."));
    assert!(choices[0].detail.contains("test"));
}

#[test]
fn command_server_find_status_survives_old_connection_health() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    runtime.page = "settings";
    runtime.status = "Unrelated catalog status".into();
    for message in ["Select a command server", "Discovery failed. Try again or enter an address."] {
        runtime.remote_snapshot = json!({"enabled":true,"state":"connected",
            "message":"Remote control connected","pairing_code":"", "find_configured":true,
            "find_state":"choose","find_message":message});
        let view = runtime.raw_view();
        assert_eq!(view.status, message);
        // The live Find row carries its own state in either presentation mode.
        assert_eq!(view.rows.iter().find(|row| row.id == "remote.find").unwrap().detail, message);
    }
    runtime.remote_snapshot["find_message"] = json!("");
    runtime.remote_snapshot["find_state"] = json!("");
    assert_eq!(runtime.raw_view().status, "Unrelated catalog status");
}


#[test]
fn stale_discovery_choice_error_is_visible_and_find_can_retry() {
    let _serial = plx_base::testlock::serial();
    let mut app = app_fixture();
    let mut runtime = app.ott.take().unwrap();
    runtime.loaded = true;
    runtime.page = "settings";
    let catalog = |name: &str| serde_json::to_vec(&json!({"version":1,"servers":[{
        "id":format!("{name}._ottplay-ctrl._tcp.test.invalid."),
        "domain":"test.invalid", "address":format!("https://{name}.invalid/control")
    }]})).unwrap();
    runtime.remote.find_catalog_for_test(&catalog("one"), 100).unwrap();
    runtime.remote_snapshot = runtime.remote.snapshot();
    let stale = runtime.raw_view().rows.into_iter()
        .find(|row| row.id.starts_with("remote.server:")).unwrap().action;
    runtime.remote.find_catalog_for_test(&catalog("two"), 200).unwrap();
    runtime.remote_snapshot = runtime.remote.snapshot();
    let enabled_before = runtime.remote_snapshot["enabled"].clone();
    let error = handle_action(&mut runtime, &mut app, stale).unwrap_err();
    // Match the real runner's error and next-snapshot publication boundary.
    runtime.status = error.into();
    runtime.remote_snapshot = runtime.remote.snapshot();
    assert_eq!(error, "Server choices changed. Find servers again.");
    assert_eq!(runtime.raw_view().status, error);
    assert_eq!(runtime.remote_snapshot["enabled"], enabled_before);
    assert_eq!(runtime.remote_snapshot["pairing_code"], "");
    assert!(runtime.remote_snapshot["servers"].as_array().unwrap().is_empty());
    handle_action(&mut runtime, &mut app, OttAction::EditSetting("remote.find".into())).unwrap();
    runtime.remote_snapshot = runtime.remote.snapshot();
    assert_eq!(runtime.raw_view().status, "Finding command servers…");
    assert_eq!(runtime.remote_snapshot["pairing_code"], "");
}


fn helper_save_refusal() -> private::Error {
    use plx_platform::storage::StoreError;
    use plx_platform::storage::wire::failure::{HelperFailure, Stage};
    private::Error::HelperFailed {
        source: StoreError::HelperUnavailable,
        failure: HelperFailure::new(Stage::RuntimeAbsent, Some(2)),
    }
}

// Retry using only focus/OK, never replacing the draft. Pending controls can have
// moved the engine seat to Cancel; two Up presses return to the first value row.
fn retry_unchanged_address(app: &mut App, session: &EditorSession, value: &str) -> OttAction {
    for key in [Key::Up, Key::Up, Key::Down, Key::Ok] {
        let tick = next_tick(app);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(key, tick));
        settle_pages(app);
    }
    let mut actions = app.bridge.take_ott_reqs().into_iter().filter_map(|request| match request {
        OttOut::Action { action: action @ OttAction::SubmitSetting { .. }, .. } => Some(action),
        _ => None,
    });
    let action = actions.next().expect("the retained draft can be retried after rejection");
    assert_eq!(action, OttAction::SubmitSetting {
        session: session.id, key: session.key.clone(), value: value.into(),
    });
    assert!(actions.next().is_none(), "one retry emits one addressed save");
    action
}

#[test]
fn typed_remote_save_refusal_preserves_draft_and_reports_safe_stage_and_code() {
    use plx_platform::storage::{CommitStage, StoreError};
    let _serial = plx_base::testlock::serial();
    for (error, diagnostic) in [
        (helper_save_refusal(), "storage: helper · no runtime dir (2)"),
        (private::Error::Store(StoreError::Io { stage: CommitStage::Write, errno: -13 }),
            "storage: write (-13)"),
    ] {
        let mut app = mounted();
        let session = edit_address(&mut app);
        let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        let address = "https://control.invalid/retained-draft";
        let _candidate = pending_address(&mut app, &session, address);
        deliver(&mut app, Job::PublicSaved(Err(public_preferences::WriteFailure::Rejected(error)), None, json!({"status":"ok"})));
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert_eq!(remote_settings::address_input(runtime), "");
        assert!(runtime.edit_session.as_ref() == Some(&session));
        assert!(runtime.pending_editor_save.is_none());
        assert!(!runtime.saving);
        assert!(!runtime.persistence_uncertain);
        assert!(runtime.status.contains(diagnostic));
        assert!(!runtime.status.contains(address), "diagnostics must not copy the submitted URL");
        assert_ne!(runtime.remote.snapshot()["state"], "discovering");
        assert_eq!(runtime.remote.snapshot()["pairing_code"], "");
        assert!(visible(&app));
        let _retry = retry_unchanged_address(&mut app, &session, address);
        assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), before);
    }
}

#[test]
fn typed_remote_save_refusal_for_retired_editor_cannot_reopen_or_pair() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let session = edit_address(&mut app);
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let _candidate = pending_address(&mut app, &session, "https://control.invalid/retired-refusal");
    first_run_key(&mut app, Key::Back);
    assert!(app.ott.as_ref().unwrap().edit_session.is_none());
    assert!(!visible(&app));
    deliver(&mut app, Job::PublicSaved(Err(public_preferences::WriteFailure::Rejected(helper_save_refusal())), None, json!({"status":"ok"})));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert_eq!(remote_settings::address_input(runtime), "");
    assert!(runtime.edit_session.is_none());
    assert!(runtime.pending_editor_save.is_none());
    assert!(!runtime.saving);
    assert_ne!(runtime.remote.snapshot()["state"], "discovering");
    assert_eq!(runtime.remote.snapshot()["pairing_code"], "");
    assert!(!visible(&app));
}

#[test]
fn successful_remote_save_retry_clears_typed_refusal_and_starts_existing_pairing_path() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let session = edit_address(&mut app);
    let address = "https://control.invalid/retry";
    let candidate = pending_address(&mut app, &session, address);
    deliver(&mut app, Job::PublicSaved(Err(public_preferences::WriteFailure::Rejected(helper_save_refusal())), None, json!({"status":"ok"})));
    assert!(app.ott.as_ref().unwrap().status.contains("no runtime dir (2)"));
    let action = retry_unchanged_address(&mut app, &session, address);
    // Model the same sole-writer admission as pending_address, after the real
    // retained-draft Save. No protected store or transport worker is started.
    let mut runtime = app.ott.take().unwrap();
    runtime.saving = true;
    runtime.save_navigation = SaveNavigation::Preserve;
    runtime.save_playback_epoch = runtime.playback_epoch;
    runtime.pending_editor_save = Some(session.clone());
    handle_action(&mut runtime, &mut app, action).unwrap();
    let _held_transport = runtime.remote.hold_transport_for_test();
    app.ott = Some(runtime);
    deliver(&mut app, Job::PublicSaved(Ok(address_snapshot(&candidate)), None, json!({"status":"ok"})));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(remote_settings::address_input(runtime), "https://control.invalid:443/retry");
    assert_eq!(runtime.remote.snapshot()["state"], "discovering");
    assert_eq!(runtime.remote.snapshot()["pairing_code"], "");
    assert!(!runtime.status.contains("no runtime dir"));
    assert!(!runtime.status.contains("storage:"));
    assert!(runtime.edit_session.is_none());
    assert!(runtime.pending_editor_save.is_none());
    assert!(!runtime.saving);
    assert!(!visible(&app));
}


#[test]
fn manual_multi_server_rows_bound_domain_labels_without_changing_choices() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    runtime.page = "settings";
    let domains = [
        ("Living\nroom".to_owned(), String::new()),
        ("界".repeat(200), "界".repeat(84)),
        ("x".repeat(600), "x".repeat(253)),
        ("Living room".to_owned(), "Living room".to_owned()),
        ("token=synthetic".to_owned(), String::new()),
    ];
    let servers: Vec<_> = domains.iter().enumerate().map(|(index, (domain, _))| json!({
        "id":format!("server-{index}"), "domain":domain,
        "address":format!("https://server-{index}.invalid/team")
    })).collect();
    let body = serde_json::to_vec(&json!({"version":1,"servers":servers})).unwrap();
    runtime.remote.find_manual_for_test("https://control.invalid/team", &body, 0).unwrap();
    runtime.remote_snapshot = runtime.remote.snapshot();
    assert_eq!(runtime.remote_snapshot["state"], "choose");
    let snapshot = runtime.remote_snapshot.clone();
    let view = runtime.raw_view();
    let choices: Vec<_> = view.rows.iter().filter(|row| row.id.starts_with("remote.server:")).collect();
    assert_eq!(choices.len(), domains.len());
    for (index, (_, label)) in domains.iter().enumerate() {
        let id = format!("server-{index}");
        let address = format!("https://server-{index}.invalid:443/team");
        let server = &snapshot["servers"][index];
        assert_eq!(server["id"], id);
        assert_eq!(server["address"], address);
        assert!(label.len() <= 253);
        assert_eq!(choices[index].title, address);
        assert_eq!(choices[index].detail,
            if label.is_empty() { id } else { format!("{label} · {id}") });
        assert_eq!(server["domain"].as_str(), Some(label.as_str()));
        assert!(!choices[index].detail.chars().any(char::is_control));
        assert_eq!(choices[index].action, OttAction::EditSetting(format!("remote.choice:{}",
            server["choice"].as_str().unwrap())));
    }
    let choice = snapshot["servers"][1]["choice"].as_str().unwrap();
    runtime.remote.select_discovered_choice(choice, 1).unwrap();
    let selected = runtime.remote.snapshot();
    assert_eq!(selected["state"], "pairing");
    assert_eq!(selected["pairing_server"], "https://server-1.invalid:443/team");
    assert_eq!(selected["enabled"], false);
    assert_eq!(selected["saving"], false);
}

#[test]
fn settings_admission_refusal_releases_the_real_pending_draft() {
    let _serial = plx_base::testlock::serial();
    for gate in ["unloaded", "uncertain", "strict", "revoked"] {
        let mut app = mounted();
        let session = edit_address(&mut app);
        let address = "https://control.invalid/unsaved-admission-draft";
        let action = stage_editor_save(&mut app, &session, address);
        let mut runtime = app.ott.take().unwrap();
        match gate {
            "unloaded" => runtime.loaded = false,
            "uncertain" => runtime.persistence_uncertain = true,
            "revoked" => runtime.write_permit.revoke(),
            "strict" => {
                let origin = json!({"provider":"m3u","profile":"Home","subprofile":null,"sourceAlias":"Local"});
                runtime.saved.settings.insert("nativeKiosk".into(), json!({
                    "version":1,"strict":true,"origin":origin,"target":{"kind":"item","entry":{
                        "title":"Fixture channel","origin":origin,
                        "locator":{"kind":"channel","channelId":"fixture-1"}
                    }}
                }));
                assert!(matches!(runtime.track_selection_kiosk(),
                    Ok(crate::app::ott_kiosk_policy::State::Locked { strict: true })));
            }
            _ => unreachable!(),
        }
        let before = serde_json::to_value(&runtime.saved).unwrap();
        assert!(handle_action(&mut runtime, &mut app, action).is_err(), "{gate}");
        assert!(runtime.edit_session.as_ref() == Some(&session));
        assert!(!runtime.saving);
        assert!(runtime.pending_editor_save.is_none());
        assert!(matches!(runtime.rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert_ne!(runtime.remote.snapshot()["state"], "discovering");
        app.ott = Some(runtime);
        settle_pages(&mut app);
        assert!(visible(&app));
        // This uses only focus and OK, never rewrites the draft. A missing
        // addressed refusal leaves Save disabled and cannot produce this action.
        let _retry = retry_unchanged_address(&mut app, &session, address);
        assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), before);
    }
}

#[test]
fn settings_admission_does_not_ack_a_writer_duplicate_or_foreign_submission() {
    let _serial = plx_base::testlock::serial();
    for envelope in ["duplicate", "wrong-key", "old-session", "foreign-owner"] {
        let mut app = mounted();
        let session = edit_address(&mut app);
        let address = "https://control.invalid/admitted-draft";
        let _candidate = pending_address(&mut app, &session, address);
        let mut runtime = app.ott.take().unwrap();
        // A later gate refusal cannot release a separately admitted writer.
        runtime.loaded = false;
        let before = serde_json::to_value(&runtime.saved).unwrap();
        let queued = app.pages.queued();
        let mut id = session.id;
        let mut key = session.key.clone();
        match envelope {
            "duplicate" => {}
            "wrong-key" => key = "uiFontScale".into(),
            "old-session" => id = session.id.saturating_add(1),
            "foreign-owner" => runtime.reply = Some(InstanceId(session.owner.0.wrapping_add(1))),
            _ => unreachable!(),
        }
        let outcome = handle_action(&mut runtime, &mut app, OttAction::SubmitSetting {
            session: id, key, value: address.into(),
        });
        if envelope == "duplicate" { assert!(outcome.is_ok()); }
        else { assert!(outcome.is_err()); }
        assert_eq!(app.pages.queued(), queued, "{envelope} cannot acknowledge this editor");
        assert!(runtime.edit_session.as_ref() == Some(&session));
        assert!(runtime.pending_editor_save.as_ref() == Some(&session));
        assert!(runtime.saving);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        app.ott = Some(runtime);
    }
}


#[test]
fn discovery_failure_diagnostic_is_visible_in_settings_status_and_find_row() {
    let _serial = plx_base::testlock::serial();
    for language in plx_platform::i18n::SHIPPED {
        let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
        let mut runtime = runtime_fixture();
        runtime.page = "settings";
        open_remote_group(&mut runtime);
        runtime.status = "Unrelated old player error".into();
        let message = "Discovery failed. Try again or enter an address.";
        for diagnostic in ["HTTP 503", "TLS certificate verification failed", "Request timed out", ""] {
            runtime.remote_snapshot = json!({"find_configured":true, "find_state":"error",
                "find_message":message, "find_diagnostic":diagnostic,
                "state":"connected", "message":"Remote control connected", "pairing_code":""});
            let expected = if diagnostic.is_empty() { ui_text::text(message).to_owned() }
                else { format!("{} · {diagnostic}", ui_text::text(message)) };
            let view = runtime.view();
            assert_eq!(view.status, expected);
            assert_eq!(view.rows.iter().find(|row| row.id == "remote.find").unwrap().detail, expected);
            assert_eq!(runtime.remote_snapshot["find_message"], message, "wire copy stays unchanged");
            assert_eq!(runtime.remote_snapshot["find_diagnostic"], diagnostic);
        }
    }
}

#[test]
fn discovery_diagnostic_does_not_replace_protected_failure_or_outlive_find_status() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    runtime.page = "settings";
    open_remote_group(&mut runtime);
    runtime.remote_snapshot = json!({"find_configured":true,
        "find_message":"Discovery failed. Try again or enter an address.", "find_diagnostic":"HTTP 503",
        "state":"waiting", "message":"Approve the pairing code with ott", "pairing_code":"TEST1234"});
    runtime.state_load = StateLoad::Failed("Could not read protected OTT state");
    assert_eq!(runtime.view().status, "Could not read protected OTT state");
    runtime.state_load = StateLoad::Idle;
    runtime.remote_snapshot["find_message"] = json!("");
    let view = runtime.view();
    assert_eq!(view.status, "Pairing code: TEST1234");
    assert!(view.rows.iter().find(|row| row.id == "remote.find").unwrap().detail.is_empty());
}

#[test]
fn discovery_result_replacement_changes_the_row_input_identity() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    runtime.page = "settings";
    let catalog = |name: &str| serde_json::to_vec(&json!({"version":1,"servers":[{
        "id":format!("{name}._ottplay-ctrl._tcp.test.invalid."), "domain":"test.invalid",
        "address":format!("https://{name}.invalid/control")
    }]})).unwrap();
    runtime.remote.find_catalog_for_test(&catalog("one"), 100).unwrap();
    runtime.remote_snapshot = runtime.remote.snapshot();
    let before = runtime.raw_view().rows.into_iter().find(|row| row.id.starts_with("remote.server:")).unwrap();
    // A health/status-only publication retains identity while a new result generation does not.
    runtime.remote_snapshot["message"] = json!("Remote control connected");
    let unchanged = runtime.raw_view().rows.into_iter().find(|row| row.id.starts_with("remote.server:")).unwrap();
    assert_eq!(unchanged.id, before.id);
    assert_eq!(unchanged.action, before.action);
    runtime.remote.find_catalog_for_test(&catalog("two"), 200).unwrap();
    runtime.remote_snapshot = runtime.remote.snapshot();
    let after = runtime.raw_view().rows.into_iter().find(|row| row.id.starts_with("remote.server:")).unwrap();
    assert_ne!(after.action, before.action);
    assert_ne!(after.title, before.title);
    assert_ne!(after.id, before.id, "a new choice must not inherit the old presented/armed row key");
    assert_eq!(runtime.remote.snapshot()["enabled"], false);
    assert_eq!(runtime.remote.snapshot()["pairing_code"], "");
}


#[test]
fn discovered_server_choices_follow_find_and_clear_on_retry() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    runtime.page = "settings";
    runtime.remote_snapshot = runtime.remote.snapshot();
    let original_ids: Vec<_> = runtime.raw_view().rows.into_iter().map(|row| row.id).collect();
    let body = serde_json::to_vec(&json!({"version":1,"servers":[
        {"id":"one", "domain":"test.invalid", "address":"https://one.invalid/control"},
        {"id":"two", "domain":"test.invalid", "address":"https://two.invalid/control"}
    ]})).unwrap();
    runtime.remote.find_catalog_for_test(&body, 100).unwrap();
    runtime.remote_snapshot = runtime.remote.snapshot();
    let snapshot = runtime.remote_snapshot.clone();
    let view = runtime.raw_view();
    let find = view.rows.iter().position(|row| row.id == "remote.find").unwrap();
    let servers = snapshot["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 2);
    for (offset, server) in servers.iter().enumerate() {
        let row = &view.rows[find + 1 + offset];
        let token = server["choice"].as_str().unwrap();
        assert_eq!(row.id, format!("remote.server:{offset}:{token}"));
        assert_eq!(row.action, OttAction::EditSetting(format!("remote.choice:{token}")));
        assert_eq!(row.title, server["address"].as_str().unwrap());
    }
    assert_eq!(view.rows[find + 1 + servers.len()].id, "uiFontScale");
    assert_eq!(view.rows.into_iter().filter(|row| !row.id.starts_with("remote.server:"))
        .map(|row| row.id).collect::<Vec<_>>(), original_ids);
    assert_eq!(runtime.remote_snapshot, snapshot, "rendering must not consume a choice");

    // Real Find admission retires the previous table before its next completion.
    runtime.remote.find_servers(200).unwrap();
    runtime.remote_snapshot = runtime.remote.snapshot();
    assert_eq!(runtime.remote_snapshot["find_state"], "discovering");
    assert_eq!(runtime.raw_view().rows.into_iter().map(|row| row.id).collect::<Vec<_>>(), original_ids);
}

#[test]
fn manual_server_choices_follow_address_when_find_is_unavailable() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime_fixture();
    runtime.page = "settings";
    let body = serde_json::to_vec(&json!({"version":1,"servers":[
        {"id":"one", "domain":"test.invalid", "address":"https://one.invalid/control"},
        {"id":"two", "domain":"test.invalid", "address":"https://two.invalid/control"}
    ]})).unwrap();
    runtime.remote.find_manual_for_test("https://control.invalid", &body, 100).unwrap();
    runtime.remote_snapshot = runtime.remote.snapshot();
    assert_eq!(runtime.remote_snapshot["state"], "choose");
    // Exercise the existing no-Find presentation branch with a real manual result.
    runtime.remote_snapshot["find_configured"] = json!(false);
    let snapshot = runtime.remote_snapshot.clone();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let view = runtime.raw_view();
    assert!(!view.rows.iter().any(|row| row.id == "remote.find"));
    let address = view.rows.iter().position(|row| row.id == "remote.address").unwrap();
    let servers = snapshot["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 2);
    for (offset, server) in servers.iter().enumerate() {
        let row = &view.rows[address + 1 + offset];
        let token = server["choice"].as_str().unwrap();
        assert_eq!(row.id, format!("remote.server:{offset}:{token}"));
        assert_eq!(row.action, OttAction::EditSetting(format!("remote.choice:{token}")));
    }
    assert_eq!(view.rows[address + 1 + servers.len()].id, "uiFontScale");
    assert_eq!(runtime.remote_snapshot, snapshot);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert_eq!(runtime.remote.snapshot()["pairing_code"], "");
}
