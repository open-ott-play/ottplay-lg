use super::*;
use crate::ott::{classic, navigation};
use plx_machine::machine::UiMode;

struct ClassicHost;
impl Host for ClassicHost {
    type Arg = crate::registry::AppArg;
    type Fx = AppFx;
    type Msg = AppMsg;
    type Elem = u32;
    type Views<'a> = ();
    type Init = crate::registry::AppArg;
    type Memory = PageMemory;
    fn ui_mode(_: &()) -> UiMode { UiMode::Classic }
}

fn event(screen: &mut OttScreen, ms: u32, event: ScreenEvent<ClassicHost>) -> bool {
    let cx = Cx::<ClassicHost> { views: (), tick: plx_machine::machine::Tick { ms, dt_us: 0 },
        measure: &plx_ui::fixture::FixtureMeasure, press: Default::default(), focus: Default::default(),
        owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
    let mut output = Vec::new();
    let mut present = plx_machine::present::Present::new();
    screen.step(&event, &cx, &mut Effects::new(&mut output, MachineId::Instance(InstanceId(1)), &mut present));
    assert!(!output.iter().any(|effect| matches!(effect.fx, Fx::App(_))), "display-only update");
    present.changed()
}

fn tick(screen: &mut OttScreen, ms: u32) -> bool {
    event(screen, ms, ScreenEvent::Tick(plx_machine::machine::Tick { ms, dt_us: 0 }))
}

fn painted(screen: &mut OttScreen) -> Vec<String> {
    plx_ui::draw_census::capture_runs(|| {
        let cx = Cx::<ClassicHost> { views: (), tick: Default::default(),
            measure: &plx_ui::fixture::FixtureMeasure, focus: Default::default(), press: Default::default(),
            owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
        let mut frame = DrawFrame::new(&cx, plx_ui::Painter::recording());
        plx_gfx::gfx::without_frame_clear(|| screen.draw(&mut frame));
    }).into_iter().filter(|run| run.ink[3] > 0.0).map(|run| run.text).collect()
}

fn fixture(next_description: &str) -> OttScreen {
    let mut view = (*snapshot(1, &["station"])).clone();
    view.rows[0].description = "STALE_SNAPSHOT".into();
    view.rows[0].guide_available = true;
    view.rows[0].guide = Some(OttNowNext { now_s: 1_700_000_010,
        current: Some(OttGuideProgramme { title: "NOW_A".into(), description: "SYNOPSIS_A".into(),
            start_s: 1_700_000_000, end_s: 1_700_000_060 }),
        next: Some(OttGuideProgramme { title: "NOW_B".into(), description: next_description.into(),
            start_s: 1_700_000_060, end_s: 1_700_000_120 }) });
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(Arc::new(view));
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    screen.classic_visible = true;
    tick(&mut screen, 0);
    screen
}

#[test]
fn classic_paint_never_substitutes_snapshot_text_for_a_missing_current_synopsis() {
    let _serial = plx_base::testlock::serial();
    for next in ["SYNOPSIS_B", ""] {
        let mut screen = fixture(next);
        let snapshot = screen.snapshot.clone();
        for (ms, expected) in [(0, "SYNOPSIS_A"), (50_000, next), (110_000, "")] {
            tick(&mut screen, ms);
            let text = painted(&mut screen);
            assert!(!text.iter().any(|run| run.contains("STALE_SNAPSHOT")));
            assert_eq!(text.iter().any(|run| run.contains("SYNOPSIS_A")), expected == "SYNOPSIS_A");
            assert_eq!(text.iter().any(|run| run.contains("SYNOPSIS_B")), expected == "SYNOPSIS_B");
            assert_eq!(screen.description(&screen.snapshot.rows[0]), expected);
            assert_eq!(screen.has_description(), !expected.is_empty());
            assert!(Arc::ptr_eq(&snapshot, &screen.snapshot), "clock updates do not fetch or replace the catalogue");
            assert!(screen.viewport_aperture().is_none(), "metadata does not manufacture a video ACK");
        }
    }
}

#[test]
fn classic_open_reader_repaints_at_rollover_and_expiry() {
    let _serial = plx_base::testlock::serial();
    let mut screen = fixture("SYNOPSIS_B");
    effects(|fx| screen.show_details(fx));
    assert!(screen.reading);
    assert!(painted(&mut screen).iter().any(|run| run.contains("SYNOPSIS_A")));
    assert!(tick(&mut screen, 50_000), "the open reader needs a repaint when its programme changes");
    let text = painted(&mut screen);
    assert!(text.iter().any(|run| run.contains("SYNOPSIS_B")));
    assert!(!text.iter().any(|run| run.contains("SYNOPSIS_A") || run.contains("STALE_SNAPSHOT")));
    assert!(tick(&mut screen, 110_000), "expiry clears already painted synopsis text");
    assert!(!painted(&mut screen).iter().any(|run| run.contains("SYNOPSIS_") || run.contains("STALE_SNAPSHOT")));
    assert!(screen.reading, "expiry does not navigate or change the reader owner");
    event(&mut screen, 110_000, ScreenEvent::Input(plx_machine::machine::InputEvent {
        at: Default::default(), source: plx_machine::machine::Source::Script,
        kind: InputKind::Key { key: Key::Back, sym: 0, wcode: 0, edge: Edge::Down, at_edge: false },
    }));
    assert!(!screen.reading);
    assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:station"));
}

#[test]
fn classic_browse_retires_only_description_when_its_current_synopsis_disappears() {
    let _serial = plx_base::testlock::serial();
    let mut screen = fixture("");
    effects(|fx| screen.open_browse(fx));
    let description = screen.keys["nav:description"];
    assert!(screen.form.index_of_key(RowKey(description)).is_some());
    let retained: Vec<_> = ["nav:play", "nav:guide", "nav:favorite", "nav:categories"]
        .into_iter().map(|id| (id, screen.keys[id])).collect();
    assert!(tick(&mut screen, 50_000));
    assert!(screen.browse);
    assert!(screen.form.index_of_key(RowKey(description)).is_none());
    for (id, key) in retained {
        assert_eq!(screen.keys[id], key);
        assert!(screen.form.index_of_key(RowKey(key)).is_some(), "{id} remains available");
    }
    assert!(effects(|fx| screen.activate(description, false, fx)).is_empty());
    assert!(screen.browse && !screen.reading, "a retired action cannot open stale text");
    effects(|fx| screen.close_browse(fx));
    let header = screen.header(&plx_ui::fixture::FixtureMeasure).unwrap();
    assert!(!header.stops.iter().any(|(key, _)| *key == description));
}

#[test]
fn classic_replacement_uses_its_own_guide_sample_and_non_guide_text_stays_available() {
    let _serial = plx_base::testlock::serial();
    let mut screen = fixture("SYNOPSIS_B");
    tick(&mut screen, 110_000);
    let mut replacement = (*screen.snapshot).clone();
    replacement.revision += 1;
    replacement.rows[0].guide = Some(OttNowNext { now_s: 1_700_001_010,
        current: Some(OttGuideProgramme { title: "NOW_C".into(), description: "SYNOPSIS_C".into(),
            start_s: 1_700_001_000, end_s: 1_700_001_060 }), next: None });
    event(&mut screen, 110_000, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(replacement))));
    assert!(screen.classic_mode);
    assert_eq!(screen.guide_clock.now(), Some(1_700_000_120), "new snapshot precedes the next clock tick");
    let text = painted(&mut screen);
    assert!(text.iter().any(|run| run.contains("NOW_C")));
    assert!(text.iter().any(|run| run.contains("SYNOPSIS_C")));
    assert!(!text.iter().any(|run| run.contains("STALE_SNAPSHOT")));
    assert_eq!(screen.description(&screen.snapshot.rows[0]), "SYNOPSIS_C");
    effects(|fx| screen.open_browse(fx));
    assert!(screen.form.index_of(&"nav:description".into()).is_some());
    effects(|fx| screen.close_browse(fx));

    let mut non_guide = (*screen.snapshot).clone();
    non_guide.revision += 1;
    non_guide.rows[0].guide = None;
    non_guide.rows[0].description = "FILE_SYNOPSIS".into();
    event(&mut screen, 110_000, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(non_guide))));
    assert_eq!(screen.description(&screen.snapshot.rows[0]), "FILE_SYNOPSIS");
    assert!(painted(&mut screen).iter().any(|run| run.contains("FILE_SYNOPSIS")));
    effects(|fx| screen.show_details(fx));
    assert!(screen.reading);
    assert!(painted(&mut screen).iter().any(|run| run.contains("FILE_SYNOPSIS")));
}

