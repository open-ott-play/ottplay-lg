//! Passive source queries must not retire admitted playback. The real remote
//! dispatcher, held resolver and native admission boundary are used. Local
//! observations open only this fixture's temporary directory; no worker,
//! decoder, network or protected writer is started.
use super::super::local_integration::{acquire_root, TempDir};
use super::*;
use crate::app::ott_local_root::LocalSource;
use crate::app::ott_local_session::{AuthorityRef, BoundSource, SourceKind};
use crate::app::ott_local_sources::{FolderDescriptor, LocalSourceKind, RememberedFolders};

fn local_source(f: &mut Fixture, temp: &TempDir) -> LocalSource {
    let descriptor = BoundSource {
        id: "query-root".into(),
        name: "Query root".into(),
        kind: SourceKind::Folder,
        available: true,
        authority_ref: AuthorityRef(17),
        generation: 7,
        canonical_path: Some(temp.folder("query-root")),
    };
    let source = acquire_root(descriptor.clone());
    let runtime = f.app.ott.as_mut().unwrap();
    RememberedFolders::read(&runtime.saved.settings)
        .unwrap()
        .with_connected(FolderDescriptor {
            id: descriptor.id.clone(),
            name: descriptor.name.clone(),
            path: descriptor.canonical_path.clone().unwrap(),
            kind: LocalSourceKind::Folder,
            usb: None,
        })
        .unwrap()
        .put_into(&mut runtime.saved.settings)
        .unwrap();
    runtime.local_inventory.install(vec![source.clone()], 7);
    // Hold catalogue/metadata slots; exact_play holds checkpoint IO only after
    // real admission. Binding uses its actual worker function and completion.
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    source
}

fn exact_play(f: &mut Fixture) -> ResolveRequest {
    assert!(
        !f.app.ott.as_ref().unwrap().saving,
        "real catalog admission must not be blocked by a fixture writer"
    );
    let categories = f.categories();
    let page = f.channels(categories["catalog"].as_str().unwrap(), "all", 0);
    assert!(f
        .request(
            "play_catalog",
            json!({
                "catalog":page["catalog"], "id":"video-0"
            })
        )
        .is_none());
    assert!(commands::pending_catalog_current(
        f.app.ott.as_ref().unwrap()
    ));
    let runtime = f.app.ott.as_mut().unwrap();
    let request = runtime.resolver_next.take().unwrap();
    // Only after actual admission may the fixture hold unrelated checkpoint IO.
    runtime.saving = true;
    request
}

fn sources(f: &mut Fixture) -> Option<Value> {
    f.request(
        "local_media",
        json!({"op":"sources", "runtime":f.runtime_id()}),
    )
}

fn held_observation(f: &mut Fixture) -> CatalogEvent {
    let runtime = f.app.ott.as_mut().unwrap();
    let Some(ott_catalog::Task::LocalBindings(request)) = transient::take_binding_task(runtime)
    else {
        panic!("the accepted query must use the existing binding task");
    };
    assert!(!request.allow_new && !request.restore_remembered && !request.allow_reconnect);
    let result = ott_catalog::validate_local_bindings_for_test(
        &plx_base::task::OffFrame::for_test(),
        request,
        &runtime.cancel,
    );
    CatalogEvent::LocalBindings(runtime.generation, result)
}
fn finish_observation(f: &mut Fixture, event: CatalogEvent) {
    f.app
        .ott
        .as_ref()
        .unwrap()
        .catalog_tx
        .send(event)
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut f.app);
    assert!(f.app.ott.as_ref().unwrap().transient.candidate.is_none());
}
fn finish_sources_observation(f: &mut Fixture) {
    let event = held_observation(f);
    finish_observation(f, event);
}

