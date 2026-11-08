use super::*;

fn searchable(revision: u64, query: &str) -> Arc<OttView> {
    let mut view = (*snapshot(revision, &["first", "second"])).clone();
    view.search = Some(OttSearch { query: query.into(), scope: "Home · Drama".into() });
    Arc::new(view)
}

fn search_editor(screen: &mut OttScreen, session: u64, value: &str) {
    deliver(screen, ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor { text_policy: OttTextPolicy::Predictive,
        session, kind: OttEditorKind::Search, key: String::new(), title: String::new(),
        value: value.into(), secret: false, limit: 256,
    }))));
}

#[test]
fn search_is_advertised_only_by_the_projected_runtime_capability() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    // Stable interned keys reserve identities; only the current capability advertises actions.
    let search = screen.keys["nav:search"];
    assert!(screen.form.index_of_key(RowKey(search)).is_none());
    assert!(!screen.header(&plx_ui::fixture::FixtureMeasure).unwrap().stops.iter().any(|(key, _)| *key == search));
    assert!(effects(|fx| screen.activate(search, false, fx)).is_empty());
    screen.install(searchable(1, ""));
    assert_eq!(screen.keys["nav:search"], search);
    assert!(screen.header(&plx_ui::fixture::FixtureMeasure).unwrap().stops.iter().any(|(key, _)| *key == search));
    let changes = effects(|fx| screen.activate(search, false, fx));
    assert!(changes.iter().any(|x| matches!(&x.fx,
        Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::BeginSearch, .. })))));
    screen.install(snapshot(2, &["first"]));
    assert!(screen.form.index_of_key(RowKey(search)).is_none());
    assert!(!screen.header(&plx_ui::fixture::FixtureMeasure).unwrap().stops.iter().any(|(key, _)| *key == search));
    assert!(effects(|fx| screen.activate(search, false, fx)).is_empty());
}

#[test]
fn direct_catalogue_navigation_emits_no_playback_or_provider_change() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(searchable(1, "Drama"));
    effects(|fx| screen.open_browse(fx));
    let catalog = screen.keys["nav:catalog"];
    let changes = effects(|fx| screen.activate(catalog, false, fx));
    let actions: Vec<_> = changes.iter().filter_map(|x| match &x.fx {
        Fx::App(AppFx::Ott(OttOut::Action { action, .. })) => Some(action), _ => None,
    }).collect();
    assert_eq!(actions, vec![&OttAction::ShowCatalog]);
}

#[test]
fn search_drafts_are_private_and_submit_only_the_typed_search_action() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(searchable(1, "previous"));
    search_editor(&mut screen, 71, "previous");
    let mut before = Canon::new(); screen.write(&mut before);
    let changes = effects(|fx| {
        screen.edit(&TextEdit::Clear, fx);
        screen.edit(&TextEdit::Commit("new private query".into()), fx);
    });
    assert!(!changes.iter().any(|x| matches!(x.fx, Fx::App(_))));
    let mut after = Canon::new(); screen.write(&mut after);
    assert_eq!(before.finish(), after.finish());
    let save = screen.keys["edit:save"];
    let changes = effects(|fx| screen.activate(save, false, fx));
    assert_eq!(changes.iter().filter(|x| matches!(&x.fx,
        Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::SubmitSearch { session: 71, value }, .. }))
        if value == "new private query")).count(), 1);
    assert!(!changes.iter().any(|x| matches!(&x.fx,
        Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::SubmitSetting { .. }, .. })))));
    assert!(screen.editor.as_ref().unwrap().pending);
    assert!(effects(|fx| screen.activate(save, false, fx)).iter().all(|x| !matches!(x.fx, Fx::App(_))));
    assert!(!format!("{:?}", OttAction::SubmitSearch { session: 71, value: "private-token".into() }).contains("private-token"));
    assert!(!format!("{:?}", searchable(1, "private-token")).contains("private-token"));
}

#[test]
fn cancelling_search_retains_the_committed_query_and_return_focus() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(searchable(1, "previous"));
    let focus = screen.form.selected_id().cloned();
    search_editor(&mut screen, 71, "draft");
    let changes = deliver_recorded(&mut screen, key_event(Key::Back));
    assert_eq!(changes.iter().filter(|x| matches!(&x.fx,
        Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::CancelSearch { session: 71 }, .. })))).count(), 1);
    assert!(screen.editor.is_none());
    assert_eq!(screen.snapshot.search.as_ref().unwrap().query, "previous");
    assert_eq!(screen.form.selected_id(), focus.as_ref());
}

