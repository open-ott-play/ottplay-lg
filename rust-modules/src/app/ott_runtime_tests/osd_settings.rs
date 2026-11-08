//! Actual command admission; revoked worker permits prohibit storage IO.
//! Saved/Loaded/Uncertain below model results, not disk durability or rendered pixels.
use super::*;

fn setup() -> App {
    let mut app = app_fixture();
    let r = app.ott.as_mut().unwrap();
    r.loaded = true;
    r.entropy = Some([1; 16]);
    r.write_permit.revoke();
    r.catalog_done.store(false, Ordering::Release);
    r.metadata_done.store(false, Ordering::Release);
    r.resolver_busy = true;
    r.plex_refresh_pending = true;
    r.continuity.stop(r.generation);
    r.navigation_epoch = 1;
    app
}
fn invocation(app: &mut App, serial: u64, params: Value) -> Invocation {
    let r = app.ott.as_mut().unwrap();
    let now = r.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{serial:032x}"),"action":"player_settings","params":params,"expires_at":1020
    }]}).to_string();
    let mut requests = r.remote.raw_poll_for_test(&body, now);
    assert_eq!(requests.len(), 1);
    requests.pop().unwrap()
}
fn dispatch(app: &mut App, inv: &Invocation) -> Option<Value> {
    let mut r = app.ott.take().unwrap();
    let result = commands::command(&mut r, app, inv);
    app.ott = Some(r);
    result
}
fn get(app: &mut App, serial: u64, id: &str) -> Value {
    let token = app.ott.as_ref().unwrap().remote.runtime().to_owned();
    let inv = invocation(app,serial,json!({"op":"get","runtime":token,"id":id}));
    let result = dispatch(app,&inv).unwrap();
    assert_eq!(result["status"],"ok");
    result["data"].clone()
}
fn params(get: &Value, value: Value) -> Value {
    json!({"op":"set","runtime":get["runtime"],"id":get["setting"]["id"],
        "context":get["context"]["token"],"revision":get["setting"]["revision"],"value":value})
}
fn displayed(r: &Runtime) -> (u8,u64) {
    let (opacity,clock) = r.osd_preferences();
    (opacity.level(),clock.index())
}
fn stage(app: &mut App, id: &str, value: u64) -> (Invocation,Saved,Job) {
    let before = get(app,1,id);
    let inv = invocation(app,2,params(&before,json!(value)));
    let r = app.ott.as_ref().unwrap();
    let old = serde_json::to_value(&r.saved).unwrap();
    let mut candidate = r.saved.clone();
    candidate.settings.insert(id.into(),json!(value));
    let stamp: String = plx_base::sha256::sha256(json!(inv.id).to_string().as_bytes())
        .iter().map(|b|format!("{b:02x}")).collect();
    candidate.settings.insert(format!("settingRevision:{id}"),json!(stamp));
    assert!(dispatch(app,&inv).is_none(),"set must await durable completion");
    let r = app.ott.as_ref().unwrap();
    assert!(r.saving);
    assert_eq!(serde_json::to_value(&r.saved).unwrap(),old);
    assert_eq!(displayed(r),(7,0));
    assert!(r.remote.response_for_id_for_test(&inv.id).is_none());
    let job = r.rx.recv_timeout(Duration::from_secs(5)).expect("revoked writer completion");
    assert!(matches!(&job,Job::Saved(Err(_),Some(_),false,_)));
    (inv,candidate,job)
}
fn deliver(app: &mut App, inv: &Invocation, job: Job) -> Value {
    app.ott.as_ref().unwrap().tx.send(job).unwrap_or_else(|_|panic!("fixture receiver disappeared"));
    runtime_tick(app);
    app.ott.as_ref().unwrap().remote.response_for_id_for_test(&inv.id).expect("addressed result")
}

