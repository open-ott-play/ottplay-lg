//! Native Plex input shares the existing Player owner and native transport.
//! Route/engine fixtures are inert; no resolver, store write or decoder is run.
use super::*;
use plx_screens::registry::{OttPlayerAction as Action, OttPlayerRequest, PlayerReq};

fn app() -> App {
    let (mut app, _) = native_catalog_app();
    bridge::player_mut(&mut app.pages).unwrap().hud.hide(false);
    player::SHARED.duration_ns.store(60_000_000_000, Ordering::Release);
    app.inputs.clear();
    app
}
fn request(app: &mut App, action: Action) -> OttPlayerRequest {
    crate::app::ott_runtime::publish_remote_player_offer(app);
    let source = app.bridge.ott_player_offer().expect("native source has an offer").source.clone();
    OttPlayerRequest { owner: app.pages.top_page().unwrap(), source, action }
}
fn perform(app: &mut App, action: Action) -> Result<Option<PlayerReq>, &'static str> {
    let req = request(app, action);
    crate::app::ott_runtime::remote_player_action(app, req)
}
fn native_key(app: &mut App, sym: u32, code: u32) {
    let tick = next_tick(app);
    let ctrl = player_hud::slot(&app.player.session, app.bridge.metadata_view());
    let mut fr = run::Frame { ctrl, now: tick.ms, dt: 0.016, underlay_moving: false,
        player: true, present: false, deferred: false, rn: "player" };
    if let Some(screen) = bridge::player_mut(&mut app.pages) { screen.slot = ctrl; }
    app.ev = crate::app::events::encode_key(sym, code, true);
    unsafe { run::ingest_sdl_event_with_window(app, &mut fr, |_| panic!("remote key must not restore a window")); }
    run::publish_playback_frame(app, true);
    let inputs = std::mem::take(&mut app.inputs);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, inputs);
    content::content_requests(app, &fr);
    runtime_tick(app);
    app.ev = [0; 128];
}

#[test]
fn native_plex_blue_opens_bound_categories_without_adopting_playback() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let mut app = app();
    let identity = route::playback_identity(&app.player.session);
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let owner = app.pages.top_page();
    let position = player::SHARED.playpos_ns.load(Ordering::Acquire);
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    native_key(&mut app, 0, 489);
    idle(&mut app);
    assert!(ott_surface(&app).is_some());
    assert_eq!(app.ott.as_ref().unwrap().page, "categories");
    assert!(app.ott.as_ref().unwrap().native_catalog.is_some());
    key(&mut app, Key::Back);
    let r = app.ott.as_ref().unwrap();
    assert!(ott_surface(&app).is_none(), "Classic Categories must close in one Back");
    assert!(!app.pages.surface_up());
    assert_eq!(Some(input_owner(&app)), owner);
    assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), position);
    assert_eq!(r.playback_epoch, epoch);
    assert!(r.playing.is_none() && r.play_target.is_none() && r.resolver_next.is_none());
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(app.pages.top_page(), owner);
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), saved);
    assert_eq!(player::state(&app.player.session), player::PlaybackState::Paused);
}

#[test]
fn native_plex_fullscreen_catalog_binds_current_row_without_restart() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let mut app = app();
    let identity = route::playback_identity(&app.player.session);
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    assert!(perform(&mut app, Action::OpenCatalog).unwrap().is_none());
    idle(&mut app);
    let (current, _) = native_catalog_rows(&mut app);
    surface_action(&mut app, OttAction::Play(current));
    retire_closing_surfaces(&mut app);
    let r = app.ott.as_ref().unwrap();
    assert!(ott_surface(&app).is_none());
    assert!(r.playing.is_none() && r.play_target.is_none() && r.resolver_next.is_none());
    assert_eq!(r.playback_epoch, epoch);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), 7_000_000_000);
}

#[test]
fn native_plex_seek_and_pause_use_existing_transport_requests() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let mut app = app();
    let identity = route::playback_identity(&app.player.session);
    assert!(matches!(perform(&mut app, Action::TogglePause).unwrap(), Some(PlayerReq::Transport(None))));
    assert!(matches!(perform(&mut app, Action::SeekForward).unwrap(), Some(PlayerReq::CommitSeek(17_000_000_000))));
    assert!(app.ott.as_ref().unwrap().playing.is_none());
    assert!(app.ott.as_ref().unwrap().catalog_next.is_none());
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(player::state(&app.player.session), player::PlaybackState::Paused);
}

#[test]
fn native_plex_remote_rejects_replacement_or_new_surface_before_effect() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    for changed in [false, true] {
        let mut app = app();
        let req = request(&mut app, Action::OpenCatalog);
        if changed {
            let sid = route::cur_sid(&app.player.session);
            route::install_plex_playback_for_test(&mut app.player.session, sid, "42");
        } else {
            bridge::open_settings(&mut app.pages);
        }
        assert!(crate::app::ott_runtime::remote_player_action(&mut app, req).is_err());
        let r = app.ott.as_ref().unwrap();
        assert!(r.catalog_next.is_none() && r.playing.is_none() && r.native_catalog.is_none());
    }
}