#[test]
fn passive_sources_defer_without_retiring_remote_play_and_retry_after_settlement() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let temp = TempDir::new();
    let mut f = Fixture::new(1);
    let source = local_source(&mut f, &temp);
    let request = exact_play(&mut f);
    let runtime = f.app.ott.as_ref().unwrap();
    let generation = runtime.generation;
    let play_generation = runtime.play_generation;
    let target = runtime.play_target.clone();
    let invocation = runtime.pending_play.as_ref().unwrap().id.clone();
    let cancel = runtime.cancel.clone();
    let ticket = plx_media::route::play_request_ticket();
    let status = runtime.status.clone();
    let reply = sources(&mut f).expect("passive query reports a finite busy result");
    assert_eq!(reply["data"]["error"], "operation_in_progress");
    let runtime = f.app.ott.as_ref().unwrap();
    assert_eq!(runtime.generation, generation);
    assert_eq!(runtime.play_generation, play_generation);
    assert!(runtime.play_target == target);
    assert_eq!(runtime.pending_play.as_ref().unwrap().id, invocation);
    assert!(Arc::ptr_eq(&runtime.cancel, &cancel));
    assert!(!cancel.load(Ordering::Acquire));
    assert!(plx_media::route::play_request_ticket() == ticket);
    assert_eq!(runtime.status, status);
    assert!(runtime.transient.candidate.is_none());
    assert!(commands::pending_catalog_current(runtime));
    assert!(source.is_live());
    pending_append_result(&mut f, request);
    assert_eq!(
        f.app.ott.as_ref().unwrap().status,
        "The native decoder cannot open this source"
    );
    assert_eq!(
        f.app
            .bridge
            .metadata_view()
            .now_playing()
            .map(|info| info.title.as_str()),
        Some("Video 0")
    );
    assert!(!plx_media::route::has_url(&f.app.player.session));
    assert!(f.app.ott.as_ref().unwrap().play_target.is_none());
    assert!(
        sources(&mut f).is_none(),
        "a fresh query is accepted after playback settles"
    );
    finish_sources_observation(&mut f);
    assert!(source.is_live());
    assert!(f.app.ott.as_ref().unwrap().local_inventory.sources[0].is_live());
}

#[test]
fn passive_sources_also_defer_for_ui_playback_without_remote_pending_receipt() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let temp = TempDir::new();
    let mut f = Fixture::new(1);
    let _source = local_source(&mut f, &temp);
    explicit_play(&mut f.app, "video-0");
    let runtime = f.app.ott.as_ref().unwrap();
    assert!(runtime.pending_play.is_none());
    assert!(runtime.resolver_next.is_some());
    let target = runtime.play_target.clone();
    assert!(target.is_some());
    let generation = runtime.generation;
    let cancel = runtime.cancel.clone();
    assert_eq!(
        sources(&mut f).unwrap()["data"]["error"],
        "operation_in_progress"
    );
    let runtime = f.app.ott.as_ref().unwrap();
    assert_eq!(runtime.generation, generation);
    assert!(runtime.play_target == target);
    assert!(runtime.resolver_next.is_some());
    assert!(Arc::ptr_eq(&runtime.cancel, &cancel));
    assert!(!cancel.load(Ordering::Acquire));
    assert!(runtime.transient.candidate.is_none());
}

#[test]
fn source_query_after_settlement_still_withdraws_actual_observed_root_loss() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let temp = TempDir::new();
    let mut f = Fixture::new(1);
    let source = local_source(&mut f, &temp);
    let profile = f.app.ott.as_ref().unwrap().effective_profile().cloned();
    let request = exact_play(&mut f);
    assert_eq!(
        sources(&mut f).unwrap()["data"]["error"],
        "operation_in_progress"
    );
    pending_append_result(&mut f, request);
    assert!(source.is_live());
    std::fs::remove_dir(source.descriptor().canonical_path.as_deref().unwrap()).unwrap();
    assert!(sources(&mut f).is_none());
    finish_sources_observation(&mut f);
    let runtime = f.app.ott.as_ref().unwrap();
    assert!(
        !source.is_live(),
        "real observation retires the borrowed grant too"
    );
    assert!(!runtime.local_inventory.sources[0].descriptor().available);
    assert!(!runtime.local_inventory.sources[0].is_live());
    assert!(runtime.effective_profile().cloned() == profile);
}

#[test]
fn deferring_sources_does_not_authorize_a_real_provider_change() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let temp = TempDir::new();
    let mut f = Fixture::new(1);
    let _source = local_source(&mut f, &temp);
    let request = exact_play(&mut f);
    assert_eq!(
        sources(&mut f).unwrap()["data"]["error"],
        "operation_in_progress"
    );
    f.app.ott.as_mut().unwrap().saved.profiles.profiles[0].source =
        "portal::[key:fixture]https://changed.invalid/nas/api".into();
    assert!(!commands::pending_catalog_current(
        f.app.ott.as_ref().unwrap()
    ));
    pending_append_result(&mut f, request);
    assert_eq!(
        f.app.ott.as_ref().unwrap().status,
        "Catalog changed before playback admission"
    );
    assert!(f.app.bridge.metadata_view().now_playing().is_none());
    assert!(!plx_media::route::has_url(&f.app.player.session));
}

