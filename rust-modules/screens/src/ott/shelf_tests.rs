//! Real dispatcher coverage of category occurrences; no provider or playback owner here.
use super::*;

fn fixture(kind: OttMediaKind) -> Arc<OttView> {
    let ids: Vec<String> = (0..16).map(|i| format!("entry-{i}")).collect();
    let refs: Vec<_> = ids.iter().map(String::as_str).collect();
    let mut view = (*card_snapshot("channels", 1, kind, &refs, "entry-0")).clone();
    view.shelf_context = Some("presentation-source".into());
    view.shelves = vec![
        OttShelf { id: "category-a".into(), title: "First category".into(), rows: (0..12).collect() },
        OttShelf { id: "category-b".into(), title: "Second category".into(), rows: vec![0, 12, 13, 14, 15] },
    ];
    Arc::new(view)
}
fn screen(d: &Dispatcher<TestHost>) -> &OttScreen {
    d.nav.top_page().unwrap().inst.as_ref().unwrap().screen.as_any().unwrap().downcast_ref().unwrap()
}
fn assert_item(d: &Dispatcher<TestHost>, category: &str, item: &str) {
    let current = screen(d);
    let id = media::occurrence_id(current.snapshot.shelf_context.as_deref().unwrap(), category, item);
    assert_focus(d, &id);
}
fn step(d: &mut Dispatcher<TestHost>, rig: &mut TestRig, now: &mut u32, direction: Key) {
    frame(d, rig, *now, vec![key(direction, tick(*now))]);
    frame(d, rig, *now + 16, vec![key_up(direction, tick(*now + 16))]);
    for offset in (32..=240).step_by(16) { frame(d, rig, *now + offset, vec![]); }
    *now += 256;
}
fn selected_shelf_destination(d: &Dispatcher<TestHost>) -> (GroupId, u32, f32) {
    let current = screen(d);
    let key = d.focus().unwrap().elem;
    let shelf = current.shelves.iter().find(|shelf| shelf.index.contains_key(&key)).unwrap();
    let motion = current.card_stack.shelf(media::Section::Shelf(shelf.group.0)).unwrap();
    (shelf.group, key, motion.scroll() - motion.settle_lag(shelf.keys.len(), shelf.index[&key]))
}
fn assert_scroll_positions_retained(actual: &[(media::Section, f32)], expected: &[(media::Section, f32)]) {
    assert_eq!(actual.len(), expected.len());
    for ((actual_id, actual), (expected_id, expected)) in actual.iter().zip(expected) {
        assert_eq!(actual_id, expected_id);
        // A snapshot/restore frame still advances the same scroll spring. The
        // observed CI residual is <=0.0037 logical px; allow only 1/100 px here.
        // The selected identity and shared scroller's destination are pinned
        // exactly at the call site, and source-reset-to-zero remains exact.
        assert!((actual - expected).abs() <= 0.01, "retained viewport: {actual} vs {expected}");
    }
}
fn settle(d: &mut Dispatcher<TestHost>, rig: &mut TestRig, now: &mut u32) {
    for offset in (0..=800).step_by(16) { frame(d, rig, *now + offset, vec![]); }
    *now += 816;
}

#[test]
fn category_shelves_remember_each_rows_item_on_vertical_and_wheel_keys() {
    let _guard = plx_base::testlock::serial();
    let (mut d, mut rig) = mount_snapshot(fixture(OttMediaKind::Movie));
    let mut now = 256;
    assert_item(&d, "category-a", "entry-0");
    for _ in 0..3 { step(&mut d, &mut rig, &mut now, Key::Right); }
    assert_item(&d, "category-a", "entry-3");
    // SDL wheel_input emits this same normalized Down/Up pair.
    step(&mut d, &mut rig, &mut now, Key::Down);
    assert_item(&d, "category-b", "entry-0");
    step(&mut d, &mut rig, &mut now, Key::Right);
    assert_item(&d, "category-b", "entry-12");
    step(&mut d, &mut rig, &mut now, Key::Up);
    assert_item(&d, "category-a", "entry-3");
    step(&mut d, &mut rig, &mut now, Key::Down);
    assert_item(&d, "category-b", "entry-12");
    assert!(rig.actions.iter().all(|action| matches!(action,
        OttAction::Preview(_) | OttAction::PreviewAt { .. } | OttAction::FocusAt { .. })));
}

