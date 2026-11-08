//! Actual settings admission with revoked worker permits; completions are modeled,
//! not a claim about disk durability, rendered layout or remote hardware.
use super::*;
use plx_appkit::classic_settings::Preferences;

fn setup() -> App {
    let mut app = ready_app_fixture();
    let r = app.ott.as_mut().unwrap();
    r.write_permit.revoke();
    r.catalog_done.store(false, Ordering::Release);
    r.metadata_done.store(false, Ordering::Release);
    r.resolver_busy = true;
    r.plex_refresh_pending = true;
    r.continuity.stop(r.generation);
    r.navigation_epoch = 1;
    app
}
fn request(app: &mut App, serial: u64, params: Value) -> Invocation {
    let r = app.ott.as_mut().unwrap();
    let now = r.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{serial:032x}"),"action":"player_settings","params":params,"expires_at":1020
    }]}).to_string();
    let mut received = r.remote.raw_poll_for_test(&body, now);
    assert_eq!(received.len(), 1);
    received.pop().unwrap()
}
fn dispatch(app: &mut App, inv: &Invocation) -> Option<Value> {
    let mut r = app.ott.take().unwrap();
    let result = commands::command(&mut r, app, inv);
    app.ott = Some(r);
    result
}
fn get(app: &mut App, serial: u64, id: &str) -> Value {
    let token = app.ott.as_ref().unwrap().remote.runtime().to_owned();
    let inv = request(app, serial, json!({"op":"get","runtime":token,"id":id}));
    let result = dispatch(app, &inv).unwrap();
    assert_eq!(result["status"], "ok");
    result["data"].clone()
}
fn set_params(get: &Value, value: Value) -> Value {
    json!({"op":"set","runtime":get["runtime"],"id":get["setting"]["id"],
        "context":get["context"]["token"],"revision":get["setting"]["revision"],"value":value})
}
fn stage(app: &mut App, id: &str, value: u64) -> (Invocation, Saved, Job) {
    let before = get(app, 1, id);
    let inv = request(app, 2, set_params(&before, json!(value)));
    let r = app.ott.as_ref().unwrap();
    let old = serde_json::to_value(&r.saved).unwrap();
    let mut candidate = r.saved.clone();
    let [value_key, revision_key] = r.osd_setting_storage_keys(id).unwrap();
    candidate.settings.insert(value_key, json!(value));
    let stamp: String = plx_base::sha256::sha256(json!(inv.id).to_string().as_bytes())
        .iter().map(|b| format!("{b:02x}")).collect();
    candidate.settings.insert(revision_key, json!(stamp));
    assert!(dispatch(app, &inv).is_none());
    let r = app.ott.as_ref().unwrap();
    assert!(r.saving);
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), old);
    assert_eq!(r.classic_preferences(), Preferences::default());
    assert!(r.remote.response_for_id_for_test(&inv.id).is_none());
    let job = r.rx.recv_timeout(Duration::from_secs(5)).expect("revoked writer result");
    assert!(matches!(&job, Job::Saved(Err(_), Some(_), false, _)));
    (inv, candidate, job)
}
fn deliver(app: &mut App, inv: &Invocation, job: Job) -> Value {
    app.ott.as_ref().unwrap().tx.send(job).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(app);
    app.ott.as_ref().unwrap().remote.response_for_id_for_test(&inv.id).expect("addressed reply")
}

