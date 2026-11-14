use super::*;
use plx_machine::machine::{Edge, InputEvent, InputKind, PressId, Source, Tick, UiMode};

struct CoverHost;
impl Host for CoverHost {
    type Arg = crate::registry::AppArg;
    type Fx = AppFx;
    type Msg = AppMsg;
    type Elem = u32;
    type Views<'a> = UiMode;
    type Init = crate::registry::AppArg;
    type Memory = PageMemory;
    fn ui_mode(mode: &UiMode) -> UiMode { *mode }
}
fn context(mode: UiMode) -> Cx<'static, CoverHost> {
    Cx { views: mode, tick: Tick { ms: 1_000, dt_us: 16_000 },
        measure: &plx_ui::fixture::FixtureMeasure, focus: Default::default(),
        press: Default::default(), owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) }
}
fn send(screen: &mut OttScreen, mode: UiMode, event: ScreenEvent<CoverHost>) -> Vec<Stamped<CoverHost>> {
    let mut out = Vec::new();
    let mut present = plx_machine::present::Present::new();
    screen.step(&event, &context(mode), &mut Effects::new(&mut out,
        MachineId::Instance(InstanceId(1)), &mut present));
    out
}
fn key(key: Key) -> ScreenEvent<CoverHost> {
    ScreenEvent::Input(InputEvent { kind: InputKind::Key { key, sym: 0, wcode: 0,
        edge: Edge::Down, at_edge: false }, at: Tick::default(), source: Source::Sdl })
}
fn drawn_text(screen: &mut OttScreen, mode: UiMode) -> Vec<String> {
    screen.refresh_presentation(mode, &plx_ui::fixture::FixtureMeasure);
    let cx = context(mode);
    let mut frame = DrawFrame::new(&cx, plx_ui::Painter::recording());
    plx_ui::draw_census::capture_runs(|| plx_gfx::gfx::without_frame_clear(|| screen.draw(&mut frame)))
        .into_iter().filter(|run| run.ink[3] > 0.0).map(|run| run.text).collect()
}

#[test]
fn initial_loading_paints_only_the_cover_without_menu_or_intro() {
    let _serial = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        assert!(screen.loading_cover());
        assert!(drawn_text(&mut screen, mode).is_empty(),
            "an unknown startup target cannot expose the Menu, intro or catalogue text");
        let mut groups = Vec::new();
        <OttScreen as Focusable<CoverHost>>::groups(&screen, &context(mode), &mut groups);
        assert!(groups.is_empty());
        assert!(screen.viewport_target().is_none());
    }
}

#[test]
fn configured_startup_cover_paints_only_its_exact_target_and_no_stale_metrics() {
    let _serial = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for target_present in [true, false] {
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            let mut view = (*snapshot(1, &["old", "target"])).clone();
            view.rows[0].title = "Old channel".into();
            view.rows[1].title = "Target channel".into();
            view.rows[1].content_title = Some("Target channel".into());
            view.rows[1].guide_available = true;
            view.rows[1].guide = Some(OttNowNext { now_s: 150,
                current: Some(OttGuideProgramme { title: "Target programme".into(),
                    description: String::new(), start_s: 100, end_s: 200 }),
                next: Some(OttGuideProgramme { title: "Target next".into(),
                    description: String::new(), start_s: 200, end_s: 300 }) });
            view.selected_row = Some("old".into());
            view.startup_pending = true;
            view.startup_target = Some(if target_present { "target" } else { "not-yet-discovered" }.into());
            view.pending_play_ordinal = Some(33);
            screen.install(Arc::new(view));
            let text = drawn_text(&mut screen, mode);
            if target_present {
                for expected in ["Target channel", "Target programme", "Target next", "33"] {
                    assert!(text.iter().any(|run| run == expected), "{mode:?}: missing {expected}: {text:?}");
                }
            } else {
                assert!(text.is_empty(), "missing target must not borrow a browsed row's footer");
            }
            assert!(!text.iter().any(|run| run.contains("Old channel") || run.contains("Mbps")
                || run.contains("1920") || run == "0"));
            assert!(screen.loading_cover());
        }
    }
}

#[test]
fn projected_pending_target_paints_without_a_catalogue_row_and_live_without_epg_keeps_ordinal() {
    let _serial = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for startup in [true, false] {
            for with_guide in [true, false] {
                let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
                let mut view = (*snapshot(1, &["old"])).clone();
                view.rows[0].title = "Old catalogue channel".into();
                if startup {
                    view.startup_pending = true;
                    view.startup_target = Some("outside-current-category".into());
                } else { view.pending_play = Some("outside-current-category".into()); }
                // The source-fenced projection is primary, including its
                // ordinal; list-relative or earlier row hints cannot replace it.
                view.pending_play_ordinal = Some(7);
                view.pending_display = Some(OttPendingDisplay {
                    title: "Projected target".into(), description: String::new(), live: true,
                    ordinal: Some(33), poster: None,
                    guide: with_guide.then(|| OttNowNext { now_s: 150,
                        current: Some(OttGuideProgramme { title: "Projected programme".into(),
                            description: String::new(), start_s: 100, end_s: 200 }),
                        next: Some(OttGuideProgramme { title: "Projected next".into(),
                            description: String::new(), start_s: 200, end_s: 300 }) }),
                });
                screen.install(Arc::new(view));
                let text = drawn_text(&mut screen, mode);
                for expected in ["Projected target", "33"] {
                    assert!(text.iter().any(|run| run == expected),
                        "{mode:?}, startup={startup}, guide={with_guide}: missing {expected}: {text:?}");
                }
                if with_guide {
                    for expected in ["Projected programme", "Projected next"] {
                        assert!(text.iter().any(|run| run == expected), "{mode:?}: missing {expected}: {text:?}");
                    }
                } else {
                    assert!(!text.iter().any(|run| run.contains("Projected programme") || run.contains("Projected next")));
                }
                assert!(!text.iter().any(|run| run.contains("Old catalogue channel")
                    || run.contains("Mbps") || run.contains("1920") || run == "7" || run == "0"));
                assert!(screen.viewport_target().is_none());
            }
        }
    }
}

