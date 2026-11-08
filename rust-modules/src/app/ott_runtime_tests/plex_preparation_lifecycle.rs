//! Source-only lifecycle regressions. Existing held worker slots are drained by
//! real Job terminal events; these tests never perform network or decoder IO.
use super::*;
use crate::app::ott_runtime::work_lifecycle::RestoreStartError;

#[test]
fn failed_preparation_retains_addressed_receipt_and_observes_real_worker_occupancy() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    register_source(&mut app, "wanted");
    let entries = kiosk_entries(&app, "wanted", &["42", "42"]);
    let owner = begin_kiosk_restore(&mut app, &entries);
    let mut runtime = app.ott.take().unwrap();
    let original = runtime.kiosk_preparation_receipt_for_test().unwrap();
    transient::plex_remote::prepared(
        &mut runtime,
        &mut app,
        owner,
        transient::plex_remote::PreparationId::KioskRestore,
        Err(queue::WireError::Queue(queue::QueueError::MissingItem)),
    );
    let terminal = runtime.kiosk_preparation_receipt_for_test().unwrap();
    assert!(original.same_attempt(&terminal));
    assert!(transient::plex_remote::kiosk_preparation_is_current(
        &runtime,
        &app,
        terminal.plex().unwrap()
    ));
    assert_eq!(
        transient::plex_remote::retry_kiosk_preparation(
            &mut runtime,
            &mut app,
            terminal.plex().unwrap()
        ),
        Err(RestoreStartError::Busy)
    );
    runtime
        .tx
        .send(Job::PlexPrepared(
            owner,
            transient::plex_remote::PreparationId::KioskRestore,
            Err(queue::WireError::Queue(queue::QueueError::MissingItem)),
        ))
        .unwrap_or_else(|_| panic!("receiver"));
    app.ott = Some(runtime);
    runtime_tick(&mut app);
    let mut runtime = app.ott.take().unwrap();
    assert!(!runtime.resolver_busy);
    assert!(transient::plex_remote::kiosk_preparation_is_current(
        &runtime,
        &app,
        terminal.plex().unwrap()
    ));
    runtime.playback_epoch = runtime.playback_epoch.wrapping_add(1);
    assert!(!transient::plex_remote::kiosk_preparation_is_current(
        &runtime,
        &app,
        terminal.plex().unwrap()
    ));
    app.ott = Some(runtime);
}

#[test]
fn expired_preparation_has_a_terminal_failure_receipt_despite_its_cancelled_worker() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    register_source(&mut app, "wanted");
    let entries = kiosk_entries(&app, "wanted", &["42"]);
    begin_kiosk_restore(&mut app, &entries);
    let mut runtime = app.ott.take().unwrap();
    let cancel = match runtime.transient.candidate.as_mut().unwrap() {
        transient::Candidate::Plex(candidate) => {
            let transient::plex_remote::Purpose::KioskRestore { expires_ms, .. } =
                &mut candidate.work.purpose
            else {
                panic!("typed restore")
            };
            *expires_ms = 0;
            candidate.work.cancel.clone()
        }
        _ => panic!("Plex candidate"),
    };
    transient::plex_remote::pending_tick(&mut runtime, &mut app);
    assert!(runtime.transient.candidate.is_none());
    assert!(cancel.load(Ordering::Acquire));
    assert!(runtime.resolver_busy);
    let receipt = runtime.kiosk_preparation_receipt_for_test().unwrap();
    assert!(transient::plex_remote::kiosk_preparation_is_current(
        &runtime,
        &app,
        receipt.plex().unwrap()
    ));
    app.ott = Some(runtime);
}

#[test]
fn explicit_cancel_retires_the_receipt_instead_of_scheduling_a_failure_retry() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    register_source(&mut app, "wanted");
    let entries = kiosk_entries(&app, "wanted", &["42"]);
    begin_kiosk_restore(&mut app, &entries);
    let mut runtime = app.ott.take().unwrap();
    let receipt = runtime.kiosk_preparation_receipt_for_test().unwrap();
    transient::plex_remote::cancel_candidate(&mut runtime, &mut app);
    assert!(runtime.kiosk_preparation_receipt_for_test().is_none());
    assert!(!transient::plex_remote::kiosk_preparation_is_current(
        &runtime,
        &app,
        receipt.plex().unwrap()
    ));
    app.ott = Some(runtime);
}

