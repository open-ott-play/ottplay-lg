use super::*;
use crate::ott::navigation;
use plx_machine::machine::{UiMode, FontScale, TypographyMeasure};

#[test]
fn classic_channel_footer_uses_current_capabilities_and_short_labels() {
    use plx_appkit::menu_visibility::{Item, Visibility};
    use plx_platform::i18n::msg;
    let _serial = plx_base::testlock::serial();
    let _locale = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let measure = plx_ui::fixture::FixtureMeasure;
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    let mut view = (*snapshot(1, &["one", "two"])).clone();
    view.page_id = "channels:filtered-owner".into();
    view.order_context = Some("owned-order".into());
    view.rows[0].guide_available = true;
    screen.install(Arc::new(view));
    screen.refresh_presentation(UiMode::Classic, &measure);
    let ids = |screen: &OttScreen| screen.header(&measure).unwrap().stops.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    assert_eq!(ids(&screen), ["nav:guide", "nav:categories", "nav:actions", "nav:description", "nav:favorite", "nav:browse"]
        .map(|id| screen.keys[id]));
    let before = screen.header(&measure).unwrap();
    let actions = navigation::available(&screen.snapshot, screen.media, Visibility::default());
    let controls = navigation::classic_controls(&screen.snapshot, &actions, &screen.keys);
    assert_eq!(controls.iter().find(|(id, _)| *id == screen.keys["nav:guide"]).unwrap().1, msg::ott_classic_epg());
    assert_eq!(controls.iter().find(|(id, _)| *id == screen.keys["nav:categories"]).unwrap().1, msg::ott_classic_category());

    let second = screen.keys["item:two"];
    classic_deliver(&mut screen, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: second }, by: plx_ui::screen::By::Dir });
    assert_eq!(ids(&screen), ["nav:description", "nav:categories", "nav:actions", "nav:favorite", "nav:browse"]
        .map(|id| screen.keys[id]), "Description replaces unavailable EPG without a duplicate");
    assert!(!std::rc::Rc::ptr_eq(&before, &screen.header(&measure).unwrap()), "selected capabilities invalidate the footer");
    assert!(effects(|fx| screen.activate(screen.keys["nav:guide"], false, fx)).is_empty(), "old guide key stays inert");

    let mut hidden = Visibility::default();
    hidden.hide(Item::Description); hidden.hide(Item::Categories);
    let actions = navigation::available(&screen.snapshot, screen.media, hidden);
    let controls = navigation::classic_controls(&screen.snapshot, &actions, &screen.keys);
    assert_eq!(controls.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![screen.keys["nav:actions"], screen.keys["nav:favorite"]]);
    let mut disabled = (*screen.snapshot).clone();
    disabled.revision += 1;
    disabled.rows[1].enabled = false;
    let actions = navigation::available(&disabled, Some(1), Visibility::default());
    let controls = navigation::classic_controls(&disabled, &actions, &screen.keys);
    assert_eq!(controls.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![screen.keys["nav:categories"], screen.keys["nav:actions"]], "disabled metadata cannot advertise a selected-item action");
}

#[test]
fn compact_classic_footer_preserves_browse_actions_and_modern_header() {
    let _serial = plx_base::testlock::serial();
    let measure = plx_ui::fixture::FixtureMeasure;
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["one"]));
    screen.refresh_presentation(UiMode::Modern, &measure);
    let modern = screen.header(&measure).unwrap();
    let modern_stops = modern.stops.clone();
    screen.refresh_presentation(UiMode::Classic, &measure);
    let compact = screen.header(&measure).unwrap();
    for id in ["nav:play", "nav:favorites"] {
        assert!(!compact.stops.iter().any(|(key, _)| *key == screen.keys[id]));
    }
    assert!(compact.stops.iter().any(|(key, _)| *key == screen.keys["nav:favorite"]),
        "the current item's Favorite action is directly discoverable");
    assert!(compact.stops.iter().any(|(key, _)| *key == screen.keys["nav:browse"]));
    let actions = navigation::available(&screen.snapshot, screen.media, Default::default());
    let controls = navigation::classic_controls(&screen.snapshot, &actions, &screen.keys);
    let prior: Vec<_> = controls.iter().filter(|(key, _)|
        !["nav:search", "nav:favorite"].iter().any(|id| screen.keys.get(*id) == Some(key))).cloned().collect();
    for percent in [20, 100, 200] {
        let mut scaled = TypographyMeasure::new(plx_ui::fixture::FixtureMeasure);
        scaled.set_font_scale(FontScale::from_percent(percent).unwrap());
        for width in [640.0, 1280.0, 1920.0] {
            let shortcuts = navigation::classic_footer_keys(&screen.snapshot, &controls, &screen.keys);
            let baseline = navigation::context_header_classic(&scaled, width, screen.keys["nav:browse"],
                &prior, false, &shortcuts, 33);
            let expanded = navigation::context_header_classic(&scaled, width, screen.keys["nav:browse"],
                &controls, false, &shortcuts, 33);
            assert!(expanded.classic_footer_fits(width));
            for (key, _) in &baseline.stops {
                assert!(expanded.stops.iter().any(|(candidate, _)| candidate == key),
                    "new common actions cannot displace an existing priority at {width}/{percent}%");
            }
            assert!(expanded.stops.iter().any(|(key, _)| *key == screen.keys["nav:browse"]),
                "overflow must retain the path to every contextual action");
        }
    }
    effects(|fx| screen.open_browse(fx));
    for id in ["nav:play", "nav:favorite", "nav:favorites"] {
        assert!(screen.form.index_of(&id.to_owned()).is_some(), "{id} retains its ordinary Browse route");
    }
    effects(|fx| screen.close_browse(fx));
    assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:one"));
    screen.refresh_presentation(UiMode::Modern, &measure);
    assert_eq!(screen.header(&measure).unwrap().stops, modern_stops);
}

