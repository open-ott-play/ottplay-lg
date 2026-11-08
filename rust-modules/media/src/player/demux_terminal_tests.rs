//! Producer terminal publication versus native position callbacks. These use
//! actual host Load/Play, callback ingress, access-unit queues, and the pump.
//! The transport error is injected at the same boundary as an observed HTTPS
//! 503; these are not HTTP/FFmpeg decoder or device playback tests.
use super::*;
use crate::player::{adapter::PlayerAdapter, engine, ffi_host, shared::PlaybackState, volume::VolumeState};
use plx_telemetry::telemetry::classes::DemuxExitClass;
use std::time::{Duration, Instant};

struct Rig {
    ps: crate::route::PlaybackSession,
    pa: PlayerAdapter,
    epoch: u32,
    now: u32,
    // Last field: teardown/reset precedes releasing automatic clock delivery.
    _clock: ffi_host::ManualClockForTest,
}

impl Rig {
    fn new() -> Self {
        let clock = ffi_host::manual_clock_for_test();
        SHARED.reset_session();
        TX.reset();
        crate::player::claim_hold::clear();
        ffi_host::reset_native_lifecycle_for_test();
        ffi_host::reset_seek_probe_for_test(None);
        ffi_host::force_clocksink_for_test(true);
        let ps = crate::route::PlaybackSession::IDLE;
        crate::route::reset_player_control_for_test(&ps);
        let mut rig = Self {
            ps,
            pa: PlayerAdapter::new(unsafe { MainThread::assume() }),
            epoch: 0,
            now: 1_000,
            _clock: clock,
        };
        rig.load();
        rig
    }

    fn load(&mut self) {
        self.epoch = SHARED.begin_native_session().expect("fresh native epoch");
        assert_eq!(unsafe { crate::player::sink().load(c"{}".as_ptr(), self.epoch) }, 1);
        assert!(SHARED.mark_native_load_returned(self.epoch, None).is_some());
        let mut eng = engine::streaming_engine_for_test(self.epoch);
        eng.load_th = None;
        // The reported failure happened after the native Play was accepted.
        eng.prime_play = false;
        assert!(eng.stage >= Stage::Streaming);
        self.pa.install(eng);
        assert_eq!(unsafe { crate::player::sink().play(self.pa.mt()) }, 1);
        assert_eq!(SHARED.duration_ns.load(Relaxed), 0);
    }

    fn eng(&mut self) -> &mut Engine { self.pa.engine().expect("live test engine") }

    fn clock_without_data(&mut self) {
        let eng = self.eng();
        assert_eq!(crate::aq::aq_bytes(&mut **eng.aq_video.as_mut().unwrap()), 0);
        assert_eq!(crate::aq::aq_bytes(&mut **eng.aq_audio.as_mut().unwrap()), 0);
        assert_eq!(eng.max_fed_video_pts, 0);
        ffi_host::advance_clock_for_test(200);
        ffi_host::tick_for_test();
        assert!(SHARED.frames.load(Relaxed) > 0,
            "premise: the current native clock callback was accepted before any AU");
        assert_eq!(SHARED.playpos_ns.load(Relaxed), 0);
        assert!(!SHARED.demux_failed.load(Acquire));
    }

    fn frame(&mut self) {
        let _frame = plx_base::task::FrameScope::enter();
        self.now += 16;
        pump(&mut self.ps, &mut self.pa, &mut VolumeState::new(), self.now);
    }

    fn stop(&mut self) {
        engine::stop_bufferfeed(&mut self.ps, &mut self.pa);
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.pa.has_pending_release() {
            assert!(Instant::now() < deadline, "host Unload did not retire");
            engine::poll_release(&mut self.ps, &mut self.pa, Instant::now());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!self.pa.is_live());
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // Synchronous host teardown leaves no release worker referring to the rig.
        engine::suspend_bufferfeed(&mut self.ps, &mut self.pa);
        ffi_host::reset_native_lifecycle_for_test();
        ffi_host::reset_seek_probe_for_test(None);
        ffi_host::force_clocksink_for_test(false);
        SHARED.test_force_native_idle();
        SHARED.reset_session();
        TX.reset();
        crate::route::reset_player_control_for_test(crate::route::idle_session_for_test());
    }
}

fn finish(eng: &mut Engine, produced: bool) {
    let video = &mut **eng.aq_video.as_mut().unwrap() as *mut crate::aq::AuQueue;
    let audio = &mut **eng.aq_audio.as_mut().unwrap() as *mut crate::aq::AuQueue;
    crate::ff::finish_demux_queues(video, audio, produced, true);
}

