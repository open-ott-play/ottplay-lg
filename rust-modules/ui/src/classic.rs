//! Shared Classic text/list composition. Coordinates are logical surface units;
//! this module neither starts video nor chooses a hardware display window.
use plx_machine::machine::Measure;
use crate::{theme, Painter, Rect};
use crate::route_screen::RouteLayout;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub caption: Rect,
    pub body: Rect,
    pub preview: Rect,
    pub details: Rect,
    pub list: Rect,
    pub hints: Rect,
}

/// The web Classic default fontShift is four reference pixels. The visible
/// page count is fitted separately; glyph size uses the saved requested count.
/// Native text accepts integer sizes, so round down at that final boundary.
pub fn catalogue_text_size(viewport_height: f32, requested: usize) -> i32 {
    let sy = viewport_height.max(1.0) / 540.0;
    let count = requested.max(1) as f32;
    let row = (viewport_height / count).floor().max(1.0);
    (630.0 * sy / count - 3.0 * sy).max(16.0 * sy)
        .min(40.0 * sy).min((10.0 * sy).max(row * 0.98)).floor().max(1.0) as i32
}

pub fn chrome_text_size(viewport_height: f32, requested: usize) -> i32 {
    let sy = viewport_height.max(1.0) / 540.0;
    (catalogue_text_size(viewport_height, requested) as f32)
        .max(22.0 * sy).min(28.0 * sy).floor().max(1.0) as i32
}
impl Layout {
    pub fn new(bounds: Rect, measure: &dyn Measure, preview: bool) -> Self {
        // Studio Classic reference at 1280×720: 40px side margins, a 440×248
        // preview at (40,100), and the channel list at x520. Keep all consumers
        // (including the video lease and pointer hit regions) on this geometry.
        let sx = bounds.w / 1280.0;
        let sy = bounds.h / 720.0;
        let inset = 40.0 * sx;
        let band_h = (74.0 * sy).max(measure.cap_h(theme::size::BODY) + theme::space::SM * 2.0);
        let hints_h = band_h.max(crate::page_header::HEIGHT + theme::space::SM);
        let caption = Rect::new(bounds.x + inset, bounds.y, (bounds.w - inset * 2.0).max(0.0), band_h);
        let hints = Rect::new(caption.x, bounds.y + bounds.h - hints_h, caption.w, hints_h);
        let body_y = caption.y + caption.h + 26.0 * sy;
        let body = Rect::new(caption.x, body_y, caption.w, (hints.y - 6.0 * sy - body_y).max(0.0));
        let split = bounds.x + 520.0 * sx;
        let left_w = 440.0 * sx;
        let preview_h = if preview { (248.0 * sy).min(body.h * 0.55) } else { 0.0 };
        let preview = Rect::new(body.x, body.y, left_w, preview_h);
        let detail_y = body.y + preview_h + if preview_h > 0.0 { 20.0 * sy } else { 0.0 };
        let details = Rect::new(body.x, detail_y, left_w, (body.y + body.h - detail_y).max(0.0));
        let list = Rect::new(split, body.y, (body.x + body.w - split).max(0.0), body.h);
        Self { caption, body, preview, details, list, hints }
    }
    /// Mirror the owned content columns, retaining the full-width caption and hints.
    /// Existing callers keep the default right-hand list unless they opt in.
    pub fn with_list_left(mut self, left: bool) -> Self {
        if left {
            let mirror = |rect: &mut Rect| { rect.x = self.body.x * 2.0 + self.body.w - rect.x - rect.w; };
            mirror(&mut self.list);
            mirror(&mut self.preview);
            mirror(&mut self.details);
        }
        self
    }
    pub fn route(self) -> RouteLayout {
        RouteLayout { narrative: self.details, content: self.list, action: self.hints }
    }
    pub fn header_offset(self) -> (f32, f32) {
        (0.0, self.hints.y + (self.hints.h - crate::page_header::HEIGHT) * 0.5 - crate::page_header::TOP)
    }
    pub fn bounds(self) -> Rect {
        let inset = self.caption.w / 1200.0 * 40.0;
        Rect::new(self.caption.x - inset, self.caption.y, self.caption.w + 2.0 * inset,
            self.hints.y + self.hints.h - self.caption.y)
    }
}

pub fn ground(p: Painter, bounds: Rect) {
    ground_color(p, bounds, crate::palette::color(crate::palette::Role::Background,
        [theme::CLEAR_RGB.0, theme::CLEAR_RGB.1, theme::CLEAR_RGB.2, 1.0]));
}

fn ground_color(p: Painter, bounds: Rect, c: [f32; 4]) {
    p.rect(bounds, 0.0, c, c, 0.0);
}