#[test]
fn switching_presentations_preserves_catalogue_selection_and_actions() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["one", "two"]));
    let target = screen.keys["item:two"];
    deliver(&mut screen, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: target }, by: plx_ui::screen::By::Dir });
    for mode in [UiMode::Classic, UiMode::Modern, UiMode::Classic] {
        screen.refresh_presentation(mode, &plx_ui::fixture::FixtureMeasure);
        assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:two"));
        assert_eq!(screen.keys["item:two"], target);
        let out = effects(|fx| screen.activate(target, false, fx));
        assert!(out.iter().any(|effect| matches!(&effect.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::Play(id), .. })) if id == "two")));
        if mode == UiMode::Classic { assert!(!screen.grid_active()); }
    }
}

#[test]
fn classic_provider_category_and_settings_lists_use_the_same_content_frame() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    for page in ["providers", "profiles:m3u", "categories:fixture", "channels:fixture", "favorites:fixture", "settings"] {
        let mut view = (*snapshot(1, &["one", "two"])).clone();
        view.page_id = page.into();
        screen.install(Arc::new(view));
        screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
        let layout = screen.classic_layout.get().unwrap();
        assert_eq!(screen.view(&plx_ui::fixture::FixtureMeasure).table.frame, layout.list);
        assert!(layout.list.y + layout.list.h < layout.hints.y);
    }
}

#[test]
fn classic_editor_keeps_save_cancel_and_back_at_large_font() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["one"]));
    editor(&mut screen, "draft source");
    let mut measure = TypographyMeasure::new(plx_ui::fixture::FixtureMeasure);
    measure.set_font_scale(FontScale::from_percent(200).unwrap());
    screen.refresh_presentation(UiMode::Classic, &measure);
    let frame = screen.view(&measure).table.frame;
    screen.form.table.fit_layout(&measure);
    screen.form.table.update(1.0, frame.h);
    for id in ["edit:save", "edit:cancel"] {
        let i = screen.form.index_of(&id.to_owned()).unwrap();
        let row = screen.form.table.row_frame(frame, i as i32).unwrap();
        assert!(row.y >= frame.y && row.y + row.h <= frame.y + frame.h);
        assert_eq!(screen.form.table.hit_row(frame, row.x + row.w * 0.5, row.y + row.h * 0.5), Some(i as i32));
    }
    assert!(screen.header(&measure).is_none());
    effects(|fx| screen.cancel_editor(fx));
    assert!(screen.editor.is_none());
    assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:one"));
}

