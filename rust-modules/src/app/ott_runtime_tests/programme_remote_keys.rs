//! Actual SDL-byte ingress, shared Player dispatch, and the ordinary content runner.
//! The inherited engine is inert; these tests qualify commands and ownership, not a TV.
use super::*;
use plx_machine::machine::Edge;
use plx_ui::consts;

fn remote_app() -> App {
    let mut app = catalog_app();
    // Hold the existing writer and resolver. A step can be admitted without any
    // protected-store write, source fetch, or decoder worker in this fixture.
    app.ott.as_mut().unwrap().saving = true;
    bridge::player_mut(&mut app.pages).unwrap().hud.hide(false);
    app.inputs.clear();
    app
}

fn dispatch_bytes(app: &mut App, bytes: [u8; 128]) -> run::Frame {
    dispatch_batch(app, &[bytes])
}

fn dispatch_batch(app: &mut App, batch: &[[u8; 128]]) -> run::Frame {
    dispatch_batch_observed(app, batch, |_, _| {})
}

fn dispatch_batch_observed(
    app: &mut App, batch: &[[u8; 128]], mut before: impl FnMut(&App, &[u8; 128]),
) -> run::Frame {
    let tick = next_tick(app);
    let ctrl = player_hud::slot(&app.player.session, app.bridge.metadata_view());
    let mut fr = run::Frame { ctrl, now: tick.ms, dt: 0.016, underlay_moving: false,
        player: matches!(app.route(), AppArg::Player), present: false, deferred: false, rn: "player" };
    if let Some(screen) = bridge::player_mut(&mut app.pages) { screen.slot = ctrl; }
    for bytes in batch {
        before(app, bytes);
        app.ev = *bytes;
        unsafe {
            run::ingest_sdl_event_with_window(app, &mut fr, |_| panic!("keyboard must not restore a window"));
        }
    }
    // This is the actual pre-dispatch production hook in RED and GREEN alike.
    // The only baseline carrier is pub(super) visibility, not a fake publication.
    run::publish_playback_frame(app, fr.player);
    let input = std::mem::take(&mut app.inputs);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, input);
    fr
}

fn finish_input(app: &mut App, fr: &run::Frame) {
    content::content_requests(app, fr);
    runtime_tick(app);
    app.ev = [0; 128];
}

fn native_edge(app: &mut App, sym: u32, code: u32, edge: Edge) {
    let bytes = if edge == Edge::Repeat {
        crate::app::events::encode_key_repeat(sym, code)
    } else {
        crate::app::events::encode_key(sym, code, edge == Edge::Down)
    };
    let fr = dispatch_bytes(app, bytes);
    finish_input(app, &fr);
}

fn queued(app: &App) -> Option<&str> {
    app.ott.as_ref().unwrap().play_target.as_ref().map(|request| request.id.as_str())
}

#[test]
fn native_channel_rocker_steps_from_playing_item_not_browse_focus() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for (raw, wanted) in [(300, "a"), (301, "b")] {
        let mut app = remote_app();
        app.ott.as_mut().unwrap().selected = Some("b".into());
        let identity = route::playback_identity(&app.player.session);
        let owner = app.pages.top_page();
        native_edge(&mut app, 0, raw, Edge::Down);
        assert_eq!(queued(&app), Some(wanted), "rocker direction is playback order, not pager sign");
        assert!(!app.ott.as_ref().unwrap().play_target.as_ref().unwrap().advance_on_failure);
        assert_eq!(route::playback_identity(&app.player.session), identity,
            "admission alone must not pretend the next decoder is already running");
        assert_eq!(app.pages.top_page(), owner);
        assert!(app.adapters.player.is_live());
        assert!(app.player.pending_exit.is_none());
        native_edge(&mut app, 0, raw, Edge::Repeat);
        native_edge(&mut app, 0, raw, Edge::Up);
        assert_eq!(queued(&app), Some(wanted), "pending admission/release cannot queue another item");
    }
}

