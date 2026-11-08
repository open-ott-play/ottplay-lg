//! Source-only regression packet. Actual owned grants and dispatcher state are
//! used; modeled ACKs below do not claim protected-disk or decoder execution.
use super::*;

fn remembered(app: &mut App, root: &local::BoundSource, stable: bool) {
    let runtime = app.ott.as_mut().unwrap();
    let mut row = json!({"id":root.id,"name":root.name,"path":root.canonical_path});
    if stable {
        row["usb"] = json!({"namespace":"linux-filesystem-uuid-v1", "uuid":"offline-ssd-1",
            "filesystem_type":"ext4", "filesystem_relative_folder":"Movies"});
    }
    runtime.saved.settings.insert(
        "nativeLocalSources".into(),
        json!({"version":if stable {2}else{1},"folders":[row]}),
    );
    runtime.entropy = Some([3; 16]);
    runtime.saving = false;
    runtime.resume_source_work();
}

#[test]
fn folder_kiosk_reserves_one_bounded_resume_slot_in_its_saved_candidate() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, root) = setup();
    remembered(&mut app, &root, true);
    install(&mut app, Some(&root), "first");
    let runtime = app.ott.as_ref().unwrap();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let prepared = kiosk_source::prepare(runtime, &app, Some("folder:Movies"), true).unwrap();
    let candidate = kiosk_source::save_candidate(runtime, &prepared).unwrap();
    assert!(candidate.resume.contains_key("@kiosk.local"));
    assert_eq!(candidate.resume["@kiosk.local"].entry_id, "first");
    assert_eq!(candidate.resume["@kiosk.local"].position_ms, 0);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(
        runtime.checkpoints.is_empty(),
        "preflight cannot publish a checkpoint"
    );
}

#[test]
fn full_unrelated_resume_map_refuses_before_kiosk_policy_or_playback_mutation() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, root) = setup();
    remembered(&mut app, &root, true);
    install(&mut app, Some(&root), "first");
    let runtime = app.ott.as_mut().unwrap();
    for n in 0..91 {
        runtime.saved.resume.insert(
            format!("profile-{n}"),
            SavedResume {
                entry_id: "other".into(),
                position_ms: 1,
                source_fingerprint: "other".into(),
            },
        );
    }
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let runtime = app.ott.take().unwrap();
    // Use the same source preflight as the command; no write or source mutation.
    let error = kiosk_source::prepare(&runtime, &app, Some("folder:Movies"), true)
        .err()
        .expect("capacity refusal");
    assert_eq!(error.message(), "Playback resume storage is full");
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(runtime.play_target.is_none() && runtime.pending_play.is_none());
    app.ott = Some(runtime);
}

#[test]
fn non_usb_folder_and_one_shot_local_do_not_gain_durable_resume() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, root) = setup();
    remembered(&mut app, &root, false);
    install(&mut app, Some(&root), "first");
    let runtime = app.ott.as_ref().unwrap();
    let prepared = kiosk_source::prepare(runtime, &app, Some("folder:Movies"), true).unwrap();
    assert!(!kiosk_source::save_candidate(runtime, &prepared)
        .unwrap()
        .resume
        .contains_key("@kiosk.local"));
    app.ott.as_mut().unwrap().saving = true; // no writer; real checkpoint stays memory-only
    checkpoint(&mut app, 12345);
    assert!(!app
        .ott
        .as_ref()
        .unwrap()
        .checkpoints
        .contains_key("@kiosk.local"));
}

/// Exercise the real Saved-ACK handler, while explicitly modeling the writer
/// outcome. Protected-disk execution belongs to the separate ignored fixture.
fn designate(app: &mut App) {
    let mut runtime = app.ott.take().unwrap();
    let prepared = kiosk_source::prepare(&runtime, app, Some("folder:Movies"), true).unwrap();
    let candidate = kiosk_source::save_candidate(&runtime, &prepared).unwrap();
    runtime.saving = true;
    runtime
        .tx
        .send(Job::Saved(Ok(candidate), None, false, ok(json!({}))))
        .unwrap();
    app.ott = Some(runtime);
    runtime_tick(app);
    let mut runtime = app.ott.take().unwrap();
    assert!(runtime.saved.resume.contains_key("@kiosk.local"));
    kiosk_source::activate(&mut runtime, app, &prepared).unwrap();
    assert!(runtime.resolver_next.is_some());
    runtime.cancel_play_request(); // model cancelled designation replay; existing admitted file stays
    runtime.saving = true; // hold writer; subsequent checkpoint content is inspected
    app.ott = Some(runtime);
}

