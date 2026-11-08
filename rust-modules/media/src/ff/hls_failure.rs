//! Closed diagnostics for the existing terminal HLS failure path.
//!
//! These exact literals come from the HLS producers. Unknown text stays in the
//! original HlsFailed class; it is never copied into a diagnostic field.
use plx_telemetry::telemetry::classes::DemuxExitClass;

pub(super) fn classify(reason: &str) -> DemuxExitClass {
    use DemuxExitClass::*;
    match reason {
        "HLS transport unavailable"
        | "Could not fetch HLS playlist"
        | "Initial HLS playlist request timed out"
        | "Selected HLS playlist request timed out"
        | "HLS playlist request timed out"
        | "HLS playlist body transfer timed out"
        | "HLS playlist server returned HTTP 401"
        | "HLS playlist server returned HTTP 403"
        | "HLS playlist server returned HTTP 404"
        | "HLS playlist server returned HTTP 429"
        | "HLS playlist server returned a server error"
        | "HLS playlist server returned an HTTP error"
        | "HLS playlist network transport failed"
        | "HLS playlist local transport failed"
        | "HLS playlist local transport setup failed"
        | "HLS playlist server refused the requested byte range"
        | "HLS playlist transfer failed"
        | "Truncated HLS response" => HlsPlaylistFetchFailed,

        "Invalid HLS resource address"
        | "HLS resource would leave HTTPS"
        | "Invalid HLS resource"
        | "Malformed HLS attribute"
        | "Malformed HLS quoted attribute"
        | "Malformed HLS attribute separator"
        | "Duplicate HLS attribute"
        | "Invalid HLS playlist header"
        | "Mixed HLS playlist types"
        | "Separate HLS rendition groups are not supported"
        | "Missing HLS variant bandwidth"
        | "Invalid HLS segment duration"
        | "Duplicate HLS target duration"
        | "Invalid HLS target duration"
        | "Invalid HLS media sequence"
        | "Encrypted HLS is not supported"
        | "This HLS container or encryption feature is not supported"
        | "Separate HLS audio is not supported"
        | "Duplicate HLS header"
        | "Unsupported HLS feature"
        | "HLS URI has no media declaration"
        | "Truncated HLS playlist"
        | "Missing HLS target duration"
        | "HLS playlist has no media"
        | "HLS playlist is not UTF-8"
        | "HLS media address became a master playlist" => HlsPlaylistInvalid,

        "Seeking requires a finite HLS playlist"
        | "HLS live window advanced past playback"
        | "HLS segment sequence changed"
        | "HLS timeline overflow"
        | "HLS sequence overflow"
        | "HLS playlist stopped advancing" => HlsPlaylistWindow,

        "HTTP request failed"
        | "HTTPS request failed"
        | "HTTPS reservation failed"
        | "HTTP redirect refused"
        | "insecure credential transport refused"
        | "invalid PMS host"
        | "invalid HLS request path"
        | "HLS segment was not produced in time" => HlsTransportOpenFailed,

        "segment AVIO buffer allocation failed"
        | "segment AVIO allocation failed"
        | "segment format allocation failed"
        | "segment open/probe failed"
        | "segment stream discovery failed"
        | "segment packet allocation failed"
        | "Invalid media stream table"
        | "Invalid media stream entry"
        | "Invalid media stream codec"
        | "Invalid media track identity" => HlsSegmentProbeFailed,

        "HLS segment has no video stream"
        | "HLS segment video is not H.264"
        | "HLS segment has invalid video dimensions"
        | "HLS segment audio is not AAC" => HlsSegmentCodec,

        "H.264 packet is not Annex-B"
        | "bytes precede the first Annex-B start code"
        | "empty Annex-B NAL"
        | "IDR has no in-band or cached SPS"
        | "IDR has no in-band or cached PPS" => HlsVideoFraming,

        "HLS segment does not begin with an IDR" => HlsFirstIdr,
        "segment body ended before Content-Length" => HlsShortBody,

        "HLS segment audio inventory is incomplete"
        | "HLS audio inventory changed; reopen the source"
        | "Selected HLS audio is missing"
        | "HLS audio identity is ambiguous"
        | "Selected HLS audio changed within the segment" => HlsAudioIdentity,

        "Selected HLS audio produced no packets" => HlsAudioEmpty,
        "HLS video packet has no timestamp" => HlsVideoTimestamp,
        "AAC timestamp hole has no duration anchor"
        | "AAC timestamps move backwards" => HlsAudioTimestamp,
        "AAC packet is neither ADTS nor reframable" => HlsAudioFraming,
        "HLS segment produced no video access units" => HlsNoVideoAu,

        "HLS playlist exceeds size limit"
        | "HLS playlist line exceeds limit"
        | "Too many HLS variants"
        | "Too many HLS segments"
        | "HLS segment exceeds size limit"
        | "HLS access units exceed size limit" => HlsSizeLimit,

        "HLS transport stalled"
        | "segment body transport failed" => HlsTransportFailed,
        _ => HlsFailed,
    }
}

#[cfg(test)]
#[path = "hls_failure_tests.rs"]
mod tests;
