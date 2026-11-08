//! Actual command/content admission; modelled successful Saved acknowledgements
//! are explicitly distinct from the real revoked-writer failure exercised here.
use super::*;

fn get_geometry(app: &mut App, id: &str) -> Value {
    app.ott.as_mut().unwrap().entropy = Some([1; 16]);
    let response = setting(app, json!({"op":"get","runtime":"fixture","id":id})).unwrap();
    assert_eq!(response["status"], "ok", "{response}");
    response
}

fn set_geometry(read: &Value, value: Value) -> Value {
    json!({"op":"set","runtime":"fixture","id":read["data"]["setting"]["id"],"value":value,
        "context":read["data"]["context"]["token"],"revision":read["data"]["setting"]["revision"]})
}

fn remote_failure(app: &mut App) -> Job {
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.saving);
    assert!(!runtime.write_permit.is_valid());
    let result = runtime.rx.recv_timeout(Duration::from_secs(5)).expect("revoked writer must finish without IO");
    assert!(matches!(&result, Job::Saved(Err("Could not prepare protected OTT settings for saving"), Some(_), false, _)));
    result
}

#[test]
fn geometry_schema_get_set_uses_actual_saved_completion_and_shared_native_consumer() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture_with_subtitle(true);
    app.ott.as_mut().unwrap().entropy = Some([1; 16]);
    let first = setting(&mut app, json!({"op":"schema","runtime":"fixture"})).unwrap();
    assert_eq!(first["status"], "ok");
    let second = setting(&mut app, json!({"op":"schema","runtime":"fixture","cursor":first["data"]["next_cursor"]})).unwrap();
    assert_eq!(second["status"], "ok");
    let ids: Vec<_> = second["data"]["settings"].as_array().unwrap().iter().map(|entry| entry["id"].as_str().unwrap()).collect();
    // Other settings may follow geometry; each geometry field must still appear once, in order.
    let geometry: Vec<_> = ids.into_iter().filter(|id| matches!(*id,
        "nativeSubtitleSize" | "nativeSubtitlePosition" | "nativeSubtitleTone"
    )).collect();
    assert_eq!(geometry, ["nativeSubtitleSize", "nativeSubtitlePosition", "nativeSubtitleTone"]);
    let owner = TrackCommitSource::capture(&app.player.session);
    for (id, value, edit) in [
        ("nativeSubtitleSize", json!("large"), subtitle_settings::Edit::Size(SubtitleSize::Large)),
        ("nativeSubtitlePosition", json!("high"), subtitle_settings::Edit::Position(SubtitlePosition::High)),
        ("nativeSubtitleTone", json!("grey_85"), subtitle_settings::Edit::Tone(SubtitleTone::Silver)),
    ] {
        let read = get_geometry(&mut app, id);
        assert_eq!(read["data"]["setting"]["storage"], "default");
        let saved_before = app.ott.as_ref().unwrap().saved.settings.clone();
        let live_before = route::native_subtitle_preferences(&app.player.session).unwrap();
        let candidate = edit.candidate(&app.ott.as_ref().unwrap().saved).unwrap();
        assert!(setting(&mut app, set_geometry(&read, value.clone())).is_none());
        assert_eq!(app.ott.as_ref().unwrap().saved.settings, saved_before);
        assert_eq!(route::native_subtitle_preferences(&app.player.session).unwrap(), live_before);
        // Drain the real worker before substituting only its terminal outcome.
        // This tests publication/routing, not a physical encrypted disk write.
        let Job::Saved(_, invocation, reload, reply) = remote_failure(&mut app) else { unreachable!() };
        app.ott.as_ref().unwrap().tx.send(Job::Saved(Ok(candidate), invocation, reload, reply)).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
        runtime_tick(&mut app);
        assert!(!app.ott.as_ref().unwrap().saving);
        let after = get_geometry(&mut app, id);
        assert_eq!(after["data"]["setting"]["persisted_value"], value);
        assert_eq!(after["data"]["setting"]["effective_value"], value);
        assert!(owner.matches(&app.player.session), "settings must not restart the source");
        assert_eq!(player::state(&app.player.session), PlaybackState::Paused);
    }
    assert_eq!(route::native_subtitle_preferences(&app.player.session).unwrap(), NativeSubtitlePreferences {
        size: SubtitleSize::Large, position: SubtitlePosition::High, tone: SubtitleTone::Silver,
    });
}

