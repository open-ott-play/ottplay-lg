//! Real queue draining and native feed/clock seams; no network or decoder starts.
use super::*;

struct Rig { engine: Engine, mt: MainThread, volume: super::super::volume::VolumeState }
impl Rig {
    fn new(duration_ns: i64, audio: bool) -> Self {
        SHARED.reset_session();
        TX.reset();
        super::super::ffi_host::force_clocksink_for_test(true);
        let epoch = SHARED.begin_native_session().unwrap();
        assert!(SHARED.mark_native_load_returned(epoch, None).is_some());
        let mut engine = engine_after_reload_for_test();
        engine.native_epoch = epoch;
        SHARED.duration_ns.store(duration_ns, Ordering::Release);
        SHARED.seek_to_ns.store(-1, Ordering::Release);
        SHARED.pres_fed.store(PRES_NONE, Ordering::Relaxed);
        SHARED.hls_audio_expected.store(audio, Ordering::Release);
        Self { engine, mt: unsafe { MainThread::assume() },
            volume: super::super::volume::VolumeState::new() }
    }
    fn segment(&mut self, video_frames: i64, audio_frames: i64, eof: bool) {
        let bytes = [0u8; 32];
        let video = &mut **self.engine.aq_video.as_mut().unwrap();
        for i in 0..video_frames {
            assert_eq!(crate::aq::aq_push(video, bytes.as_ptr(), bytes.len() as c_int,
                i * 40_000_000, c_int::from(i == 0), 1), 0);
        }
        SHARED.hls_video_tail_ns.store((video_frames - 1) * 40_000_000, Ordering::Release);
        if eof { crate::aq::aq_set_eof(video); }
        let audio = &mut **self.engine.aq_audio.as_mut().unwrap();
        for i in 0..audio_frames {
            assert_eq!(crate::aq::aq_push(audio, bytes.as_ptr(), bytes.len() as c_int,
                i * 20_000_000, 0, 2), 0);
        }
        if audio_frames > 0 {
            SHARED.hls_audio_tail_ns.store((audio_frames - 1) * 20_000_000, Ordering::Release);
        }
        if eof { crate::aq::aq_set_eof(audio); }
    }
    fn tick(&mut self) { feed_both_lanes(&self.mt, &mut self.engine, &mut self.volume); }
    fn prime(&mut self) { try_prime(&self.mt, &mut self.engine, &mut self.volume); }
    fn finish(&mut self) {
        crate::aq::aq_set_eof(&mut **self.engine.aq_video.as_mut().unwrap());
        crate::aq::aq_set_eof(&mut **self.engine.aq_audio.as_mut().unwrap());
    }
}
fn destroy_queues(engine: &mut Engine) {
        for q in [&mut engine.aq_video, &mut engine.aq_audio].into_iter().flatten() {
            crate::aq::aq_abort(&mut **q);
            loop {
                let n = crate::aq::aq_pop(&mut **q, std::ptr::null_mut());
                if n.is_null() { break; }
                unsafe { libc::free(n.cast()); }
            }
            crate::aq::aq_destroy(&mut **q);
        }
}
impl Drop for Rig {
    fn drop(&mut self) {
        destroy_queues(&mut self.engine);
        super::super::ffi_host::force_clocksink_for_test(false);
        SHARED.reset_session();
        TX.reset();
    }
}

#[test]
fn finite_short_av_tail_waits_for_audio_to_reach_the_native_sink_then_plays() {
    let _serial = plx_base::testlock::serial();
    let mut rig = Rig::new(600_000_000, true);
    rig.segment(15, 14, true); // 560 ms video, 260 ms audio: below both prime thresholds.
    feed_stream(&rig.mt, &mut rig.engine, &mut rig.volume);
    assert!(rig.engine.eos_pushed);
    assert!(rig.engine.accepted_any_au);
    rig.prime();
    assert!(rig.engine.prime_play, "queued audio is not accepted audio");
    feed_audio_lane(&rig.mt, &mut rig.engine);
    rig.prime();
    assert!(!rig.engine.prime_play, "a complete finite clip cannot wait for nonexistent later AUs");
}

#[test]
fn finite_audioless_clip_below_video_only_prime_window_plays_at_true_eof() {
    let _serial = plx_base::testlock::serial();
    let mut rig = Rig::new(1_000_000_000, false);
    rig.segment(25, 0, true);
    rig.tick();
    assert!(rig.engine.eos_pushed && rig.engine.accepted_any_au);
    assert!(rig.engine.max_fed_video_pts < PRIME_VIDEO_MAX_NS);
    assert!(!rig.engine.prime_play);
}

#[test]
fn ordinary_av_prime_still_starts_before_eof_when_both_lanes_have_runway() {
    let _serial = plx_base::testlock::serial();
    let mut rig = Rig::new(0, true);
    rig.segment(30, 30, false);
    for _ in 0..4 { rig.tick(); }
    assert!(!rig.engine.eos_pushed);
    assert!(!rig.engine.prime_play);
    assert!(rig.engine.max_fed_video_pts >= PRIME_NS);
    assert!(rig.engine.max_fed_audio_pts >= PRIME_AUDIO_NS);
}

