use super::*;
use crate::multiview_layout::Mode;
use plx_ui::{draw_census, fixture::FixtureMeasure};

fn state() -> ViewState {
    ViewState { revision: 31, geometry_revision: 31, active: Some(0), audio: Some(1), focused: Some(2), input_owned: true }
}
fn views() -> Vec<SlotView> {
    [SlotState::Playing, SlotState::Loading, SlotState::Failed].into_iter().enumerate()
        .map(|(id, state)| SlotView { id: id as SlotId, title: format!("Window {}", id + 1), state }).collect()
}
fn frame() -> ChromeFrame {
    ChromeFrame::new(Layout::new(Rect::FULL, Mode::Grid4, 1.0).unwrap(), &views(), state()).unwrap()
}
fn playing(title: &str) -> NowPlaying {
    NowPlaying { title: title.into(), ott: None, is_episode: false, is_real_episode: false,
        ep_title: String::new(), season: 0, index: 0, summary: String::new(), year: 0,
        dur_ms: 0, rating: String::new(), thumb: String::new(), detail_rk: String::new() }
}

#[test]
fn bare_grid_has_no_slot_numbers_labels_badges_or_dividers() {
    let f = frame();
    let mut ui = WindowFooters::default();
    let mut draw = || ui.draw(&f, Painter::recording(), &FixtureMeasure, UiMode::Classic,
        OsdOpacity::default(), None, 700, 2_000);
    assert!(draw_census::capture_runs(&mut draw).is_empty());
    assert!(draw_census::capture_icons(&mut draw).is_empty());
    assert!(draw_census::capture_rect_inks(&mut draw).is_empty());
}

#[test]
fn occupied_window_paints_the_shared_footer_to_scale_until_the_configured_deadline() {
    for mode in [Mode::Grid4, Mode::Grid9, Mode::Perimeter { satellites: 8 }] {
        let mut f = ChromeFrame::new(Layout::new(Rect::FULL, mode, 1.0).unwrap(), &views(), state()).unwrap();
        f.set_footer(0, playing("Film A"));
        f.set_footer(1, playing("Film B"));
        f.set_footer(3, playing("Stale empty"));
        let mut ui = WindowFooters::default();
        let draw = |ui: &mut WindowFooters, now| draw_census::capture_runs(|| ui.draw(
            &f, Painter::recording(), &FixtureMeasure, UiMode::Classic, OsdOpacity::default(),
            None, now, 2_000));
        let shown = draw(&mut ui, 10);
        for (id, title) in [(0, "Film A"), (1, "Film B")] {
            let bounds = f.slots()[id].bounds;
            let run = shown.iter().find(|line| line.text == title).unwrap();
            assert_eq!(run.clip, Some(bounds));
            assert!(run.at.y > bounds.cy() && run.at.y < bounds.y + bounds.h);
        }
        assert!(!shown.iter().any(|line| line.text.contains("Window") || line.text.contains("Stale")));
        assert!(draw(&mut ui, 2_010).is_empty());
        ui.reveal(1, 2_500);
        let shown = draw(&mut ui, 2_501);
        assert!(shown.iter().any(|line| line.text == "Film B"));
        assert!(!shown.iter().any(|line| line.text == "Film A"));
    }
}

#[test]
fn pointer_activation_is_topmost_revision_bound_and_never_scroll_or_hover() {
    let layout = Layout::new(Rect::FULL, Mode::Pip, 1.0).unwrap();
    let view = ViewState { revision: 99, active: Some(0), ..Default::default() };
    let dormant = ChromeFrame::new(layout.clone(), &[], view).unwrap();
    let enabled = ChromeFrame::new(layout.clone(), &[], ViewState { input_owned: true, ..view }).unwrap();
    let map = PointerMap::contain(Rect::FULL, Rect::new(20.0, 40.0, 1920.0, 1920.0)).unwrap();
    let inset = layout.slot(1).unwrap().bounds;
    let painted = map.painted();
    let x = painted.x + inset.cx() / Rect::FULL.w * painted.w;
    let y = painted.y + inset.cy() / Rect::FULL.h * painted.h;
    assert_eq!(dormant.pointer(&map, PointerAction::PrimaryClick, x, y), None);
    assert_eq!(enabled.pointer(&map, PointerAction::PrimaryClick, x, y),
        Some(Event::ActivateSlot { revision: 99, slot: 1 }));
    for action in [PointerAction::Move, PointerAction::Scroll, PointerAction::SecondaryClick] {
        assert_eq!(enabled.pointer(&map, action, x, y), None);
    }
    assert_eq!(enabled.pointer(&map, PointerAction::PrimaryClick, painted.cx(), painted.y - 1.0), None);
    assert_eq!(enabled.state(), ViewState { input_owned: true, ..view }, "events never commit active/audio state");
}

