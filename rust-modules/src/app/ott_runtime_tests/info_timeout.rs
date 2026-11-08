//! Regression-first tests use existing settings and mounter entry points.
//! No media IO; the only persistence attempt uses a revoked writer permit.
use super::*;

struct Reset(plx_machine::machine::FontScale);
impl Drop for Reset { fn drop(&mut self) { plx_gfx::text::set_ui_typography(self.0); } }
// Declare before App so its teardown cannot overwrite the captured transport state.
struct TransportPaused(bool);
impl TransportPaused {
    fn keep() -> Self { Self(plx_media::player::transport::paused()) }
}
impl Drop for TransportPaused {
    fn drop(&mut self) { plx_media::player::transport::set_paused(self.0); }
}

fn prepared() -> App {
    let mut app = app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.entropy = Some([1; 16]);
    runtime.write_permit.revoke();
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.continuity.stop(runtime.generation);
    app
}
fn load(app: &mut App, saved: Saved) {
    app.ott.as_ref().unwrap().tx.send(Job::Loaded(Ok((saved, [1; 16])))).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(app);
    assert!(app.ott.as_ref().unwrap().loaded);
}
fn query(app: &mut App, id: u64, mut params: Value) -> Option<Value> {
    let mut runtime = app.ott.take().unwrap();
    params["runtime"] = json!(runtime.remote.runtime());
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{id:032x}"),"action":"player_settings","params":params,"expires_at":1020
    }]}).to_string();
    let mut requests = runtime.remote.raw_poll_for_test(&body, runtime.now_ms());
    assert_eq!(requests.len(), 1);
    let inv = requests.pop().unwrap();
    let response = match super::super::input_policy::remote(&mut runtime, &inv) {
        Ok(()) => commands::command(&mut runtime, app, &inv),
        Err(error) => Some(json!({"status":"rejected","data":{"error":error}})),
    };
    app.ott = Some(runtime);
    response
}
fn get(app: &mut App, id: u64) -> Value {
    query(app, id, json!({"op":"get","id":"infoTimeout"})).unwrap()
}

#[test]
fn info_timeout_default_is_a_real_setting_and_visible_editor_in_both_modes() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    for mode in [0, 1] {
        let mut app = prepared();
        let mut saved = Saved::default();
        saved.settings.insert("interfaceMode".into(), json!(mode));
        load(&mut app, saved);
        app.ott.as_mut().unwrap().page = "settings";
        bridge::show_page(&mut app.pages, AppArg::Ott);
        settle_pages(&mut app);
        runtime_tick(&mut app);
        settle_pages(&mut app);
        let setting = get(&mut app, 1);
        assert_eq!(setting["status"], "ok", "infoTimeout must exist, not be an inert saved key");
        assert_eq!(setting["data"]["setting"]["value"], 5);
        assert_eq!(setting["data"]["setting"]["effective_value"], 5);
        let opener = app.ott.as_ref().unwrap().view().rows.into_iter().find(|r| r.id == "settings.open:infobar")
            .expect("ordinary HUD timeout must be reachable through Infobar");
        assert!(opener.enabled);
        assert_eq!(opener.action, OttAction::EditSetting("settings.open:infobar".into()));
        let mut runtime = app.ott.take().unwrap();
        runtime.reply = app.pages.top_page();
        handle_action(&mut runtime, &mut app, opener.action).unwrap();
        let row = runtime.view().rows.into_iter().find(|r| matches!(&r.action,
            OttAction::EditSetting(key) if key.ends_with(":pick:infoTimeout"))).unwrap();
        assert!(row.enabled);
        assert_eq!(row.detail, "5 s");
        handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
        // Keep testing the existing scalar editor as well as the grouped entry.
        handle_action(&mut runtime, &mut app, OttAction::EditSetting("infoTimeout".into())).unwrap();
        let editor = runtime.edit_session.clone().expect("existing editor owns the setting");
        let before = serde_json::to_value(&runtime.saved).unwrap();
        handle_action(&mut runtime, &mut app, OttAction::CancelSetting { session: editor.id }).unwrap();
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert!(!runtime.saving);
        app.ott = Some(runtime);
    }
}

#[test]
fn info_timeout_loaded_value_reaches_new_player_mount_in_both_modes() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    for mode in [0, 1] {
        for seconds in [3_u32, 20] {
            let mut app = prepared();
            let mut saved = Saved::default();
            saved.settings.insert("interfaceMode".into(), json!(mode));
            saved.settings.insert("infoTimeout".into(), json!(seconds));
            load(&mut app, saved);
            let before = app.loop_t;
            mount_player(&mut app);
            let until = bridge::player(&app.pages).expect("real Player mounter").hud.until;
            assert!(until >= before + seconds * 1000,
                "accepted {seconds}s must reach mount, not the old fixed 4.5s deadline: {until}");
            assert!(until <= app.loop_t + seconds * 1000);
        }
    }
}

