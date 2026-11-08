//! Current list identity must survive the Modern card route.
use super::*;
use plx_machine::machine::{FontScale, TypographyMeasure, UiMode};
use plx_ui::page_header::Slot;

fn catalog() -> OttScreen {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    let mut view = (*snapshot(1, &["one", "two"])).clone();
    view.title = "Channel list".into();
    view.category_label = Some("News / World".into());
    for row in &mut view.rows {
        row.media = Some(OttMedia { kind: OttMediaKind::LiveChannel,
            duration_ms: None, progress: None, synopsis: None });
    }
    screen.install(Arc::new(view));
    screen.refresh_presentation(UiMode::Modern, &plx_ui::fixture::FixtureMeasure);
    screen
}

#[test]
fn modern_grid_paints_current_list_and_category_without_a_new_focus_stop() {
    let _serial = plx_base::testlock::serial();
    let screen = catalog();
    assert!(screen.grid_active());
    let header = screen.header(&plx_ui::fixture::FixtureMeasure).unwrap();
    let caption = "Channel list · News / World";
    assert!(header.page.spec().left.iter().any(|item| item.title && item.label == caption));
    assert!(header.stops.iter().all(|(_, index)| header.layout.items[*index].focusable));
    assert_eq!(header.stops.iter().filter(|(key, _)| *key == screen.keys["nav:browse"]).count(), 1);
    let runs = plx_ui::draw_census::capture_runs(|| header.draw(plx_ui::Painter::recording(),
        0.0, &screen.header_capsules, 1.0, None));
    assert!(runs.iter().any(|run| run.text == caption));
}

#[test]
fn modern_caption_refreshes_between_categories_and_favorites_with_the_same_item() {
    let mut screen = catalog();
    let first = screen.header(&plx_ui::fixture::FixtureMeasure).unwrap();
    let mut view = (*screen.snapshot).clone();
    view.revision += 1;
    view.category_label = Some("Documentaries".into());
    screen.install(Arc::new(view));
    let changed = screen.header(&plx_ui::fixture::FixtureMeasure).unwrap();
    assert!(!std::rc::Rc::ptr_eq(&first, &changed));
    assert!(changed.page.spec().left.iter().any(|item| item.label == "Channel list · Documentaries"));
    let mut view = (*screen.snapshot).clone();
    view.revision += 1;
    view.page_id = "favorites:owner".into();
    view.title = "Favorites".into();
    view.category_label = Some("Favorites".into());
    screen.install(Arc::new(view));
    let favorites = screen.header(&plx_ui::fixture::FixtureMeasure).unwrap();
    assert!(favorites.page.spec().left.iter().any(|item| item.label == "Favorites"));
    assert_eq!(screen.snapshot.rows[0].id, "one");
}

#[test]
fn modern_caption_keeps_grid_geometry_and_browse_return_subject() {
    let mut screen = catalog();
    let route = screen.route();
    let key = screen.keys["item:two"];
    deliver(&mut screen, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: key }, by: plx_ui::screen::By::Dir });
    let opened = effects(|fx| screen.open_browse(fx));
    assert!(screen.browse);
    assert!(!screen.header(&plx_ui::fixture::FixtureMeasure).unwrap().page.spec().left
        .iter().any(|item| item.title), "the Browse table already paints its title");
    let closed = effects(|fx| screen.close_browse(fx));
    assert!(screen.grid_active());
    assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:two"));
    assert_eq!(screen.route().content, route.content);
    assert!(opened.iter().chain(&closed).all(|item| !matches!(&item.fx,
        Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::Play(_), .. })))));
}

#[test]
fn modern_long_caption_fits_shared_header_at_shipped_font_scales() {
    for language in plx_platform::i18n::SHIPPED {
        let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
        for scale in [20, 100, 200] {
            let mut measure = TypographyMeasure::new(plx_ui::fixture::FixtureMeasure);
            measure.set_font_scale(FontScale::from_percent(scale).unwrap());
            let mut screen = catalog();
            let mut view = (*screen.snapshot).clone();
            view.title = msg::ott_channel_list().into();
            view.category_label = Some("A long category / nested title / ".repeat(12));
            screen.install(Arc::new(view));
            let header = screen.header(&measure).unwrap();
            let title = header.page.spec().left.iter().find(|item| item.title).unwrap();
            assert!(!title.label.is_empty());
            assert!(header.layout.items.iter().any(|item| item.slot == Slot::Left(1) && !item.focusable));
            for (index, a) in header.layout.items.iter().enumerate() {
                assert!(a.plate.x >= 0.0 && a.plate.x + a.plate.w <= Rect::FULL.w);
                for b in header.layout.items.iter().skip(index + 1) {
                    assert!(a.plate.x + a.plate.w <= b.plate.x || b.plate.x + b.plate.w <= a.plate.x,
                        "{} at {}%", language.tag(), scale);
                }
            }
        }
    }
}

