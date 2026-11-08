//! Composed Runtime admission and Saved-settlement regressions, using the
//! existing app fixture. Revoked write permits keep storage untouched.
use super::*;
use crate::app::ott_runtime::menu_visibility as menu;
use plx_appkit::menu_visibility::{Change, Item, Visibility};

struct ResetProjection(Visibility);
impl Drop for ResetProjection {
    fn drop(&mut self) { plx_appkit::menu_visibility::project(self.0); }
}
fn mounted() -> App {
    let mut app = app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.entropy = Some([1; 16]);
    runtime.write_permit.revoke();
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.continuity.stop(runtime.generation);
    runtime.page = "settings";
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    app
}
fn act(app: &mut App, action: OttAction) -> Result<(), &'static str> {
    let mut runtime = app.ott.take().unwrap();
    runtime.reply = app.pages.top_page();
    let result = handle_action(&mut runtime, app, action);
    app.ott = Some(runtime);
    result
}
fn set(app: &App) -> OttAction {
    let runtime = app.ott.as_ref().unwrap();
    let revision = runtime.saved.settings.get(menu::KEY).and_then(|value| value["revision"].as_u64()).unwrap_or(0);
    OttAction::MenuVisibility { revision, context: runtime.navigation_epoch,
        change: Change::Set { item: Item::Audio, visible: false } }
}

#[test]
fn actual_menu_page_offers_toggles_reset_and_settings_recovery() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    act(&mut app, OttAction::EditSetting("menu.items".into())).unwrap();
    let runtime = app.ott.as_ref().unwrap();
    let view = runtime.view();
    assert_eq!(view.page_id, "menu-items");
    assert_eq!(view.rows.len(), Item::ALL.len() + 2);
    assert!(view.rows.iter().any(|row| row.id == "menu:reset" && row.enabled));
    assert!(view.rows.iter().any(|row| row.action == OttAction::ShowSettings));
    assert!(!runtime.saving);
}

#[test]
fn stale_menu_choice_and_wrong_page_do_not_start_a_writer() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    act(&mut app, OttAction::EditSetting("menu.items".into())).unwrap();
    let old = set(&app);
    app.ott.as_mut().unwrap().accepted_navigation(false);
    assert!(act(&mut app, old).is_err());
    let wrong_page = set(&app);
    act(&mut app, OttAction::ShowSettings).unwrap();
    assert!(act(&mut app, wrong_page).is_err());
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn actual_menu_set_uses_one_writer_and_failed_save_preserves_visibility() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetProjection(plx_appkit::menu_visibility::projected());
    plx_appkit::menu_visibility::project(Default::default());
    let mut app = mounted();
    act(&mut app, OttAction::EditSetting("menu.items".into())).unwrap();
    let action = set(&app);
    act(&mut app, action.clone()).unwrap();
    assert!(act(&mut app, action).is_err());
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.saving);
    assert!(runtime.saved.settings.get(menu::KEY).is_none());
    assert!(plx_appkit::menu_visibility::projected().visible(Item::Audio));
    let job = runtime.rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(&job, Job::Saved(Err(_), None, false, _)));
    runtime.tx.send(job).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert!(plx_appkit::menu_visibility::projected().visible(Item::Audio));
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn menu_change_is_parental_settings_protected_and_kiosk_blocked() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    act(&mut app, OttAction::EditSetting("menu.items".into())).unwrap();
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.settings.insert("parentPin".into(), json!("pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
    runtime.saved.settings.insert("psOptions".into(), json!(1));
    let action = set(&app);
    assert!(act(&mut app, action).is_err());
    let runtime = app.ott.as_mut().unwrap();
    assert!(!runtime.saving);
    runtime.saved.settings.remove("parentPin");
    runtime.saved.settings.remove("psOptions");
    // Unknown kiosk authority must fail closed before Saved, for Reset too.
    runtime.saved.settings.insert("nativeKiosk".into(), Value::Null);
    let context = runtime.navigation_epoch;
    assert!(act(&mut app, OttAction::MenuVisibility { revision: 0, context, change: Change::Reset }).is_err());
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn accepted_saved_projects_without_navigation_but_unknown_saved_never_does() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetProjection(plx_appkit::menu_visibility::projected());
    plx_appkit::menu_visibility::project(Default::default());
    let mut app = mounted();
    let runtime = app.ott.as_mut().unwrap();
    let candidate = menu::candidate(&runtime.saved, Change::Set { item: Item::Audio, visible: false }).unwrap();
    let generation = runtime.play_generation;
    runtime.page = "profiles";
    runtime.tx.send(Job::Saved(Ok(candidate.clone()), None, false, json!({}))).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    assert_eq!(runtime.page, "profiles");
    assert_eq!(runtime.play_generation, generation);
    assert!(!plx_appkit::menu_visibility::projected().visible(Item::Audio));
    runtime.tx.send(Job::Uncertain(Some(candidate), None)).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert!(plx_appkit::menu_visibility::projected().is_default());
    assert!(app.ott.as_ref().unwrap().persistence_uncertain);
}

fn wire(app: &mut App, id: u64, params: Value) -> Option<Value> {
    let mut runtime = app.ott.take().unwrap();
    let now = runtime.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{id:032x}"),"action":"player_settings","params":params,"expires_at":1020
    }]}).to_string();
    let mut requests = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(requests.len(), 1);
    let invocation = requests.pop().unwrap();
    let response = match crate::app::ott_runtime::input_policy::remote(&mut runtime, &invocation) {
        Ok(()) => commands::command(&mut runtime, app, &invocation),
        Err(error) => Some(rejected(error)),
    };
    app.ott = Some(runtime);
    response
}

