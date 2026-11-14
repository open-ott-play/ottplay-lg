use super::*;

fn track(codec: &str, decoder_available: bool) -> InspectedTrack {
    InspectedTrack { kind: inspect::TrackKind::Subtitle, stream_index: 2, ordinal: 0,
        codec: codec.into(), language: "en".into(), title: "English".into(), channels: 0,
        sample_rate: 0, bit_depth: 0, decoder_available, disposition_known: false, default: false, forced: false }
}
fn scope(pid: u16) -> Scope {
    Scope { source: "https://example.invalid/media.m3u8".into(), worker: 7, discontinuity: 3,
        transport: super::super::hls_observed::TransportIdentity { transport_stream_id: 1, program_id: 1, pid, stream_type: 0x06 } }
}
fn text_state() -> State {
    State { scope: Some(scope(300)), sequence: Some(10), committed: Some(9),
        decoder: Some(Decoder { codec: "webvtt".into(), codec_id: 0, extra: Vec::new(),
            header: Header::Text(TextDialect::WebVtt), context: std::ptr::null_mut(),
            charset: crate::player::sub_charset::TrackCharset::new(Some("en")), track: 0 }),
        pending: Some(Pending::default()), reset: true }
}
#[test]
fn supported_codecs_do_not_infer_cea_from_a_decoder_or_title() {
    for codec in ["ass", "ssa", "webvtt", "subrip", "text", "mov_text"] { assert!(supported(&track(codec, false))); }
    for codec in ["hdmv_pgs_subtitle", "dvd_subtitle", "dvb_subtitle"] {
        assert!(!supported(&track(codec, false)));
        assert!(supported(&track(codec, true)));
    }
    for codec in ["eia_608", "cea_708", "dvb_teletext", "unknown"] { assert!(!supported(&track(codec, true))); }
    let mut audio = track("webvtt", true);
    audio.kind = inspect::TrackKind::Audio;
    assert!(!supported(&audio));
}
#[test]
fn abort_discards_uncommitted_decoder_and_pending_cues() {
    let mut state = text_state();
    state.pending.as_mut().unwrap().cues.push(Cue::Text { track: 0, start: 0, end: 1,
        payload: b"Never published".to_vec(), identity: 1 });
    state.abort();
    assert!(state.commit().is_err());
    assert!(state.scope.is_none() && state.decoder.is_none() && state.pending.is_none());
}
#[test]
fn memory_and_cue_bounds_refuse_before_retaining_the_next_cue() {
    let mut pending = Pending::default();
    pending.admit(SEGMENT_BYTES).unwrap();
    assert!(pending.admit(1).is_err());
    let mut pending = Pending::default();
    for _ in 0..CUE_LIMIT { pending.cues.push(Cue::Ass { track: 0, start: 0, end: 1, payload: Vec::new() }); }
    assert!(pending.admit(0).is_err());
}
#[test]
fn only_commit_advances_continuity_and_returns_owned_payload() {
    let mut state = text_state();
    let mut payload = b"Committed subtitle".to_vec();
    state.pending.as_mut().unwrap().cues.push(Cue::Text { track: 0, start: 100, end: 200,
        payload: payload.clone(), identity: 7 });
    payload.fill(0);
    assert_eq!(state.committed, Some(9));
    let batch = state.commit().unwrap();
    assert_eq!(state.committed, Some(10));
    assert_eq!(batch.cue_count(), 1);
    let Cue::Text { payload, .. } = &batch.cues[0] else { panic!("text cue"); };
    assert_eq!(payload, b"Committed subtitle");
    assert!(batch.reset);
    assert!(state.commit().is_err());
}
#[test]
fn invalid_packet_poisons_the_segment_instead_of_committing_partial_cues() {
    let mut state = text_state();
    assert!(unsafe { state.packet(std::ptr::null_mut(), std::ptr::null_mut()) }.is_err());
    assert!(state.commit().is_err());
}

#[test]
fn bitmap_display_intervals_use_authored_offsets_and_unknown_end_is_open() {
    assert_eq!(bitmap_times(10_000_000_000, AV_NOPTS_VALUE, 250, 1750),
        Ok((10_250_000_000, Some(11_750_000_000))));
    assert_eq!(bitmap_times(0, 2_000_000, 100, u32::MAX), Ok((2_100_000_000, None)));
    assert_eq!(bitmap_times(0, AV_NOPTS_VALUE, 0, 0), Ok((0, None)));
    assert!(bitmap_times(0, AV_NOPTS_VALUE, 200, 100).is_err());
    assert!(bitmap_times(i64::MAX, AV_NOPTS_VALUE, 1, 2).is_err());
}