#[test]
fn classic_settings_schema_appends_all_classic_fields_and_hides_duplicate_arrow_choice() {
    let _serial = plx_base::testlock::serial();
    let mut app = setup();
    let token = app.ott.as_ref().unwrap().remote.runtime().to_owned();
    let mut cursor = Value::Null;
    let mut ids = Vec::new();
    for serial in 10..13 {
        let mut params = json!({"op":"schema","runtime":token,"limit":20});
        if !cursor.is_null() { params["cursor"] = cursor; }
        let inv = request(&mut app, serial, params);
        let result = dispatch(&mut app, &inv).unwrap();
        assert_eq!(result["status"], "ok");
        ids.extend(result["data"]["settings"].as_array().unwrap().iter()
            .map(|v| v["id"].as_str().unwrap().to_owned()));
        cursor = result["data"]["next_cursor"].clone();
        if cursor.is_null() { break; }
    }
    assert_eq!(ids.len(), 41);
    assert_eq!(ids[40], "uiColorConfig");
    assert_eq!(&ids[26..40], &["volumeStep", "sleepTimeout", "infoTimeout", "osdOpacity", "permanentTime",
        "pageSize", "listPosition", "seek13Duration", "arrowFun", "showNumber", "showName", "showProgram", "showDescription", "showProgress"]);
    for (serial,id,value) in [(20,"pageSize",25),(21,"listPosition",0),(22,"seek13Duration",15),(23,"arrowFun",0)] {
        let result = get(&mut app, serial, id);
        assert_eq!(result["setting"]["value"], value);
        assert_eq!(result["setting"]["effective_value"], value);
        assert_eq!(result["descriptor"]["sensitive"], false);
    }
    let arrow = get(&mut app, 24, "arrowFun");
    assert_eq!(arrow["descriptor"]["constraints"]["values"], json!([0,1,2,3]));
    let choices = arrow["descriptor"]["choices"].as_array().unwrap();
    assert_eq!(choices.iter().map(|v|v["value"].as_u64().unwrap()).collect::<Vec<_>>(), vec![0,1,2]);
}

#[test]
fn classic_settings_invalid_values_and_unknown_fields_leave_cas_and_saved_intact() {
    let _serial = plx_base::testlock::serial();
    for (id,bad,valid) in [("pageSize",31,10),("listPosition",2,1),("seek13Duration",16,20),("arrowFun",4,3),
        ("showNumber",2,0),("showName",2,0),("showProgram",2,0),("showDescription",2,0),("showProgress",2,0)] {
        let mut app = setup();
        let before = get(&mut app,1,id);
        let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        for (at,value) in [json!(-1),json!(bad),json!(1.0),json!(true),json!("1"),Value::Null,json!(u64::MAX)].into_iter().enumerate() {
            let inv = request(&mut app,10+at as u64,set_params(&before,value));
            assert_eq!(dispatch(&mut app,&inv).unwrap()["data"]["reason"], "invalid_value");
        }
        let mut unknown = set_params(&before,json!(valid)); unknown["unexpected"] = json!(1);
        let inv = request(&mut app,30,unknown);
        assert_eq!(dispatch(&mut app,&inv).unwrap()["data"]["reason"], "invalid_request");
        let r=app.ott.as_ref().unwrap();
        assert_eq!(serde_json::to_value(&r.saved).unwrap(),saved);
        assert!(!r.saving); assert!(r.rx.try_recv().is_err());
        let inv = request(&mut app,31,set_params(&before,json!(valid)));
        assert!(dispatch(&mut app,&inv).is_none(),"original revision remains usable");
        assert!(matches!(app.ott.as_ref().unwrap().rx.recv_timeout(Duration::from_secs(5)).unwrap(),Job::Saved(Err(_),Some(_),false,_)));
    }
}

#[test]
fn classic_settings_publish_only_after_saved_and_restore_through_loaded() {
    let _serial = plx_base::testlock::serial();
    for (id,value,expected) in [("pageSize",10,(10,0,0,15)),("listPosition",1,(25,1,0,15)),
        ("seek13Duration",3600,(25,0,0,3600)),("arrowFun",3,(25,0,3,15))] {
        let mut app = setup();
        let r=app.ott.as_ref().unwrap();
        let before=(r.provider,r.navigation_epoch,r.playback_epoch,r.saved.profiles.clone());
        let (inv,candidate,job)=stage(&mut app,id,value);
        let Job::Saved(_,pending,reload,response)=job else { unreachable!() };
        assert!(!reload);
        let response=deliver(&mut app,&inv,Job::Saved(Ok(candidate.clone()),pending,reload,response));
        assert_eq!(response["status"],"ok"); assert_eq!(response["data"]["persisted"],true);
        let expected=Preferences::from_values(expected.0,expected.1,expected.2,expected.3).unwrap();
        let r=app.ott.as_ref().unwrap();
        assert_eq!(r.classic_preferences(),expected);
        assert_eq!((r.provider,r.navigation_epoch,r.playback_epoch,r.saved.profiles.clone()),before);
        assert_eq!(get(&mut app,3,id)["setting"]["effective_value"],value);
        drop(app);
        let mut reopened=setup();
        reopened.ott.as_ref().unwrap().tx.send(Job::Loaded(Ok((candidate,[1;16])))).unwrap_or_else(|_|panic!("fixture receiver disappeared"));
        runtime_tick(&mut reopened);
        assert_eq!(reopened.ott.as_ref().unwrap().classic_preferences(),expected);
    }
}