#[test]
fn classic_disabled_item_remains_unplayable_and_never_becomes_preview() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    let mut view = (*snapshot(1, &["disabled", "available"])).clone();
    view.rows[0].enabled = false;
    screen.install(Arc::new(view));
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    let key = screen.keys["item:disabled"];
    screen.preview_focus(key);
    for hold in [false, true] { assert!(effects(|fx| screen.activate(key, hold, fx)).is_empty()); }
    assert!(screen.preview.is_none());
}

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
fn tick_mode(screen: &mut OttScreen, ms: u32, mode: UiMode, present: &mut plx_machine::present::Present) {
    let tick = plx_machine::machine::Tick { ms, dt_us: 0 };
    let cx = Cx::<ModeHost> { views: mode, tick, measure: &plx_ui::fixture::FixtureMeasure,
        press: Default::default(), focus: Default::default(),
        owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
    let mut output = Vec::new();
    let mut fx = Effects::new(&mut output, MachineId::Instance(InstanceId(1)), present);
    screen.step(&ScreenEvent::Tick(tick), &cx, &mut fx);
}
#[test]
fn committed_mode_is_observed_on_an_ordinary_tick_without_input() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["one"]));
    let key = screen.keys["item:one"];
    let mut present = plx_machine::present::Present::new();
    tick_mode(&mut screen, 0, UiMode::Classic, &mut present);
    assert!(screen.classic_mode);
    assert_eq!(screen.keys["item:one"], key);
    tick_mode(&mut screen, 1, UiMode::Modern, &mut present);
    assert!(!screen.classic_mode);
    assert_eq!(screen.keys["item:one"], key);
}
#[test]
fn idle_visible_guide_repaints_on_seconds_and_expiry_but_hidden_or_expired_does_not() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    let mut view = (*snapshot(1, &["live"])).clone();
    view.rows[0].guide = Some(OttNowNext { now_s: 15,
        current: Some(OttGuideProgramme { title: "one".into(), description: String::new(), start_s: 10, end_s: 20 }),
        next: Some(OttGuideProgramme { title: "two".into(), description: String::new(), start_s: 20, end_s: 30 }) });
    screen.install(Arc::new(view));
    screen.classic_visible = true;
    let mut present = plx_machine::present::Present::new();
    tick_mode(&mut screen, 0, UiMode::Classic, &mut present);
    present.take(0);
    tick_mode(&mut screen, 500, UiMode::Classic, &mut present);
    assert!(!present.changed());
    tick_mode(&mut screen, 6000, UiMode::Classic, &mut present);
    assert!(present.changed());
    assert_eq!(screen.guide_clock.now(), Some(21));
    present.take(6000);
    screen.classic_visible = false;
    tick_mode(&mut screen, 7000, UiMode::Classic, &mut present);
    assert!(!present.changed());
    screen.classic_visible = true;
    tick_mode(&mut screen, 16000, UiMode::Classic, &mut present);
    assert!(present.changed());
    assert_eq!(screen.guide_clock.now(), Some(31));
    present.take(16000);
    tick_mode(&mut screen, 17000, UiMode::Classic, &mut present);
    assert!(!present.changed());
}
#[test]
fn classic_retains_current_setting_and_provider_details() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    let mut view = (*snapshot(1, &["mode"])).clone();
    view.rows[0].action = OttAction::EditSetting("interfaceMode".into());
    view.rows[0].detail = "Classic".into();
    for (page, compact) in [("settings", true), ("profile-setup:m3u", false)] {
        view.page_id = page.into();
        screen.install(Arc::new(view.clone()));
        screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
        assert_eq!(screen.form.table.is_classic_choices(), compact);
        let row = screen.form.table.sections.iter().flat_map(|s| &s.rows)
            .find(|row| row.label == "mode").expect("current setting remains visible");
        // Compact Settings puts the value on the same line; provider forms keep their detail.
        if compact {
            assert_eq!(row.value.as_deref(), Some("Classic"));
            assert!(row.detail.is_empty());
        } else {
            assert_eq!(row.detail, "Classic");
            assert_eq!(row.value.as_deref(), None);
        }
        assert_eq!(screen.snapshot.rows[0].detail, "Classic", "presentation keeps the source detail");
        let index = screen.form.index_of(&"item:mode".to_owned()).unwrap();
        assert!(matches!(screen.form.activate(index),
            Some(Activation::Action(FormAction::Forward(OttAction::EditSetting(key))))
                if key == "interfaceMode"));
    }
}

#[test]
fn classic_viewport_targets_browse_forms_without_starting_selected_media() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["one"]));
    screen.classic_visible = true;
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    let expected = screen.viewport_target().expect("visible Classic list");
    effects(|fx| screen.open_browse(fx));
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    assert_eq!(screen.viewport_target(), Some(expected));
    assert!(screen.viewport_aperture().is_none(), "a request is not an admitted hole");
    let requests = effects(|fx| screen.sync_viewport(fx));
    assert!(requests.is_empty(), "no native Source capability means no request");
    screen.classic_visible = false;
    assert!(screen.viewport_target().is_none());
    screen.classic_visible = true;
    screen.reading = true;
    assert!(screen.viewport_target().is_none());
    screen.reading = false;
    screen.refresh_presentation(UiMode::Modern, &plx_ui::fixture::FixtureMeasure);
    assert!(screen.viewport_target().is_none());
}

