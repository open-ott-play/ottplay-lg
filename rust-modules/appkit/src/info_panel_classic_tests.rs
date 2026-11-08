use super::*;
use plx_ui::fixture::FixtureMeasure;

struct TestHost;
impl Host for TestHost {
    type Arg = plx_ui::fixture::FixtureArg;
    type Fx = plx_ui::fixture::FixtureFx;
    type Msg = plx_ui::fixture::FixtureMsg;
    type Elem = u32;
    type Views<'a> = ();
    type Init = plx_ui::fixture::FixtureInit;
    type Memory = ();
}

#[test]
fn classic_has_no_action_group_or_stale_button_hit_target() {
    let state = InfoPanelState::new();
    let cx = Cx::<TestHost> {
        views: (), tick: Default::default(), measure: &FixtureMeasure,
        focus: Default::default(), press: Default::default(),
        owner: plx_machine::machine::InputOwner::Entry(EntryId(1)),
    };
    let classic = InfoPanelPart { state: &state, entry: EntryId(1), group: GroupId(0),
        can_go_to_detail: true, mode: UiMode::Classic };
    let mut groups = Vec::new();
    classic.groups(&cx, &mut groups);
    assert!(groups.is_empty());
    for key in [0, 1] {
        assert!(classic.group_of(&key, &cx).is_none());
        assert!(classic.place(&key, &cx, At::Drawn).is_none());
    }
    let modern = InfoPanelPart { mode: UiMode::Modern, ..classic };
    modern.groups(&cx, &mut groups);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].len, 2);
    assert!(modern.place(&0, &cx, At::Drawn).is_some());
}

#[test]
fn classic_cannot_activate_an_old_modern_info_button() {
    let store = plx_data::stores::metadata::MetadataStore::default();
    let state = InfoPanelState::new();
    assert_eq!(state.on_ok_for_mode(store.view(), UiMode::Classic), InfoAction::None);
    assert_eq!(state.on_ok_for_mode(store.view(), UiMode::Modern), InfoAction::FromBeginning);
}

fn playing(summary: &str) -> metadata::NowPlaying {
    metadata::NowPlaying {
        ott: None, is_episode: false, is_real_episode: false,
        title: "Playing film".into(), ep_title: String::new(), season: 0, index: 0,
        summary: summary.into(), year: 2026, dur_ms: 90_000,
        rating: String::new(), thumb: String::new(), detail_rk: "playing".into(),
    }
}

#[test]
fn classic_description_joins_the_footer_and_uses_the_full_reading_column() {
    let m = FixtureMeasure;
    let text = ClassicCopy { title: "Playing film", summary: "Complete synopsis.", duration_ms: 90_000 };
    let layout = ClassicLayout::new(&text, 810.0, &m);
    assert_eq!(layout.panel.x, 0.0);
    assert_eq!(layout.panel.w, SCR_W);
    assert_eq!(layout.panel.y + layout.panel.h, 810.0);
    assert!(layout.body.w > SCR_W * 0.7, "there is no artwork or action column beside the synopsis");
    assert!(layout.title.y < layout.duration.y);
    assert!(layout.duration.y < layout.body.y);
    assert!(layout.body.y + layout.body.h < 810.0);
}

#[test]
fn classic_live_description_uses_current_programme_not_channel_or_plex_page() {
    let mut np = playing("Channel fallback");
    np.ott = Some(metadata::OttNowPlaying {
        live: Some(metadata::OttLivePlaying {
            channel_title: "Channel".into(), ordinal: Some(12),
            guide: Some(metadata::OttNowNext { now_s: 150,
                current: Some(metadata::OttGuideProgramme {
                    title: "Current programme".into(), description: "Full current programme description.".into(),
                    start_s: 100, end_s: 200,
                }), next: None }),
        }), archive: None, collection_label: "Television".into(),
        confirmed_start_unix_ms: None, next: None, art: None,
    });
    let unrelated = metadata::Detail { title: "Wrong film".into(), summary: "Wrong description".into(), ..Default::default() };
    let text = classic_copy(Some(&np), Some(&unrelated), None);
    assert_eq!(text.title, "Current programme");
    assert_eq!(text.summary, "Full current programme description.");
    assert_eq!(text.duration_ms, 100_000);
    np.ott.as_mut().unwrap().live.as_mut().unwrap().guide = None;
    np.summary.clear();
    assert_eq!(classic_copy(Some(&np), Some(&unrelated), None).summary, "");
}