fn live_m3u_remote_app(mode: plx_machine::machine::UiMode, visible_hud: bool) -> App {
    let mut app = continuity_catalog_app(mode);
    let runtime = app.ott.as_mut().unwrap();
    let profile = &mut runtime.saved.profiles.profiles[0];
    profile.kind = ProviderKind::M3u;
    profile.source = "https://media.invalid/live.m3u".into();
    let profile = profile.clone();
    runtime.provider = ProviderKind::M3u;
    for row in &mut runtime.catalog.entries {
        row.media = MediaLocator::Http { url: format!("https://media.invalid/{}.m3u8", row.id) };
        row.duration_ms = None;
    }
    let playing = runtime.playing.as_mut().unwrap();
    playing.profile = profile;
    playing.entry = runtime.catalog.entries[0].clone();
    runtime.selected = Some("b".into()); // Browse focus is not the playing channel.
    runtime.saving = true;
    runtime.resolver_busy = true;
    player::SHARED.duration_ns.store(0, Ordering::Release);
    player::TX.commit_paused(false);
    player::swap_state_for_test(player::PlaybackState::Playing);
    publish(&mut app);
    assert!(plx_appkit::info_panel::live_channel(app.bridge.metadata_view()));
    let now = app.loop_t;
    let hud = &mut bridge::player_mut(&mut app.pages).unwrap().hud;
    if visible_hud {
        hud.note_fresh_press(&app.player.session, now, false);
        hud.extend(now, 60_000);
    } else {
        hud.hide(false);
    }
    assert_eq!(hud.visible(&app.player.session, now, false), visible_hud);
    assert!(!app.pages.surface_up());
    app.inputs.clear();
    app
}

fn live_m3u_rocker_case(visible_hud: bool) {
    use plx_machine::machine::UiMode;
    for mode in [UiMode::Classic, UiMode::Modern] {
        for (raw, wanted) in [(300, "a"), (301, "b")] {
            let mut app = live_m3u_remote_app(mode, visible_hud);
            let identity = route::playback_identity(&app.player.session);
            let owner = app.pages.top_page();
            let seek = player::seek_pending();
            let fr = dispatch_bytes(&mut app, crate::app::events::encode_key(0, raw, true));
            let offer = app.bridge.ott_player_offer().expect("live M3U has a fullscreen action offer");
            assert!(!offer.seek_arrows && offer.guide, "live channel arrows must not borrow VOD seek authority");
            finish_input(&mut app, &fr);
            let runtime = app.ott.as_ref().unwrap();
            assert_eq!(queued(&app), Some(wanted), "{mode:?}, visible HUD={visible_hud}, CH code={raw}");
            assert_eq!(runtime.resolver_next.as_ref().unwrap().entry.as_ref().unwrap().id, wanted);
            assert!(!runtime.play_target.as_ref().unwrap().advance_on_failure);
            assert_eq!(player::seek_pending(), seek, "channel selection must not enqueue a seek");
            assert_eq!(player::duration_ns(), 0, "a finite timeline is not required");
            assert_eq!(route::playback_identity(&app.player.session), identity);
            assert_eq!(app.pages.top_page(), owner);
            assert!(app.adapters.player.is_live() && app.player.pending_exit.is_none());
            let generation = runtime.play_generation;
            native_edge(&mut app, 0, raw, Edge::Repeat);
            native_edge(&mut app, 0, raw, Edge::Up);
            assert_eq!(queued(&app), Some(wanted));
            assert_eq!(app.ott.as_ref().unwrap().play_generation, generation,
                "repeat/release cannot replace an opening channel");
        }
    }
}

#[test]
fn native_m3u_live_channel_rocker_with_hidden_hud_needs_no_duration() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    live_m3u_rocker_case(false);
}

#[test]
fn native_m3u_live_channel_rocker_with_visible_hud_needs_no_duration() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    live_m3u_rocker_case(true);
}

