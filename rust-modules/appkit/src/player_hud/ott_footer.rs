//! Linear OTT information in the existing transport title region. No timers,
//! focus, playback policy or metadata fetching belong to this presentation.
use super::{Busy, TransportMark};
use plx_data::metadata::{NowPlaying, OttNowPlaying};
use plx_machine::machine::Measure;
use plx_ui::text_view::TextView;
use plx_ui::hstack::{Align, HStack, Item};
use plx_ui::label::HAlign;
use plx_ui::{theme, Painter, Rect};

/// Keep the channel name without its catalogue ordinal during the central
/// loading readout or a transport spinner. A directional seek owns an arrow,
/// not a spinner, and keeps its ordinary number. The caller supplies the same
/// resolved mark used by its transport; this projection does not rederive it.
pub(super) fn visible_ordinal(ordinal: Option<u32>, busy: Busy, mark: TransportMark) -> Option<u32> {
    if matches!(busy, Busy::Readout(plx_ui::widgets::StatusKind::Working, _))
        || (busy == Busy::Transport && mark == TransportMark::Working) {
        None
    } else {
        ordinal
    }
}

/// Cached rendering labels only. Conversion runs when the supplied timestamps or
/// immutable locale changes, never on every video frame.
#[derive(Default)]
pub(super) struct ClockLabels {
    key: Option<(Option<i64>, Option<i64>, usize)>,
    current: String,
    next: String,
    wall_key: Option<(Option<i64>, usize)>,
    wall: String,
    raster: String,
    raster_key: (i32, i32),
    technical: String,
    end_key: Option<(Option<i64>, Option<i64>)>,
    remaining: String,
    next_duration: String,
}
impl ClockLabels {
    fn refresh_with(
        &mut self,
        current: Option<i64>,
        next: Option<i64>,
        locale: usize,
        mut format: impl FnMut(i64) -> Option<String>,
    ) {
        let key = (current, next, locale);
        if self.key == Some(key) {
            return;
        }
        self.current = current.and_then(&mut format).unwrap_or_default();
        self.next = next.and_then(&mut format).unwrap_or_default();
        self.key = Some(key);
    }
    fn refresh_classic(&mut self, info: &OttNowPlaying, wall_now_s: Option<i64>) {
        let locale = plx_platform::i18n::current();
        let locale_id = locale as *const _ as usize;
        let (current, next) = super::model::programme_start_times(info, wall_now_s);
        self.refresh_with(current, next, locale_id, |ms|
            plx_platform::i18n::local_clock_parts(ms / 1000).and_then(|parts| locale.time(parts)));
        self.refresh_wall(wall_now_s);
    }
    pub(super) fn refresh_wall(&mut self, wall_now_s: Option<i64>) {
        let locale = plx_platform::i18n::current();
        // The sampled runtime clock is the only wall-clock input. No draw-time
        // clock read and no reformat while the sample/locale remain unchanged.
        let key = (wall_now_s, locale as *const _ as usize);
        if self.wall_key != Some(key) {
            self.wall = wall_now_s.and_then(plx_platform::i18n::local_clock_parts)
                .and_then(|parts| locale.time(parts)).unwrap_or_default();
            self.wall_key = Some(key);
            self.refresh_technical();
        }
    }
    pub(super) fn refresh_raster(&mut self, raster: (i32, i32)) {
        let raster = if raster.0 > 0 && raster.1 > 0 { raster } else { (0, 0) };
        if self.raster_key == raster { return; }
        self.raster_key = raster;
        self.raster = if raster == (0, 0) { String::new() } else { format!("{}×{}", raster.0, raster.1) };
        self.refresh_technical();
    }
    fn refresh_technical(&mut self) {
        self.technical = [&self.wall, &self.raster].into_iter().filter(|s| !s.is_empty()).map(String::as_str).collect::<Vec<_>>().join(" · ");
    }
    pub(super) fn technical(&self) -> &str { &self.technical }
    /// Match Classic's minute columns, using only the admitted guide or the
    /// supplied decoder sample. A finite successor has no duration in this
    /// descriptor, so its column stays empty instead of reusing today's file.
    pub(super) fn refresh_ends(&mut self, info: Option<&OttNowPlaying>, wall_now_s: Option<i64>,
        decoder: Option<(i64, i64)>) {
        let minutes = |value: i128, units_per_second: i128| -> i64 {
            ((value.max(0) + 30 * units_per_second) / (60 * units_per_second))
                .min(i64::MAX as i128) as i64
        };
        let key = if let Some(live) = info.and_then(|info| info.live.as_ref()) {
            let (current, next, now) = live.guide.as_ref().map(|guide| {
                let now = wall_now_s.unwrap_or(guide.now_s);
                let (current, next) = guide.at(now);
                (current, next, now)
            }).unwrap_or((None, None, 0));
            let current = current.filter(|row| !row.title.trim().is_empty())
                .map(|row| minutes(row.end_s as i128 - now as i128, 1));
            let next = next.filter(|row| !row.title.trim().is_empty())
                .map(|row| minutes(row.end_s as i128 - row.start_s as i128, 1));
            (current, next)
        } else {
            let current = decoder.filter(|(position, duration)| *position >= 0 && *duration > 0)
                .map(|(position, duration)| minutes(duration as i128 - position as i128, 1_000_000_000));
            (current, None)
        };
        if self.end_key == Some(key) { return; }
        self.remaining = key.0.map(|value| format!("+{value}")).unwrap_or_default();
        self.next_duration = key.1.map(|value| value.to_string()).unwrap_or_default();
        self.end_key = Some(key);
    }
    pub(super) fn refresh_wall_minute(&mut self, seconds: i64) -> &str {
        let locale = plx_platform::i18n::current();
        let minute = plx_platform::i18n::clock_minute_key(seconds);
        let key = (minute, locale as *const _ as usize);
        if self.wall_key != Some(key) {
            self.wall = minute.and_then(|_| plx_platform::i18n::local_clock_parts(seconds))
                .and_then(|parts| locale.time_minute(parts)).unwrap_or_default();
            self.wall_key = Some(key);
        }
        &self.wall
    }
    pub(super) fn refresh_context(&mut self, context: &super::model::NowPlayingContext<'_>) {
        let locale = plx_platform::i18n::current();
        self.refresh_with(context.current_start_unix_ms, context.next_start_unix_ms,
            locale as *const _ as usize, |ms|
                plx_platform::i18n::local_clock_parts(ms / 1000).and_then(|parts| locale.time(parts)));
    }
    pub(super) fn current(&self) -> &str { &self.current }
    pub(super) fn next(&self) -> &str { &self.next }
    pub(super) fn remaining(&self) -> &str { &self.remaining }
    pub(super) fn next_duration(&self) -> &str { &self.next_duration }

}

