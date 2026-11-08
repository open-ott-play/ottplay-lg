//! Actual callback ingress to the authenticated status reply. The inert Engine
//! supplies ownership only; no device, decoder or producer is started.
use super::*;

fn evidence(reply: &Value) -> &Value {
    reply["data"].get("video_evidence").expect("required additive diagnostic")
}

#[test]
fn video_evidence_reports_permission_without_claiming_visibility() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = fixture();
    assert_eq!(evidence(&read(&mut app, "status", 1)), &json!({
        "version":1, "window_present_allowed":true, "native":null,
    }));
    for (index, (event, allowed)) in [(0x103, false), (0x104, false),
        (0x105, false), (0x106, true)].into_iter().enumerate() {
        app.window_activity.event(event);
        assert_eq!(evidence(&read(&mut app, "state", index as u64 + 2)), &json!({
            "version":1, "window_present_allowed":allowed, "native":null,
        }));
    }
}

#[test]
fn video_evidence_keeps_position_callbacks_and_raw_sink_reports_distinct() {
    use plx_media::player;
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = fixture();
    admit(&mut app, "first");
    let epoch = install_owned_engine(&mut app);
    player::swap_state_for_test(PlaybackState::Playing);
    player::sf_on_event(epoch, 0, 1_000_000_000, std::ptr::null());
    assert_eq!(evidence(&read(&mut app, "status", 1))["native"], json!({
        "position_callbacks":1, "sink_report":null,
    }));
    let major = plx_platform::tv::device::info().major;
    let ty = if major < 5 { 47 } else { 49 };
    // Negative and zero values must survive verbatim, not become absence or a
    // manufactured count of decoded/displayed frames.
    for (index, raw) in [i64::MIN, 0, i64::MAX].into_iter().enumerate() {
        player::sf_on_event(epoch, ty, raw, std::ptr::null());
        let expected = json!({"version":1, "window_present_allowed":true,
            "native":{"position_callbacks":1, "sink_report":{
                "callback_type":ty, "callback_count":index + 1,
                "firmware_major":major, "raw_value":raw.to_string(),
            }}});
        for (offset, params) in [json!({}), json!({"playback_failure_version":2}),
            json!({"playback_failure_version":3})].into_iter().enumerate() {
            let reply = read_params(&mut app, "status", 10 + index as u64 * 3 + offset as u64, params);
            assert_eq!(evidence(&reply), &expected);
            assert!(failure(&reply).is_null());
        }
    }
    // A live native Engine cannot lend its observations to a missing OTT owner.
    let playing = app.ott.as_mut().unwrap().playing.take();
    assert!(evidence(&read(&mut app, "state", 30))["native"].is_null());
    app.ott.as_mut().unwrap().playing = playing;
    // The physical epoch is independently authoritative even with the same
    // logical source and an installed Engine left behind after reset.
    player::SHARED.reset_session();
    player::sf_on_event(epoch, ty, 999, std::ptr::null());
    assert!(evidence(&read(&mut app, "status", 31))["native"].is_null());
}

#[test]
fn video_evidence_does_not_attribute_outgoing_engine_to_pending_request() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = fixture();
    admit(&mut app, "first");
    install_owned_engine(&mut app);
    plx_media::player::swap_state_for_test(PlaybackState::Playing);
    assert!(evidence(&read(&mut app, "status", 1))["native"].is_object());
    let outgoing = app.ott.as_ref().unwrap().playing.as_ref().unwrap().identity.clone();
    queue(&mut app, "second");
    assert!(app.adapters.player.is_live(), "outgoing native Engine still exists");
    assert!(app.ott.as_ref().unwrap().playing.as_ref().unwrap().identity == outgoing);
    assert!(app.ott.as_ref().unwrap().play_target.is_some());
    assert!(evidence(&read(&mut app, "status", 2))["native"].is_null());
}