/// Fill everything except an admitted video destination. The caller may use
/// this only after the existing player confirms the matching viewport request.
pub fn ground_around(p: Painter, bounds: Rect, aperture: Rect, color: [f32; 4]) {
    let right = bounds.x + bounds.w;
    let bottom = bounds.y + bounds.h;
    for r in [
        Rect::new(bounds.x, bounds.y, bounds.w, (aperture.y - bounds.y).max(0.0)),
        Rect::new(bounds.x, aperture.y, (aperture.x - bounds.x).max(0.0), aperture.h),
        Rect::new(aperture.x + aperture.w, aperture.y, (right - aperture.x - aperture.w).max(0.0), aperture.h),
        Rect::new(bounds.x, aperture.y + aperture.h, bounds.w, (bottom - aperture.y - aperture.h).max(0.0)),
    ] { if r.w > 0.0 && r.h > 0.0 { ground_color(p, r, color); } }
}

pub fn caption(p: Painter, layout: Layout, title: &str, measure: &dyn Measure) {
    caption_inner(p, layout, title, None, theme::size::BODY, measure);
}

/// The clock must be supplied from the runtime's accepted wall-time sample.
/// Missing samples leave no invented clock or empty reserved column.
pub fn caption_styled(p: Painter, layout: Layout, title: &str, requested: usize,
    clock: Option<&str>, measure: &dyn Measure) {
    caption_inner(p, layout, title, clock, chrome_text_size(layout.list.h, requested), measure);
}