#[test]
fn empty_search_is_a_real_submission_and_stale_ack_cannot_close_a_new_editor() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(searchable(1, "previous"));
    search_editor(&mut screen, 71, "");
    let save = screen.keys["edit:save"];
    let changes = effects(|fx| screen.activate(save, false, fx));
    assert!(changes.iter().any(|x| matches!(&x.fx,
        Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::SubmitSearch { session: 71, value }, .. })) if value.is_empty())));
    deliver(&mut screen, ScreenEvent::App(AppMsg::OttSnapshot(searchable(2, ""))));
    deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditorResult { session: 71, error: None }));
    assert!(screen.editor.is_none());
    assert!(screen.snapshot.search.as_ref().unwrap().query.is_empty());
    search_editor(&mut screen, 72, "new draft");
    effects(|fx| screen.activate(save, false, fx));
    deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditorResult { session: 71, error: None }));
    assert_eq!(screen.editor.as_ref().unwrap().spec.session, 72);
    assert!(screen.editor.as_ref().unwrap().pending);
}

#[test]
fn replacing_search_with_setting_cancels_the_search_protocol_only() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    search_editor(&mut screen, 71, "draft");
    let changes = deliver_recorded(&mut screen, ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor { text_policy: OttTextPolicy::Predictive,
        session: 72, kind: OttEditorKind::Setting, key: "profile.name".into(), title: "Name".into(),
        value: "Home".into(), secret: false, limit: 128,
    }))));
    assert_eq!(changes.iter().filter(|x| matches!(&x.fx,
        Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::CancelSearch { session: 71 }, .. })))).count(), 1);
    assert!(!changes.iter().any(|x| matches!(&x.fx,
        Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::CancelSetting { .. }, .. })))));
    assert_eq!(screen.editor.as_ref().unwrap().spec.kind, OttEditorKind::Setting);
}

#[test]
fn search_labels_fit_every_shipped_language() {
    let mut failures = Vec::new();
    let width = RouteLayout::screen().sectioned_table().w;
    for language in plx_platform::i18n::SHIPPED {
        let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
        for favorites in [false, true] {
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            let mut view = (*searchable(1, "")).clone();
            if favorites {
                view.page_id = "favorites:scope".into();
                view.search.as_mut().unwrap().scope = msg::ott_favorites().into();
            }
            screen.install(Arc::new(view));
            failures.extend(screen.form.table.app_fit_failures(width, language.tag()));
            search_editor(&mut screen, 71, "");
            failures.extend(screen.form.table.app_fit_failures(width, language.tag()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn an_empty_search_result_keeps_a_visible_route_to_change_or_clear_the_query() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    let mut view = (*searchable(1, "no match")).clone();
    view.rows.clear();
    screen.install(Arc::new(view));
    assert_eq!(screen.form.selected_id().map(String::as_str), Some("nav:search"));
}


#[test]
fn clear_search_is_a_typed_action_and_disappears_after_clearing() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    let mut view = (*searchable(1, "missing")).clone();
    view.rows.clear();
    screen.install(Arc::new(view));
    let clear = screen.keys["nav:clear-search"];
    let changes = effects(|fx| screen.activate(clear, false, fx));
    let actions: Vec<_> = changes.iter().filter_map(|x| match &x.fx {
        Fx::App(AppFx::Ott(OttOut::Action { action, .. })) => Some(action), _ => None,
    }).collect();
    assert_eq!(actions, vec![&OttAction::ClearSearch]);
    assert!(screen.editor.is_none());
    screen.install(searchable(2, ""));
    assert!(effects(|fx| screen.activate(clear, false, fx)).is_empty());
    assert_eq!(format!("{:?}", OttAction::ClearSearch), "ClearSearch");
}


#[test]
fn search_caption_and_navigation_detail_preserve_the_exact_scope_in_every_locale() {
    let _serial = plx_base::testlock::serial();
    for language in plx_platform::i18n::SHIPPED {
        let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
        for favorites in [false, true] {
            let scope = if favorites { msg::ott_favorites() } else { "My folder · Drama" };
            let search = OttSearch { query: "needle".into(), scope: scope.into() };
            let expected = if favorites { msg::ott_search_favorites().to_owned() }
                else { msg::ott_search_scope_title(scope) };
            assert_eq!(search.title(favorites), expected);
            let mut view = (*snapshot(1, &["first"])).clone();
            view.page_id = if favorites { "favorites:owner" } else { "channels:owner" }.into();
            view.search = Some(search.clone());
            let actions = crate::ott::navigation::available(&view, Some(0), Default::default());
            let action = actions.iter().find(|action| action.id == "nav:search").unwrap();
            assert_eq!(action.row.label, expected);
            assert_eq!(action.row.detail, format!("{scope} · needle"));
            assert!(matches!(action.action, FormAction::Forward(OttAction::BeginSearch)));
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            screen.install(Arc::new(view));
            deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor {
                session: 71, kind: OttEditorKind::Search, text_policy: OttTextPolicy::Predictive,
                key: String::new(), title: if favorites { String::new() } else { scope.into() }, value: String::new(),
                secret: false, limit: 256,
            }))));
            assert_eq!(screen.view(&plx_ui::fixture::FixtureMeasure).header.title, expected);
        }
    }
}

