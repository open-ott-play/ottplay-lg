//! Modern presentation of the existing transport. All rectangles are shared by
//! paint, focus and pointer placement; this module owns no input or playback state.
use super::geometry::HudGeometry;
use super::model::{now_playing_context, HudModel, NowPlayingKind, OsdOpacity};
use super::motion::HudMotion;
use super::state::*;
use super::HudBand;
use plx_data::metadata::NowPlaying;
use plx_machine::machine::Measure;
use plx_ui::hstack::{Align, HStack, Item};
use plx_ui::text_view::TextView;
use plx_ui::vstack::{Anchor, VStack};
use plx_ui::widgets::{Button, ControlGround, TransportButton};
use plx_ui::{theme, Painter, Rect, View};
use std::ffi::CString;

pub(super) struct Layout {
    pub geometry: HudGeometry,
    pub panel: Rect,
    pub context: Rect,
    pub title: Rect,
    title_lines: usize,
    pub detail: Rect,
}

pub fn shows_timeline(model: &HudModel, _playing: Option<&NowPlaying>) -> bool {
    model.duration_ns > 0
}

pub(super) fn focused_label(slot: ControlSlot, focused: Option<i32>) -> &'static str {
    use plx_platform::i18n::msg;
    let (ControlSlot::Discs(controls), Some(index)) = (slot, focused) else { return ""; };
    if !controls.enabled(index) { return ""; }
    match index {
        BTN_PREVIOUS if controls.restart => msg::modern_player_restart(),
        BTN_PREVIOUS => msg::modern_player_previous(),
        BTN_NEXT => msg::modern_player_next(),
        BTN_SUBTITLES => msg::widgets_tracks_subtitles(),
        BTN_AUDIO => msg::widgets_tracks_audio(),
        BTN_MORE => msg::modern_player_more(),
        _ => "",
    }
}

/// A single nonwrapping rail. Keep off-rail identities for FocusEngine travel;
/// its pointer clip and the paint clip are both `rail`. Only the actual focused
/// item expands. Revealing it is derived, not a second scroll/focus owner.
fn control_rects(controls: Controls, motion: &HudMotion, focused: Option<i32>,
    label: &str, rail: Rect, measure: &dyn Measure) -> [Option<Rect>; BTN_N as usize] {
    let caption = CString::new(label).unwrap_or_default();
    let h = super::classic::CTRL_H.max(
        TextView::new("", theme::size::CAPTION, plx_ui::palette::color(plx_ui::palette::Role::Text, theme::TEXT_PRIMARY))
            .with_measure(measure).line_h() + theme::space::SM * 2.0);
    let scales: [f32; BTN_N as usize] = std::array::from_fn(|i|
        motion.scale(i as i32).max(plx_ui::widgets::CTRL_FOCUS_SCALE));
    let widths: [f32; BTN_N as usize] = std::array::from_fn(|i| {
        let natural = if focused == Some(i as i32) && !label.is_empty() {
            // Round strictly outward before recovering the text budget in draw_controls:
            // f32 icon/padding sums can otherwise shorten an exactly fitting caption.
            Button::pill_w_measured(&caption, theme::size::CAPTION, true, None, measure)
                .next_up().ceil().max(h)
        } else { super::classic::CTRL_H };
        natural.min((rail.w / scales[i]).max(0.0))
    });
    let items: [Item; BTN_N as usize] = std::array::from_fn(|i| {
        if i < BTN_SUBTITLES as usize && !controls.pair() { Item::absent() }
        else { Item::new(widths[i] * scales[i]) }
    });
    let stack = HStack::new(&items, theme::space::SM, Align::Trailing(rail.x + rail.w));
    let mut shift = 0.0;
    if let Some((x, w)) = focused.and_then(|i| usize::try_from(i).ok()).and_then(|i| stack.span(i)) {
        if x < rail.x { shift = rail.x - x; }
        else if x + w > rail.x + rail.w { shift = rail.x + rail.w - x - w; }
    }
    let mut result = [None; BTN_N as usize];
    for span in stack.placed() {
        let height = if focused == Some(span.i as i32) { h } else { super::classic::CTRL_H };
        result[span.i] = Some(Rect::new(span.x + shift + (span.w - widths[span.i]) * 0.5,
            rail.y + (rail.h - height) * 0.5, widths[span.i], height));
    }
    result
}