fn file_choices() -> OttScreen {
    let mut view = (*snapshot(1, &["torrent.file:23:lease-1:4", "torrent.file:23:lease-1:19", "torrent.retry"])).clone();
    view.page_id = "torrent-files".into();
    for (row, (title, name, detail)) in view.rows.iter_mut().zip([
        ("1  First.mp4", Some("First.mp4"), "1 MiB"),
        ("2  Folder/Second.mkv", Some("Folder/Second.mkv"), "2 MiB"),
        ("Retry", None, ""),
    ]) {
        row.title = title.into();
        row.content_title = name.map(str::to_owned);
        row.detail = detail.into();
        row.description = detail.into();
        row.action = OttAction::EditSetting(row.id.clone());
        assert!(row.media.is_none() && row.poster.is_none());
    }
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(Arc::new(view));
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    screen.classic_visible = true;
    screen
}

#[test]
fn classic_file_choice_metadata_preserves_its_exact_action_without_playback_authority() {
    let _serial = plx_base::testlock::serial();
    let mut screen = file_choices();
    assert_eq!(screen.media, Some(0));
    let id = "torrent.file:23:lease-1:19";
    let key = screen.keys[&format!("item:{id}")];
    event(&mut screen, 0, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: key }, by: plx_ui::screen::By::Dir });
    assert_eq!(screen.media, Some(1));
    assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:torrent.file:23:lease-1:19"));
    assert!(painted(&mut screen).iter().any(|run| run == "Folder/Second.mkv"));
    assert!(screen.has_description());
    let actions = navigation::available(&screen.snapshot, screen.media, Default::default());
    assert!(actions.iter().any(|action| action.id == "nav:description"));
    for unavailable in ["nav:play", "nav:favorite", "nav:guide", "nav:records"] {
        assert!(!actions.iter().any(|action| action.id == unavailable));
    }
    screen.preview_focus(key);
    assert!(screen.preview.is_none());
    assert!(effects(|fx| screen.preview_tick(1.0, fx)).is_empty());
    let cx = Cx::<ClassicHost> { views: (), tick: Default::default(),
        measure: &plx_ui::fixture::FixtureMeasure, focus: Default::default(), press: Default::default(),
        owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
    assert!(Focusable::<ClassicHost>::press_kind(&screen, &key, &cx).is_none());
    assert!(effects(|fx| screen.activate(key, true, fx)).is_empty(), "no fake card-hold action");
    let output = effects(|fx| screen.activate(key, false, fx));
    let actions: Vec<_> = output.iter().filter_map(|effect| match &effect.fx {
        Fx::App(AppFx::Ott(OttOut::Action { action, .. })) => Some(action), _ => None,
    }).collect();
    assert_eq!(actions, vec![&OttAction::EditSetting(id.into())]);
}

