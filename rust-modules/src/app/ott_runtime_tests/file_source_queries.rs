//! Sources responses may describe saved intent, but cannot publish unobserved
//! loss or mint a grant. Events use real temporary-file acquisitions; no worker runs.
use super::*;
use crate::app::ott_local_root::LocalSource;
use crate::app::ott_local_session::{AuthorityRef, BoundSource, LocalError, SourceKind};
use crate::app::ott_local_sources::{FolderDescriptor, LocalSourceKind, RememberedFolders};
use plx_base::task::OffFrame;
use std::path::PathBuf;

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        for n in 0..128 {
            let path = std::env::temp_dir().join(format!("ott-query-intent-{}-{n}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    let path = std::fs::canonicalize(path).unwrap();
                    std::fs::write(path.join("movie.mp4"), b"synthetic fixture").unwrap();
                    return Self(path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => panic!("test directory: {e}"),
            }
        }
        panic!("test directory slots exhausted")
    }
    fn descriptor(&self, generation: u64) -> BoundSource {
        BoundSource { id: "saved-file".into(), name: "Saved movie".into(), kind: SourceKind::File,
            available: false, authority_ref: AuthorityRef(1), generation,
            canonical_path: Some(self.0.join("movie.mp4").to_str().unwrap().into()) }
    }
    fn acquire(&self, generation: u64) -> LocalSource {
        LocalSource::acquire_authorized(&OffFrame::for_test(), self.descriptor(generation), &mut || true).unwrap()
    }
    fn remember_with_proof(&self, runtime: &mut Runtime) {
        let mut source = self.acquire(7);
        RememberedFolders::read(&runtime.saved.settings).unwrap().with_connected(FolderDescriptor {
            id: source.descriptor().id.clone(), name: source.descriptor().name.clone(),
            path: source.descriptor().canonical_path.clone().unwrap(), kind: LocalSourceKind::File, usb: None,
        }).unwrap().put_into(&mut runtime.saved.settings).unwrap();
        let profile = transient::source_profile(source.descriptor()).unwrap();
        let row = plx_data::ott::catalog_from_local_files(&[plx_data::ott::LocalFile {
            relative_path: "movie.mp4".into(), size: 0, duration_ms: None,
        }], &profile).unwrap().entries.remove(0);
        let profiles = ProviderProfiles::default();
        let bindings = selection::SelectionBindings::new(&profiles, [source.descriptor()], 7).unwrap();
        let owner = selection::SelectionOwner::LocalRoot(source.descriptor());
        let favorite = selection::capture_bound(&bindings, owner, &row).unwrap();
        runtime.saved.favorites.toggle(favorite).unwrap();
        runtime.saved.local_favorite_proofs.record_capture_with_pins(
            &bindings, &runtime.saved.favorites, &[], owner, std::slice::from_ref(&row), &row,
        ).unwrap();
        // Model a prior session's saved descriptor/proof, not a current grant.
        source.withdraw();
    }
}
impl Drop for Files { fn drop(&mut self) { plx_base::testscratch::remove_tree(&self.0); } }
fn setup(files: &Files) -> App {
    let mut app = app_fixture();
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.generation = 7;
    runtime.resume_source_work();
    // Hold actual shared slots; commands/events below cannot start IO.
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    files.remember_with_proof(runtime);
    app
}
fn begin(runtime: &mut Runtime, app: &mut App, id: u64, op: &str, path: Option<&str>) -> Invocation {
    let mut params = json!({"op":op,"runtime":runtime.remote.runtime()});
    if let Some(path) = path { params[if op == "sources" { "path" } else { "source" }] = json!(path); }
    let now = runtime.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{id:032x}"),"action":"local_media","params":params,"expires_at":1020
    }]}).to_string();
    let mut parsed = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(parsed.len(), 1);
    let invocation = parsed.remove(0);
    assert!(runtime.remote.invocation_is_current(&invocation));
    assert!(commands::command(runtime, app, &invocation).is_none());
    invocation
}
fn binding_request(runtime: &mut Runtime) -> ott_catalog::LocalBindingRequest {
    let Some(transient::Candidate::Local(candidate)) = runtime.transient.candidate.as_mut() else { panic!("Local candidate"); };
    let transient::CandidatePhase::Binding(request) = &mut candidate.phase else { panic!("binding phase"); };
    request.take().expect("worker request is still held")
}

