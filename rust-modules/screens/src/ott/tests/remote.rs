use super::*;
use plx_ui::consts;

fn raw(code: u32, edge: Edge) -> ScreenEvent<TestHost> {
    ScreenEvent::Input(plx_machine::machine::InputEvent { at: Default::default(),
        source: plx_machine::machine::Source::RemoteFifo,
        kind: InputKind::Key { key: Key::Other, sym: 0, wcode: code, edge, at_edge: false } })
}
fn actions(events: &[Stamped<TestHost>]) -> Vec<OttAction> {
    events.iter().filter_map(|event| match &event.fx {
        Fx::App(AppFx::Ott(OttOut::Action { action, .. })) => Some(action.clone()), _ => None,
    }).collect()
}

#[test]
fn remote_colors_use_the_current_admitted_context_actions() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    let mut view = (*snapshot(1, &["first", "second"])).clone();
    view.rows[1].guide_available = true;
    view.order_context = Some("owned-order".into());
    screen.install(Arc::new(view));
    let selected = screen.keys["item:second"];
    deliver(&mut screen, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: selected }, by: plx_ui::screen::By::Dir });
    assert_eq!(actions(&deliver_recorded(&mut screen, raw(consts::WCODE_RED, Edge::Down))),
        vec![OttAction::OpenGuide("second".into())]);
    assert_eq!(actions(&deliver_recorded(&mut screen, raw(consts::WCODE_YELLOW, Edge::Down))),
        vec![OttAction::ShowActionsAt { context: "owned-order".into(), id: "second".into() }]);
    assert_eq!(actions(&deliver_recorded(&mut screen, raw(consts::WCODE_BLUE, Edge::Down))),
        vec![OttAction::ShowCategories]);
}

#[test]
fn remote_red_reads_movie_description_and_never_starts_playback() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["movie"]));
    assert!(actions(&deliver_recorded(&mut screen, raw(consts::WCODE_RED, Edge::Down))).is_empty());
    assert!(screen.reading);
    assert!(actions(&deliver_recorded(&mut screen, raw(consts::WCODE_BLUE, Edge::Down))).is_empty());
    assert!(screen.reading, "a reading modal retains ownership");
}

#[test]
fn remote_colors_do_not_duplicate_on_repeat_release_or_escape_editor() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["movie"]));
    for edge in [Edge::Repeat, Edge::Up] {
        assert!(actions(&deliver_recorded(&mut screen, raw(consts::WCODE_BLUE, edge))).is_empty());
    }
    assert!(actions(&deliver_recorded(&mut screen, raw(consts::WCODE_GREEN, Edge::Down))).is_empty());
    editor(&mut screen, "private draft");
    assert!(actions(&deliver_recorded(&mut screen, raw(consts::WCODE_BLUE, Edge::Down))).is_empty());
    assert!(screen.editor.is_some());
}

#[test]
fn remote_channel_paging_is_a_focus_request_not_a_play_request() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    let names: Vec<_> = (0..40).map(|i| format!("row-{i}")).collect();
    let refs: Vec<_> = names.iter().map(String::as_str).collect();
    screen.install(snapshot(1, &refs));
    let first = screen.keys["item:row-0"];
    let measure = plx_ui::fixture::FixtureMeasure;
    let mut cx = Cx { views: (), tick: Default::default(), measure: &measure,
        focus: Default::default(), press: Default::default(),
        owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
    cx.focus.current = Some(FocusKey { entry: EntryId(1), elem: first });
    let output = effects(|fx| { screen.step(&raw(consts::WCODE_CH_DOWN_KEY, Edge::Down), &cx, fx); });
    assert!(actions(&output).is_empty());
    assert!(output.iter().any(|event| matches!(&event.fx,
        Fx::Deliver(_, Delivery::Screen(ScreenEvent::Enter(plx_ui::screen::Enter::Fresh {
            focus: plx_ui::screen::FocusTarget::Elem(key) }))) if key.elem != first)));
}