#[derive(Clone, Copy)]
struct Line<'a> {
    title: &'a str,
    clock: &'a str,
    end: &'a str,
    size: i32,
    bold: bool,
}
impl Line<'_> {
    fn text<'a>(&'a self, measure: &'a dyn Measure) -> TextView<'a> {
        let text = TextView::new(
            self.title,
            self.size,
            if self.bold {
                plx_ui::palette::color(plx_ui::palette::Role::Text, theme::TEXT_PRIMARY)
            } else {
                plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_SECONDARY)
            },
        )
        .max_lines(1)
        .with_measure(measure)
        .fade_last(theme::space::MD);
        if self.bold {
            text.bold()
        } else {
            text
        }
    }
}

fn lines<'a>(
    playing: &'a NowPlaying,
    info: &'a OttNowPlaying,
    clocks: &'a ClockLabels,
) -> [Line<'a>; 3] {
    [
        Line {
            title: &info.collection_label,
            clock: "",
            end: "",
            size: theme::size::HEADLINE,
            bold: true,
        },
        Line {
            title: playing.programme_title(),
            clock: &clocks.current,
            end: &clocks.remaining,
            size: theme::size::BODY,
            bold: true,
        },
        Line {
            title: info
                .next
                .as_ref()
                .map(|next| next.title.as_str())
                .unwrap_or(""),
            clock: &clocks.next,
            end: &clocks.next_duration,
            size: theme::size::CAPTION,
            bold: false,
        },
    ]
}

/// The selected browse row never enters this projection: all guide facts are
/// supplied by the immutable current-playing identity.
struct ClassicInfo<'a> {
    lines: [Line<'a>; 3],
    ordinal: Option<u32>,
}
fn classic_info<'a>(playing: &'a NowPlaying, info: &'a OttNowPlaying,
    clocks: &'a ClockLabels, wall_now_s: Option<i64>) -> ClassicInfo<'a> {
    let mut rows = lines(playing, info, clocks);
    rows[0].size = theme::size::HEADLINE; rows[0].bold = true;
    rows[1].bold = false; rows[2].size = theme::size::BODY;
    let Some(live) = &info.live else { return ClassicInfo { lines: rows, ordinal: None }; };
    let (current, next) = live.guide.as_ref().map(|guide|
        guide.at(wall_now_s.unwrap_or(guide.now_s))).unwrap_or((None, None));
    rows[0].title = &live.channel_title;
    rows[1].title = current.map_or("", |programme| programme.title.as_str());
    rows[2].title = next.map_or("", |programme| programme.title.as_str());
    ClassicInfo { lines: rows, ordinal: live.ordinal.filter(|ordinal| *ordinal > 0) }
}