#[test]
fn search_label_origin_tracks_application_copy_and_user_scope() {
    use plx_ui::table::Origin;
    for (page, scope, origin) in [
        ("favorites:owner", "Favorites", Origin::App),
        ("channels:owner", "", Origin::App),
        ("channels:owner", "My folder · Drama", Origin::Server),
    ] {
        let mut view = (*snapshot(1, &["first"])).clone();
        view.page_id = page.into();
        view.search = Some(OttSearch { query: String::new(), scope: scope.into() });
        let actions = crate::ott::navigation::available(&view, Some(0), Default::default());
        assert_eq!(actions.iter().find(|action| action.id == "nav:search").unwrap().row.label_origin, origin);
    }
}

// Drive the actual screen event path in Classic mode; TestHost's default is Modern.
struct SearchReturnHost;
impl Host for SearchReturnHost {
    type Arg = crate::registry::AppArg;
    type Fx = AppFx;
    type Msg = AppMsg;
    type Elem = u32;
    type Views<'a> = plx_machine::machine::UiMode;
    type Init = crate::registry::AppArg;
    type Memory = PageMemory;
    fn ui_mode(mode: &plx_machine::machine::UiMode) -> plx_machine::machine::UiMode { *mode }
}
fn return_step(screen: &mut OttScreen, mode: plx_machine::machine::UiMode,
    event: ScreenEvent<SearchReturnHost>) -> Vec<OttAction> {
    let mut cx = Cx::<SearchReturnHost> { views: mode, tick: Default::default(),
        measure: &plx_ui::fixture::FixtureMeasure, focus: Default::default(), press: Default::default(),
        owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
    // A snapshot may precede reconciliation of the old Actions/header focus.
    cx.focus.current = screen.keys.get("nav:search").map(|elem| FocusKey { entry: EntryId(1), elem: *elem });
    let mut output = Vec::new();
    let mut present = plx_machine::present::Present::new();
    screen.step(&event, &cx, &mut Effects::new(&mut output, MachineId::Instance(InstanceId(1)), &mut present));
    assert_eq!(screen.classic_mode, mode == plx_machine::machine::UiMode::Classic);
    output.into_iter().filter_map(|event| match event.fx {
        Fx::App(AppFx::Ott(OttOut::Action { action, .. })) => Some(action), _ => None,
    }).collect()
}
fn return_key(key: Key, edge: Edge) -> ScreenEvent<SearchReturnHost> {
    ScreenEvent::Input(plx_machine::machine::InputEvent { at: Default::default(),
        source: plx_machine::machine::Source::RemoteFifo,
        kind: InputKind::Key { key, sym: 0, wcode: 0, edge, at_edge: false } })
}
fn return_view(page: &str, revision: u64, query: &str, ids: &[&str]) -> Arc<OttView> {
    let mut view = (*snapshot(revision, ids)).clone();
    view.page_id = page.into();
    view.search = Some(OttSearch { query: query.into(), scope: "Fixture category".into() });
    view.selected_row = Some("origin".into());
    view.preview_context = Some("fixture-source".into());
    Arc::new(view)
}
fn return_editor(screen: &mut OttScreen, session: u64) {
    return_step(screen, plx_machine::machine::UiMode::Classic,
        ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor {
            session, kind: OttEditorKind::Search, text_policy: OttTextPolicy::Predictive,
            key: String::new(), title: "Fixture category".into(), value: "needle".into(),
            secret: false, limit: 256,
        }))));
}
fn return_submit(screen: &mut OttScreen, page: &str, revision: u64, session: u64) {
    let mode = plx_machine::machine::UiMode::Classic;
    let save = screen.keys["edit:save"];
    assert!(matches!(return_step(screen, mode, ScreenEvent::Activate(save)).as_slice(),
        [OttAction::SubmitSearch { session: actual, .. }] if *actual == session));
    // Production publishes the filtered snapshot before acknowledging the editor.
    return_step(screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(
        return_view(page, revision, "needle", &["first", "last"]))));
    return_step(screen, mode, ScreenEvent::App(AppMsg::OttEditorResult { session, error: None }));
    assert!(screen.editor.is_none());
}
fn return_results(page: &str) -> OttScreen {
    let mode = plx_machine::machine::UiMode::Classic;
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    // Simulate Actions -> admitted catalogue publication -> Search editor. The
    // old header focus in return_step must not become the return destination.
    return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(
        return_view(page, 1, "", &["first", "origin", "last"]))));
    let mut actions = (*snapshot(2, &["action"])).clone();
    actions.page_id = "actions".into();
    return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(actions))));
    // The runtime's last recorded focus may lag the real pre-Actions highlight.
    let mut returned = (*return_view(page, 2, "", &["first", "origin", "last"])).clone();
    returned.selected_row = Some("first".into());
    return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(returned))));
    assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:first"));
    return_editor(&mut screen, 71);
    return_submit(&mut screen, page, 3, 71);
    assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:first"));
    screen
}