#[test]
fn remote_channel_paging_skips_disabled_destinations_in_both_presentations() {
    use plx_machine::machine::UiMode;
    let measure = plx_ui::fixture::FixtureMeasure;
    for mode in [UiMode::Classic, UiMode::Modern] {
        for direction in [-1, 1] {
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            screen.refresh_presentation(mode, &measure);
            let names: Vec<_> = (0..80).map(|i| format!("row-{i}")).collect();
            let refs: Vec<_> = names.iter().map(String::as_str).collect();
            screen.install(snapshot(1, &refs));
            screen.refresh_presentation(mode, &measure);
            let current = if direction > 0 { 0 } else { 79 };
            let key = screen.keys[&format!("item:row-{current}")];
            let destination = screen.remote_page_key(key, direction).unwrap();
            let index = screen.catalog_keys.iter().position(|key| *key == destination).unwrap();
            assert_ne!(index, current);
            let mut view = (*screen.snapshot).clone();
            view.revision += 1;
            view.rows[index].enabled = false;
            screen.install(Arc::new(view));
            screen.refresh_presentation(mode, &measure);
            assert!(!screen.grid_active(), "disabled rows use the ordinary table");
            let wanted = (index as i32 + direction) as usize;
            let expected = screen.catalog_keys[wanted];
            assert_eq!(screen.remote_page_key(key, direction), Some(expected));

            let mut cx = Cx::<FavoritesModeHost> { views: mode, tick: Default::default(), measure: &measure,
                focus: Default::default(), press: Default::default(),
                owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
            cx.focus.current = Some(FocusKey { entry: EntryId(1), elem: key });
            let mut output = Vec::new();
            let mut present = plx_machine::present::Present::new();
            let code = if direction > 0 { consts::WCODE_CH_DOWN_KEY } else { consts::WCODE_CH_UP_KEY };
            screen.step(&raw_mode(code, Edge::Down), &cx,
                &mut Effects::new(&mut output, MachineId::Instance(InstanceId(1)), &mut present));
            assert!(mode_actions(&output).is_empty(), "paging does not play or select a provider");
            let requested = output.iter().find_map(|event| match &event.fx {
                Fx::Deliver(_, Delivery::Screen(ScreenEvent::Enter(plx_ui::screen::Enter::Fresh {
                    focus: plx_ui::screen::FocusTarget::Elem(target) }))) => Some(target.elem), _ => None,
            });
            assert_eq!(requested, Some(expected), "{mode:?}, direction {direction}, destination {index}");
        }
    }
}

#[test]
fn remote_channel_paging_clamps_to_available_rows_without_wrapping() {
    use plx_machine::machine::UiMode;
    for mode in [UiMode::Classic, UiMode::Modern] {
        for direction in [-1, 1] {
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            screen.refresh_presentation(mode, &plx_ui::fixture::FixtureMeasure);
            let names: Vec<_> = (0..8).map(|i| format!("row-{i}")).collect();
            let refs: Vec<_> = names.iter().map(String::as_str).collect();
            let mut view = (*snapshot(1, &refs)).clone();
            let (current, wanted, disabled) = if direction > 0 {
                (0, 5, [6, 7])
            } else { (7, 2, [0, 1]) };
            for index in disabled { view.rows[index].enabled = false; }
            screen.install(Arc::new(view));
            let current = screen.catalog_keys[current];
            let boundary = screen.catalog_keys[wanted];
            assert_eq!(screen.remote_page_key(current, direction), Some(boundary));
            assert_eq!(screen.remote_page_key(boundary, direction), Some(boundary),
                "a disabled end must not wrap to the opposite end");
            let mut view = (*screen.snapshot).clone();
            view.revision += 1;
            for row in &mut view.rows { row.enabled = false; }
            screen.install(Arc::new(view));
            assert_eq!(screen.remote_page_key(current, direction), None);
        }
    }
}

#[test]
fn classic_paging_uses_content_bindings_with_interleaved_controls() {
    use plx_machine::machine::UiMode;
    let measure = plx_ui::fixture::FixtureMeasure;
    for direction in [-1, 1] {
        for destination_enabled in [false, true] {
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            screen.refresh_presentation(UiMode::Classic, &measure);
            let mut view = (*snapshot(1, &["first", "search", "second"])).clone();
            view.page_id = "torrent-results".into();
            let current = if direction > 0 { 0 } else { 2 };
            let destination = 2 - current;
            for index in [0, 2] {
                view.rows[index].id = format!("torznab:1:source:{index}");
                view.rows[index].action = OttAction::Play(format!("torrent:{}", view.rows[index].id));
            }
            view.rows[1].id = "torrent.query:1:edit".into();
            view.rows[1].action = OttAction::EditSetting(view.rows[1].id.clone());
            view.rows[1].enabled = !destination_enabled;
            view.rows[destination].enabled = destination_enabled;
            // A leading control also prevents the first content item from
            // accidentally retaining its snapshot index when paging left.
            let control = view.rows[1].clone();
            let mut leading = control;
            leading.id = "torrent.retry".into();
            leading.action = OttAction::EditSetting(leading.id.clone());
            view.rows.insert(0, leading);
            screen.install(Arc::new(view));
            screen.refresh_presentation(UiMode::Classic, &measure);
            screen.classic_visible = true;
            assert!(screen.form.table.is_classic_catalogue());
            assert_eq!(screen.catalog_keys.len(), 2, "controls are excluded from the catalogue");
            let key = screen.keys[&format!("item:torznab:1:source:{current}")];
            let expected = if destination_enabled {
                screen.keys[&format!("item:torznab:1:source:{destination}")]
            } else { key };
            assert_eq!(screen.remote_page_key(key, direction), Some(expected));

            let mut cx = Cx::<FavoritesModeHost> { views: UiMode::Classic, tick: Default::default(), measure: &measure,
                focus: Default::default(), press: Default::default(),
                owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
            cx.focus.current = Some(FocusKey { entry: EntryId(1), elem: key });
            let input = ScreenEvent::Input(plx_machine::machine::InputEvent { at: Default::default(),
                source: plx_machine::machine::Source::RemoteFifo,
                kind: InputKind::Key { key: if direction > 0 { Key::Right } else { Key::Left },
                    sym: 0, wcode: 0, edge: Edge::Down, at_edge: false } });
            let mut output = Vec::new();
            let mut present = plx_machine::present::Present::new();
            screen.step(&input, &cx,
                &mut Effects::new(&mut output, MachineId::Instance(InstanceId(1)), &mut present));
            assert!(mode_actions(&output).is_empty(), "paging never activates content or controls");
            let requested = output.iter().find_map(|event| match &event.fx {
                Fx::Deliver(_, Delivery::Screen(ScreenEvent::Enter(plx_ui::screen::Enter::Fresh {
                    focus: plx_ui::screen::FocusTarget::Elem(target) }))) => Some(target.elem), _ => None,
            });
            assert_eq!(requested, Some(expected), "direction {direction}, enabled {destination_enabled}");
        }
    }
}

#[test]
fn classic_categories_use_existing_wrapping_rows_and_clamped_pages() {
    let measure = plx_ui::fixture::FixtureMeasure;
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.refresh_presentation(plx_machine::machine::UiMode::Classic, &measure);
    let mut view = (*snapshot(1, &["one", "two"])).clone();
    view.page_id = "categories".into();
    for row in &mut view.rows { row.action = OttAction::SelectCategory(row.id.clone()); }
    screen.install(Arc::new(view));
    assert!(screen.form.table.is_classic_catalogue());
    let first = screen.form.index_of_key(RowKey(screen.keys["item:one"])).unwrap() as i32;
    let last = screen.form.index_of_key(RowKey(screen.keys["item:two"])).unwrap() as i32;
    assert_eq!(screen.form.table.catalogue_next(first, -1, None), Some(last));
    assert_eq!(screen.form.table.catalogue_next(last, 1, None), Some(first));
    assert_eq!(screen.form.table.catalogue_next(first, -1, Some(700.0)), Some(first));
    assert_eq!(screen.form.table.catalogue_next(last, 1, Some(700.0)), Some(last));
}

struct FavoritesModeHost;
impl Host for FavoritesModeHost {
    type Arg = crate::registry::AppArg;
    type Fx = AppFx;
    type Msg = AppMsg;
    type Elem = u32;
    type Views<'a> = plx_machine::machine::UiMode;
    type Init = crate::registry::AppArg;
    type Memory = PageMemory;
    fn ui_mode(mode: &plx_machine::machine::UiMode) -> plx_machine::machine::UiMode { *mode }
}
fn raw_mode(code: u32, edge: Edge) -> ScreenEvent<FavoritesModeHost> {
    ScreenEvent::Input(plx_machine::machine::InputEvent { at: Default::default(),
        source: plx_machine::machine::Source::RemoteFifo,
        kind: InputKind::Key { key: Key::Other, sym: 0, wcode: code, edge, at_edge: false } })
}
fn mode_actions(events: &[Stamped<FavoritesModeHost>]) -> Vec<OttAction> {
    events.iter().filter_map(|event| match &event.fx {
        Fx::App(AppFx::Ott(OttOut::Action { action, .. })) => Some(action.clone()), _ => None,
    }).collect()
}
fn step_mode(screen: &mut OttScreen, mode: plx_machine::machine::UiMode,
    event: ScreenEvent<FavoritesModeHost>) -> Vec<Stamped<FavoritesModeHost>> {
    let cx = Cx::<FavoritesModeHost> { views: mode, tick: Default::default(),
        measure: &plx_ui::fixture::FixtureMeasure, focus: Default::default(), press: Default::default(),
        owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
    let mut output = Vec::new();
    let mut present = plx_machine::present::Present::new();
    screen.step(&event, &cx, &mut Effects::new(&mut output, MachineId::Instance(InstanceId(1)), &mut present));
    assert_eq!(screen.classic_mode, mode == plx_machine::machine::UiMode::Classic);
    output
}
fn editor_mode(screen: &mut OttScreen, mode: plx_machine::machine::UiMode) {
    let spec = Arc::new(OttEditor { session: 7, kind: OttEditorKind::Setting,
        text_policy: OttTextPolicy::Literal, key: "profile.source".into(), title: "Source".into(),
        value: "private draft".into(), secret: true, limit: 64 });
    step_mode(screen, mode, ScreenEvent::App(AppMsg::OttEditor(spec)));
}

fn favorites_screen(mode: plx_machine::machine::UiMode) -> OttScreen {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.refresh_presentation(mode, &plx_ui::fixture::FixtureMeasure);
    let mut view = (*snapshot(1, &["favorite:first", "favorite:second"])).clone();
    // Runtime::search_page_id scopes even an unfiltered Favorites list.
    view.page_id = "favorites:current-source-context".into();
    for row in &mut view.rows { row.favorite = true; }
    screen.install(Arc::new(view));
    let selected = screen.keys["item:favorite:second"];
    step_mode(&mut screen, mode, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: selected }, by: plx_ui::screen::By::Dir });
    screen
}

#[test]
fn classic_favorites_yellow_uses_the_selected_rows_existing_action() {
    let mode = plx_machine::machine::UiMode::Classic;
    let mut screen = favorites_screen(plx_machine::machine::UiMode::Classic);
    assert!(screen.snapshot.order_context.is_none(), "Favorites has no Local order menu");
    let before = screen.snapshot.clone();
    let selected = screen.form.selected_key();
    let output = step_mode(&mut screen, mode, raw_mode(consts::WCODE_YELLOW, Edge::Down));
    assert_eq!(mode_actions(&output), vec![OttAction::ToggleFavorite("favorite:second".into())]);
    assert!(output.iter().any(|event| matches!(&event.fx,
        Fx::App(AppFx::Ott(OttOut::Action { reply: InstanceId(1), action: OttAction::ToggleFavorite(id) }))
            if id == "favorite:second")));
    assert!(Arc::ptr_eq(&before, &screen.snapshot), "only the runtime may publish removal");
    assert_eq!(screen.form.selected_key(), selected);
    assert_eq!(mode_actions(&step_mode(&mut screen, mode, raw_mode(consts::WCODE_BLUE, Edge::Down))),
        vec![OttAction::ShowCategories]);
}

#[test]
fn classic_favorites_yellow_does_not_repeat_or_act_at_an_edge() {
    let mode = plx_machine::machine::UiMode::Classic;
    let mut screen = favorites_screen(plx_machine::machine::UiMode::Classic);
    assert_eq!(mode_actions(&step_mode(&mut screen, mode, raw_mode(consts::WCODE_YELLOW, Edge::Down))),
        vec![OttAction::ToggleFavorite("favorite:second".into())]);
    for edge in [Edge::Repeat, Edge::Up] {
        assert!(mode_actions(&step_mode(&mut screen, mode, raw_mode(consts::WCODE_YELLOW, edge))).is_empty());
    }
    let mut event = raw_mode(consts::WCODE_YELLOW, Edge::Down);
    if let ScreenEvent::Input(input) = &mut event {
        if let InputKind::Key { at_edge, .. } = &mut input.kind { *at_edge = true; }
    }
    assert!(mode_actions(&step_mode(&mut screen, mode, event)).is_empty());
}

#[test]
fn classic_favorites_yellow_preserves_other_modes_and_exact_page_boundaries() {
    use plx_machine::machine::UiMode;
    for (mode, page, order) in [
        (UiMode::Modern, "favorites", false),
        (UiMode::Classic, "favorites-other", false),
        (UiMode::Classic, "channels", true),
        (UiMode::Modern, "channels", true),
    ] {
        let mut screen = favorites_screen(mode);
        let mut view = (*screen.snapshot).clone();
        view.page_id = page.into();
        view.order_context = order.then(|| "owned-order".into());
        screen.install(Arc::new(view));
        let selected = screen.keys["item:favorite:second"];
        step_mode(&mut screen, mode, ScreenEvent::FocusMoved { from: None,
            to: FocusKey { entry: EntryId(1), elem: selected }, by: plx_ui::screen::By::Dir });
        let expected = if order {
            vec![OttAction::ShowActionsAt { context: "owned-order".into(), id: "favorite:second".into() }]
        } else { vec![] };
        assert_eq!(mode_actions(&step_mode(&mut screen, mode, raw_mode(consts::WCODE_YELLOW, Edge::Down))),
            expected, "{page}");
    }
}

#[test]
fn classic_favorites_yellow_respects_editor_reader_and_browse_owners() {
    let mode = plx_machine::machine::UiMode::Classic;
    for owner in ["editor", "reader", "browse"] {
        let mut screen = favorites_screen(plx_machine::machine::UiMode::Classic);
        match owner {
            "editor" => editor_mode(&mut screen, mode),
            "reader" => {
                step_mode(&mut screen, mode, raw_mode(consts::WCODE_RED, Edge::Down));
                assert!(screen.reading);
            },
            "browse" => {
                let key = screen.keys["nav:browse"];
                step_mode(&mut screen, mode, ScreenEvent::Activate(key));
                assert!(screen.browse);
            },
            _ => unreachable!(),
        }
        assert!(mode_actions(&step_mode(&mut screen, mode, raw_mode(consts::WCODE_YELLOW, Edge::Down))).is_empty(), "{owner}");
        match owner {
            "editor" => assert!(screen.editor.is_some()),
            "reader" => assert!(screen.reading),
            "browse" => assert!(screen.browse),
            _ => unreachable!(),
        }
    }
}

#[test]
fn classic_favorites_yellow_does_nothing_without_an_admitted_playable_row() {
    let mode = plx_machine::machine::UiMode::Classic;
    for case in ["empty", "disabled", "non-playable"] {
        let mut screen = favorites_screen(plx_machine::machine::UiMode::Classic);
        let mut view = (*snapshot(2, &["unavailable"])).clone();
        view.page_id = "favorites".into();
        match case {
            "empty" => view.rows.clear(),
            "disabled" => view.rows[0].enabled = false,
            "non-playable" => view.rows[0].action = OttAction::ShowCategories,
            _ => unreachable!(),
        }
        screen.install(Arc::new(view));
        assert!(mode_actions(&step_mode(&mut screen, mode, raw_mode(consts::WCODE_YELLOW, Edge::Down))).is_empty(), "{case}");
    }
}

#[test]
fn scoped_favorites_footer_and_yellow_target_the_same_selected_favorite() {
    use plx_machine::machine::UiMode;
    use plx_ui::page_header::FooterTone;
    for page in ["favorites", "favorites:current-source-context"] {
        let mut screen = favorites_screen(UiMode::Classic);
        let mut view = (*screen.snapshot).clone();
        view.page_id = page.into();
        // Even a stray order offer must not turn Favorites removal into ordering.
        view.order_context = Some("unrelated-order".into());
        screen.install(Arc::new(view));
        let selected = screen.keys["item:favorite:second"];
        step_mode(&mut screen, UiMode::Classic, ScreenEvent::FocusMoved { from: None,
            to: FocusKey { entry: EntryId(1), elem: selected }, by: plx_ui::screen::By::Dir });
        let actions = crate::ott::navigation::available(&screen.snapshot, screen.media,
            plx_appkit::menu_visibility::Visibility::default());
        let controls = crate::ott::navigation::classic_controls(&screen.snapshot, &actions, &screen.keys);
        let hints = crate::ott::navigation::classic_footer_keys(&screen.snapshot, &controls, &screen.keys);
        let yellow: Vec<_> = hints.iter().filter(|(_, keys)| keys.iter()
            .any(|key| matches!(key.tone, FooterTone::Yellow))).map(|(key, _)| *key).collect();
        assert_eq!(yellow, vec![screen.keys["nav:favorite"]], "{page}");
        assert_eq!(mode_actions(&step_mode(&mut screen, UiMode::Classic,
            raw_mode(consts::WCODE_YELLOW, Edge::Down))),
            vec![OttAction::ToggleFavorite("favorite:second".into())], "{page}");
    }
}

#[test]
fn classic_footer_badges_match_the_pages_actual_remote_handlers() {
    use plx_machine::machine::UiMode;
    use plx_ui::page_header::FooterTone;
    let _serial = plx_base::testlock::serial();
    for page in ["channels:filtered-owner", "favorites:filtered-owner", "torrent-files"] {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        let mut view = (*snapshot(1, &["torrent.file:4:session:19"])).clone();
        view.page_id = page.into();
        view.order_context = Some("owned-order".into());
        view.rows[0].guide_available = true;
        screen.install(Arc::new(view));
        screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
        let actions = crate::ott::navigation::available(&screen.snapshot, screen.media, Default::default());
        let controls = crate::ott::navigation::classic_controls(&screen.snapshot, &actions, &screen.keys);
        let hints = crate::ott::navigation::classic_footer_keys(&screen.snapshot, &controls, &screen.keys);
        let color = |tone| hints.iter().find(|(_, keys)| keys.iter().any(|key| key.tone == tone)).map(|(id, _)| *id);
        assert_eq!(color(FooterTone::Blue), Some(screen.keys["nav:categories"]));
        assert!(hints.iter().all(|(id, keys)| keys.iter().all(|key| key.text.is_empty()
            || (*id == screen.keys["nav:description"] && key.text == "OK")
            || (*id == screen.keys["nav:categories"] && key.text == "▶Ⅱ"
                && matches!(page.split(':').next(), Some("channels" | "favorites"))))),
            "only implemented Description OK and channel-list Category Play/Pause shortcuts are advertised");
        if page != "torrent-files" {
            assert_eq!(mode_actions(&step_mode(&mut screen, UiMode::Classic,
                raw_mode(consts::WCODE_PLAYPAUSE, Edge::Down))), vec![OttAction::ShowCategories],
                "the advertised Category transport badge must reach the real handler");
        }
        let red = mode_actions(&step_mode(&mut screen, UiMode::Classic, raw_mode(consts::WCODE_RED, Edge::Down)));
        let yellow = mode_actions(&step_mode(&mut screen, UiMode::Classic, raw_mode(consts::WCODE_YELLOW, Edge::Down)));
        if page == "torrent-files" {
            assert_eq!(color(FooterTone::Red), None);
            assert_eq!(color(FooterTone::Yellow), None);
            assert!(red.is_empty() && yellow.is_empty(), "the existing Torrent page has no Red/Yellow route");
        } else {
            assert_eq!(color(FooterTone::Red), Some(screen.keys["nav:guide"]));
            assert_eq!(red, vec![OttAction::OpenGuide("torrent.file:4:session:19".into())]);
            if page.starts_with("favorites:") {
                assert_eq!(color(FooterTone::Yellow), Some(screen.keys["nav:favorite"]));
                assert_eq!(yellow, vec![OttAction::ToggleFavorite("torrent.file:4:session:19".into())]);
                assert!(controls.iter().any(|(id, label)| *id == screen.keys["nav:favorite"] && label == plx_platform::i18n::msg::ott_favorite_add()),
                    "a non-favorite row must not be labeled Remove");
            } else {
                assert_eq!(color(FooterTone::Yellow), Some(screen.keys["nav:actions"]));
                assert_eq!(yellow, vec![OttAction::ShowActionsAt { context: "owned-order".into(), id: "torrent.file:4:session:19".into() }]);
            }
        }
        assert_eq!(mode_actions(&step_mode(&mut screen, UiMode::Classic, raw_mode(consts::WCODE_BLUE, Edge::Down))),
            vec![OttAction::ShowCategories]);
        if hints.iter().any(|(id, keys)| *id == screen.keys["nav:description"] && keys.iter().any(|key| key.text == "OK")) {
            let description = screen.keys["nav:description"];
            step_mode(&mut screen, UiMode::Classic, ScreenEvent::Activate(description));
            assert!(screen.reading, "the advertised Description button opens its actual reader");
        }
    }
}

#[test]
fn empty_favorites_focus_existing_categories_without_a_synthetic_media_row() {
    use plx_machine::machine::UiMode;
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        screen.refresh_presentation(mode, &plx_ui::fixture::FixtureMeasure);
        let mut view = (*snapshot(1, &[])).clone();
        view.page_id = "favorites:current-source-context".into();
        view.search = Some(OttSearch { query: String::new(), scope: "Favorites".into() });
        screen.install(Arc::new(view));
        assert_eq!(screen.form.selected_id().map(String::as_str), Some("nav:categories"));
        assert!(screen.snapshot.rows.is_empty());
        assert!(screen.catalog_keys.is_empty());
        assert!(screen.media.is_none());
        let before = screen.snapshot.clone();
        let key = screen.form.selected_key().unwrap().0;
        assert_eq!(mode_actions(&step_mode(&mut screen, mode, ScreenEvent::Activate(key))),
            vec![OttAction::ShowCategories]);
        assert!(Arc::ptr_eq(&before, &screen.snapshot));
        assert!(mode_actions(&step_mode(&mut screen, mode,
            raw_mode(consts::WCODE_YELLOW, Edge::Down))).is_empty());
    }
}

