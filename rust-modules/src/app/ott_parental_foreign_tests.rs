use super::*;

#[path = "ott_parental_queue_tests.rs"]
mod queue_tests;
#[path = "ott_native_start_admission_tests.rs"]
mod owned_start_tests;

struct Registry;
impl Registry {
    fn new() -> Self {
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
        Self
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
    }
}

fn fixture() -> (App, plx_plex::plex::ServerId) {
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    app.running = true;
    let mut init = app.bridge.snapshot_session_init();
    init.epoch = 71;
    init.phase = plx_session::auth::Phase::Ready;
    init.apply_pending = false;
    init.pending_commit = None;
    init.persistence_warning = None;
    app.bridge = crate::app::bridge::Bridge::for_session_test(init);
    let origin = plx_plex::plex::Origin::parse("https://foreign-parental.invalid:443").unwrap();
    let sid = plx_plex::plex::register_pinned_with_client_id(
        "foreign-parental", &origin, "synthetic-token", None, "fixture", Default::default());
    assert!(plx_plex::plex::set_current(sid));
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.loaded = true;
    runtime.state_load = StateLoad::Idle;
    runtime.entropy = Some([1; 16]);
    runtime.clock_sample = Some(plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap());
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    assert!(classic_source::ensure_server_catalog(&mut runtime, &mut app, sid).unwrap());
    runtime.catalog_loading = false;
    runtime.catalog_complete = true;
    runtime.catalog.entries = vec![CatalogEntry {
        protection: None, id: "ordinary".into(), title: "Fixture".into(), category: "Documentary".into(),
        description: String::new(), synopsis: None, poster: String::new(),
        media: MediaLocator::Plex { server_id: "foreign-parental".into(), rating_key: "item".into() },
        duration_ms: Some(10_000), provider_id: Some("item".into()), provider_id_field: None, live: None,
    }];
    app.ott = Some(runtime);
    (app, sid)
}

fn intent(sid: plx_plex::plex::ServerId, key: &str) -> plx_screens::registry::PlayIntent {
    plx_screens::registry::PlayIntent::Item {
        sid, rk: key.into(), part: "/library/parts/1".into(), vcodec: "h264".into(), acodec: "aac".into(),
        title: "Untrusted presentation label".into(), context: String::new(), version: None,
    }
}

#[test]
fn foreign_content_ingress_allows_ordinary_source_owned_leaf_with_default_policy() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (mut app, sid) = fixture();
    assert!(crate::app::ott_runtime::foreign_content_allowed(&mut app), "navigation is not a protected channel");
    assert!(crate::app::content::play_intent_allowed(&mut app, &intent(sid, "item")));
    assert!(app.ott.as_ref().unwrap().saved.settings.get("parentPin").is_none());
    assert!(app.ott.as_ref().unwrap().play_target.is_none(), "authorization cannot itself start playback");
}

#[test]
fn foreign_content_ingress_refuses_protected_unknown_and_ambiguous_targets() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (mut app, sid) = fixture();
    app.ott.as_mut().unwrap().catalog.entries[0].protection = Some(true);
    assert!(!crate::app::content::play_intent_allowed(&mut app, &intent(sid, "item")));
    assert!(!crate::app::content::play_intent_allowed(&mut app, &intent(sid, "missing")));
    let runtime = app.ott.as_mut().unwrap();
    runtime.catalog.entries[0].protection = None;
    let duplicate = runtime.catalog.entries[0].clone();
    runtime.catalog.entries.push(duplicate);
    assert!(!crate::app::content::play_intent_allowed(&mut app, &intent(sid, "item")));
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
}

#[test]
fn foreign_content_ingress_rejects_old_source_receipt_after_session_rebinding() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (mut app, sid) = fixture();
    assert!(crate::app::content::play_intent_allowed(&mut app, &intent(sid, "item")));
    let mut init = app.bridge.snapshot_session_init();
    init.epoch += 1;
    app.bridge = crate::app::bridge::Bridge::for_session_test(init);
    assert!(!crate::app::content::play_intent_allowed(&mut app, &intent(sid, "item")));
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
}

#[test]
fn foreign_content_ingress_keeps_explicit_disable_and_kiosk_fences() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (mut app, sid) = fixture();
    app.ott.as_mut().unwrap().saved.settings.insert("parentPin".into(), json!("*"));
    assert!(crate::app::content::play_intent_allowed(&mut app, &intent(sid, "missing")));
    let runtime = app.ott.as_mut().unwrap();
    let pinned = crate::app::ott_kiosk_stored::Stored::item(Favorite {
        title: "Fixture".into(),
        origin: plx_data::ott::favorites::Origin { provider: "plex".into(), profile: "Plex".into(),
            subprofile: None, source_alias: "Plex".into() },
        locator: plx_data::ott::favorites::Locator::Plex {
            server_id: "foreign-parental".into(), rating_key: "1".into(),
        },
    }, true).unwrap();
    runtime.saved.settings.insert("nativeKiosk".into(), pinned.encode().unwrap());
    assert!(!crate::app::ott_runtime::foreign_content_allowed(&mut app));
}

