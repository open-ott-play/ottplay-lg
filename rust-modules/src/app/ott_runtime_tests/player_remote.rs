//! Fullscreen channel changes reopen the real catalogue at their accepted row.
//! Resolver observations and native resources are inert; input and panel mount are real.
use super::*;
use plx_machine::machine::UiMode;

pub(super) fn forty_channels(mode: UiMode) -> App {
    let mut app = live_m3u_remote_app(mode, false);
    let runtime = app.ott.as_mut().unwrap();
    let template = runtime.catalog.entries[0].clone();
    runtime.catalog.entries = (1..=40).map(|number| {
        let mut entry = template.clone();
        entry.id = number.to_string();
        entry.provider_id = Some(number.to_string());
        entry.title = format!("Channel {number}");
        entry.media = MediaLocator::Http {
            url: format!("https://media.invalid/{number}.m3u8"),
        };
        entry
    }).collect();
    runtime.catalog_revision += 1;
    runtime.selected = Some("1".into());
    let playing = runtime.playing.as_mut().unwrap();
    playing.entry = runtime.catalog.entries[0].clone();
    playing.selection_id = "1".into();
    runtime.continuity.user_selected(runtime.generation, "1");
    browse_focus::record(runtime, "1");
    assert_eq!(commands::selected_ids(runtime),
        (1..=40).map(|number| number.to_string()).collect::<Vec<_>>());
    publish(&mut app);
    app
}

pub(super) fn open_fullscreen_catalog(app: &mut App) -> plx_machine::machine::InstanceId {
    bridge::player_mut(&mut app.pages).unwrap().hud.hide(false);
    native_edge(app, consts::SDLK_RETURN, 40, Edge::Down);
    native_edge(app, consts::SDLK_RETURN, 40, Edge::Up);
    idle(app);
    assert_catalog_card_focus(app);
    ott_surface(app).expect("fullscreen OK mounts the catalogue panel")
}