#[test]
fn sources_then_play_or_shuffle_reacquires_saved_file_intent_without_publishing_query_loss() {
    let _serial = plx_base::testlock::serial();
    for op in ["play", "shuffle"] {
        let files = Files::new(); let mut app = setup(&files);
        let mut runtime = app.ott.take().unwrap();
        let before = serde_json::to_value(&runtime.saved).unwrap();
        let saved_path = files.descriptor(7).canonical_path.unwrap();
        begin(&mut runtime, &mut app, 1, "sources", Some(&saved_path));
        let query = binding_request(&mut runtime);
        assert!(!query.restore_remembered && !query.allow_new && !query.allow_reconnect);
        assert!(query.known.is_empty());
        let generation = runtime.generation;
        let intent = files.descriptor(generation);
        transient::binding_result(&mut runtime, &mut app, generation, Ok(ott_catalog::LocalBindings {
            sources: vec![LocalSource::unavailable(intent.clone())], lookup: Some(saved_path),
            matched: vec![intent], operation_error: None,
        }));
        assert!(runtime.transient.candidate.is_none());
        assert!(runtime.local_inventory.sources.is_empty(), "unacquired intent is response-only");
        assert!(runtime.local_inventory.proof_cleanup == transient::ProofCleanup::Clean);
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        let mut future_saved = runtime.saved.clone();
        transient::revoke_unavailable_proofs(&runtime, &mut future_saved);
        assert!(future_saved.local_favorite_proofs == runtime.saved.local_favorite_proofs);

        begin(&mut runtime, &mut app, 2, op, Some("saved-file"));
        let request = binding_request(&mut runtime);
        assert!(request.restore_remembered && request.allow_new);
        assert!(request.known.is_empty(), "query must not leave an unavailable placeholder blocking restore");
        assert_eq!(request.remembered.len(), 1);
        assert!(request.remembered[0].kind == LocalSourceKind::File);
        let generation = runtime.generation;
        let restored = files.acquire(generation);
        transient::binding_result(&mut runtime, &mut app, generation, Ok(ott_catalog::LocalBindings {
            sources: vec![restored.clone()], lookup: None, matched: vec![], operation_error: None,
        }));
        let Some(transient::Candidate::Local(candidate)) = runtime.transient.candidate.as_ref() else { panic!("play keeps a staged candidate"); };
        let transient::CandidatePhase::Scanning { source, profile, .. } = &candidate.phase else { panic!("saved File must advance to catalogue scan"); };
        assert!(source.kind == SourceKind::File && source.available);
        assert!(candidate.inventory[0].same_grant(&restored));
        assert!(matches!(runtime.local_catalog_task(profile.clone()), Ok(ott_catalog::Task::LocalFile { .. })));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before, "temporary playback cannot rewrite persistent state");
        app.ott = Some(runtime);
    }
}

#[test]
fn failed_or_cancelled_sources_still_applies_true_known_loss_without_installing_new_intent() {
    let _serial = plx_base::testlock::serial();
    for cancelled in [false, true] {
        let files = Files::new(); let mut app = setup(&files);
        let mut runtime = app.ott.take().unwrap();
        let known = files.acquire(7);
        let held = known.single_file("movie.mp4").unwrap();
        runtime.local_inventory.install(vec![known.clone()], 7);
        let invocation = begin(&mut runtime, &mut app, 3, "sources", None);
        let request = binding_request(&mut runtime);
        assert_eq!(request.known.len(), 1);
        std::fs::remove_file(files.0.join("movie.mp4")).unwrap();
        let loss = known.observe(&OffFrame::for_test(), &mut || true).unwrap();
        assert!(!loss.is_live() && held.is_live(), "worker observation has not withdrawn the owner's grant");
        let mut intent = files.descriptor(runtime.generation);
        intent.id = "never-acquired".into(); intent.name = "Other saved intent".into();
        intent.authority_ref = AuthorityRef(2);
        intent.canonical_path = Some(files.0.join("other.mp4").to_str().unwrap().into());
        if cancelled {
            runtime.remote.cancel_pairing();
            assert!(!runtime.remote.invocation_is_current(&invocation));
        }
        let generation = runtime.generation;
        transient::binding_result(&mut runtime, &mut app, generation, Ok(ott_catalog::LocalBindings {
            sources: vec![loss, LocalSource::unavailable(intent)], lookup: None, matched: vec![],
            operation_error: (!cancelled).then_some(LocalError::SourceUnavailable),
        }));
        assert!(!held.is_live(), "true known loss remains authoritative even if query cannot complete");
        assert_eq!(runtime.local_inventory.sources.len(), 1);
        assert_eq!(runtime.local_inventory.sources[0].descriptor().id, "saved-file");
        assert!(!runtime.local_inventory.sources[0].is_live());
        assert!(runtime.local_inventory.proof_cleanup == transient::ProofCleanup::Pending);
        assert!(runtime.transient.candidate.is_none());
        app.ott = Some(runtime);
    }
}
