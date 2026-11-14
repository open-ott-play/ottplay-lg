//! Admission only: no resolver, compositor, native decoder or network is started.
use super::*;

fn setup() -> (App, ProviderProfile, CatalogEntry) {
    let mut app = ready_app_fixture();
    app.running = true;
    let profile = ProviderProfile {
        id: "mv-source".into(),
        name: "Fixture".into(),
        kind: ProviderKind::M3u,
        source: "https://example.invalid/list.m3u".into(),
        source_alias: "fixture".into(),
        enabled: true,
    };
    let mut entry = synthetic_entry();
    entry.id = "mv-channel".into();
    entry.title = "Fixture".into();
    entry.category = "Documentary".into();
    entry.media = MediaLocator::Http {
        url: "https://example.invalid/stream.m3u8".into(),
    };
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.provider = profile.kind;
    runtime.catalog.entries = vec![entry.clone()];
    runtime.catalog_complete = true;
    runtime.resume_source_work();
    (app, profile, entry)
}

#[test]
fn allowed_source_does_not_change_saved_selection_or_queue_playback() {
    let _serial = plx_base::testlock::serial();
    let (mut app, profile, entry) = setup();
    let mut runtime = app.ott.take().unwrap();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let selected = runtime.selected.clone();
    let generation = runtime.play_generation;
    let request = PlaybackRequest::ordinary(&entry.id, 0);
    assert!(runtime
        .authorize_multiview_source(&app, &request, &profile, Some(&entry), None, Some(&entry))
        .is_ok());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert_eq!(runtime.selected, selected);
    assert_eq!(runtime.play_generation, generation);
    assert!(runtime.play_target.is_none());
    assert!(runtime.resolver_next.is_none());
    app.ott = Some(runtime);
}

#[test]
fn stopped_foreground_and_malformed_kiosk_fail_before_any_media_effect() {
    let _serial = plx_base::testlock::serial();
    let (mut app, profile, entry) = setup();
    let mut runtime = app.ott.take().unwrap();
    let request = PlaybackRequest::ordinary(&entry.id, 0);
    runtime.saved.settings.insert(
        crate::app::ott_kiosk_stored::KEY.into(),
        json!({"invalid":true}),
    );
    assert!(runtime
        .authorize_multiview_source(&app, &request, &profile, Some(&entry), None, Some(&entry))
        .is_err());
    runtime
        .saved
        .settings
        .remove(crate::app::ott_kiosk_stored::KEY);
    app.running = false;
    assert!(runtime
        .authorize_multiview_source(&app, &request, &profile, Some(&entry), None, Some(&entry))
        .is_err());
    assert!(runtime.resolver_next.is_none());
    assert!(runtime.play_target.is_none());
    app.ott = Some(runtime);
}

#[test]
fn resolved_source_classification_is_checked_again_before_export() {
    let _serial = plx_base::testlock::serial();
    let (mut app, profile, entry) = setup();
    let mut runtime = app.ott.take().unwrap();
    let request = PlaybackRequest::ordinary(&entry.id, 0);
    let mut resolved = entry.clone();
    resolved.category = "Catalog / 18+".into();
    assert!(runtime
        .authorize_multiview_source(
            &app,
            &request,
            &profile,
            Some(&entry),
            None,
            Some(&resolved)
        )
        .is_err());
    assert!(runtime.resolver_next.is_none());
    assert!(runtime.play_target.is_none());
    app.ott = Some(runtime);
}

#[test]
fn renderer_offer_without_an_admitted_picture_cannot_control_a_scene() {
    let _serial = plx_base::testlock::serial();
    let (mut app, _, _) = setup();
    let layout = plx_appkit::multiview_layout::Layout::new(
        plx_ui::Rect::FULL,
        plx_appkit::multiview_layout::Mode::Grid4,
        1.0,
    )
    .unwrap();
    let frame = plx_appkit::multiview_chrome::ChromeFrame::new(
        layout,
        &[],
        plx_appkit::multiview_chrome::ViewState {
            revision: 71,
            active: Some(0),
            input_owned: true,
            ..Default::default()
        },
    )
    .unwrap();
    app.bridge.publish_multiview_offer(Some(frame));
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    super::super::multiview_player_action(
        &mut app,
        plx_appkit::multiview_chrome::Event::ActivateSlot {
            revision: 71,
            slot: 1,
        },
    );
    assert!(app.bridge.multiview_offer().is_none());
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.multiview_effects.is_empty());
    assert!(runtime.multiview.layout().is_none());
    assert!(runtime.resolver_next.is_none());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
}