fn fail_live_m3u(app: &mut App) {
    let mut runtime = app.ott.take().unwrap();
    let identity = runtime.playing.as_ref().unwrap().identity.clone();
    runtime.playback_failure.admitted(runtime.play_generation, identity);
    runtime.playing.as_mut().unwrap().advance_on_failure = false;
    player::swap_state_for_test(player::PlaybackState::Error);
    runtime.reconcile_failure(app);
    assert!(runtime.playing.is_none() && !app.adapters.player.is_live());
    assert!(!bridge::player(&app.pages).unwrap().scrub.is_live(), "terminal cleanup retires stale gesture ownership");
    runtime.reconcile_owner(app);
    assert!(failed_playback::admissible(&runtime, app).is_some());
    assert!(runtime.playback_failure.snapshot().is_some());
    assert_eq!(state(app, &runtime)["playback"]["state"], "error");
    app.ott = Some(runtime);
}

#[test]
fn native_m3u_failed_channel_rocker_keeps_exact_list_authority_after_cleanup() {
    use plx_machine::machine::UiMode;
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [UiMode::Classic, UiMode::Modern] {
        for (raw, wanted) in [(300, "a"), (301, "b")] {
            let mut app = live_m3u_remote_app(mode, true);
            bridge::player_mut(&mut app.pages).unwrap().scrub.reveal = true;
            fail_live_m3u(&mut app);
            let observation = json!(app.ott.as_ref().unwrap().playback_failure.snapshot());
            // EPG/art/title publication does not change the occurrence's media binding.
            app.ott.as_mut().unwrap().catalog.entries[0].description = "Updated programme".into();
            app.ott.as_mut().unwrap().catalog_revision += 1;
            let fr = dispatch_bytes(&mut app, crate::app::events::encode_key(0, raw, true));
            assert!(app.bridge.ott_player_offer().unwrap().recovery_only);
            assert_eq!(json!(app.ott.as_ref().unwrap().playback_failure.snapshot()), observation);
            finish_input(&mut app, &fr);
            assert_eq!(queued(&app), Some(wanted), "{mode:?}: failed channel CH {raw}");
            let runtime = app.ott.as_ref().unwrap();
            assert!(runtime.failed_playback.is_none() && runtime.playback_failure.snapshot().is_none());
            assert!(!runtime.play_target.as_ref().unwrap().advance_on_failure);
            assert_eq!(player::seek_pending(), -1);
        }
    }
}

#[test]
fn failed_channel_recovery_rechecks_source_view_and_exact_media_before_dispatch() {
    use plx_screens::registry::{OttPlayerAction as Action, OttPlayerRequest};
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for change in 0..7 {
        let mut app = live_m3u_remote_app(plx_machine::machine::UiMode::Modern, false);
        fail_live_m3u(&mut app);
        crate::app::ott_runtime::publish_remote_player_offer(&mut app);
        let request = OttPlayerRequest { owner: app.pages.top_page().unwrap(),
            source: app.bridge.ott_player_offer().unwrap().source.clone(), action: Action::Next };
        if change == 0 {
            for action in [Action::SeekForward, Action::TogglePause, Action::OpenMenu, Action::OpenCatalog] {
                assert!(crate::app::ott_runtime::remote_player_action(&mut app,
                    OttPlayerRequest { action, ..request.clone() }).is_err(),
                    "a retained failure grants no transport or menu authority");
            }
        }
        let runtime = app.ott.as_mut().unwrap();
        match change {
            0 => runtime.saved.profiles.profiles[0].source = "https://other.invalid/list.m3u".into(),
            1 => runtime.generation += 1,
            2 => runtime.saved.category = "Another list".into(),
            3 => runtime.playback_epoch += 1,
            4 => runtime.catalog.entries[0].media = MediaLocator::Http { url: "https://other.invalid/replaced.ts".into() },
            5 => runtime.failed_playback = None,
            _ => app.adapters.player.install(player::engine::engine_after_reload_for_test()),
        }
        assert!(crate::app::ott_runtime::remote_player_action(&mut app, request).is_err(), "change {change}");
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert_eq!(player::seek_pending(), -1);
    }
}

