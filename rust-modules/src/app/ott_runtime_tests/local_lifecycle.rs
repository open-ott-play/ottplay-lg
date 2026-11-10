//! Runs as a child of local_integration, using its real Runtime/root fixture.
use super::*;

fn network_entry(id: &str) -> CatalogEntry {
    let mut entry = item(id);
    entry.provider_id = Some(id.into());
    entry.media = MediaLocator::Http { url: format!("https://media.invalid/{id}") };
    entry
}

fn remember_test_root(runtime: &mut Runtime, root: &local::BoundSource) {
    crate::app::ott_local_sources::RememberedFolders::read(&runtime.saved.settings)
        .unwrap().with_connected(crate::app::ott_local_sources::FolderDescriptor {
            kind: crate::app::ott_local_sources::LocalSourceKind::Folder, usb: None,
            id: root.id.clone(), name: root.name.clone(), path: root.canonical_path.clone().unwrap(),
        }).unwrap().put_into(&mut runtime.saved.settings).unwrap();
}

#[test]
fn nonlocal_startup_continues_when_unrelated_local_inventory_fails() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for mode in ["provider", "favorites-first", "favorites-restore"] {
        let (_root_dir, mut app, _root) = setup();
        let entry = network_entry("live");
        let favorite = selection::capture(&permanent(), &entry).unwrap();
        let selected = format!("favorite:{}", favorite.identity());
        let expected_id = if mode == "provider" { entry.id.clone() } else { selected.clone() };
        let position_ms = if mode == "favorites-restore" { 12_500 } else { 0 };
        {
            let runtime = app.ott.as_mut().unwrap();
            if mode != "provider" {
                runtime.saved.favorites.toggle(favorite).unwrap();
                runtime.saved.favorites_selected = true;
            }
            if position_ms > 0 {
                runtime.saved.resume.insert("favorites".into(), SavedResume {
                    entry_id: selected, position_ms,
                    source_fingerprint: profile_fingerprint(&permanent()),
                });
            }
            runtime.load_catalog();
            runtime.local_inventory.phase = transient::InventoryPhase::Loading(runtime.generation);
            let generation = runtime.generation;
            runtime.catalog_tx.send(CatalogEvent::LocalBindings(generation,
                Err(local::LocalError::NativeError))).unwrap_or_else(|_| panic!("catalogue receiver"));
            runtime.catalog_tx.send(CatalogEvent::Batch(generation,
                Catalog { hierarchy: None, entries: vec![entry] })).unwrap_or_else(|_| panic!("catalogue receiver"));
            runtime.catalog_tx.send(CatalogEvent::Finished(generation, Ok(())))
                .unwrap_or_else(|_| panic!("catalogue receiver"));
        }
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(matches!(runtime.local_inventory.phase, transient::InventoryPhase::Failed));
        let target = runtime.play_target.as_ref().expect("network startup must reach the held resolver");
        assert_eq!(target.id, expected_id, "{mode}");
        assert_eq!(target.position_ms, position_ms, "{mode}");
        assert_eq!(runtime.saved.profiles.active_profile.as_deref(), Some("permanent"));
        assert!(runtime.resolver_next.as_ref().is_some_and(|request| request.profile.kind == ProviderKind::M3u));
    }
}

