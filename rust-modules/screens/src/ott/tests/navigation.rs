use super::*;

#[test]
fn browse_tools_do_not_extend_the_media_list_and_back_restores_selection() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["first", "second"]));
    let selected = screen.form.selected_id().cloned();
    assert!(screen.form.index_of(&"nav:providers".into()).is_none());
    assert!(screen.header(&plx_ui::fixture::FixtureMeasure).unwrap().stops.iter()
        .any(|(key, _)| *key == screen.keys["nav:browse"]));
    let changes = effects(|fx| screen.open_browse(fx));
    assert!(!changes.iter().any(|x| matches!(x.fx, Fx::App(_))));
    for id in ["nav:providers", "nav:profiles", "nav:catalog", "nav:categories", "nav:favorites", "nav:settings", "nav:description", "nav:play", "nav:favorite"] {
        assert!(screen.form.index_of(&id.into()).is_some(), "{id}");
    }
    assert!(screen.form.index_of(&"item:first".into()).is_none());
    deliver(&mut screen, key_event(Key::Back));
    assert!(!screen.browse);
    assert_eq!(screen.form.selected_id(), selected.as_ref());
}

#[test]
fn visible_favorite_action_uses_the_same_selected_identity_as_hold_ok() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["first", "second"]));
    let key = screen.keys["item:second"];
    deliver(&mut screen, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: key }, by: plx_ui::screen::By::Dir });
    effects(|fx| screen.open_browse(fx));
    let favorite = screen.keys["nav:favorite"];
    let changes = effects(|fx| screen.activate(favorite, false, fx));
    assert_eq!(changes.iter().filter(|x| matches!(&x.fx,
        Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::ToggleFavorite(id), .. })) if id == "second")).count(), 1);
    assert!(!changes.iter().any(|x| matches!(&x.fx,
        Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::Play(_), .. })))));
}

#[test]
fn disabled_media_is_never_selected_previewed_or_activated() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    let mut view = (*snapshot(1, &["disabled", "available"])).clone();
    view.rows[0].enabled = false;
    view.selected_row = Some("disabled".into());
    screen.install(Arc::new(view));
    assert_eq!(screen.media, Some(1));
    let disabled = screen.keys["item:disabled"];
    screen.preview_focus(disabled);
    assert!(!effects(|fx| screen.preview_tick(0.3, fx)).iter().any(|x| matches!(x.fx, Fx::App(_))));
    for hold in [false, true] {
        assert!(!effects(|fx| screen.activate(disabled, hold, fx)).iter().any(|x| matches!(x.fx, Fx::App(_))));
    }
    deliver(&mut screen, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: disabled }, by: plx_ui::screen::By::Pointer });
    assert_eq!(screen.media, None);
    deliver(&mut screen, key_event(Key::Right));
    assert!(!screen.reading);
    effects(|fx| screen.open_browse(fx));
    for id in ["nav:play", "nav:favorite", "nav:description"] {
        assert!(screen.form.index_of(&id.into()).is_none());
    }
    effects(|fx| screen.close_browse(fx));
    let mut view = (*screen.snapshot).clone();
    view.revision += 1;
    view.rows[1].enabled = false;
    screen.install(Arc::new(view));
    assert_eq!(screen.media, None);
    effects(|fx| screen.open_browse(fx));
    assert!(screen.form.index_of(&"nav:play".into()).is_none());
    assert!(screen.form.index_of(&"nav:favorite".into()).is_none());
    assert!(screen.form.index_of(&"nav:description".into()).is_none());
}

#[test]
fn disabling_a_row_retires_its_pending_preview_before_the_dwell_finishes() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["first"]));
    screen.preview_focus(screen.keys["item:first"]);
    let mut view = (*screen.snapshot).clone();
    view.revision += 1;
    view.rows[0].enabled = false;
    screen.install(Arc::new(view));
    assert!(!effects(|fx| screen.preview_tick(0.3, fx)).iter().any(|x| matches!(x.fx, Fx::App(_))));
}

#[test]
fn an_open_editor_keeps_the_full_route_frame_and_excludes_header_input() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["first"]));
    effects(|fx| screen.open_browse(fx));
    editor(&mut screen, "draft");
    assert!(screen.header(&plx_ui::fixture::FixtureMeasure).is_none());
    assert_eq!(screen.route().content, RouteLayout::screen().content);
    for id in ["edit:value", "edit:save", "edit:cancel"] {
        assert!(screen.form.index_of(&id.into()).is_some(), "{id}");
    }
    deliver(&mut screen, key_event(Key::Back));
    assert!(screen.browse);
}

#[test]
fn successful_search_leaves_browse_and_shows_the_filtered_list() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["first", "second"]));
    effects(|fx| screen.open_browse(fx));
    deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor { text_policy: OttTextPolicy::Predictive,
        session: 72, kind: OttEditorKind::Search, key: String::new(), title: "Search".into(),
        value: "first".into(), secret: false, limit: 256,
    }))));
    let save = screen.keys["edit:save"];
    effects(|fx| screen.activate(save, false, fx));
    deliver(&mut screen, ScreenEvent::App(AppMsg::OttSnapshot(snapshot(2, &["first"]))));
    deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditorResult { session: 72, error: None }));
    assert!(!screen.browse);
    assert!(screen.editor.is_none());
    assert!(screen.form.index_of(&"item:first".into()).is_some());
}

#[test]
fn repeated_focus_queries_reuse_the_same_header_layout() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["first"]));
    for _ in 0..20 { assert!(screen.header(&plx_ui::fixture::FixtureMeasure).is_some()); }
    assert_eq!(screen.header_cache.builds(), 1);
}

#[test]
fn empty_search_recovery_and_header_have_disjoint_focus_keys() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    let mut view = (*snapshot(1, &[])).clone();
    view.search = Some(OttSearch { query: "no match".into(), scope: "Home".into() });
    screen.install(Arc::new(view));
    assert!(screen.form.index_of(&"nav:search".into()).is_some());
    let header = screen.header(&plx_ui::fixture::FixtureMeasure).unwrap();
    for (key, _) in &header.stops { assert!(screen.form.index_of_key(RowKey(*key)).is_none()); }
}

#[test]
fn browse_item_actions_do_not_retarget_when_the_original_subject_disappears() {
    for disable in [false, true] {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        screen.install(snapshot(1, &["original", "neighbour"]));
        effects(|fx| screen.open_browse(fx));
        let play = screen.keys["nav:play"];
        let favorite = screen.keys["nav:favorite"];
        deliver(&mut screen, ScreenEvent::FocusMoved { from: None,
            to: FocusKey { entry: EntryId(1), elem: play }, by: plx_ui::screen::By::Dir });
        let mut update = (*screen.snapshot).clone();
        update.revision += 1;
        if disable { update.rows[0].enabled = false; } else { update.rows.remove(0); }
        screen.install(Arc::new(update));
        assert_eq!(screen.media, None);
        for id in ["nav:play", "nav:favorite", "nav:description"] {
            assert!(screen.form.index_of(&id.into()).is_none());
        }
        for key in [play, favorite] {
            assert!(!effects(|fx| screen.activate(key, false, fx)).iter().any(|x| matches!(x.fx, Fx::App(_))));
        }
    }
}
