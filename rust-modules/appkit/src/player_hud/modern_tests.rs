use super::*;
use plx_machine::machine::{FontScale, TypographyMeasure};
use plx_ui::fixture::FixtureMeasure;

fn playing(title: &str) -> plx_data::metadata::NowPlaying {
    plx_data::metadata::NowPlaying { ott: None, is_episode: false, is_real_episode: false,
        title: title.into(), ep_title: String::new(), season: 0, index: 0,
        summary: String::new(), year: 0, dur_ms: 0, rating: String::new(),
        thumb: String::new(), detail_rk: String::new() }
}

fn contains(outer: Rect, inner: Rect) -> bool {
    inner.x >= outer.x - 0.01 && inner.y >= outer.y - 0.01
        && inner.x + inner.w <= outer.x + outer.w + 0.01
        && inner.y + inner.h <= outer.y + outer.h + 0.01
}

#[test]
fn focused_control_is_fully_visible_at_each_end_of_a_narrow_rail() {
    let controls = Controls::for_item(true, true, true, false);
    let motion = HudMotion::new();
    for percent in [100, 150, 200] {
        let mut measure = TypographyMeasure::new(FixtureMeasure);
        measure.set_font_scale(FontScale::from_percent(percent).unwrap());
        for width in [375.0, 720.0, 1728.0] {
            for focus in 0..BTN_N {
                let rail = Rect::new(96.0, 720.0, width, 112.0);
                let items = control_rects(controls, &motion, Some(focus),
                    "A deliberately long localized action caption", rail, &measure);
                assert!(contains(rail, items[focus as usize].unwrap().scaled(motion.scale(focus))));
                assert!(items.iter().all(Option::is_some));
                for pair in items.windows(2) {
                    let a = pair[0].unwrap(); let b = pair[1].unwrap();
                    assert!(a.x + a.w <= b.x);
                }
            }
        }
    }
}

#[test]
fn absent_episode_controls_keep_their_existing_identity_slots() {
    let motion = HudMotion::new();
    let items = control_rects(Controls::NONE, &motion, Some(BTN_AUDIO), "Audio",
        Rect::new(96.0,720.0,1728.0,112.0), &FixtureMeasure);
    assert_eq!(items[0], None); assert_eq!(items[1], None);
    for i in BTN_SUBTITLES..BTN_N { assert!(items[i as usize].is_some()); }
    assert_eq!(Controls::NONE.command(BTN_AUDIO), Some(super::super::command::HudCommand::OpenTracks(super::super::command::TrackTab::Audio)));
}

#[test]
fn measured_information_never_crosses_the_rail_at_large_type() {
    let mut playing = playing("");
    playing.title = "A very long programme title that remains bounded to its own measured information region even at the largest supported font size".into();
    playing.summary = "An equally long description must not displace or obscure any transport action, progress line or bottom tab.".into();
    let motion = HudMotion::new();
    for percent in [100, 150, 200] {
        let mut measure = TypographyMeasure::new(FixtureMeasure);
        measure.set_font_scale(FontScale::from_percent(percent).unwrap());
        let base = super::super::classic::geometry(ControlSlot::Discs(Controls::NONE), &motion, 1, &measure);
        let plan = compose(base, ControlSlot::Discs(Controls::NONE), &motion,
            Some(BTN_AUDIO), "Audio", Some(&playing), None, &measure);
        assert!(plan.title.y >= plx_ui::consts::MARGIN_Y);
        assert!(plan.title.y + plan.title.h <= plan.detail.y + 0.01);
        assert!(plan.detail.y + plan.detail.h < plan.geometry.row.y);
        assert!(contains(plan.panel, plan.title));
        assert!(contains(plan.panel, plan.geometry.row));
        assert_eq!(plan.geometry.playbar, base.playbar);
        assert_eq!(plan.geometry.tabs, base.tabs);
    }
}

#[test]
fn missing_metadata_does_not_reserve_empty_information_rows() {
    let motion = HudMotion::new();
    let slot = ControlSlot::Discs(Controls::NONE);
    let base = super::super::classic::geometry(slot, &motion, 1, &FixtureMeasure);
    let plan = compose(base, slot, &motion, None, "", None, None, &FixtureMeasure);
    assert_eq!(plan.title.h, 0.0);
    assert_eq!(plan.context.h, 0.0);
    assert_eq!(plan.detail.h, 0.0);
    assert!(plan.panel.y <= plan.geometry.row.y);
}