#[test]
fn live_short_or_empty_buffers_do_not_borrow_finite_eof_escape() {
    let _serial = plx_base::testlock::serial();
    for audio in [false, true] {
        let mut rig = Rig::new(0, audio);
        rig.segment(15, if audio { 14 } else { 0 }, false);
        rig.tick();
        assert!(rig.engine.accepted_any_au);
        assert!(rig.engine.prime_play);
        rig.finish(); // Even an erroneous live EOF supplies no finite certificate.
        rig.prime();
        assert!(rig.engine.prime_play);
    }
    let mut rig = Rig::new(600_000_000, false);
    rig.segment(0, 0, true);
    rig.tick();
    assert!(!rig.engine.accepted_any_au);
    assert!(rig.engine.prime_play);
}

#[test]
fn failed_or_superseded_native_source_cannot_release_a_short_finite_tail() {
    let _serial = plx_base::testlock::serial();
    for stale in [false, true] {
        let mut rig = Rig::new(600_000_000, true);
        rig.segment(15, 14, false);
        rig.tick();
        assert!(rig.engine.prime_play && rig.engine.accepted_any_au);
        rig.finish();
        if stale {
            let old = rig.engine.native_epoch;
            SHARED.reset_session();
            let replacement = SHARED.begin_native_session().unwrap();
            assert_ne!(replacement, old);
            assert!(SHARED.mark_native_load_returned(replacement, None).is_some());
            assert!(SHARED.arm_initial_clock_hold(false, false));
            SHARED.duration_ns.store(600_000_000, Ordering::Release);
            SHARED.hls_video_tail_ns.store(560_000_000, Ordering::Release);
            SHARED.hls_audio_tail_ns.store(260_000_000, Ordering::Release);
            SHARED.hls_audio_expected.store(true, Ordering::Release);
        } else {
            SHARED.demux_io_failed.store(true, Ordering::Release);
        }
        rig.prime();
        assert!(rig.engine.prime_play);
    }
}

#[test]
fn short_finite_tail_respects_user_pause_candidate_and_new_seek_fences() {
    let _serial = plx_base::testlock::serial();
    for guard in ["pause", "candidate", "seek"] {
        let mut rig = Rig::new(600_000_000, true);
        rig.segment(15, 14, false);
        rig.tick();
        assert!(rig.engine.prime_play);
        rig.finish();
        match guard {
            "pause" => TX.paused.store(true, Ordering::Release),
            "candidate" => SHARED.hls_candidate_generation.store(1, Ordering::Release),
            "seek" => TX.seek_to_ns.store(100_000_000, Ordering::Release),
            _ => unreachable!(),
        }
        rig.prime();
        assert!(rig.engine.prime_play, "{guard} must retain input/source authority");
    }
}

#[test]
fn acb_short_finite_prime_plays_before_frame_gated_binding_then_pushes_native_eos() {
    let _serial = plx_base::testlock::serial();
    struct Acb;
    impl Drop for Acb {
        fn drop(&mut self) {
            ACB_OK.store(false, Ordering::Relaxed);
            super::super::ffi_host::reset_acb_for_test(false);
            super::super::ffi_host::force_object_ready_for_test(false);
            crate::route::reset_player_control_for_test(&crate::route::PlaybackSession::IDLE);
        }
    }
    let _acb = Acb;
    let mut ps = crate::route::external_hls_session_for_test(600_000_000);
    let mut rig = Rig::new(600_000_000, true);
    // Allocate the inert replacement before any Play: its fixture constructor
    // arms an initial hold, which must not reset the clock after it starts.
    let placeholder = engine_after_reload_for_test();
    ACB_OK.store(true, Ordering::Relaxed);
    super::super::ffi_host::reset_acb_for_test(true);
    *SHARED.media_id.lock().unwrap() = Some(std::ffi::CString::new("finite-tail").unwrap());
    *SHARED.source_info.lock().unwrap() = Some(b"{\"context\":\"finite-tail\",\"video\":{}}\0".to_vec());
    rig.engine.stage = Stage::Playing;
    rig.engine.video_info_sent = false;
    rig.segment(15, 14, true);
    rig.tick();
    assert!(!rig.engine.eos_pushed, "ACB has not reached Streaming before the first frames");
    assert!(!rig.engine.prime_play, "producer EOF must permit the first Play without native EOS");

    let mut pa = super::super::adapter::PlayerAdapter::new(unsafe { MainThread::assume() });
    pa.install(std::mem::replace(&mut rig.engine, placeholder));
    super::super::ffi_host::force_object_ready_for_test(true);
    SHARED.frames.store(0, Ordering::Relaxed);
    super::super::pump::pump(&mut ps, &mut pa, &mut rig.volume, 1_000);
    let media_id_present = SHARED.media_id.lock().unwrap().is_some();
    assert!(pa.engine().unwrap().stage == Stage::Bound,
        "stage={} acb={} media_id={}", pa.engine().unwrap().stage as u8,
        ACB_OK.load(Ordering::Relaxed), media_id_present);
    // Model the native decoder's first two frame callbacks. The following
    // binding/EOS transition is the production pump, not a manual stage change.
    SHARED.frames.store(2, Ordering::Relaxed);
    super::super::pump::pump(&mut ps, &mut pa, &mut rig.volume, 1_016);
    let actual = pa.engine().unwrap();
    assert!(actual.stage == Stage::Streaming);
    assert!(actual.eos_pushed);
    let (video_data, starts, _, _) = super::super::ffi_host::acb_calls_for_test();
    assert_eq!((video_data, starts), (1, 1));
    let mut unused = std::mem::replace(&mut rig.engine, pa.take().unwrap());
    destroy_queues(&mut unused);
}