/// Qualify only this fixture's saved USB intent through the real worker path.
/// Mount/identity observations are synthetic; the opened root and device check are not.
fn qualify_remembered_usb(
    app: &mut App,
    root: &local::BoundSource,
) -> plx_platform::tv::removable::MountCandidate {
    use crate::app::{ott_local_sources::RememberedFolders, ott_usb as usb};
    use plx_platform::tv::removable::{identity, MountCandidate};

    let runtime = app.ott.as_mut().unwrap();
    let folders = RememberedFolders::read(&runtime.saved.settings).unwrap();
    let binding = folders.folders().iter().find(|folder| folder.id == root.id)
        .unwrap().usb.clone().expect("remembered stable USB intent");
    let off = plx_base::task::OffFrame::for_test();
    let mount_path = std::path::Path::new(root.canonical_path.as_deref().unwrap())
        .parent().unwrap();
    let directory = std::fs::File::open(mount_path).unwrap();
    let device = identity::directory_device(&off, &directory).unwrap();
    let mount = MountCandidate::directory_device_for_test(
        mount_path.to_str().unwrap(), 8, device,
    ).unwrap();
    let mut dormant = root.clone();
    dormant.available = false;
    let usb::Answer::Restored { source, relocation } = usb::execute_with_identity(
        &off,
        usb::Task::Restore { binding: binding.clone(), source: dormant },
        &AtomicBool::new(false),
        &mut |_, _| Ok(vec![mount.clone()]),
        &mut |_, mounts, _| Ok(mounts.iter().map(|observed| {
            identity::IdentityObservation::for_test(
                observed.clone(),
                identity::StableIdentity::new(&binding.uuid, observed.filesystem()).unwrap(),
            )
        }).collect()),
    ).unwrap() else {
        panic!("verified USB restore");
    };
    assert!(relocation.is_current(&source));
    assert!(source.is_live());
    assert!(source.usb_witness().unwrap().is_observed(&[mount.clone()], root));
    assert!(source.usb_witness().unwrap().stable_binding() == Some(&binding));
    let Some(transient::TransientMedia::Local(session)) = runtime.transient.accepted.as_ref()
    else { panic!("initial Local session"); };
    let current = session.context.source();
    let restored = source.descriptor();
    assert!(current.id == restored.id && current.name == restored.name
        && current.kind == restored.kind && current.available == restored.available
        && current.authority_ref == restored.authority_ref
        && current.generation == restored.generation
        && current.canonical_path == restored.canonical_path);
    runtime.local_inventory.install(vec![source], runtime.generation);
    let selection = runtime.bind_local_selection(root).unwrap();
    let read = runtime.capture_local_source(&selection).unwrap();
    assert!(runtime.local_source_read_is_current(&read));
    let Some(transient::TransientMedia::Local(session)) = runtime.transient.accepted.as_mut()
    else { panic!("initial Local session"); };
    session.read = read;
    mount
}