pub(super) fn compose(mut geometry: HudGeometry, slot: ControlSlot, motion: &HudMotion,
    focused: Option<i32>, label: &str, playing: Option<&NowPlaying>, wall_now_s: Option<i64>,
    measure: &dyn Measure) -> Layout {
    let line_h = TextView::new("", theme::size::CAPTION, plx_ui::palette::color(plx_ui::palette::Role::Text, theme::TEXT_PRIMARY))
        .with_measure(measure).line_h();
    let mut content_bottom = geometry.row.y - theme::space::LG;
    if let ControlSlot::Discs(controls) = slot {
        let scale = (0..BTN_N).map(|i| motion.scale(i))
            .fold(plx_ui::widgets::CTRL_FOCUS_SCALE, f32::max);
        let h = super::classic::CTRL_H.max(line_h + theme::space::SM * 2.0) * scale;
        geometry.row = Rect::new(super::classic::SB_X,
            super::classic::SB_Y - theme::space::LG - h, super::classic::sb_w(), h);
        geometry.items = control_rects(controls, motion, focused, label, geometry.row, measure);
        geometry.row_anchor.top = geometry.row.y;
        geometry.row_anchor.h = h;
        content_bottom = geometry.row.y - theme::space::LG;
    } else if matches!(slot, ControlSlot::UpNext(_)) {
        let next = crate::up_next::layout_peek(motion, measure);
        content_bottom = content_bottom.min(next.still.y - theme::space::LG)
            .min(next.caption.y - theme::space::LG);
    }
    let content = playing.map(|p| now_playing_context(p, wall_now_s));
    let title = content.as_ref().map_or("", |c| c.title);
    let detail = content.as_ref().map_or("", |c| c.next.unwrap_or(c.description));
    let width = super::classic::sb_w();
    // Preserve the existing stand-in geometry. Metadata yields whole lines
    // when a wide localized Up Next card consumes the upper part of the panel.
    let available = (content_bottom - plx_ui::consts::MARGIN_Y - theme::space::LG).max(0.0);
    let title_view = TextView::new(title, theme::size::TITLE, plx_ui::palette::color(plx_ui::palette::Role::Text, theme::TEXT_PRIMARY))
        .bold().with_measure(measure);
    let title_lines = if title.is_empty() { 0 } else {
        ((available / title_view.line_h()).floor() as usize).min(2)
    };
    let title_h = if title_lines == 0 { 0.0 } else {
        title_view.max_lines(title_lines).measure_h(width)
    };
    let mut used = title_h;
    let mut admit_line = |present: bool| {
        let gap = if used > 0.0 { theme::space::SM } else { 0.0 };
        if present && used + gap + line_h <= available {
            used += gap + line_h;
            Item::new(line_h)
        } else { Item::absent() }
    };
    let context_row = admit_line(content.is_some());
    let detail_row = admit_line(!detail.is_empty());
    let heights = [context_row,
        if title_h > 0.0 { Item::new(title_h) } else { Item::absent() },
        detail_row];
    let stack = VStack::new(&heights, theme::space::SM, Anchor::Bottom(content_bottom));
    let empty = Rect::new(super::classic::SB_X, content_bottom, width, 0.0);
    let mut lines = [empty; 3];
    for span in stack.placed() { lines[span.i] = Rect::new(empty.x, span.y, width, span.h); }
    let top = (content_bottom - stack.height() - theme::space::LG)
        .min(geometry.row.y - theme::space::SM).max(plx_ui::consts::MARGIN_Y);
    geometry.title = lines[1];
    geometry.band = HudBand::from_controls_top(top);
    Layout { geometry, context: lines[0], title: lines[1], title_lines, detail: lines[2],
        panel: Rect::new(super::classic::SB_X - theme::space::SM, top,
            width + 2.0 * theme::space::SM, plx_ui::consts::SCR_H - plx_ui::consts::MARGIN_Y - top) }
}

pub(super) fn draw_panel(p: Painter, layout: &Layout, opacity: OsdOpacity) {
    super::osd::draw_background(p, layout.panel, opacity);
}

/// Split an existing information row; unknown clocks consume neither room nor gap.
fn clocked_frames(frame: Rect, clock: &str, measure: &dyn Measure) -> [Rect; 2] {
    if clock.is_empty() {
        return [Rect::new(frame.x, frame.y, 0.0, frame.h), frame];
    }
    let clock_w = measure.width_str(clock, theme::size::CAPTION, false)
        .next_up().ceil().min(frame.w);
    let body_w = (frame.w - clock_w - theme::space::SM).max(0.0);
    let items = [Item::new(clock_w), if body_w > 0.0 { Item::new(body_w) } else { Item::absent() }];
    let mut frames = [Rect::new(frame.x, frame.y, 0.0, frame.h); 2];
    for span in HStack::new(&items, theme::space::SM, Align::Leading(frame.x)).placed() {
        frames[span.i] = Rect::new(span.x, frame.y, span.w, frame.h);
    }
    frames
}