#[test]
fn mixed_favorite_network_failure_never_selects_a_local_successor_when_binding_becomes_ready() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    let local_id = add_root_proof(&mut app, &root);
    let network_a = selection::capture(&permanent(), &network_entry("a")).unwrap();
    let network_c = selection::capture(&permanent(), &network_entry("c")).unwrap();
    let selected = format!("favorite:{}", network_a.identity());
    {
        let runtime = app.ott.as_mut().unwrap();
        let local_favorite = runtime.saved.favorites.entries()[0].clone();
        runtime.saved.favorites.toggle(local_favorite.clone()).unwrap();
        runtime.saved.favorites.toggle(network_a).unwrap();
        runtime.saved.favorites.toggle(local_favorite).unwrap();
        runtime.saved.favorites.toggle(network_c).unwrap();
        remember_test_root(runtime, &root);
        runtime.saved.favorites_selected = true;
        runtime.saved.resume.insert("favorites".into(), SavedResume {
            entry_id: selected.clone(), position_ms: 12_500,
            source_fingerprint: profile_fingerprint(&permanent()),
        });
        runtime.load_catalog();
        runtime.local_inventory.phase = transient::InventoryPhase::Loading(runtime.generation);
        let generation = runtime.generation;
        runtime.catalog_tx.send(CatalogEvent::LocalBindings(generation,
            Err(local::LocalError::NativeError))).unwrap_or_else(|_| panic!("catalogue receiver"));
        runtime.catalog_tx.send(CatalogEvent::Batch(generation,
            Catalog { hierarchy: None, entries: vec![network_entry("a"), network_entry("c")] }))
                .unwrap_or_else(|_| panic!("catalogue receiver"));
        runtime.catalog_tx.send(CatalogEvent::Finished(generation, Ok(())))
            .unwrap_or_else(|_| panic!("catalogue receiver"));
    }
    runtime_tick(&mut app);
    {
        let runtime = app.ott.as_mut().unwrap();
        let target = runtime.play_target.as_ref().expect("mixed network restore must start");
        assert_eq!(target.id, selected);
        assert_eq!(target.position_ms, 12_500);
        let resolver = runtime.resolver_next.take().unwrap();
        runtime.tx.send(Job::Play(resolver.generation, resolver.profile, Err("fixture network failure")))
            .unwrap_or_else(|_| panic!("resolver receiver"));
    }
    runtime_tick(&mut app);
    for _ in 0..3 { runtime_tick(&mut app); }
    let mut runtime = app.ott.take().unwrap();
    assert!(!runtime.continuity.has_deferred_start(runtime.generation));
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none(),
        "a failed exact favorite cannot select another network or local item");
    assert!(!runtime.continuity.accepts_end(runtime.generation, &local_id));
    let generation = runtime.generation;
    let sources = runtime.local_inventory.sources.clone();
    runtime.local_inventory.phase = transient::InventoryPhase::Loading(generation);
    transient::binding_result(&mut runtime, &mut app, generation, Ok(ott_catalog::LocalBindings {
        sources, lookup: None, matched: vec![], operation_error: None,
    }));
    assert!(matches!(runtime.local_inventory.phase, transient::InventoryPhase::Ready(_)));
    app.ott = Some(runtime);
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none(),
        "new local readiness cannot revive a forbidden automatic successor");
    assert!(!runtime.continuity.has_deferred_start(runtime.generation));
}

#[test]
fn deferred_local_restore_preserves_position_and_reason_after_ready_transition() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    let selected = add_root_proof(&mut app, &root);
    let mut runtime = app.ott.take().unwrap();
    let local_profile = transient::source_profile(&root).unwrap();
    remember_test_root(&mut runtime, &root);
    runtime.saved.favorites_selected = true;
    runtime.saved.resume.insert("favorites".into(), SavedResume {
        entry_id: selected.clone(), position_ms: 23_000,
        source_fingerprint: profile_fingerprint(&local_profile),
    });
    runtime.load_catalog();
    runtime.local_inventory.phase = transient::InventoryPhase::Loading(runtime.generation);
    let generation = runtime.generation;
    transient::binding_result(&mut runtime, &mut app, generation, Err(local::LocalError::NativeError));
    let point = continuity::ResumePoint { entry_id: selected.clone(), position_ms: 23_000 };
    runtime.catalog_dirty = true;
    app.ott = Some(runtime);
    runtime_tick(&mut app);
    let mut runtime = app.ott.take().unwrap();
    assert_eq!(runtime.continuity.pending_restore(), Some(&point));
    assert!(runtime.continuity.has_deferred_start(runtime.generation));
    assert!(!runtime.catalog_dirty);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    let sources = runtime.local_inventory.sources.clone();
    runtime.local_inventory.phase = transient::InventoryPhase::Loading(generation);
    transient::binding_result(&mut runtime, &mut app, generation, Ok(ott_catalog::LocalBindings {
        sources, lookup: None, matched: vec![], operation_error: None,
    }));
    assert!(runtime.catalog_dirty);
    app.ott = Some(runtime);
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    let target = runtime.play_target.as_ref().expect("deferred restore must reach the held resolver");
    assert_eq!(target.id, selected);
    assert_eq!(target.position_ms, 23_000);
    assert!(target.local_selection.is_some());
    assert!(runtime.continuity.pending_restore().is_none());
    assert_eq!(runtime.restore_play_generation, Some(runtime.play_generation));
}

