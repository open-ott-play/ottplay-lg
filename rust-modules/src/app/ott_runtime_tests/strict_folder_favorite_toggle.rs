//! Strictness-only changes retain the actual current Favorite selection ID.
//! The real command/EOF path runs with modeled writer ACK and native acceptance.
use super::*;

fn favorite_b(app: &mut App, root: &local::BoundSource, ids: &[&str], complete: bool) -> String {
    let mut runtime = app.ott.take().unwrap();
    runtime.catalog.entries = ids.iter().map(|id| row(id)).collect();
    runtime.catalog_complete = complete;
    transient::accepted_catalog_update(&mut runtime);
    let entry = row("B");
    let favorite = selection::capture_bound(
        &runtime.selection_bindings().unwrap(),
        selection::SelectionOwner::LocalRoot(root),
        &entry,
    )
    .unwrap();
    let id = format!("favorite:{}", favorite.identity());
    runtime.saved.favorites.toggle(favorite).unwrap();
    let profiles = runtime.saved.profiles.clone();
    let roots = runtime.local_inventory.sources.clone();
    let bindings = selection::SelectionBindings::new(
        &profiles,
        roots.iter().map(|source| source.descriptor()),
        root.generation,
    )
    .unwrap();
    runtime
        .saved
        .local_favorite_proofs
        .record_capture(
            &bindings,
            &runtime.saved.favorites,
            selection::SelectionOwner::LocalRoot(root),
            &runtime.catalog.entries,
            &entry,
        )
        .unwrap();
    runtime
        .saved
        .settings
        .insert(stored::KEY.into(), policy(root, false).encode().unwrap());
    runtime.play(app, &id).unwrap();
    app.ott = Some(runtime);
    id
}

fn admit_favorite(app: &mut App, resources: &mut AcceptedResources) {
    let mut runtime = app.ott.take().unwrap();
    let request = runtime
        .play_target
        .clone()
        .expect("actual Favorite request");
    assert!(request.id.starts_with("favorite:"));
    let resolver = runtime.resolver_next.as_ref().unwrap();
    let profile = resolver.profile.clone();
    let entry = crate::app::ott_catalog::find_favorite(
        &plx_base::task::OffFrame::for_test(),
        &profile,
        resolver.favorite.as_ref().unwrap(),
        resolver.local_proof.as_ref(),
        &resolver.cancel,
    )
    .unwrap();
    assert!(transient::target_current(&runtime));
    transient::before_native(&mut runtime, app).unwrap();
    runtime
        .rebind_native_admission(app, &profile, &entry)
        .unwrap();
    let source = transient::open_video_source(
        &plx_base::task::OffFrame::for_test(),
        runtime.resolver_next.as_ref().unwrap().local_read.as_ref(),
        &profile,
        &entry,
        &AtomicBool::new(false),
    )
    .unwrap();
    let mut info = unsupported_info();
    info.video_codec = "h264".into();
    info.audio_codec = "aac".into();
    info.audio_channels = 2;
    info.audio_sample_rate = 48_000;
    info.audio_bit_depth = 16;
    let before = plx_media::route::play_request_ticket();
    let _ = runtime.play_ticket.take();
    let _ = runtime.resolver_next.take();
    let generation = runtime.play_generation;
    start_resolved_with(
        &mut runtime,
        app,
        profile,
        entry,
        ReadySource::External(source, info, None),
        resources,
    )
    .unwrap();
    let native = NativeStartReceipt {
        generation,
        before,
        after: plx_media::route::play_request_ticket(),
    };
    if let Some(reply) = transient::finish_native(&mut runtime, app, Ok(()), Some(&native)) {
        assert_eq!(reply["status"], "ok");
        assert_eq!(reply["data"]["state"], "ready");
    }
    let _ = runtime.take_completed_play_target();
    assert_eq!(
        runtime
            .folder_row_for(runtime.playing.as_ref().unwrap())
            .as_deref(),
        Some("B")
    );
    runtime.saving = true;
    app.ott = Some(runtime);
    settle_pages(app);
    assert_owned_player(app, &request.id);
}