#[test]
fn browsed_category_focus_preview_and_kiosk_designation_do_not_borrow_the_playing_queue() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = forty_channels(UiMode::Classic);
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.resolver_hold_for_test = true;
        runtime.metadata_done.store(false, Ordering::Release);
        for (index, entry) in runtime.catalog.entries.iter_mut().enumerate() {
            entry.category = if index < 20 { "Playing" } else { "Browse" }.into();
        }
        runtime.playing.as_mut().unwrap().entry = runtime.catalog.entries[0].clone();
        runtime.saved.category = "Playing".into();
        runtime.catalog_revision += 1;
    }
    publish(&mut app);
    native_edge(&mut app, 0, 480, Edge::Down);
    native_edge(&mut app, 0, 480, Edge::Up);
    admit_latest_channel(&mut app, "2");
    open_fullscreen_catalog(&mut app);
    surface_action(&mut app, OttAction::ShowCategories);
    let mut runtime = app.ott.take().unwrap();
    let candidate = runtime.category_choice_candidate(Some("Browse")).unwrap();
    runtime.pending_category_choice = Some(category_state::Pending::capture(&runtime, Some("Browse")));
    runtime.saved = candidate;
    assert!(runtime.finish_category_choice(&app, true));
    assert!(runtime.ordered_ids().iter().all(|id| id.parse::<usize>().unwrap() <= 20));
    assert!(commands::selected_ids(&runtime).iter().all(|id| id.parse::<usize>().unwrap() > 20));
    let identity = route::playback_identity(&app.player.session);
    let generation = runtime.play_generation;
    let selected = runtime.selected.clone();
    let context = runtime.view().preview_context.unwrap();
    let focus = OttAction::FocusAt { context: context.clone(), id: "21".into() };
    assert_eq!(preview::resolve(&runtime, focus.clone()).unwrap(), focus);
    handle_action(&mut runtime, &mut app, focus).unwrap();
    assert_eq!(runtime.view().selected_row.as_deref(), Some("21"));
    let metadata = OttAction::PreviewAt { context: context.clone(), id: "22".into() };
    assert_eq!(preview::resolve(&runtime, metadata.clone()).unwrap(), OttAction::Preview("22".into()));
    handle_action(&mut runtime, &mut app, metadata).unwrap();
    assert_eq!(runtime.metadata_target, Some((runtime.effective_profile().unwrap().id.clone(), "22".into())));
    assert!(runtime.metadata_next.is_some(), "the existing held metadata worker remains unstarted");
    assert!(!runtime.metadata_done.load(Ordering::Acquire));
    assert_eq!(runtime.view().selected_row.as_deref(), Some("21"), "dwell metadata cannot change browse intent");
    for action in [OttAction::FocusAt { context: context.clone(), id: "2".into() },
        OttAction::PreviewAt { context, id: "2".into() }] {
        assert!(preview::resolve(&runtime, action).is_err(), "an admitted queue row outside Browse is not visible");
    }
    assert_eq!(runtime.selected, selected);
    assert_eq!(runtime.play_generation, generation);
    assert_eq!(runtime.playing.as_ref().unwrap().selection_id, "2");
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert_eq!(runtime.resolver_launches_for_test, 0);

    // Preparation is pure: neither designation nor bare arming activates a
    // source or writes settings. The displayed category owns its ordinals.
    runtime.saving = false;
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    for (query, wanted) in [("1", "21"), ("Channel 22", "22")] {
        let prepared = kiosk_source::prepare(&runtime, &app, Some(query), false).unwrap();
        let entry = runtime.catalog.entries.iter().find(|entry| entry.id == wanted).unwrap();
        let expected = selection::capture(runtime.effective_profile().unwrap(), entry).unwrap();
        assert!(matches!(prepared.stored().target(), crate::app::ott_kiosk_stored::TargetRef::Item(pin)
            if pin.identity() == expected.identity()), "designation retains the displayed item's exact source");
        assert!(kiosk_source::still_current(&runtime, &app, &prepared));
    }
    assert!(matches!(kiosk_source::prepare(&runtime, &app, Some("Channel 1"), false),
        Err(kiosk_source::SourceError::NotFound)), "a playing-list title outside Browse cannot become a pin");
    runtime.catalog.entries[0].media = MediaLocator::Http { url: "https://changed.invalid/1.m3u8".into() };
    runtime.catalog_revision += 1;
    assert!(runtime.ordered_ids().is_empty(), "the outgoing queue's receipt is now stale");
    let armed = kiosk_source::prepare(&runtime, &app, None, false).unwrap();
    assert!(matches!(armed.stored().target(), crate::app::ott_kiosk_stored::TargetRef::Armed));
    assert!(kiosk_source::still_current(&runtime, &app, &armed));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert_eq!(runtime.metadata_target.as_ref().map(|(_, id)| id.as_str()), Some("22"));
    assert!(!runtime.metadata_done.load(Ordering::Acquire));
    assert_eq!(runtime.resolver_launches_for_test, 0);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert_eq!(route::playback_identity(&app.player.session), identity);
}

pub(super) fn admit_latest_channel(app: &mut App, wanted: &str) {
    let queued = app.ott.as_mut().unwrap().resolver_next.take()
        .expect("physical channel input queued its ordinary resolver");
    assert_eq!(queued.entry.as_ref().unwrap().id, wanted);
    let mut info = unsupported_info();
    info.video_codec = "h264".into();
    info.audio_codec = "aac".into();
    info.audio_channels = 2;
    info.audio_sample_rate = 48_000;
    info.audio_bit_depth = 16;
    info.duration_ns = 0;
    // A resolved .m3u8 is an HLS source. Passing it as ordinary HTTP is
    // correctly rejected by native route admission before resource preparation.
    let source = ReadySource::External(plx_media::ff::ExternalSource::Hls(
        format!("https://media.invalid/{wanted}.m3u8")), info, None);
    app.ott.as_ref().unwrap().tx.send(Job::Play(queued.generation, queued.profile,
        Ok((queued.entry.unwrap(), source))))
        .unwrap_or_else(|_| panic!("fixture play receiver disappeared"));
    let mut resources = CatalogAcceptedResources::default();
    let clock = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    tick_with_start(app, clock, |runtime, app, profile, entry, source| {
        runtime.start_resolved_with_kiosk_using(app, profile, entry, source,
            |runtime, app, profile, entry, source|
                start_resolved_with(runtime, app, profile, entry, source, &mut resources))
    });
    assert_eq!(resources.starts, 1, "native admission: {}", app.ott.as_ref().unwrap().status);
    let runtime = app.ott.as_mut().unwrap();
    assert_eq!(runtime.playing.as_ref().unwrap().selection_id, wanted);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    runtime.resolver_busy = true;
    // Resource acceptance does not run a decoder. Supply the existing inert
    // engine observation before traversing the real fullscreen catalogue door.
    route::reset_player_control_for_test(&app.player.session);
    app.adapters.player.install(player::engine::engine_after_reload_for_test());
    player::swap_state_for_test(player::PlaybackState::Playing);
    player::SHARED.duration_ns.store(0, Ordering::Release);
    player::SHARED.start_queued.store(false, Ordering::Release);
    player::SHARED.seen_frame.store(true, Ordering::Release);
    idle(app);
}

