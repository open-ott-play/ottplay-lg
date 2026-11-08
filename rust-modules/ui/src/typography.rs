//! One UI typography scale for measurement, paint, wrapping and reading flow.
//! The application applies it at a frame boundary; glyph caches use physical
//! sizes and media captions explicitly retain their own sizing.
pub use plx_machine::machine::{FontScale, TypographyMeasure};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidFontScale;

pub fn apply<M>(percent: u16, measure: &mut TypographyMeasure<M>) -> Result<bool, InvalidFontScale> {
    let scale = FontScale::from_percent(percent).ok_or(InvalidFontScale)?;
    let changed = measure.scale() != scale || plx_gfx::text::ui_typography() != scale;
    measure.set_font_scale(scale);
    plx_gfx::text::set_ui_typography(scale);
    if changed { plx_machine::idle::invalidate(); }
    Ok(changed)
}

pub fn current() -> FontScale { plx_gfx::text::ui_typography() }
pub fn text_cap_band(sz: i32, bold: i32) -> (f32, f32) { plx_gfx::text::text_cap_band(current().pixels(sz), bold) }
pub fn text_height(sz: i32, bold: i32) -> f32 { plx_gfx::text::text_height(current().pixels(sz), bold) }
pub fn text_vcenter_y(sz: i32, bold: i32, cy: f32) -> f32 { plx_gfx::text::text_vcenter_y(current().pixels(sz), bold, cy) }
pub fn baseline_y(sz: i32, bold: i32, on_sz: i32, on_bold: i32, on_y: f32) -> f32 {
    let scale = current();
    plx_gfx::text::baseline_y(scale.pixels(sz), bold, scale.pixels(on_sz), on_bold, on_y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_machine::machine::Measure;
    use std::{cell::Cell, ffi::CStr};

    struct Physical;
    impl Measure for Physical {
        fn width(&self, text: &CStr, size: i32, _: bool) -> f32 { text.to_bytes().len() as f32 * size as f32 * 0.5 }
        fn cap_h(&self, size: i32) -> f32 { size as f32 * 0.75 }
        fn line_h(&self, size: i32) -> f32 { size as f32 }
    }
    struct Restore(FontScale);
    impl Drop for Restore { fn drop(&mut self) { plx_gfx::text::set_ui_typography(self.0); } }

    #[test]
    fn default_is_identity_and_invalid_setting_is_atomic() {
        let _restore = Restore(current());
        let mut measure = TypographyMeasure::new(Physical);
        apply(100, &mut measure).unwrap();
        assert_eq!(measure.scale(), FontScale::DEFAULT);
        assert_eq!(measure.width(c"Play", 28, false), Physical.width(c"Play", 28, false));
        assert_eq!(measure.scale().pixels(72), 72);
        assert!(!apply(100, &mut measure).unwrap());
        for percent in [0, 19, 201, u16::MAX] { assert_eq!(apply(percent, &mut measure), Err(InvalidFontScale)); }
        assert_eq!(current(), FontScale::DEFAULT);
        assert_eq!(measure.scale(), FontScale::DEFAULT);
    }

    #[test]
    fn full_supported_range_changes_measure_and_paint_without_changing_default() {
        let _restore = Restore(current());
        let mut measure = TypographyMeasure::new(Physical);
        assert_eq!(FontScale::DEFAULT.percent(), 100);
        for (percent, pixels) in [(20, 6), (200, 56)] {
            apply(percent, &mut measure).unwrap();
            assert_eq!(measure.scale().pixels(28), pixels);
            assert_eq!(current().percent(), percent);
            assert_eq!(measure.width(c"Play", 28, false), 2.0 * pixels as f32);
            assert_eq!(measure.unscaled_width(c"Play", 28, false), 56.0);
        }
    }

    #[test]
    fn recorder_sees_physical_sizes_and_caption_width_remains_independent() {
        let _restore = Restore(current());
        static PHYSICAL: Physical = Physical;
        let mut measure = TypographyMeasure::new(crate::rec::Measurements::record(&PHYSICAL));
        apply(150, &mut measure).unwrap();
        assert_eq!(measure.width(c"Play", 28, false), 84.0);
        assert_eq!(measure.unscaled_width(c"Play", 28, false), 56.0);
        let rows = measure.drain().unwrap();
        assert!(rows.iter().any(|(key,_)| matches!(key, crate::rec::MetricKey::Width {sz:42,..})));
        assert!(rows.iter().any(|(key,_)| matches!(key, crate::rec::MetricKey::Width {sz:28,..})));
        let mut replay = TypographyMeasure::new(crate::rec::TableMeasure::new(rows.into_iter().collect()));
        replay.set_font_scale(FontScale::from_percent(150).unwrap());
        assert_eq!(replay.width(c"Play", 28, false), 84.0);
        assert_eq!(replay.unscaled_width(c"Play", 28, false), 56.0);
        assert_eq!(replay.take_miss(), None);
    }

    #[test]
    fn painter_records_exactly_one_scale_and_caption_opts_out() {
        let _restore = Restore(current());
        let mut measure = TypographyMeasure::new(Physical);
        apply(150, &mut measure).unwrap();
        let runs = crate::draw_census::capture_runs(|| {
            crate::Painter::recording().text(c"Play".as_ptr(), 0.0, 0.0, 28, crate::theme::TEXT_PRIMARY, 0, 0);
            crate::Painter::recording().unscaled_text().text(c"Caption".as_ptr(), 0.0, 0.0, 28, crate::theme::TEXT_PRIMARY, 0, 0);
        });
        assert_eq!(runs.iter().map(|run|run.size).collect::<Vec<_>>(), [42,28]);
        assert_eq!(crate::Painter::root().font_scale().pixels(72), 108);
    }

    #[test]
    fn live_wrapping_recomputes_at_new_scale_and_leading_matches_it() {
        let _restore = Restore(current());
        struct Counting(Cell<usize>);
        impl Measure for Counting {
            fn width(&self, text: &CStr, size: i32, bold: bool) -> f32 {
                self.0.set(self.0.get()+1); Physical.width(text,size,bold)
            }
            fn cap_h(&self, size: i32) -> f32 { Physical.cap_h(size) }
            fn line_h(&self, size: i32) -> f32 { Physical.line_h(size) }
            fn live_font(&self) -> bool { true }
        }
        let mut measure=TypographyMeasure::new(Counting(Cell::new(0)));
        let body="Scaling retains every word and recomputes the complete wrapped paragraph.";
        apply(100,&mut measure).unwrap();
        let before=crate::text_view::TextView::new(body,28,crate::theme::TEXT_PRIMARY)
            .leading(40.0).with_measure(&measure).measure_h(280.0);
        let calls=measure.0.get();
        apply(150,&mut measure).unwrap();
        let view=crate::text_view::TextView::new(body,28,crate::theme::TEXT_PRIMARY)
            .leading(40.0).with_measure(&measure);
        assert_eq!(view.line_h(),60.0);
        assert!(view.measure_h(280.0)>before);
        assert!(measure.0.get()>calls);
    }
}