#[test]
fn foreign_admission_does_not_cancel_an_outgoing_request_to_establish_unknown_source() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (mut app, _) = fixture();
    let origin = plx_plex::plex::Origin::parse("https://other-parental.invalid:443").unwrap();
    let other = plx_plex::plex::register_pinned_with_client_id(
        "other-parental", &origin, "synthetic-token", None, "fixture", Default::default());
    let runtime = app.ott.as_mut().unwrap();
    runtime.play_target = Some(PlaybackRequest::ordinary("ordinary", 0));
    let before = (runtime.generation, runtime.play_generation, runtime.play_target.clone(),
        runtime.effective_profile().cloned(), runtime.catalog.entries.clone());
    assert!(!crate::app::ott_runtime::foreign_play_allowed(&mut app, other, "unknown"));
    let runtime = app.ott.as_ref().unwrap();
    assert!(before == (runtime.generation, runtime.play_generation, runtime.play_target.clone(),
        runtime.effective_profile().cloned(), runtime.catalog.entries.clone()));
}

#[test]
fn missing_parental_runtime_cannot_authorize_foreign_playback() {
    let _serial = plx_base::testlock::serial();
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    app.ott = None;
    assert!(!crate::app::ott_runtime::foreign_play_allowed(
        &mut app, plx_plex::plex::ServerId::default(), "unknown"));
    assert!(!crate::app::ott_runtime::foreign_ui_play_allowed(
        &mut app, plx_plex::plex::ServerId::default(), "unknown"));
}

fn navigation_frames(app: &mut App, now: &mut u32) {
    for _ in 0..32 {
        *now += 16;
        crate::app::bridge::frame(&mut app.pages, &mut app.bridge,
            plx_machine::machine::Tick { ms: *now, dt_us: 16000 }, Vec::new());
    }
}
fn prompt_tick(app: &mut App) {
    let mut runtime = app.ott.take().unwrap();
    parental_runtime::tick(&mut runtime, app);
    app.ott = Some(runtime);
}
fn foreground_home() -> (App, plx_plex::plex::ServerId, u32) {
    let (mut app, sid) = fixture();
    app.ott.as_mut().unwrap().remote = Remote::disabled_for_test("dev_fixture", "fixture");
    crate::app::bridge::show_page(&mut app.pages, AppArg::Home);
    let mut now = 0;
    navigation_frames(&mut app, &mut now);
    let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    crate::app::ott_runtime::tick(&mut app, sample);
    assert!(playback_allowed(&app));
    assert!(matches!(app.route(), AppArg::Home));
    (app, sid, now)
}
fn real_settings_owner(app: &mut App, now: &mut u32) {
    navigation_frames(app, now);
    assert!(matches!(app.route(), AppArg::Ott));
    let owner = app.pages.top_page().unwrap();
    app.ott.as_mut().unwrap().reply = Some(owner);
    prompt_tick(app);
}
fn parental_action(app: &mut App, action: OttAction) -> Result<(), &'static str> {
    let mut runtime = app.ott.take().unwrap();
    let result = handle_action(&mut runtime, app, action);
    app.ott = Some(runtime);
    result
}

#[test]
fn foreign_unknown_local_choice_uses_real_settings_owner_and_preserves_home_return() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (mut app, sid, mut now) = foreground_home();
    let home = app.pages.top_page().unwrap();
    assert!(crate::app::content::play_intent_allowed(&mut app, &intent(sid, "item")));
    assert!(!app.pages.has_pending_navigation(), "ordinary source-owned rows do not open PIN UI");
    assert!(!crate::app::content::play_intent_allowed(&mut app, &intent(sid, "unknown")));
    assert!(app.ott.as_ref().unwrap().edit_session.is_none(), "the old Home is not a fabricated editor owner");
    navigation_frames(&mut app, &mut now);
    let owner = app.pages.top_page().unwrap();
    assert_ne!(owner, home);
    prompt_tick(&mut app);
    assert!(app.ott.as_ref().unwrap().edit_session.is_none(), "new page must actually register its reply");
    app.ott.as_mut().unwrap().reply = Some(owner);
    prompt_tick(&mut app);
    let editor = app.ott.as_ref().unwrap().edit_session.clone().unwrap();
    assert_eq!(editor.owner, owner);
    assert_eq!(editor.key, "parental.unlock");
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    parental_action(&mut app, OttAction::CancelSetting { session: editor.id }).unwrap();
    assert!(app.ott.as_ref().unwrap().edit_session.is_none());
    crate::app::bridge::nav_pop(&mut app.pages);
    navigation_frames(&mut app, &mut now);
    assert_eq!(app.pages.top_page(), Some(home), "PIN Settings must preserve the native page and its retained focus");
    assert!(matches!(app.route(), AppArg::Home));
}