#[test]
fn resume_policy_uses_current_strictness_without_requiring_foreground_or_reviving_changed_source() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    register_source(&mut app, "wanted");
    let entries = kiosk_entries(&app, "wanted", &["42"]);
    begin_kiosk_restore(&mut app, &entries);
    let mut runtime = app.ott.take().unwrap();
    let receipt = runtime.kiosk_preparation_receipt_for_test().unwrap();
    app.ott_standby = true;
    let policy = crate::app::ott_kiosk_stored::Stored::queue(entries, false).unwrap();
    runtime.saved.settings.insert(
        crate::app::ott_kiosk_stored::KEY.into(),
        policy.encode().unwrap(),
    );
    assert_eq!(
        transient::plex_remote::kiosk_preparation_resume_policy(
            &runtime,
            &app,
            receipt.plex().unwrap()
        ),
        Some(policy)
    );
    runtime
        .saved
        .profiles
        .profiles
        .iter_mut()
        .find(|p| p.id == "plex-wanted")
        .unwrap()
        .source = "changed".into();
    assert!(
        transient::plex_remote::kiosk_preparation_resume_policy(
            &runtime,
            &app,
            receipt.plex().unwrap()
        )
        .is_none()
    );
    app.ott = Some(runtime);
}

#[test]
fn staged_native_binding_is_exact_and_does_not_publish_the_candidate_catalog() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "wanted");
    let (owner, id) = begin_plex(
        &mut app,
        "plex_queue",
        json!({"op":"play","ids":["42","7"]}),
    );
    let mut runtime = app.ott.take().unwrap();
    transient::plex_remote::prepared(
        &mut runtime,
        &mut app,
        owner,
        transient::plex_remote::PreparationId::Remote(id),
        Ok(proposal(
            owner,
            sid,
            prepare::Prepared::Queue {
                kind: queue::PrepareKind::Play,
                selection: selection(&["42", "7"]),
            },
        )),
    );
    let request = runtime.play_target.clone().unwrap();
    let (_, entry) = transient::plex_remote::staged_native_binding(&runtime, &request)
        .unwrap()
        .unwrap();
    assert_eq!(entry.id, request.id);
    assert_eq!(runtime.catalog.entries[0].id, "live");
    let mut wrong = request.clone();
    wrong.id = "live".into();
    assert!(
        transient::plex_remote::staged_native_binding(&runtime, &wrong)
            .unwrap()
            .is_err()
    );
    wrong = request;
    wrong.transient_owner = None;
    assert!(
        transient::plex_remote::staged_native_binding(&runtime, &wrong)
            .unwrap()
            .is_err()
    );
    app.ott = Some(runtime);
}

#[test]
fn invalid_native_fence_cannot_refresh_a_failed_preparation_receipt() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "wanted");
    let entries = kiosk_entries(&app, "wanted", &["42"]);
    let owner = begin_kiosk_restore(&mut app, &entries);
    let mut runtime = app.ott.take().unwrap();
    transient::plex_remote::prepared(
        &mut runtime,
        &mut app,
        owner,
        transient::plex_remote::PreparationId::KioskRestore,
        Ok(proposal(
            owner,
            sid,
            prepare::Prepared::Queue {
                kind: queue::PrepareKind::Play,
                selection: selection(&["42"]),
            },
        )),
    );
    let ticket = plx_media::route::play_request_ticket();
    let invalid = crate::app::ott_runtime::NativeStartReceipt {
        generation: runtime.play_generation.wrapping_add(1),
        before: ticket,
        after: ticket,
    };
    transient::plex_remote::finish_native(
        &mut runtime,
        &mut app,
        Err("native failure"),
        Some(&invalid),
    );
    assert!(runtime.kiosk_preparation_receipt_for_test().is_none());
    app.ott = Some(runtime);
}

