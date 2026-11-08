//! Classic catalogue presentation. It reads the same accepted OTT snapshot;
//! there is no source selection, fetching or playback policy in this module.
use super::{OttAction, OttNowNext, OttPoster, OttRow, OttView};
use plx_machine::machine::Measure;
use plx_ui::{classic::Layout, theme, Painter, Rect};
use plx_ui::text_view::TextView;
use plx_ui::vstack::{Anchor, VFlow, VStack};
use plx_ui::hstack::Item;

/// Compact only an adapter-owned content list and its existing controls. A
/// pending/empty chooser or another provider's rows keep the ordinary form.
pub(super) fn torrent_catalogue(view: &OttView) -> bool {
    view.rows.iter().any(|row| row.enabled && super::torrent_catalogue_row(&view.page_id, row))
        && view.rows.iter().all(|row| {
            if super::torrent_catalogue_row(&view.page_id, row) { return true; }
            let OttAction::EditSetting(key) = &row.action else { return false; };
            key == &row.id && match view.page_id.as_str() {
                "torrent-results" => matches!(key.as_str(), "torrent.retry" | "torrent.back")
                    || key.starts_with("torrent.more:") || key.starts_with("torrent.query:"),
                "torrent-files" => matches!(key.as_str(), "torrent.retry" | "torrent.back"),
                _ => false,
            }
        })
}

pub(super) fn list_label(row: &OttRow, preferences: plx_appkit::classic_settings::Preferences, now: Option<i64>) -> String {
    let name = row.content_title.as_deref().filter(|name| !name.is_empty()).unwrap_or(&row.title);
    let mut parts = Vec::with_capacity(2);
    if preferences.show_name() { parts.push(name); }
    if preferences.show_program() {
        if let Some(current) = current_programme(row, now).filter(|current| !current.title.is_empty()) {
            parts.push(current.title);
        }
    }
    parts.join("  ")
}

/// A numbered live channel keeps its identity on the primary line; the
/// current programme is a separate row detail so long titles cannot hide it.
pub(super) fn catalogue_title(row: &OttRow) -> &str {
    row.content_title.as_deref().filter(|name| !name.trim().is_empty()).unwrap_or(&row.title)
}

pub(super) fn catalogue_detail(row: &OttRow, now: Option<i64>) -> String {
    if !row.guide_available { return String::new(); }
    let programme = row.current_programme.as_ref().filter(|programme| {
        let now = now.unwrap_or(programme.start_s);
        programme.start_s <= now && now < programme.end_s
    }).map(|programme| programme.title.as_str())
        .or_else(|| current_programme(row, now).map(|programme| programme.title));
    programme.unwrap_or_default().to_owned()
}

/// The secondary run is derived from the same name/visibility projection as
/// `list_label`; it never parses server-provided punctuation or changes identity.
pub(super) fn list_secondary_start(row: &OttRow, preferences: plx_appkit::classic_settings::Preferences, label: &str) -> Option<usize> {
    if !preferences.show_program() || !live_guide(row) { return None; }
    let name = row.content_title.as_deref().filter(|name| !name.is_empty()).unwrap_or(&row.title);
    let at = if preferences.show_name() { name.len() + 2 } else { 0 };
    (at < label.len() && label.is_char_boundary(at)).then_some(at)
}

/// Interval only: the shared table paints it against the latest addressed UTC
/// sample. Presentation never reads a clock or widens selected-guide ownership.
pub(super) fn list_progress_interval(row: &OttRow, now: Option<i64>) -> Option<(i64, i64)> {
    current_programme(row, now).map(|current| (current.start_s, current.end_s))
}

#[derive(Clone, Copy)]
struct ProgrammeView<'a> {
    title: &'a str,
    start_s: i64,
    end_s: i64,
}

/// One interval/authority rule for the row and selected sidebar. A full guide,
/// including an empty one, wins; compact data supplies no next programme or synopsis.
fn current_programme(row: &OttRow, now: Option<i64>) -> Option<ProgrammeView<'_>> {
    if !live_guide(row) { return None; }
    let current = if let Some(guide) = row.guide.as_ref() {
        let programme = guide.at(now.unwrap_or(guide.now_s)).0?;
        ProgrammeView { title: &programme.title, start_s: programme.start_s, end_s: programme.end_s }
    } else {
        let programme = row.current_programme.as_ref()?;
        ProgrammeView { title: &programme.title, start_s: programme.start_s, end_s: programme.end_s }
    };
    (current.end_s > current.start_s
        && now.is_none_or(|now| current.start_s <= now && now < current.end_s)).then_some(current)
}

