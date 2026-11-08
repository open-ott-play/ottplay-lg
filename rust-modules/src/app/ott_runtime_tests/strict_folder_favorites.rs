//! Actual Local grants, native admission, EOF and catalogue-event boundaries.
//! Decoder resource acceptance is substituted using the existing parent fixture.
//! Designation runs a pre-revoked writer and models its successful ACK; no
//! decoder, protected write, scanner or network is exercised.
use super::*;
use crate::app::ott_kiosk_stored::{self as stored, Stored};
use crate::app::ott_local_session as local;
use plx_data::ott::favorites::{Locator, Origin};

fn row(id: &str) -> CatalogEntry {
    let mut entry = synthetic_entry();
    entry.id = id.into();
    entry.title = id.into();
    entry.category = "Movies".into();
    entry.media = MediaLocator::Local {
        relative_path: format!("{id}.mp4"),
    };
    entry
}

fn add_mixed_favorites(runtime: &mut Runtime, root: &local::BoundSource) {
    // Deliberately neither the order nor the membership of the folder A/B/C.
    runtime
        .saved
        .favorites
        .toggle(Favorite {
            title: "C from folder".into(),
            origin: Origin {
                provider: "local".into(),
                profile: "Local videos".into(),
                subprofile: None,
                source_alias: root.name.clone(),
            },
            locator: Locator::Local {
                root_alias: root.name.clone(),
                relative_path: "C.mp4".into(),
            },
        })
        .unwrap();
    let mut entry = row("live");
    entry.media = MediaLocator::Http {
        url: "https://media.invalid/live".into(),
    };
    let profile = runtime.active().unwrap().clone();
    runtime
        .saved
        .favorites
        .toggle(selection::capture(&profile, &entry).unwrap())
        .unwrap();
}

fn fixture(mixed: bool) -> (local_integration::TempDir, Box<App>, local::BoundSource) {
    let (directory, mut app, root) = local_integration::setup();
    app.running = true; // Actual kiosk completion requires a running application.
    for id in ["A", "B", "C"] {
        std::fs::write(
            std::path::Path::new(root.canonical_path.as_deref().unwrap()).join(format!("{id}.mp4")),
            b"fixture bytes, not decoder evidence",
        )
        .unwrap();
    }
    let runtime = app.ott.as_mut().unwrap();
    runtime.entropy = Some([7; 16]);
    runtime.resume_source_work();
    runtime.saved.favorites_selected = true;
    runtime.saved.category = "Retained preference".into();
    if mixed {
        add_mixed_favorites(runtime, &root);
    }
    let folders = crate::app::ott_local_sources::RememberedFolders::read(&runtime.saved.settings)
        .unwrap()
        .with_connected(crate::app::ott_local_sources::FolderDescriptor {
            id: root.id.clone(),
            name: root.name.clone(),
            path: root.canonical_path.clone().unwrap(),
            kind: crate::app::ott_local_sources::LocalSourceKind::Folder,
            usb: None,
        })
        .unwrap();
    folders.put_into(&mut runtime.saved.settings).unwrap();
    runtime.saving = false;
    runtime.resolver_busy = true;
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    app.refresh_hubs = false;
    (directory, app, root)
}

fn policy(root: &local::BoundSource, strict: bool) -> Stored {
    Stored::local_folder(
        Origin {
            provider: "local".into(),
            profile: "Local videos".into(),
            subprofile: None,
            source_alias: root.name.clone(),
        },
        strict,
        root.id.clone(),
        selection::local_root_fingerprint(root).unwrap(),
    )
    .unwrap()
}