#[test]
fn classic_hold_opens_actions_tap_plays_and_modern_hold_still_toggles_favorite() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["one", "two"]));
    let key = screen.keys["item:one"];
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    assert!(screen.form.index_of(&"nav:back".into()).is_none(), "media rows have no fake channel footer");
    assert!(screen.form.table.is_classic_catalogue());
    let tap = effects(|fx| screen.activate(key, false, fx));
    assert!(tap.iter().any(|effect| matches!(&effect.fx, Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::Play(id), .. })) if id == "one")));
    let hold = effects(|fx| screen.activate(key, true, fx));
    assert!(screen.browse);
    assert!(!screen.form.table.is_classic_catalogue());
    assert!(!hold.iter().any(|effect| matches!(effect.fx, Fx::App(_))));
    for id in ["nav:categories", "nav:favorite", "nav:settings", "nav:back"] {
        assert!(screen.form.index_of(&id.to_owned()).is_some(), "{id}");
    }
    effects(|fx| screen.close_browse(fx));
    assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:one"));
    screen.refresh_presentation(UiMode::Modern, &plx_ui::fixture::FixtureMeasure);
    let hold = effects(|fx| screen.activate(key, true, fx));
    assert!(!screen.browse);
    assert!(hold.iter().any(|effect| matches!(&effect.fx, Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::ToggleFavorite(id), .. })) if id == "one")));
}

#[test]
fn disabled_only_classic_list_keeps_reachable_controls_and_does_not_open_item_actions() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    let mut view = (*snapshot(1, &["one"])).clone(); view.rows[0].enabled = false;
    screen.install(Arc::new(view));
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    assert!(!screen.form.table.is_classic_catalogue());
    assert!(screen.form.index_of(&"nav:back".into()).is_some());
    let key = screen.keys["item:one"];
    let hold = effects(|fx| screen.activate(key, true, fx));
    assert!(!screen.browse);
    assert!(!hold.iter().any(|effect| matches!(effect.fx, Fx::App(_))));
}

fn classic_deliver(screen: &mut OttScreen, event: ScreenEvent<ModeHost>) {
    let cx = Cx::<ModeHost> { views: UiMode::Classic, tick: Default::default(),
        measure: &plx_ui::fixture::FixtureMeasure, press: Default::default(), focus: Default::default(),
        owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
    let mut present = plx_machine::present::Present::new(); let mut output = Vec::new();
    let mut fx = Effects::new(&mut output, MachineId::Instance(InstanceId(1)), &mut present);
    screen.step(&event, &cx, &mut fx);
}

#[test]
fn classic_caption_uses_projected_category_without_parsing_profile_or_page_id() {
    let mut view = (*snapshot(1, &["one"])).clone();
    view.title = "Channel list".into(); view.description = "Profile name".into();
    view.page_id = "channels:opaque-owner".into();
    assert_eq!(navigation::caption(&view), "Channel list");
    view.category_label = Some("News / World".into());
    assert_eq!(navigation::caption(&view), "Channel list · News / World");
    view.title = "Favorites".into(); view.category_label = Some("Favorites".into());
    assert_eq!(navigation::caption(&view), "Favorites");
}

#[test]
fn classic_context_action_tracks_current_item_without_rebuilding_the_catalogue() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    let mut view = (*snapshot(1, &["one", "two"])).clone();
    for row in &mut view.rows { row.description = "Description".into(); row.guide_available = true; }
    screen.install(Arc::new(view.clone()));
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    let second = screen.keys["item:two"];
    classic_deliver(&mut screen, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: second }, by: plx_ui::screen::By::Dir });
    // Both descriptions exist, so this focus move did not require an install.
    assert_eq!(screen.snapshot.revision, 1);
    for (id, expect) in [("nav:play", 0), ("nav:guide", 1), ("nav:favorite", 2)] {
        let key = screen.keys[id];
        let out = effects(|fx| screen.activate(key, false, fx));
        assert!(out.iter().any(|effect| match &effect.fx {
            Fx::App(AppFx::Ott(OttOut::Action { action, .. })) => match (expect, action) {
                (0, OttAction::Play(id)) | (1, OttAction::OpenGuide(id)) | (2, OttAction::ToggleFavorite(id)) => id == "two",
                _ => false,
            }, _ => false,
        }), "{id} must use the current selected item");
    }
    // A capability that disappears cannot be activated by an old key.
    view.revision = 2; view.rows[1].guide_available = false;
    screen.install(Arc::new(view));
    let key = screen.keys["nav:guide"];
    assert!(effects(|fx| screen.activate(key, false, fx)).is_empty());
}

#[test]
fn classic_context_controls_preserve_visibility_disabled_rows_and_browse_escape() {
    use plx_appkit::menu_visibility::{Item, Visibility};
    let mut view = (*snapshot(1, &["one"])).clone();
    view.rows[0].description = "Description".into(); view.rows[0].guide_available = true;
    let mut hidden = Visibility::default();
    for item in [Item::Categories, Item::Favorites, Item::Play, Item::Favorite, Item::Description] { hidden.hide(item); }
    let actions = navigation::available(&view, Some(0), hidden);
    for id in ["nav:categories", "nav:favorites", "nav:play", "nav:favorite", "nav:description"] {
        assert!(!actions.iter().any(|a| a.id == id), "{id}");
    }
    view.rows[0].enabled = false;
    let actions = navigation::available(&view, Some(0), Visibility::default());
    for id in ["nav:play", "nav:guide", "nav:favorite", "nav:description"] {
        assert!(!actions.iter().any(|a| a.id == id), "disabled row: {id}");
    }
    let row = navigation::context_header_classic(&plx_ui::fixture::FixtureMeasure,
        1920.0, 777, &[], false, &[], plx_ui::theme::size::BODY);
    assert_eq!(row.stops.len(), 1); assert_eq!(row.stops[0].0, 777);
}