#[test]
fn info_timeout_failed_save_does_not_change_accepted_setting() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    let mut app = prepared();
    load(&mut app, Saved::default());
    let before = get(&mut app, 1);
    assert_eq!(before["status"], "ok");
    let pending = query(&mut app, 2, json!({"op":"set","id":"infoTimeout",
        "context":before["data"]["context"]["token"],"revision":before["data"]["setting"]["revision"],"value":20}));
    assert!(pending.is_none(), "Save result waits for existing durable completion");
    assert_eq!(get(&mut app, 3)["data"]["reason"], "revision_conflict");
    assert!(!app.ott.as_ref().unwrap().saved.settings.contains_key("infoTimeout"));
    let failed = app.ott.as_ref().unwrap().rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(&failed, Job::Saved(Err(_), _, false, _)));
    app.ott.as_ref().unwrap().tx.send(failed).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert_eq!(get(&mut app, 4)["data"]["setting"]["value"], 5);
    assert!(!app.ott.as_ref().unwrap().saving);
}

fn hud_request(app: &mut App, request: PlayerReq) -> u32 {
    app.pages.emit(MachineId::Nav, Fx::App(AppFx::Player(request)));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    let frame = run::Frame { ctrl: plx_appkit::player_hud::slot(&app.player.session, app.bridge.metadata_view()),
        now: tick.ms, dt: 0.016, underlay_moving: false, player: true, present: false, deferred: false, rn: "player" };
    content::content_requests(app, &frame);
    tick.ms
}

#[test]
fn info_timeout_remote_values_are_strict_and_success_updates_existing_player_only_after_saved() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    let mut app = prepared();
    load(&mut app, Saved::default());
    mount_player(&mut app);
    let before = get(&mut app, 20);
    assert_eq!(before["data"]["descriptor"]["type"], "integer");
    assert_eq!(before["data"]["descriptor"]["constraints"], json!({"minimum":3,"maximum":20}));
    assert_eq!(before["data"]["descriptor"]["unit"], "seconds");
    assert_eq!(before["data"]["descriptor"]["access"], json!({"read":true,"write":true}));
    for (n, value) in [json!(2), json!(21), json!(5.0), json!(true), json!("5"), Value::Null].into_iter().enumerate() {
        let result = query(&mut app, 21 + n as u64, json!({"op":"set","id":"infoTimeout",
            "context":before["data"]["context"]["token"],"revision":before["data"]["setting"]["revision"],"value":value})).unwrap();
        assert_eq!(result["data"]["reason"], "invalid_value");
        assert!(!app.ott.as_ref().unwrap().saving);
    }
    let held = bridge::player(&app.pages).unwrap().hud.until;
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    assert!(query(&mut app, 40, json!({"op":"set","id":"infoTimeout",
        "context":before["data"]["context"]["token"],"revision":before["data"]["setting"]["revision"],"value":20})).is_none());
    assert_eq!(app.bridge.info_timeout().seconds(), 5);
    assert_eq!(bridge::player(&app.pages).unwrap().hud.info_timeout().seconds(), 5);
    let Job::Saved(Err(_), Some(invocation), false, response) = app.ott.as_ref().unwrap().rx.recv_timeout(Duration::from_secs(1)).unwrap() else {
        panic!("revoked writer must settle before synthetic accepted completion");
    };
    let id = invocation.id.clone();
    assert!(app.ott.as_ref().unwrap().remote.response_for_id_for_test(&id).is_none());
    let mut accepted = app.ott.as_ref().unwrap().saved.clone();
    accepted.settings.insert("infoTimeout".into(), json!(20));
    let digest = plx_base::sha256::sha256(json!(id).to_string().as_bytes());
    let revision: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    accepted.settings.insert("settingRevision:infoTimeout".into(), json!(revision));
    app.ott.as_ref().unwrap().tx.send(Job::Saved(Ok(accepted), Some(invocation), false, response)).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert_eq!(get(&mut app, 41)["data"]["setting"]["effective_value"], 20);
    assert_eq!(app.bridge.info_timeout().seconds(), 20);
    assert_eq!(bridge::player(&app.pages).unwrap().hud.info_timeout().seconds(), 20);
    assert_eq!(bridge::player(&app.pages).unwrap().hud.until, held, "configuration must not reveal or shorten existing HUD");
    assert_eq!(app.ott.as_ref().unwrap().playback_epoch, epoch);
    let now = hud_request(&mut app, PlayerReq::ExtendDefaultHud);
    assert_eq!(bridge::player(&app.pages).unwrap().hud.until, now + 20_000);
    let stale = query(&mut app, 42, json!({"op":"set","id":"infoTimeout",
        "context":before["data"]["context"]["token"],"revision":before["data"]["setting"]["revision"],"value":3})).unwrap();
    assert_eq!(stale["data"]["reason"], "revision_conflict");
}