// HTTP/M3U retains an unknown Video subtype. The admitted Guide capability is
// authoritative and must not be overridden by that deliberately coarse subtype.
fn live_guide(row: &OttRow) -> bool {
    row.guide_available || row.media.as_ref().is_none_or(|media| media.kind == super::OttMediaKind::LiveChannel)
}

fn programme_clock(at: i64) -> String {
    plx_platform::i18n::local_clock_parts(at)
        .and_then(|parts| plx_platform::i18n::current().time_minute(parts)).unwrap_or_default()
}

fn programme_interval(programme: ProgrammeView<'_>, now: i64) -> String {
    let duration = programme.end_s.saturating_sub(programme.start_s).max(0);
    let elapsed = now.saturating_sub(programme.start_s).clamp(0, duration);
    let minutes = duration.saturating_add(30) / 60;
    let elapsed = elapsed.saturating_add(30) / 60;
    let total = plx_platform::i18n::msg::core_duration_minutes_in(plx_platform::i18n::current(), minutes);
    let length = if elapsed > 0 { format!("{elapsed}/{total}") } else { total };
    format!("{} - {} ({length})", programme_clock(programme.start_s), programme_clock(programme.end_s))
}

#[derive(Default)]
pub(super) struct GuideClock {
    sample: Option<i64>,
    anchor: u32,
    elapsed_s: u32,
}
impl GuideClock {
    /// Returns true only when a visible guide can change at this second.
    pub(super) fn update(&mut self, guide: Option<&OttNowNext>, tick: u32) -> bool {
        let before = self.now();
        let sample = guide.map(|guide| guide.now_s);
        if self.sample != sample { self.sample = sample; self.anchor = tick; }
        self.elapsed_s = tick.wrapping_sub(self.anchor) / 1000;
        let end = guide.and_then(|guide| [guide.current.as_ref(), guide.next.as_ref()].into_iter().flatten()
            .filter(|p| p.end_s > p.start_s).map(|p| p.end_s).max());
        before != self.now() && end.is_some_and(|end| before.is_none_or(|before| before < end))
    }
    pub(super) fn now(&self) -> Option<i64> {
        self.sample.map(|sample| sample.saturating_add(i64::from(self.elapsed_s)))
    }
    /// A replacement row can arrive before the next Tick binds this clock.
    /// Advance only its matching sample; otherwise use the row's own UTC fact.
    pub(super) fn now_for(&self, guide: Option<&OttNowNext>) -> Option<i64> {
        let guide = guide?;
        if self.sample == Some(guide.now_s) { self.now() } else { Some(guide.now_s) }
    }
    pub(super) fn write(&self, out: &mut plx_machine::machine::Canon) {
        out.option(self.sample, |out, sample| { out.u64(sample as u64); });
        out.u32(self.anchor).u32(self.elapsed_s);
    }

}


fn fit_text<'a>(view: TextView<'a>, width: f32, height: f32, max_lines: usize) -> Option<(TextView<'a>, f32)> {
    if width <= 0.0 { return None; }
    let lines = ((height / view.line_h()).floor().max(0.0) as usize).min(max_lines);
    if lines == 0 { return None; }
    let view = view.max_lines(lines);
    let height = view.measure_h(width);
    Some((view, height))
}

struct DetailBands {
    content: Rect,
    next: Option<Rect>,
    status: Option<Rect>,
}
impl DetailBands {
    fn new(area: Rect, status_h: Option<f32>, next_h: Option<f32>) -> Self {
        let item = |height: Option<f32>| height.map_or(Item::absent(), |h| Item::new(h + theme::space::SM));
        let items = [item(next_h), item(status_h)];
        let stack = VStack::new(&items, 0.0, Anchor::Bottom(area.y + area.h));
        let rect = |i| stack.span(i).map(|(y, h)| Rect::new(0.0, y, area.w, h - theme::space::SM));
        let next = rect(0);
        let status = rect(1);
        let bottom = next.or(status).map_or(area.y + area.h, |r| r.y);
        Self { content: Rect::new(0.0, area.y, area.w, (bottom - area.y).max(0.0)), next, status }
    }
}

fn text_slot<'a>(view: TextView<'a>, flow: &mut VFlow, width: f32, bottom: f32, max_lines: usize) -> Option<(TextView<'a>, Rect)> {
    let (view, height) = fit_text(view, width, bottom - flow.next_y(), max_lines)?;
    let span = flow.push(Item::new(height))?;
    Some((view, Rect::new(0.0, span.y, width, span.h)))
}

