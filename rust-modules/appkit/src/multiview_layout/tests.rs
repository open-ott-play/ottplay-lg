use super::*;

fn intersects(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

fn modes() -> Vec<Mode> {
    [Mode::Pip, Mode::Grid4, Mode::Grid9].into_iter()
        .chain((0..=8).map(|satellites| Mode::Perimeter { satellites })).collect()
}

#[test]
fn every_layout_is_stable_bounded_and_only_pip_overlaps() {
    for viewport in [Rect::FULL, Rect::new(13.0, 27.0, 1024.0, 768.0),
        Rect::new(-40.0, 60.0, 2560.0, 720.0), Rect::new(11.0, -9.0, 720.0, 1280.0),
        Rect::new(0.0, 0.0, 16.0, 900.0)] {
        for scale in [0.2, 1.0, 2.0] {
            for mode in modes() {
                let layout = Layout::new(viewport, mode, scale).unwrap();
                assert_eq!(layout, Layout::new(viewport, mode, scale).unwrap());
                let expected = match mode { Mode::Pip => 2, Mode::Grid4 => 4, Mode::Grid9 => 9,
                    Mode::Perimeter { satellites } => 1 + usize::from(satellites) };
                assert_eq!(layout.slots().len(), expected);
                for (index, slot) in layout.slots().iter().enumerate() {
                    assert_eq!(usize::from(slot.id), index);
                    assert!(inside(viewport, slot.bounds));
                    let border = layout.active_border(slot.id).unwrap();
                    for (i, band) in border.iter().enumerate() {
                        assert!(inside(slot.bounds, *band));
                        for other in &border[i + 1..] { assert!(!intersects(*band, *other)); }
                    }
                    for other in &layout.slots()[index + 1..] {
                        assert_eq!(intersects(slot.bounds, other.bounds), mode == Mode::Pip);
                    }
                }
                assert!(layout.slot(9).is_none());
                assert!(layout.active_border(9).is_none());
            }
        }
    }
}

#[test]
fn grids_represent_all_empty_cells_and_cardinal_navigation_does_not_wrap() {
    for (mode, n) in [(Mode::Grid4, 2u8), (Mode::Grid9, 3u8)] {
        let layout = Layout::new(Rect::FULL, mode, 1.0).unwrap();
        for id in 0..n * n {
            assert_eq!(layout.neighbor(id, Dir::Left), (id % n > 0).then(|| id - 1));
            assert_eq!(layout.neighbor(id, Dir::Right), (id % n + 1 < n).then(|| id + 1));
            assert_eq!(layout.neighbor(id, Dir::Up), (id >= n).then(|| id - n));
            assert_eq!(layout.neighbor(id, Dir::Down), (id + n < n * n).then(|| id + n));
            let r = layout.slot(id).unwrap().bounds;
            assert_eq!(layout.hit_test(r.cx(), r.cy()), Some(id));
        }
        assert_eq!(layout.neighbor(9, Dir::Left), None);
    }
}

#[test]
fn perimeter_slots_keep_their_clockwise_anchor_as_the_ring_fills() {
    let full = Layout::new(Rect::FULL, Mode::Perimeter { satellites: 8 }, 1.0).unwrap();
    let center = full.slot(0).unwrap().bounds;
    let sign = |value: f32| if value < 0.0 { -1 } else if value > 0.0 { 1 } else { 0 };
    let positions: Vec<_> = full.slots()[1..].iter().map(|s|
        (sign(s.bounds.cx() - center.cx()), sign(s.bounds.cy() - center.cy()))).collect();
    assert_eq!(positions, [(0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1)]);
    for satellites in 0..=8 {
        let partial = Layout::new(Rect::FULL, Mode::Perimeter { satellites }, 1.0).unwrap();
        for slot in &full.slots()[..=usize::from(satellites)] {
            assert_eq!(partial.hit_test(slot.bounds.cx(), slot.bounds.cy()), Some(slot.id));
        }
        if satellites == 0 { assert_eq!(partial.slots()[0].bounds, Rect::FULL); }
    }
    assert_eq!(full.neighbor(0, Dir::Up), Some(1));
    assert_eq!(full.neighbor(0, Dir::Right), Some(3));
    assert_eq!(full.neighbor(0, Dir::Down), Some(5));
    assert_eq!(full.neighbor(0, Dir::Left), Some(7));
    assert_eq!(full.neighbor(1, Dir::Right), Some(2));
    assert_eq!(full.neighbor(1, Dir::Down), Some(0));
}

#[test]
fn pip_sits_flush_to_the_corner_and_shared_grid_edges_have_one_target() {
    let layout = Layout::new(Rect::FULL, Mode::Pip, 1.0).unwrap();
    let inset = layout.slot(1).unwrap();
    assert!(inset.layer > layout.slot(0).unwrap().layer);
    assert_eq!(layout.hit_test(inset.bounds.cx(), inset.bounds.cy()), Some(1));
    assert_eq!(layout.hit_test(Rect::FULL.cx(), Rect::FULL.cy()), Some(0));
    assert_eq!(layout.hit_test(inset.bounds.x, inset.bounds.y), Some(1));
    assert_eq!(inset.bounds.x + inset.bounds.w, Rect::FULL.w);
    assert_eq!(inset.bounds.y + inset.bounds.h, Rect::FULL.h);
    assert_eq!(layout.hit_test(inset.bounds.x - 0.5, inset.bounds.cy()), Some(0));
    assert_eq!(layout.hit_test(inset.bounds.x + inset.bounds.w, inset.bounds.cy()), None);
    assert_eq!(layout.neighbor(0, Dir::Right), Some(1));
    assert_eq!(layout.neighbor(1, Dir::Left), Some(0));
    let grid = Layout::new(Rect::FULL, Mode::Grid4, 1.0).unwrap();
    assert_eq!(grid.hit_test(Rect::FULL.cx(), Rect::FULL.cy()), Some(3));
    assert_eq!(grid.hit_test(Rect::FULL.cx() - 0.5, Rect::FULL.cy()), Some(2));
    assert_eq!(layout.hit_test(1920.0, 100.0), None);
    assert_eq!(layout.hit_test(-1.0, 100.0), None);
}

#[test]
fn pointer_containment_maps_physical_surfaces_and_never_activates_scroll_or_letterbox() {
    for logical in [Rect::FULL, Rect::new(25.0, -30.0, 720.0, 1280.0)] {
        for surface in [Rect::new(0.0, 0.0, 3840.0, 2160.0),
            Rect::new(70.0, 120.0, 900.0, 900.0), Rect::new(-30.0, 25.0, 600.0, 300.0)] {
            for scale in [0.2, 1.0, 2.0] {
                let layout = Layout::new(logical, Mode::Grid9, scale).unwrap();
                let map = PointerMap::contain(logical, surface).unwrap();
                let p = map.painted();
                for slot in layout.slots() {
                    let x = p.x + (slot.bounds.cx() - logical.x) / logical.w * p.w;
                    let y = p.y + (slot.bounds.cy() - logical.y) / logical.h * p.h;
                    assert_eq!(layout.activate_pointer(&map, PointerAction::PrimaryClick, x, y), Some(slot.id));
                    for action in [PointerAction::Move, PointerAction::Scroll, PointerAction::SecondaryClick] {
                        assert_eq!(layout.activate_pointer(&map, action, x, y), None);
                    }
                }
                for (x, y) in [(p.x - 1.0, p.cy()), (p.x + p.w, p.cy()),
                    (p.cx(), p.y - 1.0), (p.cx(), p.y + p.h), (f32::NAN, p.cy())] {
                    assert_eq!(layout.activate_pointer(&map, PointerAction::PrimaryClick, x, y), None);
                }
            }
        }
    }
    let layout = Layout::new(Rect::FULL, Mode::Pip, 1.0).unwrap();
    let unrelated = PointerMap::contain(Rect::new(0.0, 0.0, 10.0, 10.0), Rect::FULL).unwrap();
    assert_eq!(layout.activate_pointer(&unrelated, PointerAction::PrimaryClick, 960.0, 540.0), None);
}

#[test]
fn invalid_geometry_is_refused_before_layout_or_input() {
    for bad in [Rect::new(0.0, 0.0, 0.0, 1080.0), Rect::new(0.0, 0.0, 100.0, -1.0),
        Rect::new(f32::NAN, 0.0, 100.0, 100.0), Rect::new(0.0, 0.0, f32::INFINITY, 100.0),
        Rect::new(f32::MAX, 0.0, f32::MAX, 100.0)] {
        assert_eq!(Layout::new(bad, Mode::Grid4, 1.0), Err(GeometryError::InvalidViewport));
        assert!(PointerMap::contain(bad, Rect::FULL).is_err());
        assert!(PointerMap::contain(Rect::FULL, bad).is_err());
    }
    for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(Layout::new(Rect::FULL, Mode::Pip, scale), Err(GeometryError::InvalidScale));
    }
    assert_eq!(Layout::new(Rect::FULL, Mode::Perimeter { satellites: 9 }, 1.0),
        Err(GeometryError::TooManySatellites));
}

