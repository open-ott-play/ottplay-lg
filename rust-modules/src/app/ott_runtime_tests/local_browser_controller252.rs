//! Additional Controller 252 boundaries. Uses actual raw delivery/dispatch and
//! the existing temporary-directory worker fixture; no decoder or storage IO.
use super::*;

fn dispatch(app: &mut App, id: u64, action: &str, params: Value) -> Value {
    let mut r = app.ott.take().unwrap();
    let inv = invocation(&mut r, id, action, params);
    assert!(r.remote.invocation_is_current(&inv));
    assert!(inv.deadline_ms > r.now_ms());
    let reply = commands::command(&mut r, app, &inv).expect("synchronous boundary");
    r.remote.complete(inv.clone(), reply);
    let reply = r.remote.response_for_id_for_test(&inv.id).unwrap();
    app.ott = Some(r);
    reply
}

#[test]
fn controller252_capability_probe_is_closed_and_true_only() {
    let _serial = plx_base::testlock::serial();
    let mut app = ready();
    let before = state(app.ott.as_ref().unwrap());
    let flags = [
        "local_media_session",
        "local_media_progressive",
        "local_media_handles",
    ];
    let mut id = 100;
    for mask in 0..8 {
        let mut params = json!({});
        for (index, flag) in flags.iter().enumerate() {
            if mask & (1 << index) != 0 {
                params[*flag] = json!(true);
            }
        }
        let response = dispatch(&mut app, id, "capabilities", params);
        id += 1;
        assert_eq!(response["status"], "ok");
        assert_eq!(
            response["data"]["local_browser"],
            json!({
                "version":1,"operations":["roots","list","more","select"],"max_entries":64
            })
        );
        assert_eq!(
            response["data"]["local_media_handles"],
            json!({
                "version":1,"operations":["connect","play","shuffle"]
            })
        );
    }
    for flag in flags {
        for invalid in [json!(false), Value::Null, json!(1), json!("true")] {
            let mut params = json!({});
            params[flag] = invalid;
            assert_eq!(
                dispatch(&mut app, id, "capabilities", params)["status"],
                "rejected"
            );
            id += 1;
        }
    }
    assert_eq!(
        dispatch(&mut app, id, "capabilities", json!({"local_browser":true}))["status"],
        "rejected"
    );
    assert_eq!(state(app.ott.as_ref().unwrap()), before);
    assert!(app.ott.as_ref().unwrap().local_inventory.sources.is_empty());
}

#[test]
fn controller252_handle_xor_and_extensions_reject_before_native_or_saved_effects() {
    let _serial = plx_base::testlock::serial();
    let _reset = NativeReset;
    let disk = Disk::new();
    let mut app = ready();
    let selection = selected(&mut app, &disk);
    let handle = selection["data"]["source"]["handle"].as_str().unwrap();
    super::super::local_integration::install(&mut app, None, "already-playing");
    let native = plx_media::route::playback_identity(&app.player.session);
    let ticket = plx_media::route::play_request_ticket();
    let before = state(app.ott.as_ref().unwrap());
    let cancel = app.ott.as_ref().unwrap().cancel.clone();
    let source = app.ott.as_ref().unwrap().usb_remote_handle(handle).unwrap();
    let mut id = 100;
    for op in ["connect", "play", "shuffle"] {
        let mut invalid = vec![
            json!({"source":"Movies"}),
            json!({"source_handle":null}),
            json!({"source_handle":false}),
            json!({"path":"/untrusted"}),
        ];
        if op == "connect" {
            invalid.extend([json!({"session":true}), json!({"progressive":true})]);
        } else {
            invalid.extend([
                json!({"session":false}),
                json!({"progressive":false}),
                json!({"session":null}),
                json!({"progressive":"true"}),
            ]);
        }
        for extra in invalid {
            let mut params = json!({"runtime":"browser-fixture","op":op,"source_handle":handle});
            params
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let response = dispatch(&mut app, id, "local_media", params);
            id += 1;
            assert_eq!(response["status"], "rejected");
            assert_eq!(
                response["data"],
                json!({"version":1,"runtime":"browser-fixture","op":op,
                "state":"error","error":"invalid_request"})
            );
        }
    }
    let r = app.ott.as_ref().unwrap();
    assert_eq!(state(r), before);
    assert!(Arc::ptr_eq(&r.cancel, &cancel));
    assert!(!cancel.load(Ordering::Acquire));
    assert!(r.usb_remote_handle(handle).unwrap().same_grant(&source));
    assert!(r.transient.candidate.is_none() && r.play_target.is_none() && r.pending_play.is_none());
    assert!(!r.usb_remote_waiting_result());
    assert_eq!(
        plx_media::route::playback_identity(&app.player.session),
        native
    );
    assert!(plx_media::route::play_request_is_current(ticket));
}

