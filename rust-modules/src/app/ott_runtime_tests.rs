//! Whole-app regressions at the native player request / OTT ownership boundary.
//! No boot loader, protected state, network, renderer or decoder is started.
#![cfg(feature = "hostsim")]

#[path = "ott_runtime_tests/notifications.rs"]
mod notifications;
#[path = "ott_runtime_tests/modern_shelves.rs"]
mod modern_shelves;

#[path = "ott_runtime_tests/favorites_items_capability.rs"]
mod favorites_items_capability;

#[path = "ott_runtime_tests/favorites_items.rs"]
mod favorites_items;

#[path = "ott_runtime_tests/managed_plex.rs"]
mod managed_plex;

use super::*;
use crate::app::{bridge, content, playback, run};
use plx_machine::machine::{Fx, MachineId, Tick};
use plx_screens::registry::{AppArg, AppFx, PlayerReq};
use std::time::Duration;

/// Callers hold the process-wide test lock before constructing an App.
fn runtime_fixture() -> Runtime {
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
    runtime.status.clear();
    runtime
}

fn app_fixture() -> App {
    let mut app = run::lifecycle_regression_tests::app();
    app.ott = Some(runtime_fixture());
    // Sample time at the real application input boundary before an Exit can
    // checkpoint; the disabled remote and empty provider prevent IO.
    runtime_tick(&mut app);
    assert!(super::playback_allowed(&app));
    app
}

/// An explicitly ready application: consume a successful, empty protected-state
/// result through the real handler. No storage worker, source or decoder starts.
/// Keep app_fixture() unloaded for startup and failure-admission regressions.
fn ready_app_fixture() -> App {
    let mut app = app_fixture();
    app.ott.as_ref().unwrap().tx.send(Job::Loaded(Ok((Saved::default(), [1; 16]))))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.loaded);
    assert_eq!(runtime.state_load, StateLoad::Idle);
    assert!(super::input_policy::navigation(runtime).is_ok(), "empty verified state has no kiosk");
    assert!(runtime.saved.profiles.profiles.is_empty());
    assert!(runtime.resolver_next.is_none());
    assert!(!runtime.catalog_loading);
    assert!(!runtime.plex_refresh_pending);
    app
}

fn runtime_tick(app: &mut App) {
    let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt())
        .expect("host fixture clock");
    super::tick(app, sample);
}

fn explicit_play(app: &mut App, id: &str) {
    let mut runtime=app.ott.take().unwrap();
    runtime.play(app,id).unwrap();
    app.ott=Some(runtime);
}

fn next_tick(app: &mut App) -> Tick {
    app.loop_t = app.loop_t.wrapping_add(16);
    Tick {
        ms: app.loop_t,
        dt_us: 16_000,
    }
}

fn settle_pages(app: &mut App) {
    for _ in 0..24 {
        let tick = next_tick(app);
        bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    }
}

fn mount_player(app: &mut App) {
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(app);
    playback::enter_player(
        &mut app.pages,
        &mut app.bridge,
        playback::Origin::Here,
        None,
    );
    settle_pages(app);
}

/// The same semantic effect a bare-picture Back or Stop key emits. This crosses
/// the real dispatcher/Bridge queue and content performer; it does not call an
/// OTT cancellation helper, so an absent ownership hook stays observable.
fn exit_through_content(app: &mut App) {
    app.pages
        .emit(MachineId::Nav, Fx::App(AppFx::Player(PlayerReq::Exit)));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    let frame = run::Frame {
        ctrl: plx_appkit::player_hud::slot(&app.player.session, app.bridge.metadata_view()),
        now: tick.ms,
        dt: 0.016,
        underlay_moving: false,
        player: true,
        present: false,
        deferred: false,
        rn: "player",
    };
    content::content_requests(app, &frame);
    settle_pages(app);
}

fn synthetic_profile() -> ProviderProfile {
    ProviderProfile {
        id: "portal-fixture".into(),
        name: "Fixture".into(),
        kind: ProviderKind::VPortal,
        source: "portal::[key:fixture]https://media.invalid/nas/api".into(),
        source_alias: String::new(),
        enabled: true,
    }
}

