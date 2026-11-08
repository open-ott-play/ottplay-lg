//! Passive notifications above either skin. The application owns authenticated delivery,
//! expiry and image decoding; this module only paints values on the main/GL thread.
//! No input, focus, playback or persistence capability is accepted here.
use std::sync::Arc;

use plx_machine::machine::Measure;
use plx_ui::icons::{self, Icon};
use plx_ui::label::HAlign;
use plx_ui::tex::{Decoded, Tex, Uploader};
use plx_ui::text_view::TextView;
use plx_ui::{Painter, Rect};

const WIDTH: f32 = 480.0;
const PAD: f32 = 20.0;
const ICON: f32 = 42.0;
const GAP: f32 = 16.0;
const TEXT_WIDTH: f32 = WIDTH - PAD * 2.0 - ICON - GAP;
const ENTER_MS: u64 = 480;
const EXIT_MS: u64 = 320;
// Hold the fully entered card for 500ms before its surface dims and heading folds.
const DIM_AT_MS: u64 = ENTER_MS + 500;
const FOLD_AT_MS: u64 = DIM_AT_MS;
const FOLD_MS: u64 = 960;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity { Info, Warning, Critical }

pub struct Banner {
    pub id: String,
    pub title: String,
    pub message: String,
    pub severity: Severity,
    /// Both instants use the receiver's monotonic clock, including across SDL tick wrap.
    pub started_ms: u64,
    pub expires_ms: u64,
    /// Chosen once by the receiver, never randomized on repaint.
    pub offset_px: u8,
    pub image: Option<Arc<Decoded>>,
}

impl Severity {
    fn label(self) -> &'static str {
        match self { Self::Info => "Home update", Self::Warning => "Attention", Self::Critical => "Alert" }
    }
    fn color(self) -> [f32; 4] {
        match self {
            Self::Info => [0.65, 0.83, 1.0, 1.0],
            Self::Warning => [1.0, 0.85, 0.60, 1.0],
            Self::Critical => [1.0, 0.70, 0.74, 1.0],
        }
    }
}

fn active(b: &Banner, now: u64) -> bool {
    b.started_ms <= now && now < b.expires_ms
}

/// Timed presentation must wake an otherwise settled catalogue. The receiver invalidates
/// once more when removing the last item, so its last painted pixels are cleared.
pub fn needs_frame(banners: &[Banner], now_ms: u64) -> bool {
    banners.iter().any(|b| active(b, now_ms))
}

#[derive(Clone, Copy, Debug)]
struct Phase {
    slide: f32,
    aperture: f32,
    heading: f32,
    surface: f32,
    text: f32,
    ring: f32,
}

fn progress(elapsed: u64, duration: u64) -> f32 {
    (elapsed as f64 / duration.max(1) as f64).clamp(0.0, 1.0) as f32
}
fn smooth(x: f32) -> f32 { x * x * (3.0 - 2.0 * x) }

fn phase(b: &Banner, now: u64, reduced_motion: bool) -> Option<Phase> {
    if !active(b, now) { return None; }
    if reduced_motion {
        return Some(Phase { slide: 0.0, aperture: 1.0, heading: 1.0, surface: 1.0, text: 1.0, ring: 0.0 });
    }
    let age = now - b.started_ms;
    let lifetime = b.expires_ms - b.started_ms;
    let enter = 1.0 - (1.0 - progress(age, ENTER_MS)).powi(3);
    let out = progress(EXIT_MS.saturating_sub(b.expires_ms - now), EXIT_MS).powi(2);
    let dim = progress(age.saturating_sub(DIM_AT_MS), lifetime.saturating_sub(EXIT_MS + DIM_AT_MS));
    Some(Phase {
        slide: (1.0 - enter).max(out),
        aperture: (0.08 + 0.92 * enter) * (1.0 - 0.92 * out),
        heading: 1.0 - smooth(progress(age.saturating_sub(FOLD_AT_MS), FOLD_MS)),
        surface: (0.04 + 0.96 * enter) * (1.0 - 0.82 * dim),
        // Never apply the long surface fade to readable text.
        text: enter * (1.0 - out),
        // A one-second lap keeps the notification's activity visible without adding motion to its
        // layout. This is painted as a segmented circle around the severity glyph below.
        ring: (age % 1_000) as f32 / 1_000.0,
    })
}

