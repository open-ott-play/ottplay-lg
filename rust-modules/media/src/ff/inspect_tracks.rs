//! Owned embedded-track facts from the same inspected container as the video.
//! No Plex identities, route policy, decoder choice or active-player mutations live here.
use super::super::{
    avcodec_find_decoder, avcodec_get_name, stream_codecpar, stream_index, stream_language,
    stream_name, AVFormatContext, AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_SUBTITLE,
};

const STREAM_LIMIT: usize = 256;
const TITLE_BYTES: usize = 1024;
const LANGUAGE_BYTES: usize = 128;
const CODEC_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    Audio,
    Subtitle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectedTrack {
    pub kind: TrackKind,
    /// AVStream.index: identity within this one container, never a server stream ID.
    pub stream_index: i32,
    /// Dense zero-based position among streams of this kind in container order.
    /// This is the ordinal consumed by native audio/subtitle selection.
    pub ordinal: i32,
    pub codec: String,
    /// The container's language tag, without guessing a display name or locale.
    pub language: String,
    pub title: String,
    pub channels: i64,
    pub sample_rate: i64,
    pub bit_depth: i64,
    /// The bundled FFmpeg exposes a decoder; this does not claim native passthrough support.
    pub decoder_available: bool,
    /// The current checked FFmpeg ABI exposes no disposition accessor. These flags
    /// are meaningful only when disposition_known is true; never infer them from a title.
    pub disposition_known: bool,
    pub default: bool,
    pub forced: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackInventory {
    // Only a successful complete collect may establish this fact. Empty vectors
    // alone say nothing: fallback metadata and an inspected video-only file differ.
    complete: bool,
    pub audio: Vec<InspectedTrack>,
    pub subtitles: Vec<InspectedTrack>,
}

impl TrackInventory {
    /// True only after all entries of a bounded, probed container table were read.
    /// This does not claim that a decoder observed a selected track or that every
    /// variant in a segmented presentation has the same streams.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Known rows without evidence of a complete container inventory. Manual
    /// menus may use these rows; a complete remote catalog must not infer more.
    pub fn from_partial(audio: Vec<InspectedTrack>, subtitles: Vec<InspectedTrack>) -> Self {
        Self { complete: false, audio, subtitles }
    }

    /// A probe of one segment cannot establish a whole presentation's inventory.
    pub(crate) fn mark_incomplete(&mut self) {
        self.complete = false;
    }

    /// Synthetic complete table for cross-crate tests only. Production completeness
    /// still comes exclusively from a successful full inspector collection.
    #[cfg(any(test, feature = "test-support"))]
    pub fn complete_for_test(rows: Vec<InspectedTrack>) -> Result<Self, &'static str> {
        let mut inventory=Self::default();
        for row in rows { inventory.push(row)?; }
        inventory.complete=true;
        Ok(inventory)
    }

    fn push(&mut self, mut track: InspectedTrack) -> Result<(), &'static str> {
        if track.stream_index < 0
            || track.stream_index as usize >= STREAM_LIMIT
            || self.audio.len() + self.subtitles.len() >= STREAM_LIMIT
            || self
                .audio
                .iter()
                .chain(&self.subtitles)
                .any(|t| t.stream_index == track.stream_index)
        {
            return Err("Invalid media track identity");
        }
        track.codec = bounded(track.codec, CODEC_BYTES);
        track.language = bounded(track.language, LANGUAGE_BYTES);
        track.title = bounded(track.title, TITLE_BYTES);
        let target = match track.kind {
            TrackKind::Audio => &mut self.audio,
            TrackKind::Subtitle => &mut self.subtitles,
        };
        track.ordinal = target.len() as i32;
        target.push(track);
        Ok(())
    }
}

