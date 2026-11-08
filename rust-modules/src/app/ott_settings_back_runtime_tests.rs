//! Hardware Back must use the same inner destination as the visible return row.
use super::*;
use plx_machine::machine::{Key, UiMode};

fn ui(app: &mut App, action: OttAction) {
    let reply = app.pages.top_page().expect("mounted OTT instance");
    app.pages.emit(MachineId::Instance(reply),
        Fx::App(AppFx::Ott(OttOut::Action { reply, action })));
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
    runtime_tick(app);
    settle_pages(app);
}

#[test]
fn ordinary_inner_settings_back_matches_visible_return_without_cancelling_catalog() {
    let _serial = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for (setting, page, back_id) in [
            ("menu.items", "menu-items", "menu:back"),
            ("interfaceMode", "interface-mode", "interface:back"),
        ] {
            for physical in [false, true] {
                let mut app = mounted();
                let runtime = app.ott.as_mut().unwrap();
                if setting == "interfaceMode" {
                    // The shared fixture revokes writes; opening this separate picker
                    // requires loaded public preferences, even when only testing Back.
                    runtime.write_permit = private::WritePermit::new();
                    runtime.public_preferences.accept(None);
                    assert!(public_preferences::write_refusal(runtime).is_none());
                }
                runtime.saved.settings.insert("interfaceMode".into(), json!(mode.index()));
                app.bridge.apply_ui_mode(mode);
                ui(&mut app, OttAction::EditSetting(setting.into()));
                let runtime = app.ott.as_ref().unwrap();
                assert_eq!(runtime.page, page);
                let back = runtime.view().rows.into_iter().find(|row| row.id == back_id).unwrap().action;
                assert_eq!(back, OttAction::ShowSettings);
                let epochs = (runtime.generation, runtime.play_generation, runtime.playback_epoch);
                let cancels = (runtime.cancel.load(Ordering::Acquire), runtime.play_cancel.load(Ordering::Acquire));
                let saved = serde_json::to_value(&runtime.saved).unwrap();
                let owner = app.pages.top_page();
                if physical {
                    let tick = next_tick(&mut app);
                    bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(Key::Back, tick));
                    runtime_tick(&mut app);
                    settle_pages(&mut app);
                } else { ui(&mut app, back); }
                let runtime = app.ott.as_ref().unwrap();
                assert_eq!(runtime.page, "settings", "{mode:?} {page} physical={physical}");
                assert_eq!((runtime.generation, runtime.play_generation, runtime.playback_epoch), epochs);
                assert_eq!((runtime.cancel.load(Ordering::Acquire), runtime.play_cancel.load(Ordering::Acquire)), cancels);
                assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
                assert_eq!(app.pages.top_page(), owner);
                assert!(!runtime.saving);
                assert_eq!(app.bridge.ui_mode(), mode);
            }
        }
    }
}
