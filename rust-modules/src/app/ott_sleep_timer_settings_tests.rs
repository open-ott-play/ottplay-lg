use super::*;
use super::super::super::sleep_timer::Choice;

#[test]
fn sleep_timer_schema_uses_classic_indices_and_application_scope() {
    let field = Setting::Sleep;
    let descriptor = field.descriptor();
    assert_eq!(descriptor["id"], "sleepTimeout");
    assert_eq!(descriptor["scope"], "application");
    assert_eq!(descriptor["default_value"], 0);
    assert_eq!(descriptor["access"]["write"], true);
    assert_eq!(descriptor["constraints"]["values"], json!([0, 1, 2, 3, 4]));
    assert_eq!(Setting::ALL[26..29].iter().map(|setting| setting.id()).collect::<Vec<_>>(),
        ["volumeStep", "sleepTimeout", "infoTimeout"]);
    for index in 0..=4 { assert!(field.valid(&json!(index))); }
    for value in [json!(-1), json!(5), json!(30), json!(1.0), json!(true), json!("1"), Value::Null] {
        assert!(!field.valid(&value), "{value}");
    }
}

#[test]
fn absent_or_invalid_sleep_choice_is_off_without_disclosing_invalid_bytes() {
    let mut saved = Saved::default();
    assert_eq!(stored(&saved, Setting::Sleep)["value"], 0);
    assert_eq!(stored(&saved, Setting::Sleep)["storage"], "default");
    saved.settings.insert("sleepTimeout".into(), json!("private-invalid"));
    let invalid = stored(&saved, Setting::Sleep);
    assert_eq!(invalid["value"], 0);
    assert_eq!(invalid["storage"], "invalid");
    assert!(invalid.get("persisted_value").is_none());
}

#[test]
fn sleep_candidate_preserves_profiles_and_refreshes_same_value_and_aba_revision() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
    runtime.saved.category = "Favorites".into();
    runtime.saved.settings.insert("unrelated".into(), json!({"keep":true}));
    let before = runtime.saved.clone();
    let original = signature(&before, Setting::Sleep);
    let thirty = runtime.sleep_timer_candidate(Choice::Minutes30);
    assert_eq!(runtime.sleep_timer_choice(), Choice::Off, "candidate is not published");
    runtime.saved = thirty;
    let first = signature(&runtime.saved, Setting::Sleep);
    runtime.saved = runtime.sleep_timer_candidate(Choice::Minutes30);
    assert_ne!(signature(&runtime.saved, Setting::Sleep), first);
    let mut off = runtime.sleep_timer_candidate(Choice::Off);
    assert_ne!(signature(&off, Setting::Sleep), original);
    off.settings.remove("sleepTimeout");
    off.settings.remove("settingRevision:sleepTimeout");
    assert_eq!(serde_json::to_value(off).unwrap(), serde_json::to_value(before).unwrap());
}