#[test]
fn addressed_native_failure_refreshes_the_same_attempt_for_lifecycle_retry() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "wanted");
    let entries = kiosk_entries(&app, "wanted", &["42"]);
    let owner = begin_kiosk_restore(&mut app, &entries);
    let mut runtime = app.ott.take().unwrap();
    let initial = runtime.kiosk_preparation_receipt_for_test().unwrap();
    transient::plex_remote::prepared(
        &mut runtime,
        &mut app,
        owner,
        transient::plex_remote::PreparationId::KioskRestore,
        Ok(proposal(
            owner,
            sid,
            prepare::Prepared::Queue {
                kind: queue::PrepareKind::Play,
                selection: selection(&["42"]),
            },
        )),
    );
    let ticket = plx_media::route::play_request_ticket();
    let native = crate::app::ott_runtime::NativeStartReceipt {
        generation: runtime.play_generation,
        before: ticket,
        after: ticket,
    };
    transient::plex_remote::finish_native(&mut runtime, &mut app, Err("native failure"), Some(&native));
    let terminal = runtime.kiosk_preparation_receipt_for_test().unwrap();
    assert!(initial.same_attempt(&terminal));
    assert!(transient::plex_remote::kiosk_preparation_is_current(
        &runtime,
        &app,
        terminal.plex().unwrap()
    ));
    app.ott = Some(runtime);
}

#[test]
fn due_pending_preparation_times_out_without_releasing_the_real_worker() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    register_source(&mut app, "wanted");
    let entries = kiosk_entries(&app, "wanted", &["42"]);
    begin_kiosk_restore(&mut app, &entries);
    let mut runtime = app.ott.take().unwrap();
    let pending = runtime.kiosk_preparation_receipt_for_test().unwrap();
    assert_eq!(
        transient::plex_remote::retry_kiosk_preparation(
            &mut runtime,
            &mut app,
            pending.plex().unwrap()
        ),
        Err(RestoreStartError::Busy)
    );
    assert!(runtime.resolver_busy);
    assert!(runtime.transient.candidate.is_none());
    let failed = runtime.kiosk_preparation_receipt_for_test().unwrap();
    assert!(pending.same_attempt(&failed));
    assert!(transient::plex_remote::kiosk_preparation_is_current(
        &runtime,
        &app,
        failed.plex().unwrap()
    ));
    app.ott = Some(runtime);
}

#[test]
fn negotiated_next_and_previous_keep_v2_through_deferred_native_completion() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "large");
    let ids = vec!["42"; 1000];
    seed_selection(&mut app, sid, &ids, selection_for(&ids, true));
    for (op, index) in [("next", 1), ("previous", 0)] {
        assert!(command(&mut app, "plex_queue", json!({"op":op,"state_version":2})).is_none());
        let mut runtime = app.ott.take().unwrap();
        // This is the same cursor admission invoked by successful start_resolved.
        transient::plex_remote::admitted(&mut runtime);
        let reply =
            transient::plex_remote::finish_native(&mut runtime, &mut app, Ok(()), None).unwrap();
        assert_eq!(reply["data"]["version"], 2);
        assert_eq!(reply["data"]["index"], index);
        assert_eq!(reply["data"]["ids"].as_array().unwrap().len(), 1000);
        runtime.pending_play.take();
        runtime.play_target.take();
        runtime.resolver_next.take();
        app.ott = Some(runtime);
    }
}

#[test]
fn negotiated_step_native_error_keeps_v2_and_previous_cursor() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "large");
    let ids = vec!["42"; 1000];
    seed_selection(&mut app, sid, &ids, selection_for(&ids, true));
    assert!(
        command(
            &mut app,
            "plex_queue",
            json!({"op":"next","state_version":2})
        )
        .is_none()
    );
    let mut runtime = app.ott.take().unwrap();
    let reply =
        transient::plex_remote::finish_native(&mut runtime, &mut app, Err("native refused"), None)
            .unwrap();
    assert_eq!(reply["data"]["version"], 2);
    assert_eq!(reply["data"]["state"], "error");
    let Some(transient::TransientMedia::Plex(session)) = runtime.transient.accepted.as_ref() else {
        panic!("queue")
    };
    assert_eq!(session.queue.index(), 0);
    app.ott = Some(runtime);
}