#[test]
fn native_m3u_number_one_uses_live_channel_owner_across_hud_and_failure() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [plx_machine::machine::UiMode::Classic, plx_machine::machine::UiMode::Modern] {
        for visible_hud in [false, true] {
            for failed in [false, true] {
                let mut app = live_m3u_remote_app(mode, visible_hud);
                if failed { fail_live_m3u(&mut app); }
                let now = app.loop_t;
                let hud = &mut bridge::player_mut(&mut app.pages).unwrap().hud;
                if visible_hud {
                    hud.note_fresh_press(&app.player.session, now, false);
                    hud.extend(now, 60_000);
                } else {
                    hud.hide(false);
                }
                assert_eq!(hud.visible(&app.player.session, now, false), visible_hud);
                app.ott.as_mut().unwrap().saving = false;
                native_edge(&mut app, u32::from(b'1'), 0, Edge::Down);
                native_edge(&mut app, u32::from(b'1'), 0, Edge::Up);
                assert!(queued(&app).is_none(), "number entry waits for the multi-digit deadline");
                let mut runtime = app.ott.take().unwrap();
                let deadline = runtime.now_ms() + 2_001;
                crate::app::ott_runtime::numeric_input::tick(&mut runtime, &mut app, deadline);
                app.ott = Some(runtime);
                assert_eq!(queued(&app), Some("c"), "{mode:?}, HUD={visible_hud}, failed={failed}: 1 selects first channel, not VOD seek");
                assert_eq!(player::seek_pending(), -1);
            }
        }
    }
}

#[test]
fn native_blue_opens_categories_and_back_keeps_the_same_paused_decoder() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = remote_app();
    let identity = route::playback_identity(&app.player.session);
    let owner = app.pages.top_page();
    let paused = lifecycle::viewer_paused();
    native_edge(&mut app, 0, 489, Edge::Down);
    idle(&mut app);
    assert!(ott_surface(&app).is_some());
    assert_eq!(app.ott.as_ref().unwrap().page, "categories");
    key(&mut app, Key::Back);
    assert_eq!(app.ott.as_ref().unwrap().page, "channels");
    assert!(ott_surface(&app).is_some());
    key(&mut app, Key::Back);
    assert!(!app.pages.surface_up());
    assert_eq!(app.pages.top_page(), owner);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(lifecycle::viewer_paused(), paused);
    assert!(app.adapters.player.is_live());
}

#[test]
fn native_paused_portal_ok_resumes_finite_transport_before_catalog() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = remote_app();
    assert_eq!(app.ott.as_ref().unwrap().playing.as_ref().unwrap().profile.kind, ProviderKind::VPortal);
    assert!(lifecycle::viewer_paused());
    let identity = route::playback_identity(&app.player.session);
    native_edge(&mut app, consts::SDLK_RETURN, 40, Edge::Down);
    idle(&mut app);
    assert!(!app.pages.surface_up(), "VPortal has no Classic catalogBrowser override");
    assert!(!lifecycle::viewer_paused(), "paused finite playback resumes before ordinary catalog navigation");
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert!(queued(&app).is_none());
}

#[test]
fn native_portal_seek_does_not_depend_on_browsed_category_membership() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = remote_app();
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.catalog.entries[1].category = "Other folder".into();
        runtime.saved = runtime.category_choice_candidate(Some("Other folder")).unwrap();
        assert_eq!(runtime.effective_category(), "Other folder");
        assert!(!commands::selected_ids(runtime).iter().any(|id| id == "c"));
    }
    let identity = route::playback_identity(&app.player.session);
    native_edge(&mut app, consts::SDLK_RIGHT, 79, Edge::Down);
    assert_eq!(player::seek_pending(), 17_000_000_000, "same owned finite item still seeks +10 seconds");
    assert!(queued(&app).is_none());
    assert!(!app.pages.surface_up());
    assert_eq!(route::playback_identity(&app.player.session), identity);
}