#[test]
fn osd_schema_and_get_append_typed_defaults_without_reordering_existing_fields() {
    let _serial = plx_base::testlock::serial();
    let mut app = setup();
    let token = app.ott.as_ref().unwrap().remote.runtime().to_owned();
    let mut ids = Vec::new();
    let mut cursor = Value::Null;
    for serial in 10..13 {
        let mut p = json!({"op":"schema","runtime":token,"limit":20});
        if !cursor.is_null() { p["cursor"] = cursor.clone(); }
        let inv = invocation(&mut app,serial,p);
        let response = dispatch(&mut app,&inv).unwrap();
        assert_eq!(response["status"],"ok");
        ids.extend(response["data"]["settings"].as_array().unwrap().iter()
            .map(|row|row["id"].as_str().unwrap().to_owned()));
        cursor = response["data"]["next_cursor"].clone();
        if cursor.is_null() { break; }
    }
    assert_eq!(ids.len(),41);
    assert_eq!(ids[40], "uiColorConfig");
    assert_eq!(&ids[23..40],&["nativeSubtitleSize","nativeSubtitlePosition","nativeSubtitleTone","volumeStep","sleepTimeout","infoTimeout","osdOpacity","permanentTime","pageSize","listPosition","seek13Duration","arrowFun", "showNumber", "showName", "showProgram", "showDescription", "showProgress"]);
    for (serial,id,default,max) in [(20,"osdOpacity",7,10),(21,"permanentTime",0,2)] {
        let data = get(&mut app,serial,id);
        assert_eq!(data["descriptor"]["type"],"integer");
        assert_eq!(data["descriptor"]["scope"],"application");
        assert_eq!(data["descriptor"]["access"],json!({"read":true,"write":true}));
        assert_eq!(data["descriptor"]["default_value"],default);
        assert_eq!(data["descriptor"]["constraints"],json!({"minimum":0,"maximum":max}));
        assert_eq!(data["setting"]["value"],default);
        assert_eq!(data["setting"]["effective_value"],default);
        assert_eq!(data["setting"]["storage"],"default");
    }
    assert_eq!(displayed(app.ott.as_ref().unwrap()),(7,0));
}

#[test]
fn osd_wire_invalid_values_do_not_consume_revision_or_start_a_write() {
    let _serial = plx_base::testlock::serial();
    for (id,max) in [("osdOpacity",10),("permanentTime",2)] {
        let mut app = setup();
        let before = get(&mut app,1,id);
        let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        for (i,value) in [json!(-1),json!(max+1),json!(1.0),json!(true),json!("1"),Value::Null,json!(u64::MAX)].into_iter().enumerate() {
            let inv = invocation(&mut app,10+i as u64,params(&before,value));
            assert_eq!(dispatch(&mut app,&inv).unwrap()["data"]["reason"],"invalid_value");
            let r = app.ott.as_ref().unwrap();
            assert_eq!(serde_json::to_value(&r.saved).unwrap(),saved);
            assert!(!r.saving);
            assert!(r.rx.try_recv().is_err());
        }
        let inv = invocation(&mut app,99,params(&before,json!(1)));
        assert!(dispatch(&mut app,&inv).is_none(),"original CAS remains usable");
        assert!(matches!(app.ott.as_ref().unwrap().rx.recv_timeout(Duration::from_secs(5)).unwrap(),Job::Saved(Err(_),Some(_),false,_)));
    }
}

#[test]
fn osd_saved_then_loaded_applies_boundaries_without_changing_source_or_navigation() {
    let _serial = plx_base::testlock::serial();
    for (id,value,expected) in [("osdOpacity",0,(0,0)),("osdOpacity",10,(10,0)),("permanentTime",1,(7,1)),("permanentTime",2,(7,2))] {
        let mut app = setup();
        let r = app.ott.as_ref().unwrap();
        let before = (r.provider,r.playback_epoch,r.navigation_epoch,r.saved.profiles.active_profile.clone());
        let (inv,candidate,job) = stage(&mut app,id,value);
        let Job::Saved(_,pending,reload,reply) = job else { unreachable!() };
        assert!(!reload);
        let response = deliver(&mut app,&inv,Job::Saved(Ok(candidate.clone()),pending,reload,reply));
        assert_eq!(response["status"],"ok");
        assert_eq!(response["data"]["persisted"],true);
        let fresh = get(&mut app,3,id);
        assert_eq!(fresh["setting"]["persisted_value"],value);
        assert_eq!(fresh["setting"]["effective_value"],value);
        let r = app.ott.as_ref().unwrap();
        assert_eq!(displayed(r),expected);
        assert_eq!((r.provider,r.playback_epoch,r.navigation_epoch,r.saved.profiles.active_profile.clone()),before);
        drop(app);
        let mut reopened = setup();
        reopened.ott.as_mut().unwrap().loaded = false;
        reopened.ott.as_ref().unwrap().tx.send(Job::Loaded(Ok((candidate,[1;16])))).unwrap_or_else(|_|panic!("fixture receiver disappeared"));
        runtime_tick(&mut reopened);
        assert_eq!(displayed(reopened.ott.as_ref().unwrap()),expected);
        assert_eq!(get(&mut reopened,4,id)["setting"]["persisted_value"],value);
    }
}

