//! Automatic boot presentation never replaces catalogue/admission authority.
use super::*;

fn initial_app(saved: Saved) -> App {
    let mut app = app_fixture();
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    // Hold all IO workers. The real Loaded handler and continuity run below.
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    super::super::boot_resume::arm(runtime);
    assert!(runtime.view().startup_pending);
    runtime.tx.send(Job::Loaded(Ok((saved, [1; 16])))).unwrap();
    runtime_tick(&mut app);
    app
}

fn configured() -> Saved {
    let profile = synthetic_profile();
    let mut saved = Saved::default();
    saved.profiles.active_profile = Some(profile.id.clone());
    saved.profiles.profiles.push(profile);
    saved
}

#[test]
fn configured_boot_covers_real_catalogue_navigation_without_fake_player() {
    let _lock = plx_base::testlock::serial();
    let app = initial_app(configured());
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.loaded);
    assert_eq!(runtime.page, "channels");
    assert!(runtime.catalog_loading);
    assert!(runtime.view().startup_pending);
    assert!(matches!(app.route(), AppArg::Ott));
    assert!(runtime.playing.is_none());
    assert!(!app.adapters.player.is_live());
}

#[test]
fn unconfigured_and_failed_state_expose_honest_setup_or_recovery() {
    let _lock = plx_base::testlock::serial();
    let mut app = initial_app(Saved::default());
    assert!(!app.ott.as_ref().unwrap().view().startup_pending);
    assert_eq!(app.ott.as_ref().unwrap().page, "providers");
    let runtime = app.ott.as_mut().unwrap();
    super::super::boot_resume::arm(runtime);
    runtime.tx.send(Job::Loaded(Err("State unavailable"))).unwrap();
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.view().startup_pending);
    assert!(!runtime.loaded);
    assert!(matches!(runtime.state_load, StateLoad::Failed(_)));
}

#[test]
fn stale_saved_resume_does_not_become_playback_authority() {
    let _lock = plx_base::testlock::serial();
    let mut saved = configured();
    // Key format is source-owned; a mismatched fingerprint is independently
    // ineligible even when an old checkpoint is present in the document.
    let profile = saved.profiles.profiles[0].clone();
    let mut probe = runtime_fixture();
    probe.saved = saved.clone();
    let key = probe.resume_key().unwrap();
    saved.resume.insert(key, SavedResume { entry_id: "missing".into(),
        position_ms: 40_000, source_fingerprint: "different-source".into() });
    assert_ne!(profile_fingerprint(&profile), "different-source");
    let app = initial_app(saved);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.continuity.pending_restore().is_none());
    assert!(runtime.view().startup_pending);
    assert!(runtime.play_target.is_none());
}

#[test]
fn back_cancels_restore_but_preserves_progressive_catalogue() {
    let _lock = plx_base::testlock::serial();
    let mut app = initial_app(configured());
    let mut runtime = app.ott.take().unwrap();
    runtime.reply = current_ui_owner(&app);
    assert!(runtime.reply.is_some());
    let generation = runtime.generation;
    let play_generation = runtime.play_generation;
    assert_eq!(super::super::boot_resume::back(&mut runtime, &app, &OttAction::Back), Some(Ok(())));
    assert!(!runtime.view().startup_pending);
    assert_eq!(runtime.generation, generation);
    assert!(runtime.catalog_loading);
    assert_ne!(runtime.play_generation, play_generation);
    assert!(runtime.resume_pending.is_none());
    assert!(runtime.continuity.pending_restore().is_none());
    // Refresh and later reconciliation cannot re-arm automatic restore.
    super::super::boot_resume::reconcile(&mut runtime, &app);
    assert!(!runtime.view().startup_pending);
    app.ott = Some(runtime);
}

