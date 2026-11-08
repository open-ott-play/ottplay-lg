//! Physical Mac input against the real native Plex catalogue fixture.
//! Only admission is exercised: installed decoder and queue ownership stay native.
use super::*;
use plx_ui::consts::{SDLK_DOWN, SDLK_UP};

fn physical_edge(app: &mut App, sym: u32, down: bool, repeat: bool) {
    app.ev = [0; 128];
    app.ev[..4].copy_from_slice(&(if down { 0x300_u32 } else { 0x301_u32 }).to_ne_bytes());
    app.ev[8..12].copy_from_slice(&1_u32.to_ne_bytes());
    app.ev[12] = u8::from(down);
    app.ev[13] = u8::from(repeat);
    app.ev[20..24].copy_from_slice(&sym.to_ne_bytes());
    let ctrl = player_hud::slot(&app.player.session, app.bridge.metadata_view());
    let mut frame = run::Frame { ctrl, now: clock::now(), dt: 0.016,
        underlay_moving: false, player: true, present: false,
        deferred: false, rn: "player" };
    unsafe {
        run::ingest_sdl_event_with_window(app, &mut frame,
            |_| panic!("physical input cannot restore a window"));
    }
}

fn bare_native(app: &mut App) {
    player::TX.commit_paused(false);
    player::swap_state_for_test(player::PlaybackState::Playing);
    bridge::player_mut(&mut app.pages).unwrap().hud.hide(false);
    app.inputs.clear();
    assert!(app.ott.as_ref().unwrap().playing.is_none());
    assert!(player_browse::native_identity(app.ott.as_ref().unwrap(), app).is_some());
}

#[test]
fn physical_native_plex_arrows_use_the_bound_category_order_without_adopting_playback() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    for (sym, expected) in [(SDLK_UP, 44), (SDLK_DOWN, 43)] {
        let mut app = list_app(&[99, 43, 42, 44], 42);
        bare_native(&mut app);
        let prefix = app.ott.as_ref().unwrap().effective_profile().unwrap().id.clone();
        let identity = route::playback_identity(&app.player.session);
        let source = route::playback_request_identity(&app.player.session)
            .map(|(sid, rk)| (sid, rk.to_owned()));
        let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        physical_edge(&mut app, sym, true, false);
        let epoch = app.ott.as_ref().unwrap().playback_epoch;
        physical_edge(&mut app, sym, true, true);
        physical_edge(&mut app, sym, false, false);
        let runtime = app.ott.as_ref().unwrap();
        let target = runtime.play_target.as_ref().expect("physical arrow admits one list item");
        assert_eq!(target.id, format!("{prefix}:{expected}"));
        assert!(!target.advance_on_failure);
        assert_eq!(runtime.playback_epoch, epoch);
        assert!(runtime.playing.is_none());
        assert_eq!(route::playback_identity(&app.player.session), identity);
        assert_eq!(route::playback_request_identity(&app.player.session)
            .map(|(sid, rk)| (sid, rk.to_owned())), source);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(runtime.resolver_busy && runtime.resolver_next.is_some());
        assert!(app.inputs.is_empty());
        assert_eq!(app.down_sym, 0);
    }
}

#[test]
fn physical_native_plex_next_uses_the_mixed_favorite_destination_binding() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let mut app = list_app(&[42, 43], 42);
    let runtime = app.ott.as_mut().unwrap();
    let profile = runtime.effective_profile().unwrap().clone();
    let current = selection::capture(&profile, &runtime.catalog.entries[0]).unwrap();
    let other_profile = synthetic_profile();
    let other = selection::capture(&other_profile, &synthetic_entry()).unwrap();
    let expected = format!("favorite:{}", other.identity());
    runtime.saved.profiles.profiles.push(other_profile.clone());
    runtime.saved.favorites.toggle(current).unwrap();
    runtime.saved.favorites.toggle(other).unwrap();
    runtime.saved.favorites_selected = true;
    let active = runtime.saved.profiles.active_profile.clone();
    bare_native(&mut app);
    let identity = route::playback_identity(&app.player.session);
    physical_edge(&mut app, SDLK_UP, true, false);
    physical_edge(&mut app, SDLK_UP, false, false);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.play_target.as_ref().map(|r| r.id.as_str()), Some(expected.as_str()));
    assert_eq!(runtime.play_profile.as_ref().map(|(_, p)| p), Some(&other_profile));
    assert!(runtime.resolver_next.as_ref().unwrap().favorite.is_some());
    assert!(runtime.playing.is_none());
    assert_eq!(runtime.saved.profiles.active_profile, active);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert!(app.inputs.is_empty());
}

#[test]
fn physical_native_plex_missing_binding_refuses_without_loading_a_catalogue() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let mut app = app();
    bare_native(&mut app);
    let identity = route::playback_identity(&app.player.session);
    physical_edge(&mut app, SDLK_UP, true, false);
    physical_edge(&mut app, SDLK_UP, false, false);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.status.contains("Open the channel list"));
    assert!(runtime.playing.is_none() && runtime.play_target.is_none()
        && runtime.resolver_next.is_none() && runtime.catalog_next.is_none());
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert!(app.inputs.is_empty(), "a refused owned action must not fall through");
}