#[test]
fn shared_item_occurrences_have_distinct_focus_and_canonical_tap_hold_actions() {
    let _guard = plx_base::testlock::serial();
    for hold in [false, true] {
        let (mut d, mut rig) = mount_snapshot(fixture(OttMediaKind::Movie));
        let mut now = 256;
        let first = d.focus().unwrap();
        step(&mut d, &mut rig, &mut now, Key::Down);
        assert_item(&d, "category-b", "entry-0");
        assert_ne!(d.focus().unwrap(), first);
        assert_eq!(screen(&d).snapshot.rows.len(), 16, "membership never duplicates metadata");
        assert_eq!(screen(&d).form.selected_id().map(String::as_str), Some("item:entry-0"));
        rig.actions.clear();
        frame(&mut d, &mut rig, now, vec![key(Key::Ok, tick(now))]);
        let end = now + if hold { plx_ui::press::LONG_MS + 64 } else { 64 };
        for ms in (now + 16..end).step_by(16) { frame(&mut d, &mut rig, ms, vec![]); }
        frame(&mut d, &mut rig, end, vec![key_up(Key::Ok, tick(end))]);
        now = end + 16;
        settle(&mut d, &mut rig, &mut now);
        let actions: Vec<_> = rig.actions.iter().filter(|action| !matches!(action,
            OttAction::Preview(_) | OttAction::PreviewAt { .. } | OttAction::FocusAt { .. })).collect();
        assert_eq!(actions, vec![&if hold { OttAction::ToggleFavorite("entry-0".into()) }
            else { OttAction::Play("entry-0".into()) }]);
    }
}

#[test]
fn shelf_boundaries_and_channel_paging_stay_in_the_current_row() {
    let _guard = plx_base::testlock::serial();
    let (mut d, mut rig) = mount_snapshot(fixture(OttMediaKind::Movie));
    let mut now = 256;
    step(&mut d, &mut rig, &mut now, Key::Left);
    assert_item(&d, "category-a", "entry-0");
    let first = d.focus().unwrap().elem;
    let expected = screen(&d).remote_page_key(first, 1).unwrap();
    let event = |edge| InputEvent { at: tick(now), source: plx_machine::machine::Source::Sdl,
        kind: InputKind::Key { key: Key::Other, sym: 0, wcode: 301, edge, at_edge: false } };
    frame(&mut d, &mut rig, now, vec![event(Edge::Down), event(Edge::Up)]);
    assert_eq!(d.focus().unwrap().elem, expected);
    now += 256;
    for _ in 0..20 { step(&mut d, &mut rig, &mut now, Key::Right); }
    assert_item(&d, "category-a", "entry-11");
    step(&mut d, &mut rig, &mut now, Key::Right);
    assert_item(&d, "category-a", "entry-11");
    assert_eq!(screen(&d).remote_page_key(d.focus().unwrap().elem, 1), Some(d.focus().unwrap().elem));
}

#[test]
fn shelf_append_reorder_and_source_replacement_preserve_only_valid_occurrences() {
    let _guard = plx_base::testlock::serial();
    let (mut d, mut rig) = mount_snapshot(fixture(OttMediaKind::Movie));
    let mut now = 256;
    step(&mut d, &mut rig, &mut now, Key::Down);
    step(&mut d, &mut rig, &mut now, Key::Right);
    let kept = d.focus().unwrap();
    let mut next = (*rig.snapshot).clone();
    next.revision += 1;
    next.shelves.reverse();
    next.shelves[0].rows.swap(0, 1);
    publish_snapshot(&mut d, &mut rig, Arc::new(next), now);
    assert_eq!(d.focus(), Some(kept));
    assert_item(&d, "category-b", "entry-12");
    now += 256;
    let mut next = (*rig.snapshot).clone();
    next.revision += 1;
    next.preview_context = Some("replacement-action".into());
    next.shelf_context = Some("replacement-source".into());
    next.selected_row = Some("entry-0".into());
    publish_snapshot(&mut d, &mut rig, Arc::new(next), now);
    assert_ne!(d.focus(), Some(kept));
    assert_item(&d, "category-b", "entry-0");
}