// This follows the production rebind -> start -> transient completion sequence.
// It never supplies an order or modifies Continuity to make EOF choose a row.
fn admit(app: &mut App, resources: &mut AcceptedResources) {
    let refresh_before = app.refresh_hubs;
    let mut runtime = app.ott.take().unwrap();
    let request = runtime
        .play_target
        .clone()
        .expect("actual queued playback request");
    let (profile, entry) = match transient::staged_native_binding(&runtime, &request) {
        Some(result) => result.unwrap(),
        None => {
            let (_, profile, entry, _) = runtime.prepare_play_request(request.clone()).unwrap();
            (profile, entry.expect("folder row"))
        }
    };
    assert!(transient::target_current(&runtime));
    transient::before_native(&mut runtime, app).unwrap();
    runtime
        .rebind_native_admission(app, &profile, &entry)
        .unwrap();
    let read = runtime
        .resolver_next
        .as_ref()
        .unwrap()
        .local_read
        .as_ref()
        .unwrap();
    let source = transient::open_video_source(
        &plx_base::task::OffFrame::for_test(),
        Some(read),
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
    // Hold only the existing writer seam after admission. EOF/clock checkpoints
    // are inspected in memory, never allowed to write the user's store.
    runtime.saving = true;
    app.ott = Some(runtime);
    settle_pages(app);
    assert_owned_player_preserving_refresh(app, &request.id, refresh_before);
}

fn restored(
    app: &mut App,
    root: &local::BoundSource,
    ids: &[&str],
    complete: bool,
    resources: &mut AcceptedResources,
) {
    stage_restored(app, root, ids, complete);
    admit(app, resources);
}

fn stage_restored(app: &mut App, root: &local::BoundSource, ids: &[&str], complete: bool) {
    let mut runtime = app.ott.take().unwrap();
    runtime
        .saved
        .settings
        .insert(stored::KEY.into(), policy(root, true).encode().unwrap());
    // The persisted policy is decoded through the same Saved serde boundary.
    let bytes = serde_json::to_vec(&runtime.saved).unwrap();
    runtime.saved = serde_json::from_slice(&bytes).unwrap();
    runtime.saved.validate().unwrap();
    runtime.resolver_busy = false;
    transient::kiosk_restore_local_folder(
        &mut runtime,
        app,
        &root.id,
        &selection::local_root_fingerprint(root).unwrap(),
    )
    .unwrap();
    runtime.resolver_busy = true;
    let mut incoming = Catalog {
        entries: ids.iter().map(|id| row(id)).collect(),
        hierarchy: None,
    };
    let mut finished = complete.then_some(Ok(()));
    assert!(transient::scan_result(
        &mut runtime,
        app,
        &mut incoming,
        &mut finished
    ));
    assert_eq!(runtime.play_target.as_ref().unwrap().id, "A");
    assert!(matches!(runtime.transient.candidate.as_ref(),
        Some(transient::Candidate::Local(candidate))
        if matches!(candidate.purpose, transient::LocalPurpose::KioskFolder(_))));
    app.ott = Some(runtime);
}

fn assert_saved(app: &App, saved: &Value) {
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.saved.favorites_selected);
    assert_eq!(&serde_json::to_value(&runtime.saved).unwrap(), saved);
}

fn next(app: &mut App, resources: &mut AcceptedResources, id: &str) {
    let previous = plx_media::route::playback_identity(&app.player.session);
    assert!(finish_catalog_playback(app));
    assert_eq!(
        app.ott
            .as_ref()
            .unwrap()
            .play_target
            .as_ref()
            .map(|r| r.id.as_str()),
        Some(id)
    );
    admit(app, resources);
    assert_ne!(
        plx_media::route::playback_identity(&app.player.session),
        previous
    );
}

fn batch(app: &mut App, ids: &[&str], finished: bool) {
    let runtime = app.ott.as_ref().unwrap();
    runtime
        .catalog_tx
        .send(CatalogEvent::Batch(
            runtime.generation,
            Catalog {
                entries: ids.iter().map(|id| row(id)).collect(),
                hierarchy: None,
            },
        ))
        .unwrap();
    if finished {
        runtime
            .catalog_tx
            .send(CatalogEvent::Finished(runtime.generation, Ok(())))
            .unwrap();
    }
    runtime_tick(app);
    settle_pages(app);
}

#[test]
fn strict_restored_folder_loops_all_rows_with_empty_or_mixed_global_favorites() {
    let _serial = plx_base::testlock::serial();
    for mixed in [false, true] {
        let _reset = Reset;
        let (_directory, mut app, root) = fixture(mixed);
        let mut resources = AcceptedResources::default();
        restored(&mut app, &root, &["A", "B", "C"], true, &mut resources);
        let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        let binding = app
            .ott
            .as_ref()
            .unwrap()
            .playing
            .as_ref()
            .unwrap()
            .local_selection
            .clone()
            .unwrap();
        for id in ["B", "C", "A"] {
            next(&mut app, &mut resources, id);
            assert_saved(&app, &saved);
            let runtime = app.ott.as_ref().unwrap();
            assert!(runtime.playing.as_ref().unwrap().local_selection.as_ref() == Some(&binding));
            assert!(runtime.current_local_root(&binding).is_some());
        }
        assert_eq!(resources.starts, 4);
    }
}

