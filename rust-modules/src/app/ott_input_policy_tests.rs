use super::*;

mod favorites_kiosk {
    use super::*;

    fn setup(pending: bool) -> App {
        let mut app = fixture();
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.settings.insert("nativeKiosk".into(), pin(true));
        let favorite = serde_json::from_value(pin(true)["target"]["entry"].clone()).unwrap();
        assert!(runtime.saved.favorites.toggle(favorite).unwrap());
        assert!(policy::strict(runtime));
        if pending {
            assert!(request(&mut app, "kiosk", json!({"mode":"off"})).is_none());
            let runtime = app.ott.as_ref().unwrap();
            // Hold the real pending transition without processing its writer's
            // pre-IO refusal. No successful persistence is modeled here.
            assert!(matches!(
                runtime.rx.recv_timeout(Duration::from_secs(5)).unwrap(),
                Job::Saved(Err(_), Some(_), false, _)
            ));
            assert!(runtime.saving && policy::pending(runtime));
        }
        app
    }

    fn call(app: &mut App, params: Value) -> Value {
        let mut runtime = app.ott.take().unwrap();
        // Reuse the raw-envelope helper; it supports any action and assigns a
        // distinct identity from its parameters. These are the same policy and
        // Favorites adapters invoked in sequence by the main remote tick.
        let invocation = plex_invocation(&mut runtime, "favorites_global", params);
        assert!(runtime.remote.invocation_is_current(&invocation));
        let id = invocation.id.clone();
        let now = runtime.now_ms();
        let response = match policy::remote(&mut runtime, &invocation) {
            Err(error) => rejected(error),
            Ok(()) => favorites::command(&mut runtime, &invocation, now)
                .expect("inspection must not start an asynchronous write"),
        };
        runtime.remote.complete(invocation, response);
        let response = runtime.remote.response_for_id_for_test(&id).unwrap();
        app.ott = Some(runtime);
        response
    }

    fn inspection(pending: bool) {
        let _serial = plx_base::testlock::serial();
        let mut app = setup(pending);
        let runtime = app.ott.as_ref().unwrap();
        let before = serde_json::to_value(&runtime.saved).unwrap();
        let bytes = runtime.saved.favorites.encode().unwrap();
        let epochs = (runtime.generation, runtime.play_generation, runtime.playback_epoch);
        let description = call(&mut app, json!({"op":"describe"}));
        assert_eq!(description["status"], "ok", "describe must pass the kiosk fence");
        assert_eq!(description["data"]["op"], "describe");
        assert_eq!(description["data"]["subject"], "global");
        assert_eq!(description["data"]["export"]["entries"], 1);
        let snapshot = description["data"]["export"]["snapshot_id"].as_str().unwrap();
        let export = call(&mut app, json!({"op":"export","snapshot_id":snapshot,
            "offset":0,"max_bytes":65536}));
        assert_eq!(export["status"], "ok");
        assert_eq!(export["data"]["data_base64"], plx_base::b64::encode(&bytes));
        assert_eq!(export["data"]["eof"], true);
        assert_eq!(export["data"]["size_bytes"], bytes.len());
        assert_eq!(export["data"]["document_sha256"],
            plx_data::ott::favorites_wire::hash(&bytes));
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert_eq!((runtime.generation, runtime.play_generation, runtime.playback_epoch), epochs);
        assert_eq!(runtime.saving, pending);
        assert_eq!(policy::pending(runtime), pending);
        assert!(policy::strict(runtime));
        assert!(runtime.rx.try_recv().is_err(), "inspection must not launch a writer");
    }

    fn mutations_stay_blocked(pending: bool) {
        let _serial = plx_base::testlock::serial();
        let mut app = setup(pending);
        let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        let error = if pending {
            "A kiosk transition is being saved. Try again when it settles."
        } else {
            "This action is restricted by kiosk policy."
        };
        for op in ["begin", "append", "preview", "commit", "abort"] {
            // The exact policy error proves refusal before transaction parsing,
            // even though these envelopes have no transaction payload.
            let response = call(&mut app, json!({"op":op}));
            assert_eq!(response["status"], "rejected", "{op}");
            assert_eq!(response["data"]["error"], error, "{op}");
        }
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert_eq!(runtime.saving, pending);
        assert_eq!(policy::pending(runtime), pending);
        assert!(runtime.rx.try_recv().is_err());
    }

