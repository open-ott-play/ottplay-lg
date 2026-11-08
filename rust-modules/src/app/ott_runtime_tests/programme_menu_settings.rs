//! The real More row opens grouped Settings above the same retained Menu.
use super::*;

fn prepare(app: &mut App) {
    let runtime = app.ott.as_mut().unwrap();
    runtime.write_permit = private::WritePermit::new();
    runtime.public_preferences.accept(None);
    runtime.entropy = Some([7; 16]);
    runtime.last_checkpoint_ms = runtime.now_ms();
    // The paused fixture already sampled this position. Keep periodic resume
    // persistence idle while the Settings tests exercise real writer readiness.
    runtime.saved.resume.extend(runtime.checkpoints.clone());
}

fn choose_settings(app: &mut App) {
    assert!(more_keys(app).contains(&36), "the actual Menu must offer Settings");
    for _ in 0..40 {
        let overlay = bridge::player_overlay_mut(&mut app.pages).unwrap();
        let Panel::More(menu) = overlay.panel() else { panic!("More panel") };
        if menu.keys().get(menu.sel() as usize) == Some(&36) { break; }
        key(app, Key::Down);
    }
    let overlay = bridge::player_overlay_mut(&mut app.pages).unwrap();
    let Panel::More(menu) = overlay.panel() else { panic!("More panel") };
    assert_eq!(menu.keys().get(menu.sel() as usize), Some(&36));
    key(app, Key::Ok);
    assert!(ott_surface(app).is_some(), "Settings uses the existing OTT surface");
}

fn action_ending(app: &App, suffix: &str) -> OttAction {
    app.ott.as_ref().unwrap().view().rows.into_iter()
        .find(|row| row.id.ends_with(suffix)).expect("visible grouped action").action
}

#[test]
fn menu_settings_group_cancel_save_back_return_to_same_menu_and_selection() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [plx_machine::machine::UiMode::Classic, plx_machine::machine::UiMode::Modern] {
        for exit in ["cancel", "save", "back"] {
            let mut app = catalog_app();
            prepare(&mut app);
            app.bridge.apply_ui_mode(mode);
            let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
            let identity = route::playback_identity(&app.player.session);
            let epoch = app.ott.as_ref().unwrap().playback_epoch;
            let position = player::SHARED.playpos_ns.load(Ordering::Acquire);
            open_more(&mut app);
            let menu_owner = input_owner(&app);
            choose_settings(&mut app);
            let group = action_ending(&app, "settings.open:lists");
            surface_action(&mut app, group);
            // No staged changes: Save exercises the real grouped no-write branch.
            match exit {
                "cancel" => { let action = action_ending(&app, ":back"); surface_action(&mut app, action); }
                "save" => { let action = action_ending(&app, ":save"); surface_action(&mut app, action); }
                _ => key(&mut app, Key::Back),
            }
            assert_eq!(app.ott.as_ref().unwrap().view().selected_row.as_deref(), Some("settings.open:lists"));
            key(&mut app, Key::Back);
            assert_eq!(input_owner(&app), menu_owner, "same retained Menu: {mode:?}/{exit}");
            let overlay = bridge::player_overlay_mut(&mut app.pages).unwrap();
            let Panel::More(menu) = overlay.panel() else { panic!("More retained") };
            assert_eq!(menu.keys().get(menu.sel() as usize), Some(&36));
            choose_settings(&mut app);
            // Reopening restores the selected stable group, not its old numeric index.
            key(&mut app, Key::Ok);
            assert_eq!(app.ott.as_ref().unwrap().view().title, "Lists");
            key(&mut app, Key::Back);
            key(&mut app, Key::Back);
            assert_eq!(input_owner(&app), menu_owner);
            let runtime = app.ott.as_ref().unwrap();
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
            assert_eq!(runtime.playback_epoch, epoch);
            assert_eq!(route::playback_identity(&app.player.session), identity);
            assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), position);
            assert!(!runtime.saving && runtime.play_target.is_none());
            assert!(app.adapters.player.is_live());
        }
    }
}

#[test]
fn native_menu_settings_does_not_load_a_catalogue_or_adopt_playback() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let (mut app, _) = native_catalog_app();
    prepare(&mut app);
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let generation = app.ott.as_ref().unwrap().generation;
    let identity = route::playback_identity(&app.player.session);
    open_more(&mut app);
    let menu = input_owner(&app);
    choose_settings(&mut app);
    assert!(app.ott.as_ref().unwrap().view().rows.iter().any(|row| row.id == "settings.open:lists"));
    key(&mut app, Key::Back);
    assert_eq!(input_owner(&app), menu);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.generation, generation);
    assert!(runtime.playing.is_none() && runtime.catalog_next.is_none() && runtime.play_target.is_none());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert_eq!(route::playback_identity(&app.player.session), identity);
}