#[test]
fn strict_partial_folder_waits_for_real_batches_and_only_wraps_after_completion() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root) = fixture(true);
    let mut resources = AcceptedResources::default();
    restored(&mut app, &root, &["A"], false, &mut resources);
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    // Exercise the pre-EOF dirty-catalogue path: it must not latch Favorites'
    // complete=true into the folder's Continuity while its scan is partial.
    runtime_tick(&mut app);
    assert!(finish_catalog_playback(&mut app));
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    assert_eq!(resources.starts, 1);
    batch(&mut app, &["B"], false);
    assert_eq!(
        app.ott
            .as_ref()
            .unwrap()
            .play_target
            .as_ref()
            .map(|r| r.id.as_str()),
        Some("B")
    );
    admit(&mut app, &mut resources);
    assert!(finish_catalog_playback(&mut app));
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    batch(&mut app, &["C"], false);
    assert_eq!(
        app.ott
            .as_ref()
            .unwrap()
            .play_target
            .as_ref()
            .map(|r| r.id.as_str()),
        Some("C")
    );
    admit(&mut app, &mut resources);
    assert!(finish_catalog_playback(&mut app));
    assert!(
        app.ott.as_ref().unwrap().play_target.is_none(),
        "C must not wrap while scan is partial"
    );
    batch(&mut app, &[], true);
    assert_eq!(
        app.ott
            .as_ref()
            .unwrap()
            .play_target
            .as_ref()
            .map(|r| r.id.as_str()),
        Some("A")
    );
    admit(&mut app, &mut resources);
    assert_eq!(resources.starts, 4);
    assert_saved(&app, &saved);
}

// The real command retains its Prepared proof. Only its durable outcome is
// modeled after a pre-revoked writer has completed without touching storage.
fn designate(app: &mut App) {
    let mut runtime = app.ott.take().unwrap();
    runtime.saving = false; // release the known synthetic writer hold
    assert!(runtime.play_target.is_none() && runtime.pending_play.is_none());
    let prepared = kiosk_source::prepare(&runtime, app, Some("folder:Movies"), true).unwrap();
    let candidate = kiosk_source::save_candidate(&runtime, &prepared).unwrap();
    runtime.write_permit.revoke();
    let now = runtime.now_ms();
    let body = json!({"commands":[], "server_time":1000, "requests":[{
        "id":"00000000000000000000000000009001", "action":"kiosk",
        "params":{"mode":"on","query":"folder:Movies","strict":true}, "expires_at":1020
    }]})
    .to_string();
    let mut parsed = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(parsed.len(), 1);
    let invocation = parsed.pop().unwrap();
    assert!(runtime.remote.invocation_is_current(&invocation));
    assert!(commands::command(&mut runtime, app, &invocation).is_none());
    let completion = runtime
        .rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let Job::Saved(Err(_), Some(owned), reload, response) = completion else {
        panic!("pre-revoked writer must finish before modeled ACK");
    };
    assert_eq!(owned.id, invocation.id);
    runtime
        .tx
        .send(Job::Saved(Ok(candidate), Some(owned), reload, response))
        .unwrap();
    app.ott = Some(runtime);
    runtime_tick(app);
    settle_pages(app);
    assert_eq!(
        app.ott
            .as_ref()
            .unwrap()
            .play_target
            .as_ref()
            .map(|r| r.id.as_str()),
        Some("A")
    );
}

