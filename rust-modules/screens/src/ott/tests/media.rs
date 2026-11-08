use super::*;

fn typed(revision: u64, ids: &[&str], kind: OttMediaKind) -> Arc<OttView> {
    let mut view = (*snapshot(revision, ids)).clone();
    for row in &mut view.rows { row.media = Some(OttMedia { kind, duration_ms: None, progress: None, synopsis: None }); }
    Arc::new(view)
}

#[test]
fn cards_require_typed_enabled_media_and_keep_unknown_or_action_rows_in_the_list() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(typed(1, &["one", "two"], OttMediaKind::Movie));
    assert!(screen.grid_active());
    for change in 0..3 {
        let mut view = (*typed(change + 2, &["one", "two"], OttMediaKind::Movie)).clone();
        match change { 0 => view.rows[1].media = None, 1 => view.rows[1].enabled = false,
            _ => view.rows[1].action = OttAction::ShowSettings }
        screen.install(Arc::new(view));
        assert!(!screen.grid_active());
        assert!(screen.form.index_of(&"item:one".into()).is_some());
        assert!(screen.form.index_of(&"item:two".into()).is_some());
    }
}

#[test]
fn card_presentation_uses_media_type_and_not_the_provider_name() {
    assert_eq!(super::super::media::presentation(&typed(1, &["a"], OttMediaKind::LiveChannel)), Some(true));
    assert_eq!(super::super::media::presentation(&typed(1, &["a"], OttMediaKind::Episode)), Some(true));
    for kind in [OttMediaKind::Movie, OttMediaKind::Series, OttMediaKind::LocalVideo, OttMediaKind::Video] {
        assert_eq!(super::super::media::presentation(&typed(1, &["a"], kind)), Some(false));
    }
}

#[test]
fn progress_uses_known_counts_without_inventing_a_total() {
    let _locale = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::from_tag("en"));
    let mut view = (*typed(1, &["a"], OttMediaKind::Movie)).clone();
    view.catalog_progress = Some(OttCatalogProgress { loaded: 1, total: None, loading: true, complete: false });
    let status = super::super::media::status(&view);
    assert!(status.contains("Loaded: 1"));
    assert!(!status.contains('/'));
    view.catalog_progress.as_mut().unwrap().total = Some(20);
    assert!(super::super::media::status(&view).contains("1 / 20"));
    view.catalog_progress.as_mut().unwrap().total = Some(0);
    assert!(!super::super::media::status(&view).contains('/'));
    view.catalog_progress = None;
    view.status = "Source unavailable".into();
    assert_eq!(super::super::media::status(&view), view.status);
}

#[test]
fn card_memory_does_not_restore_selection_from_a_different_context() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(typed(1, &["one", "two"], OttMediaKind::Movie));
    let key = screen.keys["item:two"];
    let memory = Screen::<TestHost>::memory_at(&screen, Some(FocusKey { entry: EntryId(1), elem: key }));
    let mut fresh = OttScreen::new(EntryId(1), InstanceId(1));
    deliver(&mut fresh, ScreenEvent::RestoreMemory(memory));
    let mut other = (*typed(2, &["other"], OttMediaKind::Movie)).clone();
    other.page_id = "channels:other-profile".into();
    fresh.install(Arc::new(other));
    assert_eq!(fresh.form.selected_id().map(String::as_str), Some("item:other"));
    assert!(!fresh.keys.contains_key("item:two"));
}

#[test]
fn late_catalogue_batch_restores_saved_item_without_holding_the_first_batch() {
    let mut original = OttScreen::new(EntryId(1), InstanceId(1));
    original.install(typed(1, &["first", "late"], OttMediaKind::Movie));
    let late = original.keys["item:late"];
    let memory = Screen::<TestHost>::memory_at(&original, Some(FocusKey { entry: EntryId(1), elem: late }));
    let mut fresh = OttScreen::new(EntryId(1), InstanceId(1));
    deliver(&mut fresh, ScreenEvent::RestoreMemory(memory));
    let mut partial = (*typed(2, &["first"], OttMediaKind::Movie)).clone();
    partial.catalog_progress = Some(OttCatalogProgress { loaded: 1, total: None, loading: true, complete: false });
    fresh.install(Arc::new(partial));
    assert!(fresh.grid_active());
    assert_eq!(fresh.form.selected_id().map(String::as_str), Some("item:first"));
    assert!(fresh.restore_memory.is_some());
    fresh.install(typed(3, &["first", "late"], OttMediaKind::Movie));
    assert_eq!(fresh.form.selected_id().map(String::as_str), Some("item:late"));
    assert_eq!(fresh.focus_request, Some(late));
    assert!(fresh.restore_memory.is_none());
}

