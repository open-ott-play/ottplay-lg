//! Store-level HLS coverage; route revision/worker admission is tested by its owner.
use super::*;
use super::super::ass::{self, Origin};
use crate::ff::hls_presentation::HlsSubtitleId;

struct Cleanup;
impl Drop for Cleanup {
    fn drop(&mut self) {
        reset();
        crate::route::reset_player_control_for_test(&crate::route::PlaybackSession::IDLE);
    }
}

fn identity_for(source: crate::ff::ExternalSource) -> PlaybackIdentity {
    let info = crate::ff::inspect::MediaInfo {
        tracks: Default::default(), resolved_source: None, hls_playback: None,
        video_codec: "h264".into(), audio_codec: "aac".into(), width: 1280, height: 720,
        fps: 24.0, duration_ns: 60_000_000_000, audio_channels: 2,
        audio_sample_rate: 48000, audio_bit_depth: 16, dovi: Default::default(),
    };
    let mut ps = crate::route::PlaybackSession::IDLE;
    crate::route::reset_player_control_for_test(&ps);
    crate::route::request_external_source(&mut ps, &info, source,
        "Synthetic", "Fixture", plx_platform::devcaps::assumed_shared()).unwrap();
    crate::route::reset_player_control_for_test(&ps);
    crate::route::playback_identity(&ps).unwrap()
}

fn identity() -> PlaybackIdentity {
    identity_for(crate::ff::ExternalSource::Hls("https://example.invalid/movie.m3u8".into()))
}

fn ready(playback: &PlaybackIdentity, id: HlsSubtitleId, text: &str, style: TextStyle) -> PreparedHls {
    prepare_hls(&OffFrame::for_test(), playback.clone(), id, text.as_bytes().to_vec(), style, &mut || false).unwrap()
}
fn script(source: &ass::Source) -> &[u8] {
    let ass::Content::Script { bytes, .. } = &source.content else { panic!("whole script expected") };
    bytes
}
const CAPTION: &str = "WEBVTT\n\n00:05.000 --> 00:07.000 line:0\nMapped <b>caption</b> tail\n";

#[test]
fn hls_preparation_and_wrong_owner_install_preserve_the_current_source() {
    let _serial = plx_base::testlock::serial();
    let _cleanup = Cleanup;
    reset();
    let first = identity();
    let reopened = identity();
    let style = TextStyle::default();
    let id = HlsSubtitleId(1);
    assert!(install_hls(&first, id, ready(&first, id, CAPTION, style)));
    let held = ass_source(false, style).unwrap();
    let generation = state().generation;
    let candidate = ready(&first, HlsSubtitleId(2), CAPTION, style);
    assert_eq!(state().generation, generation, "preparation has no selection side effect");
    assert_eq!(selected_hls(&first), Some(id));
    assert!(Arc::ptr_eq(&held, &ass_source(false, style).unwrap()));
    assert!(!install_hls(&first, id, candidate), "prepared rendition cannot be retargeted");
    assert!(!install_hls(&reopened, id, ready(&first, id, CAPTION, style)),
        "reopening the same URL does not reuse playback identity");
    assert!(matches!(prepare_hls(&OffFrame::for_test(), first.clone(), id,
        b"not a WebVTT document".to_vec(), style, &mut || false), Err(HlsPrepareError::InvalidContent)));
    let mut oversized = b"WEBVTT\n\n".to_vec();
    oversized.resize(owned_byte_limit() + 1, b' ');
    assert!(matches!(prepare_hls(&OffFrame::for_test(), first.clone(), id,
        oversized, style, &mut || false), Err(HlsPrepareError::InvalidContent)));
    assert_eq!(state().generation, generation);
    assert!(Arc::ptr_eq(&held, &ass_source(false, style).unwrap()));
    assert_eq!(selected_hls(&reopened), None);
}

#[test]
fn hls_install_uses_the_shared_text_script_and_allows_a_complete_empty_track() {
    let _serial = plx_base::testlock::serial();
    let _cleanup = Cleanup;
    reset();
    let playback = identity();
    let id = HlsSubtitleId(1);
    let style = TextStyle { size: plx_plex::plex::session::SubtitleSize::Large,
        ..TextStyle::default() };
    assert!(install_hls(&playback, id, ready(&playback, id, CAPTION, style)));
    let source = ass_source(false, style).unwrap();
    assert_eq!(source.origin, Origin::Text(style));
    let text = std::str::from_utf8(script(&source)).unwrap();
    assert!(text.starts_with(&text_ass::header(&style)));
    assert!(text.contains("0:00:05.00,0:00:07.00"), "already-mapped times are not rebased");
    assert!(text.contains("Mapped {\\b1}caption{\\b0}"));
    assert!(ass_source(true, style).is_none(), "shared burn suppression is retained");
    assert!(Arc::ptr_eq(&source, &ass_source(false, style).unwrap()));
    assert!(install_hls(&playback, id, ready(&playback, id, "WEBVTT\n\n", style)));
    let empty = ass_source(false, style).unwrap();
    assert_ne!(empty.id, source.id);
    assert!(!std::str::from_utf8(script(&empty)).unwrap().contains("Dialogue:"));
    assert_eq!(selected_hls(&playback), Some(id));
    assert!(decode(b"WEBVTT\n\n".to_vec(), "vtt", style).is_err(),
        "the existing non-HLS empty-sidecar contract is unchanged");
}