#[test]
fn back_requires_current_ui_owner_and_authoritative_policy() {
    let _lock = plx_base::testlock::serial();
    let app = initial_app(configured());
    let mut runtime = runtime_fixture();
    super::super::boot_resume::arm(&mut runtime);
    assert!(super::super::boot_resume::back(&mut runtime, &app, &OttAction::Back).unwrap().is_err());
    runtime.reply = current_ui_owner(&app);
    assert!(runtime.reply.is_some());
    assert!(super::super::boot_resume::back(&mut runtime, &app, &OttAction::Back).unwrap().is_err());
    assert!(runtime.view().startup_pending, "unknown policy cannot cancel startup");
}

#[test]
fn changed_source_or_accepted_navigation_retires_only_presentation() {
    let _lock = plx_base::testlock::serial();
    let mut app = initial_app(configured());
    let mut runtime = app.ott.take().unwrap();
    runtime.saved.profiles.profiles[0].source.push_str("?changed=1");
    super::super::boot_resume::reconcile(&mut runtime, &app);
    assert!(!runtime.view().startup_pending);
    super::super::boot_resume::arm(&mut runtime);
    runtime.accepted_navigation(false);
    assert!(!runtime.view().startup_pending);
    app.ott = Some(runtime);
}

#[test]
fn synchronous_admission_refusal_reveals_catalogue_error() {
    let _lock = plx_base::testlock::serial();
    let mut app = initial_app(configured());
    let runtime = app.ott.as_mut().unwrap();
    runtime.apply_continuity(continuity::Decision::Start {
        entry_id: "not-in-the-catalogue".into(), position_ms: 0,
        reason: continuity::Reason::First,
    });
    assert!(!runtime.view().startup_pending);
    assert!(!runtime.status.is_empty());
    assert!(runtime.playing.is_none());
}

#[test]
fn automatic_resolver_projects_exact_target_source_number_and_accepted_opacity() {
    let _lock = plx_base::testlock::serial();
    let mut saved = configured();
    let profile = &mut saved.profiles.profiles[0];
    profile.kind = ProviderKind::M3u;
    profile.source = "https://boot.invalid/list.m3u".into();
    saved.settings.insert("osdOpacity".into(), json!(3));
    let mut app = initial_app(saved);
    let runtime = app.ott.as_mut().unwrap();
    runtime.catalog = plx_data::ott::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"First\",One\nhttps://boot.invalid/one.m3u8\n#EXTINF:-1 group-title=\"Second\",Two\nhttps://boot.invalid/two.m3u8\n",
        runtime.active().unwrap()).unwrap();
    let id = runtime.catalog.entries[1].id.clone();
    runtime.catalog.entries[1].description = "Target description".into();
    runtime.catalog.entries[1].poster = "https://boot.invalid/two.png".into();
    runtime.apply_continuity(continuity::Decision::Start {
        entry_id: id.clone(), position_ms: 0, reason: continuity::Reason::First,
    });
    assert!(runtime.resolver_next.is_some(), "normal queue must own the resolver request");
    // A target footer is independent of the presently filtered list. Changing
    // this view does not change the queued source or its canonical number.
    runtime.saved.category = runtime.catalog.category_choices().into_iter()
        .find(|category| category.label == "First").expect("parsed M3U category").id;
    let view = runtime.view();
    assert!(!view.rows.iter().any(|row| row.id == id));
    let display = view.pending_display.as_ref().expect("source-owned target facts");
    assert_eq!(display.title, "Two");
    assert_eq!(display.description, "Target description");
    assert!(display.live, "live is a source fact even with no EPG");
    assert_eq!(display.ordinal, Some(2));
    assert_eq!(display.poster, Some(plx_data::metadata::MediaArt::Http {
        url: "https://boot.invalid/two.png".into() }));
    assert!(view.startup_pending);
    assert_eq!(view.startup_target.as_deref(), Some(id.as_str()));
    assert_eq!(view.pending_play, None, "automatic restore is not an interactive receipt");
    assert_eq!(view.pending_play_ordinal, Some(2));
    assert_eq!(view.osd_opacity.level(), 3);
    assert!(runtime.playing.is_none(), "display snapshot never installs a decoder");
    runtime.play_cancel.store(true, Ordering::Release);
    let view = runtime.view();
    assert!(view.startup_target.is_none());
    assert!(view.pending_display.is_none());
    assert!(view.pending_play_ordinal.is_none());
    assert_eq!(view.osd_opacity.level(), 3);
}