fn image_valid(image: &Decoded) -> bool {
    image.w > 0 && image.h > 0 && image.w <= 640 && image.h <= 360
        && image.rgba.len() == image.w as usize * image.h as usize * 4
}

fn image_size(image: &Decoded) -> (f32, f32) {
    let scale = (WIDTH / image.w as f32).min(270.0 / image.h as f32);
    (image.w as f32 * scale, image.h as f32 * scale)
}

fn card_rect(width: f32, height: f32, top: f32, offset: u8, state: Phase) -> Rect {
    Rect::new(
        Rect::FULL.w - plx_ui::consts::MARGIN_X - width
            + (width + plx_ui::consts::MARGIN_X) * state.slide,
        top + offset.min(5) as f32,
        width,
        height,
    )
}

struct Resident { id: String, image: Arc<Decoded>, tex: Tex }

/// At most three transient textures, unrelated to catalogue poster keys/cache.
/// The current receiver explicitly replaces its previous camera banner; accepting
/// three here also keeps every valid image-only card correct for a full snapshot.
#[derive(Default)]
pub struct Renderer { resident: Vec<Resident> }

struct Gpu;
impl Uploader for Gpu {
    fn upload(&mut self, d: &Decoded) -> Tex {
        Tex { id: plx_gfx::gfx::upload_rgba(0, d.w as i32, d.h as i32, d.rgba.as_ptr()), w: d.w, h: d.h }
    }
    fn warm(&mut self, _t: Tex) {}
    fn free(&mut self, t: Tex) { plx_gfx::gfx::delete_tex(t.id); }
}

impl Renderer {
    fn sync(&mut self, banners: &[Banner], now: u64, uploader: &mut dyn Uploader) {
        let wanted = banners.iter().filter(|b| active(b, now)).take(3)
            .filter_map(|b| b.image.as_ref().filter(|d| image_valid(d)).map(|d| (b, d)));
        // Free obsolete resources before uploading replacements: never temporarily
        // exceed 3*640*360*4 bytes of retained image pixels on the GPU.
        self.resident.retain(|current| {
            let keep = wanted.clone().any(|(banner, image)|
                current.id == banner.id && Arc::ptr_eq(&current.image, image));
            if !keep { uploader.free(current.tex); }
            keep
        });
        for (banner, image) in wanted {
            if self.resident.iter().any(|r| r.id == banner.id && Arc::ptr_eq(&r.image, image)) { continue; }
            let tex = uploader.upload(image);
            if tex.id != 0 {
                self.resident.push(Resident { id: banner.id.clone(), image: Arc::clone(image), tex });
            }
        }
    }

    fn release(&mut self, uploader: &mut dyn Uploader) {
        for resident in self.resident.drain(..) { uploader.free(resident.tex); }
    }

    /// Main/GL thread only, as for draw. Call when suspending the receiver.
    pub fn clear(&mut self) { self.release(&mut Gpu); }

    pub fn draw(&mut self, banners: &[Banner], now_ms: u64, osd_opacity: f32,
        reduced_motion: bool, painter: Painter, measure: &dyn Measure)
    {
        // Discovery/text preparation must not allocate or retire GPU resources.
        if !painter.is_recording() && !plx_ui::frame::backdrop::discovering() {
            self.sync(banners, now_ms, &mut Gpu);
        }
        let mut top = plx_ui::consts::MARGIN_Y;
        for b in banners.iter().filter(|b| active(b, now_ms)).take(3) {
            let state = phase(b, now_ms, reduced_motion).expect("active banner");
            let image = self.resident.iter().find(|r| r.id == b.id
                && b.image.as_ref().is_some_and(|image| Arc::ptr_eq(image, &r.image)));
            let height = if let Some(image) = image {
                let (w, h) = image_size(&image.image);
                let rect = card_rect(w, h, top, b.offset_px, state);
                let clip = Rect::new(rect.x, rect.cy() - h * state.aperture * 0.5, w, h * state.aperture);
                painter.clip(clip.intersect(Rect::FULL));
                // The complete camera frame replaces all redundant text; no crop.
                painter.alpha(state.text).tex(image.tex.id, rect, 20.0, [1.0; 4]);
                painter.clip_clear();
                h
            } else {
                draw_text(b, state, top, osd_opacity, painter, measure)
            };
            top += height + 8.0 + 5.0;
        }
    }
}