fn synthetic_entry() -> CatalogEntry {
    CatalogEntry { protection: None,
        synopsis: None,
        id: "late-fixture".into(),
        title: "MUST NOT BE INSTALLED AFTER EXIT".into(),
        category: String::new(),
        description: String::new(),
        poster: String::new(),
        media: MediaLocator::VPortal {
            request: serde_json::json!({"id":1}),
        },
        duration_ms: Some(10_000),
        provider_id: Some("1".into()),
        provider_id_field: Some("id".into()),
        live: None,
    }
}

fn unsupported_info() -> plx_media::ff::inspect::MediaInfo {
    plx_media::ff::inspect::MediaInfo {
        tracks: Default::default(),
        resolved_source: None,
        hls_playback: None,
        // A regression may reach request_external_source, but cannot start IO.
        video_codec: "unsupported-fixture-codec".into(),
        audio_codec: String::new(),
        width: 1280,
        height: 720,
        fps: 24.0,
        duration_ns: 10_000_000_000,
        audio_channels: 0,
        audio_sample_rate: 0,
        audio_bit_depth: 0,
        dovi: Default::default(),
    }
}

#[test]
fn explicit_player_exit_releases_an_active_nas_lease() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready_app_fixture();
    mount_player(&mut app);
    let (lease, release, _finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    app.ott.as_mut().unwrap().active_lease = Some(lease.into());

    exit_through_content(&mut app);
    runtime_tick(&mut app);

    assert!(
        bridge::player(&app.pages).is_none(),
        "Exit leaves the native player, not a mini-player transition"
    );
    assert!(
        !plx_media::route::has_url(&app.player.session),
        "Exit cancels the native route"
    );
    assert_eq!(
        release.recv_timeout(Duration::from_millis(100)),
        Ok(()),
        "explicit native Exit must signal OTT's managed server session to stop"
    );
    // Lease::run_with separately pins exactly one HTTP stop after the first
    // release signal. A destructor may enqueue another signal after it is gone.
}

#[test]
fn delayed_ready_after_player_exit_cannot_reinstall_media() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready_app_fixture();
    mount_player(&mut app);
    let entry = synthetic_entry();
    let (generation, cancel, tx) = {
        let runtime = app.ott.as_mut().unwrap();
        runtime.play_generation = 7;
        runtime.resolver_busy = true;
        runtime.play_target = Some(PlaybackRequest::ordinary(&entry.id, 0));
        (
            runtime.play_generation,
            runtime.play_cancel.clone(),
            runtime.tx.clone(),
        )
    };

    exit_through_content(&mut app);
    // Simulate the response that had already completed just before cancellation,
    // but had not yet been drained by the main thread. No resolver is spawned.
    let (lease, release, _finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    tx.send(Job::Play(
        generation,
        synthetic_profile(),
        Ok((
            entry,
            ReadySource::External(
                plx_media::ff::ExternalSource::Http("https://media.invalid/video".into()),
                unsupported_info(),
                Some(lease),
            ),
        )),
    ))
    .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);

    assert!(
        app.bridge
            .metadata_view()
            .now_playing()
            .is_none_or(|info| info.title != "MUST NOT BE INSTALLED AFTER EXIT"),
        "a stale Ready result reached start_resolved"
    );
    assert!(
        cancel.load(Ordering::Acquire),
        "Exit must cancel the OTT resolver, not only the native route"
    );
    assert!(app.ott.as_ref().unwrap().playing.is_none());
    assert!(bridge::player(&app.pages).is_none());
    assert_eq!(
        release.recv_timeout(Duration::from_millis(100)),
        Ok(()),
        "stale result still needs lease cleanup"
    );
}

