//! **`HudModel`: the per-frame, read-only snapshot of every value the HUD's presentation reads.**
//!
//! Before the split the draw path read process globals in the middle of drawing: `TX.paused`, the
//! scrub target, `playpos_ns()`, `seek_display_ns()`, `duration_ns()`, `loading(ps)` and the
//! subtitle picks and bitmap cue. The player screen is a machine and reads no global
//! (`docs/sans-io.md`), so those reads become fields of this value, which `src/app/run.rs` fills ONCE
//! a frame and pushes into the screen the way it already pushes `busy` and `transport`. Every draw
//! below it takes `&HudModel`.
//!
//! Pure data: every field is `Copy`, there is no borrowed string and no allocation, so building one is
//! a dozen atomic loads and the snapshot costs nothing to hand down the draw. It carries no
//! `PartialEq` because of [`HudModel::bitmap_by_key`], a function pointer.
//!
//! The model holds exactly what presentation reads. What the HUD's STATE logic samples for itself
//! ([`super::slot`], [`super::busy`], [`super::transport_hidden`], `HudMotion::step`) stays in
//! `state.rs` and `motion.rs`: those run at input and Tick time, and moving them onto a model sampled
//! at prepare time would shift them by a frame.

use plx_plex::plex::session::{SubtitlePosition, SubtitleSize, SubtitleTone};

/// Playback background strength relative to the native theme: level 7 preserves
/// every original alpha exactly. This is not a percentage of final composited pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OsdOpacity(u8);
impl OsdOpacity {
    pub const fn from_level(level: u8) -> Option<Self> {
        if level <= 10 { Some(Self(level)) } else { None }
    }
    pub const fn level(self) -> u8 { self.0 }
    pub fn background_alpha(self, base: f32) -> f32 {
        if self.0 == 7 { base } else { (base * self.0 as f32 / 7.0).clamp(0.0, 1.0) }
    }
}
impl Default for OsdOpacity { fn default() -> Self { Self(7) } }

/// The permanent clock is presentation only; it has no input or timer authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PermanentClock { #[default] Off, Background, Transparent }
impl PermanentClock {
    pub const fn from_index(index: u64) -> Option<Self> {
        match index { 0 => Some(Self::Off), 1 => Some(Self::Background),
            2 => Some(Self::Transparent), _ => None }
    }
    pub const fn index(self) -> u64 {
        match self { Self::Off => 0, Self::Background => 1, Self::Transparent => 2 }
    }
}

/// The image-subtitle cue lookup [`HudModel::bitmap_by_key`] injects: the canvas size and the
/// rects of the selected track's display set with this `start_ns` key.
pub type BitmapLookup = fn(i64) -> Option<(i32, i32, Vec<plx_media::player::SubRect>)>;

/// Every global the HUD's presentation reads, sampled once for the frame. The doc comment of each
/// field quotes the exact expression `src/app/run.rs` samples it with.
#[derive(Clone, Copy, Debug)]
pub struct HudModel {
    pub osd_opacity: OsdOpacity,
    pub permanent_clock: PermanentClock,
    /// Raw UTC epoch seconds supplied by the runtime's existing ClockSample.
    /// None when no calendar sample is available; never decoder or monotonic time.
    pub wall_now_s: Option<i64>,
    /// Actual currently published decoder raster; zero dimensions mean unknown.
    pub video_raster: (i32, i32),
    /// `plx_media::player::TX.paused.load(Relaxed)`: the viewer is logically paused. Drives the
    /// title's reveal and the playbar's state mark.
    pub paused: bool,
    /// `plx_media::player::TX.scrub_ns.load(Relaxed)`: the live scrub preview's target, -1 when no
    /// scrub is in progress.
    pub scrub_ns: i64,
    /// `plx_media::player::playpos_ns()`: the PUBLISHED playhead.
    pub playpos_ns: i64,
    /// `plx_media::player::seek_display_ns()`: the target a loading seek freezes the playhead at,
    /// -1 when none.
    pub seek_display_ns: i64,
    /// `plx_media::player::duration_ns()`.
    pub duration_ns: i64,
    /// `plx_media::player::loading(ps)`: a seek is resolving (the playbar shows `seek_display_ns`).
    pub loading: bool,
    /// `plx_media::route::subtitle_size()`: the viewer's caption size pick.
    pub sub_size: SubtitleSize,
    /// `plx_media::route::subtitle_position()`: the viewer's caption position pick.
    pub sub_position: SubtitlePosition,
    /// `plx_media::player::subtitle_tone()`: the caption ink's tone.
    pub sub_tone: SubtitleTone,
    /// `plx_media::player::desired_sub_idx()`: the selected subtitle track, -1 when off.
    pub sub_sel: i32,
    /// `plx_media::player::active_bitmap_key(plx_media::player::playpos_ns())`: the `start_ns` key
    /// of the selected track's image cue active now, `None` between cues.
    pub bitmap_key: Option<i64>,
    /// `Controls::now(ps)`: which of the Previous/Next pair the playing episode has, and whether
    /// Previous restarts. The enabled predicates of `Previous` and `Next` read it
    /// ([`super::command::HudCommand::enabled`]).
    pub controls: super::state::Controls,
    /// `plx_appkit::chapters_panel::has_chapters(meta)`: the playing item has chapters.
    pub chapters: bool,
    /// `plx_appkit::info_panel::detail_rk(meta).is_some()`: the playing item has a ratingKey to
    /// open the page of (`go_to_detail`).
    pub detail: bool,
    /// `plx_media::player::bitmap_by_key`: the lookup `draw_subtitle_bitmap` calls when the key
    /// changes. Injected so this module never names the player.
    pub bitmap_by_key: BitmapLookup,
    /// The accepted, unclipped picture destination, sampled by the app. Authored subtitles
    /// follow this rectangle; generated text remains in screen space. Not a libass raster size.
    pub video_presentation: plx_media::player::video_geometry::Viewport,
}