#[test]
fn classic_header_focus_restores_the_selected_media_behind_it() {
    let measure = plx_ui::fixture::FixtureMeasure;
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["one", "two"])); screen.refresh_presentation(UiMode::Classic, &measure);
    let second = screen.keys["item:two"];
    classic_deliver(&mut screen, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: second }, by: plx_ui::screen::By::Dir });
    let categories = screen.keys["nav:categories"];
    let memory = Screen::<TestHost>::memory_at(&screen, Some(FocusKey { entry: EntryId(1), elem: categories }));
    let mut fresh = OttScreen::new(EntryId(1), InstanceId(1)); fresh.refresh_presentation(UiMode::Classic, &measure);
    classic_deliver(&mut fresh, ScreenEvent::RestoreMemory(memory)); fresh.install(snapshot(2, &["one", "two"]));
    assert_eq!(fresh.media.map(|i| fresh.snapshot.rows[i].id.as_str()), Some("two"));
    assert_eq!(fresh.focus_request, Some(categories));
    assert!(fresh.header(&measure).unwrap().stops.iter().any(|(key,_)| *key == categories));
}

#[test]
fn actual_category_body_with_favorites_and_manage_uses_shared_classic_pages() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    let mut view = (*snapshot(1, &["all", "favorites", "news", "organization.open"])).clone();
    view.page_id = "categories".into();
    view.rows[0].action = OttAction::SelectCategoryAt { context: "scope".into(), category: String::new() };
    view.rows[1].action = OttAction::ShowFavorites;
    view.rows[2].action = OttAction::SelectCategoryAt { context: "scope".into(), category: "news".into() };
    view.rows[3].action = OttAction::EditSetting("organization.open".into());
    screen.install(Arc::new(view));
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    assert!(screen.form.table.is_classic_catalogue());
    assert!(screen.form.index_of(&"nav:back".into()).is_none());
    assert_eq!(screen.form.table.catalogue_next(0, -1, None), Some(3));
    let favorite = screen.keys["item:favorites"];
    let out = effects(|fx| screen.activate(favorite, false, fx));
    assert!(out.iter().any(|e| matches!(&e.fx,
        Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::ShowFavorites, .. })))));
}

#[test]
fn accepted_list_side_snapshot_reflows_preview_and_hit_frame_without_retargeting_selection() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.classic_visible = true;
    classic_deliver(&mut screen, ScreenEvent::App(AppMsg::OttSnapshot(snapshot(1, &["one", "two"]))));
    let key = screen.keys["item:two"];
    classic_deliver(&mut screen, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: key }, by: plx_ui::screen::By::Dir });
    let old_target = screen.viewport_target().unwrap();
    let old_layout = screen.classic_layout.get().unwrap();
    let mut next = (*screen.snapshot).clone(); next.revision += 1;
    next.classic_preferences = plx_appkit::classic_settings::Preferences::from_values(25, 1, 0, 15).unwrap();
    classic_deliver(&mut screen, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(next))));
    let layout = screen.classic_layout.get().unwrap();
    assert_ne!(screen.viewport_target(), Some(old_target));
    assert_eq!(layout.list.w, old_layout.list.w);
    assert!(layout.list.x + layout.list.w < layout.preview.x);
    assert_eq!(screen.view(&plx_ui::fixture::FixtureMeasure).table.frame, layout.list);
    assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:two"));
    assert_eq!(screen.keys["item:two"], key);
    assert!(screen.viewport_aperture().is_none(), "new geometry does not invent an acknowledged video lease");
    let row = screen.form.index_of_key(RowKey(key)).unwrap();
    let frame = screen.form.table.row_frame(layout.list, row as i32).unwrap();
    assert_eq!(screen.form.table.hit_row(layout.list, frame.x + frame.w * 0.5, frame.y + frame.h * 0.5), Some(row as i32));
}