#[test]
fn classic_geometry_does_not_depend_on_modern_focus_or_metadata() {
    let motion = HudMotion::new(); let slot = ControlSlot::Discs(Controls::NONE);
    let a = super::super::classic::geometry_in_mode(slot, &motion, 1, &FixtureMeasure,
        plx_machine::machine::UiMode::Classic, None, None, None);
    let b = super::super::classic::geometry_in_mode(slot, &motion, 1, &FixtureMeasure,
        plx_machine::machine::UiMode::Classic, Some(BTN_MORE), Some(&playing("")), Some(123));
    assert_eq!(a,b);
}


#[test]
fn modern_timeline_requires_positive_decoder_duration_even_with_metadata() {
    let mut metadata = playing("A known title is not a seek duration");
    metadata.dur_ms = 60_000;
    for duration_ns in [-1, 0, 1, 60_000_000_000] {
        let model = HudModel { duration_ns, ..HudModel::default() };
        assert_eq!(shows_timeline(&model, Some(&metadata)), duration_ns > 0);
        assert_eq!(shows_timeline(&model, None), duration_ns > 0);
    }
}

#[test]
fn modern_composition_keeps_skip_and_up_next_action_geometry() {
    let marker = plx_data::metadata::Marker {
        start_ms: 0, end_ms: 10_000, kind: plx_data::metadata::MarkerKind::Intro,
        final_seg: false,
    };
    let motion = HudMotion::new();
    for slot in [ControlSlot::Skip(crate::skip_pill::prompt_for(marker)), ControlSlot::UpNext(marker)] {
        let base = super::super::classic::geometry(slot, &motion, 2, &FixtureMeasure);
        let metadata = playing("Current programme");
        let plan = compose(base, slot, &motion, Some(0), "", Some(&metadata), None, &FixtureMeasure);
        assert_eq!(plan.geometry.items, base.items);
        assert_eq!(plan.geometry.row, base.row);
        assert_eq!(plan.geometry.row_anchor, base.row_anchor);
        assert_eq!(plan.geometry.playbar, base.playbar);
        assert_eq!(plan.geometry.tabs, base.tabs);
        assert!(plan.title.y + plan.title.h < plan.geometry.row.y);
    }
}


#[test]
fn every_shipped_focused_caption_fits_with_its_actual_leading_icon() {
    let motion = HudMotion::new();
    for language in plx_platform::i18n::SHIPPED {
        let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
        for percent in [100, 150, 200] {
            let mut measure = TypographyMeasure::new(FixtureMeasure);
            measure.set_font_scale(FontScale::from_percent(percent).unwrap());
            for restart in [false, true] {
                let controls = Controls::for_item(true, true, true, restart);
                let slot = ControlSlot::Discs(controls);
                for index in 0..BTN_N {
                    let label = focused_label(slot, Some(index));
                    let base = super::super::classic::geometry(slot, &motion, 1, &measure);
                    let plan = compose(base, slot, &motion, Some(index), label, None, None, &measure);
                    let frame = plan.geometry.items[index as usize].unwrap();
                    let caption = CString::new(label).unwrap();
                    // Same actual widget/icon shape as draw_controls. The boolean
                    // denotes the leading icon; the None is a trailing accessory.
                    let button = Button::new(caption.as_ptr(), theme::size::CAPTION, frame)
                        .icon(controls.icon(index));
                    let required = Button::pill_w_measured(&caption, button.sz,
                        button.icon.is_some(), button.trailing, &measure);
                    assert!(required <= frame.w + 0.01, "caption {label:?} at {percent}%");
                    let empty = CString::default();
                    let air = Button::pill_w_measured(&empty, button.sz,
                        button.icon.is_some(), button.trailing, &measure);
                    let fitted = measure.fit_line(label, (frame.w - air).max(0.0), button.sz, true);
                    assert_eq!(fitted.to_str().unwrap(), label);
                    assert!(contains(plan.geometry.row, frame.scaled(motion.scale(index))));
                }
            }
        }
    }
}


#[test]
fn up_next_large_type_keeps_information_in_the_safe_region() {
    let marker = plx_data::metadata::Marker {
        start_ms: 0, end_ms: 10_000, kind: plx_data::metadata::MarkerKind::Credits,
        final_seg: true,
    };
    let slot = ControlSlot::UpNext(marker);
    let motion = HudMotion::new();
    let mut metadata = playing(&"Long current programme name ".repeat(16));
    metadata.summary = "Full description remains in the metadata; this excerpt yields space to the real Up Next controls.".into();
    for language in plx_platform::i18n::SHIPPED {
        let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
        for percent in [100, 150, 200] {
            let mut measure = TypographyMeasure::new(FixtureMeasure);
            measure.set_font_scale(FontScale::from_percent(percent).unwrap());
            let base = super::super::classic::geometry(slot, &motion, 2, &measure);
            let plan = compose(base, slot, &motion, Some(0), "", Some(&metadata), None, &measure);
            let next = crate::up_next::layout_peek(&motion, &measure);
            for line in [plan.context, plan.title, plan.detail] {
                if line.h > 0.0 {
                    assert!(line.y >= plx_ui::consts::MARGIN_Y, "{language:?}/{percent}: {line:?}");
                    assert!(line.y + line.h < next.still.y);
                }
            }
            assert_eq!(plan.geometry.items, base.items);
            assert_eq!(plan.geometry.row, base.row);
        }
    }
}