/// The lookup an idle model carries: no cue.
fn no_bitmap(_key: i64) -> Option<(i32, i32, Vec<plx_media::player::SubRect>)> {
    None
}

impl Default for HudModel {
    /// What an idle process reads from those globals: their initialisers. Not paused, no scrub and no
    /// seek target (-1), the playhead and duration at 0, nothing loading, the default caption picks
    /// (Medium, Low, white), no subtitle track (-1) and no cue.
    fn default() -> Self {
        Self {
            wall_now_s: None,
            video_raster: (0, 0),
            osd_opacity: OsdOpacity::default(),
            permanent_clock: PermanentClock::default(),
            paused: false,
            scrub_ns: -1,
            playpos_ns: 0,
            seek_display_ns: -1,
            duration_ns: 0,
            loading: false,
            sub_size: SubtitleSize::default(),
            sub_position: SubtitlePosition::default(),
            sub_tone: SubtitleTone::default(),
            sub_sel: -1,
            controls: super::state::Controls::NONE,
            chapters: false,
            detail: false,
            bitmap_key: None,
            bitmap_by_key: no_bitmap,
            video_presentation: plx_media::player::video_geometry::Viewport {
                x: 0, y: 0, width: 1920, height: 1080,
            },
        }
    }
}


/// Presentation kind established by the accepted playing item, never its duration or filename.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NowPlayingKind {
    Live,
    Replay,
    Video,
}

/// Borrowed presentation only. The existing HUD model still owns decoder-time progress.
#[derive(Clone, Copy, Debug)]
pub struct NowPlayingContext<'a> {
    pub kind: NowPlayingKind,
    /// Proven 1-based live catalogue position; never parsed from the title.
    pub ordinal: Option<u32>,
    /// Broadcast start for live guide rows; confirmed playback start for files/replay.
    pub current_start_unix_ms: Option<i64>,
    /// Broadcast next start, or a source-owned finite successor estimate.
    pub next_start_unix_ms: Option<i64>,
    pub title: &'a str,
    pub context: &'a str,
    pub description: &'a str,
    pub next: Option<&'a str>,
    pub archive: Option<&'a plx_data::metadata::OttArchivePlaying>,
}

/// Shared clock inputs for both footer presentations. Missing times stay absent;
/// a decoder duration or wall-clock sample alone never supplies a start time.
pub(super) fn programme_start_times(
    info: &plx_data::metadata::OttNowPlaying,
    wall_now_s: Option<i64>,
) -> (Option<i64>, Option<i64>) {
    if let Some(live) = &info.live {
        let (current, next) = live.guide.as_ref()
            .map(|guide| guide.at(wall_now_s.unwrap_or(guide.now_s)))
            .unwrap_or((None, None));
        let start = |row: Option<&plx_data::metadata::OttGuideProgramme>| row
            .filter(|row| !row.title.trim().is_empty())
            .and_then(|row| row.start_s.checked_mul(1000))
            .filter(|ms| *ms > 0);
        return (start(current), start(next));
    }
    let current = info.confirmed_start_unix_ms.filter(|ms| *ms > 0);
    let replay = info.archive.as_ref()
        .is_some_and(|archive| archive.start_s >= 0 && archive.end_s > archive.start_s);
    let next = info.next.as_ref().filter(|next| !replay && !next.title.trim().is_empty())
        .and_then(|next| next.estimated_start_unix_ms)
        .filter(|next| current.is_some_and(|current| *next >= current));
    (current, next)
}

