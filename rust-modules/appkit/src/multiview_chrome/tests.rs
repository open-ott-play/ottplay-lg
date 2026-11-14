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
fn inside(a: Rect, b: Rect) -> bool {
    b.x >= a.x && b.y >= a.y && b.x + b.w <= a.x + a.w && b.y + b.h <= a.y + a.h
}

#[test]
fn chrome_projection_keeps_active_audio_focus_and_placeholders_independent() {
    let frame = frame();
    assert_eq!(frame.slots().len(), 4);
    for slot in frame.slots() {
        assert_eq!(slot.active, slot.id == 0);
        assert_eq!(slot.audio, slot.id == 1);
        assert_eq!(slot.focused, slot.id == 2);
        for r in [slot.title_band, slot.title_text, slot.body].into_iter()
            .chain(slot.active_marker).chain(slot.audio_marker).chain(slot.border) {
            assert!(inside(slot.bounds, r));
        }
    }
    assert_eq!(frame.slots()[3].state, SlotState::Empty);
    assert_eq!(frame.slots()[3].title, "");
    let mut input = views(); input[0].title = "Title\n\0X".into();
    let clean = ChromeFrame::new(frame.layout().clone(), &input, state()).unwrap();
    assert_eq!(clean.slots()[0].title, "TitleX");
}

#[test]
fn real_draw_paints_markers_and_closed_placeholders_without_covering_playing_video() {
    let frame = frame();
    let labels = Labels { empty: "Choose video", loading: "Loading", failed: "Unavailable" };
    let draw = || frame.draw(Painter::recording(), &FixtureMeasure, &labels);
    let icons = draw_census::capture_icons(draw);
    assert_eq!(icons.len(), 2);
    assert!(icons.iter().any(|(icon, rect)| *icon == Icon::Check && Some(*rect) == frame.slots()[0].active_marker));
    assert!(icons.iter().any(|(icon, rect)| *icon == Icon::Audio && Some(*rect) == frame.slots()[1].audio_marker));
    let runs = draw_census::capture_runs(draw);
    for label in ["Window 1", "Loading", "Unavailable", "Choose video"] {
        assert!(runs.iter().any(|run| run.text == label), "actual recorded text: {label}");
    }
    assert!(runs.iter().all(|run| run.clip.is_some_and(|clip|
        frame.slots().iter().any(|slot| slot.bounds == clip))));
    assert_eq!(draw_census::clip(), None);
    let fills = draw_census::capture_rect_inks(draw);
    assert!(!fills.iter().any(|(r, _, _)| *r == frame.slots()[0].bounds));
    for slot in &frame.slots()[1..] {
        assert!(fills.iter().any(|(r, top, bottom)| *r == slot.bounds && *top == 1.0 && *bottom == 1.0));
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
fn chrome_bounds_remain_inside_every_slot_across_layouts_and_scales() {
    for mode in [Mode::Pip, Mode::Grid4, Mode::Grid9, Mode::Perimeter { satellites: 8 }] {
        for scale in [0.2, 1.0, 2.0] {
            for viewport in [Rect::FULL, Rect::new(10.0, 20.0, 720.0, 1280.0)] {
                let layout = Layout::new(viewport, mode, scale).unwrap();
                let frame = ChromeFrame::new(layout, &[], ViewState { revision: 1, geometry_revision: 2, active: Some(0),
                    audio: Some(0), focused: Some(1), input_owned: true }).unwrap();
                for slot in frame.slots() {
                    for rect in [slot.title_band, slot.title_text, slot.body].into_iter()
                        .chain(slot.active_marker).chain(slot.audio_marker).chain(slot.border) {
                        assert!(inside(slot.bounds, rect), "{mode:?} {scale}: {rect:?} outside {:?}", slot.bounds);
                    }
                }
            }
        }
    }
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
fn lower_pip_placeholder_never_covers_a_higher_playing_native_picture() {
    let labels = Labels { empty: "Empty", loading: "Loading", failed: "Unavailable" };
    for scale in [0.2, 1.0, 2.0] {
        for state in [SlotState::Empty, SlotState::Loading, SlotState::Failed] {
            let layout = Layout::new(Rect::FULL, Mode::Pip, scale).unwrap();
            let inset = layout.slot(1).unwrap().bounds;
            let frame = ChromeFrame::new(layout, &[
                SlotView { id: 0, title: "Main".into(), state },
                SlotView { id: 1, title: "Inset".into(), state: SlotState::Playing },
            ], ViewState { revision: 800, geometry_revision: 31, active: Some(1),
                audio: Some(1), focused: Some(1), input_owned: true }).unwrap();
            let runs = draw_census::capture_runs(|| frame.draw(Painter::recording(), &FixtureMeasure, &labels));
            assert!(runs.iter().any(|run| run.text == "Inset"));
            let main_runs: Vec<_> = runs.iter().filter(|run| run.text == "Main").collect();
            assert!(!main_runs.is_empty());
            for run in main_runs {
                let clip = run.clip.expect("lower painting is clipped");
                assert!(clip.x + clip.w <= inset.x || clip.x >= inset.x + inset.w
                    || clip.y + clip.h <= inset.y || clip.y >= inset.y + inset.h,
                    "lower slot paint must never cover inset: {clip:?} / {inset:?}");
            }
            let regions = visible_regions(frame.slots()[0].bounds, std::iter::once(inset));
            let fills = draw_census::capture_rect_inks(|| frame.draw(Painter::recording(), &FixtureMeasure, &labels));
            assert!(!fills.iter().any(|(rect, _, _)| *rect == frame.slots()[0].bounds),
                "no full-main opaque placeholder may cover the inset");
            for region in &regions {
                assert!(fills.iter().any(|(rect, top, bottom)| rect == region && *top == 1.0 && *bottom == 1.0));
            }
            let area: f64 = regions.iter().map(|r| f64::from(r.w) * f64::from(r.h)).sum();
            let expected = f64::from(Rect::FULL.w) * f64::from(Rect::FULL.h) - f64::from(inset.w) * f64::from(inset.h);
            assert!((area - expected).abs() < 1.0, "visible placeholder keeps all remaining main area");
            assert_eq!(draw_census::clip(), None);
        }
    }
}
