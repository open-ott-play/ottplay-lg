use super::*;

fn segment(map: &str, start: &str, end: &str, body: &str) -> String {
    format!("WEBVTT\n{map}\n\n{start} --> {end}\n{body}\n\n")
}
fn window(start: i64, end: i64) -> Window { Window { start_ns: start * 1_000_000_000, end_ns: end * 1_000_000_000 } }
fn clock() -> Clock { Clock { pts_90k: 900_000, presentation_ns: 0 } }

#[test]
fn hls_webvtt_nonzero_pes_anchor_and_local_outside_segment_map_to_video_time() {
    let text = segment("X-TIMESTAMP-MAP=MPEGTS:900000,LOCAL:01:00:00.000", "01:00:01.000", "01:00:02.000", "Caption");
    let mut assembly = Assembly::new(clock());
    assembly.push(&text, window(0, 3)).unwrap();
    let canonical = String::from_utf8(assembly.finish().unwrap()).unwrap();
    assert!(canonical.contains("00:00:01.000 --> 00:00:02.000\nCaption"));
    assert!(!canonical.contains("X-TIMESTAMP-MAP"));
    assert_eq!(text_ass::script_from_vtt(&canonical).cues, 1);
}

#[test]
fn hls_webvtt_missing_map_is_zero_to_zero_not_segment_start() {
    let text = "WEBVTT\n\n00:11.000 --> 00:12.000\nCaption\n\n";
    let mut assembly = Assembly::new(clock());
    assembly.push(text, window(1, 2)).unwrap();
    let canonical = String::from_utf8(assembly.finish().unwrap()).unwrap();
    assert!(canonical.contains("00:00:01.000 --> 00:00:02.000"));
    assert!(Assembly::new(clock()).push(text, window(50, 51)).is_err());
}

#[test]
fn hls_webvtt_wrap_matches_positive_and_negative_unwrapped_video_pts() {
    let text = segment("X-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:0", "00:00.000", "00:00.500", "Across wrap");
    for pts in [(WRAP - RATE) as i64, -90_000] {
        let mut assembly = Assembly::new(Clock { pts_90k: pts, presentation_ns: 0 });
        assembly.push(&text, window(1, 2)).unwrap();
        assert!(String::from_utf8(assembly.finish().unwrap()).unwrap().contains("00:00:01.000 --> 00:00:01.500"));
    }
}

#[test]
fn hls_webvtt_cross_segment_full_cue_is_deduplicated_and_not_clipped() {
    let text = segment("X-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:900000", "00:01.000", "00:03.000", "<i>Across boundary</i>");
    let mut assembly = Assembly::new(clock());
    assembly.push(&text, window(0, 2)).unwrap();
    assembly.push(&text, window(2, 4)).unwrap();
    assembly.push("WEBVTT\n\n", window(4, 6)).unwrap();
    assert_eq!(parse("WEBVTT\n").unwrap().1.len(), 0);
    let canonical = String::from_utf8(assembly.finish().unwrap()).unwrap();
    assert_eq!(canonical.matches("-->").count(), 1);
    assert!(canonical.contains("00:00:01.000 --> 00:00:03.000"));
    let same_text_later = segment("X-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:900000", "00:04.000", "00:05.000", "<i>Across boundary</i>");
    let mut assembly = Assembly::new(clock());
    assembly.push(&text, window(0, 2)).unwrap();
    assembly.push(&same_text_later, window(4, 6)).unwrap();
    assert_eq!(String::from_utf8(assembly.finish().unwrap()).unwrap().matches("-->").count(), 2);
}

#[test]
fn hls_webvtt_repeat_preparation_for_seek_has_identical_clock_and_settings() {
    let text = "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:900000\n\ncue-id\n00:01.000 --> 00:03.000 line:0\nTop &amp; <b>bold</b>\n\n";
    let prepare = || {
        let mut assembly = Assembly::new(clock());
        assembly.push(text, window(0, 2)).unwrap();
        assembly.finish().unwrap()
    };
    assert_eq!(prepare(), prepare());
    let canonical = String::from_utf8(prepare()).unwrap();
    assert!(canonical.contains("line:0"));
    assert!(text_ass::script_from_vtt(&canonical).events.contains("An8"));
}

#[test]
fn hls_webvtt_invalid_maps_and_cue_timing_fail_closed() {
    for map in [
        "X-TIMESTAMP-MAP=LOCAL:00:00.000",
        "X-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:8589934592",
        "X-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:-1",
        "X-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:0,MPEGTS:1",
        "X-TIMESTAMP-MAP=LOCAL:00:00.000,LOCAL:00:00.000,MPEGTS:0",
        "X-TIMESTAMP-MAP=LOCAL:00:60.000,MPEGTS:0",
        "X-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:0\nX-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:0",
        "X-TIMESTAMP-MAP:LOCAL:00:00.000,MPEGTS:0",
    ] { assert!(parse(&segment(map, "00:00.000", "00:01.000", "Caption")).is_err(), "{map}"); }
    for (start, end) in [("00:00", "00:01.000"), ("00:00.000", "00:60.000"), ("00:01.000", "00:01.000")] {
        assert!(parse(&segment("", start, end, "Caption")).is_err());
    }
    assert!(parse("not vtt\n\n").is_err());
    assert!(parse("WEBVTT\nUNKNOWN:opaque\n\n").is_err());
    assert!(parse("WEBVTT\n\n00:00.000 --> 00:01.000 bogus:1\nCaption\n\n").is_err());
    assert!(parse("WEBVTT\n\n00:00.000 --> 00:01.000 line:0 line:1\nCaption\n\n").is_err());
    assert!(parse("WEBVTT\n\n00:00.000 --> 00:01.000\nCaption\n00:01.000 --> 00:02.000\nNo separator\n\n").is_err());
    assert!(parse("WEBVTT\n\nSTYLE\n::cue { color:red }\n\n").is_err());
}