#[test]
fn filtered_empty_favorites_keep_search_recovery_before_category_navigation() {
    use plx_machine::machine::UiMode;
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        screen.refresh_presentation(mode, &plx_ui::fixture::FixtureMeasure);
        let mut view = (*snapshot(1, &[])).clone();
        view.page_id = "favorites:current-source-context".into();
        view.search = Some(OttSearch { query: "no match".into(), scope: "Favorites".into() });
        screen.install(Arc::new(view));
        assert_eq!(screen.form.selected_id().map(String::as_str), Some("nav:search"));
        let key = screen.form.selected_key().unwrap().0;
        assert_eq!(mode_actions(&step_mode(&mut screen, mode, ScreenEvent::Activate(key))),
            vec![OttAction::BeginSearch]);
        assert!(screen.form.index_of(&"nav:categories".into()).is_some());
        assert!(screen.snapshot.rows.is_empty());
    }
}

fn settings_group_screen(mode: plx_machine::machine::UiMode) -> OttScreen {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.refresh_presentation(mode, &plx_ui::fixture::FixtureMeasure);
    let mut view = (*snapshot(1, &["field", "settings.draft:42:save", "cancel"])).clone();
    view.page_id = "settings".into();
    view.selected_row = Some("field".into());
    for row in &mut view.rows {
        row.action = OttAction::EditSetting(row.id.clone());
        row.media = None;
    }
    view.rows[1].title = "Localized save label".into();
    screen.install(Arc::new(view));
    screen
}