#[test]
fn modern_description_retires_expired_guide_text_before_opening_the_reader() {
    let mut screen = catalog();
    let mut view = (*screen.snapshot).clone();
    view.rows[0].description = "Stale synopsis".into();
    let guide = OttNowNext { now_s: 15,
        current: Some(OttGuideProgramme { title: "Now".into(), description: "Current synopsis".into(), start_s: 10, end_s: 20 }),
        next: Some(OttGuideProgramme { title: "Next".into(), description: "Next synopsis".into(), start_s: 20, end_s: 30 }) };
    view.rows[0].guide = Some(guide.clone());
    screen.install(Arc::new(view));
    screen.guide_clock.update(Some(&guide), 0);
    assert!(screen.has_description());
    screen.guide_clock.update(Some(&guide), 16_000);
    assert!(!screen.has_description());
    let out = effects(|fx| {
        assert!(!screen.activate_context("nav:description", fx));
        screen.show_details(fx);
    });
    assert!(!screen.reading);
    assert!(out.iter().all(|effect| !matches!(effect.fx, Fx::App(_))));
}

#[test]
fn modern_open_reader_uses_rolled_guide_body_instead_of_snapshot_synopsis() {
    let _serial = plx_base::testlock::serial();
    let mut screen = catalog();
    let mut view = (*screen.snapshot).clone();
    view.rows[0].description = "Stale synopsis".into();
    let guide = OttNowNext { now_s: 15,
        current: Some(OttGuideProgramme { title: "Now".into(), description: "Current synopsis".into(), start_s: 10, end_s: 20 }),
        next: Some(OttGuideProgramme { title: "Next".into(), description: "Next synopsis".into(), start_s: 20, end_s: 30 }) };
    view.rows[0].guide = Some(guide.clone());
    screen.install(Arc::new(view));
    screen.guide_clock.update(Some(&guide), 0);
    effects(|fx| screen.show_details(fx));
    assert!(screen.reading);
    screen.guide_clock.update(Some(&guide), 6_000);
    let measure = plx_ui::fixture::FixtureMeasure;
    let cx = Cx::<TestHost> { views: (), tick: Default::default(), measure: &measure,
        focus: Default::default(), press: Default::default(),
        owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
    let runs = plx_ui::draw_census::capture_runs(|| {
        let mut frame = DrawFrame::new(&cx, plx_ui::Painter::recording());
        plx_gfx::gfx::without_frame_clear(|| screen.draw(&mut frame));
    });
    assert!(runs.iter().any(|run| run.text == "Next synopsis"));
    assert!(runs.iter().all(|run| run.text != "Current synopsis" && run.text != "Stale synopsis"));
}

#[test]
fn modern_table_pages_keep_their_existing_title_without_duplicate_header_caption() {
    let mut screen = catalog();
    for page in ["providers", "profiles:m3u", "settings"] {
        let mut view = (*snapshot(2, &["one"])).clone();
        view.page_id = page.into();
        view.title = page.into();
        view.rows[0].action = OttAction::ShowProfiles;
        screen.install(Arc::new(view));
        assert!(!screen.grid_active());
        assert!(!screen.header(&plx_ui::fixture::FixtureMeasure).unwrap().page.spec().left
            .iter().any(|item| item.title));
    }
}

fn guide_only() -> (OttScreen, OttNowNext) {
    let mut screen = catalog();
    let mut view = (*screen.snapshot).clone();
    view.rows[0].description.clear();
    let guide = OttNowNext { now_s: 15,
        current: Some(OttGuideProgramme { title: "Now".into(), description: "Current guide-only synopsis".into(), start_s: 10, end_s: 20 }),
        next: None };
    view.rows[0].guide = Some(guide.clone());
    screen.install(Arc::new(view));
    screen.guide_clock.update(Some(&guide), 0);
    (screen, guide)
}

#[test]
fn modern_browse_uses_guide_only_description_and_keeps_its_subject() {
    let (mut screen, _) = guide_only();
    let subject = screen.snapshot.rows[screen.media.unwrap()].id.clone();
    effects(|fx| screen.open_browse(fx));
    let key = screen.keys["nav:description"];
    assert!(screen.form.index_of_key(RowKey(key)).is_some());
    let out = effects(|fx| screen.activate(key, false, fx));
    assert!(screen.reading);
    assert!(!screen.browse);
    assert_eq!(screen.snapshot.rows[screen.media.unwrap()].id, subject);
    assert!(out.iter().all(|effect| !matches!(&effect.fx,
        Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::Play(_), .. })))));
}