    #[test]
    fn strict_kiosk_keeps_describe_and_export_available() {
        inspection(false);
    }

    #[test]
    fn pending_kiosk_keeps_describe_and_export_available() {
        inspection(true);
    }

    #[test]
    fn strict_kiosk_keeps_favorites_transactions_blocked() {
        mutations_stay_blocked(false);
    }

    #[test]
    fn pending_kiosk_keeps_favorites_transactions_blocked() {
        mutations_stay_blocked(true);
    }
}
use crate::app::ott_runtime::input_policy as policy;
const RECORD:&str="pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65";
fn fixture() -> App {
    let mut app = app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.entropy = Some([4; 16]);
    runtime.write_permit.revoke();
    app
}
fn request(app: &mut App, action: &str, params: Value) -> Option<Value> {
    let mut runtime = app.ott.take().unwrap();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":"0123456789abcdef0123456789abcdef","action":action,"params":params,"expires_at":1020
    }]})
    .to_string();
    let mut invocations = runtime.remote.raw_poll_for_test(&body, 100);
    assert_eq!(invocations.len(), 1);
    let mut invocation = invocations.remove(0);
    invocation.deadline_ms = runtime.now_ms().saturating_add(10000);
    let response = commands::command(&mut runtime, app, &invocation);
    app.ott = Some(runtime);
    response
}
fn pin(strict: bool) -> Value {
    let origin = json!({"provider":"m3u","profile":"Home","subprofile":null,"sourceAlias":"Local"});
    json!({"version":1,"strict":strict,"origin":origin,"target":{"kind":"item","entry":{
        "title":"Fixture channel","origin":origin,"locator":{"kind":"channel","channelId":"fixture-1"}
    }}})
}
#[test]
fn protected_kiosk_designation_is_denied_before_any_saved_candidate_or_worker() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    app.ott
        .as_mut()
        .unwrap()
        .saved
        .settings
        .insert("parentPin".into(), json!(RECORD));
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let response = request(&mut app, "kiosk", json!({"mode":"set","query":"Fixture"})).unwrap();
    assert_eq!(response["status"], "rejected");
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.saving);
    assert!(!commands::kiosk_pending(runtime));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(runtime.rx.try_recv().is_err());
}
#[test]
fn normal_pin_allows_browsing_but_refuses_provider_replacement() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime
        .saved
        .settings
        .insert("nativeKiosk".into(), pin(false));
    assert!(policy::ui(runtime, &OttAction::ShowCategories).is_ok());
    assert!(policy::ui(runtime, &OttAction::SelectProvider("m3u".into())).is_err());
    assert!(policy::replace_source(runtime).is_err());
}
#[test]
fn strict_pin_keeps_footer_requests_but_rejects_transport_and_exit() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime
        .saved
        .settings
        .insert("nativeKiosk".into(), pin(true));
    assert!(policy::strict(runtime));
    assert!(policy::player(runtime, &PlayerReq::ExtendHud(5000)));
    assert!(policy::player(runtime, &PlayerReq::ExtendDefaultHud));
    assert!(!policy::player(runtime, &PlayerReq::SeekTo(1000)));
    for request in [
        PlayerReq::Exit,
        PlayerReq::Transport(None),
        PlayerReq::RetryPlayback,
    ] {
        assert!(!policy::player(runtime, &request));
    }
    assert!(policy::ui(runtime, &OttAction::ShowCategories).is_err());
}
#[test]
fn strict_status_remains_available_at_real_dispatcher() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    app.ott
        .as_mut()
        .unwrap()
        .saved
        .settings
        .insert("nativeKiosk".into(), pin(true));
    let response = request(&mut app, "state", json!({})).unwrap();
    assert_eq!(response["status"], "ok");
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn strict_kiosk_can_be_turned_off_remotely_without_parental_grant() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    {
        let r = app.ott.as_mut().unwrap();
        r.saved.settings.insert("nativeKiosk".into(), pin(true));
        r.saved.settings.insert("parentPin".into(), json!(RECORD));
    }
    assert!(request(&mut app, "kiosk", json!({"mode":"off"})).is_none());
    let r = app.ott.as_ref().unwrap();
    assert!(r.saving);
    assert!(commands::kiosk_pending(r));
    assert!(policy::ui(r, &OttAction::Play("other".into())).is_err());
    assert!(
        r.saved.settings.contains_key("nativeKiosk"),
        "Off is not applied before verified save"
    );
    assert!(matches!(
        r.rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        Job::Saved(Err(_), Some(_), false, _)
    ));
}
#[test]
fn unavailable_policy_rejects_transport_but_keeps_footer() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let r = app.ott.as_mut().unwrap();
    r.saved.settings.insert("nativeKiosk".into(), json!(null));
    assert!(policy::navigation(r).is_err());
    assert!(!policy::player(r, &PlayerReq::Transport(None)));
    assert!(!policy::player(r, &PlayerReq::SeekTo(0)));
    assert!(policy::player(r, &PlayerReq::ExtendHud(5000)));
    assert!(policy::player(r, &PlayerReq::ExtendDefaultHud));
}