#[test]
fn durable_activation_checkpoints_same_file_but_new_same_folder_command_is_temporary() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, root) = setup();
    remembered(&mut app, &root, true);
    let mount = qualify_remembered_usb(&mut app, &root);
    install(&mut app, Some(&root), "first");
    designate(&mut app);
    checkpoint(&mut app, 23456);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.checkpoints["@kiosk.local"].entry_id, "first");
    assert_eq!(runtime.checkpoints["@kiosk.local"].position_ms, 23456);
    let before = runtime.checkpoints["@kiosk.local"].clone();
    let mut runtime = app.ott.take().unwrap();
    runtime.saving = false;
    runtime.resolver_busy = true;
    let params = json!({"op":"shuffle","runtime":runtime.remote.runtime(),
        "source":root.id,"progressive":true,"session":true});
    let saved_before = serde_json::to_value(&runtime.saved).unwrap();
    let invocation = local_invocation(&mut runtime, 801, params.clone());
    let rejected = commands::command(&mut runtime, &mut app, &invocation)
        .expect("strict kiosk must reject a source-changing command");
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved_before);
    assert!(runtime.checkpoints["@kiosk.local"] == before);
    assert!(runtime.transient.candidate.is_none());

    // Model a verified writer result through the real Saved handler, as in
    // designate(). Turning kiosk off deliberately removes its durable slot.
    let mut off = runtime.saved.clone();
    off.settings.remove("nativeKiosk");
    kiosk_resume::merge(&mut off, &runtime.checkpoints);
    kiosk_resume::sanitize(&mut off);
    runtime.saving = true;
    runtime.save_navigation = SaveNavigation::Preserve;
    runtime.save_playback_epoch = runtime.playback_epoch;
    runtime.tx.send(Job::Saved(Ok(off), None, false, ok(json!({})))).unwrap();
    app.ott = Some(runtime);
    runtime_tick(&mut app);
    let mut runtime = app.ott.take().unwrap();
    assert!(!runtime.saved.settings.contains_key("nativeKiosk"));
    assert!(!runtime.saved.resume.contains_key("@kiosk.local"));
    assert!(!runtime.checkpoints.contains_key("@kiosk.local"));
    runtime.saving = false;
    runtime.resolver_busy = true;
    let invocation = local_invocation(&mut runtime, 802, params);
    assert!(commands::command(&mut runtime, &mut app, &invocation).is_none());
    let transient::Candidate::Local(candidate) = runtime.transient.candidate.as_mut().unwrap()
    else {
        panic!("Local candidate");
    };
    let transient::CandidatePhase::Binding(binding) = &mut candidate.phase else {
        panic!("binding");
    };
    let request = binding.take().unwrap();
    let generation = runtime.generation;
    let answer = crate::app::ott_catalog::validate_local_bindings_observed_for_test(
        &plx_base::task::OffFrame::for_test(),
        request,
        &AtomicBool::new(false),
        &mut |_, _| Ok(vec![mount.clone()]),
    )
    .unwrap();
    assert!(answer.operation_error.is_none());
    assert_eq!(answer.sources.len(), 1);
    assert!(answer.sources[0].is_live());
    assert!(answer.sources[0].same_grant(&runtime.local_inventory.sources[0]));
    transient::binding_result(&mut runtime, &mut app, generation, Ok(answer));
    assert!(matches!(runtime.transient.candidate.as_ref(),
        Some(transient::Candidate::Local(candidate))
            if matches!(&candidate.phase, transient::CandidatePhase::Scanning { .. })));
    let mut incoming = Catalog {
        hierarchy: None, entries: vec![item("new-first")],
    };
    transient::scan_result(&mut runtime, &mut app, &mut incoming, &mut None);
    assert_eq!(
        runtime.play_target.as_ref().unwrap().position_ms,
        0,
        "an explicit same-folder command never resumes the durable kiosk offset"
    );
    app.ott = Some(runtime);
    admit_pending(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    runtime.saving = true;
    let Some(transient::TransientMedia::Local(session)) = runtime.transient.accepted.as_ref()
    else {
        panic!("Local");
    };
    assert!(session.kiosk_resume.is_none());
    checkpoint(&mut app, 999);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.checkpoints.contains_key("@kiosk.local"));
    assert!(!runtime.saved.resume.contains_key("@kiosk.local"));
}

