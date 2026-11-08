//! Contextual Actions enter through the real app dispatcher. Catalogs and Saved
//! ACKs are modeled; the existing revoked writer never persists fixture data.
use super::*;

fn open_at(app: &mut App, id: &str) -> u64 {
    let context = app.ott.as_ref().unwrap().local_order_key().unwrap();
    dispatch(app, OttAction::ShowActionsAt { context, id: id.into() });
    assert_eq!(app.ott.as_ref().unwrap().page, "actions");
    app.ott.as_ref().unwrap().organization.order_menu.as_ref().unwrap().revision
}
fn invoke(app: &mut App, operation: &str) -> Result<(), &'static str> {
    let revision = app.ott.as_ref().unwrap().organization.order_menu.as_ref().unwrap().revision;
    action(app, OttAction::EditSetting(format!("order.{revision}.{operation}")))
}
fn dispatch(app: &mut App, action: OttAction) {
    let mut runtime = app.ott.take().unwrap();
    // Same production wrapper and navigation classification as these actions
    // in the addressed OttOut::Action branch, including started-save rebase.
    apply_ui_action(&mut runtime, app, action, true, false, false);
    app.ott = Some(runtime);
}
fn perform(app: &mut App, operation: &str) {
    let revision = app.ott.as_ref().unwrap().organization.order_menu.as_ref().unwrap().revision;
    dispatch(app, OttAction::EditSetting(format!("order.{revision}.{operation}")));
}
fn settle(app: &mut App, result: Result<Saved, &'static str>) {
    deliver(app, result);
    // Successful focus publication requests the existing separate focus flush.
    // Drain its revoked writer and settle it too; no detached fixture work remains.
    if app.ott.as_ref().unwrap().saving {
        writer(app);
        deliver(app, Err("Fixture focus writer rejected"));
    }
}
fn projected_candidate(runtime: &Runtime, operation: Operation) -> Saved {
    let context = &runtime.organization.order_menu.as_ref().unwrap().context;
    let mut document = Organization::read(&runtime.saved.settings).unwrap();
    document.apply(&Source::new(&context.profile).unwrap(), &runtime.catalog, operation).unwrap();
    let mut saved = runtime.saved.clone();
    document.write_into(&mut saved.settings).unwrap();
    saved
}
fn source_category(runtime: &Runtime) -> String {
    category(runtime, &runtime.organization.order_menu.as_ref().unwrap().context).unwrap()
}
fn has_row(app: &App, id: &str) -> bool {
    app.ott.as_ref().unwrap().local_order_view().rows.iter().any(|row| row.id == id && row.enabled)
}

#[test]
fn contextual_move_uses_highlight_instead_of_playing_and_publishes_focus_after_ack() {
    let _serial = plx_base::testlock::serial();
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            plx_media::player::SHARED.reset_session();
            plx_media::player::TX.reset();
            plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
        }
    }
    let _reset = Reset;
    let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
    local_integration::install(&mut app, None, "live");
    bridge::show_page(&mut app.pages, AppArg::Ott);
    for _ in 0..24 {
        app.loop_t += 16;
        bridge::frame(&mut app.pages, &mut app.bridge,
            plx_machine::machine::Tick { ms: app.loop_t, dt_us: 16_000 }, Vec::new());
    }
    app.ott.as_mut().unwrap().reply = app.pages.top_page();
    let playing = app.ott.as_ref().unwrap().playing.clone().unwrap();
    let ids = app.ott.as_ref().unwrap().ordered_ids();
    let selected = ids[0].clone();
    assert_ne!(selected, playing.selection_id);
    let ticket = plx_media::route::play_request_ticket();
    let play_generation = app.ott.as_ref().unwrap().play_generation;
    open_at(&mut app, &selected);
    let runtime = app.ott.as_ref().unwrap();
    let saved = projected_candidate(runtime, Operation::MoveMember {
        category: source_category(runtime), item: selected.clone(), direction: Direction::Up,
    });
    let navigation = runtime.navigation_epoch;
    perform(&mut app, "up");
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.navigation_epoch, navigation + 1);
    assert_eq!(runtime.organization.order_pending.as_ref().unwrap().menu.context.navigation,
        runtime.navigation_epoch, "only the admitted operation rebases its pending ACK");
    writer(&app);
    assert_eq!(app.ott.as_ref().unwrap().ordered_ids(), ids);
    settle(&mut app, Ok(saved));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.ordered_ids(), vec![ids[1].clone(), ids[2].clone(), ids[0].clone()]);
    assert_eq!(browse_focus::selected(runtime, &runtime.raw_view().rows), Some(selected));
    assert!(runtime.playing.as_ref().unwrap().identity == playing.identity);
    assert_eq!(runtime.playing.as_ref().unwrap().selection_id, playing.selection_id);
    assert_eq!(runtime.play_generation, play_generation);
    assert!(plx_media::route::play_request_ticket() == ticket);
}