fn refused_open(eng: &mut Engine) {
    // Exact HTTPS-open failure publication followed by the real producer tail.
    // FIRST WINS must keep this cause when NoAccessUnits is subsequently noted.
    SHARED.fail_demux(DemuxExitClass::HttpsOpenFailed);
    finish(eng, false);
    assert_eq!(SHARED.dg_demux_exit.load(Acquire), DemuxExitClass::HttpsOpenFailed.latch());
}

// The production video feeder adds its default -1 ns PTS nudge, then clamps at zero.
// The second queued 40 ms AU therefore reaches the sink at this nonzero timestamp.
const SHORT_CLIP_VIDEO_END_NS: i64 = 39_999_999;

fn feed_short_clip(eng: &mut Engine) {
    let au = [0u8; 64];
    for (q, es) in [(eng.aq_video.as_mut().unwrap(), 1), (eng.aq_audio.as_mut().unwrap(), 2)] {
        for pts in [0, 40_000_000] {
            assert_eq!(crate::aq::aq_push(&mut **q, au.as_ptr(), au.len() as _, pts, 1, es), 0);
        }
        assert!(crate::aq::aq_bytes(&mut **q) > 0);
    }
}

#[test]
fn a_current_clock_position_cannot_mask_an_empty_demux_failure() {
    let _serial = plx_base::testlock::serial();
    let mut rig = Rig::new();
    rig.clock_without_data();
    refused_open(rig.eng());
    rig.frame();
    assert_eq!(crate::player::state(&rig.ps), PlaybackState::Error,
        "zero produced AUs must fail even after a valid position callback");
    assert!(!rig.eng().eos_pushed, "failed open is not a successful file EOF");
    assert!(!SHARED.ended.load(Relaxed));
}

#[test]
fn empty_failure_between_the_pump_check_and_feed_cannot_push_success_eos() {
    let _serial = plx_base::testlock::serial();
    let mut rig = Rig::new();
    rig.clock_without_data();
    let mut visited = false;
    {
        let _frame = plx_base::task::FrameScope::enter();
        pump_with_before_feed(&mut rig.ps, &mut rig.pa, &mut VolumeState::new(), 1_016, |eng| {
            assert!(!SHARED.demux_failed.load(Acquire));
            visited = true;
            refused_open(eng);
        });
    }
    assert!(visited, "premise: the actual pump reached its post-check feed boundary");
    assert!(!rig.eng().eos_pushed, "a newly published failed terminal must not become clean EOS");
    rig.frame();
    assert_eq!(crate::player::state(&rig.ps), PlaybackState::Error);
    assert!(!SHARED.ended.load(Relaxed));
}

#[test]
fn a_produced_clip_with_unknown_duration_still_ends_through_pipeline_eos() {
    let _serial = plx_base::testlock::serial();
    let mut rig = Rig::new();
    feed_short_clip(rig.eng());
    finish(rig.eng(), true);
    rig.frame();
    assert!(rig.eng().eos_pushed);
    assert_eq!(rig.eng().max_fed_video_pts, SHORT_CLIP_VIDEO_END_NS,
        "premise: actual media reached the sink before its EOF");
    ffi_host::advance_clock_for_test(200);
    ffi_host::tick_for_test();
    assert!(SHARED.pipeline_eos.load(Relaxed), "the real sink reported completion");
    rig.frame();
    assert_eq!(SHARED.duration_ns.load(Relaxed), 0);
    assert!(SHARED.ended.load(Relaxed));
    assert_ne!(crate::player::state(&rig.ps), PlaybackState::Error);
}

#[test]
fn a_midstream_generic_demux_failure_keeps_its_existing_drain_policy() {
    let _serial = plx_base::testlock::serial();
    let mut rig = Rig::new();
    feed_short_clip(rig.eng());
    rig.frame();
    ffi_host::advance_clock_for_test(200);
    ffi_host::tick_for_test();
    assert!(SHARED.frames.load(Relaxed) > 0);
    SHARED.fail_demux(DemuxExitClass::DemuxPanicked);
    finish(rig.eng(), true);
    rig.frame();
    assert!(rig.eng().eos_pushed);
    assert!(!SHARED.demux_io_failed.load(Acquire),
        "produced media must not be reclassified as an empty failed open");
    assert_ne!(crate::player::state(&rig.ps), PlaybackState::Error);
}