fn draw_clocked_text(p: Painter, frame: Rect, clock: &str, text: TextView<'_>,
    max_lines: usize, measure: &dyn Measure) {
    let [clock_frame, text_frame] = clocked_frames(frame, clock, measure);
    if clock_frame.w > 0.0 {
        TextView::new(clock, theme::size::CAPTION, plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_SECONDARY))
            .max_lines(1).with_measure(measure).draw(p, clock_frame);
    }
    // compose measured the full-width title. Adding the clock may change its wrap,
    // but cannot grant another line beyond that existing measured allocation.
    let lines = ((frame.h / text.line_h()).round() as usize).min(max_lines);
    if lines > 0 && text_frame.w > 0.0 {
        text.max_lines(lines).draw(p, text_frame);
    }
}

pub(super) fn draw_information(p: Painter, layout: &Layout, playing: Option<&NowPlaying>,
    wall_now_s: Option<i64>, clocks: &mut super::ott_footer::ClockLabels, measure: &dyn Measure) {
    let Some(playing) = playing else { return; };
    let content = now_playing_context(playing, wall_now_s);
    clocks.refresh_context(&content);
    let state = match content.kind {
        NowPlayingKind::Live => plx_platform::i18n::msg::modern_player_live(),
        NowPlayingKind::Replay => plx_platform::i18n::msg::modern_player_replay(),
        NowPlayingKind::Video => plx_platform::i18n::msg::modern_player_video(),
    };
    if layout.context.h > 0.0 {
        let ordinal = content.ordinal.map(|value| value.to_string()).unwrap_or_default();
        let state_w = measure.width_str(state, theme::size::CAPTION, true).min(layout.context.w);
        let ordinal_room = (layout.context.w - state_w - theme::space::SM).max(0.0);
        let ordinal_item = if ordinal.is_empty() || ordinal_room == 0.0 { Item::absent() } else {
            Item::new(measure.width_str(&ordinal, theme::size::CAPTION, false)
                .next_up().ceil().min(ordinal_room))
        };
        let mut items = [Item::new(state_w), ordinal_item, Item::absent()];
        let used = HStack::new(&items, theme::space::SM, Align::Leading(layout.context.x)).width();
        let context_room = (layout.context.w - used - theme::space::SM).max(0.0);
        if !content.context.is_empty() && context_room > 0.0 { items[2] = Item::new(context_room); }
        for span in HStack::new(&items, theme::space::SM, Align::Leading(layout.context.x)).placed() {
            let text = match span.i { 0 => state, 1 => &ordinal, _ => content.context };
            let view = TextView::new(text, theme::size::CAPTION, plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_SECONDARY))
                .max_lines(1).with_measure(measure);
            let view = if span.i == 0 { view.bold() } else { view };
            view.draw(p, Rect::new(span.x, layout.context.y, span.w, layout.context.h));
        }
    }
    if layout.title.h > 0.0 {
        let title = TextView::new(content.title, theme::size::TITLE, plx_ui::palette::color(plx_ui::palette::Role::Text, theme::TEXT_PRIMARY))
            .bold().with_measure(measure);
        draw_clocked_text(p, layout.title, clocks.current(), title, layout.title_lines, measure);
    }
    if layout.detail.h > 0.0 {
        let detail = TextView::new(content.next.unwrap_or(content.description), theme::size::CAPTION, plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_SECONDARY))
            .max_lines(1).with_measure(measure);
        let detail = if content.next.is_some() {
            detail.lead(plx_platform::i18n::msg::modern_player_next(), plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_SECONDARY))
        } else { detail };
        draw_clocked_text(p, layout.detail,
            if content.next.is_some() { clocks.next() } else { "" }, detail, 1, measure);
    }
}

pub(super) fn draw_controls(p: Painter, layout: &Layout, controls: Controls, motion: &HudMotion,
    focused: Option<i32>, label: &str, measure: &dyn Measure) {
    let _clip = plx_ui::screen::ClipScope::open_in(p, layout.geometry.row);
    let env = super::classic::hud_env();
    for i in 0..BTN_N {
        let Some(rect) = layout.geometry.items[i as usize] else { continue; };
        if focused == Some(i) && controls.enabled(i) && !label.is_empty() {
            let empty = CString::default();
            let air = Button::pill_w_measured(&empty, theme::size::CAPTION, true, None, measure);
            let text = measure.fit_line(label, (rect.w - air).max(0.0), theme::size::CAPTION, true);
            Button::new(text.as_ptr(), theme::size::CAPTION, rect).icon(controls.icon(i))
                .focused(true).scale(motion.scale(i)).ground(ControlGround::Unkeyed).draw(&env, p);
        } else {
            TransportButton::new(i, rect).icon(controls.icon(i)).enabled(controls.enabled(i))
                .scale(motion.scale(i)).ground(ControlGround::Unkeyed).draw(&env, p);
        }
    }
}

#[cfg(test)]
#[path = "modern_tests.rs"]
mod tests;