#[test]
fn classic_remote_page_uses_fitted_count_and_preserves_row_actions() {
    let measure = plx_ui::fixture::FixtureMeasure;
    for wanted in [10, 25, 30] {
        let names: Vec<_> = (0..60).map(|n| format!("channel-{n}")).collect();
        let refs: Vec<_> = names.iter().map(String::as_str).collect();
        let mut view = (*snapshot(1, &refs)).clone();
        view.classic_preferences = plx_appkit::classic_settings::Preferences::from_values(wanted, 0, 0, 15).unwrap();
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        screen.install(Arc::new(view));
        screen.refresh_presentation(UiMode::Classic, &measure);
        let count = screen.form.table.classic_page_rows().unwrap();
        assert!(count <= wanted);
        let first = screen.catalog_keys[0];
        let next = screen.remote_page_key(first, 1).unwrap();
        assert_eq!(next, screen.catalog_keys[count]);
        assert_eq!(screen.remote_page_key(first, -1), Some(first));
        let out = effects(|fx| screen.activate(next, false, fx));
        assert!(out.iter().any(|e| matches!(&e.fx, Fx::App(AppFx::Ott(OttOut::Action {
            action: OttAction::Play(id), .. })) if id == &names[count])));
    }
}

#[test]
fn classic_preferences_leave_modern_frames_and_generic_settings_paging_unchanged() {
    let measure = plx_ui::fixture::FixtureMeasure;
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["one", "two"]));
    screen.refresh_presentation(UiMode::Modern, &measure);
    let frame = screen.view(&measure).table.frame;
    let mut view = (*screen.snapshot).clone(); view.revision += 1;
    view.classic_preferences = plx_appkit::classic_settings::Preferences::from_values(10, 1, 3, 120).unwrap();
    screen.install(Arc::new(view)); screen.refresh_presentation(UiMode::Modern, &measure);
    assert_eq!(screen.view(&measure).table.frame, frame);
    assert!(screen.form.table.classic_page_rows().is_none());
    let mut settings = (*screen.snapshot).clone(); settings.page_id = "settings".into();
    for row in &mut settings.rows { row.action = OttAction::EditSetting("osdOpacity".into()); }
    screen.install(Arc::new(settings.clone())); screen.refresh_presentation(UiMode::Classic, &measure);
    assert!(!screen.form.table.is_classic_catalogue());
    assert!(screen.form.table.is_classic_choices());
    assert_eq!(screen.form.table.classic_page_rows(), Some(10));
    // Only the explicit compact Settings pages opt in; generic provider forms do not.
    settings.page_id = "profile-setup:m3u".into();
    screen.install(Arc::new(settings)); screen.refresh_presentation(UiMode::Classic, &measure);
    assert!(!screen.form.table.is_classic_catalogue());
    assert!(!screen.form.table.is_classic_choices());
    assert!(screen.form.table.classic_page_rows().is_none());
}

#[test]
fn classic_programme_tone_boundary_tracks_visibility_without_changing_row_identity() {
    let mut view = (*snapshot(1, &["live"])).clone();
    let row = &mut view.rows[0];
    row.title = "1  ČT".into();
    row.content_title = Some("ČT".into());
    row.current_programme = Some(plx_data::ott::guide::rows::CurrentProgramme { title: "Zprávy".into(), start_s: 10, end_s: 20 });
    row.guide = None;
    row.guide_available = true;
    let original_id = row.id.clone();
    for (name, programme, expected) in [(1, 1, Some(5)), (0, 1, Some(0)), (1, 0, None), (0, 0, None)] {
        let prefs = view.classic_preferences.with_channel_display(1, name, programme, 1).unwrap();
        let label = crate::ott::classic::list_label(row, prefs, Some(15));
        assert_eq!(crate::ott::classic::list_secondary_start(row, prefs, &label), expected);
    }
    let label = crate::ott::classic::list_label(row, view.classic_preferences, Some(20));
    assert_eq!(label, "ČT");
    assert_eq!(crate::ott::classic::list_secondary_start(row, view.classic_preferences, &label), None);
    assert_eq!(row.id, original_id);
}


#[test]
fn classic_list_ordinals_follow_visible_order_without_changing_row_identity() {
    let _serial = plx_base::testlock::serial();
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    let mut view = (*snapshot(1, &["m3u", "plex", "vportal", "local", "torrent"])).clone();
    for (index, row) in view.rows.iter_mut().enumerate() {
        row.content_title = Some(format!("Title {index}"));
        row.title = format!("{}  Title {index}", 900 + index);
    }
    // Provider rows without a numeric title still occupy a position in the list.
    view.rows[1].title = "Title 1".into();
    view.rows[2].enabled = false;
    for indices in [vec![0, 1, 2, 3, 4], vec![4, 1, 0], vec![3]] {
        let mut next = view.clone();
        next.revision += screen.snapshot.revision;
        next.rows = indices.iter().map(|index| view.rows[*index].clone()).collect();
        screen.install(Arc::new(next));
        screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
        let frame = screen.classic_layout.get().unwrap().list;
        screen.form.table.fit_layout(&plx_ui::fixture::FixtureMeasure);
        let runs = plx_ui::draw_census::capture_runs(|| screen.form.table.draw(
            plx_ui::Painter::recording(), frame, &plx_ui::fixture::FixtureMeasure));
        let numbers: Vec<_> = runs.iter().filter(|run| run.align == 2
            && !run.text.is_empty() && run.text.bytes().all(|byte| byte.is_ascii_digit()))
            .map(|run| run.text.clone()).collect();
        assert_eq!(numbers, (1..=indices.len()).map(|index| index.to_string()).collect::<Vec<_>>());
        for index in &indices {
            let row = &view.rows[*index];
            let key = screen.keys[&format!("item:{}", row.id)];
            let output = effects(|fx| screen.activate(key, false, fx));
            if row.enabled {
                assert!(output.iter().any(|effect| matches!(&effect.fx,
                    Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::Play(id), .. })) if id == &row.id)));
            } else { assert!(output.is_empty()); }
        }
    }
}