#[test]
fn native_plex_step_requires_a_bound_list_without_borrowing_saved_or_loading_it() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let mut app = app();
    let unrelated = synthetic_profile();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.profiles.profiles = vec![unrelated.clone()];
    runtime.saved.profiles.active_profile = Some(unrelated.id.clone());
    runtime.provider = unrelated.kind;
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let identity = route::playback_identity(&app.player.session);
    // No Classic list has been admitted. A rocker cannot guess from the
    // unrelated saved profile or populate a new catalogue as a side effect.
    for action in [Action::Previous, Action::Next] {
        assert!(perform(&mut app, action).unwrap_err().contains("Open"));
    }
    const PIN: &str = "pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65";
    app.ott.as_mut().unwrap().saved.settings.insert("parentPin".into(), json!(PIN));
    for action in [Action::Previous, Action::Next] {
        let error = perform(&mut app, action).unwrap_err();
        assert!(error.contains("Open"));
    }
    app.ott.as_mut().unwrap().saved.settings.remove("parentPin");
    let r = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), saved);
    assert!(r.catalog_next.is_none() && r.playing.is_none() && r.play_target.is_none() && r.resolver_next.is_none());
    assert_eq!(route::playback_identity(&app.player.session), identity);
}

// The published rows represent the current materialized list. Reordering it
// here tests the common step consumer, not a separate shuffle implementation.
fn list_app(order: &[u32], current: u32) -> App {
    let mut app = app();
    let sid = route::cur_sid(&app.player.session);
    route::install_plex_playback_for_test(&mut app.player.session, sid, &current.to_string());
    assert!(perform(&mut app, Action::OpenCatalog).unwrap().is_none());
    idle(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    let profile = runtime.effective_profile().unwrap().clone();
    let rows = order.iter().map(|rk| {
        let mut row = synthetic_entry();
        row.id = format!("{}:{rk}", profile.id);
        row.title = format!("Film {rk}");
        row.category = if *rk == 99 { "Other" } else { "Selected" }.into();
        row.media = MediaLocator::Plex { server_id: profile.source.clone(), rating_key: rk.to_string() };
        row
    }).collect();
    runtime.catalog_tx.send(CatalogEvent::Batch(runtime.generation,
        Catalog { entries: rows, hierarchy: None })).unwrap();
    runtime.catalog_tx.send(CatalogEvent::Finished(runtime.generation, Ok(()))).unwrap();
    frame(&mut app, vec![]);
    idle(&mut app);
    key(&mut app, Key::Back);
    assert!(ott_surface(&app).is_none());
    let runtime = app.ott.as_mut().unwrap();
    classic_source::set_view(runtime, "Selected");
    runtime.selected = Some("unrelated-highlight".into());
    assert!(runtime.playing.is_none());
    app
}

#[test]
fn native_plex_step_uses_visible_category_order_and_wrap_not_highlight_or_queue() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    for (current, action, expected) in [(42, Action::Next, 44), (42, Action::Previous, 43),
        (43, Action::Previous, 44), (44, Action::Next, 43)] {
        let mut app = list_app(&[99, 43, 42, 44], current);
        let identity = route::playback_identity(&app.player.session);
        let r = app.ott.as_mut().unwrap();
        let prefix = r.effective_profile().unwrap().id.clone();
        // The same locator outside this category is not an ambiguous visible anchor.
        let mut hidden = r.catalog.entries.iter().find(|row| row.id == format!("{prefix}:{current}")).unwrap().clone();
        hidden.id.push_str(":outside");
        hidden.category = "Other".into();
        r.catalog.entries.push(hidden);
        assert_eq!(commands::selected_ids(r), [43,42,44].map(|rk| format!("{prefix}:{rk}")));
        // No new full-scan wait: these are already the visible rows.
        r.catalog_complete = false;
        r.catalog_loading = true;
        assert!(perform(&mut app, action).unwrap().is_none());
        let r = app.ott.as_ref().unwrap();
        assert_eq!(r.play_target.as_ref().unwrap().id, format!("{prefix}:{expected}"));
        assert_eq!(r.resolver_next.as_ref().unwrap().entry.as_ref().unwrap().id, format!("{prefix}:{expected}"));
        assert!(r.playing.is_none());
        assert_eq!(route::playback_identity(&app.player.session), identity);
    }
}