#[test]
fn classic_file_choice_reader_returns_to_its_row_and_retires_a_disabled_subject() {
    let _serial = plx_base::testlock::serial();
    let mut screen = file_choices();
    let key = screen.keys["item:torrent.file:23:lease-1:19"];
    event(&mut screen, 0, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: key }, by: plx_ui::screen::By::Dir });
    effects(|fx| screen.open_browse(fx));
    assert_eq!(screen.media, Some(1), "Browse retains the explicitly selected subject");
    effects(|fx| { assert!(screen.activate_context("nav:description", fx)); });
    assert!(screen.reading && !screen.browse);
    assert!(painted(&mut screen).iter().any(|run| run == "2 MiB"));
    event(&mut screen, 0, ScreenEvent::Input(plx_machine::machine::InputEvent {
        at: Default::default(), source: plx_machine::machine::Source::Script,
        kind: InputKind::Key { key: Key::Back, sym: 0, wcode: 0, edge: Edge::Down, at_edge: false },
    }));
    assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:torrent.file:23:lease-1:19"));
    effects(|fx| screen.show_details(fx));
    let mut disabled = (*screen.snapshot).clone();
    disabled.revision += 1;
    disabled.rows[1].enabled = false;
    event(&mut screen, 1, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(disabled))));
    assert!(!screen.reading && screen.media.is_none(), "do not substitute another file's description");
    assert!(effects(|fx| screen.activate(key, false, fx)).is_empty());
    screen.refresh_presentation(UiMode::Modern, &plx_ui::fixture::FixtureMeasure);
    assert!(screen.media.is_none() && !screen.grid_active(), "Modern file-choice presentation stays unchanged");
}

#[test]
fn classic_metadata_draws_equal_detail_and_description_once_but_keeps_distinct_text() {
    let _serial = plx_base::testlock::serial();
    let screen = file_choices();
    let layout = screen.classic_layout.get().unwrap();
    let mut row = screen.snapshot.rows[0].clone();
    for detail in ["1 MiB", "Distinct detail"] {
        row.detail = detail.into();
        let runs = plx_ui::draw_census::capture_runs(|| {
            classic::paint_left(plx_ui::Painter::recording(), layout, &row, "", false, None, Default::default(),
                &plx_ui::fixture::FixtureMeasure);
        });
        assert_eq!(runs.iter().filter(|run| run.text == "1 MiB").count(), 1);
        assert_eq!(runs.iter().any(|run| run.text == "Distinct detail"), detail == "Distinct detail");
    }
}

#[test]
fn classic_list_visibility_changes_presentation_without_changing_identity() {
    let _serial = plx_base::testlock::serial();
    let screen = fixture("SYNOPSIS_B");
    let mut row = screen.snapshot.rows[0].clone();
    row.title = "3450  Station".into(); row.content_title = Some("Station".into());
    let all = plx_appkit::classic_settings::Preferences::default();
    assert_eq!(classic::list_label(&row, all, Some(1_700_000_010)), "Station  NOW_A");
    assert_eq!(classic::list_label(&row, all.with_channel_display(0,0,1,1).unwrap(), Some(1_700_000_010)), "NOW_A");
    assert_eq!(classic::list_label(&row, all.with_channel_display(1,1,0,1).unwrap(), Some(1_700_000_010)), "Station");
    assert_eq!(classic::list_label(&row, all.with_channel_display(0,0,0,1).unwrap(), Some(1_700_000_010)), "");
    assert!(matches!(&row.action, OttAction::Play(id) if id == "station"));
    row.guide_available = false;
    row.media = Some(OttMedia { kind: OttMediaKind::Movie, duration_ms: None, progress: None });
    assert_eq!(classic::list_label(&row, all, Some(1_700_000_010)), "Station", "media does not duplicate its title as EPG");
    row.title = "2026 news".into(); row.content_title = None;
    assert_eq!(classic::list_label(&row, all, Some(1_700_000_010)), "2026 news");
}

#[test]
fn hidden_classic_synopsis_keeps_explicit_reader_and_modern_list() {
    let _serial = plx_base::testlock::serial();
    let mut screen = fixture("SYNOPSIS_B");
    let mut view = (*screen.snapshot).clone();
    view.revision += 1;
    view.classic_preferences = view.classic_preferences.with_channel_display(0,0,0,0).unwrap();
    event(&mut screen, 0, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(view))));
    assert!(!painted(&mut screen).iter().any(|s|s.contains("SYNOPSIS_A")));
    effects(|fx| screen.show_details(fx));
    assert!(screen.reading);
    assert!(painted(&mut screen).iter().any(|s|s.contains("SYNOPSIS_A")));
    screen.reading = false;
    screen.refresh_presentation(UiMode::Modern, &plx_ui::fixture::FixtureMeasure);
    let index = screen.form.index_of_key(RowKey(screen.keys["item:station"])).unwrap();
    assert_eq!(plx_ui::table::row_mut_in(&mut screen.form.table.sections, index).unwrap().label, "station");
}