#[test]
fn shelf_browse_return_and_navigation_memory_keep_occurrence_and_scroll() {
    let _guard = plx_base::testlock::serial();
    let (mut d, mut rig) = mount_snapshot(fixture(OttMediaKind::Movie));
    let mut now = 256;
    for _ in 0..8 { step(&mut d, &mut rig, &mut now, Key::Right); }
    settle(&mut d, &mut rig, &mut now);
    let expected = d.focus().unwrap();
    let memory = Memory::capture(screen(&d), Some(expected));
    assert!(memory.shelves.iter().any(|(_, scroll)| *scroll > 0.0));
    let completed = open_browse(&mut d, &mut rig, now);
    now = completed + 16;
    step(&mut d, &mut rig, &mut now, Key::Back);
    assert_eq!(d.focus(), Some(expected));
    let saved = screen(&d).card_stack.memory();
    let destination = selected_shelf_destination(&d);
    let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
    d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
        Delivery::Screen(ScreenEvent::RestoreMemory(PageMemory::Ott(memory)))));
    frame(&mut d, &mut rig, now, vec![]);
    assert_eq!(d.focus(), Some(expected));
    assert_eq!(selected_shelf_destination(&d), destination);
    assert_scroll_positions_retained(&screen(&d).card_stack.memory().shelves, &saved.shelves);
}

#[test]
fn category_shelves_do_not_change_classic_search_favorites_or_invalid_maps() {
    let _guard = plx_base::testlock::serial();
    for case in 0..5 {
        let mut view = (*fixture(OttMediaKind::Movie)).clone();
        let mode = if case == 0 { UiMode::Classic } else { UiMode::Modern };
        match case {
            1 => view.page_id = "favorites".into(),
            2 => view.search = Some(OttSearch { query: "entry".into(), scope: String::new() }),
            3 => view.shelves[0].rows.push(999),
            4 => { view.shelves.pop(); }
            _ => {}
        }
        let (d, _) = mount_snapshot_in_mode(Arc::new(view), mode);
        assert!(screen(&d).shelves.is_empty());
        assert_focus(&d, "item:entry-0");
    }
}

#[test]
fn portrait_and_landscape_shelf_focus_is_visible_in_the_right_content_column() {
    let _guard = plx_base::testlock::serial();
    for kind in [OttMediaKind::Movie, OttMediaKind::LiveChannel] {
        let (mut d, mut rig) = mount_snapshot(fixture(kind));
        let mut now = 256;
        for _ in 0..8 { step(&mut d, &mut rig, &mut now, Key::Right); }
        settle(&mut d, &mut rig, &mut now);
        let current = d.focus().unwrap();
        let cx = Cx { views: rig.mode, tick: tick(now), measure: &rig.measure,
            press: Default::default(), focus: plx_machine::machine::FocusRead { current: Some(current), ..Default::default() },
            owner: InputOwner::Entry(current.entry) };
        let placed = Focusable::<TestHost>::place(screen(&d), &current.elem, &cx, At::Drawn).unwrap();
        let content = navigation::route().content;
        assert!(placed.rect.x >= content.x - 1.0, "left edge: {:?}", placed.rect);
        assert!(placed.rect.x + placed.rect.w <= content.x + content.w + 1.0, "right edge: {:?}", placed.rect);
        assert!(placed.rect.y >= content.y - 1.0);
        assert!(placed.rect.y + placed.rect.h <= content.y + content.h + 1.0);
    }
}

#[test]
fn removed_remembered_item_reenters_its_category_without_stale_or_other_row_focus() {
    let _guard = plx_base::testlock::serial();
    let (mut d, mut rig) = mount_snapshot(fixture(OttMediaKind::Movie));
    let mut now = 256;
    step(&mut d, &mut rig, &mut now, Key::Right);
    assert_item(&d, "category-a", "entry-1");
    step(&mut d, &mut rig, &mut now, Key::Down);
    step(&mut d, &mut rig, &mut now, Key::Right);
    let mut next = (*rig.snapshot).clone();
    next.revision += 1;
    next.shelves[0].rows.retain(|row| *row != 1);
    // The entry remains in the shared catalogue under another real category.
    next.shelves[1].rows.push(1);
    publish_snapshot(&mut d, &mut rig, Arc::new(next), now);
    now += 256;
    step(&mut d, &mut rig, &mut now, Key::Up);
    assert_item(&d, "category-a", "entry-0");
}