#[test]
fn deferred_local_ready_during_partial_rebase_waits_for_exact_member_then_starts_once() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    {
        let runtime = app.ott.as_mut().unwrap();
        let profile = transient::source_profile(&root).unwrap();
        runtime.saved.profiles.active_profile = Some(profile.id.clone());
        runtime.saved.profiles.profiles = vec![profile];
        remember_test_root(runtime, &root);
        runtime.load_catalog();
        let generation = runtime.generation;
        runtime.local_inventory.phase = transient::InventoryPhase::Loading(generation);
        runtime.catalog_tx.send(CatalogEvent::LocalBindings(generation,
            Err(local::LocalError::NativeError))).unwrap_or_else(|_| panic!("catalogue receiver"));
        runtime.catalog_tx.send(CatalogEvent::Batch(generation,
            Catalog { hierarchy: None, entries: vec![item("first")] }))
            .unwrap_or_else(|_| panic!("catalogue receiver"));
    }
    runtime_tick(&mut app);
    {
        let runtime = app.ott.as_mut().unwrap();
        assert!(runtime.continuity.has_deferred_start(runtime.generation));
        runtime.refresh_catalog_preserving_playback();
        assert!(runtime.catalog.entries.is_empty() && !runtime.catalog_complete);
        let generation = runtime.generation;
        runtime.local_inventory.phase = transient::InventoryPhase::Loading(generation);
        let sources = runtime.local_inventory.sources.clone();
        runtime.catalog_tx.send(CatalogEvent::LocalBindings(generation, Ok(ott_catalog::LocalBindings {
            sources, lookup: None, matched: vec![], operation_error: None,
        }))).unwrap_or_else(|_| panic!("catalogue receiver"));
    }
    runtime_tick(&mut app);
    for _ in 0..3 {
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.continuity.has_deferred_start(runtime.generation));
        assert!(!runtime.catalog_dirty);
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        runtime_tick(&mut app);
    }
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.catalog_tx.send(CatalogEvent::Batch(runtime.generation,
            Catalog { hierarchy: None, entries: vec![item("other"), item("first")] }))
            .unwrap_or_else(|_| panic!("catalogue receiver"));
    }
    runtime_tick(&mut app);
    let generation = {
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(runtime.play_target.as_ref().map(|target| target.id.as_str()), Some("first"));
        assert!(!runtime.continuity.has_deferred_start(runtime.generation));
        runtime.play_generation
    };
    runtime_tick(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().play_generation, generation);
}

fn suspend(runtime: &mut Runtime, app: &mut App) -> u64 {
    let previous = runtime.generation;
    runtime.resume_source_work();
    runtime.suspend_source_work(app);
    previous.wrapping_add(1).max(1)
}