struct NativeSession;
impl NativeSession {
    fn open() -> Self {
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
        let origin = plx_plex::plex::Origin::parse("https://boot-session.invalid:443").unwrap();
        let sid = plx_plex::plex::register_pinned_with_client_id("boot-session", &origin,
            "synthetic-token", None, "synthetic-client", Default::default());
        assert!(plx_plex::plex::set_current(sid));
        Self
    }
}
impl Drop for NativeSession {
    fn drop(&mut self) {
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
    }
}

fn authorized_home() -> App {
    let mut app = app_fixture();
    // The generic lifecycle fixture is stopped by default; this scenario models
    // the real running application before its protected boot image arrives.
    app.running = true;
    let mut init = plx_session::auth::SessionInit::captured(Default::default());
    init.epoch = 10;
    init.phase = plx_session::auth::Phase::Ready;
    app.bridge = bridge::Bridge::for_session_test(init);
    assert!(app.bridge.session_ready_current());
    bridge::nav_root(&mut app.pages, AppArg::Home);
    settle_pages(&mut app);
    app.last_input = app.t0;
    let runtime = app.ott.as_mut().unwrap();
    runtime.state_load = StateLoad::Loading;
    super::super::boot_resume::arm(runtime);
    app
}

#[test]
fn authorized_home_cover_waits_only_for_unknown_or_exact_stored_ott_intent() {
    let _lock = plx_base::testlock::serial();
    let _session = NativeSession::open();
    let mut app = authorized_home();
    assert!(super::super::home_boot_pending(&app));
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.state_load = StateLoad::Idle;
    runtime.saved = configured();
    runtime.saved.settings.insert("interfaceMode".into(), json!(0));
    runtime.public_preferences.accept(None);
    app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Classic);
    assert!(super::super::home_boot_pending(&app));
    app.ott.as_mut().unwrap().saved.profiles.profiles[0].kind = ProviderKind::Plex;
    assert!(!super::super::home_boot_pending(&app), "ordinary native Plex Home is revealed");
    app.ott.as_mut().unwrap().saved = Saved::default();
    assert!(!super::super::home_boot_pending(&app), "unconfigured Home must remain usable");
    app.ott.as_mut().unwrap().loaded = false;
    app.ott.as_mut().unwrap().state_load = StateLoad::Failed("fixture error");
    assert!(!super::super::home_boot_pending(&app), "state errors must reveal recovery");
}

#[test]
fn home_cover_never_covers_auth_or_consent_and_ingress_cannot_act_on_hidden_home() {
    let _lock = plx_base::testlock::serial();
    let _session = NativeSession::open();
    let mut app = authorized_home();
    let stamp = app.last_input;
    let mut frame = run::Frame {
        ctrl: plx_appkit::player_hud::ControlSlot::Discs(plx_appkit::player_hud::Controls::NONE),
        now: 1_000, dt: 0.016, underlay_moving: false, player: false,
        present: false, deferred: false, rn: "home",
    };
    // Every SDL user-input family is fenced before native decoding or activity
    // stamps, so the still-hidden Home cannot receive delayed clicks/presses.
    for event in [0x300_u32, 0x301, 0x302, 0x303, 0x400, 0x401, 0x402, 0x403] {
        app.ev[..4].copy_from_slice(&event.to_ne_bytes());
        unsafe { run::ingest_sdl_event_with_window(&mut app, &mut frame, |_| panic!("no window operation")); }
        assert_eq!(app.last_input, stamp);
        assert!(app.inputs.is_empty());
        assert!(super::super::home_boot_pending(&app));
    }
    bridge::nav_root(&mut app.pages, AppArg::Onboard);
    settle_pages(&mut app);
    assert!(!super::super::home_boot_pending(&app));
    bridge::nav_root(&mut app.pages, AppArg::Home);
    settle_pages(&mut app);
    let mut init = plx_session::auth::SessionInit::captured(Default::default());
    init.epoch = 11;
    init.phase = plx_session::auth::Phase::Idle;
    app.bridge = bridge::Bridge::for_session_test(init);
    assert!(!super::super::home_boot_pending(&app));
}