#[test]
fn remote_integer_get_set_reaches_the_same_writer_and_failed_saved_keeps_default() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetProjection(plx_appkit::menu_visibility::projected());
    plx_appkit::menu_visibility::project(Default::default());
    let mut app = mounted();
    let runtime = app.ott.as_ref().unwrap().remote.runtime().to_owned();
    let before = wire(&mut app, 71, json!({"op":"get","runtime":runtime,"id":"menuShowAudio"})).unwrap();
    assert_eq!(before["status"], "ok");
    assert_eq!(before["data"]["descriptor"]["type"], "integer");
    assert_eq!(before["data"]["descriptor"]["constraints"], json!({"values":[0,1]}));
    assert_eq!(before["data"]["setting"]["value"], 1);
    assert_eq!(before["data"]["setting"]["effective_value"], 1);
    let response = wire(&mut app, 72, json!({"op":"set","runtime":runtime,"id":"menuShowAudio",
        "context":before["data"]["context"]["token"],"revision":before["data"]["setting"]["revision"],"value":0}));
    assert!(response.is_none());
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.saving);
    assert!(!runtime.saved.settings.contains_key(menu::KEY));
    let job = runtime.rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(&job, Job::Saved(Err(_), Some(_), false, _)));
    runtime.tx.send(job).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert!(!app.ott.as_ref().unwrap().saving);
    assert!(plx_appkit::menu_visibility::projected().visible(Item::Audio));
}

#[test]
fn remote_menu_rejects_boolean_before_consuming_the_integer_write_receipt() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let runtime = app.ott.as_ref().unwrap().remote.runtime().to_owned();
    let before = wire(&mut app, 81, json!({"op":"get","runtime":runtime,"id":"menuShowAudio"})).unwrap();
    for (index, value) in [json!(false), json!(true), json!(2), json!(-1), json!("0")].into_iter().enumerate() {
        let response = wire(&mut app, 82 + index as u64, json!({"op":"set","runtime":runtime,"id":"menuShowAudio",
            "context":before["data"]["context"]["token"],"revision":before["data"]["setting"]["revision"],"value":value}));
        if response.is_none() {
            // Drain a wrongly admitted real worker before reporting the RED.
            let runtime = app.ott.as_ref().unwrap();
            let job = runtime.rx.recv_timeout(Duration::from_secs(1)).expect("admitted revoked writer must settle");
            assert!(matches!(&job, Job::Saved(Err(_), Some(_), false, _)));
            runtime.tx.send(job).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
            runtime_tick(&mut app);
        }
        assert!(response.is_some(), "v1 menu setting must reject non-0/1 input synchronously without starting Saved");
        let response = response.expect("typed refusal checked above");
        assert_eq!(response["data"]["reason"], "invalid_value");
        assert!(!app.ott.as_ref().unwrap().saving);
    }
    assert!(wire(&mut app, 89, json!({"op":"set","runtime":runtime,"id":"menuShowAudio",
        "context":before["data"]["context"]["token"],"revision":before["data"]["setting"]["revision"],"value":0})).is_none());
    let runtime = app.ott.as_ref().unwrap();
    let job = runtime.rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(&job, Job::Saved(Err(_), Some(_), false, _)));
    runtime.tx.send(job).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
}

#[test]
fn remote_menu_get_projects_stored_and_effective_integer_values() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetProjection(plx_appkit::menu_visibility::projected());
    let mut app = mounted();
    let r = app.ott.as_mut().unwrap();
    r.saved = menu::candidate(&r.saved, Change::Set { item: Item::Audio, visible: false }).unwrap();
    plx_appkit::menu_visibility::project(menu::visibility(r));
    let runtime = r.remote.runtime().to_owned();
    let reply = wire(&mut app, 91, json!({"op":"get","runtime":runtime,"id":"menuShowAudio"})).unwrap();
    assert_eq!(reply["status"], "ok");
    assert_eq!(reply["data"]["setting"]["storage"], "stored");
    for key in ["value", "persisted_value", "effective_value"] {
        assert_eq!(reply["data"]["setting"][key], 0, "{key}");
    }
}

#[path = "ott_settings_back_runtime_tests.rs"]
mod settings_back;
