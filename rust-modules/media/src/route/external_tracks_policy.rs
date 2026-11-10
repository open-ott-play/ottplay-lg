//! Pure admission policy for independently inspected, embedded tracks. The caller supplies
//! the current attempt's capabilities and the renderer fact; this module never probes a device.
use super::{TrackAvailability, TrackUnavailable};
use crate::ff::inspect::{InspectedTrack, TrackKind};
use crate::player::text_ass::TextDialect;
use crate::route::{CarriedAudio, DirectPlayMode};
use plx_platform::devcaps::{self, Caps};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalTrackTransport {
    /// The ordinary file demuxer consumes type-relative audio and subtitle ordinals.
    Demux,
    /// The segment reader needs an independently validated audio descriptor;
    /// classify_audio alone cannot authorize it. Subtitle packets remain unsupported.
    Hls,
}

/// Classify a switch, not whether the transport can continue playing its existing default.
/// In particular, segment HLS may play audio while it cannot select an audio ordinal.
pub fn classify_audio(
    track: &InspectedTrack,
    caps: &Caps,
    mode: DirectPlayMode,
    transport: ExternalTrackTransport,
) -> TrackAvailability {
    use TrackAvailability::{Supported, Unsupported};
    if track.kind != TrackKind::Audio || track.ordinal < 0 || track.stream_index < 0 {
        return Unsupported(TrackUnavailable::MissingTrack);
    }
    if transport == ExternalTrackTransport::Hls {
        return Unsupported(TrackUnavailable::Transport);
    }
    let codec = devcaps::audio_codec_id(&track.codec);
    if devcaps::is_pcm_codec(&codec) {
        // Lossless app decoding uses the candidate's facts, not the currently selected
        // track's Load declaration. A non-first PCM track may have another rate or width.
        if crate::route::pcm_format_of(Some(&carried_audio(track)), &codec).is_err() {
            return Unsupported(TrackUnavailable::Codec);
        }
        return if track.decoder_available {
            Supported
        } else {
            Unsupported(TrackUnavailable::Decoder)
        };
    }
    if !crate::route::plan::audio_direct_plays_with(
        caps,
        mode,
        &codec,
        track.channels,
        track.sample_rate,
    ) {
        return Unsupported(TrackUnavailable::Codec);
    }
    if caps.app_decodes_lossy_track(
        &codec,
        track.channels,
        track.sample_rate,
        mode == DirectPlayMode::Forced,
    ) && !track.decoder_available
    {
        return Unsupported(TrackUnavailable::Decoder);
    }
    Supported
}

/// HLS audio selection additionally needs a unique descriptor in its segment
/// snapshot. This does not claim whole-presentation catalog completeness.
pub fn classify_hls_audio(
    inventory: &crate::ff::inspect::TrackInventory,
    track: &InspectedTrack,
    caps: &Caps,
    mode: DirectPlayMode,
) -> TrackAvailability {
    if !inventory.audio.iter().any(|candidate| candidate == track)
        || crate::ff::hls_audio::Selection::new(inventory, track.ordinal).is_err()
    {
        return TrackAvailability::Unsupported(TrackUnavailable::Transport);
    }
    if track.codec == "mp2" {
        return if crate::ff::hls_mp2::supported(track, caps) {
            TrackAvailability::Supported
        } else {
            TrackAvailability::Unsupported(TrackUnavailable::Codec)
        };
    }
    // AAC/AC3 remain compressed. Only the explicitly admitted embedded MP2
    // branch above runs a PCM decoder; other software codecs remain refused.
    if matches!(track.codec.as_str(), "aac" | "ac3")
        && caps.native_audio_track(&track.codec, track.channels, track.sample_rate, mode == DirectPlayMode::Forced) {
        TrackAvailability::Supported
    } else {
        TrackAvailability::Unsupported(TrackUnavailable::Codec)
    }
}