#[test]
fn legacy_open_is_list_only_even_for_singleton_and_empty_saved_categories() {
    let _serial = plx_base::testlock::serial();
    for count in [0, 1] {
        let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
        let runtime = app.ott.as_mut().unwrap();
        runtime.catalog.entries.truncate(count);
        runtime.saved.category.clear();
        runtime.catalog_revision += 1;
        runtime.sync_saved_context();
        open(&mut app);
        assert!(has_row(&app, "order.sort"));
        assert!(!has_row(&app, "order.up"));
        assert!(!has_row(&app, "order.remove"));
        assert!(!has_row(&app, "order.add"));
        assert!(invoke(&mut app, "remove").is_err());
        assert!(!app.ott.as_ref().unwrap().saving);
    }
}

#[test]
fn contextual_open_rejects_nonmember_and_stale_owner_without_a_writer() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
    let outside = app.ott.as_ref().unwrap().organization_rows("Other")[0].1.id.clone();
    let context = app.ott.as_ref().unwrap().local_order_key().unwrap();
    assert!(action(&mut app, OttAction::ShowActionsAt { context, id: outside }).is_err());
    assert_eq!(app.ott.as_ref().unwrap().page, "channels");
    let selected = app.ott.as_ref().unwrap().ordered_ids()[0].clone();
    open_at(&mut app, &selected);
    app.ott.as_mut().unwrap().cancel = Arc::new(AtomicBool::new(false));
    assert!(invoke(&mut app, "remove").is_err());
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.saving);
    assert!(runtime.rx.try_recv().is_err());
}

#[test]
fn category_picker_holds_original_item_excludes_all_and_retires_old_nonce() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
    let ids = app.ott.as_ref().unwrap().ordered_ids();
    let revision = open_at(&mut app, &ids[0]);
    perform(&mut app, "add");
    let runtime = app.ott.as_ref().unwrap();
    let view = runtime.local_order_view();
    assert!(!view.rows.iter().any(|row| row.id == format!("order.category:{}", catalog_organization::ALL_ID)));
    let destination = runtime.organization_projection().unwrap().selection_id("Other").unwrap().to_owned();
    let target = view.rows.iter().find(|row| row.id == format!("order.category:{destination}")).unwrap().action.clone();
    let saved = projected_candidate(runtime, Operation::AddMember { category: destination.clone(), item: ids[0].clone() });
    assert!(action(&mut app, OttAction::EditSetting(format!("order.{revision}.remove"))).is_err());
    // An unrelated current selection change must not substitute this item.
    app.ott.as_mut().unwrap().selected = Some(ids[1].clone());
    dispatch(&mut app, target);
    writer(&app);
    settle(&mut app, Ok(saved));
    let runtime = app.ott.as_ref().unwrap();
    let projection = runtime.organization_projection().unwrap();
    let members = &projection.category(&destination).unwrap().members;
    assert!(members.contains(&ids[0]));
    assert!(!members.contains(&ids[1]));
    assert_eq!(runtime.ordered_ids(), ids);
    assert_eq!(browse_focus::selected(runtime, &runtime.raw_view().rows), Some(ids[0].clone()));
}