#[test]
fn classic_list_number_preference_and_modern_keep_their_existing_identity_and_labels() {
    let _serial = plx_base::testlock::serial();
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    let mut view = (*snapshot(1, &["row"])).clone();
    view.rows[0].title = "2026  News".into();
    view.rows[0].content_title = Some("News".into());
    view.classic_preferences = view.classic_preferences.with_channel_display(0, 1, 1, 1).unwrap();
    screen.install(Arc::new(view));
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    let frame = screen.classic_layout.get().unwrap().list;
    screen.form.table.fit_layout(&plx_ui::fixture::FixtureMeasure);
    let runs = plx_ui::draw_census::capture_runs(|| screen.form.table.draw(
        plx_ui::Painter::recording(), frame, &plx_ui::fixture::FixtureMeasure));
    assert!(runs.iter().any(|run| run.text == "News"));
    assert!(!runs.iter().any(|run| run.text == "1" || run.text == "2026"));
    screen.refresh_presentation(UiMode::Modern, &plx_ui::fixture::FixtureMeasure);
    assert!(screen.form.table.sections.iter().flat_map(|section| &section.rows)
        .any(|row| row.label == "2026  News"));
    assert_eq!(screen.snapshot.rows[0].id, "row");
}


fn category_shortcut_fixture() -> OttScreen {
    let mut view = (*snapshot(1, &["all", "favorites", "third", "fourth", "fifth",
        "sixth", "seventh", "eighth", "ninth", "tenth"])).clone();
    view.page_id = "categories".into();
    for row in &mut view.rows {
        row.action = OttAction::SelectCategoryAt { context: "category-owner".into(),
            category: row.id.clone() };
    }
    view.rows[0].action = OttAction::SelectCategoryAt {
        context: "category-owner".into(), category: String::new() };
    view.rows[1].action = OttAction::ShowFavorites;
    view.rows[4].enabled = false;
    let mut manage = view.rows[0].clone();
    manage.id = "organization.open".into();
    manage.title = "Manage".into();
    manage.action = OttAction::EditSetting("organization.open".into());
    view.rows.insert(2, manage);
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(Arc::new(view));
    screen.classic_visible = true;
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    screen
}

#[test]
fn classic_category_badges_and_digits_share_first_nine_real_category_actions() {
    let _serial = plx_base::testlock::serial();
    let mut screen = category_shortcut_fixture();
    let frame = screen.classic_layout.get().unwrap().list;
    screen.form.table.fit_layout(&plx_ui::fixture::FixtureMeasure);
    let runs = plx_ui::draw_census::capture_runs(|| screen.form.table.draw(
        plx_ui::Painter::recording(), frame, &plx_ui::fixture::FixtureMeasure));
    let numbers: Vec<_> = runs.iter().filter(|run| run.align == 2 && !run.text.is_empty()
        && run.text.bytes().all(|byte| byte.is_ascii_digit())).map(|run| run.text.clone()).collect();
    assert_eq!(numbers, (1..=9).map(|number| number.to_string()).collect::<Vec<_>>());
    for (number, id) in [(1, "all"), (2, "favorites"), (3, "third"), (9, "ninth")] {
        let expected = screen.snapshot.rows.iter().find(|row| row.id == id).unwrap().action.clone();
        assert_eq!(screen.category_number_action(number), Some(expected.clone()));
        let key = screen.keys[&format!("item:{id}")];
        let out = effects(|fx| screen.activate(key, false, fx));
        assert!(out.iter().any(|effect| matches!(&effect.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action, .. })) if action == &expected)));
    }
    assert!(screen.category_number_action(5).is_none(), "disabled fifth row keeps its badge, not its action");
    assert!(screen.category_number_action(0).is_none());
    assert!(screen.category_number_action(10).is_none());
}

