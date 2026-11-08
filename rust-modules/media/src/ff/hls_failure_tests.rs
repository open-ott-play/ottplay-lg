use super::classify;
use plx_telemetry::telemetry::classes::DemuxExitClass::*;

#[test]
fn a_probed_source_still_distinguishes_segment_discovery_codec_and_empty_output() {
    assert_eq!(classify("segment open/probe failed"), HlsSegmentProbeFailed);
    assert_eq!(classify("segment stream discovery failed"), HlsSegmentProbeFailed);
    assert_eq!(classify("HLS segment has no video stream"), HlsSegmentCodec);
    assert_eq!(classify("HLS segment audio is not AAC"), HlsSegmentCodec);
    assert_eq!(classify("HLS segment produced no video access units"), HlsNoVideoAu);
}

#[test]
fn video_timestamp_framing_and_first_idr_failures_do_not_collapse() {
    assert_eq!(classify("HLS video packet has no timestamp"), HlsVideoTimestamp);
    assert_eq!(classify("H.264 packet is not Annex-B"), HlsVideoFraming);
    assert_eq!(classify("IDR has no in-band or cached SPS"), HlsVideoFraming);
    assert_eq!(classify("IDR has no in-band or cached PPS"), HlsVideoFraming);
    assert_eq!(classify("HLS segment does not begin with an IDR"), HlsFirstIdr);
}

#[test]
fn audio_identity_empty_output_timing_and_framing_have_distinct_causes() {
    assert_eq!(classify("HLS audio inventory changed; reopen the source"), HlsAudioIdentity);
    assert_eq!(classify("Selected HLS audio changed within the segment"), HlsAudioIdentity);
    assert_eq!(classify("Selected HLS audio produced no packets"), HlsAudioEmpty);
    assert_eq!(classify("AAC timestamp hole has no duration anchor"), HlsAudioTimestamp);
    assert_eq!(classify("AAC timestamps move backwards"), HlsAudioTimestamp);
    assert_eq!(classify("AAC packet is neither ADTS nor reframable"), HlsAudioFraming);
}

#[test]
fn playlist_open_body_and_content_length_failures_remain_separate() {
    assert_eq!(classify("Could not fetch HLS playlist"), HlsPlaylistFetchFailed);
    assert_eq!(classify("Truncated HLS response"), HlsPlaylistFetchFailed);
    assert_eq!(classify("Truncated HLS playlist"), HlsPlaylistInvalid);
    assert_eq!(classify("HLS live window advanced past playback"), HlsPlaylistWindow);
    assert_eq!(classify("HLS playlist stopped advancing"), HlsPlaylistWindow);
    assert_eq!(classify("HTTP request failed"), HlsTransportOpenFailed);
    assert_eq!(classify("HTTPS request failed"), HlsTransportOpenFailed);
    assert_eq!(classify("HLS segment was not produced in time"), HlsTransportOpenFailed);
    assert_eq!(classify("HLS transport stalled"), HlsTransportFailed);
    assert_eq!(classify("segment body transport failed"), HlsTransportFailed);
    assert_eq!(classify("segment body ended before Content-Length"), HlsShortBody);
    assert_eq!(classify("HLS segment exceeds size limit"), HlsSizeLimit);
    assert_eq!(classify("HLS access units exceed size limit"), HlsSizeLimit);
}

#[test]
fn typed_playlist_acquisition_keeps_its_existing_structured_failure_class() {
    for reason in [
        "Initial HLS playlist request timed out", "Selected HLS playlist request timed out",
        "HLS playlist request timed out", "HLS playlist body transfer timed out",
        "HLS playlist server returned HTTP 401", "HLS playlist server returned HTTP 403",
        "HLS playlist server returned HTTP 404", "HLS playlist server returned HTTP 429",
        "HLS playlist server returned a server error", "HLS playlist server returned an HTTP error",
        "HLS playlist network transport failed", "HLS playlist local transport failed",
        "HLS playlist local transport setup failed", "HLS playlist server refused the requested byte range",
    ] {
        assert_eq!(classify(reason), HlsPlaylistFetchFailed);
        assert_eq!(classify(&format!("{reason}: private source")), HlsFailed);
    }
    assert_eq!(classify("HLS request cancelled"), HlsFailed, "cancellation policy is unchanged");
}

#[test]
fn unknown_and_near_match_text_keep_the_original_bounded_hls_failure() {
    for reason in [
        "",
        "new HLS failure",
        "HLS segment does not begin with an IDR ",
        " HLS segment does not begin with an IDR",
        "hls segment does not begin with an IDR",
        "HLS segment does not begin with an IDR: title=Private example",
        "HTTPS request failed: https://user:secret@example.invalid/movie?token=private",
        "segment body ended before Content-Length\nprivate title",
        "HLS rendition descriptor changed within the segment",
        "HLS request cancelled",
    ] {
        assert_eq!(classify(reason), HlsFailed);
    }
    // The caller accepts a borrow, not only a producer's static literal.
    let untrusted = String::from("https://example.invalid/HLS segment exceeds size limit");
    assert_eq!(classify(&untrusted), HlsFailed);
}

#[test]
fn shared_transport_reasons_do_not_claim_a_playlist_or_segment_origin() {
    // Both hls_fetch_text and segment acquisition propagate these same errors.
    // The reason alone cannot tell which resource was being opened or read.
    assert_eq!(classify("HTTPS request failed").code(), "hls_transport_open_failed");
    assert_eq!(classify("HLS transport stalled").code(), "hls_transport_failed");
}