#[test]
fn hls_reset_is_isolated_and_install_retires_a_previous_local_read() {
    let _serial = plx_base::testlock::serial();
    let _cleanup = Cleanup;
    reset();
    let playback = identity();
    let style = TextStyle::default();
    let id = HlsSubtitleId(1);
    let target = LocalSubtitleTarget::new();
    let local_playback = identity_for(crate::ff::ExternalSource::Local("/synthetic/movie.mkv".into()));
    select_local(local_playback, target.clone(), SidecarFormat::Srt, style);
    let local = take_local_request().unwrap();
    let local_ready = local.prepare_owned(&OffFrame::for_test(),
        b"1\n00:00:01,000 --> 00:00:02,000\nLocal\n".to_vec()).unwrap();
    let generation = state().generation;
    reset_hls();
    assert_eq!(state().generation, generation);
    assert!(local_request_is_current(&local), "HLS Off cannot retire a Local request");
    assert!(install_hls(&playback, id, ready(&playback, id, CAPTION, style)));
    assert!(!install_local(&local, local_ready), "late Local completion cannot replace HLS");
    assert_eq!(selected_hls(&playback), Some(id));
    reset_hls();
    assert_eq!(selected_hls(&playback), None);
    assert!(!selected());
    assert!(ass_source(false, style).is_none());
    select_without_fetch_for_test(42);
    let generation = state().generation;
    reset_hls();
    assert_eq!(state().generation, generation);
    assert!(selected(), "HLS Off cannot retire a Plex selection");
}

#[test]
fn hls_new_item_reset_removes_the_source_and_prepared_debug_has_no_cue_text() {
    let _serial = plx_base::testlock::serial();
    let _cleanup = Cleanup;
    reset();
    let playback = identity();
    let id = HlsSubtitleId(1);
    let style = TextStyle::default();
    let prepared = ready(&playback, id, CAPTION, style);
    assert!(!format!("{prepared:?}").contains("Mapped"));
    assert!(install_hls(&playback, id, prepared));
    crate::player::reset_subtitle();
    assert_eq!(selected_hls(&playback), None);
    assert!(ass_source(false, style).is_none());
}

#[test]
fn hls_complete_timeline_preserves_overlapping_cue_ends() {
    let _serial = plx_base::testlock::serial();
    let _cleanup = Cleanup;
    reset();
    let playback = identity();
    let id = HlsSubtitleId(1);
    let style = TextStyle::default();
    let overlapping = "WEBVTT\n\n00:05.000 --> 00:09.000\nFirst\n\n00:06.000 --> 00:07.000\nSecond\n";
    let prepared = ready(&playback, id, overlapping, style);
    assert!(install_hls(&playback, id, prepared));
    let source = ass_source(false, style).unwrap();
    let text = std::str::from_utf8(script(&source)).unwrap();
    assert!(text.contains("0:00:05.00,0:00:09.00"), "later HLS cue must not truncate the first cue");
    assert!(text.contains("0:00:06.00,0:00:07.00"));
    assert_eq!(text.matches("Dialogue:").count(), 2);
    let ordinary = decode(overlapping.as_bytes().to_vec(), "vtt", style).unwrap();
    let ordinary = std::str::from_utf8(script(&ordinary.source)).unwrap();
    assert!(ordinary.contains("0:00:05.00,0:00:06.00"), "ordinary sidecars retain newest-wins semantics");
}

#[test]
fn hls_expanded_script_limit_and_cancel_preserve_the_current_source() {
    let _serial = plx_base::testlock::serial();
    let _cleanup = Cleanup;
    reset();
    let playback = identity();
    let id = HlsSubtitleId(1);
    let style = TextStyle::default();
    assert!(install_hls(&playback, id, ready(&playback, id, CAPTION, style)));
    let held = ass_source(false, style).unwrap();
    let generation = state().generation;

    // Each literal closing brace becomes two ASS bytes. The input fits the
    // canonical-document budget but its converted output cannot fit with a header.
    let mut expanded = String::from("WEBVTT\n\n00:05.000 --> 00:09.000\n");
    expanded.push_str(&"}".repeat(owned_byte_limit() / 2));
    expanded.push('\n');
    assert!(expanded.len() < owned_byte_limit());
    assert!(matches!(prepare_hls(&OffFrame::for_test(), playback.clone(), HlsSubtitleId(2),
        expanded.into_bytes(), style, &mut || false), Err(HlsPrepareError::InvalidContent)));
    assert_eq!(selected_hls(&playback), Some(id));
    assert_eq!(state().generation, generation);
    assert!(Arc::ptr_eq(&held, &ass_source(false, style).unwrap()));

    // Cancellation arrives during conversion, after the entry check. It must
    // discard preparation without publishing a partial script or selection.
    let mut many_cues = String::from("WEBVTT\n\n");
    for n in 0..256 {
        many_cues.push_str(&format!("{n}\n00:05.000 --> 00:09.000\nCaption {n}\n\n"));
    }
    let mut polls = 0;
    let mut cancel_during_conversion = || { polls += 1; polls >= 3 };
    assert!(matches!(prepare_hls(&OffFrame::for_test(), playback.clone(), HlsSubtitleId(2),
        many_cues.into_bytes(), style, &mut cancel_during_conversion), Err(HlsPrepareError::Cancelled)));
    assert!(polls >= 3, "conversion must observe cancellation, not only its entry check");
    assert_eq!(selected_hls(&playback), Some(id));
    assert_eq!(state().generation, generation);
    assert!(Arc::ptr_eq(&held, &ass_source(false, style).unwrap()));
}