struct ClassicBand {
    rows: [Rect; 3],
    programmes: [[Rect; 3]; 2],
    wall: Rect,
    art: Rect,
    ordinal: Rect,
    ground: Rect,
}
fn classic_band(info: &ClassicInfo<'_>, clocks: &ClockLabels,
    layout: &super::classic::OttClassicLayout, measure: &dyn Measure) -> ClassicBand {
    let gap = theme::space::MD;
    let heading_size = info.lines[0].size;
    let ordinal_width = info.ordinal.map(|ordinal| measure.width_str(&ordinal.to_string(), heading_size, true)).unwrap_or(0.0);
    let ordinal_x = layout.heading.x;
    let x = ordinal_x + if ordinal_width > 0.0 { ordinal_width + gap } else { 0.0 };
    let width = (layout.heading.x + layout.heading.w - x).max(0.0);
    let wall_w = measure.width_str(&clocks.technical, theme::size::BODY, false).min(width * 0.4);
    let mut heading = layout.heading; heading.x = x; heading.w = width;
    let wall = Rect::new(x + width - wall_w, heading.y, wall_w, heading.h);
    heading.w = (width - if wall_w > 0.0 { wall_w + gap } else { 0.0 }).max(0.0);
    let rows = [heading, layout.programme_rows[0], layout.programme_rows[1]];
    let clock_width = info.lines.iter().skip(1).map(|line| measure.width_str(line.clock, line.size, false))
        .fold(0.0, f32::max).min(layout.programme_rows[0].w * 0.4);
    let end_width = info.lines.iter().skip(1).map(|line| measure.width_str(line.end, line.size, false))
        .fold(0.0, f32::max).min(layout.programme_rows[0].w * 0.4);
    let programmes = layout.programme_rows.map(|frame| {
        let mut items = [if clock_width > 0.0 { Item::new(clock_width) } else { Item::absent() },
            Item::new(0.0), if end_width > 0.0 { Item::new(end_width) } else { Item::absent() }];
        let budget = HStack::new(&items, gap, Align::Leading(frame.x)).width();
        items[1] = Item::new((frame.w - budget).max(0.0));
        let stack = HStack::new(&items, gap, Align::Leading(frame.x));
        let mut cells = [Rect::new(frame.x, frame.y, 0.0, frame.h); 3];
        for span in stack.placed() { cells[span.i] = Rect::new(span.x, frame.y, span.w, frame.h); }
        cells
    });
    let ordinal = Rect::new(ordinal_x, heading.y, ordinal_width, heading.h);
    ClassicBand { rows, programmes, wall, art: layout.art, ordinal, ground: layout.panel }
}

/// Unified live rail data; derived only from the accepted playing guide, never
/// decoder position or the selected browser item. Nanoseconds are saturated.
pub(super) fn live_timeline(playing: &NowPlaying, wall_now_s: Option<i64>) -> Option<(i64, i64)> {
    let guide = playing.ott.as_ref()?.live.as_ref()?.guide.as_ref()?;
    let now = wall_now_s.unwrap_or(guide.now_s);
    let (current, _) = guide.at(now);
    let current = current?;
    let ns = |seconds: i128| (seconds * 1_000_000_000).clamp(0, i128::from(i64::MAX)) as i64;
    Some((ns(now as i128 - current.start_s as i128), ns(current.end_s as i128 - current.start_s as i128)))
}