#[test]
fn remove_changes_only_membership_and_focus_moves_to_next_survivor_after_ack() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
    let before = app.ott.as_ref().unwrap().saved.clone();
    let raw = app.ott.as_ref().unwrap().catalog.clone();
    let ids = app.ott.as_ref().unwrap().ordered_ids();
    open_at(&mut app, &ids[1]);
    let runtime = app.ott.as_ref().unwrap();
    let saved = projected_candidate(runtime, Operation::RemoveMember {
        category: source_category(runtime), item: ids[1].clone(),
    });
    perform(&mut app, "remove");
    writer(&app);
    assert_eq!(app.ott.as_ref().unwrap().ordered_ids(), ids);
    settle(&mut app, Ok(saved));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.ordered_ids(), vec![ids[0].clone(), ids[2].clone()]);
    assert_eq!(runtime.catalog, raw);
    assert_eq!(runtime.saved.profiles, before.profiles);
    assert_eq!(runtime.saved.favorites, before.favorites);
    assert_eq!(browse_focus::selected(runtime, &runtime.raw_view().rows), Some(ids[2].clone()));
}

#[test]
fn failed_context_write_is_retryable_and_late_success_cannot_publish_focus() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
    let ids = app.ott.as_ref().unwrap().ordered_ids();
    open_at(&mut app, &ids[0]);
    perform(&mut app, "down");
    writer(&app);
    settle(&mut app, Err("Fixture writer rejected"));
    assert_eq!(app.ott.as_ref().unwrap().ordered_ids(), ids);
    assert!(has_row(&app, "order.down"));
    let runtime = app.ott.as_ref().unwrap();
    let saved = projected_candidate(runtime, Operation::MoveMember {
        category: source_category(runtime), item: ids[0].clone(), direction: Direction::Down,
    });
    perform(&mut app, "down");
    writer(&app);
    let runtime = app.ott.as_mut().unwrap();
    runtime.page = "profiles";
    runtime.navigation_epoch += 1;
    settle(&mut app, Ok(saved));
    assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
    app.ott.as_mut().unwrap().page = "channels";
    let runtime = app.ott.as_ref().unwrap();
    assert_ne!(browse_focus::selected(runtime, &runtime.raw_view().rows), Some(ids[0].clone()));
}

#[test]
fn alphabetical_toggle_uses_the_captured_source_and_shared_saved_ack() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
    let ids = app.ott.as_ref().unwrap().ordered_ids();
    open_at(&mut app, &ids[0]);
    let runtime = app.ott.as_ref().unwrap();
    let source = Source::new(&runtime.organization.order_menu.as_ref().unwrap().context.profile).unwrap();
    let saved = projected_candidate(runtime, Operation::SetAlphabetical { enabled: true });
    perform(&mut app, "sort");
    writer(&app);
    assert!(!Organization::read(&app.ott.as_ref().unwrap().saved.settings).unwrap().alphabetical(&source));
    settle(&mut app, Ok(saved));
    assert!(Organization::read(&app.ott.as_ref().unwrap().saved.settings).unwrap().alphabetical(&source));
    assert_eq!(app.ott.as_ref().unwrap().saved.category, "Group");
}