#[test]
fn ordinary_runtime_stop_is_independent_of_legacy_queue_state_limit() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "large");
    let ids = vec!["42"; 1000];
    seed_selection(&mut app, sid, &ids, selection_for(&ids, true));
    let mut runtime = app.ott.take().unwrap();
    runtime.saving = true;
    runtime.stop_catalog(&mut app);
    assert!(runtime.playing.is_none());
    assert!(runtime.play_target.is_none());
    app.ott = Some(runtime);
}

#[test]
fn native_accepted_receipt_remains_current_and_waits_for_real_worker_occupancy() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "wanted");
    let entries = kiosk_entries(&app, "wanted", &["42"]);
    let owner = begin_kiosk_restore(&mut app, &entries);
    let mut runtime = app.ott.take().unwrap();
    transient::plex_remote::prepared(
        &mut runtime,
        &mut app,
        owner,
        transient::plex_remote::PreparationId::KioskRestore,
        Ok(proposal(
            owner,
            sid,
            prepare::Prepared::Queue {
                kind: queue::PrepareKind::Play,
                selection: selection(&["42"]),
            },
        )),
    );
    let (before, profile, entry) = match runtime.transient.candidate.as_ref().unwrap() {
        transient::Candidate::Plex(c) => (
            c.ticket,
            c.work.profile.clone(),
            c.staged.as_ref().unwrap().1.entries[0].clone(),
        ),
        _ => panic!("Plex candidate"),
    };
    install_native_identity(&mut app, sid, "42");
    runtime.playing = Some(Playing { channel_capture: None,
        archive: None,
        kiosk_selection: None,
        transient_owner: Some(owner),
        local_selection: None,
        identity: plx_media::route::playback_identity(&app.player.session),
        profile,
        selection_id: entry.id.clone(),
        entry,
        source: None,
        advance_on_failure: false,
        requires_server_lease: false,
    });
    let native = crate::app::ott_runtime::NativeStartReceipt {
        generation: runtime.play_generation,
        before,
        after: plx_media::route::play_request_ticket(),
    };
    transient::plex_remote::finish_native(&mut runtime, &mut app, Ok(()), Some(&native));
    let receipt = runtime.kiosk_preparation_receipt_for_test().unwrap();
    assert!(transient::plex_remote::kiosk_preparation_is_current(
        &runtime,
        &app,
        receipt.plex().unwrap()
    ));
    // The lifecycle owns the 60-second watchdog; the provider must not turn an
    // addressed NativeAccepted watchdog call into ContextChanged.
    runtime.play_target = None;
    runtime.resolver_next = None;
    assert_eq!(
        transient::plex_remote::retry_kiosk_preparation(
            &mut runtime,
            &mut app,
            receipt.plex().unwrap()
        ),
        Err(RestoreStartError::Busy)
    );
    app.ott = Some(runtime);
}

fn independent_item_marker(entry: Favorite) -> kiosk_source::PinnedSelection {
    use crate::app::ott_kiosk_stored as stored;
    // Produce an opaque marker through the real startup API in a different
    // Runtime. Never manufacture private PinnedSelection fields.
    let mut other = app_fixture();
    let mut runtime = other.ott.take().unwrap();
    runtime.loaded = true;
    runtime.resume_source_work();
    let pin = stored::Stored::item(entry, false).unwrap();
    runtime
        .saved
        .settings
        .insert(stored::KEY.into(), pin.encode().unwrap());
    assert!(kiosk_source::stage_startup_item(&mut runtime, &other).unwrap());
    runtime
        .resume_pending
        .take()
        .unwrap()
        .kiosk_selection
        .unwrap()
}