fn line(p: Painter, flow: &mut VFlow, width: f32, bottom: f32, text: &str, size: i32, bold: bool, measure: &dyn Measure) {
    if text.is_empty() { return; }
    let color = if bold { plx_ui::palette::color(plx_ui::palette::Role::Text, theme::CLASSIC_TEXT) } else { plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::CLASSIC_SECONDARY) };
    let view = TextView::new(text, size, color).max_lines(2).with_measure(measure);
    let view = if bold { view.bold() } else { view };
    if let Some((view, rect)) = text_slot(view, flow, width, bottom, 2) {
        view.draw(p, rect);
    }
}

pub(super) fn paint_left(p: Painter, layout: Layout, row: &OttRow, status: &str, status_working: bool, now: Option<i64>, preferences: plx_appkit::classic_settings::Preferences, measure: &dyn Measure) {
    let area = layout.details;
    let _clip = plx_ui::screen::ClipScope::open_in(p, area);
    let p = p.translate(area.x, 0.0);
    // Reserve complete measured footer lines before any metadata is placed.
    // In a short panel, status has priority over upcoming programme metadata.
    let status_view = (!status.is_empty()).then(|| TextView::new(status, theme::size::CAPTION, plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::CLASSIC_SECONDARY)).with_measure(measure))
        .and_then(|view| fit_text(view, area.w, area.h - theme::space::SM, 2));
    let status_h = status_view.as_ref().map(|(_, height)| *height);
    let (current, next) = row.guide.as_ref().map(|g| g.at(now.unwrap_or(g.now_s))).unwrap_or((None, None));
    let live = live_guide(row);
    let current_live = current_programme(row, now);
    let next_programme = next.filter(|_| live);
    let next_clock = next_programme.map_or_else(String::new, |programme| programme_clock(programme.start_s));
    let next_budget = area.h - status_h.map_or(0.0, |height| height + theme::space::SM) - theme::space::SM;
    let next_view = next_programme.map(|programme| {
        let view = TextView::new(&programme.title, theme::size::CAPTION, plx_ui::palette::color(plx_ui::palette::Role::Accent, theme::CLASSIC_PROGRESS)).with_measure(measure);
        if next_clock.is_empty() { view } else { view.lead_quiet(&next_clock, plx_ui::palette::color(plx_ui::palette::Role::EpgTime, theme::CLASSIC_TEXT)) }
    })
        .and_then(|view| fit_text(view, area.w, next_budget, 2));
    let bands = DetailBands::new(area, status_h, next_view.as_ref().map(|(_, height)| *height));
    let content_bottom = bands.content.y + bands.content.h;
    let mut flow = VFlow::new(area.y, theme::space::SM);
    let description = super::metadata::preview_description(row, now);
    if let Some(programme) = current_live {
        // Classic TV details describe the selected programme. Repeating the
        // channel name and category here needlessly crowds out its synopsis.
        line(p, &mut flow, area.w, content_bottom, programme.title, theme::size::BODY, true, measure);
        let interval = programme_interval(programme, now.unwrap_or_else(|| row.guide.as_ref().map_or(programme.start_s, |g| g.now_s)));
        let time = TextView::new(&interval,theme::size::CAPTION,
            plx_ui::palette::color(plx_ui::palette::Role::EpgTime,theme::CLASSIC_SECONDARY))
            .max_lines(2).with_measure(measure);
        if let Some((view,rect)) = text_slot(time,&mut flow,area.w,content_bottom,2) { view.draw(p,rect); }
    } else {
        let title = row.content_title.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or(&row.title);
        line(p, &mut flow, area.w, content_bottom, title, theme::size::BODY, true, measure);
        let detail = super::media::detail(row);
        if detail != description {
            line(p, &mut flow, area.w, content_bottom, &detail, theme::size::CAPTION, false, measure);
        }
        for item in [current, next.filter(|_| !live)].into_iter().flatten() {
            let clock = plx_platform::i18n::local_clock_parts(item.start_s)
                .and_then(|parts| plx_platform::i18n::current().time(parts)).unwrap_or_default();
            line(p, &mut flow, area.w, content_bottom, &format!("{clock}  {}", item.title), theme::size::CAPTION, current.is_some_and(|c| std::ptr::eq(c, item)), measure);
        }
    }
    let body_y = flow.next_y().min(content_bottom);
    let mut body = Rect::new(0.0, body_y, area.w, (content_bottom - body_y).max(0.0));
    if let Some(poster) = row.poster.as_ref().filter(|_| preferences.show_description()) {
        let width = (body.w * 0.24).min(body.h * 2.0 / 3.0);
        if width > 0.0 {
            let art = match poster {
                OttPoster::Plex { server, key } => plx_ui::widgets::Art::Thumb { sid: *server, key, res: (250, 375) },
                OttPoster::Http { url } => plx_ui::widgets::Art::Http { url, res: (250, 375) },
                OttPoster::Local { key } => plx_ui::widgets::Art::Local { key },
            };
            super::metadata::paint_art(p, Rect::new(0.0, body.y, width, width * 1.5), art, theme::space::XS);
            body.x += width + theme::space::MD;
            body.w = (body.w - width - theme::space::MD).max(0.0);
        }
    }
    if preferences.show_description() && !description.is_empty() && body.h > 0.0 {
        let view = TextView::new(description, theme::size::CAPTION, plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::CLASSIC_SECONDARY))
            .with_measure(measure).fade_last(theme::space::LG);
        if let Some((view, _)) = fit_text(view, body.w, body.h, usize::MAX) { view.draw(p, body); }
    }
    if let (Some((view, _)), Some(rect)) = (next_view, bands.next) { view.draw(p, rect); }
    if let (Some((view, _)), Some(rect)) = (status_view, bands.status) {
        super::media::note_status(p, status, status_working);
        view.draw(p, rect); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::OttGuideProgramme;
    #[test]
    fn short_details_reserve_footer_lines_before_fitting_metadata() {
        let measure = plx_ui::fixture::FixtureMeasure;
        let area = Rect::new(0.0, 50.0, 180.0, 100.0);
        let footer = TextView::new("Loading", theme::size::CAPTION, theme::TEXT_TERTIARY)
            .with_measure(&measure).max_lines(2);
        let footer_h = footer.measure_h(area.w);
        let bands = DetailBands::new(area, Some(footer_h), None);
        let status = bands.status.unwrap();
        assert_eq!(bands.content.y + bands.content.h, status.y);
        assert!(status.y + status.h <= area.y + area.h);

        let title = "A long programme title that would wrap across both lines";
        let view = || TextView::new(title, theme::size::BODY, theme::TEXT_SECONDARY)
            .bold().with_measure(&measure).max_lines(2);
        assert!(view().measure_h(area.w) > bands.content.h, "the old unbounded title enters the footer");
        let mut flow = VFlow::new(area.y, theme::space::SM);
        let (fitted, rect) = text_slot(view(), &mut flow, area.w, status.y, 2).unwrap();
        assert_eq!(fitted.line_count(area.w), 1, "fit complete lines rather than clipping glyphs");
        assert!(rect.y + rect.h <= status.y);
        let next = flow.next_y();
        assert!(text_slot(view(), &mut flow, area.w, status.y, 2).is_none());
        assert_eq!(flow.next_y(), next, "an omitted block consumes no space");
        assert!(fit_text(footer, area.w, status.y - next, usize::MAX).is_none(),
            "less than one line must not become an unlimited TextView");
    }
    #[test]
    fn next_programme_reserves_its_own_band_below_the_synopsis() {
        let area = Rect::new(60.0, 552.0, 660.0, 410.0);
        let bands = DetailBands::new(area, Some(32.0), Some(64.0));
        let next = bands.next.unwrap();
        let status = bands.status.unwrap();
        assert_eq!(bands.content.y + bands.content.h, next.y);
        assert!(next.y + next.h < status.y);
        assert!(status.y + status.h <= area.y + area.h);
    }

    fn entry(title: &str, start_s: i64, end_s: i64) -> OttGuideProgramme {
        OttGuideProgramme { title: title.into(), description: String::new(), start_s, end_s }
    }
    #[test]
    fn guide_rolls_next_forward_and_expires_without_using_video_position() {
        let guide = OttNowNext { now_s: 15, current: Some(entry("one", 10, 20)), next: Some(entry("two", 20, 30)) };
        assert_eq!(guide.at(19).0.unwrap().title, "one");
        assert_eq!(guide.at(20).0.unwrap().title, "two");
        assert!(guide.at(20).1.is_none());
        assert!(guide.at(30).0.is_none());
    }
    #[test]
    fn guide_clock_uses_elapsed_monotonic_time_not_clamped_animation_dt() {
        let guide = OttNowNext { now_s: 100, current: None, next: None };
        let mut clock = GuideClock::default();
        clock.update(Some(&guide), u32::MAX - 500);
        clock.update(Some(&guide), 1500);
        assert_eq!(clock.now(), Some(102));
        clock.update(None, 1501);
        assert_eq!(clock.now(), None);
    }
}