impl Drop for Renderer {
    // Production Runtime is dropped in ott_runtime::shutdown, before run::shutdown
    // tears down SDL/GL. Suspend and reconfigure also clear on the same main thread.
    // An empty renderer or a recording-only draw reaches no GL call.
    fn drop(&mut self) { self.clear(); }
}

fn draw_text(b: &Banner, state: Phase, top: f32, opacity: f32, p: Painter, measure: &dyn Measure) -> f32 {
    let accent = b.severity.color();
    let label = TextView::new(b.severity.label(), 15, accent).bold().max_lines(1)
        .with_measure(measure).h(HAlign::Start);
    let title = TextView::new(&b.title, 20, [0.96, 0.97, 0.98, 1.0]).bold().max_lines(2)
        .break_long_words().with_measure(measure).h(HAlign::Start);
    let message = TextView::new(&b.message, 20, [0.85, 0.88, 0.93, 1.0]).max_lines(3)
        .break_long_words().with_measure(measure).h(HAlign::Start);
    let label_h = label.measure_h(TEXT_WIDTH);
    let title_h = if b.title.is_empty() { 0.0 } else { title.measure_h(TEXT_WIDTH) + 6.0 };
    let heading_h = label_h + title_h + 6.0;
    let message_h = message.measure_h(TEXT_WIDTH);
    let shown_heading = heading_h * state.heading;
    let height = 2.0 * PAD + (shown_heading + message_h).max(ICON);
    let rect = card_rect(WIDTH, height, top, b.offset_px, state);
    let clip = Rect::new(rect.x, rect.cy() - height * state.aperture * 0.5, WIDTH, height * state.aperture)
        .intersect(Rect::FULL);
    p.clip(clip);
    // The input is the resolved footer alpha; the web notification contract adds
    // eight points then applies an 80% surface factor (default 0.624, maximum 0.8).
    let opacity = if opacity.is_finite() { opacity.clamp(0.0, 1.0) } else { 0.70 };
    let surface = p.alpha(state.surface * (opacity + 0.08).min(1.0) * 0.8);
    surface.rrect(rect, 20.0, 20.0, [0.09, 0.11, 0.15, 1.0]);
    let icon = Rect::new(rect.x + PAD, rect.y + PAD, ICON, ICON);
    surface.rrect(icon, 14.0, 14.0, [accent[0] * 0.2, accent[1] * 0.2, accent[2] * 0.2, 1.0]);
    draw_activity_ring(surface, icon, accent, state.ring, state.text);
    icons::draw(surface, if b.severity == Severity::Info { Icon::Info } else { Icon::Alert },
        Rect::new(icon.x + 9.0, icon.y + 9.0, 24.0, 24.0), accent);
    let x = icon.x + ICON + GAP;
    let y = rect.y + PAD;
    let text = p.alpha(state.text);
    if shown_heading > 0.0 {
        p.clip(Rect::new(x, y, TEXT_WIDTH, shown_heading).intersect(clip));
        let heading_y = y - (heading_h - shown_heading);
        label.draw(text, Rect::new(x, heading_y, TEXT_WIDTH, label_h));
        if title_h > 0.0 { title.draw(text, Rect::new(x, heading_y + label_h + 6.0, TEXT_WIDTH, title_h - 6.0)); }
    }
    p.clip(clip);
    message.draw(text, Rect::new(x, y + shown_heading, TEXT_WIDTH, message_h));
    p.clip_clear();
    height
}