#[test]
fn local_context_depends_on_durable_profile_and_picker_back_keeps_the_item() {
    let _serial = plx_base::testlock::serial();
    for permanent in [false, true] {
        let (_directory, mut app) = fixture(permanent);
        let id = app.ott.as_ref().unwrap().ordered_ids()[0].clone();
        open_at(&mut app, &id);
        // All still refuses contextual removal/move. Add and source sorting
        // require the permanent exact Saved profile, not merely a Local grant.
        for operation in ["up", "down", "remove"] {
            assert!(!has_row(&app, &format!("order.{operation}")));
            assert!(invoke(&mut app, operation).is_err());
        }
        for operation in ["add", "sort"] {
            assert_eq!(has_row(&app, &format!("order.{operation}")), permanent);
            if !permanent { assert!(invoke(&mut app, operation).is_err()); }
        }
        assert!(has_row(&app, "order.shuffle"));
    }
    let (_other, mut app) = saved_provider_fixture(ProviderKind::M3u);
    let id = app.ott.as_ref().unwrap().ordered_ids()[0].clone();
    open_at(&mut app, &id);
    perform(&mut app, "add");
    dispatch(&mut app, OttAction::Back);
    assert_eq!(app.ott.as_ref().unwrap().page, "actions");
    assert!(has_row(&app, "order.remove"));
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn stale_or_pending_category_picker_can_close_without_applying_or_reopening() {
    let _serial = plx_base::testlock::serial();
    for pending in [false, true] {
        let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
        let id = app.ott.as_ref().unwrap().ordered_ids()[0].clone();
        open_at(&mut app, &id);
        perform(&mut app, "add");
        let candidate = if pending {
            let runtime = app.ott.as_ref().unwrap();
            let destination = runtime.organization_projection().unwrap().selection_id("Other").unwrap().to_owned();
            let action = runtime.local_order_view().rows.iter()
                .find(|row| row.id == format!("order.category:{destination}")).unwrap().action.clone();
            let candidate = projected_candidate(runtime, Operation::AddMember { category: destination, item: id.clone() });
            dispatch(&mut app, action);
            writer(&app);
            Some(candidate)
        } else {
            app.ott.as_mut().unwrap().cancel = Arc::new(AtomicBool::new(false));
            None
        };
        dispatch(&mut app, OttAction::Back);
        assert_eq!(app.ott.as_ref().unwrap().page, "channels");
        assert!(app.ott.as_ref().unwrap().organization.order_menu.is_none());
        if let Some(candidate) = candidate { settle(&mut app, Ok(candidate)); }
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.page, "channels");
        assert_ne!(browse_focus::selected(runtime, &runtime.raw_view().rows), Some(id));
    }
}

#[test]
fn alphabetical_mode_refuses_manual_moves_until_as_is_is_acknowledged() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
    let id = app.ott.as_ref().unwrap().ordered_ids()[0].clone();
    open_at(&mut app, &id);
    let saved = projected_candidate(app.ott.as_ref().unwrap(), Operation::SetAlphabetical { enabled: true });
    perform(&mut app, "sort");
    writer(&app);
    settle(&mut app, Ok(saved));
    open_at(&mut app, &id);
    let original = app.ott.as_ref().unwrap().saved.settings.clone();
    for operation in ["up", "down"] {
        assert!(!has_row(&app, &format!("order.{operation}")));
        assert!(invoke(&mut app, operation).is_err());
        assert_eq!(app.ott.as_ref().unwrap().saved.settings, original);
        assert!(!app.ott.as_ref().unwrap().saving);
    }
    let saved = projected_candidate(app.ott.as_ref().unwrap(), Operation::SetAlphabetical { enabled: false });
    perform(&mut app, "sort");
    writer(&app);
    settle(&mut app, Ok(saved));
    open_at(&mut app, &id);
    assert!(has_row(&app, "order.up"));
    assert!(has_row(&app, "order.down"));
}

#[test]
fn captured_open_key_is_not_reusable_after_source_authority_aba() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
    let runtime = app.ott.as_mut().unwrap();
    let context = runtime.local_order_key().unwrap();
    let id = runtime.ordered_ids()[0].clone();
    let profile = runtime.saved.profiles.profiles[0].clone();
    runtime.saved.profiles.profiles[0].name = "Other source view".into();
    runtime.sync_saved_context();
    runtime.saved.profiles.profiles[0] = profile;
    runtime.sync_saved_context();
    assert_ne!(runtime.local_order_key().as_ref(), Some(&context));
    assert!(action(&mut app, OttAction::ShowActionsAt { context, id }).is_err());
    assert_eq!(app.ott.as_ref().unwrap().page, "channels");
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn all_view_offers_add_and_sort_but_cannot_remove_or_move_members() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
    app.ott.as_mut().unwrap().saved.category.clear();
    let id = app.ott.as_ref().unwrap().ordered_ids()[0].clone();
    open_at(&mut app, &id);
    assert!(has_row(&app, "order.add"));
    assert!(has_row(&app, "order.sort"));
    for operation in ["up", "down", "remove"] {
        assert!(!has_row(&app, &format!("order.{operation}")));
        assert!(invoke(&mut app, operation).is_err());
    }
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn idempotent_add_returns_to_the_captured_item_without_an_organization_write() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = saved_provider_fixture(ProviderKind::M3u);
    let id = app.ott.as_ref().unwrap().ordered_ids()[0].clone();
    open_at(&mut app, &id);
    let category = source_category(app.ott.as_ref().unwrap());
    perform(&mut app, "add");
    let runtime = app.ott.as_ref().unwrap();
    let row = runtime.local_order_view().rows.into_iter()
        .find(|row| row.id == format!("order.category:{category}")).unwrap();
    assert!(!row.enabled, "existing membership is not offered as a change");
    // A still-current duplicate action remains an idempotent no-op.
    dispatch(&mut app, row.action);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.page, "channels");
    assert!(!runtime.saving);
    assert!(runtime.rx.try_recv().is_err());
    assert_eq!(browse_focus::selected(runtime, &runtime.raw_view().rows), Some(id));
}