#[test]
fn empty_sources_fast_path_stays_available_while_playback_is_pending() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = Fixture::new(1);
    let _request = exact_play(&mut f);
    let generation = f.app.ott.as_ref().unwrap().generation;
    let reply = sources(&mut f).unwrap();
    assert_eq!(reply["data"]["state"], "ready");
    assert_eq!(reply["data"]["sources"], json!([]));
    assert_eq!(f.app.ott.as_ref().unwrap().generation, generation);
    assert!(commands::pending_catalog_current(
        f.app.ott.as_ref().unwrap()
    ));
}

#[test]
fn sources_started_first_does_not_retire_newer_exact_catalog_play() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let temp = TempDir::new();
    let mut f = Fixture::new(1);
    let source = local_source(&mut f, &temp);
    assert!(sources(&mut f).is_none());
    let observed = held_observation(&mut f);
    let request = exact_play(&mut f);
    let runtime = f.app.ott.as_ref().unwrap();
    let generation = runtime.generation;
    let cancel = runtime.cancel.clone();
    let target = runtime.play_target.clone();
    let ticket = plx_media::route::play_request_ticket();
    assert!(commands::pending_catalog_current(runtime));
    finish_observation(&mut f, observed);
    let runtime = f.app.ott.as_ref().unwrap();
    assert_eq!(runtime.generation, generation);
    assert!(Arc::ptr_eq(&runtime.cancel, &cancel));
    assert!(!cancel.load(Ordering::Acquire));
    assert!(runtime.play_target == target);
    assert!(plx_media::route::play_request_ticket() == ticket);
    assert!(commands::pending_catalog_current(runtime));
    assert!(source.is_live());
    pending_append_result(&mut f, request);
    assert_eq!(
        f.app.ott.as_ref().unwrap().status,
        "The native decoder cannot open this source"
    );
    assert_eq!(
        f.app
            .bridge
            .metadata_view()
            .now_playing()
            .map(|info| info.title.as_str()),
        Some("Video 0")
    );
    assert!(!plx_media::route::has_url(&f.app.player.session));
}

#[test]
fn partial_sources_completion_requeues_its_producer_without_retiring_newer_ui_play() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let temp = TempDir::new();
    let mut f = Fixture::new(1);
    let _source = local_source(&mut f, &temp);
    let profile = f
        .app
        .ott
        .as_ref()
        .unwrap()
        .effective_profile()
        .cloned()
        .unwrap();
    {
        let runtime = f.app.ott.as_mut().unwrap();
        runtime.catalog_loading = true;
        runtime.catalog_complete = false;
    }
    assert!(sources(&mut f).is_none());
    let observed = held_observation(&mut f);
    // A UI choice can select an already visible row during a partial scan.
    // It does not require a new complete remote list receipt.
    explicit_play(&mut f.app, "video-0");
    let runtime = f.app.ott.as_mut().unwrap();
    assert!(runtime.pending_play.is_none());
    let request = runtime.resolver_next.take().unwrap();
    runtime.saving = true;
    let generation = runtime.generation;
    let cancel = runtime.cancel.clone();
    let target = runtime.play_target.clone();
    finish_observation(&mut f, observed);
    let runtime = f.app.ott.as_ref().unwrap();
    assert_eq!(runtime.generation, generation);
    assert!(Arc::ptr_eq(&runtime.cancel, &cancel));
    assert!(!cancel.load(Ordering::Acquire));
    assert!(runtime.play_target == target);
    assert!(runtime.catalog_loading && !runtime.catalog_complete);
    assert!(
        runtime.catalog_next.as_ref() == Some(&profile),
        "the existing catalogue slot must resume the interrupted producer"
    );
    assert!(
        !runtime.catalog_done.load(Ordering::Acquire),
        "restoring a plan cannot release the held worker slot"
    );
    pending_append_result(&mut f, request);
    assert_eq!(
        f.app.ott.as_ref().unwrap().status,
        "The native decoder cannot open this source"
    );
    assert_eq!(
        f.app
            .bridge
            .metadata_view()
            .now_playing()
            .map(|info| info.title.as_str()),
        Some("Video 0")
    );
    assert!(f.app.ott.as_ref().unwrap().catalog_next.as_ref() == Some(&profile));
}