#[test]
fn exact_stored_ott_intent_hands_home_to_existing_navigation_owner() {
    let _lock = plx_base::testlock::serial();
    let _session = NativeSession::open();
    let mut app = authorized_home();
    app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Classic);
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.state_load = StateLoad::Idle;
    runtime.saved = configured();
    runtime.saved.settings.insert("interfaceMode".into(), json!(0));
    runtime.public_preferences.accept(None);
    assert!(super::super::home_boot_pending(&app));
    assert!(super::super::reconcile_classic_navigation(&mut app));
    assert!(super::super::home_boot_pending(&app), "exact queued Root retains its cover");
    let source = app.ott.as_ref().unwrap().saved.profiles.profiles[0].source.clone();
    app.ott.as_mut().unwrap().saved.profiles.profiles[0].source.push_str("?replaced=1");
    assert!(!super::super::home_boot_pending(&app), "queued cover cannot adopt another source");
    app.ott.as_mut().unwrap().saved.profiles.profiles[0].source = source;
    settle_pages(&mut app);
    assert!(matches!(app.route(), AppArg::Ott));
    assert!(!super::super::home_boot_pending(&app));
    assert!(app.ott.as_ref().unwrap().view().startup_pending,
        "existing OTT cover takes over without a forged Player route");
    assert!(app.ott.as_ref().unwrap().playing.is_none());
}


#[test]
fn home_cover_shares_coordinator_refusal_gates_instead_of_trapping_home() {
    let _lock = plx_base::testlock::serial();
    let _session = NativeSession::open();
    for gate in ["preferences", "mode", "uncertain", "suspended", "policy"] {
        let mut app = authorized_home();
        app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Classic);
        let runtime = app.ott.as_mut().unwrap();
        runtime.loaded = true;
        runtime.state_load = StateLoad::Idle;
        runtime.saved = configured();
        runtime.saved.settings.insert("interfaceMode".into(), json!(0));
        runtime.public_preferences.accept(None);
        assert!(super::super::home_boot_pending(&app));
        match gate {
            "preferences" => app.ott.as_mut().unwrap().public_preferences = Default::default(),
            "mode" => app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Modern),
            "uncertain" => app.ott.as_mut().unwrap().persistence_uncertain = true,
            "suspended" => app.pages.nav.suspended = true,
            "policy" => {
                let origin = json!({"provider":"m3u","profile":"Home","subprofile":null,"sourceAlias":"Local"});
                app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), json!({
                    "version":1,"strict":true,"origin":origin,
                    "target":{"kind":"item","entry":{"title":"Fixture","origin":origin,
                        "locator":{"kind":"channel","channelId":"fixture-1"}}}}));
                assert!(super::super::input_policy::navigation(app.ott.as_ref().unwrap()).is_err());
            },
            _ => unreachable!(),
        }
        assert!(!super::super::home_boot_pending(&app), "{gate}");
        assert!(!super::super::reconcile_classic_navigation(&mut app), "{gate}");
        assert!(!app.pages.has_pending_navigation(), "paint cannot invent a navigation: {gate}");
    }
}

#[test]
fn considered_native_home_cannot_acquire_a_later_saved_source_cover() {
    let _lock = plx_base::testlock::serial();
    let _session = NativeSession::open();
    let mut app = authorized_home();
    app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Modern);
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.state_load = StateLoad::Idle;
    runtime.public_preferences.accept(None);
    runtime.saved.settings.insert("interfaceMode".into(), json!(1));
    assert!(!super::super::reconcile_classic_navigation(&mut app));
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved = configured();
    runtime.saved.settings.insert("interfaceMode".into(), json!(1));
    assert!(!super::super::home_boot_pending(&app), "no queued initial Root remains");
}