#[test]
fn info_timeout_delayed_mount_resolves_accepted_value_and_fixed_capture_stays_fixed() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    use plx_screens::player::input::HudStart;
    for hold in [HudStart::Ordinary, HudStart::Fixed(60_000)] {
        let mut app = prepared();
        // The start request predates accepted settings and does not capture 5 seconds.
        app.bridge.seed_player_hud(hold);
        let mut saved = Saved::default();
        saved.settings.insert("infoTimeout".into(), json!(20));
        load(&mut app, saved);
        let before = app.loop_t;
        mount_player(&mut app);
        let expected = if hold == HudStart::Ordinary { 20_000 } else { 60_000 };
        let until = bridge::player(&app.pages).unwrap().hud.until;
        assert!(until >= before + expected && until <= app.loop_t + expected);
        let old = until;
        let now = hud_request(&mut app, PlayerReq::ExtendHud(8_000));
        assert_eq!(bridge::player(&app.pages).unwrap().hud.until, old.max(now + 8_000));
        let old = bridge::player(&app.pages).unwrap().hud.until;
        let now = hud_request(&mut app, PlayerReq::ExtendDefaultHud);
        assert_eq!(bridge::player(&app.pages).unwrap().hud.until, old.max(now + 20_000));
    }
}

#[test]
fn info_timeout_local_editor_rejects_stale_and_invalid_submissions_and_keeps_live_value() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    let mut app = prepared();
    load(&mut app, Saved::default());
    app.ott.as_mut().unwrap().page = "settings";
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    let mut runtime = app.ott.take().unwrap();
    runtime.reply = app.pages.top_page();
    handle_action(&mut runtime, &mut app, OttAction::EditSetting("infoTimeout".into())).unwrap();
    let editor = runtime.edit_session.clone().unwrap();
    for value in ["", "2", "21", "5.0", "+5", "true", "100"] {
        assert_eq!(handle_action(&mut runtime, &mut app, OttAction::SubmitSetting {
            session: editor.id, key: editor.key.clone(), value: value.into(),
        }), Err("Enter an integer from 3 to 20"));
        assert!(!runtime.saving);
        assert_eq!(runtime.info_timeout().seconds(), 5);
    }
    assert!(handle_action(&mut runtime, &mut app, OttAction::SubmitSetting {
        session: editor.id + 1, key: editor.key.clone(), value: "3".into(),
    }).is_err());
    assert!(!runtime.saving);
    let action = OttAction::SubmitSetting { session: editor.id, key: editor.key.clone(), value: "3".into() };
    handle_action(&mut runtime, &mut app, action.clone()).unwrap();
    handle_action(&mut runtime, &mut app, action).unwrap();
    assert!(runtime.saving);
    assert_eq!(runtime.info_timeout().seconds(), 5);
    assert_eq!(app.bridge.info_timeout().seconds(), 5);
    let failed = runtime.rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(&failed, Job::Saved(Err(_), _, false, _)));
    assert!(runtime.rx.try_recv().is_err(), "duplicate Submit must not launch a second write");
    app.ott = Some(runtime);
    app.ott.as_ref().unwrap().tx.send(failed).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert_eq!(app.bridge.info_timeout().seconds(), 5);
}

struct NoMediaStart;
impl playback::PlaybackResources for NoMediaStart {
    fn request_movie(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_data::stores::metadata::MetadataStore, _: &plx_data::pms::PmsMovie,
        _: Option<plx_plex::plex::MediaId>) -> bool { panic!("not a media request test") }
    fn request_episode(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_data::stores::metadata::MetadataStore, _: &str,
        _: Option<plx_plex::plex::MediaId>) -> bool { panic!("not a media request test") }
    fn describe_movie(&mut self, _: &mut plx_data::stores::metadata::MetadataStore,
        _: plx_plex::plex::ServerId, _: &str) { panic!("not a media request test") }
    fn prepare_start(&mut self, _: &mut plx_media::route::PlaybackSession,
        _: &mut plx_media::player::adapter::PlayerAdapter, _: i64) -> bool { false }
}