#[test]
fn native_visible_timeline_and_more_keep_their_current_input_owner() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = catalog_app();
    native_edge(&mut app, consts::SDLK_UP, 82, Edge::Down);
    assert_eq!(bridge::player(&app.pages).unwrap().hud.nav.focus, player_hud::geometry::GROUP_ROW);
    assert!(queued(&app).is_none());
    // Start the independent menu route from its documented scrubber seat.
    drop(app);
    let mut app = catalog_app();
    open_more(&mut app);
    let owner = input_owner(&app);
    native_edge(&mut app, 0, 300, Edge::Down);
    assert_eq!(input_owner(&app), owner);
    assert_eq!(bridge::player_overlay_kind(&app.pages), Some(OverlayKind::More { page: None }));
    assert!(queued(&app).is_none());
}

#[test]
fn native_queued_action_cannot_borrow_a_retired_playing_owner() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = remote_app();
    let identity = route::playback_identity(&app.player.session);
    let fr = dispatch_bytes(&mut app, crate::app::events::encode_key(0, 489, true));
    // The same decoder now has no OTT source owner; an already emitted action
    // must revalidate at the ordinary content runner before showing any UI.
    app.ott.as_mut().unwrap().playing = None;
    finish_input(&mut app, &fr);
    idle(&mut app);
    assert!(!app.pages.surface_up());
    assert!(queued(&app).is_none());
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert!(app.adapters.player.is_live());
}

#[test]
fn native_strict_kiosk_refuses_catalog_and_channel_changes() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = remote_app();
    let mut runtime = app.ott.take().unwrap();
    runtime.saving = false;
    runtime.entropy = Some([7; 16]);
    let prepared = kiosk_source::prepare(&runtime, &app, Some("1"), true).unwrap();
    runtime.saved = kiosk_source::save_candidate(&runtime, &prepared).unwrap();
    runtime.sync_saved_context();
    runtime.saving = true;
    assert!(crate::app::ott_runtime::input_policy::navigation(&runtime).is_err());
    app.ott = Some(runtime);
    let identity = route::playback_identity(&app.player.session);
    for raw in [300, 301, 488, 489] { native_edge(&mut app, 0, raw, Edge::Down); }
    idle(&mut app);
    assert!(!app.pages.surface_up());
    assert!(queued(&app).is_none());
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert!(lifecycle::viewer_paused());
}

#[test]
fn controlled_replay_has_no_native_catalog_or_step_authority() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = remote_app();
    let initial = crate::app::bootstrap::Initial::synthetic_home(17, 32517, None).unwrap();
    app.bridge = bridge::Bridge::controlled_home(|| 0, &initial, app.adapters.player.mt(), true);
    assert!(app.bridge.is_controlled_replay());
    let identity = route::playback_identity(&app.player.session);
    native_edge(&mut app, 0, 489, Edge::Down);
    native_edge(&mut app, 0, 300, Edge::Down);
    assert!(!app.pages.surface_up());
    assert!(queued(&app).is_none());
    assert_eq!(route::playback_identity(&app.player.session), identity);
}

#[cfg(target_os = "macos")]
#[test]
fn physical_mac_arrow_keeps_desktop_volume_instead_of_native_portal_seek() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = remote_app();
    player::volume::set_volume(&mut app.adapters.player, &mut app.player.volume,
        player::volume::DesiredVolume::new(50, false).unwrap());
    let mut bytes = crate::app::events::encode_key(consts::SDLK_RIGHT, 79, true);
    // Actual host SDL window and modifier layout, not the remote injection marker.
    bytes[8..12].copy_from_slice(&1_u32.to_ne_bytes());
    bytes[24..26].copy_from_slice(&0_u16.to_ne_bytes());
    let before_seek = player::seek_pending();
    let fr = dispatch_bytes(&mut app, bytes);
    finish_input(&mut app, &fr);
    assert_eq!(app.player.volume.desired().level(), 55);
    assert_eq!(player::seek_pending(), before_seek);
    assert!(queued(&app).is_none());
    assert!(!app.pages.surface_up());
}