#[test]
fn modern_and_classic_browse_retire_expired_guide_description() {
    for classic in [false, true] {
        let (mut screen, guide) = guide_only();
        let mut view = (*screen.snapshot).clone();
        view.rows[0].description = "Stale snapshot synopsis".into();
        screen.install(Arc::new(view));
        screen.refresh_presentation(if classic { UiMode::Classic } else { UiMode::Modern },
            &plx_ui::fixture::FixtureMeasure);
        effects(|fx| screen.open_browse(fx));
        let description = screen.keys["nav:description"];
        assert!(screen.form.index_of_key(RowKey(description)).is_some());
        assert!(screen.has_description());
        assert_eq!(screen.description(&screen.snapshot.rows[0]), "Current guide-only synopsis");
        effects(|fx| screen.close_browse(fx));

        screen.guide_clock.update(Some(&guide), 6_000);
        effects(|fx| screen.open_browse(fx));
        assert!(screen.form.index_of_key(RowKey(description)).is_none());
        assert!(!screen.has_description());
        let out = effects(|fx| {
            assert!(!screen.activate_context("nav:description", fx));
            screen.show_details(fx);
        });
        assert!(!screen.reading);
        assert!(out.iter().all(|effect| !matches!(effect.fx, Fx::App(_))));
    }
}

#[test]
fn modern_guide_only_reader_survives_same_subject_snapshot_refresh() {
    let (mut screen, _) = guide_only();
    effects(|fx| screen.show_details(fx));
    screen.read_target = 4;
    let mut view = (*screen.snapshot).clone();
    view.revision += 1;
    view.status = "A harmless progress update".into();
    screen.install(Arc::new(view));
    assert!(screen.reading);
    assert_eq!(screen.read_target, 4);
    assert_eq!(screen.snapshot.rows[screen.media.unwrap()].id, "one");
}

#[test]
fn modern_expired_reader_refresh_retires_without_selecting_a_neighbour() {
    let (mut screen, guide) = guide_only();
    let mut view = (*screen.snapshot).clone();
    view.rows[0].description = "Stale snapshot synopsis".into();
    screen.install(Arc::new(view));
    effects(|fx| screen.show_details(fx));
    assert!(screen.reading);
    screen.guide_clock.update(Some(&guide), 6_000);
    let mut view = (*screen.snapshot).clone();
    view.revision += 1;
    screen.install(Arc::new(view));
    assert!(!screen.reading);
    assert_eq!(screen.media, None);
    assert_eq!(screen.focus_request, Some(screen.keys["nav:browse"]));
}

#[test]
fn modern_browse_open_through_expiry_removes_details_without_retargeting_media() {
    struct ModeHost;
    impl Host for ModeHost {
        type Arg = crate::registry::AppArg;
        type Fx = AppFx;
        type Msg = AppMsg;
        type Elem = u32;
        type Views<'a> = UiMode;
        type Init = crate::registry::AppArg;
        type Memory = PageMemory;
        fn ui_mode(mode: &UiMode) -> UiMode { *mode }
    }
    let (mut screen, _) = guide_only();
    let mut view = (*screen.snapshot).clone();
    view.rows[0].description = "Stale snapshot synopsis".into();
    screen.install(Arc::new(view));
    effects(|fx| screen.open_browse(fx));
    let details = screen.keys["nav:description"];
    deliver(&mut screen, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: details }, by: plx_ui::screen::By::Dir });
    assert_eq!(screen.form.selected_id().map(String::as_str), Some("nav:description"));
    for ms in [1_000, 6_000] {
        let tick = plx_machine::machine::Tick { ms, dt_us: 0 };
        let cx = Cx::<ModeHost> { views: UiMode::Modern, tick,
            measure: &plx_ui::fixture::FixtureMeasure, press: Default::default(),
            focus: Default::default(), owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
        let mut output = Vec::new();
        let mut present = plx_machine::present::Present::new();
        let mut fx = Effects::new(&mut output, MachineId::Instance(InstanceId(1)), &mut present);
        screen.step(&ScreenEvent::Tick(tick), &cx, &mut fx);
        assert_eq!(screen.form.index_of_key(RowKey(details)).is_some(), ms == 1_000);
        if ms == 1_000 {
            assert_eq!(screen.form.selected_id().map(String::as_str), Some("nav:description"));
        } else {
            assert_eq!(screen.form.selected_id().map(String::as_str), Some("nav:catalog"));
        }
        assert!(screen.browse);
        assert_eq!(screen.snapshot.rows[screen.media.unwrap()].id, "one");
        assert!(output.iter().all(|effect| !matches!(effect.fx, Fx::App(_))));
    }
}