fn admit_pending(app: &mut App) {
    let mut runtime = app.ott.take().unwrap();
    let request = runtime.play_target.as_ref().unwrap().clone();
    let (profile, entry) = transient::staged_native_binding(&runtime, &request)
        .unwrap()
        .unwrap();
    let relative = match &entry.media {
        MediaLocator::Local { relative_path } => relative_path,
        _ => panic!("Local"),
    };
    let path = std::path::Path::new(&profile.source).join(relative);
    std::fs::write(
        &path,
        b"synthetic fixture bytes; media decoding is not claimed",
    )
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
    info.audio_sample_rate = 48000;
    info.audio_bit_depth = 16;
    let before = plx_media::route::play_request_ticket();
    // Match start_resolved_with: retire the prior route after capturing its
    // receipt, without scheduling exit navigation. The engine remains inert.
    playback::replace_player_in_place(
        &mut app.player, &mut app.adapters.player, &mut app.repause_at, &mut app.pages,
    );
    plx_media::route::request_external_source(
        &mut app.player.session,
        &info,
        source.clone(),
        &entry.id,
        "Movies",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    // Model only the existing native-start receipt protocol, not decoding.
    let pending = plx_media::route::pending_route_start().expect("prepared route");
    let attempt = plx_media::route::claim_route_start_attempt(pending).expect("current route attempt");
    assert!(plx_media::route::settle_route_start(
        &mut app.player.session, attempt, plx_media::route::RouteStartResult::Started,
    ));
    let after = plx_media::route::play_request_ticket();
    let identity = plx_media::route::playback_identity(&app.player.session);
    assert!(identity.is_some());
    runtime.playing = Some(Playing { channel_capture: None,
        archive: None,
        kiosk_selection: None,
        identity,
        profile,
        entry,
        selection_id: request.id,
        source: Some(source),
        advance_on_failure: false,
        requires_server_lease: false,
        transient_owner: request.transient_owner,
        local_selection: request.local_selection,
    });
    let native = NativeStartReceipt {
        generation: runtime.play_generation,
        before,
        after,
    };
    // Job::Play clears its transient opening status on success before the
    // Local completion publishes any retained resume-capacity warning.
    runtime.status.clear();
    let answer = transient::finish_native(&mut runtime, app, Ok(()), Some(&native)).unwrap();
    assert_eq!(answer["data"]["state"], "ready");
    runtime.play_target = None;
    runtime.resolver_next = None;
    app.ott = Some(runtime);
}

fn cold_prefix(app: &mut App, root: &local::BoundSource, first: &str) {
    let mut runtime = app.ott.take().unwrap();
    // Reconstruct the saved document through its public serde and validation
    // boundary, then start the existing typed KioskFolder preparation.
    let bytes = serde_json::to_vec(&runtime.saved).unwrap();
    runtime.saved = serde_json::from_slice(&bytes).unwrap();
    runtime.saved.validate().unwrap();
    runtime.checkpoints.clear();
    runtime.playing = None;
    transient::discard(&mut runtime);
    runtime.cancel_play_request();
    runtime.resolver_busy = false;
    runtime.saving = false;
    let fingerprint = selection::local_root_fingerprint(root).unwrap();
    transient::kiosk_restore_local_folder(&mut runtime, app, &root.id, &fingerprint).unwrap();
    runtime.resolver_busy = true;
    let mut incoming = Catalog {
        hierarchy: None, entries: vec![item(first)],
    };
    transient::scan_result(&mut runtime, app, &mut incoming, &mut None);
    assert_eq!(runtime.play_target.as_ref().unwrap().position_ms, 0);
    app.ott = Some(runtime);
    admit_pending(app);
}

#[test]
fn cold_kiosk_starts_first_available_then_restores_saved_file_once() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, root) = setup();
    remembered(&mut app, &root, true);
    install(&mut app, Some(&root), "first");
    designate(&mut app);
    checkpoint(&mut app, 45678);
    {
        let runtime = app.ott.as_mut().unwrap();
        kiosk_resume::merge(&mut runtime.saved, &runtime.checkpoints);
    }
    cold_prefix(&mut app, &root, "fallback");
    assert_eq!(
        app.ott
            .as_ref()
            .unwrap()
            .continuity
            .pending_restore()
            .unwrap()
            .entry_id,
        "first"
    );
    app.ott.as_mut().unwrap().saving = true;
    checkpoint(&mut app, 123);
    assert!(
        app.ott.as_ref().unwrap().checkpoints.is_empty(),
        "fallback cannot overwrite restore"
    );
    let mut runtime = app.ott.take().unwrap();
    runtime.catalog.entries.push(item("first"));
    transient::accepted_catalog_update(&mut runtime);
    let ids = runtime.ordered_ids();
    let refs: Vec<_> = ids.iter().map(String::as_str).collect();
    let decision = runtime
        .continuity
        .catalog_updated(runtime.generation, &refs, false);
    assert_eq!(
        decision,
        continuity::Decision::Start {
            entry_id: "first".into(),
            position_ms: 45678,
            reason: continuity::Reason::Restore
        }
    );
    runtime.saving = false;
    runtime.apply_continuity(decision);
    assert_eq!(runtime.play_target.as_ref().unwrap().position_ms, 45678);
    assert_eq!(
        runtime.restore_play_generation,
        Some(runtime.play_generation)
    );
    app.ott = Some(runtime);
    checkpoint(&mut app, 321);
    assert!(
        app.ott.as_ref().unwrap().checkpoints.is_empty(),
        "in-flight restore also protects point"
    );
}