#[test]
fn osd_failed_and_uncertain_visible_images_never_publish_unconfirmed_display_values() {
    let _serial = plx_base::testlock::serial();
    for id in ["osdOpacity","permanentTime"] {
        for outcome in 0..3 {
            let mut app = setup();
            let (inv,candidate,job) = stage(&mut app,id,1);
            let completion = match outcome {
                0 => job,
                1 => Job::Uncertain(None,Some(inv.clone())),
                _ => Job::Uncertain(Some(candidate),Some(inv.clone())),
            };
            let response = deliver(&mut app,&inv,completion);
            assert_eq!(response["status"],"rejected");
            assert_eq!(response["data"]["reason"],"storage_failed");
            let r = app.ott.as_ref().unwrap();
            assert_eq!(displayed(r),(7,0),"a visible uncertain image is not accepted settings");
            assert!(!r.saving);
            assert_eq!(r.persistence_uncertain,outcome!=0);
            if outcome!=0 {
                assert!(!r.loaded);
                let token = r.remote.runtime().to_owned();
                let read = invocation(&mut app,3,json!({"op":"get","runtime":token,"id":id}));
                assert_eq!(dispatch(&mut app,&read).unwrap()["data"]["reason"],"revision_conflict");
            }
        }
    }
}

#[test]
fn osd_local_drafts_are_atomic_and_retire_wire_and_editor_tokens_across_aba() {
    let _serial = plx_base::testlock::serial();
    for (id,default,valid,invalid) in [("osdOpacity","7","10","11"),("permanentTime","0","2","3")] {
        let mut app = setup();
        let old_get = get(&mut app,1,id);
        let r = app.ott.as_mut().unwrap();
        let original = serde_json::to_value(&r.saved).unwrap();
        let token = r.osd_setting_token(id).unwrap();
        assert_eq!(r.osd_setting_value(id).as_deref(),Some(default));
        for text in ["","-1","+1","1.0","1e0","12x","999999999999999999999999",invalid] {
            assert!(r.osd_setting_candidate(id,text,1).is_err());
            assert_eq!(serde_json::to_value(&r.saved).unwrap(),original);
            assert!(!r.saving);
        }
        let candidate = r.osd_setting_candidate(id,valid,1).unwrap();
        assert_eq!(serde_json::to_value(&r.saved).unwrap(),original);
        r.saved = candidate;
        r.saved = r.osd_setting_candidate(id,default,2).unwrap();
        assert_ne!(r.osd_setting_token(id).unwrap(),token);
        assert_eq!(displayed(r),(7,0),"candidate construction is never display publication");
        let stale = invocation(&mut app,2,params(&old_get,json!(1)));
        assert_eq!(dispatch(&mut app,&stale).unwrap()["data"]["reason"],"revision_conflict");
    }
}

#[test]
fn osd_malformed_storage_reports_invalid_but_does_not_replace_calibrated_defaults() {
    let _serial = plx_base::testlock::serial();
    let mut app = setup();
    app.ott.as_mut().unwrap().saved.settings.insert("osdOpacity".into(),json!(11));
    app.ott.as_mut().unwrap().saved.settings.insert("permanentTime".into(),json!("2"));
    for (serial,id,default) in [(1,"osdOpacity",7),(2,"permanentTime",0)] {
        let data = get(&mut app,serial,id);
        assert_eq!(data["setting"]["storage"],"invalid");
        assert_eq!(data["setting"]["value"],default);
        assert_eq!(data["setting"]["effective_value"],default);
        assert!(data["setting"].get("persisted_value").is_none());
    }
    assert_eq!(displayed(app.ott.as_ref().unwrap()),(7,0));
}