#[test]
fn protected_menu_settings_opens_real_pin_without_publishing_protected_rows() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = catalog_app();
    prepare(&mut app);
    app.ott.as_mut().unwrap().saved.settings.insert("parentPin".into(), json!(
        "pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
    app.ott.as_mut().unwrap().saved.settings.insert("psOptions".into(), json!(1));
    open_more(&mut app);
    let menu = input_owner(&app);
    choose_settings(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    let editor = runtime.edit_session.as_ref().expect("existing PIN editor opens after real Refresh");
    assert_eq!(editor.key, "parental.unlock");
    assert_eq!(Some(editor.owner), crate::app::ott_runtime::current_ui_owner(&app));
    assert_eq!(runtime.view().rows.len(), 2);
    assert!(runtime.view().rows.iter().all(|row| !row.id.starts_with("settings.open:")));
    assert!(bridge::ott_private_text(&app.pages));
    key(&mut app, Key::Back); // PIN Cancel keeps only the neutral access panel.
    assert!(app.ott.as_ref().unwrap().edit_session.is_none());
    assert!(app.ott.as_ref().unwrap().view().rows.iter().all(|row| !row.id.starts_with("settings.open:")));
    key(&mut app, Key::Back);
    assert_eq!(input_owner(&app), menu);
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn replaced_native_source_retires_only_its_menu_settings_pair() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let (mut app, sid) = native_catalog_app();
    prepare(&mut app);
    open_more(&mut app);
    let menu = input_owner(&app);
    choose_settings(&mut app);
    let panel = input_owner(&app);
    bridge::open_player_overlay(&app.player.session, app.bridge.metadata_view(),
        &mut app.pages, OverlayKind::Info);
    idle(&mut app);
    let foreign = input_owner(&app);
    route::install_plex_playback_for_test(&mut app.player.session, sid, "43");
    idle(&mut app);
    retire_closing_surfaces(&mut app);
    assert_eq!(input_owner(&app), foreign, "an unrelated top surface is not dismissed");
    assert!(app.pages.nav.entry_of_instance(menu).is_none());
    assert!(app.pages.nav.entry_of_instance(panel).is_none());
    assert!(app.ott.as_ref().unwrap().playing.is_none());
    assert_eq!(route::cur_rk(&app.player.session), "43");
}

#[test]
fn settings_pending_mount_rejects_replacement_without_exposing_groups() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let (mut app, sid) = native_catalog_app();
    prepare(&mut app);
    open_more(&mut app);
    let menu = input_owner(&app);
    let request = plx_screens::registry::PlayerCatalogRequest {
        owner: menu, source: route::playback_identity(&app.player.session).unwrap(),
    };
    crate::app::ott_runtime::open_player_settings(&mut app, request).unwrap();
    assert!(app.pages.has_pending_surface_navigation());
    assert!(app.ott.as_ref().unwrap().view().rows.iter().all(|row| !row.id.starts_with("settings.open:")));
    route::install_plex_playback_for_test(&mut app.player.session, sid, "43");
    // Observe the source change before the queued Present reaches NAV COMMIT.
    runtime_tick(&mut app);
    assert!(app.ott.as_ref().unwrap().view().rows.iter().all(|row| !row.id.starts_with("settings.open:")));
    idle(&mut app);
    retire_closing_surfaces(&mut app);
    assert!(app.pages.nav.entry_of_instance(menu).is_none());
    assert!(ott_surface(&app).is_none());
    assert!(app.ott.as_ref().unwrap().edit_session.is_none());
    assert!(app.ott.as_ref().unwrap().playing.is_none());
    assert_eq!(route::cur_rk(&app.player.session), "43");
}

#[test]
fn stale_or_covered_more_settings_request_cannot_open_or_change_source() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    for stale_source in [false, true] {
        let (mut app, sid) = native_catalog_app();
        prepare(&mut app);
        open_more(&mut app);
        let request = plx_screens::registry::PlayerCatalogRequest {
            owner: input_owner(&app), source: route::playback_identity(&app.player.session).unwrap(),
        };
        if stale_source {
            route::install_plex_playback_for_test(&mut app.player.session, sid, "43");
        } else {
            bridge::open_player_overlay(&app.player.session, app.bridge.metadata_view(),
                &mut app.pages, OverlayKind::Info);
            idle(&mut app);
        }
        let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        let identity = route::playback_identity(&app.player.session);
        assert!(crate::app::ott_runtime::open_player_settings(&mut app, request).is_err());
        assert!(ott_surface(&app).is_none());
        assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), saved);
        assert_eq!(route::playback_identity(&app.player.session), identity);
    }
}

