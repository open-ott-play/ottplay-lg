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
