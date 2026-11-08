use super::*;
use plx_machine::machine::UiMode;
use plx_ui::placeholder::{self, Reason};

fn fixture(mode: UiMode, selected: bool, progress: bool) -> OttScreen {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    let mut view = (*snapshot(1, if selected { &["Selected video"] } else { &[] })).clone();
    if progress {
        view.catalog_progress = Some(OttCatalogProgress { loaded: 1, total: Some(20), loading: true, complete: false });
    } else {
        view.page_id = "guide:fixture".into();
        view.status = "Fixture waiting status".into();
        view.status_working = true;
    }
    if selected { view.selected_row = Some("Selected video".into()); }
    screen.install(Arc::new(view));
    screen.refresh_presentation(mode, &plx_ui::fixture::FixtureMeasure);
    screen
}

fn paint(screen: &mut OttScreen, painter: plx_ui::Painter) {
    let measure = plx_ui::fixture::FixtureMeasure;
    let cx = Cx::<TestHost> { views: (), tick: Default::default(), measure: &measure,
        focus: Default::default(), press: Default::default(),
        owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
    let mut frame = DrawFrame::new(&cx, painter);
    plx_gfx::gfx::without_frame_clear(|| screen.draw(&mut frame));
}

fn observed(screen: &mut OttScreen) -> (Vec<String>, placeholder::Frame) {
    let runs = plx_ui::draw_census::capture_runs(|| paint(screen, plx_ui::Painter::recording()));
    let text = runs.into_iter().filter(|run| run.ink[3] > 0.0).map(|run| run.text).collect();
    let debt = placeholder::capture_declared(|| paint(screen, plx_ui::Painter::root())).1;
    (text, debt)
}

#[test]
fn visible_catalogue_progress_is_counted_in_both_presentations() {
    let _serial = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for selected in [false, true] {
            let mut screen = fixture(mode, selected, true);
            let status = screen.status.clone();
            let (text, debt) = observed(&mut screen);
            assert!(text.iter().any(|run| run.contains(&status)), "the progress text is actually drawn");
            assert_eq!(debt.of(Reason::WorkingReadout), 1, "visible pending progress is counted once");
            assert!(debt.entries.iter().any(|entry| entry.key == "ott-status"));
        }
    }
}

#[test]
fn visible_typed_guide_wait_is_counted_in_both_presentations() {
    let _serial = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut screen = fixture(mode, false, false);
        let (text, debt) = observed(&mut screen);
        assert!(text.iter().any(|run| run.contains("Fixture waiting status")));
        assert_eq!(debt.of(Reason::WorkingReadout), 1, "typed Guide wait is counted once at paint");
    }
}

#[test]
fn hidden_working_status_has_no_placeholder_debt() {
    let _serial = plx_base::testlock::serial();
    for hidden in ["editor", "modern-browse", "classic-reader"] {
        let mode = if hidden == "classic-reader" { UiMode::Classic } else { UiMode::Modern };
        let mut screen = fixture(mode, true, false);
        match hidden {
            "editor" => editor(&mut screen, "draft"),
            "modern-browse" => { effects(|fx| screen.open_browse(fx)); assert!(screen.browse); }
            _ => { effects(|fx| screen.show_details(fx)); assert!(screen.reading); }
        }
        let (text, debt) = observed(&mut screen);
        assert!(!text.iter().any(|run| run.contains("Fixture waiting status")), "{hidden} hides the status");
        assert_eq!(debt.of(Reason::WorkingReadout), 0, "{hidden} must not count hidden status");
    }
}

#[test]
fn working_status_install_and_recording_do_not_accrue_debt() {
    let _serial = plx_base::testlock::serial();
    let mut screen = fixture(UiMode::Modern, true, true);
    let snapshot = screen.snapshot.clone();
    let debt = placeholder::capture_declared(|| screen.install(snapshot)).1;
    assert_eq!(debt.of(Reason::WorkingReadout), 0, "publication is not paint");
    let debt = placeholder::capture_declared(|| paint(&mut screen, plx_ui::Painter::recording())).1;
    assert_eq!(debt.of(Reason::WorkingReadout), 0, "discovery is not a presented frame");
}

#[test]
fn settled_and_untyped_status_text_does_not_create_pending_debt() {
    let _serial = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut screen = fixture(mode, true, true);
        let mut view = (*screen.snapshot).clone();
        view.catalog_progress.as_mut().unwrap().complete = true;
        view.status = "Loading is finished".into();
        screen.install(Arc::new(view));
        let (text, debt) = observed(&mut screen);
        assert!(text.iter().any(|run| run.contains("Loading is finished")));
        assert_eq!(debt.of(Reason::WorkingReadout), 0, "words do not infer pending state");
    }
}

#[test]
fn classic_initial_loading_retains_its_existing_draw_accounting() {
    let _serial = plx_base::testlock::serial();
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    let (text, debt) = observed(&mut screen);
    assert!(text.iter().any(|run| run.contains(msg::browse_library_loading())));
    assert_eq!(debt.of(Reason::WorkingReadout), 1);
}

#[test]
fn long_copy_does_not_count_a_status_elided_or_below_the_screen() {
    let _serial = plx_base::testlock::serial();
    for mode in [UiMode::Modern, UiMode::Classic] {
        let mut screen = fixture(mode, false, false);
        let mut view = (*screen.snapshot).clone();
        view.description = "A long catalogue introduction fills the narrative column. ".repeat(100);
        screen.install(Arc::new(view));
        let runs = plx_ui::draw_census::capture_runs(|| paint(&mut screen, plx_ui::Painter::recording()));
        assert!(!runs.iter().any(|run| run.text.contains("Fixture waiting status")
            && run.at.y < Rect::FULL.h && run.at.y + run.at.h > 0.0), "the pending status is not on screen");
        let debt = placeholder::capture_declared(|| paint(&mut screen, plx_ui::Painter::root())).1;
        assert_eq!(debt.of(Reason::WorkingReadout), 0);
    }
}