#[test]
fn classic_long_synopsis_can_reach_its_last_page_without_ellipsizing_content() {
    let _guard = plx_base::testlock::serial();
    let m = FixtureMeasure;
    let summary = (0..160).map(|i| format!("Paragraph {i}: all of this description stays readable.\n")).collect::<String>();
    let text = ClassicCopy { title: "Long film", summary: &summary, duration_ms: 90_000 };
    let mut state = InfoPanelState::new();
    state.reconcile_classic_copy(&text, 810.0, &m);
    let layout = ClassicLayout::new(&text, 810.0, &m);
    assert!(state.classic_pages > 1);
    assert!(state.classic_max_scroll > layout.body.h);
    for _ in 0..state.classic_pages {
        state.classic_scroll.step_page(plx_ui::consts::SDLK_DOWN);
    }
    for _ in 0..600 { state.update(1.0 / 60.0); }
    assert_eq!(state.classic_scroll.page, state.classic_pages);
    assert!((state.classic_scroll.offset(state.classic_max_scroll) - state.classic_max_scroll).abs() < 0.5);
    assert_eq!(text.summary, summary, "reading must not replace the source with an excerpt");
    let next = ClassicCopy { title: "Next film", summary: "Short text", duration_ms: 0 };
    state.reconcile_classic_copy(&next, 810.0, &m);
    assert_eq!(state.classic_scroll.page, 1);
    assert_eq!(state.classic_max_scroll, 0.0);
}

#[test]
fn classic_recorded_paint_contains_synopsis_but_no_modern_action_labels() {
    let _guard = plx_base::testlock::serial();
    let _no_live_metrics = plx_ui::text_view::ForbidLive::enter();
    let m = FixtureMeasure;
    let copy = ClassicCopy { title: "Playing film", summary: "First sentence. The rest is preserved.", duration_ms: 90_000 };
    let state = InfoPanelState::new();
    let runs = plx_ui::draw_census::capture_text(|| state.draw_classic(
        Painter::recording(), &copy, 810.0, &m, Default::default(),
    ));
    let painted = runs.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>().join(" ");
    assert!(painted.contains("Playing film"));
    assert!(painted.contains("The rest is preserved."));
    assert!(!painted.contains(plx_platform::i18n::msg::widgets_info_from_beginning()));
    assert!(!painted.contains(plx_platform::i18n::msg::widgets_info_go_movie()));
    assert!(runs.iter().all(|(_, rect)| rect.y + rect.h <= 810.0), "description must never paint over the footer");
}

#[test]
fn classic_description_background_uses_published_osd_opacity_without_dimming_text() {
    let _guard = plx_base::testlock::serial();
    let _no_live_metrics = plx_ui::text_view::ForbidLive::enter();
    let m = FixtureMeasure;
    let copy = ClassicCopy { title: "Playing film", summary: "Readable text.", duration_ms: 0 };
    for level in [0, 7, 10] {
        let opacity = crate::player_hud::model::OsdOpacity::from_level(level).unwrap();
        let state = InfoPanelState::new();
        let inks = plx_ui::draw_census::capture_rect_inks(|| state.draw_classic(
            Painter::recording(), &copy, 810.0, &m, opacity,
        ));
        let (_, top, bottom) = inks.iter().find(|(rect, _, _)| rect.w == SCR_W && rect.h > 1.0).unwrap();
        assert_eq!(*top, opacity.background_alpha(0.78));
        assert_eq!(*bottom, *top);
        let text = plx_ui::draw_census::capture_runs(|| state.draw_classic(
            Painter::recording(), &copy, 810.0, &m, opacity,
        ));
        assert!(text.iter().any(|run| run.text.contains("Readable text.")
            && run.ink[3] == theme::TEXT_READING[3]), "background opacity must not dim the glyphs");
    }
}


fn fingerprint(state: &InfoPanelState) -> u64 {
    let mut canon = plx_machine::machine::Canon::new();
    state.canon(&mut canon);
    canon.finish()
}

#[test]
fn classic_actual_draw_does_not_change_canonical_state_before_or_after_reconcile() {
    let _guard = plx_base::testlock::serial();
    let _no_live_metrics = plx_ui::text_view::ForbidLive::enter();
    let measure = FixtureMeasure;
    let summary = "All paragraphs remain in the source. ".repeat(200);
    let mut store = plx_data::stores::metadata::MetadataStore::default();
    store.run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(Some(playing(&summary))));
    let mut state = InfoPanelState::new();
    for reconciled in [false, true] {
        if reconciled { state.reconcile_classic(store.view(), &measure, Some(150)); }
        let before = fingerprint(&state);
        plx_ui::record_walk(|| state.draw_for_mode(&plx_media::route::PlaybackSession::IDLE,
            1.0, &measure, store.view(), UiMode::Classic, InfoPresentation { opacity: Default::default(), wall_now_s: Some(150) }));
        assert_eq!(fingerprint(&state), before, "actual draw must remain passive");
    }
}