#[test]
fn normal_pinned_back_closes_browse_without_retiring_source() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let mut runtime = app.ott.take().unwrap();
    runtime
        .saved
        .settings
        .insert("nativeKiosk".into(), pin(false));
    runtime.page = "categories";
    let epoch = runtime.playback_epoch;
    let generation = runtime.play_generation;
    handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
    assert_eq!(runtime.page, "providers");
    assert_eq!(runtime.playback_epoch, epoch);
    assert_eq!(runtime.play_generation, generation);
    assert!(!runtime.saving);
    app.ott = Some(runtime);
}


fn plex_invocation(runtime: &mut Runtime, action: &str, mut params: Value) -> Invocation {
    params["runtime"] = json!(runtime.remote.runtime());
    let id: String = plx_base::sha256::sha256(&serde_json::to_vec(&json!([action, &params])).unwrap())
        .iter().take(16).map(|byte| format!("{byte:02x}")).collect();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":&id[..32],"action":action,"params":params,"expires_at":1020
    }]})
    .to_string();
    let mut invocations = runtime.remote.raw_poll_for_test(&body, 100);
    assert_eq!(invocations.len(), 1);
    let mut invocation = invocations.remove(0);
    invocation.deadline_ms = runtime.now_ms().saturating_add(10000);
    invocation
}
#[test]
fn strict_kiosk_allows_plex_inspection_but_not_queue_transport() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let r = app.ott.as_mut().unwrap();
    r.saved.settings.insert("nativeKiosk".into(), pin(true));
    for (action, params) in [
        ("plex_queue", json!({"op":"status"})),
        ("plex_queue", json!({"op":"preview","ids":["1"]})),
        ("plex_library", json!({"op":"list"})),
        ("plex_library", json!({"op":"preview","library_id":"1"})),
    ] {
        let inv = plex_invocation(r, action, params);
        assert!(policy::remote(r, &inv).is_ok(), "{action} inspection");
    }
    for op in ["next", "previous", "stop"] {
        let inv = plex_invocation(r, "plex_queue", json!({"op":op}));
        assert!(policy::remote(r, &inv).is_err(), "strict {op}");
    }
}
#[test]
fn strict_plex_status_reaches_the_real_queue_dispatcher() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let mut runtime = app.ott.take().unwrap();
    runtime
        .saved
        .settings
        .insert("nativeKiosk".into(), pin(true));
    let inv = plex_invocation(&mut runtime, "plex_queue", json!({"op":"status"}));
    let response = commands::command(&mut runtime, &mut app, &inv).unwrap();
    assert_eq!(response["status"], "ok");
    assert!(!runtime.saving);
    assert!(!runtime.resolver_busy);
    app.ott = Some(runtime);
}
#[test]
fn pinned_plex_prepare_is_refused_before_any_catalog_or_native_mutation() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let mut runtime = app.ott.take().unwrap();
    runtime
        .saved
        .settings
        .insert("nativeKiosk".into(), pin(false));
    for (action, params) in [
        ("plex_queue", json!({"op":"play","ids":["1"]})),
        ("plex_library", json!({"op":"play","library_id":"1"})),
    ] {
        let inv = plex_invocation(&mut runtime, action, params);
        let generation = runtime.generation;
        let playback_epoch = runtime.playback_epoch;
        let cancel = runtime.cancel.clone();
        let response = commands::command(&mut runtime, &mut app, &inv).unwrap();
        assert_eq!(response["status"], "rejected");
        assert_eq!(runtime.generation, generation);
        assert_eq!(runtime.playback_epoch, playback_epoch);
        assert!(Arc::ptr_eq(&runtime.cancel, &cancel));
        assert!(!cancel.load(Ordering::Acquire));
        assert!(runtime.transient.candidate.is_none());
        assert!(runtime.resolver_next.is_none());
        assert!(!runtime.resolver_busy);
    }
    app.ott = Some(runtime);
}
#[test]
fn plex_prepare_requires_parental_access_before_resolver_admission() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let r = app.ott.as_mut().unwrap();
    r.saved.settings.insert("parentPin".into(), json!(RECORD));
    for action in ["plex_queue", "plex_library"] {
        let params = if action == "plex_queue" {
            json!({"op":"play","ids":["1"]})
        } else {
            json!({"op":"play","library_id":"1"})
        };
        let inv = plex_invocation(r, action, params);
        assert!(policy::remote(r, &inv).is_err());
        assert!(r.transient.candidate.is_none());
        assert!(r.resolver_next.is_none());
    }
}
#[test]
fn plex_inspection_cannot_hide_a_mutating_operation() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let r = app.ott.as_mut().unwrap();
    r.saved.settings.insert("nativeKiosk".into(), pin(true));
    let inv = plex_invocation(
        r,
        "plex_queue",
        json!({"op":"play","operation":"status","ids":["1"]}),
    );
    assert!(policy::remote(r, &inv).is_err());
    let inv = plex_invocation(r, "plex_queue", json!({"op":"status","ids":["1"]}));
    assert!(policy::remote(r, &inv).is_err());
}
#[test]
fn normal_pin_keeps_member_navigation_and_stop_available() {
    let _serial = plx_base::testlock::serial();
    let mut app = fixture();
    let r = app.ott.as_mut().unwrap();
    r.saved.settings.insert("nativeKiosk".into(), pin(false));
    for op in ["next", "previous", "stop"] {
        let inv = plex_invocation(r, "plex_queue", json!({"op":op}));
        assert!(policy::remote(r, &inv).is_ok());
    }
}