#[test]
fn classic_programme_row_rolls_over_without_catalogue_reload_or_focus_change() {
    let _serial = plx_base::testlock::serial();
    let mut screen = fixture("SYNOPSIS_B");
    let view = screen.snapshot.clone();
    let focus = screen.form.selected_id().cloned();
    tick(&mut screen, 50_000);
    let index = screen.form.index_of_key(RowKey(screen.keys["item:station"])).unwrap();
    assert_eq!(plx_ui::table::row_mut_in(&mut screen.form.table.sections, index).unwrap().label, "station  NOW_B");
    assert_eq!(screen.form.selected_id(), focus.as_ref());
    assert!(Arc::ptr_eq(&view, &screen.snapshot));
}

#[test]
fn grouped_settings_only_suppress_fallback_back_for_an_owned_valid_action() {
    let _serial = plx_base::testlock::serial();
    for (id, action, enabled, owned) in [
        ("settings.draft:1:back", "settings.draft:1:back", true, true),
        ("settings.draft:1:back", "settings.draft:1:back", false, false),
        ("settings.draft:1:back", "settings.draft:2:back", true, false),
        ("settings.draft:01:back", "settings.draft:01:back", true, false),
        ("settings.draft:0:back", "settings.draft:0:back", true, false),
        ("settings.draft:x:back", "settings.draft:x:back", true, false),
        ("settings.draft:1:back:extra", "settings.draft:1:back:extra", true, false),
    ] {
        let mut view = (*snapshot(1, &[id])).clone();
        view.page_id = "settings".into();
        view.rows[0].action = OttAction::EditSetting(action.into());
        view.rows[0].enabled = enabled;
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        screen.install(Arc::new(view));
        assert_eq!(screen.form.index_of(&"nav:back".to_owned()).is_none(), owned, "{id} {action}");
    }
}

#[test]
fn classic_number_toggle_reaches_the_shared_table_painter() {
    let _serial = plx_base::testlock::serial();
    let mut screen = fixture("SYNOPSIS_B");
    for number in [1, 0] {
        let mut view = (*screen.snapshot).clone();
        view.revision += 1;
        view.rows[0].title = "3450  Station".into();
        view.rows[0].content_title = Some("Station".into());
        view.classic_preferences = view.classic_preferences.with_channel_display(number,1,1,1).unwrap();
        event(&mut screen, 0, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(view))));
        let runs = painted(&mut screen);
        assert_eq!(runs.iter().any(|run| run == "1"), number == 1);
        assert!(!runs.iter().any(|run| run == "3450"),
            "the visible list uses its ordinal, not the source channel number");
        assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:station"));
    }
}

#[test]
fn classic_header_cache_reflows_for_scale_changes_with_equal_body_widths() {
    use plx_base::fontcov::advances::ShippedMeasure;
    use plx_machine::machine::{FontScale, Measure, TypographyMeasure};
    let _serial = plx_base::testlock::serial();
    let mut measure = TypographyMeasure::new(ShippedMeasure);
    measure.set_font_scale(FontScale::from_percent(20).unwrap());
    let mut view = (*snapshot(1, &["station"])).clone();
    view.classic_preferences = plx_appkit::classic_settings::Preferences::from_values(10,0,0,15).unwrap();
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(Arc::new(view));
    screen.refresh_presentation(UiMode::Classic, &measure);
    let before = screen.header(&measure).unwrap();
    let body_width = measure.width_str("Fixture", theme::size::BODY, false);
    measure.set_font_scale(FontScale::from_percent(21).unwrap());
    screen.refresh_presentation(UiMode::Classic, &measure);
    assert_eq!(measure.width_str("Fixture", theme::size::BODY, false), body_width,
        "BODY metrics alone do not identify this scale change");
    let after = screen.header(&measure).unwrap();
    assert!(!std::rc::Rc::ptr_eq(&before, &after));
    assert_eq!(before.stops, after.stops, "a scale change preserves action identities");
    assert!(before.layout.items.iter().zip(&after.layout.items)
        .any(|(a,b)| (a.plate.x,a.plate.w,a.plate.h) != (b.plate.x,b.plate.w,b.plate.h)),
        "paint and hit geometry must use the new footer scale");
}

#[test]
fn validated_http_channel_guide_supplies_its_classic_programme_name() {
    let _serial = plx_base::testlock::serial();
    let screen = fixture("SYNOPSIS_B");
    let mut row = screen.snapshot.rows[0].clone();
    row.content_title = Some("Station".into());
    // HTTP locators retain no finite/live subtype. The admitted guide capability
    // is the source of truth, as it is for the adjacent Guide action.
    row.media = Some(OttMedia { kind: OttMediaKind::Video, duration_ms: None, progress: None });
    assert!(row.guide_available);
    let on = plx_appkit::classic_settings::Preferences::default();
    assert_eq!(classic::list_label(&row, on, Some(1_700_000_010)), "Station  NOW_A");
    assert_eq!(classic::list_label(&row, on.with_channel_display(1,1,0,1).unwrap(), Some(1_700_000_010)), "Station");
    row.guide_available = false;
    let retained_guide = row.guide.clone();
    for kind in [OttMediaKind::Movie, OttMediaKind::LocalVideo] {
        row.media.as_mut().unwrap().kind = kind;
        row.guide = retained_guide.clone();
        assert_eq!(classic::list_label(&row, on, Some(1_700_000_010)), "Station");
        row.guide = None;
        assert_eq!(classic::list_label(&row, on, Some(1_700_000_010)), "Station");
    }
}