#[test]
fn exhausted_authority_epoch_never_revives_on_a_later_policy_edit() {
    let _serial = plx_base::testlock::serial();
    let (mut app, _, _) = setup();
    let runtime = app.ott.as_mut().unwrap();
    runtime.multiview_authority_epoch = u64::MAX;
    for value in [0, 1, 0] {
        let mut next = runtime.saved.clone();
        next.settings.insert("psChannels".into(), json!(value));
        runtime.observe_profile_edits(&next);
        runtime.saved = next;
        assert_eq!(runtime.multiview_authority_epoch, 0);
    }
}

// Durable denial test: URL shape and an actual typed route request do not mint
// the multiview adapter's private NativeAdmission. No request or decoder runs.
struct UnownedMultiviewGateReset(Box<App>);
impl Drop for UnownedMultiviewGateReset {
    fn drop(&mut self) {
        plx_media::player::SHARED.duration_ns.store(0, std::sync::atomic::Ordering::Release);
        plx_media::player::stop_bufferfeed(&mut self.0.player.session, &mut self.0.adapters.player);
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&self.0.player.session);
    }
}

#[test]
fn signed_looking_multiview_url_without_private_admission_cannot_release_native_load() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready_app_fixture();
    app.running = true;
    let mut rig = UnownedMultiviewGateReset(Box::new(app));
    let app = &mut *rig.0;
    plx_media::player::TX.reset();
    plx_media::route::cancel_play(&mut app.player.session);
    plx_media::route::reset_player_control_for_test(&app.player.session);
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let info = plx_media::ff::inspect::MediaInfo {
        tracks: Default::default(), resolved_source: None, hls_playback: None,
        video_codec: "h264".into(), audio_codec: "aac".into(), width: 384, height: 288,
        fps: 10.0, duration_ns: 68_000_000_000, audio_channels: 2,
        audio_sample_rate: 48_000, audio_bit_depth: 16, dovi: Default::default(),
    };
    // This is a synthetic string, not a real HMAC lease or source grant.
    let source = plx_media::ff::ExternalSource::Hls(
        "https://compositor.example.invalid/multiview/v1/media/synthetic-unowned-lease/index.m3u8".into());
    plx_media::route::request_external_source(&mut app.player.session, &info, source,
        "Synthetic multiview", "Fixture", plx_platform::devcaps::assumed_shared()).unwrap();
    assert!(plx_media::route::playback_identity(&app.player.session).is_some(),
        "the real route owner minted a playback identity, not this test");
    assert!(app.ott.as_ref().unwrap().multiview_picture.is_none());
    assert!(app.ott.as_ref().unwrap().playing.is_none());
    assert!(!super::super::foreign_native_start_allowed(app));

    // Use the real deferred-release admission boundary. The fake held teardown
    // prevents any native worker start; publish_playback_frame must cancel it.
    app.adapters.player.begin_release_for_test(std::time::Duration::ZERO);
    assert_eq!(plx_media::player::start_bufferfeed_tracked(
        &mut app.player.session, &mut app.adapters.player),
        plx_media::player::BufferfeedStartOutcome::Deferred);
    assert!(app.adapters.player.release_has_queued_start());
    crate::app::run::publish_playback_frame(app, false);
    assert!(!app.adapters.player.release_has_queued_start());
    assert!(!app.adapters.player.is_live());
    let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt())
        .expect("host fixture clock");
    super::super::tick_with_start(app, sample, |_, _, _, _, _| {
        panic!("unowned synthetic multiview source reached native decoder admission")
    });
    assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), saved);
    assert!(app.ott.as_ref().unwrap().multiview_effects.is_empty());
    assert_eq!(app.ott.as_ref().unwrap().multiview.admitted_pipeline_slots(), 0);
    assert!(!app.adapters.player.is_live());
}