#[test]
fn modern_browse_open_through_rollover_adds_new_synopsis_without_moving_focus() {
    struct ModeHost;
    impl Host for ModeHost {
        type Arg = crate::registry::AppArg;
        type Fx = AppFx;
        type Msg = AppMsg;
        type Elem = u32;
        type Views<'a> = UiMode;
        type Init = crate::registry::AppArg;
        type Memory = PageMemory;
        fn ui_mode(mode: &UiMode) -> UiMode { *mode }
    }
    let (mut screen, mut guide) = guide_only();
    guide.current.as_mut().unwrap().description.clear();
    guide.next = Some(OttGuideProgramme { title: "Next".into(),
        description: "Next synopsis".into(), start_s: 20, end_s: 30 });
    let mut view = (*screen.snapshot).clone();
    view.rows[0].guide = Some(guide.clone());
    view.rows[0].description.clear(); // Runtime projected the current empty synopsis.
    screen.install(Arc::new(view));
    screen.guide_clock.update(Some(&guide), 0);
    effects(|fx| screen.open_browse(fx));
    assert!(screen.form.index_of_key(RowKey(screen.keys["nav:description"])).is_none());
    let focus = screen.form.selected_id().cloned();
    let tick = plx_machine::machine::Tick { ms: 6_000, dt_us: 0 };
    let cx = Cx::<ModeHost> { views: UiMode::Modern, tick,
        measure: &plx_ui::fixture::FixtureMeasure, press: Default::default(),
        focus: Default::default(), owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
    let mut output = Vec::new();
    let mut present = plx_machine::present::Present::new();
    let mut fx = Effects::new(&mut output, MachineId::Instance(InstanceId(1)), &mut present);
    screen.step(&ScreenEvent::Tick(tick), &cx, &mut fx);
    assert!(screen.form.index_of_key(RowKey(screen.keys["nav:description"])).is_some());
    assert_eq!(screen.form.selected_id(), focus.as_ref());
    assert_eq!(screen.snapshot.rows[screen.media.unwrap()].id, "one");
    assert!(output.iter().all(|effect| !matches!(effect.fx, Fx::App(_))));
}

#[test]
fn modern_browse_hidden_description_does_not_rebuild_on_each_guide_second() {
    use plx_appkit::menu_visibility::{self, Item, Visibility};
    struct Reset(Visibility);
    impl Drop for Reset { fn drop(&mut self) { menu_visibility::project(self.0); } }
    struct ModeHost;
    impl Host for ModeHost {
        type Arg = crate::registry::AppArg;
        type Fx = AppFx;
        type Msg = AppMsg;
        type Elem = u32;
        type Views<'a> = UiMode;
        type Init = crate::registry::AppArg;
        type Memory = PageMemory;
        fn ui_mode(mode: &UiMode) -> UiMode { *mode }
    }
    let _serial = plx_base::testlock::serial();
    let _reset = Reset(menu_visibility::projected());
    let mut hidden = Visibility::default();
    hidden.hide(Item::Description);
    menu_visibility::project(hidden);
    let (mut screen, _) = guide_only();
    effects(|fx| screen.open_browse(fx));
    let focus = screen.form.selected_id().cloned();
    assert!(screen.has_description());
    for ms in [1_000, 2_000] {
        let tick = plx_machine::machine::Tick { ms, dt_us: 0 };
        let cx = Cx::<ModeHost> { views: UiMode::Modern, tick,
            measure: &plx_ui::fixture::FixtureMeasure, press: Default::default(),
            focus: Default::default(), owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
        let mut output = Vec::new();
        let mut present = plx_machine::present::Present::new();
        // A fresh gate owes its initial paint; measure only work caused by this Tick.
        assert!(present.take(0));
        assert!(!present.changed(), "fixture must start after its initial paint");
        let mut fx = Effects::new(&mut output, MachineId::Instance(InstanceId(1)), &mut present);
        screen.step(&ScreenEvent::Tick(tick), &cx, &mut fx);
        assert!(!present.changed(), "hidden Description must not rebuild Browse each second");
        assert!(screen.form.index_of_key(RowKey(screen.keys["nav:description"])).is_none());
        assert_eq!(screen.form.selected_id(), focus.as_ref());
    }
}

#[test]
fn modern_new_guide_sample_is_not_evaluated_against_the_previous_sample_clock() {
    let (mut screen, old) = guide_only();
    effects(|fx| screen.show_details(fx));
    assert!(screen.reading);
    screen.guide_clock.update(Some(&old), 40_000); // Old sample now reads 55.
    let mut view = (*screen.snapshot).clone();
    view.revision += 1;
    view.rows[0].guide = Some(OttNowNext { now_s: 105,
        current: Some(OttGuideProgramme { title: "Fresh".into(), description: "New current synopsis".into(), start_s: 100, end_s: 120 }),
        next: None });
    screen.install(Arc::new(view));
    assert!(screen.reading, "the fresh current synopsis must survive before the next Tick");
    assert!(screen.has_description());
    assert_eq!(screen.description(&screen.snapshot.rows[0]), "New current synopsis");
    assert_eq!(screen.guide_clock.now(), Some(55), "the Classic clock API retains its contract");
}
