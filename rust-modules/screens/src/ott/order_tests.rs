use super::*;

#[test]
fn actions_door_uses_existing_form_and_never_adds_direct_shuffle_or_repeat_controls() {
    let mut screen = OttScreen::new(EntryId(121), InstanceId(121));
    let view = |context| {
        Arc::new(OttView {
            order_context: context,
            page_id: "channels".into(),
            rows: vec![OttRow { current_programme: None, content_title: None, guide: None,
                guide_available: false, records_available: false,
                id: "movie".into(),
                title: "Movie".into(),
                detail: String::new(),
                description: String::new(),
                poster: None,
                media: None,
                enabled: true,
                favorite: false,
                action: OttAction::Play("movie".into()),
            }],
            ..Default::default()
        })
    };
    screen.install(view(Some("current".into())));
    screen.browse = true;
    screen.install(view(Some("current".into())));
    assert!(screen.media.is_none(), "no retained Browse item means list-only Actions");
    let key = RowKey(screen.keys["nav:actions"]);
    let index = screen
        .form
        .index_of_key(key)
        .expect("Actions in existing Browse form");
    assert!(
        matches!(screen.form.activate(index), Some(Activation::Action(
        FormAction::Forward(OttAction::ShowActions(context)))) if context == "current")
    );
    assert_eq!(screen.snapshot.rows.len(), 1);
    assert!(!screen.keys.contains_key("nav:shuffle"));
    assert!(!screen.keys.contains_key("nav:repeat"));
    screen.install(view(None));
    assert!(screen.form.index_of_key(key).is_none());
}

#[test]
fn actions_page_delivers_typed_operations_and_keeps_back_available() {
    let mut screen = OttScreen::new(EntryId(122), InstanceId(122));
    let row = |id: &str, action, enabled| OttRow { current_programme: None, content_title: None, guide: None,
        guide_available: false, records_available: false,
        id: id.into(),
        title: id.into(),
        detail: String::new(),
        description: String::new(),
        poster: None,
        media: None,
        enabled,
        favorite: false,
        action,
    };
    screen.install(Arc::new(OttView {
        page_id: "actions".into(),
        rows: vec![
            row("order.shuffle", OttAction::ShuffleOrder(9), true),
            row("order.reset", OttAction::ResetOrder(9), false),
            row("order.back", OttAction::CloseActions, true),
        ],
        ..Default::default()
    }));
    let key = RowKey(screen.keys["item:order.shuffle"]);
    let index = screen.form.index_of_key(key).unwrap();
    assert!(matches!(
        screen.form.activate(index),
        Some(Activation::Action(FormAction::Forward(
            OttAction::ShuffleOrder(9)
        )))
    ));
    let back = screen
        .form
        .index_of_key(RowKey(screen.keys["item:order.back"]))
        .unwrap();
    assert!(matches!(
        screen.form.activate(back),
        Some(Activation::Action(FormAction::Forward(
            OttAction::CloseActions
        )))
    ));
}

#[test]
fn order_context_is_part_of_the_canonical_screen_state() {
    let mut screen = OttScreen::new(EntryId(123), InstanceId(123));
    let encode = |screen: &OttScreen| {
        let mut state = Canon::new();
        screen.write(&mut state);
        state.finish()
    };
    let before = encode(&screen);
    // Keep all presentation, row and motion state identical: only the opaque
    // source/order owner changes. Replays must not treat those owners as equal.
    let mut view = (*screen.snapshot).clone();
    view.order_context = Some("source-order-a".into());
    screen.snapshot = Arc::new(view.clone());
    let first = encode(&screen);
    assert_ne!(before, first);
    view.order_context = Some("source-order-b".into());
    screen.snapshot = Arc::new(view.clone());
    assert_ne!(first, encode(&screen));
    view.order_context = None;
    screen.snapshot = Arc::new(view);
    assert_eq!(before, encode(&screen));
}

fn contextual_actions_view() -> OttView {
    let row = |id: &str, action| OttRow {
        current_programme: None, content_title: None, guide: None, guide_available: false, records_available: false,
        id: id.into(), title: "Same visible title".into(), detail: String::new(),
        description: String::new(), poster: None, media: None, enabled: true,
        favorite: false, action,
    };
    OttView {
        page_id: "channels".into(), order_context: Some("current-owner".into()),
        selected_row: Some("row-second".into()),
        rows: vec![
            row("row-first", OttAction::Play("first-id".into())),
            row("row-second", OttAction::Play("second-id".into())),
            row("folder", OttAction::SelectCategory("category-id".into())),
        ],
        ..Default::default()
    }
}

#[test]
fn contextual_actions_capture_the_highlighted_play_action_with_its_owner() {
    let view = contextual_actions_view();
    for (index, expected) in [(0, "first-id"), (1, "second-id")] {
        let actions = navigation::available(&view, Some(index), Default::default());
        let action = actions.into_iter().find(|action| action.id == "nav:actions").unwrap();
        assert!(matches!(action.action, FormAction::Forward(
            OttAction::ShowActionsAt { context, id }) if context == "current-owner" && id == expected));
    }
    assert_eq!(view.selected_row.as_deref(), Some("row-second"));
    assert_eq!(view.rows[0].action, OttAction::Play("first-id".into()));
}

#[test]
fn contextual_actions_keep_list_only_when_no_enabled_play_row_is_highlighted() {
    let mut view = contextual_actions_view();
    view.rows[1].enabled = false;
    for selected in [None, Some(1), Some(2), Some(99)] {
        let actions = navigation::available(&view, selected, Default::default());
        let action = actions.into_iter().find(|action| action.id == "nav:actions").unwrap();
        assert!(matches!(action.action, FormAction::Forward(
            OttAction::ShowActions(context)) if context == "current-owner"));
    }
    view.order_context = None;
    assert!(navigation::available(&view, Some(0), Default::default()).iter()
        .all(|action| action.id != "nav:actions"));
}