#[cfg(feature = "devtriggers")]
mod exit_ingress {
    use super::*;

    struct Events;
    impl Events {
        fn new() -> Self {
            // Exercise the production FIFO -> SDL queue -> ingress path, without
            // a window, renderer, decoder, protected state or device.
            assert_eq!(unsafe { crate::app::SDL_Init(0x4000) }, 0);
            Self
        }
    }
    impl Drop for Events {
        fn drop(&mut self) {
            unsafe { crate::app::SDL_Quit(); }
        }
    }

    fn catalog() -> App {
        let mut app = fixture();
        app.running = true;
        app.ott.as_mut().unwrap().page = "channels";
        bridge::show_page(&mut app.pages, AppArg::Ott);
        settle_pages(&mut app);
        runtime_tick(&mut app);
        settle_pages(&mut app);
        assert!(matches!(app.route(), AppArg::Ott));
        assert_eq!(app.ott.as_ref().unwrap().page, "channels");
        assert!(bridge::owns_input(&app.pages));
        app.inputs.clear();
        app
    }

    fn fifo(app: &mut App, token: &str) {
        app.remote = Some(crate::remote::Remote::buffered_for_test(&format!("{token}\n")));
        let mut frame = run::Frame {
            ctrl: plx_appkit::player_hud::slot(&app.player.session, app.bridge.metadata_view()),
            now: clock::now(),
            dt: 0.016,
            underlay_moving: false,
            player: matches!(app.route(), AppArg::Player),
            present: false,
            deferred: false,
            rn: "ott",
        };
        unsafe { run::ingest(app, &mut frame); }
    }