#[test]
fn off_or_different_ssd_discards_old_checkpoint_in_both_save_and_shutdown_projection() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, root) = setup();
    remembered(&mut app, &root, true);
    install(&mut app, Some(&root), "first");
    designate(&mut app);
    checkpoint(&mut app, 6789);
    let runtime = app.ott.as_ref().unwrap();
    for change in 0..3 {
        let mut saved = runtime.saved.clone();
        match change {
            0 => {
                saved.settings.remove("nativeKiosk");
            }
            1 => {
                saved.settings.get_mut("nativeLocalSources").unwrap()["folders"][0]["usb"]["uuid"] =
                    json!("other-ssd")
            }
            _ => {
                saved.settings.get_mut("nativeLocalSources").unwrap()["folders"][0]["usb"]
                    ["filesystem_relative_folder"] = json!("Other")
            }
        }
        kiosk_resume::merge(&mut saved, &runtime.checkpoints);
        kiosk_resume::sanitize(&mut saved);
        assert!(!saved.resume.contains_key("@kiosk.local"));
        // A repeated shutdown merge must not resurrect the still-live old map.
        kiosk_resume::merge(&mut saved, &runtime.checkpoints);
        kiosk_resume::sanitize(&mut saved);
        assert!(!saved.resume.contains_key("@kiosk.local"));
    }
}

#[test]
fn revoked_grant_cannot_write_a_new_durable_position() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, root) = setup();
    remembered(&mut app, &root, true);
    install(&mut app, Some(&root), "first");
    designate(&mut app);
    checkpoint(&mut app, 12345);
    let before = app.ott.as_ref().unwrap().checkpoints.clone();
    app.ott.as_mut().unwrap().local_inventory.sources[0].withdraw();
    checkpoint(&mut app, 99999);
    assert!(app.ott.as_ref().unwrap().checkpoints == before);
}

#[test]
fn old_full_map_kiosk_still_plays_but_reports_that_durable_resume_is_unavailable() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, root) = setup();
    remembered(&mut app, &root, true);
    let runtime = app.ott.as_mut().unwrap();
    let policy = crate::app::ott_kiosk_stored::Stored::local_folder(
        plx_data::ott::favorites::Origin {
            provider: "local".into(),
            profile: "Local videos".into(),
            subprofile: None,
            source_alias: root.name.clone(),
        },
        true,
        root.id.clone(),
        selection::local_root_fingerprint(&root).unwrap(),
    )
    .unwrap();
    runtime
        .saved
        .settings
        .insert("nativeKiosk".into(), policy.encode().unwrap());
    for n in 0..91 {
        runtime.saved.resume.insert(
            format!("unrelated-{n}"),
            SavedResume {
                entry_id: "other".into(),
                position_ms: 1,
                source_fingerprint: "other".into(),
            },
        );
    }
    cold_prefix(&mut app, &root, "first");
    let runtime = app.ott.as_ref().unwrap();
    let Some(transient::TransientMedia::Local(session)) = runtime.transient.accepted.as_ref()
    else {
        panic!("accepted Local");
    };
    assert!(session.kiosk_resume.is_none());
    assert_eq!(runtime.saved.resume.len(), 91);
    assert_eq!(runtime.status, "Playback resume storage is full");
    assert!(runtime.playing.is_some());
}