#[test]
fn designation_clears_prior_favorites_completeness_without_replacing_local_grant() {
    let _serial = plx_base::testlock::serial();
    // Both a direct Local row and an outside Favorite may be installed while
    // the retained folder's A-only catalogue is still incomplete.
    for borrowed in [false, true] {
        let _reset = Reset;
        let (_directory, mut app, root) = fixture(false);
        let mut resources = AcceptedResources::default();
        {
            let mut runtime = app.ott.take().unwrap();
            runtime.catalog.entries = vec![row("A")];
            transient::accepted_catalog_update(&mut runtime);
            runtime.play(&mut app, "A").unwrap();
            app.ott = Some(runtime);
        }
        admit(&mut app, &mut resources);
        if borrowed {
            let mut live = row("live");
            live.media = MediaLocator::Http {
                url: "https://media.invalid/live".into(),
            };
            let runtime = app.ott.as_mut().unwrap();
            let favorite = selection::capture(runtime.active().unwrap(), &live).unwrap();
            let id = format!("favorite:{}", favorite.identity());
            runtime.saved.favorites.toggle(favorite).unwrap();
            local_integration::install(&mut app, None, &id);
            let mut runtime = app.ott.take().unwrap();
            runtime.checkpoint(&mut app, true);
            assert!(matches!(runtime.transient.accepted.as_ref(),
                Some(transient::TransientMedia::Local(session)) if session.borrowed.is_some()));
            app.ott = Some(runtime);
        }
        app.ott.as_mut().unwrap().catalog_dirty = true;
        runtime_tick(&mut app); // Actual ordinary Favorites path latches complete.
        let runtime = app.ott.as_ref().unwrap();
        let binding = runtime.bind_local_selection(&root).unwrap();
        let old_read = runtime.capture_local_source(&binding).unwrap();
        let before = runtime.saved.favorites.clone();
        assert!(matches!(runtime.transient.accepted.as_ref(),
            Some(transient::TransientMedia::Local(session)) if session.kiosk_resume.is_none()));
        designate(&mut app);
        // A durable folder policy must retire an outside Favorite. That prior
        // exit owes a hub refresh; the following admission must preserve it.
        assert_eq!(app.refresh_hubs, borrowed);
        assert!(app
            .ott
            .as_ref()
            .unwrap()
            .local_source_read_is_current(&old_read));
        assert!(app.ott.as_ref().unwrap().saved.favorites == before);
        assert!(app.ott.as_ref().unwrap().saved.favorites_selected);
        admit(&mut app, &mut resources);
        let runtime = app.ott.as_ref().unwrap();
        assert!(!runtime.catalog_complete);
        assert!(finish_catalog_playback(&mut app));
        assert!(
            app.ott.as_ref().unwrap().play_target.is_none(),
            "old Favorites completeness must not wrap A"
        );
        batch(&mut app, &["B"], false);
        assert_eq!(
            app.ott
                .as_ref()
                .unwrap()
                .play_target
                .as_ref()
                .map(|r| r.id.as_str()),
            Some("B")
        );
        admit(&mut app, &mut resources);
        assert!(app
            .ott
            .as_ref()
            .unwrap()
            .current_local_root(&binding)
            .is_some());
    }
}

#[test]
fn off_nonstrict_and_wrong_folder_policies_do_not_override_global_favorites() {
    let _serial = plx_base::testlock::serial();
    for mode in 0..3 {
        let _reset = Reset;
        let (_directory, mut app, root) = fixture(true);
        let mut resources = AcceptedResources::default();
        restored(&mut app, &root, &["A", "B", "C"], true, &mut resources);
        let runtime = app.ott.as_mut().unwrap();
        match mode {
            0 => {
                runtime.saved.settings.remove(stored::KEY);
            }
            1 => {
                runtime
                    .saved
                    .settings
                    .insert(stored::KEY.into(), policy(&root, false).encode().unwrap());
            }
            _ => {
                let mut other = root.clone();
                other.id = "other-root".into();
                runtime
                    .saved
                    .settings
                    .insert(stored::KEY.into(), policy(&other, true).encode().unwrap());
            }
        }
        let expected: Vec<_> = runtime
            .saved
            .favorites
            .entries()
            .iter()
            .map(|favorite| format!("favorite:{}", favorite.identity()))
            .collect();
        assert_eq!(runtime.ordered_ids(), expected);
        assert!(runtime.selection_complete());
    }
}