#[test]
fn current_guide_synopsis_uses_the_panel_and_keeps_next_at_the_bottom() {
    use plx_base::fontcov::advances::ShippedMeasure;
    let _serial = plx_base::testlock::serial();
    let mut row = fixture("NEXT_BODY").snapshot.rows[0].clone();
    row.content_title = Some("World News".into());
    row.detail = "News".into();
    row.media = Some(OttMedia { kind: OttMediaKind::Video, duration_ms: None, progress: None });
    let description = "World News. Synthetic programme description for the OTTPlay visual comparison. No personal account or live channel is used.";
    let guide = row.guide.as_mut().unwrap();
    guide.current.as_mut().unwrap().title = "World News — current programme".into();
    guide.current.as_mut().unwrap().description = description.into();
    guide.next.as_mut().unwrap().title = "World News — next programme".into();
    let layout = plx_ui::classic::Layout::new(Rect::FULL, &ShippedMeasure, true);
    let runs = plx_ui::draw_census::capture_runs(|| plx_gfx::gfx::without_frame_clear(|| {
        classic::paint_left(plx_ui::Painter::recording(), layout, &row,
            "Could not open media source Loaded: 12 / 12", false, Some(1_700_000_010),
            Default::default(), &ShippedMeasure);
    }));
    let compact = |text: &str| text.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let all = runs.iter().filter(|run|run.ink[3]>0.0).map(|run|compact(&run.text)).collect::<String>();
    assert!(all.contains(&compact(description)), "the reference synopsis fits the genuine remaining panel: {all}");
    let last_description = runs.iter().filter(|run| run.text.contains("live channel") || run.text.contains("is used"))
        .map(|run|run.at.y).fold(f32::NEG_INFINITY, f32::max);
    let next = runs.iter().find(|run|run.text.contains("next programme")).unwrap();
    assert!(last_description.is_finite() && last_description < next.at.y,
        "upcoming programmes belong below the current synopsis");
    assert!(!runs.iter().any(|run|run.text == "News" || run.text == "World News"),
        "live details do not repeat the channel/category above the current programme");
    assert!(runs.iter().any(|run|run.text.contains("Could not open media source")), "status remains available");
}


#[test]
fn compact_programme_labels_do_not_require_the_selected_rows_full_guide() {
    use plx_data::ott::guide::rows::CurrentProgramme;
    let _serial = plx_base::testlock::serial();
    let mut row = fixture("SYNOPSIS_B").snapshot.rows[0].clone();
    row.content_title = Some("Station".into());
    row.media = Some(OttMedia { kind: OttMediaKind::Video, duration_ms: None, progress: None });
    let selected_guide = row.guide.take();
    row.current_programme = Some(CurrentProgramme {
        title: "Other programme".into(), start_s: 1_700_000_000, end_s: 1_700_000_060,
    });
    let on = plx_appkit::classic_settings::Preferences::default();
    assert_eq!(classic::list_label(&row, on, None), "Station  Other programme");
    assert!(row.guide.is_none(), "compact list data does not manufacture a full selected guide");
    assert_eq!(classic::list_label(&row, on, Some(1_700_000_010)), "Station  Other programme");
    assert_eq!(classic::list_label(&row, on.with_channel_display(1,1,0,1).unwrap(), None), "Station");
    assert_eq!(classic::list_label(&row, on, Some(1_699_999_999)), "Station");
    assert_eq!(classic::list_label(&row, on, Some(1_700_000_060)), "Station", "end is exclusive");
    row.guide = selected_guide;
    assert_eq!(classic::list_label(&row, on, Some(1_700_000_010)), "Station  NOW_A",
        "the selected guide follows its own accepted clock/projection");
    row.guide = None;
    row.current_programme.as_mut().unwrap().end_s = 1_700_000_000;
    assert_eq!(classic::list_label(&row, on, None), "Station", "invalid intervals never surface");
}

fn sidebar_runs(row: &OttRow, now: Option<i64>, preferences: plx_appkit::classic_settings::Preferences)
    -> Vec<plx_ui::draw_census::Run>
{
    let measure = plx_ui::fixture::FixtureMeasure;
    let layout = plx_ui::classic::Layout::new(Rect::FULL, &measure, true);
    plx_ui::draw_census::capture_runs(|| plx_gfx::gfx::without_frame_clear(|| {
        classic::paint_left(plx_ui::Painter::recording(), layout, row, "", false, now, preferences, &measure);
    }))
}

#[test]
fn compact_current_programme_reaches_the_sidebar_without_a_fabricated_next_or_synopsis() {
    use plx_data::ott::guide::rows::CurrentProgramme;
    let _serial = plx_base::testlock::serial();
    let _locale = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let mut row = fixture("NEXT_SYNOPSIS").snapshot.rows[0].clone();
    row.content_title = Some("CHANNEL_TITLE".into());
    row.guide = None;
    row.description.clear();
    row.current_programme = Some(CurrentProgramme {
        title: "COMPACT_CURRENT".into(), start_s: 1_700_000_000, end_s: 1_700_000_060,
    });
    let before = row.clone();
    let preferences = plx_appkit::classic_settings::Preferences::default();
    for program in [0, 1] {
        let preferences = preferences.with_channel_display(1, 1, program, 1).unwrap();
        let runs = sidebar_runs(&row, Some(1_700_000_010), preferences);
        assert!(runs.iter().any(|run| run.text == "COMPACT_CURRENT"));
        assert!(runs.iter().any(|run| run.text.contains(" - ")), "the known interval is displayed");
        assert!(!runs.iter().any(|run| run.text.contains("CHANNEL_TITLE") || run.text.contains("SYNOPSIS") || run.text.contains("NOW_B")));
        assert_eq!(classic::list_label(&row, preferences, Some(1_700_000_010)),
            if program == 1 { "CHANNEL_TITLE  COMPACT_CURRENT" } else { "CHANNEL_TITLE" });
        assert_eq!(classic::list_progress_interval(&row, Some(1_700_000_010)), Some((1_700_000_000, 1_700_000_060)));
    }
    assert_eq!(row, before, "paint does not promote compact data to a full guide or change its action");
}