#[test]
fn foreign_deferred_prompt_retires_when_roots_source_or_destination_owner_changes() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    for change in ["roots", "source", "owner"] {
        let (mut app, sid, mut now) = foreground_home();
        let home = app.pages.top_page().unwrap();
        assert!(!crate::app::content::play_intent_allowed(&mut app, &intent(sid, "unknown")));
        match change {
            "roots" => { app.ott.as_mut().unwrap().saved.settings.insert("nativeLocalSources".into(), json!([])); }
            "source" => { plx_plex::plex::grant::network_changed(); }
            "owner" => {
                navigation_frames(&mut app, &mut now);
                prompt_tick(&mut app); // observe the actual destination without granting it a reply
                crate::app::bridge::nav_pop(&mut app.pages);
                navigation_frames(&mut app, &mut now);
                assert_eq!(app.pages.top_page(), Some(home));
                prompt_tick(&mut app); // retirement consumes the deferred prompt
                crate::app::bridge::nav_push(&mut app.pages, AppArg::Ott);
            }
            _ => unreachable!(),
        }
        real_settings_owner(&mut app, &mut now);
        assert!(app.ott.as_ref().unwrap().edit_session.is_none(), "{change} must retire the deferred PIN prompt");
        assert!(app.ott.as_ref().unwrap().play_target.is_none());
    }
}

#[test]
fn foreign_wrong_pin_then_cancel_never_grants_or_starts_the_unknown_choice() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (mut app, sid, mut now) = foreground_home();
    assert!(!crate::app::content::play_intent_allowed(&mut app, &intent(sid, "unknown")));
    real_settings_owner(&mut app, &mut now);
    let editor = app.ott.as_ref().unwrap().edit_session.clone().unwrap();
    parental_action(&mut app, OttAction::SubmitSetting { session: editor.id,
        key: editor.key.clone(), value: "0000".into() }).unwrap();
    let job = app.ott.as_ref().unwrap().rx.recv_timeout(std::time::Duration::from_secs(30))
        .expect("bounded addressed PIN verification");
    let Job::Parental(completion) = job else { panic!("expected PIN verification"); };
    let mut runtime = app.ott.take().unwrap();
    parental_runtime::completed(&mut runtime, &mut app, completion);
    let profile = runtime.effective_profile().unwrap().clone();
    assert!(runtime.require_parental_entry(&profile, None).is_err());
    assert!(runtime.play_target.is_none() && !runtime.saving);
    app.ott = Some(runtime);
    parental_action(&mut app, OttAction::CancelSetting { session: editor.id }).unwrap();
    prompt_tick(&mut app);
    assert!(app.ott.as_ref().unwrap().edit_session.is_none());
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
}

#[test]
fn explicitly_disabled_pin_does_not_bypass_a_stale_source_receipt() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (mut app, sid) = fixture();
    app.ott.as_mut().unwrap().saved.settings.insert("parentPin".into(), json!("*"));
    assert!(crate::app::ott_runtime::foreign_play_allowed(&mut app, sid, "item"));
    let mut init = app.bridge.snapshot_session_init();
    init.epoch += 1;
    app.bridge = crate::app::bridge::Bridge::for_session_test(init);
    assert!(!crate::app::ott_runtime::foreign_play_allowed(&mut app, sid, "item"));
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
}