#[test]
fn stale_or_revoked_folder_grant_never_gains_the_strict_folder_order() {
    let _serial = plx_base::testlock::serial();
    for change in 0..3 {
        let _reset = Reset;
        let (_directory, mut app, root) = fixture(true);
        let mut resources = AcceptedResources::default();
        restored(&mut app, &root, &["A", "B", "C"], true, &mut resources);
        let runtime = app.ott.as_mut().unwrap();
        let binding = runtime
            .playing
            .as_ref()
            .unwrap()
            .local_selection
            .clone()
            .unwrap();
        match change {
            0 => runtime.local_inventory.sources[0].withdraw(),
            1 => {
                let Some(transient::TransientMedia::Local(session)) =
                    runtime.transient.accepted.as_mut()
                else {
                    panic!("accepted Local");
                };
                session.read.selection.generation =
                    session.read.selection.generation.wrapping_add(1);
            }
            _ => {
                let mut replacement = root.clone();
                replacement.generation = runtime.generation.wrapping_add(1);
                replacement.authority_ref = local::AuthorityRef(99);
                let source = local_integration::acquire_root(replacement);
                runtime
                    .local_inventory
                    .install(vec![source], runtime.generation.wrapping_add(1));
            }
        }
        let expected: Vec<_> = runtime
            .saved
            .favorites
            .entries()
            .iter()
            .map(|favorite| format!("favorite:{}", favorite.identity()))
            .collect();
        assert_eq!(runtime.ordered_ids(), expected);
        if change == 1 {
            assert!(
                runtime.capture_local_source(&binding).is_some(),
                "stale accepted receipt cannot revoke a still-live inventory grant"
            );
        } else {
            assert!(runtime.capture_local_source(&binding).is_none());
        }
    }
}

#[test]
fn accepted_folder_scope_survives_real_lifecycle_generation_change() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root) = fixture(true);
    let mut resources = AcceptedResources::default();
    restored(&mut app, &root, &["A", "B"], false, &mut resources);
    let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    let mut runtime = app.ott.take().unwrap();
    let owner = runtime.transient.accepted.as_ref().unwrap().owner();
    let generation = runtime.generation;
    runtime.suspend_source_work(&mut app);
    assert_ne!(runtime.generation, generation);
    assert!(runtime.transient.accepted.as_ref().unwrap().owner() == owner);
    assert_eq!(runtime.ordered_ids(), vec!["A".to_owned(), "B".to_owned()]);
    assert!(!runtime.selection_complete());
    assert!(
        !runtime.catalog_done.load(Ordering::Acquire),
        "old worker occupancy remains held"
    );
    runtime.resume_source_work();
    assert_eq!(runtime.ordered_ids(), vec!["A".to_owned(), "B".to_owned()]);
    assert!(!runtime.selection_complete());
    app.ott = Some(runtime);
    assert_saved(&app, &saved);
}

#[test]
fn ordinary_borrowed_favorites_keep_their_own_eof_order_and_saved_document() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root) = fixture(false);
    let mut ids = Vec::new();
    for id in ["live-one", "live-two"] {
        let runtime = app.ott.as_mut().unwrap();
        let mut entry = row(id);
        entry.provider_id = Some(id.into());
        entry.media = MediaLocator::Http {
            url: format!("https://media.invalid/{id}"),
        };
        let favorite = selection::capture(runtime.active().unwrap(), &entry).unwrap();
        ids.push(format!("favorite:{}", favorite.identity()));
        runtime.saved.favorites.toggle(favorite).unwrap();
    }
    assert_ne!(ids[0], ids[1], "the two channels have distinct portable identities");
    assert_eq!(app.ott.as_ref().unwrap().saved.favorites.entries().len(), 2);
    local_integration::install(&mut app, None, &ids[0]);
    // The regular checkpoint is the production boundary that marks a borrowed
    // non-Local row without transferring ownership of its old folder grant.
    app.ott.as_mut().unwrap().saving = true;
    plx_media::player::SHARED
        .playpos_ns
        .store(1_000_000_000, Ordering::Release);
    let mut runtime = app.ott.take().unwrap();
    runtime.checkpoint(&mut app, true);
    app.ott = Some(runtime);
    let runtime = app.ott.as_ref().unwrap();
    assert!(matches!(runtime.transient.accepted.as_ref(),
        Some(transient::TransientMedia::Local(session)) if session.borrowed.is_some()));
    assert!(runtime.local_inventory.sources[0].is_live());
    assert_eq!(runtime.local_inventory.sources[0].descriptor().id, root.id);
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    assert!(finish_catalog_playback(&mut app));
    assert_eq!(
        app.ott
            .as_ref()
            .unwrap()
            .play_target
            .as_ref()
            .map(|r| r.id.as_str()),
        Some(ids[1].as_str())
    );
    assert_saved(&app, &saved);
}