/// Small segmented circular activity mark. The painter intentionally has no arc primitive; twelve
/// rounded segments give the banner a device-independent circle without allocating a texture or
/// disturbing the notification's card geometry. The severity colour and glyph continue to identify
/// the alert type, while this ring communicates that the banner is active.
fn draw_activity_ring(p: Painter, icon: Rect, color: [f32; 4], phase: f32, alpha: f32) {
    const SEGMENTS: usize = 12;
    const RADIUS: f32 = 24.0;
    const DOT: f32 = 3.2;
    let center_x = icon.x + icon.w * 0.5;
    let center_y = icon.y + icon.h * 0.5;
    let first = ((phase.rem_euclid(1.0) * SEGMENTS as f32).floor() as usize) % SEGMENTS;
    for i in 0..SEGMENTS {
        let angle = (i as f32 / SEGMENTS as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let x = center_x + angle.cos() * RADIUS - DOT * 0.5;
        let y = center_y + angle.sin() * RADIUS - DOT * 0.5;
        let trail = (i + SEGMENTS - first) % SEGMENTS;
        let strength = (1.0 - trail as f32 / SEGMENTS as f32) * 0.75 + 0.25;
        let mut ink = color;
        ink[3] *= alpha.clamp(0.0, 1.0) * strength;
        p.rrect(Rect::new(x, y, DOT, DOT), DOT * 0.5, DOT * 0.5, ink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn banner() -> Banner {
        Banner { id: "one".into(), title: "Front door".into(), message: "Motion detected".into(),
            severity: Severity::Info, started_ms: 1000, expires_ms: 6000, offset_px: 3, image: None }
    }

    #[test]
    fn enters_expands_and_exits_right_before_exact_expiry() {
        let b = banner();
        let first = phase(&b, 1000, false).unwrap();
        let settled = phase(&b, 1480, false).unwrap();
        let leaving = phase(&b, 5900, false).unwrap();
        assert!(card_rect(WIDTH, 140.0, 54.0, 3, first).x >= Rect::FULL.w);
        assert!(first.aperture < settled.aperture);
        assert_eq!(settled.slide, 0.0);
        assert!(leaving.slide > 0.0 && leaving.aperture < 1.0);
        assert!(phase(&b, 999, false).is_none());
        assert!(phase(&b, 6000, false).is_none());
        assert!(phase(&b, u64::MAX, false).is_none());
    }

    #[test]
    fn surface_and_icon_hold_500ms_after_entry_but_message_never_dims() {
        let b = banner();
        for dwell in [499, 500] {
            let boundary = phase(&b, b.started_ms + ENTER_MS + dwell, false).unwrap();
            assert_eq!(boundary.surface, 1.0);
            assert_eq!(boundary.heading, 1.0);
            assert_eq!(boundary.text, 1.0);
        }
        let before = phase(&b, b.started_ms + DIM_AT_MS, false).unwrap();
        let beginning = phase(&b, b.started_ms + DIM_AT_MS + 1, false).unwrap();
        assert!(beginning.surface < 1.0 && beginning.heading < 1.0);
        let after = phase(&b, 4000, false).unwrap();
        assert_eq!(before.surface, 1.0);
        assert!(after.surface < before.surface);
        assert_eq!(after.text, 1.0);
        assert_eq!(after.heading, 0.0);
        assert!(phase(&b, 2300, false).unwrap().heading > 0.0);
    }

    #[test]
    fn activity_indicator_is_a_full_circle_and_advances() {
        let icon = Rect::new(20.0, 20.0, ICON, ICON);
        let (mut left, mut top, mut right, mut bottom) =
            (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for i in 0..12 {
            let angle = i as f32 / 12.0 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
            let x = icon.x + icon.w * 0.5 + angle.cos() * 24.0 - 1.6;
            let y = icon.y + icon.h * 0.5 + angle.sin() * 24.0 - 1.6;
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + 3.2);
            bottom = bottom.max(y + 3.2);
        }
        assert!(right - left >= 45.0 && bottom - top >= 45.0,
            "segments should surround the glyph: {left},{top}..{right},{bottom}");
        let before = phase(&banner(), 1_001, false).unwrap().ring;
        let after = phase(&banner(), 1_251, false).unwrap().ring;
        assert!(after > before, "ring phase must advance while the banner is visible");
    }

    #[test]
    fn reduced_motion_keeps_static_full_content_until_expiry() {
        let b = banner();
        for now in [1000, 1500, 4000, 5999] {
            let s = phase(&b, now, true).unwrap();
            assert_eq!((s.slide, s.aperture, s.heading, s.surface, s.text), (0.0, 1.0, 1.0, 1.0, 1.0));
        }
        assert!(phase(&b, 6000, true).is_none());
    }

    #[test]
    fn offset_is_stable_and_clamped_inside_safe_area() {
        let s = phase(&banner(), 1500, false).unwrap();
        for offset in [0, 1, 5, 6, 255] {
            let r = card_rect(WIDTH, 140.0, plx_ui::consts::MARGIN_Y, offset, s);
            assert_eq!(r.y, plx_ui::consts::MARGIN_Y + offset.min(5) as f32);
            assert_eq!(r.x + r.w, Rect::FULL.w - plx_ui::consts::MARGIN_X);
        }
    }

    #[derive(Default)]
    struct FakeGpu { uploads: usize, frees: usize, live: usize }
    impl Uploader for FakeGpu {
        fn upload(&mut self, d: &Decoded) -> Tex {
            self.uploads += 1; self.live += 1; assert!(self.live <= 3);
            Tex { id: self.uploads as u32, w: d.w, h: d.h }
        }
        fn warm(&mut self, _: Tex) {}
        fn free(&mut self, _: Tex) { self.frees += 1; self.live -= 1; }
    }
    fn image(w: u16, h: u16) -> Arc<Decoded> {
        Arc::new(Decoded { w, h, rgba: vec![255; w as usize * h as usize * 4].into_boxed_slice() })
    }

    #[test]
    fn one_image_upload_is_reused_then_freed_on_replacement_and_expiry() {
        let mut b = banner(); b.image = Some(image(640, 360));
        let mut renderer = Renderer::default(); let mut gpu = FakeGpu::default();
        renderer.sync(std::slice::from_ref(&b), 1500, &mut gpu);
        renderer.sync(std::slice::from_ref(&b), 1600, &mut gpu);
        assert_eq!(gpu.uploads, 1);
        b.id = "two".into(); b.image = Some(image(320, 180));
        renderer.sync(std::slice::from_ref(&b), 1700, &mut gpu);
        assert_eq!((gpu.uploads, gpu.frees, gpu.live), (2, 1, 1));
        renderer.sync(std::slice::from_ref(&b), 6000, &mut gpu);
        assert_eq!((gpu.frees, gpu.live), (2, 0));
    }

    #[test]
    fn invalid_pixels_are_never_uploaded_and_image_geometry_preserves_all_edges() {
        let mut b = banner();
        let mut renderer = Renderer::default(); let mut gpu = FakeGpu::default();
        for decoded in [Decoded { w: 0, h: 1, rgba: vec![].into() },
            Decoded { w: 641, h: 1, rgba: vec![0; 641 * 4].into() },
            Decoded { w: 2, h: 2, rgba: vec![0; 15].into() }] {
            b.image = Some(Arc::new(decoded));
            renderer.sync(std::slice::from_ref(&b), 1500, &mut gpu);
        }
        assert_eq!(gpu.uploads, 0);
        for (w, h) in [(640, 360), (320, 240), (100, 300)] {
            let (dw, dh) = image_size(&image(w, h));
            assert!(dw <= WIDTH && dh <= 270.0);
            assert!((dw / dh - w as f32 / h as f32).abs() < 0.001);
        }
    }

    #[test]
    fn recording_draw_keeps_message_readable_and_removes_folded_heading() {
        let _guard = plx_base::testlock::serial();
        let mut renderer = Renderer::default();
        let b = banner();
        let paint = plx_ui::draw_census::capture_runs(|| renderer.draw(std::slice::from_ref(&b),
            4000, 0.2, false, Painter::recording(), &plx_machine::machine::BareMeasure));
        assert_eq!(paint.len(), 1);
        assert_eq!(paint[0].text, "Motion detected");
        assert_eq!(paint[0].ink[3], 1.0);
        assert!(renderer.resident.is_empty(), "recording cannot allocate a texture");
        assert!(!needs_frame(&[b], 6000));
    }

    #[test]
    fn camera_draws_one_complete_image_without_redundant_text() {
        let _guard = plx_base::testlock::serial();
        let mut b = banner(); b.image = Some(image(640, 360));
        let mut renderer = Renderer::default(); let mut gpu = FakeGpu::default();
        renderer.sync(std::slice::from_ref(&b), 1500, &mut gpu);
        let mut runs = Vec::new();
        let textures = plx_ui::draw_census::capture_tex(|| {
            runs = plx_ui::draw_census::capture_runs(|| renderer.draw(std::slice::from_ref(&b),
                1500, 0.7, false, Painter::recording(), &plx_machine::machine::BareMeasure));
        });
        assert!(runs.is_empty());
        assert_eq!(textures.len(), 1);
        assert_eq!((textures[0].1.w, textures[0].1.h), (480.0, 270.0));
        b.image = None;
        let fallback = plx_ui::draw_census::capture_runs(|| renderer.draw(std::slice::from_ref(&b),
            1500, 0.7, false, Painter::recording(), &plx_machine::machine::BareMeasure));
        assert!(fallback.iter().any(|r| r.text == "Motion detected"));
        renderer.release(&mut gpu);
    }

    #[test]
    fn three_camera_snapshot_stays_image_only_with_bounded_reuse_and_retirement() {
        let _guard = plx_base::testlock::serial();
        let mut banners: Vec<_> = (0..4).map(|i| {
            let mut b = banner(); b.id = i.to_string(); b.image = Some(image(640, 360)); b
        }).collect();
        let mut renderer = Renderer::default(); let mut gpu = FakeGpu::default();
        renderer.sync(&banners, 1500, &mut gpu);
        assert_eq!((gpu.uploads, gpu.live), (3, 3));
        let mut runs = Vec::new();
        let textures = plx_ui::draw_census::capture_tex(|| {
            runs = plx_ui::draw_census::capture_runs(|| renderer.draw(&banners,
                1500, 0.7, false, Painter::recording(), &plx_machine::machine::BareMeasure));
        });
        assert!(runs.is_empty(), "a valid earlier camera must never turn into text");
        assert_eq!(textures.len(), 3);
        for (_, rect, _) in &textures {
            assert_eq!((rect.w, rect.h), (480.0, 270.0));
            assert!(rect.y + rect.h <= Rect::FULL.h - plx_ui::consts::MARGIN_Y);
        }
        banners.swap(0, 1);
        renderer.sync(&banners, 1600, &mut gpu);
        assert_eq!((gpu.uploads, gpu.frees), (3, 0));
        banners[0].image = Some(image(320, 180));
        renderer.sync(&banners, 1700, &mut gpu);
        assert_eq!((gpu.uploads, gpu.frees, gpu.live), (4, 1, 3));
        renderer.sync(&[], 1800, &mut gpu);
        renderer.release(&mut gpu);
        assert_eq!((gpu.frees, gpu.live), (4, 0), "retirement must free each texture exactly once");
    }

    #[test]
    fn recording_image_snapshot_and_empty_clear_need_no_graphics_context() {
        let _guard = plx_base::testlock::serial();
        let mut b = banner(); b.image = Some(image(640, 360));
        let mut renderer = Renderer::default();
        renderer.draw(&[b], 1500, 0.7, false, Painter::recording(), &plx_machine::machine::BareMeasure);
        assert!(renderer.resident.is_empty());
        renderer.clear();
        drop(renderer);
    }

    #[test]
    fn stack_is_bounded_and_expired_cards_leave_no_painted_pixels() {
        let _guard = plx_base::testlock::serial();
        let mut renderer = Renderer::default();
        let banners: Vec<_> = (0..4).map(|i| {
            let mut b = banner(); b.id = i.to_string();
            b.title = "A very long title ".repeat(7);
            b.message = "A long message whose lines must remain inside the card ".repeat(9);
            b
        }).collect();
        let rects = plx_ui::draw_census::capture_inks(|| renderer.draw(&banners,
            1500, 0.7, false, Painter::recording(), &plx_machine::machine::BareMeasure));
        let cards: Vec<_> = rects.iter().filter(|(_, r, _)| r.w == WIDTH).collect();
        assert_eq!(cards.len(), 3);
        for (_, r, alpha) in &cards {
            assert!(r.y >= plx_ui::consts::MARGIN_Y);
            assert!(r.y + r.h <= Rect::FULL.h - plx_ui::consts::MARGIN_Y);
            assert!((*alpha - 0.624).abs() < 0.0001);
        }
        for pair in cards.windows(2) { assert!(pair[0].1.y + pair[0].1.h < pair[1].1.y); }
        let gone = plx_ui::draw_census::capture_inks(|| renderer.draw(&banners,
            6000, 0.7, false, Painter::recording(), &plx_machine::machine::BareMeasure));
        assert!(gone.is_empty());
    }
}