#[test]
fn classic_search_back_waits_for_clear_and_restores_stable_origin_after_reorder() {
    let _serial = plx_base::testlock::serial();
    let mode = plx_machine::machine::UiMode::Classic;
    for page in ["channels:category-a", "favorites:mixed"] {
        let mut screen = return_results(page);
        assert_eq!(return_step(&mut screen, mode, return_key(Key::Back, Edge::Down)), [OttAction::ClearSearch]);
        assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:first"), "no eager restore");
        for edge in [Edge::Repeat, Edge::Up] {
            assert!(return_step(&mut screen, mode, return_key(Key::Back, edge)).is_empty());
        }
        // An unrelated catalogue publication is not an acknowledgement of clear.
        return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(
            return_view(page, 4, "needle", &["first", "last"]))));
        return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(
            return_view(page, 5, "", &["last", "first", "origin"]))));
        assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:origin"));
        assert_eq!(screen.media.map(|i| screen.snapshot.rows[i].id.as_str()), Some("origin"));
        assert_eq!(screen.focus_request, Some(screen.keys["item:origin"]));
        assert_eq!(screen.snapshot.page_id, page);
        assert_eq!(return_step(&mut screen, mode, return_key(Key::Back, Edge::Down)), [OttAction::Back]);
    }
}

#[test]
fn classic_search_requery_cancel_and_failed_submit_keep_first_origin() {
    let _serial = plx_base::testlock::serial();
    let mode = plx_machine::machine::UiMode::Classic;
    for cancel in [false, true] {
        let page = "channels:category-a";
        let mut screen = return_results(page);
        let mut actions = (*snapshot(4, &["action"])).clone();
        actions.page_id = "actions".into();
        return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(actions))));
        return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(
            return_view(page, 5, "needle", &["first", "last"]))));
        return_editor(&mut screen, 72);
        if cancel {
            assert_eq!(return_step(&mut screen, mode, return_key(Key::Back, Edge::Down)),
                [OttAction::CancelSearch { session: 72 }]);
        } else {
            let save = screen.keys["edit:save"];
            return_step(&mut screen, mode, ScreenEvent::Activate(save));
            return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttEditorResult {
                session: 72, error: Some("Fixture validation refusal".into()),
            }));
            assert!(!screen.editor.as_ref().unwrap().pending);
            return_submit(&mut screen, page, 6, 72);
        }
        assert_eq!(screen.snapshot.search.as_ref().unwrap().query, "needle");
        assert_eq!(return_step(&mut screen, mode, return_key(Key::Back, Edge::Down)), [OttAction::ClearSearch]);
        return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(
            return_view(page, 7, "", &["last", "origin", "first"]))));
        assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:origin"));
    }
}

#[test]
fn classic_search_back_respects_inner_surfaces_and_direct_clear_keeps_current_focus() {
    let _serial = plx_base::testlock::serial();
    let mode = plx_machine::machine::UiMode::Classic;
    let page = "channels:category-a";
    let mut screen = return_results(page);
    let details = screen.keys["nav:description"];
    return_step(&mut screen, mode, ScreenEvent::Activate(details));
    assert!(screen.reading);
    assert!(return_step(&mut screen, mode, return_key(Key::Back, Edge::Down)).is_empty());
    assert!(!screen.reading);
    let browse = screen.keys["nav:browse"];
    return_step(&mut screen, mode, ScreenEvent::Activate(browse));
    assert!(screen.browse);
    assert!(return_step(&mut screen, mode, return_key(Key::Back, Edge::Down)).is_empty());
    assert!(!screen.browse);
    // An ordinary clear is not an instruction to restore the search bookmark.
    return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(
        return_view(page, 4, "", &["last", "origin", "first"]))));
    assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:first"));
}