#[test]
fn compact_sidebar_obeys_full_guide_authority_and_exclusive_expiry() {
    use plx_data::ott::guide::rows::CurrentProgramme;
    let _serial = plx_base::testlock::serial();
    let mut row = fixture("NEXT_SYNOPSIS").snapshot.rows[0].clone();
    row.current_programme = Some(CurrentProgramme {
        title: "COMPACT_OLD".into(), start_s: 1_700_000_000, end_s: 1_700_000_060,
    });
    for full in [true, false] {
        if !full { row.guide = Some(OttNowNext { now_s: 1_700_000_010, current: None, next: None }); }
        let runs = sidebar_runs(&row, Some(1_700_000_010), Default::default());
        assert_eq!(runs.iter().any(|run| run.text == "NOW_A"), full);
        assert!(!runs.iter().any(|run| run.text.contains("COMPACT_OLD")));
    }
    row.guide = None;
    for now in [1_699_999_999, 1_700_000_060, 1_700_000_090] {
        assert!(!sidebar_runs(&row, Some(now), Default::default()).iter().any(|run| run.text.contains("COMPACT_OLD")));
        assert_eq!(classic::list_progress_interval(&row, Some(now)), None);
    }
    row.current_programme.as_mut().unwrap().end_s = 1_700_000_000;
    assert!(!sidebar_runs(&row, None, Default::default()).iter().any(|run| run.text.contains("COMPACT_OLD")));
}

#[test]
fn classic_description_switch_hides_selected_poster_and_text_together() {
    let _serial = plx_base::testlock::serial();
    let mut row = file_choices().snapshot.rows[0].clone();
    row.description = "SELECTED_FILE_SYNOPSIS".into();
    // Empty Plex art resolves to the ordinary poster ground before consulting
    // any source/cache. This observes the real card path without image IO.
    row.poster = Some(OttPoster::Plex { server: 0, key: String::new() });
    let preferences = plx_appkit::classic_settings::Preferences::default();
    let ground = |row: &OttRow, preferences| {
        let measure = plx_ui::fixture::FixtureMeasure;
        let layout = plx_ui::classic::Layout::new(Rect::FULL, &measure, true);
        // A recording painter intentionally skips cards. The existing declared
        // host walk runs their normal logic but declares every primitive before GL.
        plx_ui::placeholder::capture_declared(|| {
            classic::paint_left(plx_ui::Painter::root(), layout, row, "", false, None, preferences, &measure);
        }).1.of(plx_ui::placeholder::Reason::CardGround)
    };
    let mut no_poster = row.clone();
    no_poster.poster = None;
    let hidden = preferences.with_channel_display(1, 1, 1, 0).unwrap();
    assert_eq!(ground(&no_poster, preferences), 0, "no unrelated card ground participates in this fixture");
    assert_eq!(ground(&row, preferences), 1, "the enabled poster really participates in paint");
    assert_eq!(ground(&no_poster, hidden), 0);
    assert_eq!(ground(&row, hidden), 0, "a hidden description leaves no poster ground");
    for (preferences, visible) in [(hidden, false), (preferences, true)] {
        let runs = sidebar_runs(&row, None, preferences);
        assert_eq!(runs.iter().any(|run| run.text == "SELECTED_FILE_SYNOPSIS"), visible);
        assert!(runs.iter().any(|run| run.text == "First.mp4"), "title is not part of the hidden synopsis");
        assert!(runs.iter().any(|run| run.text == "1 MiB"), "file detail remains visible");
    }
    assert_eq!(super::super::metadata::description(&row, None), "SELECTED_FILE_SYNOPSIS", "the explicit reader still owns the original text");
}

#[test]
fn classic_file_metadata_never_inherits_compact_television_programmes() {
    use plx_data::ott::guide::rows::CurrentProgramme;
    let _serial = plx_base::testlock::serial();
    let mut row = file_choices().snapshot.rows[0].clone();
    row.description = "SELECTED_FILE_SYNOPSIS".into();
    row.current_programme = Some(CurrentProgramme { title: "FOREIGN_PROGRAMME".into(), start_s: 10, end_s: 20 });
    row.guide_available = false;
    for kind in [OttMediaKind::Video, OttMediaKind::Movie, OttMediaKind::Episode, OttMediaKind::LocalVideo] {
        row.media = Some(OttMedia { kind, duration_ms: Some(12_000), progress: None });
        let runs = sidebar_runs(&row, Some(15), Default::default());
        assert!(runs.iter().any(|run| run.text == "First.mp4"));
        assert!(runs.iter().any(|run| run.text == "SELECTED_FILE_SYNOPSIS"));
        assert!(!runs.iter().any(|run| run.text.contains("FOREIGN_PROGRAMME")));
        assert_eq!(classic::list_progress_interval(&row, Some(15)), None);
        assert!(matches!(&row.action, OttAction::EditSetting(id) if id == "torrent.file:23:lease-1:4"));
    }
}

