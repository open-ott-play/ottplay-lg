//! Shared playback OSD presentation. The screen owns foreground visibility and
//! invalidation; this module receives time and registers no input or timer.
use super::model::{HudModel, OsdOpacity, PermanentClock};
use super::ott_footer::ClockLabels;
use plx_machine::machine::Measure;
use plx_ui::text_view::TextView;
use plx_ui::{theme, Painter, Rect};

/// The same validity and minute key used by clock paint and the owner's dirty gate.
pub fn clock_minute_key(model: &HudModel) -> Option<i64> {
    if model.permanent_clock == PermanentClock::Off { return None; }
    plx_platform::i18n::clock_minute_key(model.wall_now_s?)
}

pub(super) fn draw_background(p: Painter, bounds: Rect, opacity: OsdOpacity) {
    plx_ui::widgets::sheet_ground_with_alpha(p, bounds, theme::ALERT_PANEL_RAD,
        |alpha| opacity.background_alpha(alpha));
}

/// Cached localized text only; visibility remains the foreground screen's decision.
#[derive(Default)]
pub struct Clock { labels: ClockLabels }
impl Clock {
    pub fn draw(&mut self, p: Painter, model: &HudModel, measure: &dyn Measure) -> Option<Rect> {
        clock_minute_key(model)?;
        let text = self.labels.refresh_wall_minute(model.wall_now_s?);
        if text.is_empty() { return None; }
        let safe = plx_ui::consts::SAFE;
        let padding = theme::space::MD;
        let view = TextView::new(text, theme::size::BODY, theme::TEXT_PRIMARY)
            .max_lines(1).with_measure(measure);
        let width = measure.width_str(text, theme::size::BODY, false);
        let height = view.line_h();
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0
            || width + 2.0 * padding > safe.w || height + 2.0 * padding > safe.h { return None; }
        let bounds = Rect::new(safe.x + safe.w - width - 2.0 * padding, safe.y,
            width + 2.0 * padding, height + 2.0 * padding);
        if model.permanent_clock == PermanentClock::Background {
            draw_background(p, bounds, model.osd_opacity);
        }
        view.draw(p, Rect::new(bounds.x + padding, bounds.y + padding, width, height));
        Some(bounds)
    }
}