/// `text_renderer_available` is an explicit backend capability supplied by its owner,
/// not inferred from an FFmpeg decoder or a subtitle's presence. Packet text is converted
/// directly to ASS; only image subtitles need an FFmpeg decoder. Decoder presence still
/// cannot guarantee that corrupt packets or a particular track's extradata will decode.
pub fn classify_subtitle(
    track: &InspectedTrack,
    transport: ExternalTrackTransport,
    text_renderer_available: bool,
) -> TrackAvailability {
    use TrackAvailability::{Supported, Unsupported};
    if track.kind != TrackKind::Subtitle || track.ordinal < 0 || track.stream_index < 0 {
        return Unsupported(TrackUnavailable::MissingTrack);
    }
    if transport == ExternalTrackTransport::Hls {
        return Unsupported(TrackUnavailable::Transport);
    }
    if matches!(track.codec.as_str(), "ass" | "ssa")
        || TextDialect::from_codec_name(&track.codec).is_some()
    {
        return if text_renderer_available {
            Supported
        } else {
            Unsupported(TrackUnavailable::Decoder)
        };
    }
    // Do not copy sub_kind's general Bitmap fallback: an arbitrary decoder can emit
    // text or another representation which the indexed PAL8 renderer does not consume.
    if !matches!(
        track.codec.as_str(),
        "hdmv_pgs_subtitle" | "dvd_subtitle" | "dvb_subtitle"
    ) {
        return Unsupported(TrackUnavailable::Codec);
    }
    if track.decoder_available {
        Supported
    } else {
        Unsupported(TrackUnavailable::Decoder)
    }
}