#[test]
fn classic_settings_failed_or_uncertain_writes_preserve_accepted_preferences() {
    let _serial = plx_base::testlock::serial();
    for (id,value) in [("pageSize",10),("listPosition",1),("seek13Duration",20),("arrowFun",2),
        ("showNumber",0),("showName",0),("showProgram",0),("showDescription",0),("showProgress",0)] {
        for outcome in 0..3 {
            let mut app=setup(); let (inv,candidate,job)=stage(&mut app,id,value);
            let completion=match outcome {0=>job,1=>Job::Uncertain(None,Some(inv.clone())),_=>Job::Uncertain(Some(candidate),Some(inv.clone()))};
            let response=deliver(&mut app,&inv,completion);
            assert_eq!(response["status"],"rejected");
            assert_eq!(app.ott.as_ref().unwrap().classic_preferences(),Preferences::default());
            assert_eq!(app.ott.as_ref().unwrap().persistence_uncertain,outcome!=0);
        }
    }
}

#[test]
fn classic_local_candidates_share_validation_and_retire_aba_without_publishing() {
    let _serial = plx_base::testlock::serial();
    for (id,default,valid,bad) in [("pageSize","25","10","31"),("listPosition","0","1","2"),
        ("seek13Duration","15","20","16"),("arrowFun","0","3","4"),
        ("showNumber","1","0","2"),("showName","1","0","2"),
        ("showProgram","1","0","2"),("showDescription","1","0","2"),("showProgress","1","0","2")] {
        let mut app=setup(); let before=get(&mut app,1,id);
        let r=app.ott.as_mut().unwrap(); let original=serde_json::to_value(&r.saved).unwrap();
        let old=r.osd_setting_token(id).unwrap();
        assert_eq!(r.osd_setting_value(id).as_deref(),Some(default));
        for text in ["", "-1", "+1", "1.0", "1e0", "999999999999999999999", bad] {
            assert!(r.osd_setting_candidate(id,text,1).is_err());
        }
        assert_eq!(serde_json::to_value(&r.saved).unwrap(),original);
        r.saved=r.osd_setting_candidate(id,valid,1).unwrap();
        r.saved=r.osd_setting_candidate(id,default,2).unwrap();
        assert_ne!(r.osd_setting_token(id).unwrap(),old);
        assert_eq!(r.classic_preferences(),Preferences::default());
        let inv=request(&mut app,2,set_params(&before,json!(valid.parse::<u64>().unwrap())));
        assert_eq!(dispatch(&mut app,&inv).unwrap()["data"]["reason"],"revision_conflict");
    }
}