#[path = "ott_runtime_tests/plex_failure.rs"]
mod plex_failure;
#[path = "ott_runtime_tests/resume.rs"]
mod resume;
#[path = "ott_runtime_tests/shutdown.rs"]
mod shutdown;
#[path = "ott_runtime_tests/navigation.rs"]
mod navigation;
#[path = "ott_runtime_tests/remote_address.rs"]
mod remote_address;
#[path = "ott_runtime_tests/structured_setup.rs"]
mod structured_setup;
#[path = "ott_runtime_tests/profile_setup.rs"]
mod profile_setup;
#[path = "ott_runtime_tests/erasure.rs"]
mod erasure;

#[test]
fn native_cancel_invalidates_a_pending_ott_ready_without_an_ott_exit() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready_app_fixture();
    let profile = synthetic_profile();
    let entry = synthetic_entry();
    let (generation, tx) = {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.profiles.profiles = vec![profile.clone()];
        runtime.saved.profiles.active_profile = Some(profile.id.clone());
        runtime.catalog.entries = vec![entry.clone()];
        runtime.resolver_busy = true;
        explicit_play(&mut app, &entry.id);
        let runtime = app.ott.as_mut().unwrap();
        // The fixture consumes the queued request as the worker would, but
        // supplies its result below instead of opening a provider connection.
        let requested = runtime
            .resolver_next
            .take()
            .expect("play queued the source request");
        (requested.generation, runtime.tx.clone())
    };
    plx_media::route::cancel_play(&mut app.player.session);
    assert_eq!(
        app.ott.as_ref().unwrap().play_generation,
        generation,
        "premise: native cancellation did not change the OTT generation"
    );
    let (lease, release, _finished) = crate::app::ott_vportal_lease::Lease::pending_for_test();
    tx.send(Job::Play(
        generation,
        profile,
        Ok((
            entry,
            ReadySource::External(
                plx_media::ff::ExternalSource::Http("https://media.invalid/video".into()),
                unsupported_info(),
                Some(lease),
            ),
        )),
    ))
    .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert!(
        app.bridge
            .metadata_view()
            .now_playing()
            .is_none_or(|info| info.title != "MUST NOT BE INSTALLED AFTER EXIT"),
        "native cancellation must reject the late OTT source before metadata installation"
    );
    assert!(app.ott.as_ref().unwrap().playing.is_none());
    assert_eq!(
        release.recv_timeout(Duration::from_millis(100)),
        Ok(()),
        "rejected native-generation result must release its session"
    );
}

#[path = "ott_runtime_tests/editor_ack.rs"]
mod editor_ack;

#[path = "ott_runtime_tests/state_load.rs"]
mod state_load;
#[path = "ott_runtime_tests/navigation_context.rs"]
mod navigation_context;

#[path = "ott_runtime_tests/plex_setup.rs"]
mod plex_setup;

#[path = "ott_runtime_tests/catalog_remote.rs"]
mod catalog_remote;
#[path = "ott_subtitle_runtime_tests.rs"]
mod subtitle_runtime;

#[path = "ott_runtime_tests/local_remote.rs"]
mod local_remote;
#[path = "ott_runtime_tests/local_integration.rs"]
pub(super) mod local_integration;

#[path = "ott_runtime_tests/search.rs"]
mod search_runtime;
#[path = "ott_runtime_tests/local_favorite_proof.rs"]
mod local_favorite_proof;
#[path = "ott_media_tracks_runtime_tests.rs"]
mod media_tracks_runtime;
#[path = "ott_runtime_tests/plex_queue_remote.rs"]
mod plex_queue_remote;

#[path="ott_runtime_tests/programme.rs"]
mod programme;

#[path = "ott_runtime_tests/ui_projection.rs"]
mod ui_projection;

#[path = "ott_kiosk_runtime_tests.rs"]
mod kiosk_runtime;

#[path = "ott_kiosk_source_integration_tests.rs"]
mod kiosk_source_integration;

#[path = "ott_runtime_tests/lifecycle.rs"]
mod lifecycle_runtime;

#[path = "ott_input_policy_tests.rs"]
mod input_policy;

#[path = "ott_runtime_tests/profile_capacity.rs"]
mod profile_capacity;

#[path = "ott_runtime_tests/scalar_profile_target.rs"]
mod scalar_profile_target;