#[test]
fn classic_tick_trace_is_identical_with_and_without_draw_including_metadata_replacement() {
    let _guard = plx_base::testlock::serial();
    let _no_live_metrics = plx_ui::text_view::ForbidLive::enter();
    let measure = FixtureMeasure;
    let mut shown = InfoPanelState::new();
    let mut hidden = InfoPanelState::new();
    let mut store = plx_data::stores::metadata::MetadataStore::default();
    for (index, summary) in ["Long paragraph ".repeat(500), "Replacement description".into()].iter().enumerate() {
        store.run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(Some(playing(summary))));
        for tick in 0..30 {
            for state in [&mut shown, &mut hidden] {
                state.reconcile_classic(store.view(), &measure, Some(150 + tick));
                if index == 0 && tick == 0 { state.scroll_classic(true); }
                state.update(1.0 / 60.0);
            }
            plx_ui::record_walk(|| shown.draw_for_mode(&plx_media::route::PlaybackSession::IDLE,
                1.0, &measure, store.view(), UiMode::Classic, InfoPresentation { opacity: Default::default(), wall_now_s: Some(150 + tick) }));
            assert_eq!(fingerprint(&shown), fingerprint(&hidden));
        }
    }
    assert_eq!(shown.classic_scroll.page, 1);
    assert_eq!(shown.classic_max_scroll, 0.0);
}

#[test]
fn classic_guide_rollover_uses_published_wall_time_and_expires_truthfully() {
    let mut now = playing("Stale first programme summary");
    now.ott = Some(metadata::OttNowPlaying { live: Some(metadata::OttLivePlaying {
        channel_title: "Channel title".into(), ordinal: None,
        guide: Some(metadata::OttNowNext { now_s: 150,
            current: Some(metadata::OttGuideProgramme { title: "First programme".into(),
                description: "First description".into(), start_s: 100, end_s: 200 }),
            next: Some(metadata::OttGuideProgramme { title: "Second programme".into(),
                description: "Second description".into(), start_s: 200, end_s: 300 }),
        }),
    }), archive: None, collection_label: "Television".into(), confirmed_start_unix_ms: None,
        next: None, art: None });
    let first = classic_copy(Some(&now), None, Some(150));
    assert_eq!((first.title, first.summary, first.duration_ms), ("First programme", "First description", 100_000));
    let next = classic_copy(Some(&now), None, Some(220));
    assert_eq!((next.title, next.summary, next.duration_ms), ("Second programme", "Second description", 100_000));
    let expired = classic_copy(Some(&now), None, Some(300));
    assert_eq!((expired.title, expired.summary, expired.duration_ms), ("Channel title", "", 0));
}

#[test]
fn classic_live_rollover_reconciles_reader_and_actual_paint_without_refetching_guide() {
    let _guard = plx_base::testlock::serial();
    let _no_live_metrics = plx_ui::text_view::ForbidLive::enter();
    let mut now = playing("Stale programme fallback must never appear");
    now.ott = Some(metadata::OttNowPlaying { live: Some(metadata::OttLivePlaying {
        channel_title: "Channel title".into(), ordinal: None,
        guide: Some(metadata::OttNowNext { now_s: 150,
            current: Some(metadata::OttGuideProgramme { title: "First programme".into(),
                description: "First description. ".repeat(600), start_s: 100, end_s: 200 }),
            next: Some(metadata::OttGuideProgramme { title: "Second programme".into(),
                description: "Second description".into(), start_s: 200, end_s: 300 }),
        }),
    }), archive: None, collection_label: "Television".into(), confirmed_start_unix_ms: None,
        next: None, art: None });
    let mut store = plx_data::stores::metadata::MetadataStore::default();
    store.run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(Some(now)));
    let mut state = InfoPanelState::new();
    for (seconds, expected, absent) in [
        (150, "First programme", "Second programme"),
        (220, "Second programme", "First programme"),
        (300, "Channel title", "Second programme"),
    ] {
        state.reconcile_classic(store.view(), &FixtureMeasure, Some(seconds));
        assert_eq!(state.classic_scroll.page, 1, "programme rollover resets reading position logically");
        let before = fingerprint(&state);
        let runs = plx_ui::draw_census::capture_text(|| plx_ui::record_walk(||
            state.draw_for_mode(&plx_media::route::PlaybackSession::IDLE, 1.0, &FixtureMeasure,
                store.view(), UiMode::Classic,
                InfoPresentation { opacity: Default::default(), wall_now_s: Some(seconds) })));
        assert_eq!(fingerprint(&state), before);
        let painted = runs.iter().map(|(text, _)| text.as_str()).collect::<Vec<_>>().join(" ");
        assert!(painted.contains(expected));
        assert!(!painted.contains(absent));
        assert!(!painted.contains("Stale programme fallback"));
        if seconds == 150 {
            assert!(state.classic_pages > 1);
            state.scroll_classic(true);
            assert!(state.classic_scroll.page > 1);
            state.update(1.0 / 60.0);
        } else {
            assert_eq!(state.classic_max_scroll, 0.0);
        }
        if seconds == 300 {
            let (_, summary, duration) = state.classic_content.as_ref().unwrap();
            assert!(summary.is_empty());
            assert_eq!(*duration, 0);
            assert!(!painted.contains("Second description"));
        }
    }
}