#[test]
fn suspend_retains_accepted_order_and_loss_cleanup_until_worker_retires() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, _) = setup();
    let mut runtime = app.ott.take().unwrap();
    runtime.catalog.entries.push(item("second"));
    transient::accepted_catalog_update(&mut runtime);
    runtime.local_inventory.proof_cleanup = transient::ProofCleanup::Pending;
    runtime.local_inventory.phase = transient::InventoryPhase::Loading(runtime.generation);
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let paused_generation = suspend(&mut runtime, &mut app);
    let Some(transient::TransientMedia::Local(session)) = &runtime.transient.accepted else {
        panic!("accepted Local owner was discarded");
    };
    assert_eq!(session.context.order(), ["first", "second"]);
    assert_eq!(session.context.current(), Some("first"));
    assert_eq!(runtime.generation, paused_generation);
    assert!(runtime.cancel.load(Ordering::Acquire));
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
    assert!(matches!(
        runtime.local_inventory.phase,
        transient::InventoryPhase::Unloaded
    ));
    assert!(runtime.local_inventory.proof_cleanup == transient::ProofCleanup::Pending);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
}

#[test]
fn wake_revalidates_unremembered_accepted_root_before_play_admission() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, _) = setup();
    let mut runtime = app.ott.take().unwrap();
    assert!(
        crate::app::ott_local_sources::RememberedFolders::read(&runtime.saved.settings)
            .unwrap()
            .folders()
            .is_empty()
    );
    runtime.resume_source_work();
    transient::resume_after_lifecycle(&mut runtime, &mut app);
    assert!(matches!(
        runtime.local_inventory.phase,
        transient::InventoryPhase::Ready(_)
    ));
    runtime.catalog_done.store(true, Ordering::Release);
    transient::resume_after_lifecycle(&mut runtime, &mut app);
    assert!(!transient::lifecycle_target_ready(
        &runtime,
        runtime.effective_profile().unwrap(),
        None
    ));
    let Some(ott_catalog::Task::LocalBindings(request)) =
        transient::take_binding_task(&mut runtime)
    else {
        panic!("unremembered accepted root skipped validation");
    };
    assert_eq!(request.known.len(), 1);
    assert!(!request.allow_new && !request.allow_reconnect);
    assert!(!transient::lifecycle_target_ready(
        &runtime,
        runtime.effective_profile().unwrap(),
        None
    ));
    assert_eq!(runtime.catalog_next.as_ref(), runtime.effective_profile());
    // This fixture does not execute the worker or imply a successful validation.
    assert!(runtime.resolver_next.is_none());
}

#[test]
fn suspend_settles_a_pending_local_command_without_restarting_its_worker() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, _) = setup();
    let _invocation = begin_candidate(&mut app, &_root_dir);
    let mut runtime = app.ott.take().unwrap();
    let paused_generation = suspend(&mut runtime, &mut app);
    assert!(runtime.transient.candidate.is_none());
    assert_eq!(runtime.generation, paused_generation);
    assert!(runtime.cancel.load(Ordering::Acquire));
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
    assert_eq!(runtime.catalog_next.as_ref(), runtime.effective_profile());
    assert!(runtime.catalog_loading && !runtime.catalog_complete);
    assert!(runtime.resolver_next.is_none());
}