#[path = "ott_runtime_tests/font_scale.rs"]
mod font_scale;
#[path = "ott_runtime_tests/volume_step.rs"]
mod volume_step;
#[path = "ott_runtime_tests/category_profile.rs"]
mod category_profile;

#[path = "ott_runtime_tests/standalone_file.rs"]
mod standalone_file;

#[path = "ott_runtime_tests/file_source_queries.rs"]
mod file_source_queries;
#[path = "ott_runtime_tests/guide.rs"]
mod guide;

#[path = "ott_runtime_tests/reminders.rs"]
mod reminders;

#[path = "ott_runtime_tests/guide_admission.rs"]
mod guide_admission;

#[path = "ott_menu_visibility_runtime_tests.rs"]
mod menu_visibility;

#[path = "ott_random_runtime_tests.rs"]
mod random;

#[path = "ott_runtime_tests/settings_wire_receipts.rs"]
mod settings_wire_receipts;

#[path = "ott_runtime_tests/successor_navigation.rs"]
mod successor_navigation;

#[path = "ott_runtime_tests/interface_mode.rs"]
mod interface_mode;

#[path = "ott_viewport_runtime_tests.rs"]
mod viewport;

#[path = "ott_geometry_runtime_tests.rs"]
mod geometry;

#[path = "ott_runtime_tests/local_scan_completion.rs"]
mod local_scan_completion;

#[path = "ott_runtime_tests/local_binding_cancel.rs"]
mod local_binding_cancel;

#[path = "ott_runtime_tests/local_error_envelope.rs"]
mod local_error_envelope;

#[cfg(target_os = "macos")]
#[path = "ott_desktop_input_tests.rs"]
mod desktop_keys;

#[path = "ott_runtime_tests/playing_favorite.rs"]
mod playing_favorite;

#[path = "ott_runtime_tests/local_folder_grant.rs"]
mod local_folder_grant;

#[path = "ott_runtime_tests/favorites_recovery.rs"]
mod favorites_recovery;

#[path = "ott_runtime_tests/continuity_diagnostics.rs"]
mod continuity_diagnostics_runtime;

#[path = "ott_runtime_tests/local_browser.rs"]
mod local_browser;

#[path = "ott_runtime_tests/vportal_categories.rs"]
mod vportal_categories;
#[path = "ott_numeric_input_tests.rs"]
mod numeric_input;

#[cfg(test)]
#[path = "ott_sleep_timer_runtime_tests.rs"]
mod sleep_timer_tests;

#[path = "ott_runtime_tests/info_timeout.rs"]
mod info_timeout_runtime;

#[path = "ott_runtime_tests/local_profile.rs"]
mod local_profile_connection;
#[path = "ott_runtime_tests/osd_settings.rs"]
mod osd_settings;

#[path = "ott_runtime_tests/osd_ui.rs"]
mod osd_ui_tests;

#[path = "ott_runtime_tests/classic_settings.rs"]
mod classic_settings;

#[path = "ott_runtime_tests/settings_schema_contract.rs"]
mod settings_schema_contract;

#[path = "ott_runtime_tests/classic_navigation.rs"]
mod classic_navigation;

#[path = "ott_runtime_tests/classic_source.rs"]
mod classic_source_overlay;

#[path = "ott_runtime_tests/classic_clock.rs"]
mod classic_clock_runtime;

#[path = "ott_runtime_tests/provider_context.rs"]
mod provider_context;

#[path = "ott_runtime_tests/catalog_diagnostics.rs"]
mod catalog_diagnostics_runtime;

#[path = "ott_runtime_tests/language_settings.rs"]
mod language_settings;

#[path = "ott_runtime_tests/preplay_diagnostics.rs"]
mod preplay_diagnostics;

#[path = "ott_runtime_tests/playback_diagnostics.rs"]
mod playback_diagnostics;

#[path = "ott_runtime_tests/catalog_publication.rs"]
mod catalog_publication;

#[path = "ott_runtime_tests/favorite_profile_rename.rs"]
mod favorite_profile_rename;

#[path = "ott_runtime_tests/classic_play_handoff.rs"]
mod classic_play_handoff_tests;