#[test]
fn cardinal_focus_and_confirmation_emit_only_semantic_selection_events() {
    let layout = Layout::new(Rect::FULL, Mode::Grid9, 1.0).unwrap();
    let owned = ViewState { revision: 17, geometry_revision: 2, active: Some(0), audio: Some(0), focused: Some(4), input_owned: true };
    let frame = ChromeFrame::new(layout.clone(), &[], owned).unwrap();
    for (direction, slot) in [(Dir::Up, 1), (Dir::Right, 5), (Dir::Down, 7), (Dir::Left, 3)] {
        assert_eq!(frame.input(Input::Direction(direction)), Some(Event::FocusSlot { revision: 17, slot }));
    }
    assert_eq!(frame.input(Input::Confirm), Some(Event::ActivateSlot { revision: 17, slot: 4 }));
    assert_eq!(frame.input(Input::Other), None);
    assert_eq!(frame.state(), owned);
    let passive = ChromeFrame::new(layout, &[], ViewState { input_owned: false, ..owned }).unwrap();
    for input in [Input::Direction(Dir::Up), Input::Confirm, Input::Other] { assert_eq!(passive.input(input), None); }
}

#[test]
fn checked_actual_raster_is_used_without_recalculating_receipt_geometry() {
    let plan = Layout::new(Rect::new(0.0, 0.0, 960.0, 540.0), Mode::Grid4, 1.0).unwrap().rasterize_even(31).unwrap();
    let frame = ChromeFrame::from_raster(&plan, &views(), state(), 1.0).unwrap();
    for (slot, actual) in frame.slots().iter().zip(plan.slots()) { assert_eq!(slot.bounds, actual.bounds()); }
    assert!(matches!(ChromeFrame::from_raster(&plan, &[], ViewState { geometry_revision: 32, ..state() }, 1.0),
        Err(ChromeError::InvalidRevision)));
    let surface = Rect::new(30.0, 20.0, 1600.0, 1200.0);
    let projected = ChromeFrame::from_raster_in(&plan, &views(), state(), 1.0, surface).unwrap();
    let projection = PointerMap::contain(frame.layout().viewport(), surface).unwrap();
    for (slot, actual) in projected.slots().iter().zip(plan.slots()) {
        assert_eq!(slot.bounds, projection.project_rect(actual.bounds()).unwrap());
    }
    let input_map = PointerMap::contain(projected.layout().viewport(), surface).unwrap();
    let target = projected.slots()[2].bounds;
    assert_eq!(projected.pointer(&input_map, PointerAction::PrimaryClick, target.cx(), target.cy()),
        Some(Event::ActivateSlot { revision: 31, slot: 2 }));
    assert_eq!(projected.pointer(&input_map, PointerAction::PrimaryClick, surface.cx(), surface.y), None);
}

#[test]
fn malformed_chrome_models_are_rejected_without_any_media_operation() {
    let layout = Layout::new(Rect::FULL, Mode::Grid4, 1.0).unwrap();
    let mut duplicate = views(); duplicate.push(duplicate[0].clone());
    assert!(matches!(ChromeFrame::new(layout.clone(), &duplicate, state()), Err(ChromeError::DuplicateSlot)));
    assert!(matches!(ChromeFrame::new(layout.clone(), &[SlotView { id: 9, title: "".into(), state: SlotState::Empty }], state()),
        Err(ChromeError::InvalidSlot)));
    assert!(matches!(ChromeFrame::new(layout.clone(), &[], ViewState { audio: Some(9), ..state() }), Err(ChromeError::InvalidSlot)));
    assert!(matches!(ChromeFrame::new(layout, &[SlotView { id: 0, title: "x".repeat(4097), state: SlotState::Playing }], state()),
        Err(ChromeError::TitleTooLong)));
}

#[test]
fn new_ui_publication_keeps_actual_geometry_and_emits_only_its_own_serial() {
    let plan = Layout::new(Rect::new(0.0, 0.0, 960.0, 540.0), Mode::Grid4, 1.0)
        .unwrap().rasterize_even(31).unwrap();
    let first = ChromeFrame::from_raster(&plan, &[], ViewState { revision: 800,
        geometry_revision: 31, ..state() }, 1.0).unwrap();
    let second = ChromeFrame::from_raster(&plan, &[], ViewState { revision: 801,
        geometry_revision: 31, focused: Some(3), ..state() }, 1.0).unwrap();
    assert_eq!(first.layout(), second.layout(), "focus must not retag or recompute actual rectangles");
    assert_eq!(plan.revision(), 31);
    assert_eq!(first.input(Input::Confirm), Some(Event::ActivateSlot { revision: 800, slot: 2 }));
    assert_eq!(second.input(Input::Confirm), Some(Event::ActivateSlot { revision: 801, slot: 3 }));
    assert_eq!(second.state().geometry_revision, 31);
}

#[test]
fn pip_masks_the_main_footer_under_the_inset() {
    let layout = Layout::new(Rect::FULL, Mode::Pip, 1.0).unwrap();
    let inset = layout.slot(1).unwrap().bounds;
    let frame = ChromeFrame::new(layout, &views()[..2], ViewState { revision: 80,
        geometry_revision: 1, active: Some(1), ..Default::default() }).unwrap();
    let regions = visible_regions(frame.slots()[0].bounds, std::iter::once(inset));
    for clip in &regions {
        assert!(clip.x + clip.w <= inset.x || clip.x >= inset.x + inset.w
            || clip.y + clip.h <= inset.y || clip.y >= inset.y + inset.h);
    }
    let area: f64 = regions.iter().map(|r| f64::from(r.w) * f64::from(r.h)).sum();
    let expected = f64::from(Rect::FULL.w) * f64::from(Rect::FULL.h) - f64::from(inset.w) * f64::from(inset.h);
    assert!((area - expected).abs() < 1.0);
}