#[test]
fn stale_sources_completion_cannot_clear_a_new_provider_catalogue_plan() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let temp = TempDir::new();
    let mut f = Fixture::new(1);
    let source = local_source(&mut f, &temp);
    {
        let runtime = f.app.ott.as_mut().unwrap();
        runtime.catalog_loading = true;
        runtime.catalog_complete = false;
    }
    assert!(sources(&mut f).is_none());
    let request_id = format!("{:032x}", f.serial);
    let observed = held_observation(&mut f);
    let mut replacement = synthetic_profile();
    replacement.id = "replacement-m3u".into();
    replacement.kind = ProviderKind::M3u;
    replacement.source = "https://media.invalid/new.m3u".into();
    let runtime = f.app.ott.as_mut().unwrap();
    // Model the already committed source selection, then use its real catalog
    // activation. The actual worker remains occupied by the held old query.
    runtime.saved.profiles.profiles = vec![replacement.clone()];
    runtime.saved.profiles.active_profile = Some(replacement.id.clone());
    runtime.load_catalog();
    runtime.saving = true;
    let generation = runtime.generation;
    let cancel = runtime.cancel.clone();
    assert!(runtime.catalog_next.as_ref() == Some(&replacement));
    assert!(runtime.catalog_loading);
    runtime.status = "Loading the replacement provider…".into();
    finish_observation(&mut f, observed);
    let runtime = f.app.ott.as_ref().unwrap();
    assert_eq!(runtime.status, "Loading the replacement provider…");
    assert_eq!(runtime.generation, generation);
    assert!(Arc::ptr_eq(&runtime.cancel, &cancel));
    assert!(!cancel.load(Ordering::Acquire));
    assert!(runtime.catalog_next.as_ref() == Some(&replacement));
    assert!(runtime.catalog_loading && !runtime.catalog_complete);
    assert!(runtime.effective_profile() == Some(&replacement));
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
    assert!(
        source.is_live(),
        "stale query rows cannot replace current authority"
    );
    let response = runtime
        .remote
        .response_for_id_for_test(&request_id)
        .unwrap();
    assert_eq!(response["data"]["error"], "runtime_changed");
}

// The result is computed by the real binding worker function but held until
// after standby. The occupied catalogue flag models its unretired task; these
// tests never release that slot through the threaded runtime launcher.
fn sources_survive_standby_as_scan_plan(f: &mut Fixture, source: LocalSource) {
    f.app.running = true;
    f.app.ott_standby = false;
    let runtime = f.app.ott.as_mut().unwrap();
    runtime.resume_source_work();
    runtime.catalog_loading = true;
    runtime.catalog_complete = false;
    runtime.catalog_next = None;
    let profile = runtime.effective_profile().cloned().unwrap();
    let accepted = runtime
        .transient
        .accepted
        .as_ref()
        .map(transient::TransientMedia::owner);
    let rows: Vec<_> = runtime
        .catalog
        .entries
        .iter()
        .map(|row| row.id.clone())
        .collect();
    assert!(sources(f).is_none());
    let request_id = format!("{:032x}", f.serial);
    let observed = held_observation(f);
    let runtime = f.app.ott.as_mut().unwrap();
    runtime.saving = true; // Hold unrelated checkpoint IO after query admission.
    assert!(!runtime.catalog_loading && !runtime.catalog_complete);
    assert!(runtime.catalog_next.is_none());
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
    let generation = runtime.generation;
    let cancel = runtime.cancel.clone();

    f.app.ott_standby = true;
    let mut runtime = f.app.ott.take().unwrap();
    runtime.sync_source_lifecycle(&mut f.app);
    assert!(!runtime.source_work_allowed());
    assert_eq!(runtime.generation, generation.wrapping_add(1).max(1));
    let suspended_generation = runtime.generation;
    assert!(Arc::ptr_eq(&runtime.cancel, &cancel));
    assert!(cancel.load(Ordering::Acquire));
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
    assert!(runtime.transient.candidate.is_none());
    assert!(runtime.catalog_loading && !runtime.catalog_complete);
    assert!(runtime.catalog_next.as_ref() == Some(&profile));
    assert!(
        runtime
            .transient
            .accepted
            .as_ref()
            .map(transient::TransientMedia::owner)
            == accepted
    );
    assert!(source.is_live());
    assert_eq!(
        runtime
            .remote
            .response_for_id_for_test(&request_id)
            .unwrap()["data"]["error"],
        "runtime_changed"
    );
    f.app.ott = Some(runtime);

    f.app.ott_standby = false;
    let mut runtime = f.app.ott.take().unwrap();
    runtime.sync_source_lifecycle(&mut f.app);
    runtime.rearm_source_work(&mut f.app);
    assert!(runtime.source_work_allowed());
    assert!(
        !runtime.catalog_work_allowed(),
        "the old task still owns its slot"
    );
    assert!(Arc::ptr_eq(&runtime.cancel, &cancel));
    assert!(cancel.load(Ordering::Acquire));
    assert_eq!(runtime.generation, suspended_generation);
    assert!(runtime.catalog_next.as_ref() == Some(&profile));
    f.app.ott = Some(runtime);

    // An old query completion is stale even after foreground returns. It cannot
    // retire the plan, publish old authority, uncancel or release its own slot.
    finish_observation(f, observed);
    let mut runtime = f.app.ott.take().unwrap();
    assert_eq!(runtime.generation, suspended_generation);
    assert!(Arc::ptr_eq(&runtime.cancel, &cancel));
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
    assert!(runtime.catalog_loading && !runtime.catalog_complete);
    assert!(runtime.catalog_next.as_ref() == Some(&profile));
    assert_eq!(
        runtime
            .catalog
            .entries
            .iter()
            .map(|row| row.id.clone())
            .collect::<Vec<_>>(),
        rows
    );
    assert!(source.is_live());

    // Only actual task retirement (modeled here by its existing done flag)
    // allows the normal shared lifecycle hook to grant a fresh binding task.
    runtime.catalog_done.store(true, Ordering::Release);
    runtime.rearm_source_work(&mut f.app);
    assert!(runtime.catalog_work_allowed());
    assert!(!Arc::ptr_eq(&runtime.cancel, &cancel));
    assert!(!runtime.cancel.load(Ordering::Acquire));
    assert!(cancel.load(Ordering::Acquire));
    assert_eq!(runtime.generation, suspended_generation);
    assert!(runtime.catalog_next.as_ref() == Some(&profile));
    let Some(ott_catalog::Task::LocalBindings(binding)) =
        transient::take_binding_task(&mut runtime)
    else {
        panic!("foreground must revalidate existing roots before resuming its scan");
    };
    assert!(binding.restore_remembered && !binding.allow_new && !binding.allow_reconnect);
    assert!(binding.lookup.is_none());
    assert!(binding.known.iter().any(|known| known.same_grant(&source)));
    assert!(
        runtime.transient.candidate.is_none(),
        "the old Sources command is never replayed"
    );
    assert!(runtime.effective_profile() == Some(&profile));
    f.app.ott = Some(runtime);
}