#[test]
fn staged_plex_binding_rejects_each_foreign_initial_request_field() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, _) = super::super::local_integration::setup();
    let local = match app
        .ott
        .as_ref()
        .unwrap()
        .transient
        .accepted
        .as_ref()
        .unwrap()
    {
        transient::TransientMedia::Local(session) => session.read.selection.clone(),
        _ => panic!("real Local source fixture"),
    };
    app.ott.as_mut().unwrap().saving = false;
    app.ott.as_mut().unwrap().resume_source_work();
    let sid = register_source(&mut app, "wanted");
    let marker = independent_item_marker(kiosk_entries(&app, "wanted", &["43"])[0].clone());
    install_native_identity(&mut app, sid, "41");
    let identity = plx_media::route::playback_identity(&app.player.session).unwrap();
    let (owner, id) = begin_plex(&mut app, "plex_queue", json!({"op":"play","ids":["42"]}));
    let mut runtime = app.ott.take().unwrap();
    transient::plex_remote::prepared(
        &mut runtime,
        &mut app,
        owner,
        transient::plex_remote::PreparationId::Remote(id),
        Ok(proposal(
            owner,
            sid,
            prepare::Prepared::Queue {
                kind: queue::PrepareKind::Play,
                selection: selection(&["42"]),
            },
        )),
    );
    let request = runtime.play_target.clone().unwrap();
    assert!(request.kiosk_selection.is_none());
    let before = runtime.catalog.entries[0].id.clone();
    for field in 0..5 {
        // The unchanged request must still be admissible at every iteration;
        // rejection cannot be explained by an unrelated expired owner/ticket.
        assert!(
            transient::plex_remote::staged_native_binding(&runtime, &request)
                .unwrap()
                .is_ok()
        );
        let mut changed = request.clone();
        match field {
            0 => changed.kiosk_selection = Some(marker.clone()),
            1 => changed.local_selection = Some(local.clone()),
            2 => changed.continuation = Some(identity.clone()),
            3 => changed.position_ms = 1,
            4 => changed.advance_on_failure = true,
            _ => unreachable!(),
        }
        assert!(
            transient::plex_remote::staged_native_binding(&runtime, &changed)
                .unwrap()
                .is_err(),
            "initial request field {field}"
        );
        assert_eq!(runtime.catalog.entries[0].id, before);
    }
    app.ott = Some(runtime);
}

#[test]
fn kiosk_restore_resume_rejects_a_foreign_item_marker_on_its_first_request() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let mut app = setup();
    let sid = register_source(&mut app, "wanted");
    let entries = kiosk_entries(&app, "wanted", &["42"]);
    let marker = independent_item_marker(kiosk_entries(&app, "wanted", &["43"])[0].clone());
    let owner = begin_kiosk_restore(&mut app, &entries);
    let mut runtime = app.ott.take().unwrap();
    transient::plex_remote::prepared(
        &mut runtime,
        &mut app,
        owner,
        transient::plex_remote::PreparationId::KioskRestore,
        Ok(proposal(
            owner,
            sid,
            prepare::Prepared::Queue {
                kind: queue::PrepareKind::Play,
                selection: selection(&["42"]),
            },
        )),
    );
    let receipt = runtime.kiosk_preparation_receipt_for_test().unwrap();
    assert!(
        transient::plex_remote::kiosk_preparation_resume_policy(
            &runtime,
            &app,
            receipt.plex().unwrap()
        )
        .is_some()
    );
    runtime.play_target.as_mut().unwrap().kiosk_selection = Some(marker);
    assert!(
        transient::plex_remote::kiosk_preparation_resume_policy(
            &runtime,
            &app,
            receipt.plex().unwrap()
        )
        .is_none()
    );
    runtime.play_target.as_mut().unwrap().kiosk_selection = None;
    assert!(
        transient::plex_remote::kiosk_preparation_resume_policy(
            &runtime,
            &app,
            receipt.plex().unwrap()
        )
        .is_some()
    );
    app.ott = Some(runtime);
}