#[test]
fn even_raster_plans_preserve_ids_layers_and_actual_hit_targets() {
    for (width, height) in [(960, 540), (1920, 1080), (720, 1280)] {
        for scale in [0.2, 1.0, 2.0] {
            for mode in modes() {
                let layout = Layout::new(Rect::new(0.0, 0.0, width as f32, height as f32), mode, scale).unwrap();
                let plan = layout.rasterize_even(81).unwrap();
                assert_eq!((plan.revision(), plan.width(), plan.height(), plan.mode()), (81, width, height, mode));
                let actual = Layout::from_raster(&plan, scale).unwrap();
                for (before, after) in layout.slots().iter().zip(plan.slots()) {
                    assert_eq!((before.id, before.layer), (after.id, after.layer));
                    assert!([after.x, after.y, after.width, after.height].iter().all(|v| v % 2 == 0));
                    let a = before.bounds;
                    let b = after.bounds();
                    for delta in [a.x - b.x, a.y - b.y, (a.x + a.w) - (b.x + b.w), (a.y + a.h) - (b.y + b.h)] {
                        assert!(delta.abs() <= 1.001, "an even edge moves at most one pixel");
                    }
                    let r = after.bounds();
                    let hit = actual.hit_test(r.cx(), r.cy());
                    assert_eq!(hit, Some(if mode == Mode::Pip && before.id == 0 { 0 } else { before.id }));
                }
                if mode == Mode::Pip {
                    let inset = plan.slots()[1].bounds();
                    assert_eq!(actual.hit_test(inset.cx(), inset.cy()), Some(1));
                }
                assert_eq!(actual.hit_test(width as f32, height as f32), None);
                let copied = RasterPlan::checked(81, width, height, mode, plan.slots().to_vec()).unwrap();
                assert_eq!(Layout::from_raster(&copied, scale).unwrap(), actual);
            }
        }
    }
}

