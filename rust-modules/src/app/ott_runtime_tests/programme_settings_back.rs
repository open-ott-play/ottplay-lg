//! Inner settings return through the real Player catalog surface and key dispatcher.
use super::*;
use plx_machine::machine::UiMode;

#[test]
fn player_inner_settings_back_matches_visible_return_and_preserves_decoder() {
    let _serial = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [UiMode::Classic, UiMode::Modern] {
        for (setting, page, back_id) in [
            ("menu.items", "menu-items", "menu:back"),
            ("interfaceMode", "interface-mode", "interface:back"),
        ] {
            for physical in [false, true] {
                let mut app = catalog_app();
                let runtime = app.ott.as_mut().unwrap();
                if setting == "interfaceMode" {
                    // The shared fixture revokes writes; opening this separate picker
                    // requires loaded public preferences, even when only testing Back.
                    runtime.write_permit = private::WritePermit::new();
                    runtime.public_preferences.accept(None);
                    runtime.last_checkpoint_ms = runtime.now_ms();
                    assert!(public_preferences::write_refusal(runtime).is_none());
                }
                runtime.saved.settings.insert("interfaceMode".into(), json!(mode.index()));
                app.bridge.apply_ui_mode(mode);
                open_more(&mut app);
                choose_catalog(&mut app);
                surface_action(&mut app, OttAction::ShowSettings);
                surface_action(&mut app, OttAction::EditSetting(setting.into()));
                let runtime = app.ott.as_ref().unwrap();
                assert_eq!(runtime.page, page);
                let back = runtime.view().rows.into_iter().find(|row| row.id == back_id).unwrap().action;
                assert_eq!(back, OttAction::ShowSettings);
                let epochs = (runtime.generation, runtime.play_generation, runtime.playback_epoch);
                let saved = serde_json::to_value(&runtime.saved).unwrap();
                let surface = ott_surface(&app).expect("settings retain the existing OTT surface");
                let owner = app.pages.top_page();
                let origin = bridge::player(&app.pages).unwrap().origin;
                let identity = route::playback_identity(&app.player.session);
                let source = route::external_source(&app.player.session);
                let position = player::SHARED.playpos_ns.load(Ordering::Acquire);
                if physical { key(&mut app, Key::Back); } else { surface_action(&mut app, back); }
                let runtime = app.ott.as_ref().unwrap();
                assert_eq!(runtime.page, "settings", "{mode:?} {page} physical={physical}");
                assert_eq!((runtime.generation, runtime.play_generation, runtime.playback_epoch), epochs);
                assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
                assert!(!runtime.cancel.load(Ordering::Acquire));
                assert_eq!(ott_surface(&app), Some(surface));
                assert_eq!(app.pages.top_page(), owner);
                assert_eq!(bridge::player(&app.pages).unwrap().origin, origin);
                assert_eq!(route::playback_identity(&app.player.session), identity);
                assert_eq!(route::external_source(&app.player.session), source);
                assert_eq!(player::SHARED.playpos_ns.load(Ordering::Acquire), position);
                assert_eq!(player::state(&app.player.session), player::PlaybackState::Paused);
                assert!(app.adapters.player.is_live());
                assert!(app.player.pending_exit.is_none());
                assert!(!app.refresh_hubs);
                assert!(!runtime.saving);
                assert_eq!(app.bridge.ui_mode(), mode);
            }
        }
    }
}