#[test]
fn staged_folder_admission_rejects_stale_owner_receipt_ticket_and_policy() {
    let _serial = plx_base::testlock::serial();
    for changed in 0..7 {
        let _reset = Reset;
        let (_directory, mut app, root) = fixture(false);
        let accepted = app.ott.as_ref().unwrap().bind_local_selection(&root).unwrap();
        stage_restored(&mut app, &root, &["A"], false);
        let mut runtime = app.ott.take().unwrap();
        let request = runtime.play_target.as_ref().unwrap().clone();
        let (profile, entry) = transient::staged_native_binding(&runtime, &request).unwrap().unwrap();
        assert_ne!(request.local_selection.as_ref().unwrap(), &accepted);
        assert!(runtime.capture_local_source(&accepted).is_some());
        assert!(runtime.capture_local_source(request.local_selection.as_ref().unwrap()).is_none(),
            "staged receipt must not become an accepted inventory receipt early");
        transient::before_native(&mut runtime, &app).unwrap();
        match changed {
            0 => runtime.cancel.store(true, Ordering::Release),
            1 => runtime.play_target.as_mut().unwrap().transient_owner = Some(local::OwnerToken(99)),
            2 => runtime.play_target.as_mut().unwrap().local_selection = Some(accepted),
            3 => runtime.local_inventory.sources[0].withdraw(),
            4 => plx_media::route::cancel_play(&mut app.player.session),
            5 => runtime.navigation_epoch = runtime.navigation_epoch.wrapping_add(1),
            _ => {
                let mut other = root.clone();
                other.id = "other-folder".into();
                runtime.saved.settings.insert(stored::KEY.into(), policy(&other, true).encode().unwrap());
            }
        }
        assert!(runtime.rebind_native_admission(&app, &profile, &entry).is_err(),
            "stale staged admission case {changed} must fail before native start");
        assert!(runtime.playing.is_none());
        app.ott = Some(runtime);
    }
}

#[path = "strict_folder_favorite_toggle.rs"]
mod favorite_toggle;

#[test]
fn accepted_same_folder_row_rearms_after_stop_but_checkpoint_does_not() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root) = fixture(false);
    let mut resources = AcceptedResources::default();
    restored(&mut app, &root, &["A"], false, &mut resources);
    let mut runtime = app.ott.take().unwrap();
    let previous = runtime.playing.clone().unwrap();
    let read = runtime.transient.accepted.as_ref().and_then(|accepted| match accepted {
        transient::TransientMedia::Local(session) => Some(session.read.clone()),
        _ => None,
    }).unwrap();
    runtime.retire_catalog_playback(&mut app);
    // Retirement keeps the bookmark and held source. A late checkpoint for
    // that same row must not reopen continuation before another accepted start.
    assert!(runtime.transient_checkpoint(&previous, 1_234));
    assert!(matches!(runtime.transient.accepted.as_ref(),
        Some(transient::TransientMedia::Local(session))
        if session.context.current() == Some("A") && session.context.position_ms() == 1_234));
    runtime.catalog_dirty = true;
    transient::accepted_catalog_update(&mut runtime);
    assert!(!runtime.catalog_dirty, "ordinary checkpoint must preserve explicit Stop");
    runtime.play(&mut app, "A").unwrap();
    runtime.catalog_dirty = true;
    transient::accepted_catalog_update(&mut runtime);
    assert!(!runtime.catalog_dirty, "queued selection is not native acceptance");
    app.ott = Some(runtime);
    admit(&mut app, &mut resources);
    let runtime = app.ott.as_mut().unwrap();
    assert!(runtime.local_source_read_is_current(&read));
    assert!(runtime.continuity.accepts_end(runtime.generation, "A"));
    runtime.catalog_dirty = true;
    transient::accepted_catalog_update(runtime);
    assert!(runtime.catalog_dirty, "accepted same-row selection must reopen continuation");
    assert!(finish_catalog_playback(&mut app));
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    batch(&mut app, &["B"], false);
    assert_eq!(app.ott.as_ref().unwrap().play_target.as_ref().map(|request| request.id.as_str()), Some("B"));
    admit(&mut app, &mut resources);
    assert!(app.ott.as_ref().unwrap().local_source_read_is_current(&read));
}