fn permanent_local_categories() -> (local_integration::TempDir, Box<App>, String, String) {
    let (directory, mut app) = fixture(true);
    let runtime = app.ott.as_mut().unwrap();
    let source = Source::new(runtime.effective_profile().unwrap()).unwrap();
    let mut document = Organization::read(&runtime.saved.settings).unwrap();
    let category = document.apply(&source, &runtime.catalog,
        Operation::Create { label: "Logical A".into() }).unwrap().category.unwrap();
    let destination = document.apply(&source, &runtime.catalog,
        Operation::Create { label: "Logical B".into() }).unwrap().category.unwrap();
    for entry in &runtime.catalog.entries {
        document.apply(&source, &runtime.catalog,
            Operation::AddMember { category: category.clone(), item: entry.id.clone() }).unwrap();
    }
    document.write_into(&mut runtime.saved.settings).unwrap();
    runtime.saved.category = category.clone();
    runtime.sync_saved_context();
    (directory, app, category, destination)
}

#[test]
fn permanent_local_logical_operations_preserve_files_grant_provider_and_raw_catalog() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app, category, destination) = permanent_local_categories();
    let runtime = app.ott.as_ref().unwrap();
    let before = runtime.saved.clone();
    let provider = runtime.provider;
    let catalog = runtime.catalog.clone();
    let grant = runtime.local_inventory.sources[0].root_handle().unwrap().clone();
    let path = std::path::PathBuf::from(runtime.local_inventory.sources[0].descriptor().canonical_path.as_ref().unwrap());
    // One-byte file fixtures match the existing synthetic catalogue. They are
    // not playable media and no decoder is exercised by these logical edits.
    let files: Vec<_> = catalog.entries.iter().map(|entry| {
        let MediaLocator::Local { relative_path } = &entry.media else { panic!("Local fixture"); };
        let file = path.join(relative_path);
        std::fs::write(&file, b"x").unwrap();
        file
    }).collect();
    let ids = runtime.ordered_ids();
    let ticket = plx_media::route::play_request_ticket();
    for (action, operation) in [
        ("up", Operation::MoveMember { category: category.clone(), item: ids[0].clone(), direction: Direction::Up }),
        ("remove", Operation::RemoveMember { category: category.clone(), item: ids[0].clone() }),
    ] {
        open_at(&mut app, &ids[0]);
        let saved = projected_candidate(app.ott.as_ref().unwrap(), operation);
        perform(&mut app, action);
        writer(&app);
        settle(&mut app, Ok(saved));
    }
    assert!(!app.ott.as_ref().unwrap().ordered_ids().contains(&ids[0]));
    open_at(&mut app, &ids[1]);
    perform(&mut app, "add");
    let runtime = app.ott.as_ref().unwrap();
    let action = runtime.local_order_view().rows.into_iter()
        .find(|row| row.id == format!("order.category:{destination}")).unwrap().action;
    let saved = projected_candidate(runtime, Operation::AddMember { category: destination.clone(), item: ids[1].clone() });
    dispatch(&mut app, action);
    writer(&app);
    settle(&mut app, Ok(saved));
    open_at(&mut app, &ids[1]);
    let saved = projected_candidate(app.ott.as_ref().unwrap(), Operation::SetAlphabetical { enabled: true });
    perform(&mut app, "sort");
    writer(&app);
    settle(&mut app, Ok(saved));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.organization_projection().unwrap().category(&destination).unwrap().members, vec![ids[1].clone()]);
    assert_eq!(runtime.catalog, catalog);
    assert_eq!(runtime.saved.profiles, before.profiles);
    assert_eq!(runtime.saved.favorites, before.favorites);
    assert_eq!(runtime.provider, provider);
    assert!(runtime.transient.accepted.is_none());
    assert!(Arc::ptr_eq(runtime.local_inventory.sources[0].root_handle().unwrap(), &grant));
    assert!(grant.is_live());
    for file in &files { assert_eq!(std::fs::read(file).unwrap(), b"x"); }
    assert_eq!(std::fs::read_dir(&path).unwrap().count(), files.len());
    assert!(plx_media::route::play_request_ticket() == ticket);
}