    #[test]
    fn fifo_exit_terminates_owned_catalog() {
        let _serial = plx_base::testlock::serial();
        let _events = Events::new();
        let mut app = catalog();
        let page = app.pages.top_page();
        assert!(!kiosk_user_keys_blocked(&app));
        fifo(&mut app, "exit");
        assert!(!app.running, "explicit Exit must reach process termination before tree ownership");
        assert_eq!(app.pages.top_page(), page, "Exit is not Back navigation");
    }

    #[test]
    fn fifo_exit_terminates_owned_player() {
        let _serial = plx_base::testlock::serial();
        let _events = Events::new();
        let mut app = catalog();
        mount_player(&mut app);
        assert!(matches!(app.route(), AppArg::Player));
        assert!(bridge::owns_input(&app.pages));
        fifo(&mut app, "exit");
        assert!(!app.running, "the mounted player must not swallow explicit Exit");
    }

    #[test]
    fn fifo_exit_terminates_owned_settings() {
        let _serial = plx_base::testlock::serial();
        let _events = Events::new();
        let mut app = catalog();
        bridge::show_page(&mut app.pages, AppArg::Settings(plx_screens::family::SettingsPage::Root));
        settle_pages(&mut app);
        assert!(bridge::owns_input(&app.pages));
        fifo(&mut app, "exit");
        assert!(!app.running, "explicit Exit is not a settings Back press");
    }

    #[test]
    fn normal_pin_does_not_swallow_explicit_exit() {
        let _serial = plx_base::testlock::serial();
        let _events = Events::new();
        let mut app = catalog();
        app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), pin(false));
        assert!(!kiosk_user_keys_blocked(&app));
        fifo(&mut app, "exit");
        assert!(!app.running, "a non-strict source pin permits exiting the application");
    }

    #[test]
    fn strict_kiosk_still_blocks_explicit_exit() {
        let _serial = plx_base::testlock::serial();
        let _events = Events::new();
        let mut app = catalog();
        app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), pin(true));
        assert!(kiosk_user_keys_blocked(&app));
        fifo(&mut app, "exit");
        assert!(app.running, "Exit must not bypass strict kiosk admission");
        assert!(app.inputs.is_empty());
    }

    #[test]
    fn pending_kiosk_still_blocks_explicit_exit() {
        let _serial = plx_base::testlock::serial();
        let _events = Events::new();
        let mut app = catalog();
        app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), pin(false));
        assert!(request(&mut app, "kiosk", json!({"mode":"off"})).is_none());
        let runtime = app.ott.as_ref().unwrap();
        // Hold the real transition before its pre-IO refusal is consumed. The
        // durable pin is deliberately non-strict: only the pending fence blocks.
        assert!(matches!(runtime.rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Job::Saved(Err(_), Some(_), false, _)));
        assert!(policy::pending(runtime));
        assert!(!policy::strict(runtime));
        fifo(&mut app, "exit");
        assert!(app.running, "Exit must not bypass a pending kiosk transition");
        assert!(app.inputs.is_empty());
    }

    #[test]
    fn unavailable_kiosk_policy_still_blocks_explicit_exit() {
        let _serial = plx_base::testlock::serial();
        let _events = Events::new();
        let mut app = catalog();
        app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), json!(null));
        assert!(kiosk_user_keys_blocked(&app));
        fifo(&mut app, "exit");
        assert!(app.running, "malformed policy must remain fail-closed");
    }

    #[test]
    fn fifo_exit_release_alone_does_not_terminate() {
        let _serial = plx_base::testlock::serial();
        let _events = Events::new();
        let mut app = catalog();
        fifo(&mut app, "holdup:exit");
        assert!(app.running, "only a fresh press may request termination");
    }

    #[test]
    fn fifo_back_keeps_catalog_parent_navigation() {
        let _serial = plx_base::testlock::serial();
        let _events = Events::new();
        let mut app = catalog();
        let page = app.pages.top_page();
        fifo(&mut app, "back");
        assert!(app.running, "Back must not become process Exit");
        let inputs = std::mem::take(&mut app.inputs);
        assert!(!inputs.is_empty(), "Back must still reach the owned dispatcher");
        let tick = next_tick(&mut app);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, inputs);
        runtime_tick(&mut app);
        settle_pages(&mut app);
        assert!(app.running);
        assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
        assert_eq!(app.pages.top_page(), page);
    }
}

