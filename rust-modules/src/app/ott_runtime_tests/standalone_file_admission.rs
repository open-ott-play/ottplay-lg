//! First-row admission uses the same File capability as its real catalogue.
//! Existing worker slots stay occupied; no resolver, decoder or writer runs.
use super::*;

fn staged(op: &str) -> (Files, App, LocalSource) {
    let files = Files::new();
    let mut app = app_fixture();
    let mut runtime = app.ott.take().unwrap();
    runtime.loaded = true;
    runtime.generation = 7;
    runtime.resume_source_work();
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    let path = files.descriptor(SourceKind::File).canonical_path.unwrap();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":"00000000000000000000000000000041","action":"local_media",
        "params":{"op":op,"runtime":runtime.remote.runtime(),"source":path},
        "expires_at":1020
    }]}).to_string();
    let now = runtime.now_ms();
    let mut invocations = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(invocations.len(), 1);
    let invocation = invocations.remove(0);
    assert!(runtime.remote.invocation_is_current(&invocation));
    assert!(commands::command(&mut runtime, &mut app, &invocation).is_none());
    let Some(transient::Candidate::Local(candidate)) = runtime.transient.candidate.as_mut() else {
        panic!("real Local command must create its candidate");
    };
    let transient::CandidatePhase::Binding(binding) = &mut candidate.phase else {
        panic!("binding phase");
    };
    let binding = binding.take().expect("shared catalogue slot holds the binding work");
    assert!(binding.allow_new && binding.restore_remembered);
    let generation = runtime.generation;
    let validated = crate::app::ott_catalog::validate_local_bindings_for_test(
        &OffFrame::for_test(), binding, &AtomicBool::new(false),
    ).expect("actual binding worker must authorize the requested file");
    assert!(validated.operation_error.is_none());
    assert_eq!(validated.sources.len(), 1);
    let source = validated.sources[0].clone();
    assert!(source.is_live() && source.descriptor().kind == SourceKind::File);
    transient::binding_result(&mut runtime, &mut app, generation, Ok(validated));
    let profile = match runtime.transient.candidate.as_ref() {
        Some(transient::Candidate::Local(candidate)) => match &candidate.phase {
            transient::CandidatePhase::Scanning { profile, .. } => profile.clone(),
            _ => panic!("File binding must enter catalogue scan"),
        },
        _ => panic!("File candidate retained"),
    };
    let mut incoming = Catalog::default();
    crate::app::ott_catalog::visit_single_file_for_test(&OffFrame::for_test(), &profile, &source,
        &AtomicBool::new(false), &mut |rows| {
            incoming.merge(Catalog { hierarchy: None, entries: rows }).map_err(|_| "fixture catalogue rejected")
        }).unwrap();
    assert_eq!(incoming.entries.len(), 1);
    assert!(transient::scan_result(&mut runtime, &mut app, &mut incoming, &mut Some(Ok(()))));
    assert!(incoming.entries.is_empty());
    assert!(runtime.play_target.is_some() && runtime.resolver_next.is_some());
    assert!(runtime.transient.accepted.is_none());
    assert!(runtime.catalog.entries.is_empty(), "staged catalogue is not yet published");
    app.ott = Some(runtime);
    (files, app, source)
}

#[test]
fn first_file_play_and_shuffle_pass_native_rebinding_without_directory_authority() {
    let _serial = plx_base::testlock::serial();
    for op in ["play", "shuffle"] {
        let (_files, mut app, source) = staged(op);
        let mut runtime = app.ott.take().unwrap();
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let request = runtime.play_target.as_ref().unwrap().clone();
        let (profile, entry) = transient::staged_native_binding(&runtime, &request).unwrap().unwrap();
        assert!(matches!(&entry.media, MediaLocator::Local { relative_path } if relative_path == "movie.mp4"));
        let queued = runtime.resolver_next.as_ref().unwrap().local_read.as_ref().unwrap();
        assert!(queued.source.same_grant(&source));
        assert!(queued.folder_read().is_none() && source.root_handle().is_none());
        assert_eq!(runtime.rebind_native_admission(&app, &profile, &entry), Ok(()));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert!(runtime.transient.accepted.is_none() && runtime.catalog.entries.is_empty());
        app.ott = Some(runtime);
    }
}

#[test]
fn file_first_admission_rejects_changed_request_and_sibling_media() {
    let _serial = plx_base::testlock::serial();
    let (_files, mut app, _source) = staged("play");
    let mut runtime = app.ott.take().unwrap();
    let request = runtime.play_target.as_ref().unwrap().clone();
    let (profile, entry) = transient::staged_native_binding(&runtime, &request).unwrap().unwrap();
    for field in 0..5 {
        let mut changed = request.clone();
        match field {
            0 => changed.local_selection.as_mut().unwrap().generation += 1,
            1 => changed.local_selection = None,
            2 => changed.id = "sibling.mp4".into(),
            3 => changed.position_ms = 1,
            _ => changed.advance_on_failure = true,
        }
        assert!(transient::staged_native_binding(&runtime, &changed).unwrap().is_err());
    }
    let mut sibling = entry.clone();
    sibling.media = MediaLocator::Local { relative_path: "sibling.mp4".into() };
    assert!(runtime.rebind_native_admission(&app, &profile, &sibling).is_err());
    assert!(runtime.transient.accepted.is_none());
    app.ott = Some(runtime);
}

#[test]
fn file_first_admission_rejects_cancelled_stale_or_withdrawn_capability() {
    let _serial = plx_base::testlock::serial();
    for invalidation in 0..3 {
        let (_files, mut app, mut source) = staged("play");
        let runtime = app.ott.as_mut().unwrap();
        let request = runtime.play_target.as_ref().unwrap().clone();
        assert!(transient::staged_native_binding(runtime, &request).unwrap().is_ok());
        match invalidation {
            0 => runtime.cancel.store(true, Ordering::Release),
            1 => runtime.generation += 1,
            _ => source.withdraw(),
        }
        assert!(transient::staged_native_binding(runtime, &request).unwrap().is_err());
        assert!(runtime.transient.accepted.is_none() && runtime.catalog.entries.is_empty());
    }
}