fn settings_key(sym: u32, wcode: u32, edge: Edge) -> ScreenEvent<FavoritesModeHost> {
    let mut event = raw_mode(wcode, edge);
    if let ScreenEvent::Input(input) = &mut event {
        if let InputKind::Key { sym: value, .. } = &mut input.kind { *value = sym; }
    }
    event
}

#[test]
fn classic_settings_footer_and_zero_dispatch_the_same_published_save() {
    use plx_machine::machine::UiMode;
    let mut screen = settings_group_screen(UiMode::Classic);
    let mut view = (*screen.snapshot).clone();
    view.revision += 1;
    view.rows[2].id = "settings.draft:42:back".into();
    view.rows[2].action = OttAction::EditSetting(view.rows[2].id.clone());
    screen.install(Arc::new(view));
    let save = screen.keys["settings-footer:settings.draft:42:save"];
    let pointer = mode_actions(&step_mode(&mut screen, UiMode::Classic, ScreenEvent::Activate(save)));
    let zero = mode_actions(&step_mode(&mut screen, UiMode::Classic, settings_key(b'0' as u32, 39, Edge::Down)));
    assert_eq!(pointer, vec![OttAction::EditSetting("settings.draft:42:save".into())]);
    assert_eq!(zero, pointer);
    let mut next = (*screen.snapshot).clone();
    next.revision += 1;
    for row in &mut next.rows[1..] {
        row.id = row.id.replace(":42:", ":43:");
        row.action = OttAction::EditSetting(row.id.clone());
    }
    screen.install(Arc::new(next));
    assert!(mode_actions(&step_mode(&mut screen, UiMode::Classic, ScreenEvent::Activate(save))).is_empty());
    assert_eq!(mode_actions(&step_mode(&mut screen, UiMode::Classic, settings_key(b'0' as u32, 39, Edge::Down))),
        vec![OttAction::EditSetting("settings.draft:43:save".into())]);
}