#[test]
fn restore_respects_deliberate_selection_and_never_jumps_afterwards() {
    let mut original = OttScreen::new(EntryId(1), InstanceId(1));
    original.install(typed(1, &["first", "late"], OttMediaKind::Movie));
    let memory = Screen::<TestHost>::memory_at(&original, Some(FocusKey { entry: EntryId(1), elem: original.keys["item:late"] }));
    let mut fresh = OttScreen::new(EntryId(1), InstanceId(1));
    deliver(&mut fresh, ScreenEvent::RestoreMemory(memory));
    let mut partial = (*typed(2, &["first"], OttMediaKind::Movie)).clone();
    partial.catalog_progress = Some(OttCatalogProgress { loaded: 1, total: None, loading: true, complete: false });
    fresh.install(Arc::new(partial));
    let first = fresh.keys["item:first"];
    deliver(&mut fresh, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: first }, by: plx_ui::screen::By::Dir });
    assert!(fresh.restore_memory.is_none());
    fresh.install(typed(3, &["first", "late"], OttMediaKind::Movie));
    assert_eq!(fresh.form.selected_id().map(String::as_str), Some("item:first"));
}

#[test]
fn restore_keeps_valid_header_focus_outside_the_body_form() {
    let mut original = OttScreen::new(EntryId(1), InstanceId(1));
    original.install(typed(1, &["first"], OttMediaKind::Movie));
    let browse = original.keys["nav:browse"];
    let memory = Screen::<TestHost>::memory_at(&original, Some(FocusKey { entry: EntryId(1), elem: browse }));
    let mut fresh = OttScreen::new(EntryId(1), InstanceId(1));
    deliver(&mut fresh, ScreenEvent::RestoreMemory(memory));
    fresh.install(typed(2, &["first"], OttMediaKind::Movie));
    assert_eq!(fresh.focus_request, Some(browse));
}

#[test]
fn favorite_card_marker_roundtrips_without_moving_selection_or_hiding_other_state() {
    use plx_ui::cards::{CardSource, StackPage, Tile};
    use plx_ui::icons::Icon;
    let _lock = plx_base::testlock::serial();
    let measure = plx_ui::fixture::FixtureMeasure;
    let cx = Cx { views: (), tick: Default::default(), measure: &measure,
        focus: Default::default(), press: Default::default(),
        owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
    for kind in [OttMediaKind::Movie, OttMediaKind::LiveChannel] {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        for (revision, favorite) in [false, true, false].into_iter().enumerate() {
            let mut snapshot = (*typed(revision as u64 + 1, &["selected", "other"], kind)).clone();
            snapshot.selected_row = Some("selected".into());
            snapshot.rows[0].favorite = favorite;
            snapshot.rows[0].media.as_mut().unwrap().duration_ms = Some(1000);
            snapshot.rows[0].media.as_mut().unwrap().progress = Some(OttMediaProgress { position_ms: 250, duration_ms: 1000 });
            screen.install(Arc::new(snapshot));
            assert!(screen.grid_active());
            assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:selected"));
            let view = screen.grid_view();
            let cards = <super::super::media::Page as StackPage<TestHost>>::cards(&view.page, &cx,
                super::super::media::Section::Items).unwrap();
            assert_eq!(CardSource::<TestHost>::progress(&cards, 0), Some(0.25));
            for focused in [false, true] {
                let style = super::super::media::style(kind == OttMediaKind::LiveChannel);
                let scale = if focused { style.focus_scale } else { 1.0 };
                let tile = Tile { rect: plx_ui::Rect::new(
                    100.0 - style.w * (scale - 1.0) * 0.5, 100.0 - style.h * (scale - 1.0) * 0.5,
                    style.w * scale, style.h * scale), scale, radius: style.radius * scale, focused };
                let marks = plx_ui::draw_census::capture_icons(||
                    CardSource::<TestHost>::overlay(&cards, plx_ui::Painter::recording(), 0, &tile, &measure));
                assert_eq!(marks.iter().filter(|(icon, _)| *icon == Icon::BookmarkFill).count(), usize::from(favorite),
                    "favorite={favorite}, focused={focused}, kind={kind:?}");
                if favorite {
                    let rect = marks.iter().find(|(icon, _)| *icon == Icon::BookmarkFill).unwrap().1;
                    assert!(rect.x > tile.rect.x + tile.rect.w * 0.65);
                    assert!(rect.y >= tile.rect.y && rect.y < tile.rect.y + tile.rect.h * 0.3);
                    assert!(rect.x + rect.w <= tile.rect.x + tile.rect.w);
                    assert!(rect.y + rect.h <= tile.rect.y + tile.rect.h);
                }
                let other = plx_ui::draw_census::capture_icons(||
                    CardSource::<TestHost>::overlay(&cards, plx_ui::Painter::recording(), 1, &tile, &measure));
                assert!(other.is_empty(), "the mark belongs only to its exact row");
            }
        }
    }
}

fn restore_late_selection_with_header_focus() -> OttScreen {
    let mut original = OttScreen::new(EntryId(1), InstanceId(1));
    original.install(typed(1, &["first", "late"], OttMediaKind::Movie));
    let late = original.keys["item:late"];
    deliver(&mut original, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: late }, by: plx_ui::screen::By::Dir });
    let memory = Screen::<TestHost>::memory_at(&original,
        Some(FocusKey { entry: EntryId(1), elem: original.keys["nav:browse"] }));
    let mut fresh = OttScreen::new(EntryId(1), InstanceId(1));
    deliver(&mut fresh, ScreenEvent::RestoreMemory(memory));
    let mut partial = (*typed(2, &["first"], OttMediaKind::Movie)).clone();
    partial.catalog_progress = Some(OttCatalogProgress { loaded: 1, total: None, loading: true, complete: false });
    fresh.install(Arc::new(partial));
    fresh
}