#[test]
fn malformed_or_collapsed_rasters_never_become_input_geometry() {
    for viewport in [Rect::new(1.0, 0.0, 960.0, 540.0), Rect::new(0.0, 0.0, 961.0, 540.0),
        Rect::new(0.0, 0.0, 960.5, 540.0), Rect::new(0.0, 0.0, 16386.0, 540.0)] {
        assert_eq!(Layout::new(viewport, Mode::Grid4, 1.0).unwrap().rasterize_even(0), Err(GeometryError::InvalidRaster));
    }
    assert_eq!(Layout::new(Rect::new(0.0, 0.0, 2.0, 2.0), Mode::Grid9, 1.0).unwrap().rasterize_even(1),
        Err(GeometryError::CollapsedRasterSlot));
    let good = Layout::new(Rect::new(0.0, 0.0, 960.0, 540.0), Mode::Grid4, 1.0).unwrap().rasterize_even(0).unwrap();
    for case in 0..7 {
        let mut slots = good.slots().to_vec();
        match case { 0 => slots[0].width = 0, 1 => slots[0].x = 1,
            2 => slots[1].id = 0, 3 => slots[1].layer = 0,
            4 => slots[1].x = u32::MAX - 1,
            5 => { let first = slots[0]; slots[1].x = first.x; slots[1].y = first.y; },
            _ => { slots.pop(); } }
        assert_eq!(RasterPlan::checked(0, 960, 540, Mode::Grid4, slots), Err(GeometryError::InvalidRaster));
    }
    assert!(RasterPlan::checked(1, 960, 540, Mode::Pip, good.slots()[..2].to_vec()).is_err());
}

#[test]
fn every_pixel_belongs_to_a_window_at_display_and_encoder_sizes() {
    for (width, height) in [(1280, 720), (1920, 1080), (3840, 2160), (1366, 768),
        (1279, 719), (1921, 1081), (3841, 2159), (321, 181)] {
        for mode in modes() {
            let layout = Layout::new(Rect::new(0.0, 0.0, width as f32, height as f32), mode, 1.0).unwrap();
            let mut plans = vec![layout.rasterize_pixels(9).unwrap()];
            if width % 2 == 0 && height % 2 == 0 { plans.push(layout.rasterize_even(9).unwrap()); }
            for plan in plans {
                let actual = Layout::from_raster(&plan, 1.0).unwrap();
                // RasterPlan already checked that disjoint slots do not overlap.
                // Equal area proves complete coverage with no internal or outer gutters.
                let area: u32 = plan.slots().iter().map(|s| s.width * s.height).sum();
                if mode == Mode::Pip {
                    let inset = plan.slots()[1];
                    assert_eq!(area, width * height + inset.width * inset.height);
                    assert_eq!((inset.x + inset.width, inset.y + inset.height), (width, height));
                } else {
                    assert_eq!(area, width * height, "{width}x{height} {mode:?}");
                }
                for tile in plan.slots() {
                    for (x, y) in [(tile.x, tile.y), (tile.x + tile.width - 1, tile.y + tile.height - 1)] {
                        let expected = if mode == Mode::Pip && x >= plan.slots()[1].x && y >= plan.slots()[1].y {
                            1
                        } else { tile.id };
                        assert_eq!(actual.hit_test(x as f32 + 0.5, y as f32 + 0.5), Some(expected));
                    }
                }
            }
        }
    }
}