#[test]
fn classic_content_loading_and_empty_snapshots_retire_old_selected_metadata() {
    let _serial = plx_base::testlock::serial();
    let mut screen = fixture("NEXT_SYNOPSIS");
    let mut empty = (*screen.snapshot).clone();
    empty.revision += 1;
    empty.preview_context = Some("replacement-source".into());
    empty.rows.clear();
    empty.selected_row = None;
    empty.description = "REPLACEMENT_SOURCE".into();
    empty.catalog_progress = Some(OttCatalogProgress { loaded: 0, total: None, loading: true, complete: false });
    event(&mut screen, 0, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(empty.clone()))));
    assert!(screen.media.is_none() && !screen.reading && screen.status_working());
    let runs = painted(&mut screen);
    assert!(runs.iter().any(|run| run.contains("REPLACEMENT_SOURCE")));
    assert!(!runs.iter().any(|run| run.contains("NOW_A") || run.contains("NOW_B") || run.contains("SYNOPSIS")));
    empty.revision += 1;
    empty.catalog_progress = Some(OttCatalogProgress { loaded: 0, total: Some(0), loading: false, complete: true });
    event(&mut screen, 1, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(empty))));
    assert!(!screen.status_working() && screen.media.is_none());
    assert!(!painted(&mut screen).iter().any(|run| run.contains("NOW_A") || run.contains("NOW_B") || run.contains("SYNOPSIS")));
}


#[test]
fn progress_visibility_and_selected_rollover_are_independent_of_programme_names() {
    let _serial = plx_base::testlock::serial();
    let mut screen = fixture("SYNOPSIS_B");
    let mut view = (*screen.snapshot).clone();
    view.classic_preferences = view.classic_preferences.with_channel_display(1,1,0,1).unwrap()
        .with_show_progress(1).unwrap();
    screen.install(Arc::new(view));
    let original = screen.snapshot.clone();
    let key = screen.keys["item:station"];
    assert_eq!(screen.form.table.sections[0].rows[0].progress_bounds(), Some((1_700_000_000, 1_700_000_060)));
    assert!(!screen.form.table.sections[0].rows[0].label.contains("NOW_A"));
    assert!(tick(&mut screen, 50_000));
    assert_eq!(screen.form.table.sections[0].rows[0].progress_bounds(), Some((1_700_000_060, 1_700_000_120)));
    assert_eq!(screen.keys["item:station"], key);
    assert!(Arc::ptr_eq(&original, &screen.snapshot));
    assert!(tick(&mut screen, 110_000));
    assert_eq!(screen.form.table.sections[0].rows[0].progress_bounds(), None);
    let mut hidden = fixture("SYNOPSIS_B");
    let mut view = (*hidden.snapshot).clone();
    view.classic_preferences = view.classic_preferences.with_show_progress(0).unwrap();
    hidden.install(Arc::new(view));
    assert_eq!(hidden.form.table.sections[0].rows[0].progress_bounds(), None);
    assert!(hidden.form.table.sections[0].rows[0].label.contains("NOW_A"),
        "hiding progress does not hide the current programme name");
}

#[test]
fn compact_progress_uses_only_valid_live_intervals() {
    use plx_data::ott::guide::rows::CurrentProgramme;
    let _serial = plx_base::testlock::serial();
    let mut row = fixture("SYNOPSIS_B").snapshot.rows[0].clone();
    row.current_programme = Some(CurrentProgramme {
        title: "Other programme".into(), start_s: 1_700_001_000, end_s: 1_700_001_060,
    });
    assert_eq!(classic::list_progress_interval(&row, Some(1_700_000_010)), Some((1_700_000_000, 1_700_000_060)));
    row.guide = None;
    assert_eq!(classic::list_progress_interval(&row, None), Some((1_700_001_000, 1_700_001_060)));
    assert_eq!(classic::list_progress_interval(&row, Some(1_700_001_060)), None);
    row.current_programme.as_mut().unwrap().end_s = 1_700_001_000;
    assert_eq!(classic::list_progress_interval(&row, None), None);
    row.current_programme.as_mut().unwrap().end_s = 1_700_001_060;
    row.guide_available = false;
    row.media = Some(OttMedia { kind: OttMediaKind::LocalVideo, duration_ms: None, progress: None });
    assert_eq!(classic::list_progress_interval(&row, None), None);
}


#[test]
fn progress_picker_remains_a_static_classic_choice_with_its_owner_back() {
    let _serial = plx_base::testlock::serial();
    let mut view = (*snapshot(1, &["progress:on", "osd:back"])).clone();
    view.page_id = "classic-show-progress".into();
    view.rows[0].action = OttAction::EditSetting("osd:showProgress:1".into());
    view.rows[1].action = OttAction::ShowSettings;
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(Arc::new(view));
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    assert!(screen.form.table.is_classic_choices());
    assert!(!screen.form.table.is_classic_catalogue());
    assert!(screen.form.index_of(&"nav:refresh".to_owned()).is_none());
    assert!(screen.form.index_of(&"nav:back".to_owned()).is_none(), "only the owner's Back is admitted");
    assert!(screen.form.index_of(&"item:osd:back".to_owned()).is_some());
}