#[test]
fn owned_batch_remaps_all_timestamps_and_refuses_overflow() {
    let batch = Batch { cues: vec![
        Cue::Text { track: 0, start: 10, end: 20, payload: b"text".to_vec(), identity: 1 },
        Cue::Ass { track: 0, start: 12, end: 22, payload: b"ass".to_vec() },
        Cue::Bitmap(subs::SubCue::BitmapClear { track: 0, pts: 25 }),
        Cue::BitmapEnd { track: 0, start: 10, end: 30 },
    ], header: Header::Text(TextDialect::WebVtt), track: 0, reset: true };
    let batch = batch.shifted(100).unwrap();
    assert!(matches!(batch.cues[0], Cue::Text { start: 110, end: 120, .. }));
    assert!(matches!(batch.cues[1], Cue::Ass { start: 112, end: 122, .. }));
    assert!(matches!(batch.cues[2], Cue::Bitmap(subs::SubCue::BitmapClear { pts: 125, .. })));
    assert!(matches!(batch.cues[3], Cue::BitmapEnd { start: 110, end: 130, .. }));
    assert!(batch.shifted(i64::MAX).is_none());
}

#[test]
fn text_ass_and_bitmap_share_content_clock_with_display_base_and_cancelled_sink_shift() {
    let source = 1_000_000_000i64;
    let transport = 2_000_000_000i64;
    let display_base = 9_000_000_000i64;
    let sink_shift = 3_000_000_000i64;
    let callback_position = (source + transport + sink_shift) - sink_shift + display_base;
    let batch = Batch { cues: vec![
        Cue::Text { track: 0, start: source, end: source + 1_000_000_000, payload: b"text".to_vec(), identity: 1 },
        Cue::Ass { track: 0, start: source, end: source + 1_000_000_000, payload: b"ass".to_vec() },
        Cue::Bitmap(subs::SubCue::BitmapClear { track: 0, pts: source }),
    ], header: Header::Text(TextDialect::WebVtt), track: 0, reset: true };
    let batch = batch.shifted(transport).unwrap().shifted(display_base).unwrap();
    assert!(matches!(batch.cues[0], Cue::Text { start, end, .. }
        if start == callback_position && end == callback_position + 1_000_000_000));
    assert!(matches!(batch.cues[1], Cue::Ass { start, end, .. }
        if start == callback_position && end == callback_position + 1_000_000_000));
    assert!(matches!(batch.cues[2], Cue::Bitmap(subs::SubCue::BitmapClear { pts, .. })
        if pts == callback_position));
}

#[test]
fn committed_bitmap_end_is_finite_and_newer_display_cannot_reveal_older_overlap() {
    let _serial = plx_base::testlock::serial();
    SHARED.reset_session();
    let rect = || crate::player::SubRect { x: 0, y: 0, w: 1, h: 1,
        index: vec![1], palette: Box::new([[0; 4]; 256]) };
    let batch = Batch { cues: vec![
        Cue::Bitmap(subs::SubCue::BitmapSet { track: 0, pts: 100, cw: 1, ch: 1,
            rects: vec![rect()], total_rects: 1 }),
        Cue::BitmapEnd { track: 0, start: 100, end: 1000 },
        Cue::Bitmap(subs::SubCue::BitmapSet { track: 0, pts: 200, cw: 1, ch: 1,
            rects: vec![rect()], total_rects: 1 }),
        Cue::BitmapEnd { track: 0, start: 200, end: 300 },
    ], header: Header::Bitmap, track: 0, reset: true };
    batch.publish("fixture:bitmap", TextStyle::default(), &mut None, 0);
    let cues = SHARED.sub_bitmaps.lock().unwrap();
    assert_eq!(cues.iter().map(|cue| (cue.start_ns, cue.end_ns)).collect::<Vec<_>>(),
        vec![(100, 200), (200, 300)]);
    drop(cues);
    SHARED.sub_bitmaps.lock().unwrap().clear();
    ass_source::retire_selection();
    SHARED.reset_session();
}

fn bitmap_set(track: i32, pts: i64) -> Cue {
    Cue::Bitmap(subs::SubCue::BitmapSet { track, pts, cw: 1, ch: 1, total_rects: 1,
        rects: vec![crate::player::SubRect { x: 0, y: 0, w: 1, h: 1,
            index: vec![1], palette: Box::new([[0; 4]; 256]) }] })
}
fn publish_bitmaps(cues: Vec<Cue>, reset: bool, generation: &mut Option<u64>) {
    Batch { cues, header: Header::Bitmap, track: 0, reset }
        .publish("fixture:bitmap-clear", TextStyle::default(), generation, 0);
}
struct BitmapReset(i64);
impl BitmapReset {
    fn new() -> Self {
        let previous = crate::player::subtitle_offset_ms();
        crate::player::set_subtitle_offset(0);
        Self(previous)
    }
}
impl Drop for BitmapReset {
    fn drop(&mut self) {
        crate::player::request_subtitle(-1);
        ass_source::retire_selection();
        SHARED.reset_session();
        crate::player::set_subtitle_offset(self.0);
    }
}