#[test]
fn live_refresh_tokens_cancel_on_reselection_off_and_foreign_source() {
    let _serial = plx_base::testlock::serial();
    let _cleanup = Cleanup;
    reset();
    let owner = identity();
    let id = HlsSubtitleId(1);
    let style = TextStyle::default();
    assert!(install_hls(&owner, id, ready(&owner, id, CAPTION, style)));
    let first = hls_refresh_token(&owner, id).unwrap();
    assert!(hls_refresh_is_current(&first));
    let held = ass_source(false, style).unwrap();
    assert!(install_hls(&owner, id, ready(&owner, id, CAPTION, style)));
    assert!(first.cancelled.load(Ordering::Acquire));
    assert!(!hls_refresh_is_current(&first));
    assert!(!clear_hls_refresh(&first), "a retired worker cannot clear a newer intent");
    assert!(ass_source(false, style).is_some());
    assert!(!Arc::ptr_eq(&held, &ass_source(false, style).unwrap()), "successful reselection replaces content");
    assert!(install_hls(&owner, id, ready(&owner, id, CAPTION, style)));
    let second = hls_refresh_token(&owner, id).unwrap();
    assert!(!clear_hls_refresh(&first), "same-target reselection still retires old generation");
    assert!(clear_hls_refresh(&second));
    assert_eq!(selected_hls(&owner), Some(id), "discontinuity clears cues without changing user selection");
    assert!(ass_source(false, style).is_none());
    reset_hls();
    assert!(second.cancelled.load(Ordering::Acquire));
    assert!(!hls_refresh_is_current(&second));
    assert!(hls_refresh_token(&owner, id).is_none(), "background refresh cannot select an Off track");
    assert!(install_hls(&owner, id, ready(&owner, id, CAPTION, style)));
    let third = hls_refresh_token(&owner, id).unwrap();
    let replacement = identity();
    assert!(install_hls(&replacement, id, ready(&replacement, id, CAPTION, style)));
    assert!(third.cancelled.load(Ordering::Acquire));
    assert!(!clear_hls_refresh(&third));
    assert_eq!(selected_hls(&replacement), Some(id));
}

#[test]
fn live_refresh_replaces_only_current_generation_and_epoch_and_hides_retired_cues() {
    let _serial = plx_base::testlock::serial();
    let _cleanup = Cleanup;
    reset();
    let owner = identity();
    let id = HlsSubtitleId(1);
    let style = TextStyle::default();
    let playback = crate::ff::hls_presentation::HlsPlayback::fixture_live_with_subtitles_for_test();
    let epoch = playback.live_subtitle_revision().unwrap();
    assert!(install_hls(&owner, id, ready(&owner, id, CAPTION, style)));
    let token = hls_refresh_token(&owner, id).unwrap();
    assert!(bind_hls_epoch(&token, &playback, epoch));
    let previous = ass_source(false, style).unwrap();
    assert!(refresh_hls(&token, ready(&owner, id, "WEBVTT\n\n00:08.000 --> 00:09.000\nNew window\n", style), &playback, epoch));
    assert!(!Arc::ptr_eq(&previous, &ass_source(false, style).unwrap()));
    assert!(!refresh_hls(&token, ready(&owner, HlsSubtitleId(2), CAPTION, style), &playback, epoch));
    playback.commit_live_clock_for_test(1, 10_000_000_000);
    assert!(ass_source(false, style).is_none(), "renderer rejects old epoch while network refresh may still be blocked");
    assert_eq!(selected_hls(&owner), Some(id));
    assert!(!refresh_hls(&token, ready(&owner, id, CAPTION, style), &playback, epoch));
    let next_epoch = playback.live_subtitle_revision().unwrap();
    assert_ne!(epoch, next_epoch);
    assert!(refresh_hls(&token, ready(&owner, id, CAPTION, style), &playback, next_epoch));
    assert!(ass_source(false, style).is_some());
    reset_hls();
    assert!(!refresh_hls(&token, ready(&owner, id, CAPTION, style), &playback, next_epoch));
}
