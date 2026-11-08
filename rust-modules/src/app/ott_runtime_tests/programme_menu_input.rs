//! Existing physical SDL ingress and dispatcher, with inert playback fixtures.
//! T is the FOSS PC Tools key; native Yellow retains its measured native code.
use super::*;

fn physical_t(modifiers: u16, edge: Edge) -> [u8; 128] {
    let mut bytes = [0; 128];
    bytes[..4].copy_from_slice(&(if edge == Edge::Up { 0x301_u32 } else { 0x300_u32 }).to_ne_bytes());
    bytes[8..12].copy_from_slice(&1_u32.to_ne_bytes());
    bytes[12] = u8::from(edge != Edge::Up);
    bytes[13] = u8::from(edge == Edge::Repeat);
    bytes[20..24].copy_from_slice(&(b't' as u32).to_ne_bytes());
    bytes[24..26].copy_from_slice(&modifiers.to_ne_bytes());
    bytes
}

fn menu_app(kind: &str, hud: bool) -> App {
    let mut app = if kind == "plex" { native_catalog_app().0 } else { remote_app() };
    {
        let runtime = app.ott.as_mut().unwrap();
        // Native Plex has no OTT Playing/checkpoint owner and must keep the
        // real public-preference admission idle. Ordinary fixtures hold writes.
        runtime.saving = kind != "plex";
        runtime.saved.settings.insert("interfaceMode".into(), json!(0));
        if kind == "live" {
            let mut profile = runtime.playing.as_ref().unwrap().profile.clone();
            profile.kind = ProviderKind::M3u;
            profile.source = "https://media.invalid/live.m3u".into();
            runtime.saved.profiles.profiles = vec![profile.clone()];
            runtime.saved.profiles.active_profile = Some(profile.id.clone());
            runtime.provider = ProviderKind::M3u;
            for entry in &mut runtime.catalog.entries {
                entry.live = Some(plx_data::ott::guide::ChannelMetadata::default());
                entry.duration_ms = None;
            }
            let playing = runtime.playing.as_mut().unwrap();
            playing.profile = profile;
            playing.entry = runtime.catalog.entries[0].clone();
        }
    }
    app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Classic);
    player::TX.commit_paused(false);
    player::swap_state_for_test(player::PlaybackState::Playing);
    let now = clock::now();
    let page = bridge::player_mut(&mut app.pages).unwrap();
    if hud { page.hud.extend(now, 60_000); } else { page.hud.hide(false); }
    app.inputs.clear();
    app
}

fn more_count(app: &App) -> usize {
    app.pages.nav.modals.surfaces.iter().filter(|surface| matches!(&surface.entry.arg,
        AppArg::PlayerOverlay(arg) if matches!(arg.kind, OverlayKind::More { .. }))).count()
}

fn physical(app: &mut App, modifiers: u16, edge: Edge) {
    let fr = dispatch_bytes(app, physical_t(modifiers, edge));
    finish_input(app, &fr);
}

#[test]
fn physical_tools_opens_one_more_for_live_vod_and_native_plex_with_or_without_hud() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    for kind in ["live", "vod", "plex"] {
        for hud in [false, true] {
            let mut app = menu_app(kind, hud);
            let identity = route::playback_identity(&app.player.session);
            let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
            let native = app.ott.as_ref().unwrap().playing.is_none();
            let epoch = app.ott.as_ref().unwrap().playback_epoch;
            physical(&mut app, 0, Edge::Down);
            idle(&mut app);
            assert_eq!(more_count(&app), 1, "{kind}, hud={hud}");
            physical(&mut app, 0, Edge::Repeat);
            physical(&mut app, 0, Edge::Up);
            idle(&mut app);
            assert_eq!(more_count(&app), 1, "hold/release cannot open another More");
            let runtime = app.ott.as_ref().unwrap();
            assert_eq!(runtime.playing.is_none(), native);
            assert_eq!(runtime.playback_epoch, epoch);
            assert!(runtime.play_target.is_none() && runtime.catalog_next.is_none());
            assert_eq!(route::playback_identity(&app.player.session), identity);
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
            assert_eq!(app.down_sym, 0);
        }
    }
}

#[test]
fn tools_rejects_modified_keys_modern_policy_and_foreign_surface_ownership() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for case in ["shift", "control", "alt", "command", "modern", "policy", "surface", "pending"] {
        let mut app = menu_app("vod", false);
        let modifiers = match case { "shift" => 1, "control" => 0x40, "alt" => 0x100, "command" => 0x400, _ => 0 };
        match case {
            "modern" => {
                app.ott.as_mut().unwrap().saved.settings.insert("interfaceMode".into(), json!(1));
                app.bridge.apply_ui_mode(plx_machine::machine::UiMode::Modern);
            }
            "policy" => { app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), json!("malformed")); }
            "surface" | "pending" => {
                bridge::open_settings(&mut app.pages);
                if case == "surface" { idle(&mut app); }
            }
            _ => {}
        }
        let identity = route::playback_identity(&app.player.session);
        physical(&mut app, modifiers, Edge::Down);
        physical(&mut app, modifiers, Edge::Up);
        idle(&mut app);
        assert_eq!(more_count(&app), 0, "{case}");
        assert_eq!(route::playback_identity(&app.player.session), identity);
        assert!(app.ott.as_ref().unwrap().play_target.is_none());
    }
}

#[test]
fn tools_captured_source_is_rechecked_before_menu_presentation() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    let mut app = menu_app("plex", false);
    let fr = dispatch_bytes(&mut app, physical_t(0, Edge::Down));
    let sid = route::cur_sid(&app.player.session);
    route::install_plex_playback_for_test(&mut app.player.session, sid, "replacement");
    let replacement = route::playback_identity(&app.player.session);
    finish_input(&mut app, &fr);
    physical(&mut app, 0, Edge::Up);
    idle(&mut app);
    assert_eq!(more_count(&app), 0);
    assert_eq!(route::playback_identity(&app.player.session), replacement);
    assert!(app.ott.as_ref().unwrap().playing.is_none());
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
}

#[test]
fn native_yellow_keeps_menu_access_for_live_vod_and_native_plex() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let _registry = NativeCatalogRegistry;
    for kind in ["live", "vod", "plex"] {
        let mut app = menu_app(kind, false);
        let identity = route::playback_identity(&app.player.session);
        native_edge(&mut app, 0, consts::WCODE_YELLOW, Edge::Down);
        native_edge(&mut app, 0, consts::WCODE_YELLOW, Edge::Up);
        idle(&mut app);
        assert_eq!(more_count(&app), 1, "{kind}");
        assert_eq!(route::playback_identity(&app.player.session), identity);
        assert!(app.ott.as_ref().unwrap().play_target.is_none());
    }
}