#[test]
fn hls_webvtt_ambiguous_wrap_and_excess_work_are_not_silently_truncated() {
    let text = segment("X-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:900000", "00:00.000", "60:00:00.000", "Too long for an unambiguous epoch");
    assert!(Assembly::new(clock()).push(&text, window(0, 1)).is_err());
    let oversized = format!("WEBVTT\n\nNOTE {}", "x".repeat(TEXT_BYTES));
    assert!(parse(&oversized).is_err());
}

#[test]
fn hls_webvtt_playlist_requires_finite_continuous_full_segments() {
    let base = "https://example.invalid/subtitles.m3u8";
    let text = "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\na.vtt\n#EXT-X-ENDLIST\n";
    assert_eq!(segments(base, text).unwrap().len(), 1);
    for invalid in [
        text.replace("#EXT-X-ENDLIST", ""),
        text.replace("#EXTINF", "#EXT-X-DISCONTINUITY\n#EXTINF"),
        text.replace("#EXTINF", "#EXT-X-MAP:URI=\"init.vtt\"\n#EXTINF"),
        text.replace("#EXTINF", "#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n#EXTINF"),
    ] { assert!(segments(base, &invalid).is_err()); }
}

#[test]
fn hls_webvtt_distinct_cue_identifiers_are_not_merged() {
    let base = "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:900000\n\nfirst\n00:01.000 --> 00:03.000\nSame text\n\n";
    let mut assembly = Assembly::new(clock());
    assembly.push(base, window(0, 2)).unwrap();
    assembly.push(base, window(2, 4)).unwrap();
    assembly.push(&base.replace("first", "second"), window(2, 4)).unwrap();
    let canonical = String::from_utf8(assembly.finish().unwrap()).unwrap();
    assert_eq!(canonical.matches("-->").count(), 2);
    assert!(canonical.contains("\nfirst\n"));
    assert!(canonical.contains("\nsecond\n"));
}

#[test]
fn hls_webvtt_timeline_converter_preserves_overlap_without_changing_old_sidecars() {
    let text = "WEBVTT\n\n00:01.000 --> 00:04.000\nFirst\n\n00:02.000 --> 00:05.000\nSecond\n\n";
    let old = text_ass::script_from_vtt(text);
    assert!(old.events.contains("0:00:01.00,0:00:02.00"));
    let mapped = text_ass::script_from_vtt_timeline_bounded(text, 4096, &mut || false).unwrap();
    assert_eq!(mapped.cues, 2);
    assert!(mapped.events.contains("0:00:01.00,0:00:04.00"));
    assert!(mapped.events.contains("0:00:02.00,0:00:05.00"));
}

#[test]
fn hls_webvtt_segment_bound_precedes_resolution_of_the_next_uri() {
    let text = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nfirst.vtt\n#EXTINF:1,\nhttp://example.invalid/forbidden-downgrade.vtt\n#EXT-X-ENDLIST\n";
    let error = external_hls::vod_segments_bounded("https://example.invalid/subs.m3u8", text, 1, TEXT_BYTES).err();
    assert_eq!(error, Some("Too many HLS segments"), "the second URI must not reach resolution");
}

#[test]
fn hls_webvtt_expanded_uri_table_is_bounded_while_materializing() {
    let base = format!("https://example.invalid/{}/subs.m3u8", "x".repeat(4000));
    let text = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\na.vtt\n#EXTINF:1,\nb.vtt\n#EXTINF:1,\nhttp://example.invalid/poison.vtt\n#EXT-X-ENDLIST\n";
    let error = external_hls::vod_segments_bounded(&base, text, SEGMENT_LIMIT, 5000).err();
    assert_eq!(error, Some("HLS resource table exceeds size limit"), "stop retaining URIs before the later poison URI");
}

#[test]
fn hls_webvtt_cancellation_during_cue_validation_does_not_publish_partial_output() {
    let text = segment("X-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:900000",
        "00:00.000", "00:01.000", "Caption");
    let mut calls = 0;
    let mut stop = || { calls += 1; calls >= 2 };
    let mut assembly = Assembly::new(clock());
    assert_eq!(assembly.push_cancellable(&text, window(0, 2), &mut stop),
        Err("HLS subtitle preparation cancelled or timed out"));
    assert!(assembly.cues.is_empty());
    assert_eq!(calls, 2);
}

#[test]
fn hls_webvtt_cancellation_during_serialization_returns_no_sidecar() {
    let text = segment("X-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:900000",
        "00:00.000", "00:01.000", "Caption");
    let mut assembly = Assembly::new(clock());
    assembly.push(&text, window(0, 2)).unwrap();
    let mut calls = 0;
    assert_eq!(assembly.finish_cancellable(&mut || { calls += 1; calls >= 2 }),
        Err("HLS subtitle preparation cancelled or timed out"));
    assert_eq!(calls, 2);
}