/// Read at most the two retained guide facts at the supplied runtime wall-clock sample.
/// No guide availability implies a live seek window, and no finite duration implies a film.
pub fn now_playing_context(
    playing: &plx_data::metadata::NowPlaying,
    wall_now_s: Option<i64>,
) -> NowPlayingContext<'_> {
    let ott = playing.ott.as_ref();
    let (current_start_unix_ms, next_start_unix_ms) = ott
        .map(|info| programme_start_times(info, wall_now_s)).unwrap_or((None, None));
    if let Some(live) = ott.and_then(|ott| ott.live.as_ref()) {
        let (current, next) = live.guide.as_ref()
            .map(|guide| guide.at(wall_now_s.unwrap_or(guide.now_s)))
            .unwrap_or((None, None));
        let title = current.map(|row| row.title.as_str())
            .filter(|title| !title.trim().is_empty()).unwrap_or(&live.channel_title);
        return NowPlayingContext {
            kind: NowPlayingKind::Live,
            ordinal: live.ordinal.filter(|ordinal| *ordinal > 0),
            current_start_unix_ms,
            next_start_unix_ms,
            title,
            context: distinct_context(&live.channel_title, title),
            description: current.map(|row| row.description.as_str()).unwrap_or(""),
            next: next.map(|row| row.title.as_str()).filter(|title| !title.trim().is_empty()),
            archive: None,
        };
    }
    let archive = ott.and_then(|ott| ott.archive.as_ref())
        .filter(|archive| archive.start_s >= 0 && archive.end_s > archive.start_s);
    let title = playing.programme_title();
    NowPlayingContext {
        kind: if archive.is_some() { NowPlayingKind::Replay } else { NowPlayingKind::Video },
        ordinal: None,
        current_start_unix_ms,
        next_start_unix_ms,
        title,
        context: ott.map(|ott| distinct_context(&ott.collection_label, title)).unwrap_or(""),
        description: &playing.summary,
        next: if archive.is_some() { None } else {
            ott.and_then(|ott| ott.next.as_ref()).map(|next| next.title.as_str())
                .filter(|title| !title.trim().is_empty())
        },
        archive,
    }
}

fn distinct_context<'a>(context: &'a str, title: &str) -> &'a str {
    if context.trim().is_empty() || context.trim() == title.trim() { "" } else { context }
}

#[cfg(test)]
mod now_playing_tests {
    use super::*;
    use plx_data::metadata::{NowPlaying, OttArchivePlaying, OttGuideProgramme,
        OttLivePlaying, OttNextPlaying, OttNowNext, OttNowPlaying};

    fn video() -> NowPlaying {
        NowPlaying {
            ott: Some(OttNowPlaying { live: None, archive: None,
                collection_label: "Folder".into(), confirmed_start_unix_ms: Some(100_000),
                next: Some(OttNextPlaying { title: "Next file".into(), estimated_start_unix_ms: None }),
                art: None }),
            is_episode: false, is_real_episode: false, title: "Current file".into(),
            ep_title: String::new(), season: 0, index: 0,
            summary: "Complete synopsis. Second sentence retained.".into(),
            year: 0, dur_ms: 60_000, rating: String::new(), thumb: String::new(),
            detail_rk: String::new(),
        }
    }
    fn live() -> NowPlaying {
        let mut playing = video();
        playing.ott.as_mut().unwrap().live = Some(OttLivePlaying {
            channel_title: "Channel".into(), ordinal: Some(1),
            guide: Some(OttNowNext { now_s: 150,
                current: Some(OttGuideProgramme { title: "Current show".into(),
                    description: "Current synopsis".into(), start_s: 100, end_s: 200 }),
                next: Some(OttGuideProgramme { title: "Next show".into(),
                    description: "Next synopsis".into(), start_s: 200, end_s: 300 }),
            }),
        });
        playing
    }