#[test]
fn explicit_bitmap_clear_shortens_a_previous_finite_end() {
    let _serial = plx_base::testlock::serial();
    SHARED.reset_session();
    let _reset = BitmapReset::new();
    crate::player::request_subtitle(0);
    let mut generation = None;
    publish_bitmaps(vec![bitmap_set(0, 0), Cue::BitmapEnd { track: 0, start: 0, end: 10_000_000_000 }],
        true, &mut generation);
    assert_eq!(crate::player::active_bitmap_key(6_000_000_000), Some(0));
    publish_bitmaps(vec![Cue::Bitmap(subs::SubCue::BitmapClear { track: 0, pts: 5_000_000_000 })],
        false, &mut generation);
    assert_eq!(SHARED.sub_bitmaps.lock().unwrap()[0].end_ns, 5_000_000_000);
    assert_eq!(crate::player::active_bitmap_key(6_000_000_000), None);
}

#[test]
fn explicit_bitmap_clear_preserves_other_tracks_and_future_displays() {
    let _serial = plx_base::testlock::serial();
    SHARED.reset_session();
    let _reset = BitmapReset::new();
    crate::player::request_subtitle(0);
    let mut generation = None;
    publish_bitmaps(vec![
        bitmap_set(0, 0), Cue::BitmapEnd { track: 0, start: 0, end: 10_000_000_000 },
        bitmap_set(1, 1_000_000_000), Cue::BitmapEnd { track: 1, start: 1_000_000_000, end: 11_000_000_000 },
        bitmap_set(0, 8_000_000_000), Cue::BitmapEnd { track: 0, start: 8_000_000_000, end: 12_000_000_000 },
    ], true, &mut generation);
    publish_bitmaps(vec![Cue::Bitmap(subs::SubCue::BitmapClear { track: 0, pts: 5_000_000_000 })],
        false, &mut generation);
    let cues = SHARED.sub_bitmaps.lock().unwrap();
    assert_eq!(cues.iter().map(|cue| (cue.track, cue.start_ns, cue.end_ns)).collect::<Vec<_>>(), vec![
        (0, 0, 5_000_000_000), (1, 1_000_000_000, 11_000_000_000),
        (0, 8_000_000_000, 12_000_000_000),
    ]);
    drop(cues);
    assert_eq!(crate::player::active_bitmap_key(6_000_000_000), None);
    assert_eq!(crate::player::active_bitmap_key(9_000_000_000), Some(8_000_000_000));
    crate::player::request_subtitle(1);
    assert_eq!(crate::player::active_bitmap_key(6_000_000_000), Some(1_000_000_000));
}

#[test]
fn scheduled_bitmap_end_only_bounds_its_authored_display_and_never_extends_it() {
    let _serial = plx_base::testlock::serial();
    SHARED.reset_session();
    let _reset = BitmapReset::new();
    crate::player::request_subtitle(0);
    let mut generation = None;
    publish_bitmaps(vec![bitmap_set(0, 0), bitmap_set(0, 5_000_000_000),
        Cue::BitmapEnd { track: 0, start: 5_000_000_000, end: 15_000_000_000 }], true, &mut generation);
    publish_bitmaps(vec![Cue::BitmapEnd { track: 0, start: 0, end: 10_000_000_000 }],
        false, &mut generation);
    let cues = SHARED.sub_bitmaps.lock().unwrap();
    assert_eq!(cues.iter().map(|cue| (cue.start_ns, cue.end_ns)).collect::<Vec<_>>(),
        vec![(0, 5_000_000_000), (5_000_000_000, 15_000_000_000)]);
    drop(cues);
    assert_eq!(crate::player::active_bitmap_key(12_000_000_000), Some(5_000_000_000));
}

#[test]
fn scheduled_bitmap_end_shifting_checks_both_authored_timestamps() {
    for (start, end, shift) in [(i64::MAX, i64::MAX, 1), (0, i64::MAX, 1), (i64::MIN, 0, -1)] {
        let batch = Batch { cues: vec![Cue::BitmapEnd { track: 0, start, end }],
            header: Header::Bitmap, track: 0, reset: false };
        assert!(batch.shifted(shift).is_none());
    }
}