fn step_to_thirty_three(app: &mut App, code: u32, presses: usize) {
    for _ in 0..presses {
        native_edge(app, 0, code, Edge::Down);
        native_edge(app, 0, code, Edge::Up);
    }
    assert_eq!(queued(app), Some("33"));
    admit_latest_channel(app, "33");
}

#[test]
fn fullscreen_channel_steps_reopen_a_fresh_catalogue_at_thirty_three() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [UiMode::Classic, UiMode::Modern] {
        for (code, presses) in [(480, 32), (481, 8)] {
            let mut app = forty_channels(mode);
            let first_panel = open_fullscreen_catalog(&mut app);
            key(&mut app, Key::Back);
            assert!(!app.pages.surface_up());
            step_to_thirty_three(&mut app, code, presses);
            let identity = route::playback_identity(&app.player.session);
            let generation = app.ott.as_ref().unwrap().play_generation;
            let panel = open_fullscreen_catalog(&mut app);
            assert_ne!(panel, first_panel, "Present mounts a fresh panel rather than restoring old row memory");
            assert_eq!(app.ott.as_ref().unwrap().view().selected_row.as_deref(), Some("33"));
            // Exercise actual focused-row activation, which also verifies the
            // screen's private form selection rather than only the projection.
            key(&mut app, Key::Ok);
            let runtime = app.ott.as_ref().unwrap();
            if mode == UiMode::Classic {
                assert!(ott_surface(&app).is_none(), "current-row OK returns to the retained Player");
                assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
                assert_eq!(runtime.play_generation, generation);
            } else {
                assert_eq!(queued(&app), Some("33"), "Modern activates the same focused media identity");
            }
            assert_eq!(route::playback_identity(&app.player.session), identity);
        }
    }
}

#[test]
fn browsing_after_channel_change_survives_publication_without_following_playback() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut app = forty_channels(mode);
        step_to_thirty_three(&mut app, 480, 32);
        open_fullscreen_catalog(&mut app);
        // Classic is a vertical channel list; Modern presents this category
        // as one horizontal shelf. Both inputs select its next media row.
        key(&mut app, if mode == UiMode::Classic { Key::Down } else { Key::Right });
        let generation = app.ott.as_ref().unwrap().play_generation;
        let identity = route::playback_identity(&app.player.session);
        let mut runtime = app.ott.take().unwrap();
        assert_eq!(runtime.view().selected_row.as_deref(), Some("34"));
        assert_eq!(runtime.playing.as_ref().unwrap().selection_id, "33");
        runtime.catalog.entries[32].description = "Updated programme".into();
        runtime.catalog_revision += 1;
        runtime.changed();
        runtime.publish(&mut app);
        app.ott = Some(runtime);
        idle(&mut app);
        assert_eq!(app.ott.as_ref().unwrap().play_generation, generation);
        assert!(queued(&app).is_none(), "browsing and metadata publication cannot start playback");
        key(&mut app, Key::Ok);
        assert_eq!(queued(&app), Some("34"), "real form focus preserves the newer browse choice");
        assert_eq!(route::playback_identity(&app.player.session), identity);
    }
}