#[test]
fn an_aborted_empty_producer_cannot_fail_stop_or_a_new_native_epoch() {
    let _serial = plx_base::testlock::serial();
    let mut rig = Rig::new();
    rig.clock_without_data();
    let old_epoch = rig.epoch;
    let eng = rig.eng();
    crate::aq::aq_abort(&mut **eng.aq_video.as_mut().unwrap());
    crate::aq::aq_abort(&mut **eng.aq_audio.as_mut().unwrap());
    finish(eng, false);
    assert!(!SHARED.demux_failed.load(Acquire));
    assert!(!SHARED.demux_io_failed.load(Acquire));
    rig.stop();
    rig.frame();
    assert_eq!(crate::player::state(&rig.ps), PlaybackState::Idle);
    rig.load();
    assert_ne!(old_epoch, rig.epoch);
    crate::player::sf_on_event(old_epoch, 0, 9_000_000_000, std::ptr::null());
    assert_eq!(SHARED.playpos_ns.load(Relaxed), 0,
        "the retired native callback must not move the new epoch's empty clock");
    assert!(!SHARED.demux_failed.load(Acquire));
    assert!(!SHARED.demux_io_failed.load(Acquire));
    feed_short_clip(rig.eng());
    rig.frame();
    assert_ne!(crate::player::state(&rig.ps), PlaybackState::Error);
    assert!(!rig.eng().eos_pushed);
}

#[test]
fn controlled_clock_drains_automatic_ticks_and_drop_restores_the_mode() {
    let _serial = plx_base::testlock::serial();
    let mut rig = Rig::new();
    let before = SHARED.frames.load(Relaxed);
    ffi_host::automatic_tick_for_test();
    assert_eq!(SHARED.frames.load(Relaxed), before,
        "automatic delivery is disabled even for an already-running ticker");
    rig.clock_without_data();
    assert!(SHARED.frames.load(Relaxed) > before,
        "manual delivery still uses the real callback path");
    drop(rig);
    assert!(!ffi_host::clock_state_for_test().1,
        "the old clock is reset before its scheduling guard is released");
    let _again = ffi_host::manual_clock_for_test();
    ffi_host::automatic_tick_for_test();
    assert_eq!(SHARED.frames.load(Relaxed), 0);
}

#[test]
fn a_position_callback_needs_an_accepted_access_unit_before_first_picture_credit() {
    use crate::player::recovery::Phase;
    let _serial = plx_base::testlock::serial();
    let mut rig = Rig::new();
    assert_eq!(rig.ps.open_recovery().phase(), Phase::Opening);
    rig.clock_without_data();
    rig.frame();
    assert_eq!(rig.ps.open_recovery().phase(), Phase::Opening,
        "a clock report before any AU must not confirm that the source produced a picture");

    feed_short_clip(rig.eng());
    rig.frame();
    assert_eq!(rig.eng().max_fed_video_pts, SHORT_CLIP_VIDEO_END_NS,
        "premise: the actual feed accepted the short clip");
    ffi_host::advance_clock_for_test(200);
    ffi_host::tick_for_test();
    rig.frame();
    assert_eq!(rig.ps.open_recovery().phase(), Phase::Playing,
        "an accepted media feed followed by a native position report still earns first-picture credit");
}


fn push_one_au(eng: &mut Engine, es: i32, pts: i64) {
    let q = if es == 1 { eng.aq_video.as_mut() } else { eng.aq_audio.as_mut() }.unwrap();
    let au = [0u8; 4];
    assert_eq!(crate::aq::aq_push(&mut **q, au.as_ptr(), au.len() as _, pts, 1, es), 0);
}

#[test]
fn zero_or_negative_pts_accepted_in_either_lane_can_credit_picture() {
    use crate::player::recovery::Phase;
    let _serial = plx_base::testlock::serial();
    for es in [1, 2] {
        for pts in [0, -1] {
            let mut rig = Rig::new();
            assert_eq!(rig.ps.open_recovery().phase(), Phase::Opening);
            assert!(!rig.eng().accepted_any_au);
            push_one_au(rig.eng(), es, pts);
            rig.frame();
            assert_eq!(ffi_host::feed_log_for_test(es), [0]);
            assert!(rig.eng().accepted_any_au);
            assert_eq!(rig.eng().max_fed_video_pts, 0);
            assert_eq!(rig.eng().max_fed_audio_pts, 0);
            ffi_host::advance_clock_for_test(200);
            ffi_host::tick_for_test();
            rig.frame();
            assert_eq!(rig.ps.open_recovery().phase(), Phase::Playing);
        }
    }
}