#[cfg(target_os = "macos")]
#[test]
fn queued_native_and_physical_keys_keep_each_events_provenance() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for native_first in [true, false] {
        let mut app = remote_app();
        player::volume::set_volume(&mut app.adapters.player, &mut app.player.volume,
            player::volume::DesiredVolume::new(50, false).unwrap());
        let native = crate::app::events::encode_key(0, 489, true);
        let mut physical = crate::app::events::encode_key(consts::SDLK_RIGHT, 79, true);
        physical[8..12].copy_from_slice(&1_u32.to_ne_bytes());
        physical[24..26].copy_from_slice(&0_u16.to_ne_bytes());
        let batch = if native_first { [native, physical] } else { [physical, native] };
        let before_seek = player::seek_pending();
        let mut physical_saw_hud = None;
        let fr = dispatch_batch_observed(&mut app, &batch, |app, bytes| {
            if bytes == &physical {
                let page = bridge::player(&app.pages).unwrap();
                physical_saw_hud = Some(page.hud.visible(
                    &app.player.session, clock::now(), lifecycle::viewer_paused()));
                assert!(!app.pages.surface_up() && !page.scrub.is_live());
            }
        });
        finish_input(&mut app, &fr);
        idle(&mut app);
        // Desktop ingress owns plain Right only while the actual HUD is hidden.
        // A prior native key may change that context; it must never grant this
        // physical key the television's immediate-seek/step authority.
        let expected_volume = if physical_saw_hud.unwrap() { 50 } else { 55 };
        assert_eq!(app.player.volume.desired().level(), expected_volume);
        assert_eq!(player::seek_pending(), before_seek);
        assert!(ott_surface(&app).is_some(), "native Blue cannot borrow the final event's namespace");
        assert_eq!(app.ott.as_ref().unwrap().page, "categories");
        assert!(queued(&app).is_none());
    }
}

#[test]
fn queued_native_presses_keep_the_first_events_bare_hud_state() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = remote_app();
    let first = crate::app::events::encode_key(consts::SDLK_UP, 82, true);
    let second = crate::app::events::encode_key(consts::SDLK_RIGHT, 79, true);
    let fr = dispatch_batch(&mut app, &[first, second]);
    finish_input(&mut app, &fr);
    assert_eq!(queued(&app), Some("a"),
        "the first bare Up must not borrow the later press's visible-HUD sample");
    assert!(app.adapters.player.is_live());
    assert!(app.player.pending_exit.is_none());
}

#[cfg(target_os = "macos")]
#[test]
fn queued_physical_back_cannot_replace_a_prior_native_press_hud_sample() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = remote_app();
    let identity = route::playback_identity(&app.player.session);
    let first = crate::app::events::encode_key(consts::SDLK_UP, 82, true);
    let mut second = crate::app::events::encode_key(consts::SDLK_ESCAPE, 41, true);
    second[8..12].copy_from_slice(&1_u32.to_ne_bytes());
    second[24..26].copy_from_slice(&0_u16.to_ne_bytes());
    let fr = dispatch_batch(&mut app, &[first, second]);
    finish_input(&mut app, &fr);
    assert_eq!(queued(&app), Some("a"),
        "a later physical Back cannot turn the earlier native Up into visible-HUD navigation");
    assert!(app.player.pending_exit.is_none(), "Back sees the HUD raised by the preceding press");
    assert!(app.adapters.player.is_live());
    assert_eq!(route::playback_identity(&app.player.session), identity);
}