#[test]
fn saved_off_retires_pending_and_inflight_restore_without_cancelling_new_user_choice() {
    let _serial = plx_base::testlock::serial();
    for mode in 0..3 {
        let _reset = Reset;
        let (_dir, mut app, root) = setup();
        remembered(&mut app, &root, true);
        install(&mut app, Some(&root), "first");
        designate(&mut app);
        checkpoint(&mut app, 12345);
        {
            let r = app.ott.as_mut().unwrap();
            kiosk_resume::merge(&mut r.saved, &r.checkpoints);
        }
        cold_prefix(&mut app, &root, "fallback");
        let mut runtime = app.ott.take().unwrap();
        runtime.catalog.entries.push(item("first"));
        if mode == 1 {
            let decision = runtime.continuity.catalog_updated(
                runtime.generation,
                &["fallback", "first"],
                false,
            );
            runtime.apply_continuity(decision);
            assert_eq!(
                runtime.restore_play_generation,
                Some(runtime.play_generation)
            );
        } else if mode == 2 {
            runtime.play(&mut app, "first").unwrap();
            assert!(runtime.restore_play_generation.is_none());
        }
        let generation = runtime.play_generation;
        let user_target = runtime.play_target.clone();
        let mut off = runtime.saved.clone();
        off.settings.remove("nativeKiosk");
        kiosk_resume::merge(&mut off, &runtime.checkpoints);
        kiosk_resume::sanitize(&mut off);
        runtime.saving = true;
        runtime
            .tx
            .send(Job::Saved(Ok(off), None, false, ok(json!({}))))
            .unwrap();
        app.ott = Some(runtime);
        runtime_tick(&mut app);
        let r = app.ott.as_ref().unwrap();
        assert!(r.continuity.pending_restore().is_none());
        assert!(!r.saved.resume.contains_key("@kiosk.local"));
        assert!(!r.checkpoints.contains_key("@kiosk.local"));
        if mode == 1 {
            assert_ne!(r.play_generation, generation);
            assert!(r.play_target.is_none());
        }
        if mode == 2 {
            assert_eq!(r.play_generation, generation);
            assert!(r.play_target == user_target);
        }
    }
}

#[test]
fn rejected_policy_write_never_marks_the_temporary_session_durable() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, root) = setup();
    remembered(&mut app, &root, true);
    install(&mut app, Some(&root), "first");
    let mut runtime = app.ott.take().unwrap();
    let prepared = kiosk_source::prepare(&runtime, &app, Some("folder:Movies"), true).unwrap();
    let _candidate = kiosk_source::save_candidate(&runtime, &prepared).unwrap();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    assert!(kiosk_source::activate(&mut runtime, &mut app, &prepared).is_err());
    runtime.saving = true;
    runtime
        .tx
        .send(Job::Saved(
            Err("fixture writer rejected"),
            None,
            false,
            ok(json!({})),
        ))
        .unwrap();
    app.ott = Some(runtime);
    runtime_tick(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    runtime.saving = true;
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    let Some(transient::TransientMedia::Local(session)) = runtime.transient.accepted.as_ref()
    else {
        panic!("Local session");
    };
    assert!(session.kiosk_resume.is_none());
    checkpoint(&mut app, 555);
    assert!(!app
        .ott
        .as_ref()
        .unwrap()
        .checkpoints
        .contains_key("@kiosk.local"));
}

#[test]
fn ninetieth_unrelated_slot_allows_reservation_and_full_reserved_map_allows_update() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, root) = setup();
    remembered(&mut app, &root, true);
    install(&mut app, Some(&root), "first");
    let mut runtime = app.ott.take().unwrap();
    for n in 0..90 {
        runtime.saved.resume.insert(
            format!("unrelated-{n}"),
            SavedResume {
                entry_id: "other".into(),
                position_ms: 1,
                source_fingerprint: "other".into(),
            },
        );
    }
    let prepared = kiosk_source::prepare(&runtime, &app, Some("folder:Movies"), true).unwrap();
    let candidate = kiosk_source::save_candidate(&runtime, &prepared).unwrap();
    assert_eq!(candidate.resume.len(), 91);
    candidate.validate().unwrap();
    runtime.saved = candidate;
    runtime
        .saved
        .resume
        .get_mut("@kiosk.local")
        .unwrap()
        .position_ms = 98765;
    let prepared = kiosk_source::prepare(&runtime, &app, Some("folder:Movies"), true).unwrap();
    let next = kiosk_source::save_candidate(&runtime, &prepared).unwrap();
    assert_eq!(next.resume.len(), 91);
    assert_eq!(next.resume["@kiosk.local"].position_ms, 98765);
    app.ott = Some(runtime);
}