#[test]
fn focused_caption_survives_rounding_at_an_integer_pill_boundary() {
    struct FractionalMeasure;
    impl Measure for FractionalMeasure {
        fn width(&self, text: &std::ffi::CStr, _sz: i32, _bold: bool) -> f32 {
            if text.to_bytes().is_empty() { 0.0 } else { 148.40001 }
        }
        fn cap_h(&self, sz: i32) -> f32 { FixtureMeasure.cap_h(sz) }
        fn line_h(&self, sz: i32) -> f32 { FixtureMeasure.line_h(sz) }
    }
    let measure = FractionalMeasure;
    let label = "Fractional caption";
    let caption = CString::new(label).unwrap();
    let required = Button::pill_w_measured(&caption, theme::size::CAPTION, true, None, &measure);
    let air = Button::pill_w_measured(&CString::default(), theme::size::CAPTION, true, None, &measure);
    // A plain ceil would leave this integer boundary unchanged and still lose text.
    assert_eq!(required, required.ceil());
    assert!(required - air < measure.width(&caption, theme::size::CAPTION, true));
    let controls = Controls::for_item(true, true, true, false);
    let motion = HudMotion::new();
    let rail = Rect::new(96.0, 720.0, 1728.0, 112.0);
    let items = control_rects(controls, &motion, Some(BTN_AUDIO), label, rail, &measure);
    let frame = items[BTN_AUDIO as usize].unwrap();
    assert_eq!(measure.fit_line(label, (frame.w - air).max(0.0), theme::size::CAPTION, true).to_str().unwrap(), label);
    assert!(contains(rail, frame.scaled(motion.scale(BTN_AUDIO))));
}


fn footer_playing() -> NowPlaying {
    let mut value = playing("Current episode");
    value.ott = Some(plx_data::metadata::OttNowPlaying {
        live: None, archive: None, collection_label: "Series / Season 2".into(),
        confirmed_start_unix_ms: Some(1_000),
        next: Some(plx_data::metadata::OttNextPlaying {
            title: "Next episode".into(), estimated_start_unix_ms: Some(61_000),
        }),
        art: None,
    });
    value
}

#[test]
fn modern_footer_draws_typed_ordinal_and_live_clocks_then_retires_stale_guide() {
    let _serial = plx_base::testlock::serial();
    use plx_data::metadata::{OttGuideProgramme, OttLivePlaying, OttNowNext};
    let mut value = footer_playing();
    let programme = |title: &str, start_s, end_s| OttGuideProgramme {
        title: title.into(), description: String::new(), start_s, end_s,
    };
    value.ott.as_mut().unwrap().live = Some(OttLivePlaying {
        channel_title: "Channel A".into(), ordinal: Some(3450),
        guide: Some(OttNowNext { now_s: 150,
            current: Some(programme("Current programme", 100, 200)),
            next: Some(programme("Next programme", 200, 300)) }),
    });
    let motion = HudMotion::new();
    let slot = ControlSlot::Discs(Controls::NONE);
    let mut clocks = super::super::ott_footer::ClockLabels::default();
    for (now, title, next) in [(150, "Current programme", Some("Next programme")),
        (200, "Next programme", None), (300, "Channel A", None)] {
        let base = super::super::classic::geometry(slot, &motion, 1, &FixtureMeasure);
        let layout = compose(base, slot, &motion, None, "", Some(&value), Some(now), &FixtureMeasure);
        let runs = plx_ui::draw_census::capture_runs(|| draw_information(Painter::recording(),
            &layout, Some(&value), Some(now), &mut clocks, &FixtureMeasure));
        for expected in ["3450", "Channel A", title] {
            assert!(runs.iter().any(|run| run.text == expected), "missing {expected}: {runs:?}");
        }
        assert_eq!(clocks.current().is_empty(), now == 300);
        assert_eq!(clocks.next().is_empty(), next.is_none());
        for clock in [clocks.current(), clocks.next()].into_iter().filter(|s| !s.is_empty()) {
            assert!(runs.iter().any(|run| run.text == clock), "missing complete {clock}: {runs:?}");
        }
        if let Some(next) = next { assert!(runs.iter().any(|run| run.text == next)); }
        if now >= 200 { assert!(!runs.iter().any(|run| run.text == "Current programme")); }
        if now == 300 { assert!(!runs.iter().any(|run| run.text == "Next programme")); }
        assert!(!runs.iter().any(|run| run.text == "Current episode" || run.text == "Next episode"));
        assert_eq!(layout.geometry.playbar, base.playbar);
        assert_eq!(layout.geometry.tabs, base.tabs);
    }
    let live = value.ott.as_mut().unwrap().live.as_mut().unwrap();
    live.ordinal = None; live.guide = None; live.channel_title = "123 Channel A".into();
    let base = super::super::classic::geometry(slot, &motion, 1, &FixtureMeasure);
    let layout = compose(base, slot, &motion, None, "", Some(&value), None, &FixtureMeasure);
    let runs = plx_ui::draw_census::capture_runs(|| draw_information(Painter::recording(),
        &layout, Some(&value), None, &mut clocks, &FixtureMeasure));
    assert!(clocks.current().is_empty() && clocks.next().is_empty());
    assert!(runs.iter().any(|run| run.text == "123 Channel A"));
    assert!(!runs.iter().any(|run| run.text == "3450" || run.text == "123"));
}