#[test]
fn saving_connection_waits_for_real_completion_and_cannot_restart_while_paused() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    let mut runtime = app.ott.take().unwrap();
    let token = runtime.remote.runtime().to_owned();
    let invocation = runtime.remote.request_for_test(
        "local_media",
        json!({"op":"connect","runtime":token,"source":root.canonical_path}),
    );
    let request = local::LocalRequest::parse(&invocation.params).unwrap();
    runtime.transient.candidate = Some(transient::Candidate::Local(transient::LocalCandidate {
        inventory: runtime.local_inventory.sources.clone(),
        admission_ticket: plx_media::route::play_request_ticket(),
        owner: local::OwnerToken(runtime.generation),
        pending: local::PendingLocal::new(
            request.clone(),
            local::OwnerToken(runtime.generation),
            invocation.deadline_ms,
        ),
        purpose: transient::LocalPurpose::Remote(invocation.clone()),
        request,
        interrupted: transient::InterruptedScan {
            owner: runtime
                .transient
                .accepted
                .as_ref()
                .map(|owner| owner.owner()),
            profile: runtime.effective_profile().cloned(),
            restart: true,
        },
        phase: transient::CandidatePhase::SavingConnection(root),
    }));
    let paused_generation = suspend(&mut runtime, &mut app);
    assert!(
        runtime.transient.candidate.is_some(),
        "Saved still owns its completion"
    );
    let response = transient::connection_saved(&mut runtime, &app, &invocation, true).unwrap();
    assert_eq!(response["data"]["state"], "error");
    assert!(runtime.transient.candidate.is_none());
    assert_eq!(runtime.generation, paused_generation);
    assert!(runtime.cancel.load(Ordering::Acquire));
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
    assert!(runtime.resolver_next.is_none());
}

#[test]
fn failed_validation_blocks_only_local_work_and_does_not_hot_retry() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for startup in [false, true] {
        for spawn_refused in [false, true] {
            let (_root_dir, mut app, root) = setup();
            let mut runtime = app.ott.take().unwrap();
            let profile = transient::source_profile(&root).unwrap();
            if startup {
                runtime.transient.accepted = None;
            }
            let generation = runtime.generation;
            runtime.local_inventory.phase = transient::InventoryPhase::Loading(generation);
            if spawn_refused {
                transient::launch_failed(&mut runtime);
            } else {
                transient::binding_result(
                    &mut runtime,
                    &mut app,
                    generation,
                    Err(local::LocalError::NativeError),
                );
            }
            assert!(matches!(
                runtime.local_inventory.phase,
                transient::InventoryPhase::Failed
            ));
            assert_eq!(runtime.local_inventory.generation(), 0);
            assert!(!transient::lifecycle_target_ready(&runtime, &profile, None));
            assert!(transient::lifecycle_target_ready(
                &runtime,
                &permanent(),
                None
            ));
            assert!(transient::take_binding_task(&mut runtime).is_none());
            assert!(
                runtime.local_inventory.sources[0].root_handle().is_some(),
                "failure to validate is not an observation that the source disappeared"
            );
        }
    }
}

#[test]
fn current_successful_explicit_retry_reopens_local_readiness() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    let mut runtime = app.ott.take().unwrap();
    let profile = transient::source_profile(&root).unwrap();
    runtime.local_inventory.phase = transient::InventoryPhase::Failed;
    let token = runtime.remote.runtime().to_owned();
    let invocation = local_invocation(
        &mut runtime, 1, json!({"op":"sources","runtime":token}),
    );
    assert!(commands::command(&mut runtime, &mut app, &invocation).is_none());
    assert!(!transient::lifecycle_target_ready(&runtime, &profile, None));
    let sources = runtime.local_inventory.sources.clone();
    let generation = runtime.generation;
    transient::binding_result(
        &mut runtime,
        &mut app,
        generation,
        Ok(ott_catalog::LocalBindings {
            sources,
            lookup: None,
            matched: vec![],
            operation_error: None,
        }),
    );
    assert!(runtime.transient.candidate.is_none());
    assert!(transient::lifecycle_target_ready(&runtime, &profile, None));
}

