use super::model::{HudModel, OsdOpacity, PermanentClock};
use super::{classic, modern, osd, ott_footer};
use plx_data::metadata::{NowPlaying, OttNowPlaying};
use plx_machine::machine::{FontScale, TypographyMeasure};
use plx_ui::fixture::FixtureMeasure;
use plx_ui::{draw_census, theme, Painter};

fn playing() -> NowPlaying {
    NowPlaying { ott: Some(OttNowPlaying { live: None, archive: None,
        collection_label: "Synthetic collection".into(), confirmed_start_unix_ms: None,
        next: None, art: None }), is_episode: false, is_real_episode: false,
        title: "Synthetic title".into(), ep_title: String::new(), season: 0, index: 0,
        summary: "Synthetic synopsis".into(), year: 0, dur_ms: 60_000,
        rating: String::new(), thumb: String::new(), detail_rk: String::new() }
}

#[test]
fn opacity_levels_are_checked_and_seven_preserves_native_reference_alphas() {
    assert_eq!(OsdOpacity::default().level(), 7);
    assert!(OsdOpacity::from_level(11).is_none());
    assert!(OsdOpacity::from_level(u8::MAX).is_none());
    for base in [0.0_f32, 0.14, 0.28, 0.78, 0.86, 0.985, 1.0] {
        assert_eq!(OsdOpacity::default().background_alpha(base).to_bits(), base.to_bits());
        let mut previous = 0.0;
        for level in 0..=10 {
            let opacity = OsdOpacity::from_level(level).unwrap();
            assert_eq!(opacity.level(), level);
            let alpha = opacity.background_alpha(base);
            assert!((0.0..=1.0).contains(&alpha));
            assert!(alpha >= previous);
            if level == 0 { assert_eq!(alpha, 0.0); }
            previous = alpha;
        }
    }
}

#[test]
fn clock_modes_are_closed_and_default_off() {
    assert_eq!(PermanentClock::default(), PermanentClock::Off);
    for (index, mode) in [(0, PermanentClock::Off), (1, PermanentClock::Background),
        (2, PermanentClock::Transparent)] {
        assert_eq!(PermanentClock::from_index(index), Some(mode));
        assert_eq!(mode.index(), index);
    }
    assert!(PermanentClock::from_index(3).is_none());
    assert!(PermanentClock::from_index(u64::MAX).is_none());
    let model = HudModel::default();
    assert_eq!(model.osd_opacity.level(), 7);
    assert_eq!(model.permanent_clock, PermanentClock::Off);
}

#[test]
fn modern_default_panel_matches_shared_sheet_and_opacity_does_not_change_text() {
    use super::state::{ControlSlot, Controls};
    let item = playing();
    let motion = super::motion::HudMotion::new();
    let slot = ControlSlot::Discs(Controls::NONE);
    let layout = modern::compose(classic::geometry(slot, &motion, 1, &FixtureMeasure),
        slot, &motion, None, "", Some(&item), None, &FixtureMeasure);
    let baseline = draw_census::capture_inks(|| plx_ui::widgets::panel_ground(
        Painter::recording(), layout.panel, theme::ALERT_PANEL_RAD, None,
        plx_ui::widgets::Ground::Sheet));
    let normal = draw_census::capture_inks(|| modern::draw_panel(
        Painter::recording(), &layout, OsdOpacity::default()));
    assert_eq!(normal, baseline);
    assert_eq!(normal.len(), 1);
    let text = draw_census::capture_runs(|| modern::draw_information(Painter::recording(),
        &layout, Some(&item), None, &mut ott_footer::ClockLabels::default(), &FixtureMeasure));
    assert!(!text.is_empty());
    for level in [0, 3, 7, 10] {
        let opacity = OsdOpacity::from_level(level).unwrap();
        let ink = draw_census::capture_inks(|| modern::draw_panel(Painter::recording(), &layout, opacity));
        assert_eq!(ink.len(), 1);
        assert_eq!(ink[0].0, baseline[0].0);
        assert_eq!(ink[0].1, baseline[0].1);
        assert_eq!(ink[0].2, opacity.background_alpha(baseline[0].2));
        let rendered = draw_census::capture_runs(|| {
            modern::draw_panel(Painter::recording(), &layout, opacity);
            modern::draw_information(Painter::recording(), &layout, Some(&item), None, &mut ott_footer::ClockLabels::default(), &FixtureMeasure);
        });
        assert_eq!(rendered, text);
    }
}

#[test]
fn classic_opacity_does_not_change_information_ink_or_layout() {
    use plx_data::stores::metadata::{MetadataCmd, MetadataStore};
    use plx_machine::machine::UiMode;
    let _guard = plx_base::testlock::serial();
    let item = playing();
    let slot = super::ControlSlot::Discs(super::Controls::NONE);
    let layout = classic::ott_classic_layout(slot,
        &super::motion::HudMotion::new(), 1, Some(&item), &FixtureMeasure);
    let mut store = MetadataStore::default();
    store.run(MetadataCmd::SetNowPlaying(Some(item)));
    // The compact background belongs to the compositor, not the information
    // helper. Exercise that owner so changing its opacity cannot dim the text.
    let render = |opacity| {
        let model = HudModel { osd_opacity: opacity, duration_ns: 60_000_000_000,
            playpos_ns: 12_000_000_000, ..HudModel::default() };
        let mut motion = super::motion::HudMotion::new();
        plx_ui::record_walk(|| classic::draw_hud_in_mode(
            plx_media::route::idle_session_for_test(), &model, &mut motion,
            &crate::up_next::Countdown::default(), slot, super::Busy::None,
            super::geometry::GROUP_SCRUB, super::BTN_SUBTITLES, 0, 1_000, true,
            &FixtureMeasure, store.view(), UiMode::Classic));
    };
    let baseline = draw_census::capture_runs(|| render(OsdOpacity::default()));
    for expected in ["Synthetic collection", "Synthetic title"] {
        assert!(baseline.iter().any(|run| run.text == expected));
    }
    for level in [0, 3, 7, 10] {
        let opacity = OsdOpacity::from_level(level).unwrap();
        assert_eq!(draw_census::capture_runs(|| render(opacity)), baseline);
        let ink = draw_census::capture_rect_inks(|| render(opacity));
        let panel: Vec<_> = ink.iter().filter(|(rect, _, _)| *rect == layout.panel).collect();
        assert_eq!(panel.len(), 1, "the real compact background must be drawn once");
        assert_eq!(panel[0].1, opacity.background_alpha(0.78));
        assert_eq!(panel[0].2, opacity.background_alpha(0.78));
    }
}