#[test]
fn grouped_pending_write_and_failure_keep_the_retained_menu_offer() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = catalog_app();
    prepare(&mut app);
    open_more(&mut app);
    let menu = input_owner(&app);
    let identity = route::playback_identity(&app.player.session).unwrap();
    choose_settings(&mut app);
    let group = action_ending(&app, "settings.open:lists");
    surface_action(&mut app, group);
    let pick = action_ending(&app, ":pick:pageSize");
    surface_action(&mut app, pick);
    let choice = action_ending(&app, ":choose:pageSize:10");
    surface_action(&mut app, choice);
    // Model an already admitted writer while this draft remains uncommitted.
    // No actual persistence worker or filesystem write is started by this test.
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saving = true;
        runtime.save_navigation = SaveNavigation::Preserve;
        runtime.save_playback_epoch = runtime.playback_epoch;
    }
    idle(&mut app);
    assert_eq!(app.bridge.ott_catalog_offer(), Some(&identity));
    let save = action_ending(&app, ":save");
    surface_action(&mut app, save); // Busy Save is refused without losing the draft.
    assert_eq!(app.ott.as_ref().unwrap().view().title, "Lists");
    app.ott.as_ref().unwrap().tx.send(Job::Saved(Err("fixture write rejected"), None, false,
        json!({"status":"rejected"}))).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    idle(&mut app);
    assert!(!app.ott.as_ref().unwrap().saving);
    assert_eq!(app.bridge.ott_catalog_offer(), Some(&identity));
    assert_eq!(app.ott.as_ref().unwrap().view().title, "Lists");
    key(&mut app, Key::Back);
    key(&mut app, Key::Back);
    assert_eq!(input_owner(&app), menu);
    assert!(more_keys(&mut app).contains(&36));
}

#[test]
fn more_settings_language_returns_to_its_group_then_the_same_menu() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = catalog_app();
    prepare(&mut app);
    app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Classic);
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let identity = route::playback_identity(&app.player.session);
    open_more(&mut app);
    let menu = input_owner(&app);
    choose_settings(&mut app);
    let panel = input_owner(&app);
    let group = action_ending(&app, "settings.open:interface");
    surface_action(&mut app, group);
    let pick = action_ending(&app, ":pick:uiFontScale");
    surface_action(&mut app, pick);
    let choice = action_ending(&app, ":choose:uiFontScale:150");
    surface_action(&mut app, choice);
    let generation = app.ott.as_ref().unwrap().generation;
    let navigation = app.ott.as_ref().unwrap().navigation_epoch;
    let language = action_ending(&app, ":language");
    surface_action(&mut app, language);
    let surface = app.pages.nav.modals.surfaces.iter().find(|surface|
        matches!(surface.entry.arg, AppArg::Settings(plx_screens::family::SettingsPage::Language)))
        .expect("the ordinary Language page covers the Settings surface").entry.id;
    assert_eq!(crate::app::ott_runtime::current_ui_owner(&app), None);
    assert_eq!(app.ott.as_ref().unwrap().reply, Some(panel));
    assert_eq!(app.ott.as_ref().unwrap().view().title, "Interface");
    // Exercise the actual projection refresh without publishing a global locale.
    crate::app::ott_runtime::locale_changed(&mut app);
    idle(&mut app);
    assert!(app.pages.nav.entry_of_instance(menu).is_some());
    assert!(app.pages.nav.entry_of_instance(panel).is_some());
    app.pages.request(MachineId::Nav, plx_machine::machine::NavOp::Dismiss(surface));
    idle(&mut app);
    retire_closing_surfaces(&mut app);
    assert_eq!(input_owner(&app), panel);
    let view = app.ott.as_ref().unwrap().view();
    assert_eq!(view.title, "Interface");
    assert!(view.rows.iter().any(|row| row.detail == "150%"));
    assert!(view.selected_row.as_deref().is_some_and(|id| id.ends_with(":language")));
    assert_eq!(app.ott.as_ref().unwrap().generation, generation);
    assert_eq!(app.ott.as_ref().unwrap().navigation_epoch, navigation);
    key(&mut app, Key::Back);
    key(&mut app, Key::Back);
    assert_eq!(input_owner(&app), menu);
    let overlay = bridge::player_overlay_mut(&mut app.pages).unwrap();
    let Panel::More(menu) = overlay.panel() else { panic!("same retained Menu") };
    assert_eq!(menu.keys().get(menu.sel() as usize), Some(&36));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert!(!runtime.saving && runtime.play_target.is_none());
}