#[test]
fn info_timeout_same_screen_start_resets_transient_hud_but_keeps_accepted_configuration() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    let _paused = TransportPaused::keep();
    use plx_screens::player::input::HudStart;
    let mut app = prepared();
    let mut saved = Saved::default();
    saved.settings.insert("infoTimeout".into(), json!(20));
    load(&mut app, saved);
    mount_player(&mut app);
    let owner = app.pages.top_page();
    bridge::player_mut(&mut app.pages).unwrap().hud.dismissed = true;
    let before = clock::now();
    assert!(!playback::start_playback_with(&mut app.player.session, &mut app.adapters.player, 0,
        playback::Origin::Unchanged, HudStart::Ordinary, None, &mut app.pages, &mut app.bridge,
        &mut NoMediaStart));
    let after = clock::now();
    let hud = &bridge::player(&app.pages).unwrap().hud;
    assert_eq!(app.pages.top_page(), owner);
    assert_eq!(hud.info_timeout().seconds(), 20);
    assert!(!hud.dismissed);
    assert!(hud.until >= before.saturating_add(20_000) && hud.until <= after.saturating_add(20_000));
}

// Own App so cancellation/drain precedes its teardown even if a pending-chain assertion fails.
struct PendingApp(App);
impl Drop for PendingApp {
    fn drop(&mut self) {
        plx_media::route::cancel_play(&mut self.0.player.session);
        let drained = plx_media::route::wait_resolve_workers_for_test(Duration::from_secs(10));
        plx_media::route::reset_player_control_for_test(&self.0.player.session);
        plx_media::player::SHARED.reset_session();
        plx_plex::plex::reset_servers_for_test();
        if !std::thread::panicking() { assert!(drained, "pending-chain fixture must drain its resolver"); }
    }
}

#[test]
fn info_timeout_pending_menu_play_uses_value_accepted_after_the_request() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(plx_ui::typography::current());
    let _paused = TransportPaused::keep();
    let mut owned = PendingApp(prepared());
    let app = &mut owned.0;
    // A registered server is deliberately absent, as in the route ticket test: the accepted
    // native request can create/drain a resolver but cannot issue a network request.
    plx_plex::plex::reset_servers_for_test();
    load(app, Saved::default());
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(app);
    let sid = plx_plex::plex::ServerId::from_raw(0);
    let now = app.loop_t;
    let mut pending = Some(crate::app::input::MenuPlayAwait::for_link(sid, "pending-info".into(), false, now));
    unsafe {
        crate::app::input::menu_play_tick(&mut app.player.session, &mut app.adapters.player,
            &mut app.pages, &mut app.bridge, &mut pending, now);
    }
    assert!(pending.is_some(), "the real menu continuation waits for its addressed item");
    assert!(bridge::player(&app.pages).is_none());
    let before = get(app, 60);
    assert!(query(app, 61, json!({"op":"set","id":"infoTimeout",
        "context":before["data"]["context"]["token"],"revision":before["data"]["setting"]["revision"],"value":20})).is_none());
    assert_eq!(app.bridge.info_timeout().seconds(), 5);
    // Resolve the real pending transaction with a typed accepted completion, without a disk write.
    let Job::Saved(Err(_), Some(invocation), false, response) = app.ott.as_ref().unwrap().rx.recv_timeout(Duration::from_secs(1)).unwrap() else {
        panic!("revoked writer must settle before synthetic accepted completion");
    };
    let mut accepted = app.ott.as_ref().unwrap().saved.clone();
    accepted.settings.insert("infoTimeout".into(), json!(20));
    let digest = plx_base::sha256::sha256(json!(invocation.id).to_string().as_bytes());
    let revision: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    accepted.settings.insert("settingRevision:infoTimeout".into(), json!(revision));
    app.ott.as_ref().unwrap().tx.send(Job::Saved(Ok(accepted), Some(invocation), false, response))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(app);
    assert_eq!(app.bridge.info_timeout().seconds(), 20);
    assert!(pending.is_some());
    // Land only the source data; keep the production continuation and start/mount dispatch.
    plx_data::metadata::set_current_for_test(app.bridge.metadata_mut().state_mut(), Some(plx_data::metadata::Detail {
        sid, rk: "pending-info".into(), kind: "movie".into(), title: "Fixture".into(),
        part: "/library/parts/2/file.mkv".into(), vcodec: "h264".into(), acodec: "aac".into(),
        ..Default::default()
    }));
    let ticket = plx_media::route::play_request_ticket();
    unsafe {
        crate::app::input::menu_play_tick(&mut app.player.session, &mut app.adapters.player,
            &mut app.pages, &mut app.bridge, &mut pending, now + 16);
    }
    assert!(pending.is_none(), "the addressed landing must consume the actual continuation");
    assert!(!plx_media::route::play_request_is_current(ticket), "production Play admission ran");
    let before_mount = app.loop_t;
    settle_pages(app);
    let hud = &bridge::player(&app.pages).expect("real menu continuation entered Player").hud;
    assert_eq!(hud.info_timeout().seconds(), 20);
    assert!(hud.until >= before_mount + 20_000 && hud.until <= app.loop_t + 20_000,
        "the continuation must not capture the old 5-second value: {}", hud.until);
}