#[test]
fn authoritative_empty_full_guide_does_not_resurrect_compact_title_or_progress() {
    use plx_data::ott::guide::rows::CurrentProgramme;
    let _serial = plx_base::testlock::serial();
    let mut row = fixture("SYNOPSIS_B").snapshot.rows[0].clone();
    row.content_title = Some("Station".into());
    row.current_programme = Some(CurrentProgramme {
        title: "Older compact programme".into(), start_s: 1_700_000_000, end_s: 1_700_000_060,
    });
    row.guide.as_mut().unwrap().current = None;
    row.guide.as_mut().unwrap().next = None;
    for now in [None, Some(1_700_000_010)] {
        assert_eq!(classic::list_label(&row, Default::default(), now), "Station");
        assert_eq!(classic::list_progress_interval(&row, now), None);
    }
    row.guide = None;
    assert_eq!(classic::list_label(&row, Default::default(), Some(1_700_000_010)), "Station  Older compact programme");
    assert_eq!(classic::list_progress_interval(&row, Some(1_700_000_010)), Some((1_700_000_000, 1_700_000_060)),
        "compact data is used only in the absence of a full selected guide");
}

#[test]
fn classic_sidebar_omits_progress_without_changing_the_list_progress_switch() {
    let _serial = plx_base::testlock::serial();
    let mut screen = fixture("NEXT_SYNOPSIS");
    for enabled in [0, 1] {
        let mut view = (*screen.snapshot).clone();
        view.revision += 1;
        view.classic_preferences = view.classic_preferences.with_show_progress(enabled).unwrap();
        screen.install(Arc::new(view));
        let row = &screen.snapshot.rows[0];
        let measure = plx_ui::fixture::FixtureMeasure;
        let layout = plx_ui::classic::Layout::new(Rect::FULL, &measure, true);
        assert!(row.poster.is_none());
        let fills = plx_ui::draw_census::capture_inks(|| plx_gfx::gfx::without_frame_clear(|| {
            classic::paint_left(plx_ui::Painter::recording(), layout, row, "", false,
                Some(1_700_000_010), screen.snapshot.classic_preferences, &measure);
        }));
        assert!(fills.is_empty(), "a text-only sidebar has no extra resume/progress bar: {fills:?}");
        assert_eq!(screen.form.table.sections[0].rows[0].progress_bounds(),
            (enabled == 1).then_some((1_700_000_000, 1_700_000_060)),
            "the list row still follows showProgress");
        let runs = sidebar_runs(row, Some(1_700_000_010), screen.snapshot.classic_preferences);
        assert!(runs.iter().any(|run| run.text == "NOW_A"));
        assert!(runs.iter().any(|run| run.text.contains(" - ")), "the programme interval remains visible");
    }
}

#[test]
fn classic_sidebar_omits_the_permanent_hint_but_keeps_hold_and_actions() {
    let _serial = plx_base::testlock::serial();
    let _locale = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let mut screen = fixture("NEXT_SYNOPSIS");
    let mut view = (*screen.snapshot).clone();
    view.revision += 1;
    view.order_context = Some("sidebar-actions".into());
    screen.install(Arc::new(view));
    let hint = plx_platform::i18n::msg::widgets_hint_hold_options(plx_ui::hold_hint::key_label());
    let runs = sidebar_runs(&screen.snapshot.rows[0], Some(1_700_000_010), screen.snapshot.classic_preferences);
    assert!(!runs.iter().any(|run| run.text.contains(&hint)));
    let selected = screen.form.selected_id().cloned();
    let key = screen.keys["item:station"];
    let output = effects(|fx| screen.activate(key, true, fx));
    assert!(screen.browse, "the existing Classic hold still opens Browse");
    assert!(!output.iter().any(|effect| matches!(&effect.fx, Fx::App(_))));
    assert!(screen.form.index_of(&"nav:actions".to_owned()).is_some());
    let output = effects(|fx| assert!(screen.activate_context("nav:actions", fx)));
    let actions: Vec<_> = output.iter().filter_map(|effect| match &effect.fx {
        Fx::App(AppFx::Ott(OttOut::Action { action, .. })) => Some(action), _ => None,
    }).collect();
    assert_eq!(actions, vec![&OttAction::ShowActionsAt { context: "sidebar-actions".into(), id: "station".into() }]);
    assert!(!screen.browse);
    assert_eq!(screen.form.selected_id(), selected.as_ref());
}

#[test]
fn classic_sidebar_next_title_is_mint_and_its_clock_is_white_until_rollover() {
    let _serial = plx_base::testlock::serial();
    let _locale = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let screen = fixture("NEXT_SYNOPSIS");
    let row = &screen.snapshot.rows[0];
    let runs = sidebar_runs(row, Some(1_700_000_010), screen.snapshot.classic_preferences);
    let title = runs.iter().find(|run| run.text == "NOW_B").expect("actual next programme title");
    assert_eq!(title.ink, theme::CLASSIC_PROGRESS);
    assert!(!title.bold);
    let clock = runs.iter().find(|run| run.at.y == title.at.y && run.at.x < title.at.x)
        .expect("the shared TextView keeps the clock on the title's first line");
    assert!(!clock.text.is_empty());
    assert_eq!(clock.ink, theme::CLASSIC_TEXT);
    assert!(!clock.bold);
    let current = runs.iter().find(|run| run.text == "NOW_A").unwrap();
    assert_eq!(current.ink, theme::CLASSIC_TEXT);
    assert!(current.bold);
    let rolled = sidebar_runs(row, Some(1_700_000_060), screen.snapshot.classic_preferences);
    assert!(rolled.iter().any(|run| run.text == "NOW_B" && run.bold && run.ink == theme::CLASSIC_TEXT));
    assert!(!rolled.iter().any(|run| run.ink == theme::CLASSIC_PROGRESS), "no fabricated next after rollover");
    let expired = sidebar_runs(row, Some(1_700_000_120), screen.snapshot.classic_preferences);
    assert!(!expired.iter().any(|run| run.text.contains("NOW_A") || run.text.contains("NOW_B")));
}
