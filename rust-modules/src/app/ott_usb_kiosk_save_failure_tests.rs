//! Real worker receipts and real Saved failures feed the existing retry owner.
use super::*;

struct NativeReset;
impl Drop for NativeReset {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}

fn stage_failed_writer(r: &mut Runtime, a: &App, directory: &Directory, fingerprint: &str) {
    assert!(r.stage_kiosk_startup(a));
    let _candidate = stage_kiosk(r, directory, fingerprint);
    arm_writer(r);
    r.save_navigation = SaveNavigation::Preserve;
}

fn fail_writer(mut a: App, r: Runtime) -> App {
    r.tx.send(Job::Saved(Err("Synthetic protected writer failure"), None, false,
        ok(json!({"persisted":false}))))
        .unwrap_or_else(|_| panic!("writer completion receiver"));
    a.ott = Some(r);
    let sample = plx_base::task::time::ClockSample::capture_main(a.adapters.player.mt()).unwrap();
    crate::app::ott_runtime::tick(&mut a, sample);
    a
}

#[test]
fn current_kiosk_writer_failure_waits_ten_seconds_without_publishing_source_or_checkpoint() {
    let _lock = plx_base::testlock::serial();
    let directory = Directory::new();
    let a = app();
    let (mut r, fingerprint) = kiosk_runtime();
    let resume = SavedResume { entry_id: "permanent-channel".into(), position_ms: 12_000,
        source_fingerprint: profile_fingerprint(r.active().unwrap()) };
    r.saved.resume.insert("permanent-tv".into(), resume.clone());
    r.checkpoints.insert("permanent-tv".into(), resume);
    let saved = serde_json::to_value(&r.saved).unwrap();
    let checkpoints = r.checkpoints.clone();
    stage_failed_writer(&mut r, &a, &directory, &fingerprint);
    let generation = r.generation;
    let mut a = fail_writer(a, r);
    let mut r = a.ott.take().unwrap();
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), saved);
    assert!(r.checkpoints == checkpoints);
    assert!(r.local_inventory.sources.is_empty());
    assert!(r.local_inventory.usb.install.is_none());
    assert!(r.local_inventory.usb.error.is_none(), "Startup consumed the one failure");
    assert!(r.local_inventory.usb.pending.is_none(), "failed Saved must not immediately queue another USB read");
    assert_eq!(r.generation, generation);
    let now = r.now_ms();
    for offset in [0, 1, 9_999] {
        r.kiosk_recovery_tick_at(&mut a, now + offset);
        assert!(r.local_inventory.usb.pending.is_none());
        assert_eq!(r.generation, generation);
    }
    r.kiosk_recovery_tick_at(&mut a, now + 10_000);
    assert!(matches!(r.local_inventory.usb.pending, Some(usb::Task::Restore { .. })));
    assert_ne!(r.generation, generation);
    assert!(r.local_inventory.sources.is_empty());
    assert!(r.checkpoints == checkpoints);
}

#[test]
fn stale_failed_writer_cannot_retry_after_newer_navigation_or_playback_epoch() {
    let _lock = plx_base::testlock::serial();
    for navigation in [true, false] {
        let directory = Directory::new();
        let a = app();
        let (mut r, fingerprint) = kiosk_runtime();
        stage_failed_writer(&mut r, &a, &directory, &fingerprint);
        if navigation { r.navigation_epoch = r.navigation_epoch.wrapping_add(1); }
        else { r.playback_epoch = r.playback_epoch.wrapping_add(1); }
        let generation = r.generation;
        let mut a = fail_writer(a, r);
        let mut r = a.ott.take().unwrap();
        assert!(r.local_inventory.usb.error.is_none());
        assert!(r.local_inventory.usb.install.is_none());
        let now = r.now_ms();
        r.kiosk_recovery_tick_at(&mut a, now + 10_000);
        assert!(r.local_inventory.usb.pending.is_none());
        assert_eq!(r.generation, generation);
        assert!(r.local_inventory.sources.is_empty());
    }
}

#[test]
fn stale_failed_writer_preserves_newer_native_route_and_never_retries_old_kiosk() {
    let _lock = plx_base::testlock::serial();
    let _reset = NativeReset;
    let directory = Directory::new();
    let mut a = app();
    let (mut r, fingerprint) = kiosk_runtime();
    stage_failed_writer(&mut r, &a, &directory, &fingerprint);
    let epoch = r.playback_epoch;
    let generation = r.generation;
    a.ott = Some(r);
    crate::app::ott_runtime::tests::local_integration::install(&mut a, None, "new-native-after-kiosk");
    let mut r = a.ott.take().unwrap();
    r.playing.as_mut().unwrap().transient_owner = None;
    assert_eq!(r.playback_epoch, epoch);
    let identity = plx_media::route::playback_identity(&a.player.session).unwrap();
    let source = plx_media::route::external_source(&a.player.session).unwrap();
    let ticket = plx_media::route::play_request_ticket();
    let mut a = fail_writer(a, r);
    let mut r = a.ott.take().unwrap();
    assert!(r.local_inventory.usb.error.is_none());
    assert!(r.local_inventory.usb.install.is_none());
    let now = r.now_ms();
    r.kiosk_recovery_tick_at(&mut a, now + 10_000);
    assert!(plx_media::route::playback_identity_matches(&a.player.session, &identity));
    assert!(plx_media::route::external_source(&a.player.session) == Some(source));
    assert!(plx_media::route::play_request_is_current(ticket));
    assert!(r.playing.as_ref().is_some_and(|playing| playing_matches(playing, &a)));
    assert!(r.local_inventory.usb.pending.is_none());
    assert_eq!(r.generation, generation);
    assert_eq!(r.playback_epoch, epoch);
}