#[test]
fn classic_settings_save_keys_dispatch_only_the_current_rows_exact_action() {
    use plx_machine::machine::UiMode;
    for (sym, wcode) in [(b'0' as u32, 39), (0, consts::WCODE_PLAY),
        (0, consts::WCODE_PAUSE), (0, consts::WCODE_GREEN)] {
        let mut screen = settings_group_screen(UiMode::Classic);
        let before = screen.snapshot.clone();
        let selection = screen.form.selected_key();
        let output = step_mode(&mut screen, UiMode::Classic, settings_key(sym, wcode, Edge::Down));
        assert_eq!(mode_actions(&output), vec![OttAction::EditSetting("settings.draft:42:save".into())]);
        assert!(output.iter().any(|event| matches!(&event.fx,
            Fx::App(AppFx::Ott(OttOut::Action { reply: InstanceId(1),
                action: OttAction::EditSetting(key) })) if key == "settings.draft:42:save")));
        assert!(Arc::ptr_eq(&before, &screen.snapshot));
        assert_eq!(screen.form.selected_key(), selection);
    }
}

#[test]
fn classic_settings_save_keys_reject_repeat_release_and_edge_redelivery() {
    use plx_machine::machine::UiMode;
    for (sym, wcode) in [(b'0' as u32, 39), (0, consts::WCODE_PLAY),
        (0, consts::WCODE_PAUSE), (0, consts::WCODE_GREEN)] {
        let mut screen = settings_group_screen(UiMode::Classic);
        for edge in [Edge::Repeat, Edge::Up] {
            assert!(mode_actions(&step_mode(&mut screen, UiMode::Classic,
                settings_key(sym, wcode, edge))).is_empty());
        }
        let mut event = settings_key(sym, wcode, Edge::Down);
        if let ScreenEvent::Input(input) = &mut event {
            if let InputKind::Key { at_edge, .. } = &mut input.kind { *at_edge = true; }
        }
        assert!(mode_actions(&step_mode(&mut screen, UiMode::Classic, event)).is_empty());
    }
}

