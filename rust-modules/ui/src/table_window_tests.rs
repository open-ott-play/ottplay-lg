//! Shared-table correctness and legacy-scan regressions. `walk` is the retained
//! general-layout oracle. `take_row_visits` observes only the existing digit scan,
//! general walk and fallback rows_at lookup. It does not count the new window,
//! eligibility proof, note scan, row_height or direct-index accesses; these tests
//! therefore do not measure total row work. Those bounds require source review
//! and separate performance evidence.
use super::*;
use crate::fixture::{FixtureHost, FixtureMeasure};
use crate::screen::{At, Dir, Focusable, Step};
use plx_machine::machine::{Cx, EntryId, FocusKey, FontScale, GroupId, Host, InputOwner, TypographyMeasure, UiMode};

struct ClassicHost;
impl Host for ClassicHost {
    type Arg = <FixtureHost as Host>::Arg;
    type Fx = <FixtureHost as Host>::Fx;
    type Msg = <FixtureHost as Host>::Msg;
    type Elem = u32;
    type Views<'a> = UiMode;
    type Init = <FixtureHost as Host>::Init;
    type Memory = ();
    fn ui_mode(views: &Self::Views<'_>) -> UiMode { *views }
}

fn uniform(n: usize, frame: Rect) -> TableView {
    let mut table = TableView::new().unfocused();
    table.set_sections(vec![Section::new("")], 0, false);
    // Direct public mutation is intentional: callers already populate sections
    // this way, and the optimization must not require a new mutation API.
    table.sections[0].rows = (0..n)
        .map(|i| Row::new(format!("window row {i}"))
            .catalogue_number((i + 1).to_string()).dim(i == n / 2))
        .collect();
    table.set_classic(true);
    table.set_classic_catalogue(true);
    table.set_classic_page_size(Some(25), frame.h);
    table.fit_layout(&FixtureMeasure);
    table
}

fn all_frames(table: &TableView, frame: Rect) -> Vec<(i32, Rect)> {
    let mut rows = Vec::new();
    table.walk(|y, i, _, row| {
        if let Some(row) = row.filter(|row| !row.sep) {
            rows.push((i, table.frame_at(frame, y, row)));
        }
    });
    rows
}

fn visible_frames(table: &TableView, frame: Rect) -> Vec<(i32, Rect)> {
    let mut rows = Vec::new();
    table.for_each_visible_row(frame, |i, rect| rows.push((i, rect)));
    rows
}

fn visible_oracle(table: &TableView, frame: Rect) -> Vec<(i32, Rect)> {
    let clip = table.content_clip(frame);
    all_frames(table, frame).into_iter()
        .filter(|(_, r)| r.y + r.h > clip.y && r.y < clip.y + clip.h)
        .collect()
}

fn assert_no_legacy_scan(table: &TableView, operation: &str) {
    let visits = table.take_row_visits();
    assert_eq!(visits, 0, "{operation} entered a counted legacy row scan");
}

fn warm(table: &TableView, frame: Rect) {
    table.draw_page(Painter::recording(), frame, &FixtureMeasure);
    table.take_row_visits();
}

fn assert_generic_geometry(table: &TableView, frame: Rect) {
    let all = all_frames(table, frame);
    let clip = table.content_clip(frame);
    assert_eq!(visible_frames(table, frame), visible_oracle(table, frame));
    for (i, rect) in &all {
        assert_eq!(table.row_frame(frame, *i), Some(*rect));
        let y = rect.y + rect.h * 0.5;
        let expected = if clip.contains(frame.x + frame.w * 0.5, y)
            && !(table.classic_page_rows().is_some() && y >= clip.y + clip.h) {
            all.iter().filter(|(_, r)| y >= r.y && y <= r.y + r.h)
                .map(|(i, _)| *i).last()
        } else { None };
        assert_eq!(table.hit_row(frame, frame.x + frame.w * 0.5, y), expected);
    }
}

#[test]
fn large_classic_page_avoids_legacy_scans_and_matches_general_layout() {
    let _serial = plx_base::testlock::serial();
    const N: usize = 10_049;
    let frame = Rect::new(80.0, 120.0, 1080.0, 810.0);
    let mut table = uniform(N, frame);
    table.take_row_visits();
    table.draw_page(Painter::recording(), frame, &FixtureMeasure);
    let cold = table.take_row_visits();
    assert_eq!(cold, N, "cold draw observes one catalogue-digit scan, not total classification work");

    for selected in [N / 2, N - 1] {
        table.sel = selected as i32;
        // Legacy counters catch repeated digit/full-layout scans; the stable
        // layout revision also detects an accidental mutable sections borrow.
        // Neither observation counts the eligibility proof or window visits.
        let layout_revision = table.layout_rev();
        for _ in 0..32 {
            table.take_row_visits();
            table.update(1.0 / 60.0, frame.h);
            table.draw_page(Painter::recording(), frame, &FixtureMeasure);
            assert_eq!(table.layout_rev(), layout_revision,
                "unchanged rows and metrics retain layout identity while animating");
            assert_no_legacy_scan(&table, "interleaved update + draw");
        }
        table.take_row_visits();
        table.update(1.0 / 60.0, frame.h);
        assert_no_legacy_scan(&table, "in-flight animation update");
        let in_flight = visible_oracle(&table, frame);
        table.take_row_visits();
        assert_eq!(visible_frames(&table, frame), in_flight);
        assert_no_legacy_scan(&table, "in-flight visible registration");
        for _ in 0..240 { table.update(1.0 / 60.0, frame.h); }
        let settled = table.scroll_pos();
        for offset in [0.0, 0.375] {
            // A fractional in-flight spring position must use the same window
            // as the retained general walk, rather than rounding to a page.
            table.scroll.jump(settled + offset);
            let all = all_frames(&table, frame);
            let expected = visible_oracle(&table, frame);
            table.take_row_visits();
            assert_eq!(visible_frames(&table, frame), expected);
            assert_no_legacy_scan(&table, "visible registration");

            let rect = all.iter().find(|(i, _)| *i == selected as i32).unwrap().1;
            assert_eq!(table.row_frame(frame, selected as i32), Some(rect));
            assert_no_legacy_scan(&table, "focus row geometry");
            assert_eq!(table.hit_row(frame, rect.x + rect.w * 0.5, rect.y + rect.h * 0.5), Some(selected as i32));
            assert_no_legacy_scan(&table, "pointer hit");

            let clip = table.content_clip(frame);
            let expected_ink: Vec<String> = all.iter()
                .filter(|(_, r)| r.y + r.h >= frame.y && r.y <= clip.y + clip.h)
                .map(|(i, _)| format!("window row {i}")).collect();
            let text = crate::draw_census::capture_text(||
                table.draw_page(Painter::recording(), frame, &FixtureMeasure));
            assert_no_legacy_scan(&table, "actual draw");
            let labels: Vec<String> = text.into_iter().map(|(s, _)| s)
                .filter(|s| s.starts_with("window row ")).collect();
            assert_eq!(labels, expected_ink, "paint keeps both touching boundary rows");
            table.list_focused = true;
            table.draw_page(Painter::recording(), frame, &FixtureMeasure);
            assert_no_legacy_scan(&table, "focused draw");
            table.list_focused = false;

            // At a shared boundary hit testing chooses the later row, whereas
            // visible registration excludes zero-area intersections.
            if let Some((i, r)) = expected.get(1) {
                assert_eq!(table.hit_row(frame, r.x + 1.0, r.y), Some(*i));
                assert_no_legacy_scan(&table, "shared-boundary hit");
            }
            assert_eq!(table.hit_row(frame, frame.x + 1.0, clip.y + clip.h), None);
            assert_no_legacy_scan(&table, "bottom remainder hit");

            let entry = EntryId(7);
            let cx = Cx::<ClassicHost> { views: UiMode::Classic, tick: Default::default(),
                measure: &FixtureMeasure, press: Default::default(), focus: Default::default(),
                owner: InputOwner::Entry(entry) };
            let layout = crate::geom::Table { table: &table, frame, group: GroupId(3), entry };
            let placed = Focusable::<ClassicHost>::place(&layout, &(selected as u32), &cx, At::Drawn).unwrap();
            assert_eq!(placed.rect, rect);
            assert_no_legacy_scan(&table, "shared Focusable::place");
            let key = FocusKey { entry, elem: selected as u32 };
            let expected_next = if selected + 1 == N { 0 } else { selected + 1 };
            assert!(matches!(Focusable::<ClassicHost>::neighbour(&layout, key, Dir::Down, &cx),
                Step::Move(next) if next.elem == expected_next as u32));
            table.take_row_visits(); // whole-list focus travel intentionally retains the general walk
            table.update(1.0 / 60.0, frame.h);
            assert_no_legacy_scan(&table, "animation update");
        }
    }
    for invalid in [-1, N as i32] {
        assert_eq!(table.row_frame(frame, invalid), None);
        assert_eq!(table.row_top(invalid), 0.0);
        assert_eq!(table.row_height(invalid), table.plain_row_h());
    }
}

#[test]
fn same_length_mutations_invalidate_cached_uniformity() {
    let _serial = plx_base::testlock::serial();
    let frame = Rect::new(20.0, 40.0, 900.0, 800.0);
    let mut table = uniform(257, frame);
    warm(&table, frame);
    table.sections[0].rows[1].detail = "A second line changes every later row's position".into();
    table.fit_notes(frame.w, &FixtureMeasure);
    assert_generic_geometry(&table, frame);
    table.take_row_visits();
    visible_frames(&table, frame);
    assert!(table.take_row_visits() >= 257, "a detail row must take the general path");

    table.sections[0].rows[1].detail.clear();
    warm(&table, frame);
    visible_frames(&table, frame);
    assert_no_legacy_scan(&table, "restored uniform rows");
    // Same count and allocation, obtained through a mutable section iterator.
    table.sections.iter_mut().next().unwrap().rows[2] = Row::separator();
    assert_generic_geometry(&table, frame);
    assert_eq!(table.row_frame(frame, 2), None);
    table.sections[0].rows[2] = Row::new("window row 2");
    warm(&table, frame);
    *table.row_mut(3).unwrap() = Row::note("Long wrapped note ".repeat(35));
    table.fit_notes(frame.w, &FixtureMeasure);
    assert!(table.sections[0].rows[3].note_lines.get() > 1);
    assert_generic_geometry(&table, frame);
    assert_eq!(table.row_frame(frame, 3), None);
    *table.row_mut(3).unwrap() = Row::new("window row 3");
    warm(&table, frame);
    // A retained mutable borrow, rather than a setter or a different length.
    { let rows = &mut table.sections[0].rows; rows[4].detail = "Borrowed edit".into(); }
    assert_generic_geometry(&table, frame);
    table.take_row_visits();
    visible_frames(&table, frame);
    assert!(table.take_row_visits() >= 257);
}

#[test]
fn section_move_default_and_measure_changes_do_not_reuse_stale_geometry() {
    let _serial = plx_base::testlock::serial();
    let frame = Rect::new(80.0, 100.0, 1080.0, 810.0);
    let mut table = uniform(137, frame);
    warm(&table, frame);
    let retained = std::mem::take(&mut table.sections);
    table.sections = Default::default();
    assert_eq!(table.row_frame(frame, 0), None);
    table.sections = retained;
    warm(&table, frame);
    let sections = std::mem::take(&mut table.sections);
    assert!(visible_frames(&table, frame).is_empty());
    let mut other = uniform(137, frame);
    other.sections[0].rows[0].catalogue_number = Some("1234567".into());
    warm(&other, frame);
    let other_sections = std::mem::take(&mut other.sections);
    let mut moved = uniform(1, frame);
    warm(&moved, frame);
    assert_eq!(moved.catalogue_max_digits(), 1);
    let old_layout = moved.layout_rev();
    moved.sections = sections;
    assert_eq!(moved.n_rows(), 137);
    assert_ne!(moved.layout_rev(), old_layout, "moving an older collection into a newer table changes layout identity");
    assert_eq!(moved.catalogue_max_digits(), 3, "the destination must not reuse its one-digit cache");
    warm(&moved, frame);
    visible_frames(&moved, frame);
    assert_no_legacy_scan(&moved, "moved sections after revalidation");
    moved.sections[0].rows[10].catalogue_number = Some("123456".into());
    assert_eq!(moved.catalogue_max_digits(), 6, "same-length direct mutation invalidates number width too");
    let edited_layout = moved.layout_rev();
    moved.sections = other_sections;
    assert_ne!(moved.layout_rev(), edited_layout, "a second older collection cannot inherit the first collection's stamp");
    assert_eq!(moved.catalogue_max_digits(), 7);
    let motion = |table: &TableView| {
        let mut canon = plx_machine::machine::Canon::new();
        table.write_motion(&mut canon);
        canon.finish()
    };
    let before_warm = motion(&moved);
    warm(&moved, frame);
    let _ = moved.layout_rev();
    assert_eq!(motion(&moved), before_warm, "derived caches are not canonical motion state");
    for (scale, rows) in [(200, 30), (100, 10), (150, 25)] {
        let mut measure = TypographyMeasure::new(FixtureMeasure);
        measure.set_font_scale(FontScale::from_percent(scale).unwrap());
        moved.set_classic_page_size(Some(rows), frame.h);
        moved.fit_notes(frame.w, &measure);
        assert_generic_geometry(&moved, frame);
        moved.take_row_visits();
        visible_frames(&moved, frame);
        assert_no_legacy_scan(&moved, "font/page relayout");
    }
    moved.set_title(Some("Page title".into()));
    assert_generic_geometry(&moved, frame);
    moved.sections[0].header = "Section heading".into();
    assert_generic_geometry(&moved, frame);
    moved.sections.push(Section::new("Second section").row(Row::new("Another row")));
    assert_generic_geometry(&moved, frame);
}

#[test]
fn variable_and_modern_tables_keep_general_layout_and_bulk_enumeration() {
    let _serial = plx_base::testlock::serial();
    let frame = Rect::new(40.0, 50.0, 1000.0, 400.0);
    let mut table = uniform(137, frame);
    table.set_classic(false);
    table.fit_notes(frame.w, &FixtureMeasure);
    assert!(table.classic_page_rows().is_none());
    table.scroll.jump(123.375);
    assert_generic_geometry(&table, frame);
    table.take_row_visits();
    table.draw_page(Painter::recording(), frame, &FixtureMeasure);
    assert!(table.take_row_visits() >= 137, "Modern retains general row layout");
    for classic in [false, true] {
        table.set_classic(classic);
        table.sections[0].rows[2] = Row::new("Two lines").detail("detail");
        table.sections[0].rows[5] = Row::note("Wrapped note ".repeat(40));
        table.sections[0].rows[7] = Row::separator();
        table.set_title(Some("A title".into()));
        table.sections[0].header = "A section".into();
        table.fit_notes(frame.w, &FixtureMeasure);
        assert_generic_geometry(&table, frame);
        let mut bulk = Vec::new();
        table.for_each_row_frame(frame, |i, r| bulk.push((i, r)));
        assert_eq!(bulk, all_frames(&table, frame), "bulk/nearest-seat callers still receive every row");
        assert_eq!(bulk.len(), 135);
        assert!(bulk.last().unwrap().1.y > frame.y + frame.h);
    }
}

#[test]
fn non_exact_float_geometry_falls_back_to_the_general_walk() {
    let _serial = plx_base::testlock::serial();
    let frame = Rect::new(0.0, 0.0, 900.0, 800.0);
    let table = uniform(257, frame);
    warm(&table, frame);
    // The public page fitter currently rounds heights. Exercise the guard too:
    // a future metric source must not silently replace repeated f32 addition by
    // multiplication when they can differ. This uses the existing metric field.
    for height in [32.1, 100_001.0] {
        let mut metrics = table.metrics.get();
        metrics.plain = height;
        table.metrics.set(metrics);
        let expected = all_frames(&table, frame);
        table.take_row_visits();
        assert_eq!(table.row_frame(frame, 256), Some(expected[256].1));
        assert!(table.take_row_visits() >= 257, "height {height} is outside the exact uniform certificate");
    }
}