#[test]
fn partial_provider_scan_survives_sources_standby_and_waits_for_worker_retirement() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let temp = TempDir::new();
    let mut f = Fixture::new(1);
    let source = local_source(&mut f, &temp);
    assert!(f.app.ott.as_ref().unwrap().transient.accepted.is_none());
    sources_survive_standby_as_scan_plan(&mut f, source);
}

#[test]
fn accepted_partial_local_scan_survives_sources_standby_with_its_same_owner() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let (_temp, app, _root) = super::super::local_integration::setup();
    let mut f = Fixture { app, serial: 0 };
    let source = f.app.ott.as_ref().unwrap().local_inventory.sources[0].clone();
    assert!(matches!(
        f.app.ott.as_ref().unwrap().transient.accepted,
        Some(transient::TransientMedia::Local(_))
    ));
    let permanent = f.app.ott.as_ref().unwrap().saved.profiles.clone();
    sources_survive_standby_as_scan_plan(&mut f, source);
    assert!(f.app.ott.as_ref().unwrap().saved.profiles == permanent);
}

#[test]
fn stale_sources_standby_preserves_a_replacement_provider_plan() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let temp = TempDir::new();
    let mut f = Fixture::new(1);
    let source = local_source(&mut f, &temp);
    let runtime = f.app.ott.as_mut().unwrap();
    runtime.resume_source_work();
    runtime.catalog_loading = true;
    runtime.catalog_complete = false;
    assert!(sources(&mut f).is_none());
    let _old_observation = held_observation(&mut f);
    let mut replacement = synthetic_profile();
    replacement.id = "replacement-after-query".into();
    replacement.kind = ProviderKind::M3u;
    replacement.source = "https://media.invalid/replacement.m3u".into();
    let runtime = f.app.ott.as_mut().unwrap();
    runtime.saved.profiles.profiles = vec![replacement.clone()];
    runtime.saved.profiles.active_profile = Some(replacement.id.clone());
    runtime.load_catalog();
    runtime.saving = true;
    let replacement_generation = runtime.generation;
    let replacement_cancel = runtime.cancel.clone();
    assert!(runtime.catalog_next.as_ref() == Some(&replacement));
    f.app.ott_standby = true;
    let mut runtime = f.app.ott.take().unwrap();
    runtime.sync_source_lifecycle(&mut f.app);
    assert_eq!(
        runtime.generation,
        replacement_generation.wrapping_add(1).max(1)
    );
    assert!(Arc::ptr_eq(&runtime.cancel, &replacement_cancel));
    assert!(replacement_cancel.load(Ordering::Acquire));
    assert!(runtime.catalog_next.as_ref() == Some(&replacement));
    assert!(runtime.catalog_loading && !runtime.catalog_complete);
    assert!(runtime.effective_profile() == Some(&replacement));
    assert!(runtime.transient.candidate.is_none());
    assert!(!runtime.catalog_done.load(Ordering::Acquire));
    assert!(source.is_live());
    f.app.ott = Some(runtime);
}