#[test]
fn classic_settings_save_keys_require_unique_enabled_canonical_address() {
    use plx_machine::machine::UiMode;
    for case in ["absent", "disabled", "duplicate", "other-nonce", "disabled-duplicate",
        "mismatch", "wrong-action", "zero", "leading-zero", "plus", "overflow", "suffix"] {
        let mut screen = settings_group_screen(UiMode::Classic);
        let mut view = (*screen.snapshot).clone();
        match case {
            "absent" => { view.rows.remove(1); }
            "disabled" => view.rows[1].enabled = false,
            "duplicate" | "other-nonce" | "disabled-duplicate" => {
                let mut extra = view.rows[1].clone();
                // Form IDs remain unique; two rows still claim the same Save action.
                extra.id = "duplicate-save-row".into();
                if case == "other-nonce" {
                    extra.id = "settings.draft:43:save".into();
                    extra.action = OttAction::EditSetting(extra.id.clone());
                }
                if case == "disabled-duplicate" { extra.enabled = false; }
                view.rows.push(extra);
            }
            "mismatch" => view.rows[1].action = OttAction::EditSetting("settings.draft:43:save".into()),
            "wrong-action" => view.rows[1].action = OttAction::ShowSettings,
            _ => {
                view.rows[1].id = match case {
                    "zero" => "settings.draft:0:save",
                    "leading-zero" => "settings.draft:042:save",
                    "plus" => "settings.draft:+42:save",
                    "overflow" => "settings.draft:18446744073709551616:save",
                    _ => "settings.draft:42:save:extra",
                }.into();
                view.rows[1].action = OttAction::EditSetting(view.rows[1].id.clone());
            }
        }
        view.revision += 1;
        screen.install(Arc::new(view));
        assert!(mode_actions(&step_mode(&mut screen, UiMode::Classic,
            raw_mode(consts::WCODE_GREEN, Edge::Down))).is_empty(), "{case}");
    }
}