#[test]
fn permanent_clock_off_or_unknown_time_has_no_draws() {
    let mut clock = osd::Clock::default();
    for (mode, sample) in [(PermanentClock::Off, Some(1_700_000_123)),
        (PermanentClock::Background, None), (PermanentClock::Transparent, Some(0)),
        (PermanentClock::Background, Some(-1))] {
        let model = HudModel { permanent_clock: mode, wall_now_s: sample, ..HudModel::default() };
        let drawn = draw_census::capture(|| assert_eq!(clock.draw(Painter::recording(),
            &model, &FixtureMeasure), None));
        assert!(drawn.is_empty());
    }
}

#[test]
fn permanent_clock_background_changes_without_dimming_or_moving_text() {
    let mut clock = osd::Clock::default();
    let mut model = HudModel { permanent_clock: PermanentClock::Transparent,
        wall_now_s: Some(1_700_000_123), ..HudModel::default() };
    let reference = draw_census::capture_runs(|| {
        assert!(clock.draw(Painter::recording(), &model, &FixtureMeasure).is_some());
    });
    assert_eq!(reference.len(), 1);
    assert!(!reference[0].text.is_empty());
    for level in [0, 3, 7, 10] {
        model.osd_opacity = OsdOpacity::from_level(level).unwrap();
        model.permanent_clock = PermanentClock::Background;
        let drawn = draw_census::capture_runs(|| { let _ = clock.draw(Painter::recording(), &model, &FixtureMeasure); });
        assert_eq!(drawn, reference);
        let ink = draw_census::capture_inks(|| { let _ = clock.draw(Painter::recording(), &model, &FixtureMeasure); });
        assert_eq!(ink.len(), 1);
        assert_eq!(ink[0].2, model.osd_opacity.background_alpha(theme::PANEL_TOP[3]));
    }
    model.permanent_clock = PermanentClock::Transparent;
    assert!(draw_census::capture_inks(|| { let _ = clock.draw(Painter::recording(), &model, &FixtureMeasure); }).is_empty());
}

#[test]
fn permanent_clock_keeps_complete_text_inside_safe_area_at_large_font() {
    let mut clock = osd::Clock::default();
    let model = HudModel { permanent_clock: PermanentClock::Background,
        wall_now_s: Some(1_700_000_123), ..HudModel::default() };
    for percent in [100, 150, 200] {
        let mut measure = TypographyMeasure::new(FixtureMeasure);
        measure.set_font_scale(FontScale::from_percent(percent).unwrap());
        let mut bounds = None;
        let runs = draw_census::capture_runs(|| { bounds = clock.draw(Painter::recording(), &model, &measure); });
        let bounds = bounds.unwrap();
        let safe = plx_ui::consts::SAFE;
        assert!(bounds.x >= safe.x && bounds.y >= safe.y);
        assert!(bounds.x + bounds.w <= safe.x + safe.w);
        assert!(bounds.y + bounds.h <= safe.y + safe.h);
        assert_eq!(runs.len(), 1);
        assert!(!runs[0].text.contains('…'));
    }
}

#[test]
fn permanent_clock_key_and_display_share_minute_and_absence_rules() {
    let mut clock = osd::Clock::default();
    let mut model = HudModel { permanent_clock: PermanentClock::Transparent,
        wall_now_s: Some(1_700_000_100), ..HudModel::default() };
    let paint = |clock: &mut osd::Clock, model: &HudModel| draw_census::capture_runs(|| {
        assert!(clock.draw(Painter::recording(), model, &FixtureMeasure).is_some());
    });
    let key = osd::clock_minute_key(&model).unwrap();
    let before = paint(&mut clock, &model);
    model.wall_now_s = Some(1_700_000_101);
    assert_eq!(osd::clock_minute_key(&model), Some(key));
    assert_eq!(paint(&mut clock, &model), before);
    model.wall_now_s = Some(1_700_000_160);
    assert_eq!(osd::clock_minute_key(&model), Some(key + 1));
    let after = paint(&mut clock, &model);
    assert_ne!(after[0].text, before[0].text);
    for invalid in [None, Some(-1), Some(0), Some(253_402_300_800), Some(i64::MAX)] {
        model.wall_now_s = invalid;
        assert_eq!(osd::clock_minute_key(&model), None);
        assert_eq!(clock.draw(Painter::recording(), &model, &FixtureMeasure), None);
    }
    model.wall_now_s = Some(1_700_000_100);
    model.permanent_clock = PermanentClock::Off;
    assert_eq!(osd::clock_minute_key(&model), None);
}