/// Classic's flat channel information band: heading above the common rail,
/// current/next rows each own start, title and minute columns. Transport admission
/// stays with the player; missing facts leave empty cells.
pub(super) fn draw_classic(
    painter: Painter, playing: &NowPlaying, info: Option<&OttNowPlaying>,
    clocks: &mut ClockLabels, layout: &super::classic::OttClassicLayout, measure: &dyn Measure, wall_now_s: Option<i64>,
    decoder: Option<(i64, i64)>, busy: Busy, mark: TransportMark,
) {
    if let Some(info) = info {
        clocks.refresh_classic(info, wall_now_s);
    } else {
        clocks.refresh_with(None, None, plx_platform::i18n::current() as *const _ as usize, |_| None);
        clocks.refresh_wall(wall_now_s);
    }
    clocks.refresh_ends(info, wall_now_s, decoder);
    let mut content = match info {
        Some(info) => classic_info(playing, info, clocks, wall_now_s),
        None => ClassicInfo {
            lines: [
                Line { title: "", clock: "", end: "", size: theme::size::HEADLINE, bold: true },
                Line { title: playing.programme_title(), clock: "", end: &clocks.remaining,
                    size: theme::size::BODY, bold: false },
                Line { title: "", clock: "", end: "", size: theme::size::BODY, bold: false },
            ],
            ordinal: None,
        },
    };
    content.ordinal = visible_ordinal(content.ordinal, busy, mark);
    let band = classic_band(&content, clocks, layout, measure);
    let _clip = plx_ui::screen::ClipScope::open_in(painter, band.ground);
    if let Some(art) = info.and_then(|info| info.art.as_ref()) {
        let art = match art {
            plx_data::metadata::MediaArt::Plex { server, key } => plx_ui::widgets::Art::Thumb { sid: *server, key, res: (240, 240) },
            plx_data::metadata::MediaArt::Http { url } => plx_ui::widgets::Art::Http { url, res: (240, 240) },
            plx_data::metadata::MediaArt::Local { key } => plx_ui::widgets::Art::Local { key },
        };
        plx_ui::widgets::card_contained(painter, band.art, art, theme::space::XS);
    }
    if let Some(ordinal) = content.ordinal {
        TextView::new(&ordinal.to_string(), content.lines[0].size, plx_ui::palette::color(plx_ui::palette::Role::Text, theme::CLASSIC_TEXT))
            .bold().max_lines(1).with_measure(measure).draw(painter, band.ordinal);
    }
    TextView::new(&clocks.technical, theme::size::BODY, plx_ui::palette::color(plx_ui::palette::Role::EpgTime, theme::TEXT_SECONDARY))
        .max_lines(1).with_measure(measure).draw(painter, band.wall);
    for (i, line) in content.lines.iter().enumerate() {
        if line.title.is_empty() { continue; }
        if i == 0 {
            line.text(measure).draw(painter, band.rows[0]);
            continue;
        }
        let cells = band.programmes[i - 1];
        TextView::new(line.clock, line.size, plx_ui::palette::color(plx_ui::palette::Role::EpgTime, theme::CLASSIC_SECONDARY)).max_lines(1).with_measure(measure)
            .draw(painter, cells[0]);
        line.text(measure).draw(painter, cells[1]);
        TextView::new(line.end, line.size, plx_ui::palette::color(plx_ui::palette::Role::EpgTime, theme::CLASSIC_SECONDARY)).max_lines(1).with_measure(measure)
            .h(HAlign::Right).draw(painter, cells[2]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn layout(playing: &NowPlaying, measure: &dyn Measure) -> super::super::classic::OttClassicLayout {
        super::super::classic::ott_classic_layout(
            super::super::ControlSlot::Discs(super::super::Controls::NONE),
            &super::super::motion::HudMotion::new(), 2, Some(playing), measure)
    }
    fn playing(title: &str) -> NowPlaying {
        NowPlaying {
            ott: Some(OttNowPlaying {
                live: None,
                archive: None,
                collection_label: "Series / Season 2".into(),
                confirmed_start_unix_ms: Some(1_000),
                next: Some(plx_data::metadata::OttNextPlaying {
                    title: "The next episode".into(),
                    estimated_start_unix_ms: Some(61_000),
                }),
                art: None,
            }),
            is_episode: false,
            is_real_episode: false,
            title: title.into(),
            ep_title: String::new(),
            season: 0,
            index: 0,
            summary: String::new(),
            year: 0,
            dur_ms: 60_000,
            rating: String::new(),
            thumb: String::new(),
            detail_rk: String::new(),
        }
    }

    #[test]
    fn current_programme_keeps_episode_name_after_metadata_enrichment() {
        let mut playing = playing("The current episode");
        let clocks = ClockLabels {
            current: "10:00".into(),
            next: "10:01".into(),
            ..ClockLabels::default()
        };
        // The catalogue leaf is shown while Plex episode metadata is pending.
        assert_eq!(lines(&playing, playing.ott.as_ref().unwrap(), &clocks)[1].title,
            "The current episode");
        // The enriched descriptor carries the parent title separately from the leaf.
        playing.title = "The series".into();
        playing.ep_title = "The current episode".into();
        playing.is_episode = true;
        playing.is_real_episode = true;
        playing.season = 2;
        playing.index = 3;
        let projected = lines(&playing, playing.ott.as_ref().unwrap(), &clocks);
        assert_eq!(projected.map(|line| line.title), [
            "Series / Season 2", "The current episode", "The next episode",
        ]);
        assert_eq!(projected.map(|line| line.clock), ["", "10:00", "10:01"]);
    }

    #[test]
    fn current_programme_preserves_non_episode_extra_and_missing_title_semantics() {
        for (is_episode, is_real_episode, title, ep_title, expected) in [
            (false, false, "Movie", "", "Movie"),
            (false, false, "Movie", "Movie trailer", "Movie"),
            (true, false, "Series", "Series trailer", "Series"),
            (true, false, "Series", "Behind the scenes", "Series"),
            (true, true, "Series", "", "Series"),
            (true, true, "Series", "  ", "Series"),
            (true, true, "Series", "Episode", "Episode"),
        ] {
            let mut playing = playing(title);
            playing.is_episode = is_episode;
            playing.is_real_episode = is_real_episode;
            playing.ep_title = ep_title.into();
            let clocks = ClockLabels::default();
            let projected = lines(&playing, playing.ott.as_ref().unwrap(), &clocks);
            assert_eq!(projected[1].title, expected,
                "is_episode={is_episode}, is_real_episode={is_real_episode}, ep_title={ep_title:?}");
            assert_eq!(projected[0].title, "Series / Season 2");
            assert_eq!(projected[2].title, "The next episode");
        }
    }

    #[test]
    fn both_footer_clock_columns_keep_complete_12h_and_24h_strings() {
        use plx_machine::machine::{FontScale, TypographyMeasure};
        struct ClockFont;
        impl Measure for ClockFont {
            fn width(&self, text: &std::ffi::CStr, size: i32, _: bool) -> f32 {
                text.to_string_lossy().chars().map(|c| if c == '\u{202f}' { 0.1 } else if c == ' ' { 0.33 } else { 0.6 }).sum::<f32>() * size as f32
            }
            fn cap_h(&self, size: i32) -> f32 { size as f32 * 0.75 }
            fn line_h(&self, size: i32) -> f32 { size as f32 * 1.32 }
        }
        let playing = playing("Current programme"); let info = playing.ott.as_ref().unwrap();
        let mut measure = TypographyMeasure::new(ClockFont);
        for scale in [20, 100, 200] {
            measure.set_font_scale(FontScale::from_percent(scale).unwrap());
            for (current, next) in [("2:10:57\u{202f}PM", "2:11:09\u{202f}PM"), ("14:10:57", "14:11:09")] {
                let mut clocks = ClockLabels { key: Some((info.confirmed_start_unix_ms,
                    info.next.as_ref().and_then(|n| n.estimated_start_unix_ms),
                    plx_platform::i18n::current() as *const _ as usize)),
                    current: current.into(), next: next.into(), ..ClockLabels::default() };
                let runs = plx_ui::draw_census::capture_runs(|| draw_classic(Painter::recording(),
                    &playing, Some(info), &mut clocks, &layout(&playing, &measure), &measure, None, None, Busy::None, TransportMark::None));
                for expected in [current, next] {
                    assert!(runs.iter().any(|run| run.text == expected), "missing complete {expected:?} at {scale}%: {runs:?}");
                }
            }
        }
    }

    #[test]
    fn unknown_times_stay_empty_and_unchanged_frames_do_not_reformat() {
        let mut labels = ClockLabels::default();
        let calls = std::cell::Cell::new(0);
        for _ in 0..10 {
            labels.refresh_with(Some(10), None, 1, |ms| {
                calls.set(calls.get() + 1);
                Some(ms.to_string())
            });
        }
        assert_eq!(calls.get(), 1);
        assert_eq!(labels.current, "10");
        assert_eq!(labels.next, "");
        labels.refresh_with(None, None, 1, |_| {
            panic!("unknown timestamps are not formatted")
        });
        assert_eq!(labels.current, "");
        assert_eq!(labels.next, "");
    }
    #[test]
    fn classic_measured_rows_do_not_overlap_and_reuse_the_freed_tab_width() {
        use plx_machine::machine::{FontScale, TypographyMeasure};
        let playing = playing("Current movie");
        let clocks = ClockLabels { current: "11:59:30 PM".into(), next: "12:29:30 AM".into(), ..ClockLabels::default() };
        for scale in [50, 100, 150] {
            let mut measure = TypographyMeasure::new(plx_ui::fixture::FixtureMeasure);
            measure.set_font_scale(FontScale::from_percent(scale).unwrap());
            let info = classic_info(&playing, playing.ott.as_ref().unwrap(), &clocks, None);
            let layout = layout(&playing, &measure);
            let band = classic_band(&info, &clocks, &layout, &measure);
            assert!(band.rows[0].y >= 0.0);
            for pair in band.rows.windows(2) {
                assert!(pair[0].y + pair[0].h <= pair[1].y);
            }
            assert!(layout.geometry.tabs.iter().all(Option::is_none));
            assert_eq!(band.rows[2].x + band.rows[2].w, layout.geometry.playbar.x + layout.geometry.playbar.w);
            assert!(band.rows[2].y + band.rows[2].h <= plx_ui::consts::SCR_H);
        }
    }

    #[test]
    fn classic_footer_draws_collection_current_and_next_without_changing_episode_identity() {
        let mut playing = playing("Show name");
        playing.is_episode = true;
        playing.is_real_episode = true;
        playing.ep_title = "Current episode".into();
        let mut clocks = ClockLabels::default();
        let runs = plx_ui::draw_census::capture_runs(|| draw_classic(
            Painter::recording(), &playing, playing.ott.as_ref(), &mut clocks,
            &layout(&playing, &plx_ui::fixture::FixtureMeasure), &plx_ui::fixture::FixtureMeasure, None, None, Busy::None, TransportMark::None));
        for title in ["Series / Season 2", "Current episode", "The next episode"] {
            assert!(runs.iter().any(|run| run.text == title), "missing {title}: {runs:?}");
        }
        assert!(!runs.iter().any(|run| run.text == "Show name"));
        assert!(runs.iter().filter(|run| run.text == "Current episode").all(|run| !run.bold));
    }

    #[test]
    fn classic_fallback_uses_actual_episode_title_without_fabricated_guide() {
        let mut playing = playing("Show name");
        playing.ott = None;
        playing.is_episode = true;
        playing.is_real_episode = true;
        playing.ep_title = "Episode one".into();
        let mut clocks = ClockLabels::default();
        let runs = plx_ui::draw_census::capture_runs(|| draw_classic(
            Painter::recording(), &playing, None, &mut clocks,
            &layout(&playing, &plx_ui::fixture::FixtureMeasure), &plx_ui::fixture::FixtureMeasure,
            None, Some((90_000_000_000, 600_000_000_000)), Busy::None, TransportMark::None));
        assert!(runs.iter().any(|run| run.text == "Episode one"));
        assert!(runs.iter().any(|run| run.text == "+9"));
        assert!(!runs.iter().any(|run| run.text == "Show name" || run.text.contains("EPG")));
    }

    fn live_playing() -> NowPlaying {
        use plx_data::metadata::{OttGuideProgramme, OttLivePlaying, OttNowNext};
        let mut value = playing("Do not substitute the channel for a programme");
        value.dur_ms = 9_999_000;
        let guide = |title: &str, start_s, end_s| OttGuideProgramme {
            title: title.into(), description: String::new(), start_s, end_s,
        };
        value.ott.as_mut().unwrap().live = Some(OttLivePlaying {
            channel_title: "Channel A".into(), ordinal: Some(7),
            guide: Some(OttNowNext { now_s: 150,
                current: Some(guide("Current programme", 100, 200)),
                next: Some(guide("Next programme", 200, 300)) }),
        });
        value
    }

    #[test]
    fn classic_live_lines_use_current_playing_guide_and_sampled_wallclock() {
        let playing = live_playing(); let info = playing.ott.as_ref().unwrap();
        let clocks = ClockLabels::default();
        let at150 = classic_info(&playing, info, &clocks, Some(150));
        assert_eq!(at150.lines.map(|line| line.title), ["Channel A", "Current programme", "Next programme"]);
        assert_eq!(at150.ordinal, Some(7)); assert_eq!(live_timeline(&playing, Some(150)), Some((50_000_000_000, 100_000_000_000)));
        let at200 = classic_info(&playing, info, &clocks, Some(200));
        assert_eq!(at200.lines.map(|line| line.title), ["Channel A", "Next programme", ""]);
        assert_eq!(live_timeline(&playing, Some(200)), Some((0, 100_000_000_000)));
        let expired = classic_info(&playing, info, &clocks, Some(300));
        assert_eq!(expired.lines.map(|line| line.title), ["Channel A", "", ""]);
        assert_eq!(live_timeline(&playing, Some(300)), None);
    }

    #[test]
    fn unknown_live_guide_never_fabricates_programme_or_ordinal_from_channel_text() {
        let mut playing = live_playing();
        let info = playing.ott.as_mut().unwrap();
        let live = info.live.as_mut().unwrap(); live.channel_title = "123 Channel A".into();
        live.ordinal = None; live.guide = None;
        let clocks = ClockLabels::default();
        let projected = classic_info(&playing, playing.ott.as_ref().unwrap(), &clocks, None);
        assert_eq!(projected.lines.map(|line| line.title), ["123 Channel A", "", ""]);
        assert_eq!(projected.ordinal, None); assert_eq!(live_timeline(&playing, None), None);
    }

    #[test]
    fn classic_live_render_contains_typed_channel_number_and_programmes() {
        let playing = live_playing(); let mut clocks = ClockLabels::default();
        let runs = plx_ui::draw_census::capture_runs(|| draw_classic(Painter::recording(),
            &playing, playing.ott.as_ref(), &mut clocks, &layout(&playing, &plx_ui::fixture::FixtureMeasure),
            &plx_ui::fixture::FixtureMeasure, Some(150), None, Busy::None, TransportMark::None));
        for expected in ["Channel A", "7", "Current programme", "Next programme"] {
            assert!(runs.iter().any(|run| run.text == expected), "missing {expected}: {runs:?}");
        }
        assert!(!runs.iter().any(|run| run.text == playing.title || run.text.contains("EPG")));
    }

    #[test]
    fn classic_spinner_hides_ordinal_but_directional_seek_keeps_it() {
        let mut playing = live_playing();
        playing.ott.as_mut().unwrap().live.as_mut().unwrap().channel_title = "123 Channel A".into();
        let measure = plx_ui::fixture::FixtureMeasure;
        let frame = layout(&playing, &measure);
        let mut clocks = ClockLabels::default();
        for (busy, displayed_ns, expected_mark, numbered) in [
            (Busy::Readout(plx_ui::widgets::StatusKind::Working, c"Preparing"), 10, TransportMark::None, false),
            (Busy::Readout(plx_ui::widgets::StatusKind::Working, c"Connecting"), 10, TransportMark::None, false),
            (Busy::Readout(plx_ui::widgets::StatusKind::Working, c"Buffering"), 10, TransportMark::None, false),
            (Busy::None, 10, TransportMark::None, true),
            (Busy::Transport, 10, TransportMark::Working, false),
            (Busy::Transport, 5, TransportMark::Rewind, true),
            (Busy::Transport, 15, TransportMark::FastForward, true),
            (Busy::Readout(plx_ui::widgets::StatusKind::Failed, c"Playback failed"), 10, TransportMark::None, true),
        ] {
            let mark = crate::player_hud::transport_mark(false, busy, false, displayed_ns, 10, None);
            assert_eq!(mark, expected_mark);
            let runs = plx_ui::draw_census::capture_runs(|| draw_classic(Painter::recording(),
                &playing, playing.ott.as_ref(), &mut clocks, &frame, &measure, Some(150), None, busy, mark));
            assert!(runs.iter().any(|run| run.text == "123 Channel A"), "missing title for {busy:?}");
            assert!(runs.iter().any(|run| run.text == "Current programme"), "missing programme for {busy:?}");
            assert_eq!(runs.iter().any(|run| run.text == "7"), numbered, "{busy:?}: {runs:?}");
        }
        assert_eq!(playing.ott.as_ref().unwrap().live.as_ref().unwrap().ordinal, Some(7));
    }

    #[test]
    fn classic_band_keeps_logo_clock_and_guide_lines_inside_measured_bounds() {
        use plx_machine::machine::{FontScale, TypographyMeasure};
        let mut playing = live_playing();
        playing.ott.as_mut().unwrap().art = Some(plx_data::metadata::MediaArt::Http { url: "https://fixture.invalid/art.png".into() });
        let info = playing.ott.as_ref().unwrap();
        let clocks = ClockLabels { current: "11:59:30 PM".into(), next: "12:29:30 AM".into(),
            wall: "11:59:59 PM".into(), ..ClockLabels::default() };
        for scale in [20, 100, 200] {
            let mut measure = TypographyMeasure::new(plx_ui::fixture::FixtureMeasure);
            measure.set_font_scale(FontScale::from_percent(scale).unwrap());
            let content = classic_info(&playing, info, &clocks, Some(150));
            let layout = layout(&playing, &measure);
            let band = classic_band(&content, &clocks, &layout, &measure);
            assert!(band.ground.y >= 0.0);
            assert!(band.art.x + band.art.w < band.rows[0].x);
            assert!(band.rows[0].x + band.rows[0].w <= band.wall.x);
            assert!(band.wall.x + band.wall.w <= layout.heading.x + layout.heading.w);
            assert!(band.rows[0].y + band.rows[0].h < layout.geometry.playbar.y);
            assert!(layout.geometry.playbar.y + layout.geometry.playbar.h <= band.rows[1].y);
            assert!(band.rows[1].y + band.rows[1].h <= band.rows[2].y);
            assert!(layout.geometry.tabs.iter().all(Option::is_none));
            assert_eq!(band.rows[2].x + band.rows[2].w, layout.geometry.playbar.x + layout.geometry.playbar.w);
            assert!(band.rows[2].y + band.rows[2].h <= plx_ui::consts::SCR_H);
            assert!(band.art.y >= band.ground.y && band.art.y + band.art.h <= plx_ui::consts::SCR_H);
        }
    }

    #[test]
    fn both_footer_modes_share_clock_inputs_across_live_rollover_and_expiry() {
        let playing = live_playing();
        let mut classic = ClockLabels::default();
        let mut modern = ClockLabels::default();
        for now in [50, 150, 200, 300] {
            classic.refresh_classic(playing.ott.as_ref().unwrap(), Some(now));
            modern.refresh_context(&super::super::model::now_playing_context(&playing, Some(now)));
            assert_eq!(classic.key, modern.key);
            assert_eq!(classic.current(), modern.current());
            assert_eq!(classic.next(), modern.next());
        }
        assert!(classic.current().is_empty() && classic.next().is_empty());
    }

    #[test]
    fn footer_clock_cache_drops_stale_values_on_unknown_or_replay_successors() {
        let mut playing = playing("File");
        let mut clocks = ClockLabels::default();
        clocks.refresh_classic(playing.ott.as_ref().unwrap(), None);
        assert!(!clocks.current().is_empty() && !clocks.next().is_empty());
        playing.ott.as_mut().unwrap().archive = Some(plx_data::metadata::OttArchivePlaying {
            start_s: 10, end_s: 20,
        });
        clocks.refresh_context(&super::super::model::now_playing_context(&playing, None));
        assert!(!clocks.current().is_empty());
        assert!(clocks.next().is_empty());
        playing.ott.as_mut().unwrap().confirmed_start_unix_ms = None;
        clocks.refresh_context(&super::super::model::now_playing_context(&playing, None));
        assert!(clocks.current().is_empty() && clocks.next().is_empty());
        playing.ott = None;
        clocks.refresh_context(&super::super::model::now_playing_context(&playing, None));
        assert!(clocks.current().is_empty() && clocks.next().is_empty());
    }

    #[test]
    fn footer_clock_cache_reformats_for_locale_and_keeps_failed_conversions_empty() {
        let mut labels = ClockLabels::default();
        labels.refresh_with(Some(1000), Some(2000), 1, |ms| Some(ms.to_string()));
        labels.refresh_with(Some(1000), Some(2000), 2, |_| None);
        assert!(labels.current().is_empty() && labels.next().is_empty());
        labels.refresh_with(None, None, 2, |_| panic!("unknown timestamp must stay absent"));
        assert!(labels.current().is_empty() && labels.next().is_empty());
    }

    #[test]
    fn one_clock_cache_can_switch_between_footer_modes_without_retaining_old_times() {
        let playing = live_playing();
        let mut labels = ClockLabels::default();
        labels.refresh_classic(playing.ott.as_ref().unwrap(), Some(150));
        let previous_next = labels.next().to_owned();
        assert!(!previous_next.is_empty());
        labels.refresh_context(&super::super::model::now_playing_context(&playing, Some(200)));
        assert_eq!(labels.current(), previous_next);
        assert!(labels.next().is_empty());
        labels.refresh_classic(playing.ott.as_ref().unwrap(), Some(300));
        assert!(labels.current().is_empty() && labels.next().is_empty());
        labels.refresh_context(&super::super::model::now_playing_context(&playing, Some(150)));
        assert!(!labels.current().is_empty());
        assert_eq!(labels.next(), previous_next);
    }

    #[test]
    fn classic_programme_minutes_are_owned_by_live_guide_not_decoder() {
        let playing = live_playing(); let info = playing.ott.as_ref().unwrap();
        let mut labels = ClockLabels::default();
        labels.refresh_ends(Some(info), Some(150), Some((0, 9_999_000_000_000)));
        assert_eq!((labels.remaining.as_str(), labels.next_duration.as_str()), ("+1", "2"));
        labels.refresh_ends(Some(info), Some(200), Some((0, 9_999_000_000_000)));
        assert_eq!((labels.remaining.as_str(), labels.next_duration.as_str()), ("+2", ""));
        labels.refresh_ends(Some(info), Some(300), Some((0, 9_999_000_000_000)));
        assert!(labels.remaining.is_empty() && labels.next_duration.is_empty());
        let mut missing = info.clone(); missing.live.as_mut().unwrap().guide = None;
        labels.refresh_ends(Some(&missing), Some(150), Some((0, 9_999_000_000_000)));
        assert!(labels.remaining.is_empty() && labels.next_duration.is_empty());
    }

    #[test]
    fn classic_finite_remaining_never_invents_next_duration_or_elapsed_wall_time() {
        let playing = playing("File"); let info = playing.ott.as_ref().unwrap();
        let mut labels = ClockLabels::default();
        labels.refresh_ends(Some(info), Some(2_000_000_000), Some((90_000_000_000, 600_000_000_000)));
        assert_eq!(labels.remaining, "+9");
        assert!(labels.next_duration.is_empty(), "next file has no supplied duration");
        labels.refresh_ends(Some(info), Some(2_000_000_001), Some((90_000_000_000, 600_000_000_000)));
        assert_eq!(labels.remaining, "+9", "paused file does not advance with wall time");
        labels.refresh_ends(Some(info), None, Some((700_000_000_000, 600_000_000_000)));
        assert_eq!(labels.remaining, "+0");
        for decoder in [None, Some((0, 0)), Some((-1, 600_000_000_000))] {
            labels.refresh_ends(Some(info), None, decoder);
            assert!(labels.remaining.is_empty() && labels.next_duration.is_empty());
        }
    }

    #[test]
    fn classic_programme_columns_reserve_whole_clocks_and_right_aligned_durations() {
        use plx_machine::machine::{FontScale, TypographyMeasure};
        let playing = live_playing(); let info = playing.ott.as_ref().unwrap();
        let clocks = ClockLabels { current: "11:59:30 PM".into(), next: "12:29:30 AM".into(),
            remaining: "+999".into(), next_duration: "125".into(), ..ClockLabels::default() };
        for scale in [20, 100, 200] {
            let mut measure = TypographyMeasure::new(plx_ui::fixture::FixtureMeasure);
            measure.set_font_scale(FontScale::from_percent(scale).unwrap());
            let content = classic_info(&playing, info, &clocks, Some(150));
            let layout = layout(&playing, &measure);
            let band = classic_band(&content, &clocks, &layout, &measure);
            for row in 0..2 {
                let cells = band.programmes[row];
                assert!(cells[0].x + cells[0].w <= cells[1].x);
                assert!(cells[1].x + cells[1].w <= cells[2].x);
                assert!(cells[2].x + cells[2].w <= layout.programme_rows[row].x + layout.programme_rows[row].w + 0.01);
                assert!(cells[0].w >= measure.width_str(if row == 0 { &clocks.current } else { &clocks.next }, theme::size::BODY, false));
                assert!(cells[2].w >= measure.width_str(if row == 0 { &clocks.remaining } else { &clocks.next_duration }, theme::size::BODY, false));
            }
        }
    }

    #[test]
    fn actual_classic_footer_paints_remaining_and_next_duration_in_separate_cells() {
        let playing = live_playing();
        let measure = plx_ui::fixture::FixtureMeasure;
        let frame = layout(&playing, &measure);
        let mut clocks = ClockLabels::default();
        let runs = plx_ui::draw_census::capture_runs(|| draw_classic(Painter::recording(), &playing,
            playing.ott.as_ref(), &mut clocks, &frame, &measure, Some(150), None, Busy::None, TransportMark::None));
        for expected in ["Channel A", "Current programme", "Next programme", "+1", "2"] {
            assert!(runs.iter().any(|run| run.text == expected), "missing {expected}: {runs:?}");
        }
        assert!(runs.iter().any(|run| run.text == "Channel A" && run.bold));
        let current = runs.iter().find(|run| run.text == "Current programme").unwrap();
        let next = runs.iter().find(|run| run.text == "Next programme").unwrap();
        let remaining = runs.iter().find(|run| run.text == "+1").unwrap();
        let duration = runs.iter().find(|run| run.text == "2").unwrap();
        assert_eq!(remaining.align, 2);
        assert_eq!(duration.align, 2);
        assert!(current.at.x + current.at.w <= remaining.at.x - remaining.at.w);
        assert!(next.at.x + next.at.w <= duration.at.x - duration.at.w);
        assert!(current.at.y < next.at.y && remaining.at.y < duration.at.y);
        assert!(!runs.iter().any(|run| run.text.starts_with('-')));
    }

    #[test]
    fn classic_footer_resolution_is_actual_sample_and_no_guide_keeps_header() {
        let _serial = plx_base::testlock::serial();
        let mut playing = live_playing();
        playing.ott.as_mut().unwrap().live.as_mut().unwrap().guide = None;
        let measure = plx_ui::fixture::FixtureMeasure;
        let plan = layout(&playing, &measure);
        let mut clocks = ClockLabels::default();
        clocks.refresh_raster((1920, 1080));
        let runs = plx_ui::draw_census::capture_runs(|| draw_classic(Painter::recording(), &playing,
            playing.ott.as_ref(), &mut clocks, &plan, &measure, Some(150), None, Busy::None, TransportMark::None));
        assert!(runs.iter().any(|run| run.text == "Channel A"));
        assert!(runs.iter().any(|run| run.text == "7"));
        assert!(runs.iter().any(|run| run.text.contains("1920×1080")));
        assert!(!runs.iter().any(|run| run.text == "Current programme"));
        clocks.refresh_raster((0, 0));
        assert!(!clocks.technical().contains('×'), "unknown raster cannot reuse previous stream resolution");
    }

}