fn caption_inner(p: Painter, layout: Layout, title: &str, clock: Option<&str>, size: i32, measure: &dyn Measure) {
    use crate::label::{HAlign, Label};
    let clock_size = (22.0 * layout.bounds().h / 720.0).floor().max(1.0) as i32;
    let clock = clock.filter(|clock| !clock.is_empty());
    let clock_width = clock.map_or(0.0, |clock| measure.width_str(clock, clock_size, false)
        + measure.font_scale().points(clock_size as f32) * 0.4);
    let title_width = (layout.caption.w - clock_width).max(0.0);
    let title = measure.fit_line(title, title_width, size, false);
    if title_width > 0.0 {
        Label::new(title.as_ptr(), size, crate::palette::color(crate::palette::Role::Text, theme::CLASSIC_TEXT)).h(HAlign::Start)
            .draw(p, Rect::new(layout.caption.x, layout.caption.y, title_width, layout.caption.h));
    }
    if let Some(clock) = clock {
        if let Ok(clock) = std::ffi::CString::new(clock) {
            Label::new(clock.as_ptr(), clock_size, crate::palette::color(crate::palette::Role::EpgTime, theme::CLASSIC_SECONDARY)).h(HAlign::Right)
                .draw(p, Rect::new(layout.caption.x + title_width, layout.caption.y,
                    clock_width, layout.caption.h));
        }
    }
    let bounds = layout.bounds();
    let line = Rect::new(bounds.x, layout.caption.y + layout.caption.h, bounds.w, 2.0);
    p.rect(line, 0.0, crate::palette::color(crate::palette::Role::Border, theme::CLASSIC_BORDER), crate::palette::color(crate::palette::Role::Border, theme::CLASSIC_BORDER), 0.0);
    let line = Rect::new(bounds.x, layout.hints.y, bounds.w, 1.0);
    p.rect(line, 0.0, crate::palette::color(crate::palette::Role::Border, theme::CLASSIC_BORDER), crate::palette::color(crate::palette::Role::Border, theme::CLASSIC_BORDER), 0.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_machine::machine::{FontScale, TypographyMeasure};
    #[test]
    fn catalogue_glyph_sizes_follow_saved_density_before_global_font_scale() {
        for (height, rows, glyphs, chrome) in [
            (540.0, 10, 40, 28), (540.0, 25, 20, 22), (540.0, 30, 17, 22),
            (810.0, 10, 60, 42), (810.0, 25, 31, 33), (810.0, 30, 26, 33),
        ] {
            assert_eq!(catalogue_text_size(height, rows), glyphs);
            assert_eq!(chrome_text_size(height, rows), chrome);
        }
    }

    #[test]
    fn styled_caption_keeps_clock_separate_and_ellipsizes_long_title() {
        let _serial = plx_base::testlock::serial();
        struct Restore(FontScale);
        impl Drop for Restore { fn drop(&mut self) { plx_gfx::text::set_ui_typography(self.0); } }
        let _restore = Restore(crate::typography::current());
        plx_gfx::text::set_ui_typography(FontScale::from_percent(100).unwrap());
        let measure = crate::fixture::FixtureMeasure;
        let layout = Layout::new(Rect::FULL, &measure, true);
        let title = "Long channel category ".repeat(30);
        let runs = crate::draw_census::capture_runs(|| caption_styled(
            Painter::recording(), layout, &title, 25, Some("12:34:56"), &measure));
        let clock = runs.iter().find(|run| run.text == "12:34:56").expect("clock drawn");
        assert_eq!(clock.align, 2);
        assert_eq!(clock.size, 33);
        assert_eq!(clock.at.x, layout.caption.x + layout.caption.w);
        let caption = runs.iter().find(|run| run.text.starts_with("Long")).expect("title drawn");
        assert!(caption.text.len() < title.len());
        assert!(caption.at.x + measure.width_str(&caption.text, caption.size, false)
            <= clock.at.x - measure.width_str(&clock.text, clock.size, false));
        assert_eq!(layout.bounds(), Rect::FULL);
    }
    #[test]
    fn classic_regions_keep_preview_list_and_controls_disjoint_at_every_scale() {
        for (width, height) in [(1280.0, 720.0), (1920.0, 1080.0)] { for scale in [20, 100, 200] {
            let mut m = TypographyMeasure::new(crate::fixture::FixtureMeasure);
            m.set_font_scale(FontScale::from_percent(scale).unwrap());
            let a = Layout::new(Rect::new(0.0, 0.0, width, height), &m, true);
            assert!(a.preview.h > 0.0 && a.details.h > 0.0 && a.list.h > 0.0);
            assert!(a.preview.x + a.preview.w < a.list.x);
            assert!(a.preview.y + a.preview.h < a.details.y);
            assert!(a.list.y + a.list.h < a.hints.y);
            assert!(a.hints.y + a.hints.h <= height);
            assert_eq!(a.route().content, a.list);
        } }
    }
    #[test]
    fn classic_default_frame_matches_studio_reference() {
        let m = crate::fixture::FixtureMeasure;
        let a = Layout::new(Rect::new(0.0, 0.0, 1280.0, 720.0), &m, true);
        assert_eq!(a.preview, Rect::new(40.0, 100.0, 440.0, 248.0));
        assert_eq!(a.list, Rect::new(520.0, 100.0, 720.0, 540.0));
        assert_eq!(a.details, Rect::new(40.0, 368.0, 440.0, 272.0));
        assert_eq!(a.caption, Rect::new(40.0, 0.0, 1200.0, 74.0));
        assert_eq!(a.hints, Rect::new(40.0, 646.0, 1200.0, 74.0));
    }
    #[test]
    fn studio_reference_scales_horizontal_and_vertical_coordinates_independently() {
        let m = crate::fixture::FixtureMeasure;
        let a = Layout::new(Rect::new(10.0, 20.0, 1920.0, 1080.0), &m, true);
        assert_eq!(a.preview, Rect::new(70.0, 170.0, 660.0, 372.0));
        assert_eq!(a.details, Rect::new(70.0, 572.0, 660.0, 408.0));
        assert_eq!(a.list, Rect::new(790.0, 170.0, 1080.0, 810.0));
        let wide = Layout::new(Rect::new(0.0, 0.0, 1920.0, 720.0), &m, true);
        assert_eq!(wide.preview, Rect::new(60.0, 100.0, 660.0, 248.0));
        assert_eq!(wide.details.y, 368.0);
    }
    #[test]
    fn forms_reclaim_preview_space_without_changing_list_or_controls() {
        let m = crate::fixture::FixtureMeasure;
        let video = Layout::new(Rect::FULL, &m, true);
        let form = Layout::new(Rect::FULL, &m, false);
        assert_eq!(form.preview.h, 0.0);
        assert_eq!(video.list, form.list);
        assert_eq!(video.hints, form.hints);
        assert!(form.details.h > video.details.h);
    }
}

#[cfg(test)]
mod side_tests {
    use super::*;
    #[test]
    fn mirrored_layout_moves_list_and_preview_as_one_geometry() {
        let measure = crate::fixture::FixtureMeasure;
        for bounds in [Rect::new(11.0, 7.0, 1280.0, 720.0), Rect::FULL] {
            let right = Layout::new(bounds, &measure, true);
            let left = right.with_list_left(true);
            for (a, b) in [(right.list, left.list), (right.preview, left.preview), (right.details, left.details)] {
                assert!((b.x - (bounds.x + bounds.w - (a.x - bounds.x) - a.w)).abs() < 0.01);
                assert_eq!((a.y, a.w, a.h), (b.y, b.w, b.h));
            }
            assert_eq!(left.route().content, left.list);
            assert_eq!(left.caption, right.caption);
            assert_eq!(left.hints, right.hints);
            assert!(left.list.x + left.list.w < left.preview.x);
            assert_eq!(right.with_list_left(false), right);
        }
    }
}

#[cfg(test)]
mod palette_tests {
    use super::*;
    #[test]
    fn actual_caption_resolves_text_and_clock_independently_after_publication() {
        let _serial = plx_base::testlock::serial();
        struct Restore(crate::palette::Config);
        impl Drop for Restore { fn drop(&mut self) { crate::palette::project(self.0); } }
        let _restore = Restore(crate::palette::current());
        let config = crate::palette::Config::default()
            .with(crate::palette::Role::Text,Some("#123456")).unwrap()
            .with(crate::palette::Role::EpgTime,Some("#ABCDEF")).unwrap();
        let measure = crate::fixture::FixtureMeasure;
        let layout = Layout::new(Rect::FULL,&measure,true);
        crate::palette::project(config);
        let runs = crate::draw_census::capture_runs(|| caption_styled(
            Painter::recording(),layout,"Palette title",25,Some("12:34:56"),&measure));
        assert_eq!(runs.iter().find(|r|r.text=="Palette title").unwrap().ink,
            config.resolve(crate::palette::Role::Text,theme::CLASSIC_TEXT));
        assert_eq!(runs.iter().find(|r|r.text=="12:34:56").unwrap().ink,
            config.resolve(crate::palette::Role::EpgTime,theme::CLASSIC_SECONDARY));
    }
}