    #[test]
    fn unknown_finite_media_is_video_and_real_episode_keeps_its_leaf_title() {
        let mut playing = video();
        let context = now_playing_context(&playing, Some(1_000));
        assert_eq!(context.kind, NowPlayingKind::Video);
        assert_eq!(context.title, "Current file");
        assert_eq!(context.context, "Folder");
        assert_eq!(context.next, Some("Next file"));
        assert!(context.archive.is_none());
        assert!(context.description.ends_with("Second sentence retained."));
        playing.is_episode = true;
        playing.ep_title = "Extra".into();
        assert_eq!(now_playing_context(&playing, None).title, "Current file");
        playing.is_real_episode = true;
        playing.ep_title = "Episode".into();
        assert_eq!(now_playing_context(&playing, None).title, "Episode");
        playing.dur_ms = 0;
        playing.ott = None;
        assert_eq!(now_playing_context(&playing, None).kind, NowPlayingKind::Video);
    }

    #[test]
    fn live_uses_sampled_guide_and_rolls_over_without_stale_summary_or_next() {
        let playing = live();
        let current = now_playing_context(&playing, None);
        assert_eq!(current.kind, NowPlayingKind::Live);
        assert_eq!((current.title, current.context), ("Current show", "Channel"));
        assert_eq!(current.description, "Current synopsis");
        assert_eq!(current.next, Some("Next show"));
        let next = now_playing_context(&playing, Some(200));
        assert_eq!((next.title, next.description), ("Next show", "Next synopsis"));
        assert!(next.next.is_none());
        let expired = now_playing_context(&playing, Some(300));
        assert_eq!(expired.title, "Channel");
        assert!(expired.description.is_empty() && expired.next.is_none());
    }

    #[test]
    fn missing_or_invalid_live_guide_does_not_borrow_finite_summary() {
        let mut playing = live();
        playing.ott.as_mut().unwrap().live.as_mut().unwrap().guide.as_mut()
            .unwrap().current.as_mut().unwrap().end_s = 100;
        let invalid = now_playing_context(&playing, Some(150));
        assert_eq!(invalid.kind, NowPlayingKind::Live);
        assert_eq!(invalid.title, "Channel");
        assert!(invalid.description.is_empty() && invalid.next.is_none());
        playing.ott.as_mut().unwrap().live.as_mut().unwrap().guide = None;
        let absent = now_playing_context(&playing, Some(150));
        assert_eq!(absent.title, "Channel");
        assert!(absent.description.is_empty() && absent.next.is_none());
    }

    #[test]
    fn replay_requires_a_valid_explicit_interval_and_never_invents_a_successor() {
        let mut playing = video();
        playing.ott.as_mut().unwrap().archive = Some(OttArchivePlaying { start_s: 100, end_s: 200 });
        let context = now_playing_context(&playing, Some(10_000));
        assert_eq!(context.kind, NowPlayingKind::Replay);
        assert_eq!(context.archive, Some(&OttArchivePlaying { start_s: 100, end_s: 200 }));
        assert!(context.next.is_none());
        assert_eq!(context.description, playing.summary);
        for (start_s, end_s) in [(-1, 100), (100, 100), (200, 100)] {
            playing.ott.as_mut().unwrap().archive = Some(OttArchivePlaying { start_s, end_s });
            let invalid = now_playing_context(&playing, None);
            assert_eq!(invalid.kind, NowPlayingKind::Video);
            assert!(invalid.archive.is_none());
        }
    }

    #[test]
    fn live_footer_projects_only_proven_ordinals_and_sampled_programme_starts() {
        let mut playing = live();
        let current = now_playing_context(&playing, Some(150));
        assert_eq!(current.ordinal, Some(1));
        assert_eq!(current.current_start_unix_ms, Some(100_000));
        assert_eq!(current.next_start_unix_ms, Some(200_000));
        let rolled = now_playing_context(&playing, Some(200));
        assert_eq!(rolled.current_start_unix_ms, Some(200_000));
        assert_eq!(rolled.next_start_unix_ms, None);
        let expired = now_playing_context(&playing, Some(300));
        assert_eq!(expired.ordinal, Some(1));
        assert_eq!(expired.current_start_unix_ms, None);
        assert_eq!(expired.next_start_unix_ms, None);
        let live = playing.ott.as_mut().unwrap().live.as_mut().unwrap();
        live.channel_title = "123 Channel".into();
        live.ordinal = None;
        live.guide = None;
        let missing = now_playing_context(&playing, Some(150));
        assert_eq!(missing.ordinal, None);
        assert_eq!(missing.current_start_unix_ms, None);
        assert_eq!(missing.next_start_unix_ms, None);
    }