#[test]
fn refused_feeds_keep_the_au_pending_and_never_credit_picture() {
    use crate::player::recovery::Phase;
    let _serial = plx_base::testlock::serial();
    for es in [1, 2] {
        for reply in [b'B', b'e'] {
            let mut rig = Rig::new();
            assert_eq!(rig.ps.open_recovery().phase(), Phase::Opening);
            push_one_au(rig.eng(), es, 0);
            let refusal = ffi_host::refuse_feed_for_test(reply);
            rig.frame();
            assert!(!rig.eng().accepted_any_au);
            let pending = if es == 1 { &rig.eng().pending_video } else { &rig.eng().pending_audio };
            assert!(pending.is_some(), "a refusal retains the exact AU for the next feed");
            assert!(ffi_host::feed_log_for_test(es).is_empty());
            ffi_host::advance_clock_for_test(200);
            ffi_host::tick_for_test();
            rig.frame();
            assert_eq!(rig.ps.open_recovery().phase(), Phase::Opening);
            assert!(!rig.eng().accepted_any_au);
            drop(refusal);
            rig.frame();
            assert_eq!(ffi_host::feed_log_for_test(es), [0], "Drop restores real Feed acceptance");
            assert!(rig.eng().accepted_any_au);
            ffi_host::advance_clock_for_test(200);
            ffi_host::tick_for_test();
            rig.frame();
            assert_eq!(rig.ps.open_recovery().phase(), Phase::Playing);
        }
    }
}

#[test]
fn sample_feed_requires_acceptance_too() {
    use crate::player::recovery::Phase;
    let _serial = plx_base::testlock::serial();
    let mut rig = Rig::new();
    assert_eq!(rig.ps.open_recovery().phase(), Phase::Opening);
    // This fixture started as an inert Stream. Release its initialized empty queues before
    // changing variants: production Sample teardown does not own Stream queue cleanup.
    {
        let eng = rig.eng();
        for mut q in [eng.aq_video.take(), eng.aq_audio.take()].into_iter().flatten() {
            crate::aq::aq_destroy(&mut *q);
        }
    }
    rig.eng().source = Source::Sample(Box::new(engine::SampleBuf {
        data: vec![0; 4], au: vec![0, 4], next: 0, loops: 0,
    }));
    let refusal = ffi_host::refuse_feed_for_test(b'e');
    rig.frame();
    assert!(!rig.eng().accepted_any_au);
    assert!(ffi_host::feed_log_for_test(1).is_empty());
    ffi_host::advance_clock_for_test(200);
    ffi_host::tick_for_test();
    rig.frame();
    assert_eq!(rig.ps.open_recovery().phase(), Phase::Opening);
    drop(refusal);
    rig.frame();
    assert!(rig.eng().accepted_any_au);
    assert_eq!(ffi_host::feed_log_for_test(1).first(), Some(&0));
    ffi_host::advance_clock_for_test(200);
    ffi_host::tick_for_test();
    rig.frame();
    assert_eq!(rig.ps.open_recovery().phase(), Phase::Playing);
}

#[test]
fn real_stop_retires_empty_failure_and_accepted_media_evidence() {
    let _serial = plx_base::testlock::serial();
    let mut rig = Rig::new();
    assert!(!SHARED.demux_empty_failed.load(Acquire));
    refused_open(rig.eng());
    assert!(SHARED.demux_empty_failed.load(Acquire));
    assert!(!SHARED.demux_io_failed.load(Acquire), "empty source retains its MediaSource cause");
    rig.stop();
    assert!(!SHARED.demux_empty_failed.load(Acquire));
    rig.load();
    assert!(!rig.eng().accepted_any_au);
    push_one_au(rig.eng(), 1, 0);
    rig.frame();
    assert!(rig.eng().accepted_any_au);
    rig.stop();
    rig.load();
    assert!(!rig.eng().accepted_any_au, "a new Engine cannot inherit an old accepted AU");
    assert!(!SHARED.demux_empty_failed.load(Acquire));
}


#[test]
fn empty_clock_report_keeps_auto_hls_fallback_but_accepted_media_does_not() {
    use crate::player::recovery::{LoadRefusal, OpenRecovery, RecoveryEffect, RecoveryEvent};
    let _serial = plx_base::testlock::serial();
    for produced in [false, true] {
        let mut rig = Rig::new();
        if produced {
            feed_short_clip(rig.eng());
            rig.frame();
            assert!(rig.eng().accepted_any_au);
        }
        ffi_host::advance_clock_for_test(200);
        ffi_host::tick_for_test();
        assert!(SHARED.frames.load(Relaxed) > 0);
        let mut facts = open_facts(&rig.ps, rig.pa.engine().unwrap(), None);
        assert_eq!(facts.no_frames, !produced);
        // Supply the existing Auto Original policy input to the real pure recovery machine.
        // Only media evidence comes from this actual engine/callback path; no network is run.
        facts.auto_original = true;
        let mut recovery = OpenRecovery::new(std::sync::Arc::new(plx_platform::devcaps::Caps::assumed()));
        let mut effects = Vec::new();
        let _ = recovery.step(&RecoveryEvent::Refused { refusal: LoadRefusal::SourceFailed, facts }, &mut effects);
        if produced {
            assert_eq!(effects, [RecoveryEffect::ShowError { refusal: LoadRefusal::SourceFailed }]);
        } else {
            assert_eq!(effects, [RecoveryEffect::StartAutoHls]);
        }
    }
}