#[test]
fn passive_sources_preserve_visible_status_at_admission_and_completion() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for status in ["", "Current provider loading…", "Current provider failed"] {
        let temp = TempDir::new();
        let mut f = Fixture::new(0);
        let source = local_source(&mut f, &temp);
        let runtime = f.app.ott.as_mut().unwrap();
        // The active M3U has no rows; remembered local folders still force the
        // asynchronous Sources path used by the post-Home status query.
        runtime.saved.profiles.profiles[0].kind = ProviderKind::M3u;
        runtime.saved.profiles.profiles[0].source = "https://media.invalid/empty.m3u".into();
        runtime.provider = ProviderKind::M3u;
        runtime.status = status.into();
        let profiles = runtime.saved.profiles.clone();
        assert!(runtime.catalog.entries.is_empty());
        assert!(f.request("local_media", json!({
            "op":"sources", "runtime":f.runtime_id(), "include_paths":true
        })).is_none());
        assert_eq!(f.app.ott.as_ref().unwrap().status, status);
        finish_sources_observation(&mut f);
        let runtime = f.app.ott.as_ref().unwrap();
        assert_eq!(runtime.status, status);
        assert!(runtime.saved.profiles == profiles);
        assert!(runtime.catalog.entries.is_empty());
        assert!(source.is_live());
        let id = format!("{:032x}", f.serial);
        let response = runtime.remote.response_for_id_for_test(&id).unwrap();
        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["sources"].as_array().unwrap().len(), 1);
    }
}

#[test]
fn passive_sources_completion_does_not_replace_a_newer_status_on_success_or_failure() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for failed in [false, true] {
        let temp = TempDir::new();
        let mut f = Fixture::new(0);
        let source = local_source(&mut f, &temp);
        f.app.ott.as_mut().unwrap().status = "Current provider loading…".into();
        assert!(sources(&mut f).is_none());
        assert_eq!(f.app.ott.as_ref().unwrap().status, "Current provider loading…");
        let observed = held_observation(&mut f);
        let observed = if failed {
            CatalogEvent::LocalBindings(
                f.app.ott.as_ref().unwrap().generation,
                Err(crate::app::ott_local_session::LocalError::NativeError),
            )
        } else { observed };
        f.app.ott.as_mut().unwrap().status = "Newer provider status".into();
        finish_observation(&mut f, observed);
        let runtime = f.app.ott.as_ref().unwrap();
        assert_eq!(runtime.status, "Newer provider status");
        assert!(source.is_live());
        let id = format!("{:032x}", f.serial);
        let response = runtime.remote.response_for_id_for_test(&id).unwrap();
        if failed {
            assert_eq!(response["data"]["error"], "native_error");
        } else {
            assert_eq!(response["status"], "ok");
        }
    }
}

#[test]
fn explicit_local_operations_still_publish_opening_status() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for op in ["connect", "play", "shuffle"] {
        let temp = TempDir::new();
        let mut f = Fixture::new(0);
        let _source = local_source(&mut f, &temp);
        f.app.ott.as_mut().unwrap().status = "Previous provider status".into();
        assert!(f.request("local_media", json!({
            "op":op, "runtime":f.runtime_id(), "source":"Query root"
        })).is_none());
        let runtime = f.app.ott.as_mut().unwrap();
        assert_eq!(runtime.status, "Opening local source…");
        let Some(ott_catalog::Task::LocalBindings(request)) =
            transient::take_binding_task(runtime)
        else { panic!("explicit local operations retain their real binding task") };
        assert!(request.allow_new && request.restore_remembered);
        assert_eq!(request.allow_reconnect, op == "connect");
    }
}