#[test]
fn action_context_rotation_keeps_shelf_memory_but_source_replacement_retires_it() {
    let _guard = plx_base::testlock::serial();
    let (mut d, mut rig) = mount_snapshot(fixture(OttMediaKind::Movie));
    let mut now = 256;
    for _ in 0..8 { step(&mut d, &mut rig, &mut now, Key::Right); }
    settle(&mut d, &mut rig, &mut now);
    let focus = d.focus().unwrap();
    let memory = Memory::capture(screen(&d), Some(focus));
    let scroll = screen(&d).card_stack.memory();
    let destination = selected_shelf_destination(&d);
    assert!(scroll.shelves.iter().any(|(_, scroll)| *scroll > 1.0));
    assert_eq!(memory.source.as_deref(), Some("presentation-source"));
    let mut next = (*rig.snapshot).clone();
    next.revision += 1;
    next.preview_context = Some("fresh-action-authority".into());
    publish_snapshot(&mut d, &mut rig, Arc::new(next), now);
    now += 256;
    assert_eq!(d.focus(), Some(focus));
    assert_eq!(selected_shelf_destination(&d), destination);
    assert_scroll_positions_retained(&screen(&d).card_stack.memory().shelves, &scroll.shelves);
    rig.actions.clear();
    step(&mut d, &mut rig, &mut now, Key::Right);
    assert!(rig.actions.iter().any(|action| matches!(action,
        OttAction::FocusAt { context, id } if context == "fresh-action-authority" && id == "entry-9")));
    let mut next = (*rig.snapshot).clone();
    next.revision += 1;
    next.shelf_context = Some("new-presentation-source".into());
    next.selected_row = Some("entry-0".into());
    publish_snapshot(&mut d, &mut rig, Arc::new(next), now);
    assert_item(&d, "category-a", "entry-0");
    assert_ne!(d.focus(), Some(focus));
    assert!(screen(&d).card_stack.memory().shelves.iter().all(|(_, scroll)| *scroll == 0.0));
    let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
    d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
        Delivery::Screen(ScreenEvent::RestoreMemory(PageMemory::Ott(memory)))));
    frame(&mut d, &mut rig, now + 16, vec![]);
    assert_item(&d, "category-a", "entry-0");
}

#[test]
fn flat_grid_and_classic_memory_keep_their_existing_restore_contract() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut snapshot = (*fixture(OttMediaKind::Movie)).clone();
        snapshot.shelves.clear();
        snapshot.shelf_context = None;
        let (mut d, mut rig) = mount_snapshot_in_mode(Arc::new(snapshot), mode);
        let mut now = 256;
        step(&mut d, &mut rig, &mut now, if mode == UiMode::Classic { Key::Down } else { Key::Right });
        assert_focus(&d, "item:entry-1");
        let memory = Memory::capture(screen(&d), d.focus());
        assert_eq!(memory.source, None);
        let mut returned = (*rig.snapshot).clone();
        returned.revision += 1;
        returned.preview_context = Some("new-action-token-after-navigation".into());
        returned.selected_row = Some("entry-0".into());
        let mut restored = OttScreen::new(d.focus().unwrap().entry, InstanceId(300));
        restored.classic_mode = mode == UiMode::Classic;
        restored.restore_memory = Some(memory);
        restored.install(Arc::new(returned));
        assert_eq!(restored.form.selected_id().map(String::as_str), Some("item:entry-1"));
        assert!(restored.shelves.is_empty());
    }
}

fn shelf_key(id: &str) -> bool { id.starts_with("shelf-item:") || id.starts_with("shelf-group:") }
fn installed_fixture() -> OttScreen {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(fixture(OttMediaKind::Movie));
    screen
}

#[test]
fn excessive_occurrences_fall_back_atomically_to_every_canonical_row() {
    let mut screen = installed_fixture();
    let mut next = (*screen.snapshot).clone();
    next.revision += 1;
    // Each category is individually valid; only their combined fan-out exceeds
    // the shared catalogue bound (3,126 * 16 = 50,016 occurrences).
    next.shelves = (0..3_126).map(|index| OttShelf { id: format!("category-{index}"),
        title: format!("Category {index}"), rows: (0..16).collect() }).collect();
    let before = screen.next_key;
    screen.install(Arc::new(next));
    assert!(screen.shelves.is_empty());
    assert_eq!(screen.next_key, before, "no partial shelf interning");
    assert_eq!(screen.catalog_keys.len(), 16);
    assert_eq!(screen.media_keys.len(), 16);
    assert!(!screen.keys.keys().any(|id| shelf_key(id)));
    assert!(screen.snapshot.rows.iter().all(|row| screen.keys.contains_key(&format!("item:{}", row.id))));
}