#[test]
fn modern_footer_keeps_episode_identity_and_only_confirmed_finite_clocks() {
    let _serial = plx_base::testlock::serial();
    let mut value = footer_playing();
    value.title = "Series title".into(); value.ep_title = "Current episode".into();
    value.is_episode = true; value.is_real_episode = true;
    let motion = HudMotion::new(); let slot = ControlSlot::Discs(Controls::NONE);
    let base = super::super::classic::geometry(slot, &motion, 1, &FixtureMeasure);
    let mut clocks = super::super::ott_footer::ClockLabels::default();
    for known in [true, false] {
        if !known {
            let info = value.ott.as_mut().unwrap(); info.confirmed_start_unix_ms = None;
            info.next.as_mut().unwrap().estimated_start_unix_ms = None;
        }
        let layout = compose(base, slot, &motion, None, "", Some(&value), None, &FixtureMeasure);
        let runs = plx_ui::draw_census::capture_runs(|| draw_information(Painter::recording(),
            &layout, Some(&value), None, &mut clocks, &FixtureMeasure));
        for expected in ["Series / Season 2", "Current episode", "Next episode"] {
            assert!(runs.iter().any(|run| run.text == expected), "missing {expected}: {runs:?}");
        }
        assert!(!runs.iter().any(|run| run.text == "Series title"));
        assert_eq!(clocks.current().is_empty(), !known);
        assert_eq!(clocks.next().is_empty(), !known);
        for clock in [clocks.current(), clocks.next()].into_iter().filter(|s| !s.is_empty()) {
            assert!(runs.iter().any(|run| run.text == clock), "missing complete {clock}: {runs:?}");
        }
    }
}

#[test]
fn modern_clock_rows_preserve_complete_labels_and_the_reserved_line_count() {
    let _serial = plx_base::testlock::serial();
    for percent in [20, 100, 200] {
        let mut measure = TypographyMeasure::new(FixtureMeasure);
        measure.set_font_scale(FontScale::from_percent(percent).unwrap());
        for clock in ["2:10:57\u{202f}PM", "2:10:57 PM", "23:59:59", ""] {
            let body = "A long programme title ".repeat(30);
            let view = TextView::new(&body, theme::size::TITLE, theme::TEXT_PRIMARY)
                .bold().with_measure(&measure);
            // The original full-width title reserved one line. A newly present
            // clock must not let it wrap into a second line and the detail row.
            let frame = Rect::new(96.0, 300.0, 1728.0, view.line_h());
            let [clock_frame, title_frame] = clocked_frames(frame, clock, &measure);
            if clock.is_empty() {
                assert_eq!(clock_frame.w, 0.0); assert_eq!(title_frame, frame);
            } else {
                assert!(clock_frame.w >= measure.width_str(clock, theme::size::CAPTION, false));
                assert!(contains(frame, clock_frame)); assert!(contains(frame, title_frame));
                assert!(clock_frame.x + clock_frame.w <= title_frame.x);
            }
            let runs = plx_ui::draw_census::capture_runs(|| draw_clocked_text(Painter::recording(),
                frame, clock, view, 2, &measure));
            let clock_count = usize::from(!clock.is_empty());
            assert_eq!(runs.iter().filter(|run| run.text == clock).count(), clock_count);
            assert_eq!(runs.len(), 1 + clock_count, "clock must not cause an extra body line: {runs:?}");
        }
    }
}