#[test]
fn native_plex_step_can_enter_a_mixed_favorite_through_its_own_source_admission() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let mut app = list_app(&[42,43], 42);
    let r = app.ott.as_mut().unwrap();
    let profile = r.effective_profile().unwrap().clone();
    let current = selection::capture(&profile, &r.catalog.entries[0]).unwrap();
    let other_profile = synthetic_profile();
    let other = selection::capture(&other_profile, &synthetic_entry()).unwrap();
    let target = format!("favorite:{}", other.identity());
    r.saved.profiles.profiles.push(other_profile.clone());
    // A different, unavailable Favorite is not evidence about the current item.
    let mut missing_profile = synthetic_profile();
    missing_profile.id = "missing-provider".into();
    missing_profile.name = "Missing source".into();
    missing_profile.source = "portal::[key:missing]https://missing.invalid/nas/api".into();
    let stale = selection::capture(&missing_profile, &synthetic_entry()).unwrap();
    r.saved.favorites.toggle(stale).unwrap();
    r.saved.favorites.toggle(current).unwrap();
    r.saved.favorites.toggle(other).unwrap();
    r.saved.favorites_selected = true;
    let active = r.saved.profiles.active_profile.clone();
    let identity = route::playback_identity(&app.player.session);
    assert!(perform(&mut app, Action::Next).unwrap().is_none());
    let r = app.ott.as_ref().unwrap();
    assert_eq!(r.play_target.as_ref().unwrap().id, target);
    assert_eq!(r.play_profile.as_ref().map(|(_, profile)| profile), Some(&other_profile));
    assert!(r.resolver_next.as_ref().unwrap().favorite.is_some());
    assert!(r.playing.is_none());
    assert_eq!(r.saved.profiles.active_profile, active);
    assert_eq!(route::playback_identity(&app.player.session), identity);
}

#[test]
fn native_plex_list_step_refuses_missing_ambiguous_or_stale_current_binding() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    for changed in ["category", "removed", "duplicate", "generation"] {
        let mut app = list_app(&[99,42,43], 42);
        let r = app.ott.as_mut().unwrap();
        let catalog_pending = r.catalog_next.is_some();
        match changed {
            "category" => classic_source::set_view(r, "Other"),
            "removed" => { r.catalog.entries.retain(|row| !row.id.ends_with(":42")); },
            "duplicate" => {
                let mut row = r.catalog.entries.iter().find(|row| row.id.ends_with(":42")).unwrap().clone(); row.id.push_str(":duplicate");
                r.catalog.entries.push(row);
            }
            "generation" => r.generation = r.generation.wrapping_add(1),
            _ => unreachable!(),
        }
        let identity = route::playback_identity(&app.player.session);
        assert!(perform(&mut app, Action::Next).is_err(), "{changed}");
        let r = app.ott.as_ref().unwrap();
        assert!(r.playing.is_none() && r.play_target.is_none() && r.resolver_next.is_none(), "{changed}");
        assert_eq!(r.catalog_next.is_some(), catalog_pending, "must not start an unrelated catalogue");
        assert_eq!(route::playback_identity(&app.player.session), identity);
    }
}

#[test]
fn native_plex_single_visible_item_never_restarts_on_either_direction() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let mut app = list_app(&[42], 42);
    let identity = route::playback_identity(&app.player.session);
    let epoch = app.ott.as_ref().unwrap().playback_epoch;
    for action in [Action::Previous, Action::Next] {
        assert!(perform(&mut app, action).unwrap().is_none());
    }
    let r = app.ott.as_ref().unwrap();
    assert!(r.playing.is_none() && r.play_target.is_none() && r.resolver_next.is_none());
    assert_eq!(r.playback_epoch, epoch);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), 7_000_000_000);
}

#[test]
fn native_plex_list_step_never_skips_an_unavailable_or_pin_protected_target() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    for protected in [false, true] {
        let mut app = list_app(&[42,43], 42);
        let r = app.ott.as_mut().unwrap();
        let profile = r.effective_profile().unwrap().clone();
        let current = selection::capture(&profile, &r.catalog.entries[0]).unwrap();
        let following = selection::capture(&profile, &r.catalog.entries[1]).unwrap();
        let target_profile = synthetic_profile();
        let target = selection::capture(&target_profile, &synthetic_entry()).unwrap();
        if protected {
            r.saved.profiles.profiles.push(target_profile);
            r.saved.settings.insert("parentPin".into(), json!("pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
        }
        r.saved.favorites.toggle(current).unwrap();
        r.saved.favorites.toggle(target).unwrap();
        r.saved.favorites.toggle(following).unwrap();
        r.saved.favorites_selected = true;
        let identity = route::playback_identity(&app.player.session);
        assert!(perform(&mut app, Action::Next).is_err());
        let r = app.ott.as_ref().unwrap();
        assert!(r.playing.is_none() && r.play_target.is_none() && r.resolver_next.is_none());
        assert_eq!(route::playback_identity(&app.player.session), identity);
    }
}

#[cfg(target_os = "macos")]
#[path = "programme_native_desktop.rs"]
mod physical_desktop;