#[test]
fn classic_malformed_storage_falls_back_per_field_and_legacy_three_is_not_rewritten() {
    let _serial = plx_base::testlock::serial();
    let mut app=setup();
    let r=app.ott.as_mut().unwrap();
    r.saved.settings.insert("pageSize".into(),json!(31));
    r.saved.settings.insert("listPosition".into(),json!("1"));
    r.saved.settings.insert("seek13Duration".into(),json!(16));
    r.saved.settings.insert("arrowFun".into(),json!(3));
    let candidate=r.saved.clone();
    r.tx.send(Job::Loaded(Ok((candidate,[1;16])))).unwrap_or_else(|_|panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().classic_preferences(),Preferences::from_values(25,0,3,15).unwrap());
    let before=serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    for (serial,id,default) in [(1,"pageSize",25),(2,"listPosition",0),(3,"seek13Duration",15)] {
        let response=get(&mut app,serial,id);
        assert_eq!(response["setting"]["storage"],"invalid");
        assert_eq!(response["setting"]["effective_value"],default);
        assert!(response["setting"].get("persisted_value").is_none());
    }
    let legacy=get(&mut app,4,"arrowFun");
    assert_eq!(legacy["setting"]["persisted_value"],3);
    assert_eq!(legacy["setting"]["effective_value"],3);
    assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(),before);
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn classic_visibility_is_provider_scoped_durable_and_shared_between_profile_slots() {
    let _serial = plx_base::testlock::serial();
    for id in ["showNumber", "showName", "showProgram", "showDescription", "showProgress"] {
        let mut app = setup();
        let initial = get(&mut app, 10, id);
        assert_eq!(initial["descriptor"]["scope"], "provider");
        assert_eq!(initial["setting"]["value"], 1);
        let (inv, candidate, job) = stage(&mut app, id, 0);
        let Job::Saved(_, pending, reload, response) = job else { unreachable!() };
        assert_eq!(deliver(&mut app, &inv, Job::Saved(Ok(candidate.clone()), pending, reload, response))["status"], "ok");
        let r = app.ott.as_mut().unwrap();
        let original_provider = r.provider;
        let original_profiles = r.saved.profiles.clone();
        let keys = r.osd_setting_storage_keys(id).unwrap();
        assert!(keys[0].starts_with("provider:"));
        assert_eq!(keys[1], format!("settingRevision:{}", keys[0]));
        assert!(!r.saved.settings.contains_key(id));
        assert_eq!(r.osd_setting_value(id).as_deref(), Some("0"));
        // Slot selection is not part of the provider preference key.
        r.saved.profiles.active_profile = Some("different-slot".into());
        assert_eq!(r.osd_setting_storage_keys(id).unwrap(), keys);
        assert_eq!(r.osd_setting_value(id).as_deref(), Some("0"));
        r.provider = if original_provider == ProviderKind::Plex { ProviderKind::M3u } else { ProviderKind::Plex };
        assert_ne!(r.osd_setting_storage_keys(id).unwrap(), keys);
        assert_eq!(r.osd_setting_value(id).as_deref(), Some("1"));
        r.provider = original_provider; r.saved.profiles = original_profiles;
        assert_eq!(get(&mut app, 11, id)["setting"]["effective_value"], 0);
        drop(app);
        let mut reopened = setup();
        reopened.ott.as_ref().unwrap().tx.send(Job::Loaded(Ok((candidate, [1;16])))).unwrap_or_else(|_|panic!("fixture receiver disappeared"));
        runtime_tick(&mut reopened);
        assert_eq!(get(&mut reopened, 12, id)["setting"]["effective_value"], 0);
    }
}

#[test]
fn classic_visibility_invalid_provider_bytes_default_without_rewriting() {
    let _serial = plx_base::testlock::serial();
    for id in ["showNumber", "showName", "showProgram", "showDescription", "showProgress"] {
        let mut app = setup();
        let r = app.ott.as_mut().unwrap();
        let [key, _] = r.osd_setting_storage_keys(id).unwrap();
        r.saved.settings.insert(key.clone(), json!(2));
        let read = get(&mut app, 1, id);
        assert_eq!(read["setting"]["storage"], "invalid");
        assert_eq!(read["setting"]["effective_value"], 1);
        assert_eq!(app.ott.as_ref().unwrap().saved.settings[&key], 2);
    }
}

#[test]
fn classic_visibility_uncertain_image_preserves_each_accepted_provider_and_slot() {
    let _serial = plx_base::testlock::serial();
    for id in ["showNumber", "showName", "showProgram", "showDescription", "showProgress"] {
        let mut app = setup();
        let (inv, accepted, job) = stage(&mut app, id, 0);
        let Job::Saved(_, pending, reload, response) = job else { unreachable!() };
        assert_eq!(deliver(&mut app, &inv, Job::Saved(Ok(accepted.clone()), pending, reload, response))["status"], "ok");
        let r = app.ott.as_ref().unwrap();
        let provider = r.provider;
        let expected = r.classic_preferences();
        let [key, _] = r.osd_setting_storage_keys(id).unwrap();
        assert_eq!(r.osd_setting_value(id).as_deref(), Some("0"));
        let mut visible = accepted;
        visible.settings.insert(key.clone(), json!(1));
        app.ott.as_ref().unwrap().tx.send(Job::Uncertain(Some(visible), None))
            .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
        runtime_tick(&mut app);
        let r = app.ott.as_mut().unwrap();
        assert!(r.persistence_uncertain);
        assert!(!r.loaded);
        assert_eq!(r.saved.settings[&key], 1, "the uncertain image remains available for reconciliation");
        assert_eq!(r.classic_preferences(), expected);
        assert_eq!(r.view().classic_preferences, expected);
        // Test source changes directly as well as the current admission fence:
        // no provider or same-provider slot can borrow an unverified flag.
        for other in [ProviderKind::M3u, ProviderKind::Xtream, ProviderKind::Stalker,
            ProviderKind::VPortal, ProviderKind::Local, ProviderKind::Plex] {
            r.provider = other;
            assert_eq!(r.osd_setting_value(id).as_deref(), Some(if other == provider { "0" } else { "1" }));
        }
        r.provider = provider;
        r.saved.profiles.active_profile = Some("another-slot".into());
        assert_eq!(r.classic_preferences(), expected);
        assert!(r.osd_setting_token(id).is_none(), "uncertainty still refuses another setting write");
    }
}