#[test]
fn classic_settings_save_keys_do_not_escape_mode_page_or_modal_owner() {
    use plx_machine::machine::UiMode;
    for (mode, page) in [(UiMode::Modern, "settings"), (UiMode::Classic, "settings:child"),
        (UiMode::Classic, "interface-mode"), (UiMode::Classic, "channels")] {
        let mut screen = settings_group_screen(mode);
        let mut view = (*screen.snapshot).clone();
        view.page_id = page.into();
        view.revision += 1;
        screen.install(Arc::new(view));
        assert!(mode_actions(&step_mode(&mut screen, mode,
            raw_mode(consts::WCODE_GREEN, Edge::Down))).is_empty(), "{page}");
    }
    for owner in ["editor", "reader", "browse"] {
        let mut screen = settings_group_screen(UiMode::Classic);
        match owner {
            "editor" => editor_mode(&mut screen, UiMode::Classic),
            "reader" => { screen.reading = true; }
            "browse" => {
                let key = screen.keys["nav:browse"];
                step_mode(&mut screen, UiMode::Classic, ScreenEvent::Activate(key));
                assert!(screen.browse);
            }
            _ => unreachable!(),
        }
        assert!(mode_actions(&step_mode(&mut screen, UiMode::Classic,
            raw_mode(consts::WCODE_GREEN, Edge::Down))).is_empty(), "{owner}");
        match owner {
            "editor" => assert!(screen.editor.is_some()),
            "reader" => assert!(screen.reading),
            "browse" => assert!(screen.browse),
            _ => unreachable!(),
        }
    }
}