#[test]
fn local_context_requires_one_exact_saved_profile_and_supports_empty_or_singleton() {
    let _serial = plx_base::testlock::serial();
    for count in [0, 1] {
        let (_directory, mut app) = fixture(true);
        let runtime = app.ott.as_mut().unwrap();
        runtime.catalog.entries.truncate(count);
        runtime.catalog_revision += 1;
        open(&mut app);
        assert!(has_row(&app, "order.sort"));
    }
    for duplicate in [false, true] {
        let (_directory, mut app) = fixture(true);
        let runtime = app.ott.as_mut().unwrap();
        let profile = runtime.effective_profile().unwrap().clone();
        if duplicate { runtime.saved.profiles.profiles.push(profile); }
        else { runtime.saved.profiles.profiles.retain(|profile| profile.kind != ProviderKind::Local); }
        if runtime.local_order_key().is_some() {
            let id = runtime.ordered_ids()[0].clone();
            open_at(&mut app, &id);
            assert!(!has_row(&app, "order.sort"));
            assert!(!has_row(&app, "order.add"));
            assert!(invoke(&mut app, "sort").is_err());
        }
        assert!(!app.ott.as_ref().unwrap().saving);
    }
}

fn invalidate_local_context(runtime: &mut Runtime, case: usize) {
    match case {
        0 => runtime.local_inventory.sources[0].withdraw(),
        1 => {
            let descriptor = runtime.local_inventory.sources[0].descriptor().clone();
            let replacement = crate::app::ott_local_root::LocalSource::acquire_authorized(
                &plx_base::task::OffFrame::for_test(), descriptor, &mut || true).unwrap();
            runtime.local_inventory.install(vec![replacement], runtime.generation);
        }
        2 => runtime.cancel = Arc::new(AtomicBool::new(false)),
        _ => {
            let at = runtime.saved.profiles.profiles.iter().position(|profile| profile.kind == ProviderKind::Local).unwrap();
            let profile = runtime.saved.profiles.profiles[at].clone();
            runtime.saved.profiles.profiles[at].name = "Other local profile".into();
            runtime.sync_saved_context();
            runtime.saved.profiles.profiles[at] = profile;
            runtime.sync_saved_context();
        }
    }
}

#[test]
fn local_grant_cancel_and_saved_aba_fence_context_before_write_and_after_ack() {
    let _serial = plx_base::testlock::serial();
    for case in 0..4 {
        for pending in [false, true] {
            let (_directory, mut app, category, _) = permanent_local_categories();
            let id = app.ott.as_ref().unwrap().ordered_ids()[0].clone();
            open_at(&mut app, &id);
            let saved = projected_candidate(app.ott.as_ref().unwrap(), Operation::RemoveMember { category, item: id.clone() });
            if pending { perform(&mut app, "remove"); writer(&app); }
            invalidate_local_context(app.ott.as_mut().unwrap(), case);
            if pending {
                settle(&mut app, Ok(saved));
                assert_eq!(app.ott.as_ref().unwrap().page, "actions");
                assert!(app.ott.as_ref().unwrap().organization.order_pending.is_none());
            } else {
                assert!(invoke(&mut app, "remove").is_err());
                assert!(!app.ott.as_ref().unwrap().saving);
                assert!(app.ott.as_ref().unwrap().rx.try_recv().is_err());
            }
        }
    }
}