#[test]
fn controller252_full_delivery_envelope_bounds_escaped_display_names_and_cursor() {
    let _serial = plx_base::testlock::serial();
    let disk = Disk::new();
    for n in 0..130 {
        // Below the filesystem's component limit; JSON escaping almost doubles
        // these valid display bytes. No display label is used as an entry ID.
        std::fs::create_dir(disk.0.join(format!("{n:03}-{}", "\"".repeat(240)))).unwrap();
    }
    let mut app = ready();
    let roots = browse(
        &mut app,
        &disk,
        1,
        json!({"version":1,"runtime":"browser-fixture","op":"roots"}),
    );
    let first = browse(
        &mut app,
        &disk,
        2,
        json!({"version":1,"runtime":"browser-fixture","op":"list",
        "generation":roots["data"]["generation"],"entry_id":roots["data"]["entries"][0]["id"]}),
    );
    let encoded = serde_json::to_vec(&first).unwrap();
    assert_eq!(first["id"], format!("{:032x}", 2));
    assert!(
        encoded.len() > 32 * 1024,
        "exercise escaped bytes, not short-label sizing"
    );
    assert!(
        encoded.len() <= 64 * 1024,
        "limit includes id, status and data"
    );
    let rows = first["data"]["entries"].as_array().unwrap();
    assert_eq!(rows.len(), 64);
    assert_eq!(rows[0]["name"], format!("000-{}", "\"".repeat(240)));
    assert!(rows.iter().all(|row| row["id"]
        .as_str()
        .unwrap()
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))));
    let second = browse(
        &mut app,
        &disk,
        3,
        json!({"version":1,"runtime":"browser-fixture","op":"list",
        "generation":first["data"]["generation"],"cursor":first["data"]["next_cursor"]}),
    );
    assert_ne!(second["data"]["generation"], first["data"]["generation"]);
    assert!(serde_json::to_vec(&second).unwrap().len() <= 64 * 1024);
    let stale = dispatch(
        &mut app,
        4,
        "local_browser",
        json!({"version":1,"runtime":"browser-fixture","op":"list",
        "generation":first["data"]["generation"],"cursor":first["data"]["next_cursor"]}),
    );
    assert_eq!(stale["data"]["error"], "stale");
}

#[test]
fn controller252_navigation_and_standby_retire_completed_handle_verification() {
    let _serial = plx_base::testlock::serial();
    for standby in [false, true] {
        let _reset = NativeReset;
        let disk = Disk::new();
        let mut app = ready();
        let selection = selected(&mut app, &disk);
        let handle = selection["data"]["source"]["handle"].as_str().unwrap();
        super::super::local_integration::install(&mut app, None, "previous-native");
        let native = plx_media::route::playback_identity(&app.player.session);
        let ticket = plx_media::route::play_request_ticket();
        let mut r = app.ott.take().unwrap();
        let source = r.usb_remote_handle(handle).unwrap();
        let inv = invocation(
            &mut r,
            10,
            "local_media",
            json!({"runtime":"browser-fixture","op":"play","source_handle":handle,"session":true}),
        );
        r.usb_handle_prepare_for_test(&inv, source.clone()).unwrap();
        let ott_catalog::Task::UsbRemote {
            serial,
            task,
            cancel,
        } = r.take_usb_remote_task().unwrap()
        else {
            panic!("verification task")
        };
        let result = disk.execute(task, &cancel);
        assert!(matches!(
            &result,
            Ok(crate::app::ott_usb::Answer::Verified(_))
        ));
        if standby {
            app.ott_standby = true;
            r.suspend_source_work(&mut app);
            assert!(!r.source_work_allowed());
        } else {
            r.navigation_epoch = r.navigation_epoch.wrapping_add(1);
        }
        let after_new_intent = state(&r);
        r.usb_remote_result(&mut app, serial, result);
        let response = r.remote.response_for_id_for_test(&inv.id).unwrap();
        assert_eq!(response["status"], "rejected");
        assert_eq!(response["data"]["runtime"], "browser-fixture");
        assert_eq!(response["data"]["error"], "runtime_changed");
        assert_eq!(state(&r), after_new_intent);
        assert!(
            r.transient.candidate.is_none() && r.play_target.is_none() && r.pending_play.is_none()
        );
        assert!(r.usb_remote_handle(handle).unwrap().same_grant(&source));
        assert_eq!(
            plx_media::route::playback_identity(&app.player.session),
            native
        );
        assert!(plx_media::route::play_request_is_current(ticket));
        app.ott = Some(r);
    }
}

#[test]
fn controller252_restart_rejects_old_handle_even_with_its_live_source_retained() {
    let _serial = plx_base::testlock::serial();
    let disk = Disk::new();
    let mut app = ready();
    let selection = selected(&mut app, &disk);
    let handle = selection["data"]["source"]["handle"].as_str().unwrap();
    let source = app.ott.as_ref().unwrap().usb_remote_handle(handle).unwrap();
    app.ott.as_mut().unwrap().remote =
        Remote::disabled_for_test("dev_browser", "restarted-runtime");
    let before = state(app.ott.as_ref().unwrap());
    let old = dispatch(
        &mut app,
        10,
        "local_media",
        json!({"runtime":"browser-fixture","op":"play","source_handle":handle}),
    );
    assert_eq!(
        old["data"],
        json!({"version":1,"runtime":"browser-fixture","op":"play","state":"error","error":"runtime_changed"})
    );
    let current = dispatch(
        &mut app,
        11,
        "local_media",
        json!({"runtime":"restarted-runtime","op":"shuffle","source_handle":handle}),
    );
    assert_eq!(
        current["data"],
        json!({"version":1,"runtime":"restarted-runtime","op":"shuffle","state":"error","error":"source_not_found"})
    );
    let r = app.ott.as_ref().unwrap();
    assert_eq!(state(r), before);
    assert!(
        source.is_live()
            && r.local_inventory
                .sources
                .iter()
                .any(|s| s.same_grant(&source))
    );
    assert!(r.transient.candidate.is_none() && !r.usb_remote_waiting_result());
}