#[test]
fn shelf_key_capacity_accounts_for_group_offset_and_later_canonical_keys() {
    // A new group must fit its reserved +3 offset, not just the allocator increment.
    let mut screen = installed_fixture();
    let mut next = (*screen.snapshot).clone();
    next.revision += 1;
    next.shelves[0].id = "replacement-category".into();
    screen.next_key = crate::registry::BAND - 2;
    screen.install(Arc::new(next));
    assert!(screen.shelves.is_empty());
    assert_eq!(screen.next_key, crate::registry::BAND - 2);
    assert_eq!(screen.catalog_keys.len(), 16);
    assert!(!screen.keys.keys().any(|id| shelf_key(id)));

    // The one new occurrence would fit by itself, but must leave the last key
    // for the canonical row allocated later by the ordinary flat form.
    let mut screen = installed_fixture();
    screen.keys.remove("item:entry-0");
    let mut next = (*screen.snapshot).clone();
    next.revision += 1;
    next.shelves[0].rows.push(12);
    screen.next_key = crate::registry::BAND - 1;
    screen.install(Arc::new(next));
    assert!(screen.shelves.is_empty());
    assert_eq!(screen.keys["item:entry-0"], crate::registry::BAND - 1);
    assert_eq!(screen.next_key, crate::registry::BAND);
    assert_eq!(screen.media_keys.len(), 16);
    assert!(!screen.keys.keys().any(|id| shelf_key(id)));
}

#[test]
fn refresh_prunes_only_retired_shelf_keys_and_never_recycles_ids_on_restore() {
    let mut screen = installed_fixture();
    let source = "presentation-source";
    let first_id = media::occurrence_id(source, "category-a", "entry-0");
    let first = screen.keys[&first_id];
    screen.card_focus = Some(first);
    let memory = Memory::capture(&screen, Some(FocusKey { entry: screen.entry, elem: first }));
    let canonical: Vec<_> = screen.keys.iter().filter(|(id, _)| !shelf_key(id))
        .map(|(id, key)| (id.clone(), *key)).collect();
    let old_next = screen.next_key;
    let mut next = (*screen.snapshot).clone();
    next.revision += 1;
    next.shelves.truncate(1);
    next.shelves[0].rows = (0..16).collect();
    screen.install(Arc::new(next.clone()));
    assert_eq!(screen.keys[&first_id], first);
    assert_eq!(screen.card_focus, Some(first));
    assert_eq!(screen.keys.keys().filter(|id| shelf_key(id)).count(), 17);
    for (id, key) in &canonical { assert_eq!(screen.keys[id], *key); }
    assert!(screen.keys.iter().filter(|(id, _)| id.starts_with("shelf-item:") && !memory.keys.contains_key(*id))
        .all(|(_, key)| *key >= old_next));
    let before_restore = screen.next_key;
    screen.restore_memory = Some(memory);
    next.revision += 1;
    screen.install(Arc::new(next));
    assert!(screen.next_key >= before_restore);
    assert_eq!(screen.keys[&first_id], first);
    assert_eq!(screen.keys.keys().filter(|id| shelf_key(id)).count(), 17, "restored obsolete category is re-pruned");
    assert!(!screen.keys.contains_key(&media::group_id(source, "category-b")));
    let retained_next = screen.next_key;
    let retained_keys = screen.keys.clone();
    let mut renamed = (*screen.snapshot).clone();
    renamed.revision += 1;
    renamed.shelves[0].title = "Renamed category".into();
    screen.install(Arc::new(renamed));
    assert_eq!(screen.next_key, retained_next);
    assert_eq!(screen.keys, retained_keys, "display-only refresh retains live intersection");
}

#[test]
fn invalid_empty_classic_and_filtered_layouts_retire_all_shelf_keys() {
    for case in 0..5 {
        let mut screen = installed_fixture();
        let retired = screen.shelves[0].keys[0];
        screen.card_focus = Some(retired);
        let canonical: Vec<_> = screen.keys.iter().filter(|(id, _)| !shelf_key(id))
            .map(|(id, key)| (id.clone(), *key)).collect();
        let mut next = (*screen.snapshot).clone();
        next.revision += 1;
        match case {
            0 => next.shelves.clear(),
            1 => next.shelves[0].rows.push(999),
            2 => screen.classic_mode = true,
            3 => next.search = Some(OttSearch { query: "entry".into(), scope: String::new() }),
            _ => next.page_id = "favorites".into(),
        }
        screen.install(Arc::new(next));
        assert!(screen.shelves.is_empty());
        assert!(!screen.keys.keys().any(|id| shelf_key(id)));
        assert!(!screen.media_keys.contains_key(&retired));
        assert_eq!(screen.card_focus, None);
        for (id, key) in canonical { assert_eq!(screen.keys[&id], key); }
    }
}
