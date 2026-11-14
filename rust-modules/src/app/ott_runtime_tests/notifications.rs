//! Real command, completion, and physical input boundaries; no feed/TV/network IO.
use super::*;

#[test]
fn notifications_before_first_runtime_clock_sample_do_not_draw_or_request_frames() {
    let _serial = plx_base::testlock::serial();
    let mut app = run::lifecycle_regression_tests::app();
    app.ott = Some(Box::new(runtime_fixture()));
    assert!(app.ott.as_ref().unwrap().clock_sample.is_none());
    assert!(!super::super::notifications_need_frame(&app));
    super::super::draw_notifications(&mut app);
}

fn invocation(runtime: &mut Runtime, id: u8, operation: &str, enabled: Option<bool>) -> Invocation {
    let mut params = json!({"version":1,"runtime":runtime.remote.runtime(),"operation":operation});
    if let Some(enabled) = enabled {
        params["enabled"] = json!(enabled);
    }
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{id:032x}"),"action":"notifications","params":params,"expires_at":1020
    }]})
    .to_string();
    runtime
        .remote
        .raw_poll_for_test(&body, runtime.now_ms())
        .remove(0)
}

#[test]
fn notifications_setting_get_and_rejected_set_work_without_waking_or_changing_kiosk_source() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready_app_fixture();
    app.ott_standby = true;
    let mut runtime = app.ott.take().unwrap();
    runtime.public_preferences.accept(None);
    runtime
        .saved
        .settings
        .insert("nativeKiosk".into(), json!({"invalid":"closed"}));
    assert!(super::super::input_policy::strict(&runtime));
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let authority = runtime.remote.authority_generation();
    let get = invocation(&mut runtime, 1, "get", None);
    assert!(super::super::input_policy::remote(&mut runtime, &get).is_ok());
    let result = commands::command(&mut runtime, &mut app, &get).unwrap();
    assert_eq!(
        result["data"],
        json!({"version":1,"runtime":runtime.remote.runtime(),
        "operation":"get","enabled":true,"persisted":true})
    );
    runtime.write_permit.revoke();
    let set = invocation(&mut runtime, 2, "set", Some(false));
    assert!(super::super::input_policy::remote(&mut runtime, &set).is_ok());
    let result = commands::command(&mut runtime, &mut app, &set).unwrap();
    assert_eq!(result["data"]["error"], "storage_unavailable");
    assert_eq!(
        public_preferences::notifications_enabled(&runtime.public_preferences),
        Some(true)
    );
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert_eq!(runtime.remote.authority_generation(), authority);
    assert!(app.ott_standby && !runtime.saving);
    app.ott = Some(runtime);
}

#[test]
fn notifications_setting_publishes_only_durable_readback_and_failure_keeps_previous_policy() {
    let _serial = plx_base::testlock::serial();
    for success in [true, false] {
        let mut app = ready_app_fixture();
        let runtime = app.ott.as_mut().unwrap();
        runtime.public_preferences.accept(None);
        runtime.status = "Existing player notice".into();
        let before = serde_json::to_value(&runtime.saved).unwrap();
        let authority = runtime.remote.authority_generation();
        let set = invocation(runtime, 1, "set", Some(false));
        // Model an admitted public writer exactly as the existing public settings
        // regressions do; the helper CAS/readback is tested at its own boundary.
        runtime.saving = true;
        assert_eq!(
            public_preferences::notifications_enabled(&runtime.public_preferences),
            Some(true)
        );
        let result = if success {
            Ok(public_preferences::Snapshot {
                value: plx_platform::storage::public_preferences::Preferences {
                    notifications_enabled: Some(false),
                    ..Default::default()
                },
                revision: 1,
                epoch: 1,
            })
        } else {
            Err(public_preferences::WriteFailure::Rejected(
                private::Error::Revoked,
            ))
        };
        let response = ok(json!({"version":1,"runtime":runtime.remote.runtime(),
            "operation":"set","enabled":false,"persisted":true}));
        runtime
            .tx
            .send(Job::PublicSaved(result, Some(set.clone()), response))
            .unwrap_or_else(|_| panic!("public completion receiver"));
        runtime_tick(&mut app);
        let mut runtime = app.ott.take().unwrap();
        let response = runtime
            .remote
            .response_for_id_for_test(&set.id)
            .expect("typed completion receipt");
        if success {
            assert_eq!(response["data"]["enabled"], false);
            assert_eq!(response["data"]["persisted"], true);
        } else {
            assert_eq!(response["data"]["error"], "storage_unavailable");
            assert!(response["data"].get("enabled").is_none());
        }
        let get = invocation(&mut runtime, 2, "get", None);
        let response = commands::command(&mut runtime, &mut app, &get).unwrap();
        assert_eq!(response["data"]["enabled"], !success);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert_eq!(
            runtime.remote.authority_generation(),
            authority,
            "policy does not rotate command authority"
        );
        assert_eq!(runtime.status, "Existing player notice");
        assert!(!runtime.saving);
        app.ott = Some(runtime);
    }
}