#[test]
fn same_root_wake_refreshes_only_retained_resume_receipt() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    install(&mut app, Some(&root), "first");
    let mut runtime = app.ott.take().unwrap();
    let playing = runtime.playing.clone().unwrap();
    let previous = playing.local_selection.clone().unwrap();
    runtime.resume_pending = Some(PlaybackRequest {
        kiosk_selection: playing.kiosk_selection.clone(),
        archive: None,
        continuation: playing.identity,
        transient_owner: playing.transient_owner,
        local_selection: Some(previous.clone()),
        id: playing.selection_id.clone(),
        position_ms: 12_500,
        advance_on_failure: false,
    });
    runtime.generation = runtime.generation.wrapping_add(1).max(1);
    let generation = runtime.generation;
    runtime.local_inventory.phase = transient::InventoryPhase::Loading(generation);
    assert_eq!(
        transient::lifecycle_resume_ready(&runtime, runtime.resume_pending.as_ref().unwrap()),
        Ok(false)
    );
    let sources = runtime.local_inventory.sources.clone();
    transient::binding_result(
        &mut runtime,
        &mut app,
        generation,
        Ok(ott_catalog::LocalBindings {
            sources,
            lookup: None,
            matched: vec![],
            operation_error: None,
        }),
    );
    let request = runtime.resume_pending.as_ref().unwrap();
    let rebound = request.local_selection.as_ref().unwrap();
    assert_ne!(*rebound, previous, "registry receipt must be refreshed");
    assert_eq!(rebound.source_id, previous.source_id);
    assert_eq!(rebound.generation, previous.generation);
    assert_eq!(request.position_ms, 12_500);
    assert_eq!(request.id, "first");
    assert_eq!(
        transient::lifecycle_resume_ready(&runtime, request),
        Ok(true)
    );
    assert!(
        runtime.resolver_next.is_none(),
        "revalidation itself never starts playback"
    );
}

#[test]
fn successfully_validated_play_candidate_can_admit_before_inventory_publication() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    let mut runtime = app.ott.take().unwrap();
    let profile = transient::source_profile(&root).unwrap();
    runtime.local_inventory.phase = transient::InventoryPhase::Failed;
    let token = runtime.remote.runtime().to_owned();
    let invocation = local_invocation(
        &mut runtime, 1,
        json!({"op":"play","runtime":token,"source":"root","progressive":true}),
    );
    assert!(commands::command(&mut runtime, &mut app, &invocation).is_none());
    assert!(!transient::lifecycle_target_ready(&runtime, &profile, None));
    let sources = runtime.local_inventory.sources.clone();
    let generation = runtime.generation;
    transient::binding_result(
        &mut runtime,
        &mut app,
        generation,
        Ok(ott_catalog::LocalBindings {
            sources,
            lookup: None,
            matched: vec![],
            operation_error: None,
        }),
    );
    assert!(matches!(
        runtime.local_inventory.phase,
        transient::InventoryPhase::Failed
    ));
    assert!(transient::lifecycle_target_ready(&runtime, &profile, None));
    assert!(runtime.transient.candidate.is_some());
    assert!(
        runtime.resolver_next.is_none(),
        "catalogue is still being prepared"
    );
}

#[test]
fn metadata_readiness_follows_the_browsed_profile_not_the_accepted_local_queue() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    let mut runtime = app.ott.take().unwrap();
    runtime.local_inventory.phase = transient::InventoryPhase::Failed;
    runtime.metadata_target = Some((permanent().id, "programme".into()));
    assert!(transient::lifecycle_metadata_ready(&runtime));
    runtime.metadata_target = Some((transient::source_profile(&root).unwrap().id, "first".into()));
    assert!(!transient::lifecycle_metadata_ready(&runtime));
}