#[cfg(target_os = "macos")]
#[test]
fn queued_physical_command_arrow_never_borrows_native_seek_authority() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for native_first in [true, false] {
        let mut app = remote_app();
        player::volume::set_volume(&mut app.adapters.player, &mut app.player.volume,
            player::volume::DesiredVolume::new(50, false).unwrap());
        let native = crate::app::events::encode_key(0, 489, true);
        let mut physical = crate::app::events::encode_key(consts::SDLK_RIGHT, 79, true);
        physical[8..12].copy_from_slice(&1_u32.to_ne_bytes());
        // Command alone is an admitted desktop seek on bare video. Add Shift so
        // this physical chord is genuinely queued, while keeping the HUD hidden:
        // a mistaken native-remote attribution must still expose an illegal seek.
        physical[24..26].copy_from_slice(&0x0401_u16.to_ne_bytes());
        let batch = if native_first { [native, physical] } else { [physical, native] };
        let before_seek = player::seek_pending();
        let fr = dispatch_batch_observed(&mut app, &batch, |app, bytes| {
            if !native_first && bytes == &native {
                assert!(app.inputs.iter().any(|event|
                    event.source == plx_machine::machine::Source::Sdl
                        && matches!(event.kind, plx_machine::machine::InputKind::Key {
                            sym: consts::SDLK_RIGHT, .. })),
                    "physical Command+Shift+Right must really be queued for the shared Player");
            }
        });
        finish_input(&mut app, &fr);
        idle(&mut app);
        assert_eq!(app.player.volume.desired().level(), 50,
            "Command+Right is not the desktop volume shortcut");
        assert_eq!(player::seek_pending(), before_seek,
            "the physical arrow stays a generic HUD input, never an OTT seek");
        assert!(ott_surface(&app).is_some(), "native Blue still owns its category action");
        assert_eq!(app.ott.as_ref().unwrap().page, "categories");
        assert!(queued(&app).is_none());
    }
}

#[test]
fn native_first_repeat_marked_key_uses_fresh_press_sampling() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = catalog_app();
    app.ott.as_mut().unwrap().saving = true;
    // Leave a real visible-HUD sample, then end that key. The following raw
    // driver-repeat is a new gesture according to ingress's existing rule.
    native_edge(&mut app, consts::SDLK_UP, 82, Edge::Down);
    native_edge(&mut app, consts::SDLK_UP, 82, Edge::Up);
    assert!(bridge::player(&app.pages).unwrap().hud.visible_at_press);
    assert_ne!(app.down_sym, consts::SDLK_UP);
    bridge::player_mut(&mut app.pages).unwrap().hud.hide(false);
    native_edge(&mut app, consts::SDLK_UP, 82, Edge::Repeat);
    assert_eq!(queued(&app), Some("a"),
        "a driver repeat without a held matching key must sample the new hidden HUD as a fresh press");
    // A subsequent matching repeat is genuinely held; it cannot create a
    // second pending step or retire the admitted source.
    native_edge(&mut app, consts::SDLK_UP, 82, Edge::Repeat);
    assert_eq!(queued(&app), Some("a"));
    assert!(app.adapters.player.is_live());
    assert!(app.player.pending_exit.is_none());
}

#[path = "programme_menu_input.rs"]
mod menu_input;

#[path = "programme_menu_pointer.rs"]
mod menu_pointer;

#[test]
fn notifications_overlay_preserves_physical_channel_dispatch_and_normal_selection() {
    let _serial=plx_base::testlock::serial();
    let _globals=Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for visible in [false,true] {
        let mut app=remote_app();
        let runtime=app.ott.as_mut().unwrap(); runtime.selected=Some("b".into());
        if visible {let now=runtime.now_ms();runtime.remote.show_notification_for_test(now);
            assert!(runtime.remote.notifications_need_frame(now));}
        let owner=app.pages.top_page();
        let identity=route::playback_identity(&app.player.session);
        native_edge(&mut app,0,300,Edge::Down);
        assert_eq!(queued(&app),Some("a"));
        assert_eq!(app.ott.as_ref().unwrap().selected.as_deref(),Some("a"),
            "banner must preserve the ordinary channel-step selection");
        assert_eq!(app.pages.top_page(),owner);
        assert_eq!(route::playback_identity(&app.player.session),identity);
    }
}