fn toggle_strict(app: &mut App) {
    let native = plx_media::route::playback_identity(&app.player.session);
    let mut runtime = app.ott.take().unwrap();
    let selection = runtime.playing.as_ref().unwrap().selection_id.clone();
    let epoch = runtime.playback_epoch;
    let mut candidate = runtime.saved.clone();
    let policy = stored::read(&candidate.settings).unwrap().unwrap();
    assert!(!policy.strict());
    candidate.settings.insert(
        stored::KEY.into(),
        policy.with_strict(true).encode().unwrap(),
    );
    runtime.saving = false; // Only the synthetic writer hold exists.
    runtime.write_permit.revoke();
    let now = runtime.now_ms();
    let raw = json!({"commands":[],"server_time":1000,"requests":[{
        "id":"00000000000000000000000000009002","action":"kiosk",
        "params":{"mode":"on","strict":true},"expires_at":1020
    }]})
    .to_string();
    let mut parsed = runtime.remote.raw_poll_for_test(&raw, now);
    assert_eq!(parsed.len(), 1);
    let invocation = parsed.pop().unwrap();
    assert!(runtime.remote.invocation_is_current(&invocation));
    assert!(commands::command(&mut runtime, app, &invocation).is_none());
    let Job::Saved(Err(_), Some(owned), reload, response) = runtime
        .rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
    else {
        panic!("pre-revoked actual writer must settle before modeled ACK");
    };
    assert_eq!(owned.id, invocation.id);
    runtime
        .tx
        .send(Job::Saved(Ok(candidate), Some(owned), reload, response))
        .unwrap();
    app.ott = Some(runtime);
    runtime_tick(app);
    settle_pages(app);
    let runtime = app.ott.as_mut().unwrap();
    assert!(
        runtime.play_target.is_none(),
        "strictness-only change must not restart playback"
    );
    assert_eq!(runtime.playback_epoch, epoch);
    assert_eq!(runtime.playing.as_ref().unwrap().selection_id, selection);
    assert_eq!(
        plx_media::route::playback_identity(&app.player.session),
        native
    );
    runtime.saving = true;
}

#[test]
fn strictness_only_toggle_advances_current_folder_favorite_without_waiting_for_scan_end() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root) = fixture(false);
    let mut resources = AcceptedResources::default();
    let id = favorite_b(&mut app, &root, &["A", "B", "C"], false);
    admit_favorite(&mut app, &mut resources);
    app.ott.as_mut().unwrap().catalog_dirty = true;
    runtime_tick(&mut app); // Latch the ordinary Favorites completion before strict toggle.
    toggle_strict(&mut app);
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    assert!(!app.ott.as_ref().unwrap().catalog_complete);
    assert_eq!(
        app.ott
            .as_ref()
            .unwrap()
            .playing
            .as_ref()
            .unwrap()
            .selection_id,
        id
    );
    next(&mut app, &mut resources, "C");
    assert_saved(&app, &saved);
    assert_eq!(
        resources.starts, 2,
        "toggle itself performs no extra native start"
    );
    assert!(finish_catalog_playback(&mut app));
    assert!(
        app.ott.as_ref().unwrap().play_target.is_none(),
        "partial C cannot wrap to A"
    );
}

#[test]
fn strictness_only_toggle_removed_folder_favorite_still_wraps_to_canonical_row() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root) = fixture(false);
    let mut resources = AcceptedResources::default();
    let id = favorite_b(&mut app, &root, &["B"], true);
    admit_favorite(&mut app, &mut resources);
    {
        let runtime = app.ott.as_mut().unwrap();
        let favorite = runtime.saved.favorites.entries()[0].clone();
        assert_eq!(format!("favorite:{}", favorite.identity()), id);
        runtime.saved.favorites.toggle(favorite).unwrap();
        runtime
            .saved
            .local_favorite_proofs
            .prune(&runtime.saved.favorites);
        assert!(runtime.saved.favorites.entries().is_empty());
    }
    toggle_strict(&mut app);
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    assert!(finish_catalog_playback(&mut app));
    assert_eq!(
        app.ott.as_ref().unwrap().play_target.as_ref().unwrap().id,
        "B"
    );
    admit(&mut app, &mut resources);
    assert_saved(&app, &saved);
    assert_eq!(resources.starts, 2);
}