#[test]
fn failed_validation_retains_durable_local_favorite_restore_until_explicit_retry() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for spawn_refused in [false, true] {
        let (_root_dir, mut app, root) = setup();
        let id = add_root_proof(&mut app, &root);
        let mut runtime = app.ott.take().unwrap();
        let profile = transient::source_profile(&root).unwrap();
        crate::app::ott_local_sources::RememberedFolders::read(&runtime.saved.settings)
            .unwrap()
            .with_connected(crate::app::ott_local_sources::FolderDescriptor {
                kind: crate::app::ott_local_sources::LocalSourceKind::Folder,
                usb: None,
                id: root.id.clone(),
                name: root.name.clone(),
                path: root.canonical_path.clone().unwrap(),
            })
            .unwrap()
            .put_into(&mut runtime.saved.settings)
            .unwrap();
        runtime.saved.favorites_selected = true;
        runtime.saved.resume.insert(
            "favorites".into(),
            SavedResume {
                entry_id: id.clone(),
                position_ms: 12_500,
                source_fingerprint: profile_fingerprint(&profile),
            },
        );
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        runtime.local_inventory.phase = transient::InventoryPhase::Unloaded;
        runtime.resume_source_work();
        // The actual load path creates the durable pending restore. The held
        // catalogue slot prevents starting an IO worker in this fixture.
        runtime.load_catalog();
        let expected = continuity::ResumePoint {
            entry_id: id.clone(),
            position_ms: 12_500,
        };
        assert_eq!(runtime.continuity.pending_restore(), Some(&expected));
        let generation = runtime.generation;
        runtime.local_inventory.phase = transient::InventoryPhase::Loading(generation);
        if spawn_refused {
            transient::launch_failed(&mut runtime);
        } else {
            transient::binding_result(
                &mut runtime,
                &mut app,
                generation,
                Err(local::LocalError::NativeError),
            );
        }
        assert!(matches!(
            runtime.local_inventory.phase,
            transient::InventoryPhase::Failed
        ));
        assert_eq!(
            runtime.continuity.pending_restore(),
            Some(&expected),
            "failed validation does not establish that the saved source was lost"
        );
        assert!(runtime.resolver_next.is_none());

        let token = runtime.remote.runtime().to_owned();
        let invocation = local_invocation(
            &mut runtime, 1, json!({"op":"sources","runtime":token}),
        );
        assert!(commands::command(&mut runtime, &mut app, &invocation).is_none());
        let sources = runtime.local_inventory.sources.clone();
        let generation = runtime.generation;
        transient::binding_result(
            &mut runtime,
            &mut app,
            generation,
            Ok(ott_catalog::LocalBindings {
                sources,
                lookup: None,
                matched: vec![],
                operation_error: None,
            }),
        );
        assert!(matches!(
            runtime.local_inventory.phase,
            transient::InventoryPhase::Ready(_)
        ));
        assert_eq!(runtime.continuity.pending_restore(), Some(&expected));
        let decision = runtime
            .continuity
            .catalog_updated(runtime.generation, &[id.as_str()], true);
        assert_eq!(
            decision,
            continuity::Decision::Start {
                entry_id: id,
                position_ms: 12_500,
                reason: continuity::Reason::Restore,
            }
        );
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(runtime.resolver_next.is_none());
    }
}

#[test]
fn explicit_local_retry_queues_the_first_scan_batch_before_inventory_publication() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, _) = setup();
    let mut runtime = app.ott.take().unwrap();
    runtime.resume_source_work();
    runtime.local_inventory.phase = transient::InventoryPhase::Failed;
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let old_owner = runtime.transient.accepted.as_ref().unwrap().owner();
    let token = runtime.remote.runtime().to_owned();
    let invocation = local_invocation(
        &mut runtime, 1,
        json!({"op":"play","runtime":token,"source":"root","progressive":true}),
    );
    assert!(commands::command(&mut runtime, &mut app, &invocation).is_none());
    let sources = runtime.local_inventory.sources.clone();
    let generation = runtime.generation;
    transient::binding_result(
        &mut runtime,
        &mut app,
        generation,
        Ok(ott_catalog::LocalBindings {
            sources,
            lookup: None,
            matched: vec![],
            operation_error: None,
        }),
    );
    assert!(matches!(
        runtime.local_inventory.phase,
        transient::InventoryPhase::Failed
    ));
    assert!(
        runtime.resolver_busy,
        "the existing resolver slot stays held"
    );
    let mut incoming = Catalog { hierarchy: None,
        entries: vec![item("candidate-first")],
    };
    let mut finished = None;
    assert!(transient::scan_result(
        &mut runtime,
        &mut app,
        &mut incoming,
        &mut finished,
    ));
    assert!(incoming.entries.is_empty());
    assert_eq!(
        runtime
            .pending_play
            .as_ref()
            .map(|pending| pending.id.as_str()),
        Some(invocation.id.as_str()),
        "a valid first batch must reach native admission, not fail its own readiness guard"
    );
    assert_eq!(
        runtime
            .play_target
            .as_ref()
            .map(|request| request.id.as_str()),
        Some("candidate-first")
    );
    assert_eq!(
        runtime
            .resolver_next
            .as_ref()
            .and_then(|request| request.entry.as_ref())
            .map(|entry| entry.id.as_str()),
        Some("candidate-first")
    );
    assert!(runtime.transient.accepted.as_ref().unwrap().owner() == old_owner);
    assert!(matches!(
        runtime.transient.candidate.as_ref(),
        Some(transient::Candidate::Local(transient::LocalCandidate {
            phase: transient::CandidatePhase::Scanning {
                proposed: Some(_),
                ..
            },
            ..
        }))
    ));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
}