#[test]
fn classic_category_digits_do_not_cross_editor_reader_browse_hidden_or_modern_boundaries() {
    let _serial = plx_base::testlock::serial();
    for boundary in 0..6 {
        let mut screen = category_shortcut_fixture();
        match boundary {
            0 => { editor(&mut screen, "draft"); screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure); }
            1 => screen.reading = true,
            2 => { effects(|fx| screen.open_browse(fx)); }
            3 => screen.classic_visible = false,
            4 => screen.refresh_presentation(UiMode::Modern, &plx_ui::fixture::FixtureMeasure),
            _ => {
                let mut view = (*screen.snapshot).clone(); view.page_id = "channels".into();
                screen.install(Arc::new(view));
            }
        }
        assert!(screen.category_number_action(1).is_none(), "boundary {boundary}");
    }
}


#[test]
fn classic_caption_uses_localized_web_wording_and_opaque_projected_category() {
    for language in plx_platform::i18n::SHIPPED {
        let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
        let mut view = (*snapshot(1, &["one"])).clone();
        let category = "<News> / {category} & Music";
        view.title = "Source title".into();
        view.description = "Not a category".into();
        view.category_label = Some(category.into());
        for page in ["channels", "channels:opaque-owner", "favorites:opaque-owner"] {
            view.page_id = page.into();
            assert_eq!(navigation::classic_caption(&view), msg::ott_classic_channel_category(category));
            assert!(navigation::classic_caption(&view).contains(category));
            assert_eq!(navigation::caption(&view), format!("Source title · {category}"),
                "Modern keeps its existing caption");
            assert_eq!(view.category_label.as_deref(), Some(category));
        }
        view.title = msg::ott_favorites().into();
        view.category_label = Some(view.title.clone());
        assert_eq!(navigation::classic_caption(&view), msg::ott_classic_channel_category(&view.title));
        assert_eq!(navigation::caption(&view), view.title);
    }
}

#[test]
fn classic_caption_keeps_missing_category_and_unrelated_routes_truthful() {
    let _locale = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    let mut view = (*snapshot(1, &["one"])).clone();
    view.title = "Source title".into();
    for page in ["channels:opaque-owner", "favorites:opaque-owner"] {
        view.page_id = page.into();
        for category in [None, Some(""), Some("  ")] {
            view.category_label = category.map(str::to_owned);
            assert_eq!(navigation::classic_caption(&view), "Source title",
                "missing projection must not invent All");
        }
    }
    for page in ["categories", "categories:opaque-owner"] {
        view.page_id = page.into();
        view.category_label = Some("Not a selected category".into());
        assert_eq!(navigation::classic_caption(&view), "Category selection");
        assert_eq!(navigation::caption(&view), "Source title · Not a selected category");
    }
    for page in ["providers", "profiles:m3u", "settings", "torrent-files", "categories-other"] {
        view.page_id = page.into();
        assert_eq!(navigation::classic_caption(&view), navigation::caption(&view));
    }
}

#[test]
fn classic_draw_relabels_caption_without_changing_selection_or_editor_title() {
    let _serial = plx_base::testlock::serial();
    let mut view = (*snapshot(1, &["one", "two"])).clone();
    view.title = "Channel list".into();
    view.category_label = Some("News".into());
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(Arc::new(view));
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    let snapshot = screen.snapshot.clone();
    let selected = screen.form.selected_id().cloned();
    let keys = screen.keys.clone();
    let draw = |screen: &mut OttScreen| {
        plx_ui::draw_census::capture_runs(|| {
            let cx = Cx::<ModeHost> { views: UiMode::Classic, tick: Default::default(),
                measure: &plx_ui::fixture::FixtureMeasure, focus: Default::default(), press: Default::default(),
                owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
            let mut frame = DrawFrame::new(&cx, plx_ui::Painter::recording());
            plx_gfx::gfx::without_frame_clear(|| screen.draw(&mut frame));
        }).into_iter().filter(|run| run.ink[3] > 0.0).map(|run| run.text).collect::<Vec<_>>()
    };
    for language in plx_platform::i18n::SHIPPED {
        let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
        let expected = msg::ott_classic_channel_category("News");
        assert!(draw(&mut screen).iter().any(|text| text == &expected));
        assert!(Arc::ptr_eq(&snapshot, &screen.snapshot));
        assert_eq!(screen.form.selected_id().cloned(), selected);
        assert_eq!(screen.keys, keys);
    }
    editor(&mut screen, "draft");
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    let editor_title = screen.editor.as_ref().unwrap().title.clone();
    let painted = draw(&mut screen);
    assert!(painted.iter().any(|text| text == &editor_title));
    assert!(!painted.iter().any(|text| text == &msg::ott_classic_channel_category("News")));
    assert_eq!(screen.editor.as_ref().unwrap().draft.text(), "draft");
}
