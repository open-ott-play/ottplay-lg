//! Synthetic time at the actual adapter; no media, filesystem or network fixture.
use super::*;

fn step(app: &mut App, now_ms: u64) {
    let mut runtime = app.ott.take().unwrap();
    sleep_timer::tick_at(&mut runtime, app, now_ms);
    app.ott = Some(runtime);
}

#[test]
fn timer_enters_existing_standby_and_retires_source_jobs_without_waking_it_again() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready_app_fixture();
    app.ott.as_mut().unwrap().saved.settings.insert("sleepTimeout".into(), json!(1));
    step(&mut app, 1_000);
    let cancel = app.ott.as_ref().unwrap().cancel.clone();
    let metadata_cancel = app.ott.as_ref().unwrap().metadata_cancel.clone();
    step(&mut app, 1_801_000);
    assert!(app.ott_standby);
    assert!(cancel.load(Ordering::Acquire));
    assert!(metadata_cancel.load(Ordering::Acquire));
    assert!(!app.ott.as_ref().unwrap().source_work_allowed());
    let generation = app.ott.as_ref().unwrap().generation;
    step(&mut app, 9_000_000);
    assert!(app.ott_standby);
    assert_eq!(app.ott.as_ref().unwrap().generation, generation);
}

#[test]
fn pending_save_uncertainty_and_user_activity_win_over_due_timer() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready_app_fixture();
    app.ott.as_mut().unwrap().saved.settings.insert("sleepTimeout".into(), json!(1));
    step(&mut app, 0);
    app.ott.as_mut().unwrap().saving = true;
    step(&mut app, 1_800_000);
    assert!(!app.ott_standby);
    app.ott.as_mut().unwrap().saving = false;
    app.last_input = app.last_input.wrapping_add(1);
    step(&mut app, 1_800_001);
    assert!(!app.ott_standby);
    app.ott.as_mut().unwrap().persistence_uncertain = true;
    step(&mut app, 3_600_001);
    assert!(!app.ott_standby);
    app.ott.as_mut().unwrap().persistence_uncertain = false;
    step(&mut app, 4_000_000);
    assert!(!app.ott_standby, "uncertain state cannot retain an old deadline");
}

#[test]
fn kiosk_armed_locked_and_invalid_policy_disable_sleep_timer() {
    let _serial = plx_base::testlock::serial();
    for strict in [false, true] {
        for target in ["armed", "item", "invalid"] {
            let mut app = ready_app_fixture();
            let runtime = app.ott.as_mut().unwrap();
            runtime.saved.settings.insert("sleepTimeout".into(), json!(1));
            let origin = json!({"provider":"m3u","profile":"Home","subprofile":null,"sourceAlias":"Local"});
            let target = if target == "item" {
                json!({"kind":"item","entry":{"title":"Fixture channel","origin":origin,
                    "locator":{"kind":"channel","channelId":"fixture-1"}}})
            } else { json!({"kind":target}) };
            runtime.saved.settings.insert("nativeKiosk".into(), json!({
                "version":1,"strict":strict,"origin":origin,"target":target
            }));
            assert!(!matches!(runtime.track_selection_kiosk(), Ok(crate::app::ott_kiosk_policy::State::Off)));
            step(&mut app, 0);
            step(&mut app, 10_800_000);
            assert!(!app.ott_standby);
        }
    }
}

#[test]
fn wake_before_next_timer_frame_gets_a_fresh_interval() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready_app_fixture();
    app.ott.as_mut().unwrap().saved.settings.insert("sleepTimeout".into(), json!(1));
    step(&mut app, 0);
    step(&mut app, 1_800_000);
    assert!(app.ott_standby);
    crate::app::ott_standby::set(&mut app, false);
    step(&mut app, 1_800_001);
    assert!(!app.ott_standby);
    step(&mut app, 3_600_001);
    assert!(app.ott_standby);
}

#[test]
fn selector_cancel_preserves_saved_state_and_retired_choice_is_rejected() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready_app_fixture();
    let mut runtime = app.ott.take().unwrap();
    runtime.page = "settings";
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    handle_action(&mut runtime, &mut app, OttAction::EditSetting("sleepTimeout".into())).unwrap();
    let view = runtime.raw_view();
    assert_eq!(view.page_id, "sleep-timer");
    assert_eq!(view.rows.len(), 6);
    assert_eq!(view.selected_row.as_deref(), Some("sleep:0"));
    let old = view.rows[1].action.clone();
    handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
    assert_eq!(runtime.page, "settings");
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    handle_action(&mut runtime, &mut app, OttAction::EditSetting("sleepTimeout".into())).unwrap();
    assert!(handle_action(&mut runtime, &mut app, old).is_err());
    assert!(!runtime.saving);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
}