#[test]
fn nonlocal_favorite_prepare_metadata_and_resume_ignore_failed_local_validation() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    let local_id = add_root_proof(&mut app, &root);
    let mut runtime = app.ott.take().unwrap();
    let favorite = Favorite {
        title: "Live programme".into(),
        origin: plx_data::ott::favorites::Origin {
            provider: "m3u".into(),
            profile: "Permanent".into(),
            subprofile: None,
            source_alias: "Permanent".into(),
        },
        locator: plx_data::ott::favorites::Locator::Channel {
            channel_id: "live".into(),
        },
    };
    let id = format!("favorite:{}", favorite.identity());
    runtime.saved.favorites.toggle(favorite).unwrap();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    runtime.local_inventory.phase = transient::InventoryPhase::Failed;
    runtime.resume_source_work();
    let request = PlaybackRequest::ordinary(&id, 7_500);
    assert_eq!(
        transient::lifecycle_resume_ready(&runtime, &request),
        Ok(true)
    );
    let (prepared, profile, entry, favorite) = runtime
        .prepare_play_request(request.clone())
        .expect("a failed unrelated Local inventory must not block an M3U favorite");
    assert_eq!(profile, permanent());
    assert!(entry.is_none() && favorite.is_some());
    assert!(prepared.local_selection.is_none());
    assert_eq!(prepared.position_ms, 7_500);
    assert!(
        prepared.transient_owner
            == runtime
                .transient
                .accepted
                .as_ref()
                .map(transient::TransientMedia::owner)
    );
    runtime.request_favorite_metadata(&id).unwrap();
    assert_eq!(runtime.metadata_target, Some((permanent().id, id.clone())));
    assert!(runtime.metadata_local_selection.is_none());
    assert!(runtime
        .metadata_next
        .as_ref()
        .is_some_and(|(profile, _)| profile.kind == ProviderKind::M3u));
    assert!(!runtime.metadata_done.load(Ordering::Acquire));

    // This is the actual method called by the runtime after the resume guard,
    // with the original retained position. Its resolver remains held by setup.
    runtime.play_request(request).unwrap();
    let queued = runtime.play_target.as_ref().unwrap();
    assert_eq!(queued.id, id);
    assert_eq!(queued.position_ms, 7_500);
    assert!(queued.local_selection.is_none());
    assert!(runtime
        .resolver_next
        .as_ref()
        .is_some_and(
            |request| request.profile.kind == ProviderKind::M3u && request.favorite.is_some()
        ));

    let local_request = PlaybackRequest::ordinary(&local_id, 0);
    assert_eq!(
        transient::lifecycle_resume_ready(&runtime, &local_request),
        Ok(false),
        "profile-only fallback must not manufacture a Local root binding"
    );
    assert!(runtime.prepare_play_request(local_request).is_err());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
}