#[test]
fn classic_search_back_drops_missing_origin_and_pending_restore_after_user_input() {
    let _serial = plx_base::testlock::serial();
    let mode = plx_machine::machine::UiMode::Classic;
    let page = "channels:category-a";
    for moved in [false, true] {
        let mut screen = return_results(page);
        return_step(&mut screen, mode, return_key(Key::Back, Edge::Down));
        if moved {
            let last = screen.keys["item:last"];
            return_step(&mut screen, mode, ScreenEvent::FocusMoved { from: None,
                to: FocusKey { entry: EntryId(1), elem: last }, by: plx_ui::screen::By::Dir });
        }
        let rows: &[&str] = if moved { &["first", "origin", "last"] } else { &["last", "first"] };
        let mut cleared = (*return_view(page, 4, "", rows)).clone();
        cleared.selected_row = Some("first".into());
        return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(cleared))));
        assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:last"));
        assert_ne!(screen.focus_request, Some(screen.keys["item:origin"]));
    }
}

#[test]
fn classic_search_origin_cannot_cross_source_category_mode_or_leave() {
    let _serial = plx_base::testlock::serial();
    let classic = plx_machine::machine::UiMode::Classic;
    for change in 0..4 {
        let page = "channels:category-a";
        let mut screen = return_results(page);
        let mut replacement = (*return_view(page, 4, "needle", &["first", "last"])).clone();
        let mode = if change == 2 { plx_machine::machine::UiMode::Modern } else { classic };
        match change {
            0 => replacement.preview_context = Some("new-source".into()),
            1 => replacement.page_id = "channels:category-b".into(),
            2 => {},
            _ => { return_step(&mut screen, mode, ScreenEvent::Unmount); },
        }
        return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(replacement.clone()))));
        let actions = return_step(&mut screen, mode, return_key(Key::Back, Edge::Down));
        assert_eq!(actions, if change == 2 { vec![OttAction::Back] } else { vec![OttAction::ClearSearch] });
        replacement.revision = 5;
        replacement.search.as_mut().unwrap().query.clear();
        replacement.rows = snapshot(5, &["first", "origin", "last"]).rows.clone();
        return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(replacement))));
        assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:first"));
    }
}

#[test]
fn classic_empty_search_visible_back_reuses_clear_and_retries_without_losing_origin() {
    let _serial = plx_base::testlock::serial();
    let mode = plx_machine::machine::UiMode::Classic;
    let page = "channels:category-a";
    let mut screen = return_results(page);
    return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(return_view(page, 4, "missing", &[]))));
    let back = screen.keys["nav:back"];
    assert!(screen.form.index_of_key(RowKey(back)).is_some());
    assert_eq!(return_step(&mut screen, mode, ScreenEvent::Activate(back)), [OttAction::ClearSearch]);
    // A refusal/ordinary update is not a successful clear. A fresh press can retry.
    let mut denied = (*return_view(page, 5, "missing", &[])).clone();
    denied.status = "Fixture policy refusal".into();
    return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(denied))));
    assert_eq!(return_step(&mut screen, mode, return_key(Key::Back, Edge::Down)), [OttAction::ClearSearch]);
    return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(
        return_view(page, 6, "", &["last", "origin", "first"]))));
    assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:origin"));
}

#[test]
fn classic_actions_return_then_deliberate_selection_does_not_reuse_old_search_origin() {
    let _serial = plx_base::testlock::serial();
    let mode = plx_machine::machine::UiMode::Classic;
    let page = "channels:category-a";
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(
        return_view(page, 1, "", &["first", "origin", "last"]))));
    let mut actions = (*snapshot(2, &["action"])).clone();
    actions.page_id = "actions".into();
    return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(actions))));
    assert_eq!(return_step(&mut screen, mode, return_key(Key::Back, Edge::Down)), [OttAction::Back]);
    return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(
        return_view(page, 3, "", &["first", "origin", "last"]))));
    let last = screen.keys["item:last"];
    return_step(&mut screen, mode, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: last }, by: plx_ui::screen::By::Dir });
    return_editor(&mut screen, 81);
    return_submit(&mut screen, page, 4, 81);
    return_step(&mut screen, mode, return_key(Key::Back, Edge::Down));
    return_step(&mut screen, mode, ScreenEvent::App(AppMsg::OttSnapshot(
        return_view(page, 5, "", &["first", "origin", "last"]))));
    assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:last"));
}