#[test]
fn absent_geometry_reports_inherited_effective_value_and_no_fake_plex_effective_value() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture();
    route::restore_native_subtitle_preferences(&mut app.player.session, NativeSubtitlePreferences {
        size: SubtitleSize::Huge, position: SubtitlePosition::High, tone: SubtitleTone::Charcoal,
    });
    for (id, expected) in [("nativeSubtitleSize", "huge"), ("nativeSubtitlePosition", "high"), ("nativeSubtitleTone", "grey_28")] {
        let read = get_geometry(&mut app, id);
        assert_eq!(read["data"]["setting"]["storage"], "default");
        assert_eq!(read["data"]["setting"]["effective_value"], expected);
        assert!(read["data"]["setting"].get("persisted_value").is_none());
    }
    route::forget_external_source(&mut app.player.session);
    let read = get_geometry(&mut app, "nativeSubtitleSize");
    assert!(read["data"]["setting"].get("effective_value").is_none());
}

#[test]
fn geometry_invalid_values_and_corrupt_shared_state_cannot_start_a_write() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture();
    for id in ["nativeSubtitleSize", "nativeSubtitlePosition", "nativeSubtitleTone"] {
        for invalid in [json!(1), json!(false), Value::Null, json!("unsupported"), json!({})] {
            let read = get_geometry(&mut app, id);
            let response = setting(&mut app, set_geometry(&read, invalid)).unwrap();
            assert_eq!(response["data"]["reason"], "invalid_value");
            assert!(!app.ott.as_ref().unwrap().saving);
        }
    }
    app.ott.as_mut().unwrap().saved.settings.insert("nativeSubtitleAppearance".into(), json!({"font":"unsupported"}));
    let read = get_geometry(&mut app, "nativeSubtitleSize");
    let response = setting(&mut app, set_geometry(&read, json!("large"))).unwrap();
    assert_eq!(response["data"]["reason"], "storage_failed");
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn failed_geometry_writes_publish_neither_live_values_nor_shared_revision() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture();
    for (id, value) in [("nativeSubtitleSize", "large"), ("nativeSubtitlePosition", "high"), ("nativeSubtitleTone", "grey_85")] {
        let read = get_geometry(&mut app, id);
        let before = app.ott.as_ref().unwrap().saved.settings.clone();
        let live = route::native_subtitle_preferences(&app.player.session).unwrap();
        assert!(setting(&mut app, set_geometry(&read, json!(value))).is_none());
        let failure = remote_failure(&mut app);
        app.ott.as_ref().unwrap().tx.send(failure).unwrap_or_else(|_| panic!("fixture receiver disappeared"));
        runtime_tick(&mut app);
        assert!(!app.ott.as_ref().unwrap().saving);
        assert_eq!(app.ott.as_ref().unwrap().saved.settings, before);
        assert_eq!(route::native_subtitle_preferences(&app.player.session).unwrap(), live);
    }
}

#[test]
fn addressed_native_ui_a_b_a_invalidates_every_geometry_alias_receipt() {
    let _serial = plx_base::testlock::serial();
    let _globals = StyleGlobals::capture();
    let mut app = style_fixture_with_subtitle(true);
    app.ott.as_mut().unwrap().saved = subtitle_settings::Edit::Size(SubtitleSize::Medium)
        .candidate(&app.ott.as_ref().unwrap().saved).unwrap();
    let reads: Vec<_> = ["nativeSubtitleSize", "nativeSubtitlePosition", "nativeSubtitleTone"].into_iter()
        .map(|id| get_geometry(&mut app, id)).collect();
    for size in [SubtitleSize::Large, SubtitleSize::Medium] {
        content_open_tracks(&mut app);
        for row in [TrackRow::Style, TrackRow::OpenField(StyleField::Size)] {
            assert!(!content_hit_track_row(&mut app, row).iter().any(|request|
                matches!(request, PlayerReq::CommitTrack(_) | PlayerReq::CommitTrackAddressed(_))));
        }
        let requests = content_hit_track_row(&mut app, TrackRow::Choice(StyleField::Size, usize::from(size.index())));
        let mut requests = requests.into_iter().filter_map(|request| match request {
            PlayerReq::CommitTrackAddressed(request) => Some(request),
            PlayerReq::CommitTrack(_) => panic!("real UI must use addressed persistence"),
            _ => None,
        });
        let request = requests.next().expect("real Size choice must emit an addressed edit");
        assert!(requests.next().is_none());
        assert!(matches!(&request.commit, TrackCommit::SubtitleSize(value) if *value == size));
        let candidate = subtitle_settings::Edit::Size(size).candidate(&app.ott.as_ref().unwrap().saved).unwrap();
        through_content(&mut app, PlayerReq::CommitTrackAddressed(request));
        drop(capture_revoked_worker(&mut app));
        deliver_model_saved(&mut app, candidate);
        assert_eq!(live_size(&app), size);
        content_close_tracks(&mut app);
    }
    for (read, value) in reads.iter().zip(["large", "high", "grey_85"]) {
        let response = setting(&mut app, set_geometry(read, json!(value))).unwrap();
        assert_eq!(response["data"]["reason"], "revision_conflict");
        assert!(!app.ott.as_ref().unwrap().saving);
    }
}