/// Project an admitted audio row before staging its codec and ordinal on the existing
/// native reload path. No server identity, enhancement or immersive flag is invented.
pub fn carried_audio(track: &InspectedTrack) -> CarriedAudio {
    CarriedAudio {
        sid: 0,
        ordinal: track.ordinal,
        codec: track.codec.clone(),
        channels: track.channels,
        can_normalize_loudness: false,
        immersive: false,
        sample_rate: track.sample_rate,
        bit_depth: track.bit_depth,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{ExternalTrackPlan, ExternalTracks};
    use super::*;
    use crate::ff::inspect::TrackInventory;
    use crate::route::PcmFormat;
    use TrackAvailability::{Supported, Unsupported};

    fn track(kind: TrackKind, codec: &str) -> InspectedTrack {
        InspectedTrack {
            kind,
            stream_index: 3,
            ordinal: 1,
            codec: codec.into(),
            language: "eng".into(),
            title: "Track".into(),
            channels: if kind == TrackKind::Audio { 2 } else { 0 },
            sample_rate: if kind == TrackKind::Audio { 48_000 } else { 0 },
            bit_depth: 0,
            decoder_available: true,
            disposition_known: false,
            default: false,
            forced: false,
        }
    }

    fn audio(row: &InspectedTrack, caps: &Caps, mode: DirectPlayMode) -> TrackAvailability {
        classify_audio(row, caps, mode, ExternalTrackTransport::Demux)
    }

    #[test]
    fn native_audio_does_not_require_a_software_decoder() {
        let mut row = track(TrackKind::Audio, "aac");
        row.decoder_available = false;
        assert_eq!(
            audio(&row, &Caps::assumed(), DirectPlayMode::Auto),
            Supported
        );
        row.sample_rate = 96_000;
        assert_eq!(
            audio(&row, &Caps::assumed(), DirectPlayMode::Auto),
            Unsupported(TrackUnavailable::Codec)
        );
        row.codec = "unknown".into();
        row.sample_rate = 48_000;
        row.decoder_available = true;
        assert_eq!(
            audio(&row, &Caps::assumed(), DirectPlayMode::Auto),
            Unsupported(TrackUnavailable::Codec)
        );
    }

    #[test]
    fn non_first_lossless_track_carries_its_own_pcm_shape() {
        let mut row = track(TrackKind::Audio, "flac");
        row.sample_rate = 96_000;
        row.bit_depth = 24;
        assert_eq!(
            audio(&row, &Caps::assumed(), DirectPlayMode::Auto),
            Supported
        );
        let carried = carried_audio(&row);
        assert_eq!(carried.ordinal, 1);
        assert_eq!(carried.sid, 0);
        assert_eq!(carried.codec, "flac");
        assert_eq!(carried.bit_depth, 24);
        assert!(!carried.immersive && !carried.can_normalize_loudness);
        assert_eq!(
            crate::route::pcm_format_of(Some(&carried), "flac"),
            Ok(PcmFormat {
                sample_rate: 48_000,
                bits: 32,
                channels: 2,
                source_rate: 96_000,
            })
        );
        row.decoder_available = false;
        assert_eq!(
            audio(&row, &Caps::assumed(), DirectPlayMode::Auto),
            Unsupported(TrackUnavailable::Decoder)
        );
    }

    #[test]
    fn pcm_requires_real_compatible_facts_and_canonical_codec() {
        for (channels, rate) in [(6, 48_000), (2, 0), (0, 48_000), (2, 37_800)] {
            let mut row = track(TrackKind::Audio, "flac");
            row.channels = channels;
            row.sample_rate = rate;
            assert_eq!(
                audio(&row, &Caps::assumed(), DirectPlayMode::Forced),
                Unsupported(TrackUnavailable::Codec)
            );
        }
        let mut row = track(TrackKind::Audio, "pcm_s24le");
        row.sample_rate = 44_100;
        row.bit_depth = 24;
        assert_eq!(
            audio(&row, &Caps::assumed(), DirectPlayMode::Auto),
            Supported
        );
        assert_eq!(
            crate::route::pcm_format_of(Some(&carried_audio(&row)), "pcm")
                .unwrap()
                .source_rate,
            44_100
        );
    }

    #[test]
    fn lossy_app_decode_checks_decoder_and_preserves_forced_surround_refusal() {
        let caps = Caps {
            app_lossy: devcaps::AppLossy::Measured,
            ..Caps::assumed()
        }
        .with_app_lossy_for("dts");
        let mut row = track(TrackKind::Audio, "dca");
        assert_eq!(audio(&row, &caps, DirectPlayMode::Auto), Supported);
        row.decoder_available = false;
        assert_eq!(
            audio(&row, &caps, DirectPlayMode::Auto),
            Unsupported(TrackUnavailable::Decoder)
        );
        row.decoder_available = true;
        row.channels = 6;
        assert_eq!(audio(&row, &caps, DirectPlayMode::Auto), Supported);
        assert_eq!(
            audio(&row, &caps, DirectPlayMode::Forced),
            Unsupported(TrackUnavailable::Codec)
        );
    }

    #[test]
    fn packet_text_uses_renderer_capability_not_ffmpeg_decoder_presence() {
        for codec in ["subrip", "webvtt", "mov_text", "sami", "ass", "ssa"] {
            let mut row = track(TrackKind::Subtitle, codec);
            row.decoder_available = false;
            assert_eq!(
                classify_subtitle(&row, ExternalTrackTransport::Demux, true),
                Supported
            );
            assert_eq!(
                classify_subtitle(&row, ExternalTrackTransport::Demux, false),
                Unsupported(TrackUnavailable::Decoder)
            );
        }
    }

    #[test]
    fn bitmap_requires_known_representation_and_decoder_but_not_libass() {
        for codec in ["hdmv_pgs_subtitle", "dvd_subtitle", "dvb_subtitle"] {
            let mut row = track(TrackKind::Subtitle, codec);
            assert_eq!(
                classify_subtitle(&row, ExternalTrackTransport::Demux, false),
                Supported
            );
            row.decoder_available = false;
            assert_eq!(
                classify_subtitle(&row, ExternalTrackTransport::Demux, true),
                Unsupported(TrackUnavailable::Decoder)
            );
        }
        for codec in ["dvb_teletext", "unknown"] {
            let row = track(TrackKind::Subtitle, codec);
            assert_eq!(
                classify_subtitle(&row, ExternalTrackTransport::Demux, true),
                Unsupported(TrackUnavailable::Codec)
            );
        }
    }

    #[test]
    fn embedded_hls_mp2_requires_real_decoder_known_shape_and_pcm_capability() {
        let mut row = track(TrackKind::Audio, "mp2");
        row.ordinal = 0;
        let mut caps = Caps::assumed();
        let classify = |row: &InspectedTrack, caps: &Caps, mode| {
            classify_hls_audio(&TrackInventory::from_partial(vec![row.clone()], vec![]), row, caps, mode)
        };
        assert_ne!(classify(&row, &caps, DirectPlayMode::Auto), Supported);
        caps.audio.push_str(",pcm");
        caps.audio_channels.insert("pcm".into(), 2);
        assert!(!caps.audio_has("mp2"), "no native MPEG claim");
        for mode in [DirectPlayMode::Auto, DirectPlayMode::Forced] {
            for rate in [32_000, 44_100, 48_000] {
                row.sample_rate = rate;
                assert_eq!(classify(&row, &caps, mode), Supported);
                assert_ne!(classify_audio(&row, &caps, mode, ExternalTrackTransport::Demux), Supported,
                    "ordinary/Plex codec policy did not change");
            }
            for (channels, rate, decoder) in [(0, 48_000, true), (6, 48_000, true),
                (2, 0, true), (2, 96_000, true), (2, 48_000, false)] {
                let mut invalid = row.clone();
                invalid.channels = channels; invalid.sample_rate = rate; invalid.decoder_available = decoder;
                assert_ne!(classify(&invalid, &caps, mode), Supported);
            }
        }
        caps.audio_channels.insert("pcm".into(), 0);
        assert_ne!(classify(&row, &caps, DirectPlayMode::Forced), Supported,
            "Force does not invent a PCM sink");
    }

    #[test]
    fn hls_audio_requires_native_aac_not_software_pcm_admission() {
        let mut a = track(TrackKind::Audio, "aac");
        a.ordinal = 0;
        a.stream_index = 1;
        let inventory = TrackInventory::from_partial(vec![a.clone()], vec![]);
        let measured = Caps { app_lossy: devcaps::AppLossy::Measured, ..Caps::assumed() };
        assert_eq!(classify_hls_audio(&inventory, &a, &measured, DirectPlayMode::Auto), Supported);
        // AAC is not an APP_DECODED row; disable the real native paths explicitly.
        let mut no_native = measured.clone();
        no_native.audio_channels.insert("aac".into(), 0);
        no_native.native_ceiling.insert("aac".into(), 0);
        for mode in [DirectPlayMode::Auto, DirectPlayMode::Forced] {
            let forced = mode == DirectPlayMode::Forced;
            assert!(!no_native.native_audio_track("aac", 2, 48_000, forced));
            assert!(!no_native.app_decodes_lossy_track("aac", 2, 48_000, forced));
            assert_eq!(classify_hls_audio(&inventory, &a, &no_native, mode), Unsupported(TrackUnavailable::Codec));
        }
        // DTS is a genuine software-only path, but this HLS reader emits AAC/ADTS.
        let software = measured.with_app_lossy_for("dts");
        let mut dts = a.clone();
        dts.codec = "dts".into();
        let software_inventory = TrackInventory::from_partial(vec![dts.clone()], vec![]);
        for mode in [DirectPlayMode::Auto, DirectPlayMode::Forced] {
            let forced = mode == DirectPlayMode::Forced;
            assert!(software.app_decodes_lossy_track("dts", 2, 48_000, forced));
            assert!(!software.native_audio_track("dts", 2, 48_000, forced));
            assert_eq!(audio(&dts, &software, mode), Supported);
            assert_eq!(classify_hls_audio(&software_inventory, &dts, &software, mode),
                Unsupported(TrackUnavailable::Transport));
        }
    }

    #[test]
    fn hls_ac3_uses_its_own_native_caps_and_never_aac_or_software_fallback() {
        let mut row=track(TrackKind::Audio,"ac3"); row.ordinal=0; row.stream_index=1;
        row.sample_rate=48_000; row.channels=6; row.decoder_available=false;
        let classify=|row: &InspectedTrack,caps: &Caps,mode| {
            classify_hls_audio(&TrackInventory::from_partial(vec![row.clone()],vec![]),row,caps,mode)
        };
        let caps=Caps::assumed();
        for mode in [DirectPlayMode::Auto,DirectPlayMode::Forced] {
            assert_eq!(classify(&row,&caps,mode),Supported);
            let mut no_ac3=caps.clone();
            no_ac3.audio_channels.insert("ac3".into(),0);
            no_ac3.native_ceiling.insert("ac3".into(),0);
            assert!(no_ac3.native_audio_track("aac",2,48_000,mode==DirectPlayMode::Forced));
            assert_eq!(classify(&row,&no_ac3,mode),Unsupported(TrackUnavailable::Codec));
            let mut no_aac=caps.clone();
            no_aac.audio_channels.insert("aac".into(),0);
            no_aac.native_ceiling.insert("aac".into(),0);
            assert_eq!(classify(&row,&no_aac,mode),Supported);
            for (channels,rate) in [(0,48_000),(7,48_000),(6,0),(6,24_000)] {
                let mut invalid=row.clone(); invalid.channels=channels; invalid.sample_rate=rate;
                assert_eq!(classify(&invalid,&caps,mode),Unsupported(TrackUnavailable::Transport));
            }
            let mut eac3=row.clone(); eac3.codec="eac3".into();
            assert_eq!(classify(&eac3,&caps,mode),Unsupported(TrackUnavailable::Transport));
        }
    }

    #[test]
    fn hls_keeps_default_visible_without_promising_switches_and_off_stays_available() {
        let mut a = track(TrackKind::Audio, "aac");
        a.ordinal = 0;
        a.stream_index = 1;
        let mut sub = track(TrackKind::Subtitle, "subrip");
        sub.ordinal = 0;
        sub.stream_index = 2;
        let a_support = classify_audio(
            &a,
            &Caps::assumed(),
            DirectPlayMode::Auto,
            ExternalTrackTransport::Hls,
        );
        let s_support = classify_subtitle(&sub, ExternalTrackTransport::Hls, true);
        assert_eq!(a_support, Unsupported(TrackUnavailable::Transport));
        assert_eq!(s_support, Unsupported(TrackUnavailable::Transport));
        let mut catalog = ExternalTracks::new(
            TrackInventory::from_partial(vec![a], vec![sub]),
            vec![a_support],
            vec![s_support],
            Some(0),
        )
        .unwrap();
        assert_eq!(catalog.selected_audio(), Some(0));
        assert!(catalog.choice(TrackKind::Audio, Some(0)).is_err());
        assert!(catalog.choice(TrackKind::Subtitle, Some(0)).is_err());
        let off = catalog.choice(TrackKind::Subtitle, None).unwrap();
        assert_eq!(catalog.commit(&off), Ok(ExternalTrackPlan::Unchanged));
    }

    #[test]
    fn wrong_track_kind_or_invalid_identity_is_not_admitted() {
        let a = track(TrackKind::Audio, "aac");
        assert_eq!(
            classify_subtitle(&a, ExternalTrackTransport::Demux, true),
            Unsupported(TrackUnavailable::MissingTrack)
        );
        let mut sub = track(TrackKind::Subtitle, "subrip");
        assert_eq!(
            audio(&sub, &Caps::assumed(), DirectPlayMode::Auto),
            Unsupported(TrackUnavailable::MissingTrack)
        );
        sub.ordinal = -1;
        assert_eq!(
            classify_subtitle(&sub, ExternalTrackTransport::Demux, true),
            Unsupported(TrackUnavailable::MissingTrack)
        );
    }
}