#[test]
fn canonical_state_tracks_pending_display_changes_without_needing_row_or_revision_changes() {
    let _serial = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        let mut view = (*snapshot(1, &["old"])).clone();
        view.pending_play = Some("not-in-rows".into());
        view.pending_display = Some(OttPendingDisplay {
            title: "First projected target".into(), description: String::new(),
            live: true, ordinal: Some(33), guide: None, poster: None,
        });
        screen.install(Arc::new(view));
        screen.refresh_presentation(mode, &plx_ui::fixture::FixtureMeasure);
        let before = screen.hash();
        let mut changed = (*screen.snapshot).clone();
        changed.pending_display.as_mut().unwrap().title = "New projected target".into();
        // Isolate the canonical field itself: no install, revision increment,
        // navigation, row refresh or focus mutation can explain this difference.
        screen.snapshot = Arc::new(changed);
        assert_ne!(screen.hash(), before, "pending display text belongs to logical screen state");
        let text = drawn_text(&mut screen, mode);
        assert!(text.iter().any(|run| run == "New projected target"));
        assert!(!text.iter().any(|run| run == "First projected target"));
        let with_display = screen.hash();
        let mut cleared = (*screen.snapshot).clone();
        cleared.pending_display = None;
        screen.snapshot = Arc::new(cleared);
        assert_ne!(screen.hash(), with_display);
        assert!(drawn_text(&mut screen, mode).is_empty(),
            "retired projection cannot survive in the footer or borrow an unrelated row");
    }
}

#[test]
fn loading_cover_rejects_hidden_controls_and_back_waits_for_the_runtime_snapshot() {
    let _serial = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for startup in [true, false] {
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            screen.install(snapshot(1, &["one", "two"]));
            screen.classic_visible = true;
            screen.refresh_presentation(mode, &plx_ui::fixture::FixtureMeasure);
            let hidden = screen.keys["item:two"];
            let selected = screen.form.selected_id().cloned();
            let media = screen.media;
            let mut cover = (*screen.snapshot).clone();
            cover.revision = 2;
            if startup { cover.startup_pending = true; cover.startup_target = Some("two".into()); }
            else { cover.pending_play = Some("two".into()); }
            send(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(cover))));
            let cx = context(mode);
            let mut groups = Vec::new();
            <OttScreen as Focusable<CoverHost>>::groups(&screen, &cx, &mut groups);
            assert!(groups.is_empty());
            assert!(<OttScreen as Focusable<CoverHost>>::place(&screen, &hidden, &cx, At::Drawn).is_none());
            let mut links = Vec::new();
            <OttScreen as Screen<CoverHost>>::links(&screen, &mut links);
            assert!(links.is_empty());
            assert!(screen.viewport_target().is_none() && screen.viewport_aperture().is_none());
            assert!(!screen.accepts_channel_number(), "numeric input must not address hidden catalogue rows");
            for event in [key(Key::Down), key(Key::Ok), ScreenEvent::Activate(hidden),
                ScreenEvent::PressCommit(PressId(1)), ScreenEvent::PressHold(PressId(1)),
                ScreenEvent::FocusMoved { from: None, to: FocusKey { entry: EntryId(1), elem: hidden },
                    by: plx_ui::screen::By::Pointer },
                ScreenEvent::Tick(Tick { ms: 2_000, dt_us: 1_000_000 })] {
                let out = send(&mut screen, mode, event);
                assert!(!out.iter().any(|event| matches!(event.fx, Fx::App(_))),
                    "hidden input must not activate, preview or start media");
                assert!(!screen.editing && screen.editor.is_none());
            }
            assert_eq!(screen.form.selected_id(), selected.as_ref());
            assert_eq!(screen.media, media);
            let out = send(&mut screen, mode, key(Key::Back));
            assert_eq!(out.iter().filter(|event| matches!(&event.fx,
                Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::Back, .. })))).count(), 1);
            assert!(screen.loading_cover(), "Back submits intent; only the runtime can retire the cover");
            send(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(snapshot(3, &["one", "two"]))));
            assert!(!screen.loading_cover());
            let mut groups = Vec::new();
            <OttScreen as Focusable<CoverHost>>::groups(&screen, &cx, &mut groups);
            assert!(!groups.is_empty(), "the accepted snapshot restores actual list controls");
            assert!(screen.accepts_channel_number());
            let out = send(&mut screen, mode, ScreenEvent::Activate(hidden));
            assert!(out.iter().any(|event| matches!(&event.fx,
                Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::Play(id), .. })) if id == "two")),
                "the same real row becomes actionable after the cover is retired");
        }
    }
}