#[test]
fn classic_settings_save_keys_preserve_canonical_navigation_and_digit_namespace() {
    use plx_machine::machine::UiMode;
    for (sym, wcode) in [(0, 48), (0, 39), (b'1' as u32, 30),
        (b'p' as u32, 0), (b' ' as u32, 0), (0, consts::WCODE_PLAYPAUSE)] {
        let mut screen = settings_group_screen(UiMode::Classic);
        assert!(mode_actions(&step_mode(&mut screen, UiMode::Classic,
            settings_key(sym, wcode, Edge::Down))).is_empty());
    }
    for key in [Key::Up, Key::Down, Key::Left, Key::Right] {
        let mut screen = settings_group_screen(UiMode::Classic);
        let mut event = raw_mode(consts::WCODE_PLAY, Edge::Down);
        if let ScreenEvent::Input(input) = &mut event {
            if let InputKind::Key { key: canonical, .. } = &mut input.kind { *canonical = key; }
        }
        assert!(mode_actions(&step_mode(&mut screen, UiMode::Classic, event)).is_empty());
    }
}

#[test]
fn classic_settings_save_keys_follow_replaced_nonce_and_do_not_save_a_child_picker() {
    use plx_machine::machine::UiMode;
    let mut screen = settings_group_screen(UiMode::Classic);
    let mut view = (*screen.snapshot).clone();
    view.revision += 1;
    view.rows[1].id = "settings.draft:18446744073709551615:save".into();
    view.rows[1].action = OttAction::EditSetting(view.rows[1].id.clone());
    screen.install(Arc::new(view));
    assert_eq!(mode_actions(&step_mode(&mut screen, UiMode::Classic,
        raw_mode(consts::WCODE_GREEN, Edge::Down))),
        vec![OttAction::EditSetting("settings.draft:18446744073709551615:save".into())]);
    let mut child = (*screen.snapshot).clone();
    child.revision += 1;
    child.rows[1].id = "settings.draft:18446744073709551615:choose:uiFontScale:100".into();
    child.rows[1].action = OttAction::EditSetting(child.rows[1].id.clone());
    screen.install(Arc::new(child));
    assert!(mode_actions(&step_mode(&mut screen, UiMode::Classic,
        raw_mode(consts::WCODE_GREEN, Edge::Down))).is_empty());
}