    #[test]
    fn live_footer_keeps_future_start_in_a_guide_gap_and_rejects_overflow() {
        let mut playing = live();
        let gap = now_playing_context(&playing, Some(50));
        assert_eq!(gap.current_start_unix_ms, None);
        assert_eq!(gap.next, Some("Current show"));
        assert_eq!(gap.next_start_unix_ms, Some(100_000));
        let guide = playing.ott.as_mut().unwrap().live.as_mut().unwrap()
            .guide.as_mut().unwrap();
        guide.current = Some(OttGuideProgramme { title: "Future".into(),
            description: String::new(), start_s: i64::MAX - 1, end_s: i64::MAX });
        guide.next = None;
        let overflow = now_playing_context(&playing, Some(i64::MAX - 1));
        assert_eq!(overflow.title, "Future");
        assert_eq!(overflow.current_start_unix_ms, None);
        assert_eq!(overflow.next_start_unix_ms, None);
    }

    #[test]
    fn blank_guide_titles_and_unknown_epoch_do_not_get_footer_clock_labels() {
        let mut playing = live();
        let info = playing.ott.as_mut().unwrap().live.as_mut().unwrap();
        info.ordinal = Some(0);
        let guide = info.guide.as_mut().unwrap();
        guide.current.as_mut().unwrap().title = "  ".into();
        guide.next.as_mut().unwrap().title = "\t".into();
        let blank = now_playing_context(&playing, Some(150));
        assert_eq!(blank.ordinal, None);
        assert_eq!(blank.title, "Channel");
        assert_eq!(blank.current_start_unix_ms, None);
        assert_eq!(blank.next_start_unix_ms, None);
        let guide = playing.ott.as_mut().unwrap().live.as_mut().unwrap().guide.as_mut().unwrap();
        guide.current.as_mut().unwrap().title = "At epoch".into();
        guide.current.as_mut().unwrap().start_s = 0;
        let epoch = now_playing_context(&playing, Some(150));
        assert_eq!(epoch.title, "At epoch");
        assert_eq!(epoch.current_start_unix_ms, None);
    }

    #[test]
    fn finite_footer_preserves_only_confirmed_starts_and_consistent_successors() {
        let mut playing = video();
        playing.ott.as_mut().unwrap().next.as_mut().unwrap()
            .estimated_start_unix_ms = Some(160_000);
        let known = now_playing_context(&playing, Some(9_000));
        assert_eq!(known.ordinal, None);
        assert_eq!(known.current_start_unix_ms, Some(100_000));
        assert_eq!(known.next_start_unix_ms, Some(160_000));
        for (current, next, title, expected_current, expected_next) in [
            (None, Some(160_000), "Next", None, None),
            (Some(100_000), None, "Next", Some(100_000), None),
            (Some(100_000), Some(99_000), "Next", Some(100_000), None),
            (Some(100_000), Some(160_000), "  ", Some(100_000), None),
            (Some(0), Some(160_000), "Next", None, None),
            (Some(-1), Some(160_000), "Next", None, None),
        ] {
            let info = playing.ott.as_mut().unwrap();
            info.confirmed_start_unix_ms = current;
            info.next = Some(OttNextPlaying { title: title.into(), estimated_start_unix_ms: next });
            let projected = now_playing_context(&playing, Some(9_000));
            assert_eq!(projected.current_start_unix_ms, expected_current);
            assert_eq!(projected.next_start_unix_ms, expected_next);
        }
        playing.ott = None;
        let absent = now_playing_context(&playing, Some(9_000));
        assert_eq!(absent.current_start_unix_ms, None);
        assert_eq!(absent.next_start_unix_ms, None);
    }

    #[test]
    fn replay_footer_uses_confirmed_play_start_not_broadcast_time_or_a_successor() {
        let mut playing = video();
        let info = playing.ott.as_mut().unwrap();
        info.archive = Some(OttArchivePlaying { start_s: 10, end_s: 20 });
        info.next.as_mut().unwrap().estimated_start_unix_ms = Some(160_000);
        let replay = now_playing_context(&playing, Some(9_000));
        assert_eq!(replay.kind, NowPlayingKind::Replay);
        assert_eq!(replay.current_start_unix_ms, Some(100_000));
        assert_eq!(replay.next_start_unix_ms, None);
        assert_eq!(replay.next, None);
    }

    #[test]
    fn duplicate_context_and_empty_next_do_not_take_optional_rows() {
        let mut playing = video();
        let ott = playing.ott.as_mut().unwrap();
        ott.collection_label = playing.title.clone();
        ott.next.as_mut().unwrap().title = "   ".into();
        let context = now_playing_context(&playing, None);
        assert!(context.context.is_empty() && context.next.is_none());
        let playing = live();
        let context = now_playing_context(&playing, Some(50));
        assert_eq!(context.title, "Channel");
        assert!(context.context.is_empty());
    }
}