// Exercises the actual dispatcher effect and content drain. The observer replaces
// only the final persistence boundary; these are admission, not storage tests.
mod native_preferences_admission {
    use super::*;
    use crate::app::preferences::admission_probe::Probe;
    use plx_machine::machine::{InputOwner, Key, NavOp};
    use plx_screens::registry::{PreferenceCmd, AccountPreferenceReply};
    use std::sync::mpsc::{self, Receiver};

    struct PlaybackReset(u8);
    impl Drop for PlaybackReset {
        fn drop(&mut self) {
            plx_media::player::SHARED.reset_session();
            plx_media::player::TX.reset();
            plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
            plx_media::player::restore_state_for_test(self.0);
        }
    }
    fn playing(app: &mut App) -> PlaybackReset {
        let old = plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Idle);
        let mut info = unsupported_info();
        info.video_codec = "h264".into();
        plx_media::route::request_external_source(&mut app.player.session, &info,
            plx_media::ff::ExternalSource::Http("https://media.invalid/preference-test.mp4".into()),
            "preference-test", "Preference admission", plx_platform::devcaps::assumed_shared())
            .expect("fixture has a genuine logical playback identity");
        plx_media::route::reset_player_control_for_test(&app.player.session);
        plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
        PlaybackReset(old)
    }
    fn settings(app: &mut App) -> plx_machine::machine::InstanceId {
        app.running = true;
        bridge::show_page(&mut app.pages,
            AppArg::Settings(plx_screens::family::SettingsPage::Language));
        settle_pages(app);
        let owner = app.pages.top_page().expect("mounted native Settings page");
        let entry = app.pages.nav.entry_of_instance(owner).unwrap();
        assert_eq!(app.pages.nav.input_owner(), Some(InputOwner::Entry(entry)));
        owner
    }
    fn emit(app: &mut App, owner: plx_machine::machine::InstanceId, command: PreferenceCmd) {
        app.pages.emit(MachineId::Instance(owner), Fx::App(AppFx::Preferences(command)));
        let tick = next_tick(app);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    }
    fn language(app: &mut App, owner: plx_machine::machine::InstanceId) -> Receiver<plx_screens::registry::LanguageApplyResult> {
        let (reply, receipt) = mpsc::channel();
        emit(app, owner, PreferenceCmd::Language {
            language: plx_platform::i18n::Preference::System, reply,
        });
        receipt
    }
    fn drain(app: &mut App) {
        let tick = next_tick(app);
        let frame = run::Frame {
            ctrl: plx_appkit::player_hud::slot(&app.player.session, app.bridge.metadata_view()),
            now: tick.ms, dt: 0.016, underlay_moving: false,
            player: matches!(app.route(), AppArg::Player), present: false,
            deferred: false, rn: "settings",
        };
        content::content_requests(app, &frame);
    }
    fn hold_strict_write(app: &mut App) -> Job {
        assert!(request(app, "kiosk", json!({"mode":"on","strict":true})).is_none());
        let r = app.ott.as_ref().unwrap();
        assert!(policy::pending(r));
        assert!(!policy::strict(r), "unsaved candidate is not effective policy");
        let completion = r.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(&completion, Job::Saved(Err(_), Some(_), false, _)));
        completion
    }
    fn assert_playing(app: &App, identity: &Option<plx_media::route::PlaybackIdentity>) {
        assert_eq!(&plx_media::route::playback_identity(&app.player.session), identity);
        assert!(identity.is_some());
        assert_eq!(plx_media::player::state(&app.player.session), plx_media::player::PlaybackState::Playing);
    }

    #[test]
    fn native_preferences_real_language_picker_reaches_current_owner_once() {
        let _serial = plx_base::testlock::serial();
        let probe = Probe::new();
        let mut app = fixture();
        let _playback = playing(&mut app);
        let identity = plx_media::route::playback_identity(&app.player.session);
        settings(&mut app);
        // Select a different language through the real native Form, then press Save's
        // actual choice row. No fake screen or direct preferences executor is used.
        for key in [Key::Down, Key::Ok] {
            let tick = next_tick(&mut app);
            bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(key, tick));
            settle_pages(&mut app);
        }
        drain(&mut app);
        assert_eq!(probe.calls(), vec!["language"]);
        assert_playing(&app, &identity);
    }

    #[test]
    fn native_preferences_queued_mutation_rechecks_saved_strict_policy() {
        let _serial = plx_base::testlock::serial();
        let probe = Probe::new();
        let mut app = fixture();
        let _playback = playing(&mut app);
        let identity = plx_media::route::playback_identity(&app.player.session);
        app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), pin(false));
        let owner = settings(&mut app);
        let receipt = language(&mut app, owner);
        let completion = hold_strict_write(&mut app);
        // The test supplies the owned successful Saved boundary, never mutates the
        // effective policy before it. No actual storage success is claimed here.
        let Job::Saved(_, invocation, reload, response) = completion else { unreachable!() };
        let mut accepted = app.ott.as_ref().unwrap().saved.clone();
        accepted.settings.insert("nativeKiosk".into(), pin(true));
        app.ott.as_ref().unwrap().tx.send(Job::Saved(Ok(accepted), invocation, reload, response)).unwrap();
        runtime_tick(&mut app);
        assert!(policy::strict(app.ott.as_ref().unwrap()));
        drain(&mut app);
        assert!(probe.calls().is_empty(), "old UI command must not reach persistence");
        assert_eq!(receipt.try_recv().expect("refusal must settle the native picker"), plx_screens::registry::LanguageApplyResult::Failed);
        assert_playing(&app, &identity);
    }

    #[test]
    fn native_preferences_queued_mutation_rejects_deposed_owner() {
        let _serial = plx_base::testlock::serial();
        let probe = Probe::new();
        let mut app = fixture();
        let _playback = playing(&mut app);
        let identity = plx_media::route::playback_identity(&app.player.session);
        let owner = settings(&mut app);
        let receipt = language(&mut app, owner);
        app.pages.request(MachineId::Nav, NavOp::Root(AppArg::Ott));
        settle_pages(&mut app);
        assert_ne!(app.pages.top_page(), Some(owner));
        drain(&mut app);
        assert!(probe.calls().is_empty());
        assert_eq!(receipt.try_recv().expect("stale picker receives explicit refusal"), plx_screens::registry::LanguageApplyResult::Failed);
        assert_playing(&app, &identity);
    }

    #[test]
    fn native_preferences_pending_transition_blocks_write_not_inspection() {
        let _serial = plx_base::testlock::serial();
        let probe = Probe::new();
        let mut app = fixture();
        app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), pin(false));
        let owner = settings(&mut app);
        let _completion = hold_strict_write(&mut app);
        let receipt = language(&mut app, owner);
        let (reply, loaded): (_, Receiver<AccountPreferenceReply>) = mpsc::channel();
        emit(&mut app, owner, PreferenceCmd::Load { reply });
        drain(&mut app);
        assert_eq!(probe.calls(), vec!["load"]);
        assert_eq!(receipt.try_recv().unwrap(), plx_screens::registry::LanguageApplyResult::Failed);
        assert!(loaded.try_recv().is_ok(), "read-only request retains its existing result path");
    }

    #[test]
    fn native_preferences_failed_strict_save_restores_prior_normal_policy() {
        let _serial = plx_base::testlock::serial();
        let probe = Probe::new();
        let mut app = fixture();
        let _playback = playing(&mut app);
        let identity = plx_media::route::playback_identity(&app.player.session);
        app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), pin(false));
        let owner = settings(&mut app);
        let before = app.ott.as_ref().unwrap().saved.clone();
        let completion = hold_strict_write(&mut app);
        app.ott.as_ref().unwrap().tx.send(completion).unwrap();
        runtime_tick(&mut app);
        let r = app.ott.as_ref().unwrap();
        assert!(!policy::pending(r));
        assert!(!policy::strict(r));
        assert_eq!(serde_json::to_value(&r.saved).unwrap(), serde_json::to_value(before).unwrap());
        let receipt = language(&mut app, owner);
        drain(&mut app);
        assert_eq!(probe.calls(), vec!["language"]);
        assert_eq!(receipt.try_recv().unwrap(), plx_screens::registry::LanguageApplyResult::LiveApplied);
        assert_playing(&app, &identity);
    }

    #[test]
    fn native_preferences_stale_read_only_owner_is_refused_too() {
        let _serial = plx_base::testlock::serial();
        let probe = Probe::new();
        let mut app = fixture();
        let owner = settings(&mut app);
        let (reply, receipt) = mpsc::channel();
        emit(&mut app, owner, PreferenceCmd::Load { reply });
        app.pages.request(MachineId::Nav, NavOp::Root(AppArg::Ott));
        settle_pages(&mut app);
        drain(&mut app);
        assert!(probe.calls().is_empty());
        assert_eq!(receipt.try_recv().unwrap().outcome.err(), Some(plx_plex::plex::account::PreferenceError::Refused));
    }

    fn opening_audio_settings(app: &mut App) -> plx_machine::machine::EntryId {
        app.running = true;
        bridge::show_page(&mut app.pages, AppArg::Ott);
        settle_pages(app);
        bridge::open_settings_at(&mut app.pages, plx_screens::family::SettingsPage::AudioSubtitles);
        let tick = next_tick(app);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
        let surface = app.pages.nav.modals.surfaces.iter().find(|surface|
            matches!(surface.entry.arg, AppArg::Settings(_)))
            .expect("real Audio & Subtitles surface mounted on its first frame");
        assert!(surface.entry.inst.is_some());
        assert_eq!(surface.phase, plx_ui::containers::modal::Phase::Opening);
        assert_eq!(app.pages.nav.input_owner(), Some(InputOwner::Entry(surface.entry.id)),
            "Opening is already the modal input owner");
        assert!(!app.pages.has_pending_navigation(), "the committed opener is not a pending page move");
        surface.entry.id
    }

    #[test]
    fn native_preferences_initial_audio_load_executes_once_while_opening() {
        let _serial = plx_base::testlock::serial();
        let probe = Probe::new();
        let mut app = fixture();
        opening_audio_settings(&mut app);
        drain(&mut app);
        assert_eq!(probe.calls(), vec!["load"], "Enter's automatic Load must not be lost to the opening animation");
        settle_pages(&mut app);
        drain(&mut app);
        assert_eq!(probe.calls(), vec!["load"], "the initial request is not repeated every tick");
    }

    #[test]
    fn native_preferences_initial_audio_load_rejects_superseded_opening() {
        let _serial = plx_base::testlock::serial();
        let probe = Probe::new();
        let mut app = fixture();
        let previous = opening_audio_settings(&mut app);
        // Root(Ott) is deliberately idempotent over the existing Ott host. Use
        // a different real root so the Settings modal actually loses ownership.
        app.pages.request(MachineId::Nav, NavOp::Root(AppArg::Settings(
            plx_screens::family::SettingsPage::Language)));
        settle_pages(&mut app);
        assert!(matches!(app.route(), AppArg::Settings(plx_screens::family::SettingsPage::Language)));
        assert!(!app.pages.has_pending_navigation());
        assert!(!app.pages.nav.tabs.stack.is_pending());
        assert_ne!(app.pages.nav.input_owner(), Some(InputOwner::Entry(previous)),
            "the old Settings modal must lose input ownership before the drain");
        drain(&mut app);
        assert!(probe.calls().is_empty(), "initial Load cannot outlive its retired Settings instance");
    }

    #[test]
    fn native_preferences_initial_audio_load_rejects_covered_opening() {
        let _serial = plx_base::testlock::serial();
        let probe = Probe::new();
        let mut app = fixture();
        let settings = opening_audio_settings(&mut app);
        bridge::open_account_menu(&mut app.pages);
        let tick = next_tick(&mut app);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
        assert_ne!(app.pages.nav.input_owner(), Some(InputOwner::Entry(settings)));
        drain(&mut app);
        assert!(probe.calls().is_empty(), "a covered startup Load cannot fetch account data for the background page");
    }
}