#[test]
fn pending_selected_item_returns_without_stealing_header_focus() {
    let mut fresh = restore_late_selection_with_header_focus();
    assert!(fresh.grid_active(), "the first loaded batch is usable");
    assert_eq!(fresh.snapshot.rows[fresh.media.unwrap()].id, "first");
    assert_eq!(fresh.focus_request, Some(fresh.keys["nav:browse"]));
    assert_eq!(fresh.restore_memory.as_ref().and_then(|memory| memory.selected.as_deref()), Some("late"));
    fresh.install(typed(3, &["first", "late"], OttMediaKind::Movie));
    assert_eq!(fresh.snapshot.rows[fresh.media.unwrap()].id, "late");
    assert_eq!(fresh.form.selected_id().map(String::as_str), Some("item:late"));
    assert_eq!(fresh.focus_request, Some(fresh.keys["nav:browse"]));
    assert!(fresh.restore_memory.is_none());
}

#[test]
fn deliberate_item_pick_cancels_pending_selection_saved_under_a_header() {
    let mut fresh = restore_late_selection_with_header_focus();
    let first = fresh.keys["item:first"];
    deliver(&mut fresh, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: first }, by: plx_ui::screen::By::Pointer });
    assert!(fresh.restore_memory.is_none());
    fresh.install(typed(3, &["first", "late"], OttMediaKind::Movie));
    assert_eq!(fresh.snapshot.rows[fresh.media.unwrap()].id, "first");
    assert_eq!(fresh.form.selected_id().map(String::as_str), Some("item:first"));
    assert_eq!(fresh.focus_request, None);
}

#[test]
fn header_navigation_updates_pending_focus_without_discarding_selected_item() {
    let mut fresh = restore_late_selection_with_header_focus();
    let description = fresh.keys["nav:description"];
    deliver(&mut fresh, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: description }, by: plx_ui::screen::By::Dir });
    assert_eq!(fresh.restore_memory.as_ref().and_then(|memory| memory.selected.as_deref()), Some("late"));
    fresh.install(typed(3, &["first", "late"], OttMediaKind::Movie));
    assert_eq!(fresh.snapshot.rows[fresh.media.unwrap()].id, "late");
    assert_eq!(fresh.focus_request, Some(description));
}

#[test]
fn a_retired_pending_selection_keeps_body_and_metadata_on_the_same_eligible_item() {
    for disabled in [false, true] {
        let mut fresh = restore_late_selection_with_header_focus();
        let mut next = (*typed(3, &["first", "late"], OttMediaKind::Movie)).clone();
        if disabled { next.rows[1].enabled = false; }
        else { next.rows[1].action = OttAction::ShowSettings; }
        fresh.install(Arc::new(next));
        assert_eq!(fresh.snapshot.rows[fresh.media.unwrap()].id, "first");
        assert_eq!(fresh.form.selected_id().map(String::as_str), Some("item:first"));
        assert_eq!(fresh.focus_request, Some(fresh.keys["nav:browse"]));
        assert!(fresh.restore_memory.is_none());
    }
}

#[test]
fn pending_restore_wins_when_the_interim_item_becomes_disabled() {
    let mut fresh = restore_late_selection_with_header_focus();
    let mut next = (*typed(3, &["first", "late"], OttMediaKind::Movie)).clone();
    next.rows[0].enabled = false;
    fresh.install(Arc::new(next));
    assert_eq!(fresh.snapshot.rows[fresh.media.unwrap()].id, "late");
    assert_eq!(fresh.form.selected_id().map(String::as_str), Some("item:late"));
    assert_eq!(fresh.focus_request, Some(fresh.keys["nav:browse"]));
    assert!(fresh.restore_memory.is_none());
}

#[test]
fn disabling_a_deliberately_chosen_interim_item_does_not_restore_the_cancelled_target() {
    let mut fresh = restore_late_selection_with_header_focus();
    let first = fresh.keys["item:first"];
    deliver(&mut fresh, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: first }, by: plx_ui::screen::By::Pointer });
    assert!(fresh.restore_memory.is_none());
    let mut next = (*typed(3, &["first", "late"], OttMediaKind::Movie)).clone();
    next.rows[0].enabled = false;
    fresh.install(Arc::new(next));
    assert_eq!(fresh.media, None, "intentional disabled selection must not retarget to late");
    assert!(fresh.restore_memory.is_none());
}
