//! Real UI writer and typed remote admission share the same protected revision.
use super::*;

fn setting(app: &mut App, params: Value) -> Option<Value> {
    let mut runtime = app.ott.take().unwrap();
    let mut remote = Remote::disabled_for_test("dev_fixture", "fixture");
    let mut request = remote.request_for_test("player_settings", params);
    request.deadline_ms = runtime.now_ms().saturating_add(10_000);
    let response = commands::command(&mut runtime, app, &request);
    app.ott = Some(runtime);
    response
}

fn read(app: &mut App) -> Value {
    app.ott.as_mut().unwrap().entropy = Some([1; 16]);
    let response = setting(
        app,
        json!({"op":"get","runtime":"fixture","id":"nativeSubtitleColor"}),
    )
    .unwrap();
    assert_eq!(response["status"], "ok");
    response
}

fn change(read: &Value) -> Value {
    json!({"op":"set","runtime":"fixture","id":"nativeSubtitleColor","value":"cyan",
        "context":read["data"]["context"]["token"],"revision":read["data"]["setting"]["revision"]})
}

#[test]
fn real_ui_a_b_a_invalidates_a_previously_read_remote_alias() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture();
    let a = TextAppearance::default();
    let initial = subtitle_settings::Edit::Appearance(a)
        .candidate(&app.ott.as_ref().unwrap().saved)
        .unwrap();
    app.ott.as_mut().unwrap().saved = initial;
    let before = read(&mut app);
    for wanted in [
        TextAppearance {
            color: TextColor::Yellow,
            ..a
        },
        a,
    ] {
        let candidate = subtitle_settings::Edit::Appearance(wanted)
            .candidate(&app.ott.as_ref().unwrap().saved)
            .unwrap();
        through_content(
            &mut app,
            PlayerReq::CommitTrack(TrackCommit::TextAppearance(wanted)),
        );
        drop(capture_revoked_worker(&mut app));
        // Exercise actual content admission, then model only verified completion.
        deliver_model_saved(&mut app, candidate);
    }
    let response = setting(&mut app, change(&before)).unwrap();
    assert_eq!(response["status"], "rejected");
    assert_eq!(response["data"]["reason"], "revision_conflict");
    assert!(!app.ott.as_ref().unwrap().saving);
    assert_eq!(route::subtitle_text_appearance(&app.player.session), a);
}

#[test]
fn failed_remote_alias_write_publishes_neither_appearance_nor_shared_revision() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture();
    let before = read(&mut app);
    let saved_before = app.ott.as_ref().unwrap().saved.settings.clone();
    let live_before = route::subtitle_text_appearance(&app.player.session);
    assert!(setting(&mut app, change(&before)).is_none());
    assert_eq!(app.ott.as_ref().unwrap().saved.settings, saved_before);
    assert_eq!(
        route::subtitle_text_appearance(&app.player.session),
        live_before
    );
    let refusal = app
        .ott
        .as_ref()
        .unwrap()
        .rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the revoked remote writer must answer without IO");
    assert!(matches!(
        &refusal,
        Job::Saved(
            Err("Could not prepare protected OTT settings for saving"),
            Some(_),
            false,
            _
        )
    ));
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(refusal)
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    assert!(!app.ott.as_ref().unwrap().saving);
    assert_eq!(app.ott.as_ref().unwrap().saved.settings, saved_before);
    assert_eq!(
        route::subtitle_text_appearance(&app.player.session),
        live_before
    );
}

#[path = "ott_subtitle_geometry_remote_tests.rs"]
mod geometry;