#[test]
fn classic_visibility_authoritative_settings_survive_items_uncertainty_and_provider_restore() {
    let _serial = plx_base::testlock::serial();
    let mut app = setup();
    let mut saved = app.ott.as_ref().unwrap().saved.clone();
    saved.settings.insert("provider:m3u:showNumber".into(), json!(0));
    saved.settings.insert("provider:plex:showName".into(), json!(0));
    app.ott.as_mut().unwrap().provider = ProviderKind::Plex;
    app.ott.as_ref().unwrap().tx.send(Job::LoadedItemsUncertain(saved, [1; 16]))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    let r = app.ott.as_mut().unwrap();
    assert!(r.persistence_uncertain);
    assert_eq!(r.provider, ProviderKind::Plex, "a load cannot replace accepted browse navigation");
    r.provider = ProviderKind::M3u;
    assert!(!r.classic_preferences().show_number());
    assert!(r.classic_preferences().show_name());
    r.provider = ProviderKind::Plex;
    assert!(r.classic_preferences().show_number());
    assert!(!r.classic_preferences().show_name());
}

#[test]
fn progress_and_programme_flags_load_independently_into_the_accepted_snapshot() {
    let _serial = plx_base::testlock::serial();
    for program in 0..=1 {
        for progress in 0..=1 {
            let mut app = setup();
            let r = app.ott.as_mut().unwrap();
            r.provider = ProviderKind::M3u;
            let mut candidate = r.saved.clone();
            candidate.settings.insert("selectedProvider".into(), json!("m3u"));
            candidate.settings.insert(r.osd_setting_storage_keys("showProgram").unwrap()[0].clone(), json!(program));
            candidate.settings.insert(r.osd_setting_storage_keys("showProgress").unwrap()[0].clone(), json!(progress));
            r.tx.send(Job::Loaded(Ok((candidate, [1; 16])))).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
            runtime_tick(&mut app);
            let r = app.ott.as_ref().unwrap();
            assert_eq!(r.provider, ProviderKind::M3u);
            assert_eq!(r.classic_preferences().show_program(), program == 1);
            assert_eq!(r.classic_preferences().show_progress(), progress == 1);
            assert_eq!(r.view().classic_preferences, r.classic_preferences());
        }
    }
}

#[test]
fn progress_saved_for_provider_a_publishes_without_changing_current_provider_b() {
    let _serial = plx_base::testlock::serial();
    let mut app = setup();
    app.ott.as_mut().unwrap().provider = ProviderKind::M3u;
    let (inv, candidate, job) = stage(&mut app, "showProgress", 0);
    let Job::Saved(_, pending, reload, response) = job else { unreachable!() };
    app.ott.as_mut().unwrap().provider = ProviderKind::Plex;
    assert_eq!(deliver(&mut app, &inv, Job::Saved(Ok(candidate), pending, reload, response))["status"], "ok");
    let r = app.ott.as_mut().unwrap();
    assert_eq!(r.provider, ProviderKind::Plex);
    assert!(r.classic_preferences().show_progress());
    assert!(r.classic_preferences().show_program());
    r.provider = ProviderKind::M3u;
    assert!(!r.classic_preferences().show_progress());
    assert!(r.classic_preferences().show_program());
}