fn bounded(mut value: String, bytes: usize) -> String {
    if value.len() > bytes {
        let mut end = bytes;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
    value
}

/// The caller owns a live, already-probed AVFormatContext. All reads use the
/// existing checked ABI helpers and all returned strings outlive that context.
pub(super) unsafe fn collect(
    context: *mut AVFormatContext,
) -> Result<TrackInventory, &'static str> {
    if context.is_null()
        || (*context).nb_streams as usize > STREAM_LIMIT
        || (*context).streams.is_null()
    {
        return Err("Invalid media stream table");
    }
    let mut inventory = TrackInventory::default();
    for i in 0..(*context).nb_streams as usize {
        let stream = *(*context).streams.add(i);
        if stream.is_null() {
            return Err("Invalid media stream entry");
        }
        let codec = stream_codecpar(stream);
        if codec.is_null() {
            return Err("Invalid media stream codec");
        }
        let kind = match (*codec).codec_type {
            AVMEDIA_TYPE_AUDIO => TrackKind::Audio,
            AVMEDIA_TYPE_SUBTITLE => TrackKind::Subtitle,
            _ => continue,
        };
        // FFmpeg packet stream_index addresses this same array. Refuse a malformed
        // table rather than silently attach a menu identity to a different stream.
        let index = stream_index(stream);
        if index != i as i32 {
            return Err("Invalid media track identity");
        }
        let codec_name = avcodec_get_name((*codec).codec_id);
        let codec_name = if codec_name.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(codec_name)
                .to_string_lossy()
                .into_owned()
        };
        let audio = kind == TrackKind::Audio;
        inventory.push(InspectedTrack {
            kind,
            stream_index: index,
            ordinal: 0,
            codec: codec_name,
            language: stream_language(stream).unwrap_or_default(),
            title: stream_name(stream),
            channels: if audio {
                i64::from((*codec).ch_layout.nb_channels).max(0)
            } else {
                0
            },
            sample_rate: if audio {
                i64::from((*codec).sample_rate).max(0)
            } else {
                0
            },
            bit_depth: if audio {
                i64::from(
                    (*codec)
                        .bits_per_raw_sample
                        .max((*codec).bits_per_coded_sample),
                )
                .max(0)
            } else {
                0
            },
            decoder_available: !avcodec_find_decoder((*codec).codec_id).is_null(),
            disposition_known: false,
            default: false,
            forced: false,
        })?;
    }
    inventory.complete = true;
    Ok(inventory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ff::AVStream;

    #[test]
    fn complete_fixture_uses_the_same_identity_and_ordinal_validation() {
        let inventory = TrackInventory::complete_for_test(vec![
            track(TrackKind::Subtitle, 1, "First"),
            track(TrackKind::Audio, 3, "Audio"),
            track(TrackKind::Subtitle, 5, "Second"),
        ]).unwrap();
        assert!(inventory.is_complete());
        assert_eq!(inventory.audio[0].ordinal, 0);
        assert_eq!(inventory.subtitles[1].ordinal, 1);
        assert!(TrackInventory::complete_for_test(vec![
            track(TrackKind::Audio, 1, "Audio"),
            track(TrackKind::Subtitle, 1, "Duplicate"),
        ]).is_err());
        assert!(TrackInventory::complete_for_test(Vec::new()).unwrap().is_complete());
    }

    fn track(kind: TrackKind, index: i32, title: &str) -> InspectedTrack {
        InspectedTrack {
            kind,
            stream_index: index,
            ordinal: -1,
            codec: if kind == TrackKind::Audio {
                "aac"
            } else {
                "subrip"
            }
            .into(),
            language: "eng".into(),
            title: title.into(),
            channels: if kind == TrackKind::Audio { 2 } else { 0 },
            sample_rate: if kind == TrackKind::Audio { 48000 } else { 0 },
            bit_depth: 0,
            decoder_available: true,
            disposition_known: false,
            default: false,
            forced: false,
        }
    }

    #[test]
    fn interleaved_streams_keep_container_identity_and_dense_type_ordinals() {
        let mut inventory = TrackInventory::default();
        for t in [
            track(TrackKind::Audio, 1, "English"),
            track(TrackKind::Subtitle, 2, ""),
            track(TrackKind::Audio, 3, "Spanish"),
            track(TrackKind::Subtitle, 4, "Full captions"),
        ] {
            inventory.push(t).unwrap();
        }
        assert_eq!(
            inventory
                .audio
                .iter()
                .map(|t| (t.stream_index, t.ordinal))
                .collect::<Vec<_>>(),
            [(1, 0), (3, 1)]
        );
        assert_eq!(
            inventory
                .subtitles
                .iter()
                .map(|t| (t.stream_index, t.ordinal))
                .collect::<Vec<_>>(),
            [(2, 0), (4, 1)]
        );
        assert_eq!(inventory.subtitles[0].title, "");
        assert_eq!(inventory.subtitles[1].title, "Full captions");
        assert_eq!(inventory.audio[1].sample_rate, 48000);
    }

    #[test]
    fn duplicate_or_invalid_stream_identity_does_not_partially_change_inventory() {
        let mut inventory = TrackInventory::default();
        inventory.push(track(TrackKind::Audio, 1, "first")).unwrap();
        let before = inventory.clone();
        for index in [-1, 1, STREAM_LIMIT as i32] {
            assert!(inventory
                .push(track(TrackKind::Subtitle, index, "wrong"))
                .is_err());
            assert_eq!(inventory, before);
        }
    }

    #[test]
    fn names_are_bounded_owned_utf8_and_unknown_dispositions_are_not_guessed() {
        let mut inventory = TrackInventory::default();
        let title = "ж".repeat(TITLE_BYTES);
        let mut value = track(TrackKind::Subtitle, 2, &title);
        value.language = "日".repeat(LANGUAGE_BYTES);
        value.codec = "x".repeat(CODEC_BYTES + 1);
        inventory.push(value).unwrap();
        let row = &inventory.subtitles[0];
        assert_eq!(row.title.len(), TITLE_BYTES);
        assert!(row.language.len() <= LANGUAGE_BYTES);
        assert_eq!(row.codec.len(), CODEC_BYTES);
        assert_eq!(title.len(), 2 * TITLE_BYTES);
        inventory
            .push(track(
                TrackKind::Subtitle,
                3,
                "Original / Default / Forced / SDH",
            ))
            .unwrap();
        let named = &inventory.subtitles[1];
        assert!(!named.disposition_known && !named.default && !named.forced);
    }

    #[test]
    fn default_and_partial_rows_do_not_claim_complete_inspection() {
        let empty = TrackInventory::default();
        assert!(!empty.is_complete());
        let mut partial = TrackInventory::from_partial(
            vec![track(TrackKind::Audio, 1, "Known row")], Vec::new());
        assert!(!partial.is_complete());
        partial.push(track(TrackKind::Subtitle, 2, "Another known row")).unwrap();
        assert!(!partial.is_complete(), "adding rows is not a complete probe");
        assert!(!partial.clone().is_complete());
    }

    // This invokes the real collector's table validation/completion edge, without
    // a codec accessor or FFmpeg call: zero entries means no pointers are read.
    fn empty_context(table: &mut [*mut AVStream; 1]) -> AVFormatContext {
        AVFormatContext {
            av_class: std::ptr::null(),
            iformat: std::ptr::null_mut(),
            oformat: std::ptr::null_mut(),
            priv_data: std::ptr::null_mut(),
            pb: std::ptr::null_mut(),
            ctx_flags: 0,
            nb_streams: 0,
            streams: table.as_mut_ptr(),
        }
    }

    #[test]
    fn completed_empty_table_is_distinct_from_unavailable_and_can_be_downgraded() {
        let mut table = [std::ptr::null_mut::<AVStream>()];
        let mut context = empty_context(&mut table);
        let complete = unsafe { collect(&mut context) }.unwrap();
        assert!(complete.is_complete());
        assert!(complete.audio.is_empty() && complete.subtitles.is_empty());
        assert_ne!(complete, TrackInventory::default());
        let mut segment = complete.clone();
        assert!(segment.is_complete());
        segment.mark_incomplete();
        assert!(!segment.is_complete());
        assert_eq!(segment.audio, complete.audio);
        assert_eq!(segment.subtitles, complete.subtitles);
        assert!(complete.is_complete(), "downgrade cannot relabel an independent snapshot");
    }

    #[test]
    fn invalid_or_oversized_tables_cannot_publish_complete_inventory() {
        let mut table = [std::ptr::null_mut::<AVStream>()];
        let mut context = empty_context(&mut table);
        context.streams = std::ptr::null_mut();
        assert_eq!(unsafe { collect(&mut context) }, Err("Invalid media stream table"));
        context.streams = table.as_mut_ptr();
        context.nb_streams = (STREAM_LIMIT + 1) as u32;
        assert_eq!(unsafe { collect(&mut context) }, Err("Invalid media stream table"));
        context.nb_streams = 1;
        assert_eq!(unsafe { collect(&mut context) }, Err("Invalid media stream entry"));
    }

    #[test]
    fn missing_context_is_refused_without_reading_memory() {
        assert_eq!(
            unsafe { collect(std::ptr::null_mut()) },
            Err("Invalid media stream table")
        );
    }

    #[test]
    #[ignore = "requires bundled host FFmpeg and OTTPLAY_EXTERNAL_TRACKS_FIXTURES"]
    fn bundled_probe_enumerates_interleaved_tracks_and_bitmap_decoder() {
        let _serial = plx_base::testlock::serial();
        let _ffmpeg = crate::ff::image_subtitle_tests::bind_host_ffmpeg();
        let directory = std::path::PathBuf::from(
            std::env::var_os("OTTPLAY_EXTERNAL_TRACKS_FIXTURES")
                .expect("generate fixtures with tests/fixtures/make_external_tracks_fixture.py"),
        )
        .canonicalize()
        .unwrap();
        let probe = |name: &str| {
            super::super::source(
                &crate::ff::ExternalSource::Local(directory.join(name)),
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )
            .unwrap()
        };
        let text = probe("external-tracks-text.mkv");
        assert_eq!(text.audio_codec, "aac");
        assert_eq!(text.audio_channels, 1);
        assert_eq!(text.audio_sample_rate, 48000);
        assert!(text.tracks.is_complete());
        assert_eq!(text.tracks.audio.len(), 2);
        assert_eq!(text.tracks.subtitles.len(), 2);
        let audios: Vec<_> = text
            .tracks
            .audio
            .iter()
            .map(|t| {
                (
                    t.stream_index,
                    t.ordinal,
                    t.codec.as_str(),
                    t.language.as_str(),
                    t.title.as_str(),
                    t.channels,
                    t.sample_rate,
                )
            })
            .collect();
        assert_eq!(
            audios,
            [
                (1, 0, "aac", "eng", "English 220 Hz", 1, 48000),
                (3, 1, "aac", "spa", "Spanish 880 Hz", 1, 48000),
            ]
        );
        let subtitles: Vec<_> = text
            .tracks
            .subtitles
            .iter()
            .map(|t| {
                (
                    t.stream_index,
                    t.ordinal,
                    t.codec.as_str(),
                    t.language.as_str(),
                    t.title.as_str(),
                )
            })
            .collect();
        assert_eq!(
            subtitles,
            [
                (2, 0, "subrip", "eng", "English SDH"),
                (4, 1, "subrip", "rus", "Russian forced"),
            ]
        );
        for track in text.tracks.audio.iter().chain(&text.tracks.subtitles) {
            assert!(
                !track.disposition_known && !track.default && !track.forced,
                "container dispositions are unknown without a checked accessor"
            );
        }
        let bitmap = probe("external-tracks-bitmap.mkv");
        assert!(bitmap.tracks.is_complete());
        assert_eq!(bitmap.tracks.subtitles.len(), 1);
        let subtitle = &bitmap.tracks.subtitles[0];
        assert_eq!(
            (
                subtitle.stream_index,
                subtitle.ordinal,
                subtitle.codec.as_str()
            ),
            (2, 0, "hdmv_pgs_subtitle")
        );
        assert!(
            subtitle.decoder_available,
            "shipped FFmpeg must expose its PGS decoder"
        );
        assert_eq!(subtitle.title, "English PGS");
        // Both probes have already closed their AVFormatContext: all returned
        // strings above are owned values, not pointers into freed dictionaries.
    }
}