#[test]
fn cross_source_foreign_choice_preserves_outgoing_until_target_pin_then_fresh_selection_is_admitted() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (mut app, sid, mut now) = foreground_home();
    let runtime = app.ott.as_mut().unwrap();
    classic_source::release(runtime);
    let old = ProviderProfile { id: "outgoing".into(), name: "Fixture".into(),
        kind: ProviderKind::M3u, source: "https://outgoing.invalid/list.m3u".into(),
        source_alias: String::new(), enabled: true };
    let target = ProviderProfile { id: "target-plex".into(), name: "Fixture".into(),
        kind: ProviderKind::Plex, source: "foreign-parental".into(),
        source_alias: String::new(), enabled: true };
    runtime.saved.profiles.profiles = vec![old.clone(), target.clone()];
    runtime.saved.profiles.active_profile = Some(old.id.clone());
    runtime.saved.settings.insert("nativePlexSources".into(), json!([{
        "machine_id":"foreign-parental", "name":"Fixture", "address":"foreign-parental.invalid",
        "port":443, "origin":"https://foreign-parental.invalid:443", "token":"synthetic-token"
    }]));
    runtime.provider = old.kind;
    runtime.play_target = Some(PlaybackRequest::ordinary("outgoing", 0));
    let mut entry = runtime.catalog.entries[0].clone();
    entry.id = "outgoing".into();
    entry.media = MediaLocator::Http { url:"https://outgoing.invalid/live.m3u8".into() };
    runtime.catalog.entries = vec![entry.clone()];
    runtime.playing = Some(Playing { channel_capture:None, kiosk_selection:None,
        archive:None, transient_owner:None, local_selection:None, identity:None,
        profile:old.clone(), entry, selection_id:"outgoing".into(), source:None,
        advance_on_failure:false, requires_server_lease:false });
    let before = (runtime.generation, runtime.play_generation, runtime.play_target.clone(),
        runtime.effective_profile().cloned(), runtime.catalog.entries.clone());
    assert!(!crate::app::content::play_intent_allowed(&mut app, &intent(sid, "unknown")));
    let runtime = app.ott.as_ref().unwrap();
    assert!(before == (runtime.generation, runtime.play_generation, runtime.play_target.clone(),
        runtime.effective_profile().cloned(), runtime.catalog.entries.clone()));
    assert!(runtime.playing.as_ref().is_some_and(|playing| playing.profile == old));
    real_settings_owner(&mut app, &mut now);
    let editor = app.ott.as_ref().unwrap().edit_session.clone().expect("target-specific PIN prompt");
    parental_action(&mut app, OttAction::SubmitSetting { session:editor.id,
        key:editor.key.clone(), value:"1234".into() }).unwrap();
    let job = app.ott.as_ref().unwrap().rx.recv_timeout(std::time::Duration::from_secs(30))
        .expect("bounded target PIN verification");
    let Job::Parental(completion) = job else { panic!("expected PIN verification"); };
    let mut runtime = app.ott.take().unwrap();
    parental_runtime::completed(&mut runtime, &mut app, completion);
    assert!(runtime.require_parental_entry(&target, None).is_ok());
    assert!(runtime.require_parental_entry(&old, None).is_err(), "a target B grant is not an outgoing A grant");
    app.ott = Some(runtime);
    assert!(crate::app::ott_runtime::foreign_play_allowed(&mut app, sid, "unknown"),
        "a fresh exact-source selection after target PIN must not dead-end");
    assert!(app.ott.as_ref().unwrap().effective_profile() == Some(&target));
    plx_media::route::install_plex_request_for_test(&mut app.player.session, sid, "unknown");
    assert!(crate::app::ott_runtime::foreign_native_start_allowed(&mut app),
        "the final async native admission must verify the bound target and retained request");
    let identity = plx_media::route::playback_identity(&app.player.session);
    let runtime = app.ott.as_mut().unwrap();
    let sample = runtime.clock_sample.unwrap();
    runtime.clock_sample = Some(sample.advance_for_test(std::time::Duration::from_secs(3601)));
    assert!(!crate::app::ott_runtime::foreign_native_start_allowed(&mut app),
        "expired target consent must refuse the eventual native Load");
    assert!(plx_media::route::playback_identity(&app.player.session) == identity);
    assert!(!app.adapters.player.is_live(), "final admission checks cannot start a decoder");
}

#[test]
fn explicitly_disabled_pin_does_not_hide_a_retired_classic_generation() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (mut app, sid) = fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.settings.insert("parentPin".into(), json!("*"));
    runtime.generation = runtime.generation.wrapping_add(1);
    assert!(classic_source::profile(runtime).is_none());
    assert!(!crate::app::ott_runtime::foreign_play_allowed(&mut app, sid, "item"));
}

#[test]
fn foreign_native_start_requires_a_retained_request_or_typed_external_source() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let (mut app, sid) = fixture();
    assert!(plx_media::route::playback_request_identity(&app.player.session).is_none());
    assert!(plx_media::route::external_source(&app.player.session).is_none());
    assert!(!crate::app::ott_runtime::foreign_native_start_allowed(&mut app));
    plx_media::route::install_plex_request_for_test(&mut app.player.session, sid, "item");
    assert!(crate::app::ott_runtime::foreign_native_start_allowed(&mut app));
    app.ott.as_mut().unwrap().catalog.entries[0].protection = Some(true);
    assert!(!crate::app::ott_runtime::foreign_native_start_allowed(&mut app));
    assert!(!app.adapters.player.is_live());
}
