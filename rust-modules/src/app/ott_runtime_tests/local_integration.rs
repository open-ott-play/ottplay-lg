//! Real Runtime checkpoint, event and command boundaries with synthetic admissions.
//! Real temporary root descriptors are opened; no filesystem worker, media decoder,
//! protected write or external server runs.
use super::*;
use crate::app::ott_local_root::LocalSource;
use crate::app::ott_local_session as local;

#[test]
fn folder_kiosk_status_uses_the_same_private_safe_item_id() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, root) = setup();
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.entropy = Some([3; 16]);
        runtime.saving = false;
        runtime.saved.settings.insert("nativeLocalSources".into(), json!({
            "version":1,"folders":[{"id":root.id,"name":root.name,"path":root.canonical_path}]
        }));
    }
    install(&mut app, Some(&root), "first");
    let mut runtime = app.ott.take().unwrap();
    let observed = kiosk_source::observe(&runtime, &app).unwrap().unwrap();
    let raw_identity = observed.favorite.identity();
    let fingerprint = selection::local_root_fingerprint(&root).unwrap();
    let policy = crate::app::ott_kiosk_stored::Stored::local_folder(
        observed.favorite.origin.clone(), true, root.id.clone(), fingerprint,
    ).unwrap();
    runtime.saved.settings.insert("nativeKiosk".into(), policy.encode().unwrap());
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let invocation = remote_invocation(&mut runtime, 1, "kiosk", json!({"mode":"status"}));
    let response = commands::command(&mut runtime, &mut app, &invocation).unwrap();
    assert_eq!(response["status"], "ok");
    let id = response["data"]["channel"]["id"].as_str().unwrap();
    assert_ne!(id, raw_identity, "folder status ID must not export its private Favorites identity");
    assert!(serde_json::from_str::<Value>(id).is_err());
    for private in ["relativePath", "rootAlias", "sourceAlias", "first.mp4",
        root.canonical_path.as_deref().unwrap()] {
        assert!(!response.to_string().contains(private), "private metadata: {private}");
    }
    assert_eq!(response["data"]["channel"]["name"], "first");
    assert_eq!(response["data"]["folder"]["name"], "Movies");
    assert_eq!(response["data"]["folder"]["available"], true);
    let item = crate::app::ott_kiosk_stored::Stored::item(observed.favorite, true).unwrap();
    let item_status = kiosk_source::status_target(&runtime, &app, &item).unwrap();
    assert_eq!(item_status["channel"]["id"], id);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(!runtime.saving);
    assert!(runtime.rx.try_recv().is_err());
    app.ott = Some(runtime);
}

/// Scoped real roots. The guard is returned before App and held until App drops.
/// Atomic directory creation supplies uniqueness without a registry or counter.
pub(in crate::app::ott_runtime) struct TempDir(std::path::PathBuf);
impl TempDir {
    pub(super) fn new() -> Self {
        for suffix in 0..64 {
            let path = std::env::temp_dir().join(format!(
                "ott-runtime-local-root-{}-{suffix}",
                std::process::id()
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(std::fs::canonicalize(path).unwrap()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create temporary root: {error}"),
            }
        }
        panic!("temporary root fixture slots exhausted")
    }
    pub(super) fn folder(&self, name: &str) -> String {
        let path = self.0.join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::canonicalize(path)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        plx_base::testscratch::remove_tree(&self.0);
    }
}
pub(super) fn acquire_root(descriptor: local::BoundSource) -> LocalSource {
    let source = LocalSource::acquire_authorized(
        &plx_base::task::OffFrame::for_test(),
        descriptor,
        &mut || true,
    )
    .expect("real temporary root is readable");
    assert!(source.root_handle().is_some_and(|handle| handle.is_live()));
    source
}

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}
/// Derive the command deadline at the real App clock through the transport
/// parser, without starting transport or altering the production expiry guard.
fn remote_invocation(
    runtime: &mut Runtime, request_id: u64, action: &str, params: serde_json::Value,
) -> Invocation {
    let now = runtime.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{request_id:032x}"),"action":action,
        "params":params,"expires_at":1020
    }]}).to_string();
    let mut admitted = runtime.remote.raw_poll_for_test(&body, now);
    assert_eq!(admitted.len(), 1, "one parsed {action} request with a fresh ID");
    let invocation = admitted.pop().unwrap();
    assert!(runtime.remote.invocation_is_current(&invocation));
    assert_eq!(invocation.deadline_ms, now.saturating_add(20_000));
    invocation
}

fn local_invocation(runtime: &mut Runtime, request_id: u64, params: serde_json::Value) -> Invocation {
    remote_invocation(runtime, request_id, "local_media", params)
}

fn permanent() -> ProviderProfile {
    ProviderProfile {
        id: "permanent".into(),
        name: "Permanent".into(),
        kind: ProviderKind::M3u,
        source: "https://media.invalid/list.m3u".into(),
        source_alias: "Permanent".into(),
        enabled: true,
    }
}
fn item(id: &str) -> CatalogEntry {
    let mut entry = synthetic_entry();
    entry.id = id.into();
    entry.title = id.into();
    entry.category = "Movies".into();
    entry.media = MediaLocator::Local {
        relative_path: format!("{id}.mp4"),
    };
    entry
}
pub(in crate::app::ott_runtime) fn setup() -> (TempDir, Box<App>, local::BoundSource) {
    let root_dir = TempDir::new();
    let mut app = Box::new(app_fixture());
    let root = local::BoundSource {
        id: "root".into(),
        name: "Movies".into(),
        kind: local::SourceKind::Folder,
        available: true,
        authority_ref: local::AuthorityRef(1),
        generation: 7,
        canonical_path: Some(root_dir.folder("Movies")),
    };
    let runtime = app.ott.as_mut().unwrap();
    runtime.loaded = true;
    runtime.generation = 7;
    runtime.provider = ProviderKind::M3u;
    runtime.saved.profiles.profiles = vec![permanent()];
    runtime.saved.profiles.active_profile = Some("permanent".into());
    runtime
        .local_inventory
        .install(vec![acquire_root(root.clone())], 7);
    runtime.catalog.entries = vec![item("first")];
    runtime.catalog_loading = true;
    runtime.catalog_complete = false;
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.saving = true;
    let request=local::LocalRequest::parse(&json!({"op":"play","runtime":runtime.remote.runtime(),"source":"root","session":true,"progressive":true})).unwrap();
    let mut pending = local::PendingLocal::new(request, local::OwnerToken(7), u64::MAX);
    let rows = [local::LocalItem {
        id: "first".into(),
        playable: true,
    }];
    let scan = local::CatalogSnapshot {
        source_id: "root",
        authority_ref: root.authority_ref,
        source_generation: 7,
        state: local::ScanState::Scanning,
        items: &rows,
    };
    let context = local::AdmissionContext {
        runtime: runtime.remote.runtime(),
        owner: local::OwnerToken(7),
        invocation_current: true,
        now_ms: runtime.now_ms(),
        mutations_allowed: true,
    };
    let local::Admission::Propose(mut selected) = pending.evaluate(&context, &root, &scan, 1)
    else {
        panic!("proposal")
    };
    selected
        .note_intent(local::OwnerToken(7), local::LocalIntent::Selected("first"))
        .unwrap();
    let binding = runtime.bind_local_selection(&root).unwrap();
    let read = runtime
        .capture_local_source(&binding)
        .expect("installed live source");
    runtime.transient.accepted = Some(transient::TransientMedia::Local(transient::LocalSession {
        read,
        context: selected,
        profile: transient::source_profile(&root).unwrap(),
        borrowed: None, kiosk_resume: None,
    }));
    runtime.continuity = continuity::Continuity::new(7, None);
    runtime.continuity.user_selected(7, "first");
    (root_dir, app, root)
}
pub(in crate::app::ott_runtime) fn install(app: &mut App, root: Option<&local::BoundSource>, selection: &str) {
    // Installing an accepted source retains a mounted Player and its origin.
    // Exit's delayed PopTo is not part of this fixture transition.
    playback::replace_player_in_place(
        &mut app.player,
        &mut app.adapters.player,
        &mut app.repause_at,
        &mut app.pages,
    );
    plx_media::route::reset_player_control_for_test(&app.player.session);
    let source = if let Some(root) = root {
        plx_media::ff::ExternalSource::Local(
            std::path::Path::new(root.canonical_path.as_deref().unwrap()).join("first.mp4"),
        )
    } else {
        plx_media::ff::ExternalSource::Http(
            plx_data::ott::http_url("https://media.invalid/live").unwrap(),
        )
    };
    let mut info = unsupported_info();
    info.video_codec = "h264".into();
    info.audio_codec = "aac".into();
    info.audio_channels = 2;
    info.audio_sample_rate = 48000;
    info.audio_bit_depth = 16;
    plx_media::route::request_external_source(
        &mut app.player.session,
        &info,
        source.clone(),
        selection,
        "Movies",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    assert!(plx_media::route::playback_identity(&app.player.session).is_some());
    assert_eq!(plx_media::route::external_source(&app.player.session), Some(source.clone()));
    let runtime = app.ott.as_mut().unwrap();
    let (profile, entry, binding) = if let Some(root) = root {
        (
            transient::source_profile(root).unwrap(),
            item("first"),
            Some(runtime.bind_local_selection(root).unwrap()),
        )
    } else {
        let mut entry = item("live");
        entry.media = MediaLocator::Http {
            url: "https://media.invalid/live".into(),
        };
        (permanent(), entry, None)
    };
    runtime.playing = Some(Playing { channel_capture: None,
        kiosk_selection: None,
        archive: None,
        identity: plx_media::route::playback_identity(&app.player.session),
        profile,
        entry,
        selection_id: selection.into(),
        source: Some(source),
        advance_on_failure: false,
        requires_server_lease: false,
        transient_owner: Some(local::OwnerToken(7)),
        local_selection: binding,
    });
    runtime.selected = Some(selection.into());
    runtime
        .continuity
        .user_selected(runtime.generation, selection);
    let installed = runtime.playing.clone().unwrap();
    assert!(playing_matches(&installed, app));
}
fn checkpoint(app: &mut App, position_ms: u64) {
    plx_media::player::SHARED
        .playpos_ns
        .store((position_ms * 1_000_000) as i64, Ordering::Release);
    let mut runtime = app.ott.take().unwrap();
    runtime.checkpoint(app, true);
    app.ott = Some(runtime);
}
fn borrowed_position(app: &App) -> Option<u64> {
    let transient::TransientMedia::Local(session) = app
        .ott
        .as_ref()
        .unwrap()
        .transient
        .accepted
        .as_ref()
        .unwrap()
    else { panic!("expected Local session") };
    session.borrowed.as_ref().map(|b| b.position_ms)
}

#[test]
fn temporary_folder_mixed_favorite_and_return_keep_all_positions_out_of_durable_state() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
    install(&mut app, Some(&root), "first");
    checkpoint(&mut app, 12500);
    assert_eq!(borrowed_position(&app), None);
    transient::set_view(app.ott.as_mut().unwrap(), "", true);
    install(&mut app, None, "favorite:live");
    checkpoint(&mut app, 32000);
    assert_eq!(borrowed_position(&app), Some(32000));
    transient::set_view(app.ott.as_mut().unwrap(), "", false);
    install(&mut app, Some(&root), "first");
    checkpoint(&mut app, 20000);
    assert_eq!(borrowed_position(&app), None);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.checkpoints.is_empty());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    let transient::TransientMedia::Local(session) = runtime.transient.accepted.as_ref().unwrap()
    else { panic!("expected Local session") };
    assert_eq!(session.context.position_ms(), 20000);
}

#[test]
fn late_local_batch_cannot_replace_borrowed_favorite_or_restore_the_folder() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, _root) = setup();
    let favorite = Favorite {
        title: "Live".into(),
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
    let selection = format!("favorite:{}", favorite.identity());
    app.ott
        .as_mut()
        .unwrap()
        .saved
        .favorites
        .toggle(favorite)
        .unwrap();
    transient::set_view(app.ott.as_mut().unwrap(), "", true);
    install(&mut app, None, &selection);
    checkpoint(&mut app, 44000);
    let old_identity = plx_media::route::playback_identity(&app.player.session);
    app.ott
        .as_ref()
        .unwrap()
        .catalog_tx
        .send(CatalogEvent::Batch(
            7,
            Catalog { hierarchy: None,
                entries: vec![item("later")],
            },
        ))
        .unwrap_or_else(|_| panic!("receiver"));
    runtime_tick(&mut app);
    assert_eq!(
        plx_media::route::playback_identity(&app.player.session),
        old_identity
    );
    assert_eq!(
        app.ott.as_ref().unwrap().selected.as_deref(),
        Some(selection.as_str())
    );
    assert!(app.ott.as_ref().unwrap().resolver_next.is_none());
    assert_eq!(borrowed_position(&app), Some(44000));
    assert!(app.ott.as_ref().unwrap().checkpoints.is_empty());
}

#[test]
fn unrelated_verified_save_and_real_exit_preserve_permanent_profile_and_no_temporary_resume() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    install(&mut app, Some(&root), "first");
    checkpoint(&mut app, 19000);
    let mut saved = app.ott.as_ref().unwrap().saved.clone();
    saved.settings.insert("test-setting".into(), json!(1));
    let expected = serde_json::to_value(&saved).unwrap();
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Saved(
            Ok(saved),
            None,
            false,
            ok(json!({"persisted":true})),
        ))
        .unwrap_or_else(|_| panic!("receiver"));
    runtime_tick(&mut app);
    assert!(app.ott.as_ref().unwrap().transient.accepted.is_some());
    mount_player(&mut app);
    exit_through_content(&mut app);
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), expected);
    assert_eq!(runtime.active().unwrap().id, "permanent");
    assert!(runtime.saved.resume.is_empty() && runtime.checkpoints.is_empty());
}

#[test]
fn failed_local_candidate_restarts_interrupted_current_scan_in_the_same_slot() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    install(&mut app, Some(&root), "first");
    let old_identity = plx_media::route::playback_identity(&app.player.session);
    let runtime = app.ott.as_mut().unwrap();
    let token = runtime.remote.runtime().to_owned();
    let inv = local_invocation(
        runtime, 1,
        json!({"op":"play","runtime":token,"source":_root_dir.folder("Another"),"progressive":true}),
    );
    let mut runtime = app.ott.take().unwrap();
    assert!(commands::command(&mut runtime, &mut app, &inv).is_none());
    let generation = runtime.generation;
    app.ott = Some(runtime);
    app.ott
        .as_ref()
        .unwrap()
        .catalog_tx
        .send(CatalogEvent::LocalBindings(
            generation,
            Err(local::LocalError::SourceUnavailable),
        ))
        .unwrap_or_else(|_| panic!("receiver"));
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.transient.candidate.is_none());
    assert!(runtime.catalog_loading && !runtime.catalog_complete);
    assert!(runtime.catalog_next.is_some());
    assert_eq!(runtime.catalog.entries[0].id, "first");
    assert_eq!(
        plx_media::route::playback_identity(&app.player.session),
        old_identity
    );
    assert_eq!(runtime.active().unwrap().id, "permanent");
}

fn begin_candidate(app: &mut App, root_dir: &TempDir) -> Invocation {
    let mut runtime = app.ott.take().unwrap();
    let token = runtime.remote.runtime().to_owned();
    let inv = local_invocation(
        &mut runtime, 1,
        json!({"op":"play","runtime":token,"source":root_dir.folder("Another"),"progressive":true}),
    );
    assert!(commands::command(&mut runtime, app, &inv).is_none());
    assert!(runtime.transient.candidate.is_some());
    app.ott = Some(runtime);
    inv
}

#[test]
fn old_eof_is_held_while_an_explicit_local_candidate_owns_admission() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    install(&mut app, Some(&root), "first");
    let identity = plx_media::route::playback_identity(&app.player.session);
    let invocation = begin_candidate(&mut app, &_root_dir);
    assert!(finish_catalog_playback(&mut app));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(
        match runtime.transient.candidate.as_ref().unwrap() {
            transient::Candidate::Local(candidate) => match &candidate.purpose {
                transient::LocalPurpose::Remote(invocation) => &invocation.id,
                _ => panic!("expected explicit Local invocation"),
            },
            _ => panic!("expected Local candidate"),
        },
        &invocation.id
    );
    assert!(runtime.resolver_next.is_none());
    assert_eq!(
        plx_media::route::playback_identity(&app.player.session),
        identity
    );
    assert!(runtime.checkpoints.is_empty());
}

#[test]
fn invalid_manual_selection_keeps_candidate_but_valid_selection_retires_closed_leased_owner() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    install(&mut app, Some(&root), "first");
    // Synthetic post-handoff state: the sole server lease has been consumed by
    // the worker, while the old decoder still retains its logical identity.
    app.ott
        .as_mut()
        .unwrap()
        .playing
        .as_mut()
        .unwrap()
        .requires_server_lease = true;
    let inv = begin_candidate(&mut app, &_root_dir);
    let identity = plx_media::route::playback_identity(&app.player.session);
    let mut runtime = app.ott.take().unwrap();
    assert!(runtime.play(&mut app, "missing").is_err());
    assert_eq!(
        match runtime.transient.candidate.as_ref().unwrap() {
            transient::Candidate::Local(candidate) => match &candidate.purpose {
                transient::LocalPurpose::Remote(invocation) => &invocation.id,
                _ => panic!("expected explicit Local invocation"),
            },
            _ => panic!("expected Local candidate"),
        },
        &inv.id
    );
    assert_eq!(
        plx_media::route::playback_identity(&app.player.session),
        identity
    );
    runtime.play(&mut app, "first").unwrap();
    assert!(runtime.transient.candidate.is_none());
    assert!(
        runtime.playing.is_none(),
        "closed old lease cannot leave a false playing owner"
    );
    assert!(!plx_media::route::has_url(&app.player.session));
    assert_eq!(
        runtime.play_target.as_ref().map(|r| r.id.as_str()),
        Some("first")
    );
    assert!(runtime.resolver_next.is_some());
    assert_eq!(runtime.active().unwrap().id, "permanent");
    app.ott = Some(runtime);
}

#[test]
fn exact_local_candidate_resolver_failure_does_not_start_the_successor() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    install(&mut app, Some(&root), "first");
    let identity = plx_media::route::playback_identity(&app.player.session);
    begin_candidate(&mut app, &_root_dir);
    let mut runtime = app.ott.take().unwrap();
    let generation = runtime.generation;
    let mut next = root;
    next.id = "next-root".into();
    next.authority_ref = local::AuthorityRef(2);
    next.canonical_path = Some(_root_dir.folder("Another"));
    next.generation = generation;
    let validated = ott_catalog::LocalBindings {
        operation_error: None,
        sources: vec![runtime.local_inventory.sources[0].clone(), acquire_root(next.clone())],
        lookup: next.canonical_path.clone(),
        matched: vec![next],
    };
    transient::binding_result(&mut runtime, &mut app, generation, Ok(validated));
    runtime
        .catalog_tx
        .send(CatalogEvent::Batch(
            generation,
            Catalog { hierarchy: None,
                entries: vec![item("candidate"), item("successor")],
            },
        ))
        .unwrap_or_else(|_| panic!("receiver"));
    app.ott = Some(runtime);
    runtime_tick(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    let request = runtime
        .resolver_next
        .take()
        .expect("first discovered item queues before scan end");
    assert_eq!(
        runtime.play_target.as_ref().map(|r| r.id.as_str()),
        Some("candidate")
    );
    assert!(!runtime.play_target.as_ref().unwrap().advance_on_failure);
    runtime
        .tx
        .send(Job::Play(
            request.generation,
            request.profile,
            Err("synthetic resolve refusal"),
        ))
        .unwrap_or_else(|_| panic!("receiver"));
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.transient.candidate.is_none());
    assert!(
        runtime.resolver_next.is_none(),
        "explicit candidate failure must not select successor"
    );
    assert_eq!(
        plx_media::route::playback_identity(&app.player.session),
        identity
    );
    assert_eq!(runtime.playing.as_ref().unwrap().selection_id, "first");
    assert_eq!(runtime.catalog.entries[0].id, "first");
    assert!(runtime.checkpoints.is_empty());
}

#[test]
fn connect_binding_is_published_only_by_verified_saved_completion() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for saved in [false, true] {
        let (_root_dir, mut app, mut root) = setup();
        let mut runtime = app.ott.take().unwrap();
        let original = serde_json::to_value(&runtime.saved).unwrap();
        root.id = "new-root".into();
        root.name = "New movies".into();
        root.authority_ref = local::AuthorityRef(2);
        root.canonical_path = Some(_root_dir.folder("New"));
        let token = runtime.remote.runtime().to_owned();
        let invocation = local_invocation(
            &mut runtime, 1,
            json!({"op":"connect","runtime":token,"source":root.canonical_path.clone().unwrap()}),
        );
        let request = local::LocalRequest::parse(&invocation.params).unwrap();
        // The durable writer itself is covered by protected storage tests. This
        // fixture injects its terminal result at the existing owner boundary.
        runtime.transient.candidate = Some(transient::Candidate::Local(transient::LocalCandidate {
            inventory: vec![runtime.local_inventory.sources[0].clone(), acquire_root(root.clone())],
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
                owner: runtime.transient.accepted.as_ref().map(|o| o.owner()),
                profile: runtime.effective_profile().cloned(),
                restart: true,
            },
            phase: transient::CandidatePhase::SavingConnection(root),
        }));
        assert_eq!(runtime.local_inventory.sources.len(), 1);
        let response = transient::connection_saved(&mut runtime, &app, &invocation, saved).unwrap();
        assert_eq!(response["data"]["state"] == "ready", saved);
        assert_eq!(
            runtime.local_inventory.sources.len(),
            if saved { 2 } else { 1 }
        );
        assert!(runtime.transient.candidate.is_none());
        assert_eq!(
            serde_json::to_value(&runtime.saved).unwrap(),
            original,
            "completion handler must not make its own unverified durable write"
        );
        app.ott = Some(runtime);
    }
}

#[test]
fn remembered_local_favorite_restore_waits_for_bound_inventory_validation() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for available in [false, true] {
        let (_root_dir, mut app, mut root) = setup();
        let mut runtime = app.ott.take().unwrap();
        let profile = transient::source_profile(&root).unwrap();
        let catalog_row = runtime.catalog.entries[0].clone();
        let favorite = selection::capture_bound(&runtime.selection_bindings().unwrap(),
            selection::SelectionOwner::LocalRoot(&root), &catalog_row).unwrap();
        let id = format!("favorite:{}", favorite.identity());
        runtime.saved.favorites.toggle(favorite).unwrap();
        let profiles = runtime.saved.profiles.clone();
        let sources = runtime.local_inventory.sources.clone();
        let bindings = selection::SelectionBindings::new(&profiles, sources.iter().map(LocalSource::descriptor), 7).unwrap();
        runtime.saved.local_favorite_proofs.record_capture(&bindings, &runtime.saved.favorites,
            selection::SelectionOwner::LocalRoot(&root), &runtime.catalog.entries, &catalog_row).unwrap();
        runtime.saved.favorites_selected = true;
        runtime.saved.resume.insert(
            "favorites".into(),
            SavedResume {
                entry_id: id.clone(),
                position_ms: 15000,
                source_fingerprint: profile_fingerprint(&profile),
            },
        );
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
        runtime.local_inventory = transient::Inventory::default();
        runtime.load_catalog(); // catalog_done=false keeps the existing worker inert
        assert_eq!(
            runtime
                .continuity
                .pending_restore()
                .map(|r| r.entry_id.as_str()),
            Some(id.as_str())
        );
        let generation = runtime.generation;
        runtime.local_inventory.phase = transient::InventoryPhase::Loading(generation);
        root.available = available;
        transient::binding_result(
            &mut runtime,
            &mut app,
            generation,
            Ok(ott_catalog::LocalBindings {
        operation_error: None,
                sources: vec![if available { acquire_root(root) } else { LocalSource::unavailable(root) }],
                lookup: None,
                matched: vec![],
            }),
        );
        assert_eq!(runtime.continuity.pending_restore().is_some(), available);
        assert!(
            runtime.resolver_next.is_none(),
            "inventory completion cannot itself start a decoder"
        );
        app.ott = Some(runtime);
    }
}

fn candidate_with_successor(op: &str) -> (TempDir, Box<App>) {
    let (_root_dir, mut app, root) = setup();
    install(&mut app, Some(&root), "first");
    let mut runtime = app.ott.take().unwrap();
    runtime.catalog.entries.push(item("second"));
    transient::accepted_catalog_update(&mut runtime);
    runtime.saving = false;
    let token = runtime.remote.runtime().to_owned();
    let params = match op {
        "sources" => json!({"op":op,"runtime":token,"path":_root_dir.folder("Another")}),
        "connect" => json!({"op":op,"runtime":token,"source":_root_dir.folder("Another")}),
        _ => json!({"op":op,"runtime":token,"source":_root_dir.folder("Another"),"progressive":true}),
    };
    let inv = local_invocation(&mut runtime, 1, params);
    let response = commands::command(&mut runtime, &mut app, &inv);
    assert!(response.is_none(), "accepted {op}: {response:?}");
    assert!(runtime.transient.candidate.is_some());
    app.ott = Some(runtime);
    (_root_dir, app)
}

#[test]
fn nonplay_local_queries_leave_current_eof_succession_active() {
    let _serial = plx_base::testlock::serial();
    for op in ["sources", "connect", "play", "shuffle"] {
        let _reset = Reset;
        let (_root_dir, mut app) = candidate_with_successor(op);
        assert!(finish_catalog_playback(&mut app));
        let runtime = app.ott.as_ref().unwrap();
        let playing_request = matches!(op, "play" | "shuffle");
        assert_eq!(
            runtime.resolver_next.is_none(),
            playing_request,
            "{op} must claim playback only when selecting new media"
        );
        assert_eq!(
            runtime.play_target.as_ref().map(|r| r.id.as_str()),
            if playing_request {
                None
            } else {
                Some("second")
            }
        );
        assert!(
            runtime.transient.candidate.is_some(),
            "root validation still owns its independent result"
        );
        assert!(runtime.checkpoints.is_empty());
    }
}

#[test]
fn nonplay_local_queries_leave_current_failure_succession_active() {
    let _serial = plx_base::testlock::serial();
    for op in ["sources", "connect", "play", "shuffle"] {
        let _reset = Reset;
        let (_root_dir, mut app) = candidate_with_successor(op);
        app.ott
            .as_mut()
            .unwrap()
            .playing
            .as_mut()
            .unwrap()
            .advance_on_failure = true;
        plx_media::route::refuse_for_test(
            &mut app.player.session,
            plx_media::route::PlayVerdict::DirectPlayDisabled,
        );
        assert_eq!(
            plx_media::player::state(&app.player.session),
            plx_media::player::PlaybackState::Error
        );
        let mut runtime = app.ott.take().unwrap();
        runtime.reconcile_failure(&mut app);
        let playing_request = matches!(op, "play" | "shuffle");
        assert_eq!(
            runtime.resolver_next.is_none(),
            playing_request,
            "{op} must not suppress ordinary failure policy"
        );
        assert_eq!(
            runtime.play_target.as_ref().map(|r| r.id.as_str()),
            if playing_request {
                None
            } else {
                Some("second")
            }
        );
        app.ott = Some(runtime);
    }
}

fn add_root_proof(app: &mut App, root: &local::BoundSource) -> String {
    let runtime = app.ott.as_mut().unwrap();
    let row = runtime.catalog.entries[0].clone();
    let favorite = selection::capture_bound(&runtime.selection_bindings().unwrap(),
        selection::SelectionOwner::LocalRoot(root), &row).unwrap();
    let id = format!("favorite:{}", favorite.identity());
    runtime.saved.favorites.toggle(favorite).unwrap();
    let profiles = runtime.saved.profiles.clone();
    let roots = runtime.local_inventory.sources.clone();
    let bindings = selection::SelectionBindings::new(&profiles, roots.iter().map(LocalSource::descriptor), 7).unwrap();
    runtime.saved.local_favorite_proofs.record_capture(&bindings, &runtime.saved.favorites,
        selection::SelectionOwner::LocalRoot(root), &runtime.catalog.entries, &row).unwrap();
    id
}
fn observed_loss(app: &mut App, root: &local::BoundSource) {
    let mut runtime = app.ott.take().unwrap();
    let mut unavailable = root.clone(); unavailable.available = false;
    let generation = runtime.generation;
    runtime.local_inventory.phase = transient::InventoryPhase::Loading(generation);
    transient::binding_result(&mut runtime, app, generation, Ok(ott_catalog::LocalBindings {
        operation_error: None,
        sources: vec![LocalSource::unavailable(unavailable)], lookup: None, matched: Vec::new(),
    }));
    app.ott = Some(runtime);
}

fn remember_stable_usb(runtime: &mut Runtime, root: &local::BoundSource) -> crate::app::ott_local_sources::FolderDescriptor {
    use crate::app::ott_local_sources::{FolderDescriptor, LocalSourceKind, RememberedFolders, UsbBinding};
    let folder = FolderDescriptor {
        id: root.id.clone(), name: root.name.clone(),
        path: root.canonical_path.clone().unwrap(), kind: LocalSourceKind::Folder,
        usb: Some(UsbBinding::from_observed("ext4", "fixture-stable-volume", "/", "Movies").unwrap()),
    };
    RememberedFolders::default().with_connected(folder.clone()).unwrap()
        .put_into(&mut runtime.saved.settings).unwrap();
    folder
}

#[test]
fn observed_stable_usb_loss_preserves_only_dormant_proof_through_runtime_cleanup() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    remember_stable_usb(app.ott.as_mut().unwrap(), &root);
    let favorite = add_root_proof(&mut app, &root);
    let runtime = app.ott.as_ref().unwrap();
    let binding = runtime.bind_local_selection(&root).unwrap();
    let source = runtime.local_inventory.sources[0].clone();
    let proofs = runtime.saved.local_favorite_proofs.clone();
    assert_ne!(serde_json::to_value(&proofs).unwrap()["entries"], json!([]));
    // Feed the same addressed observation as the binding worker. The saved USB
    // identity is fixture intent; no device, mount or filesystem worker runs.
    observed_loss(&mut app, &root);
    let runtime = app.ott.as_mut().unwrap();
    assert!(!source.is_live());
    assert!(runtime.current_local_source(&binding).is_none());
    assert!(runtime.transient.accepted.is_none());
    assert!(runtime.local_inventory.proof_cleanup == transient::ProofCleanup::Clean);
    let mut candidate = runtime.saved.clone();
    transient::revoke_unavailable_proofs(runtime, &mut candidate);
    assert_eq!(candidate.local_favorite_proofs, proofs);
    assert_eq!(candidate.favorites, runtime.saved.favorites);
    assert!(runtime.play_at(&favorite, 0).is_err(), "dormant intent must not reopen an absent source");
    assert!(runtime.resolver_next.is_none());
}

#[test]
fn runtime_cleanup_revokes_detached_proof_when_stable_intent_is_not_exact() {
    use crate::app::ott_local_sources::RememberedFolders;
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    let folder = remember_stable_usb(app.ott.as_mut().unwrap(), &root);
    add_root_proof(&mut app, &root);
    observed_loss(&mut app, &root);
    let runtime = app.ott.as_ref().unwrap();
    for change in ["uuid", "filesystem", "relative", "path", "removed", "invalid", "legacy"] {
        let mut candidate = runtime.saved.clone();
        let mut changed = folder.clone();
        match change {
            "uuid" => changed.usb.as_mut().unwrap().uuid.push_str("-other"),
            "filesystem" => changed.usb.as_mut().unwrap().filesystem_type = "vfat".into(),
            "relative" => changed.usb.as_mut().unwrap().filesystem_relative_folder.push_str("/other"),
            "path" => changed.path.push_str("/other"),
            "legacy" => changed.usb = None,
            "removed" | "invalid" => {},
            _ => unreachable!(),
        }
        let folders = if change == "removed" { RememberedFolders::default() }
            else { RememberedFolders::default().with_connected(changed).unwrap() };
        folders.put_into(&mut candidate.settings).unwrap();
        if change == "invalid" { candidate.settings.insert("nativeLocalSources".into(), json!({"version":0})); }
        transient::revoke_unavailable_proofs(runtime, &mut candidate);
        assert_eq!(serde_json::to_value(&candidate.local_favorite_proofs).unwrap()["entries"], json!([]), "{change}");
        assert_eq!(candidate.favorites, runtime.saved.favorites, "{change}");
    }
}

#[test]
fn observed_local_loss_retires_the_actual_playback_and_old_binding_before_reconnect() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    let favorite = add_root_proof(&mut app, &root);
    install(&mut app, Some(&root), "first");
    let old = app.ott.as_ref().unwrap().playing.as_ref().unwrap().local_selection.clone().unwrap();
    assert!(app.ott.as_ref().unwrap().current_local_root(&old).is_some());
    observed_loss(&mut app, &root);
    let runtime = app.ott.as_mut().unwrap();
    assert!(runtime.current_local_root(&old).is_none(), "observed absence retires the live binding");
    assert!(runtime.transient.accepted.is_none(), "old catalog cannot attest the returning folder");
    assert!(runtime.playing.is_none());
    assert!(runtime.catalog.entries.is_empty());
    assert!(runtime.play_at(&favorite, 0).is_err());
    assert!(runtime.resolver_next.is_none());
    assert!(plx_media::route::external_source(&app.player.session).is_none());
    assert!(!plx_media::route::has_url(&app.player.session));
}

#[test]
fn old_saved_after_local_loss_stays_restricted_until_verified_proof_cleanup() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    let favorite = add_root_proof(&mut app, &root);
    let before_loss = app.ott.as_ref().unwrap().saved.clone();
    observed_loss(&mut app, &root);
    {
        let runtime = app.ott.as_mut().unwrap();
        assert!(runtime.local_inventory.proof_cleanup == transient::ProofCleanup::Pending);
        // An already-admitted older writer owns its old image. Pausing loading
        // prevents this fixture from spawning the follow-up writer; the actual
        // Job::Saved publication and cleanup state transition still execute.
        runtime.loaded = false;
        runtime.tx.send(Job::Saved(Ok(before_loss), None, false, ok(json!({})))).unwrap_or_else(|_| panic!("receiver"));
    }
    runtime_tick(&mut app);
    {
        let runtime = app.ott.as_mut().unwrap();
        assert!(runtime.local_inventory.proof_cleanup == transient::ProofCleanup::Pending);
        assert!(!runtime.local_inventory.sources[0].descriptor().available);
        assert!(runtime.play_at(&favorite, 0).is_err());
        let mut verified = runtime.saved.clone();
        transient::revoke_unavailable_proofs(runtime, &mut verified);
        assert!(verified.local_favorite_proofs != runtime.saved.local_favorite_proofs);
        // Inject the normal verified write completion, not a fabricated grant.
        runtime.local_inventory.proof_cleanup = transient::ProofCleanup::Writing;
        runtime.saving = true;
        runtime.tx.send(Job::Saved(Ok(verified), None, false, ok(json!({})))).unwrap_or_else(|_| panic!("receiver"));
    }
    runtime_tick(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    assert!(runtime.local_inventory.proof_cleanup == transient::ProofCleanup::Clean);
    assert_eq!(runtime.saved.favorites.entries().len(), 1, "portable favorite is retained");
    assert_eq!(serde_json::to_value(&runtime.saved.local_favorite_proofs).unwrap()["entries"], json!([]));
    assert!(runtime.play_at(&favorite, 0).is_err(), "cleanup alone never reconnects");
}

#[test]
fn failed_local_proof_cleanup_blocks_connect_and_explicit_retry_cannot_mint_a_grant() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    let favorite = add_root_proof(&mut app, &root);
    observed_loss(&mut app, &root);
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.loaded = false;
        runtime.local_inventory.proof_cleanup = transient::ProofCleanup::Writing;
        runtime.tx.send(Job::Saved(Err("synthetic protected-write failure"), None, false, ok(json!({})))).unwrap_or_else(|_| panic!("receiver"));
    }
    runtime_tick(&mut app);
    let mut runtime = app.ott.take().unwrap();
    assert!(runtime.local_inventory.proof_cleanup == transient::ProofCleanup::Failed);
    runtime.loaded = true;
    let token = runtime.remote.runtime().to_owned();
    let invocation = local_invocation(&mut runtime, 1, json!({"op":"connect",
        "runtime":token,"source":root.canonical_path.clone().unwrap()}));
    let response = commands::command(&mut runtime, &mut app, &invocation).unwrap();
    assert_ne!(response["data"]["state"], "ready");
    assert!(runtime.local_inventory.proof_cleanup == transient::ProofCleanup::Pending);
    assert!(runtime.transient.candidate.is_none());
    assert!(!runtime.local_inventory.sources[0].descriptor().available);
    assert!(runtime.play_at(&favorite, 0).is_err());
    app.ott = Some(runtime);
}

#[test]
fn root_limit_operation_failure_still_retires_its_current_observed_loss() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    let favorite = add_root_proof(&mut app, &root);
    install(&mut app, Some(&root), "first");
    let mut runtime = app.ott.take().unwrap();
    for index in 1..32 {
        let mut other = root.clone();
        other.id = format!("root-{index}"); other.name = format!("Folder {index}");
        other.authority_ref = local::AuthorityRef(index + 1);
        other.canonical_path = Some(_root_dir.folder(&format!("folder-{index}")));
        runtime.local_inventory.sources.push(acquire_root(other));
    }
    runtime.saving = false;
    let token = runtime.remote.runtime().to_owned();
    let invocation = local_invocation(&mut runtime, 1, json!({
        "op":"connect", "runtime":token, "source":_root_dir.folder("folder-32")}));
    assert!(commands::command(&mut runtime, &mut app, &invocation).is_none());
    let generation = runtime.generation;
    let result = |mut sources: Vec<LocalSource>| {
        sources[0] = LocalSource::unavailable(sources[0].descriptor().clone());
        ott_catalog::LocalBindings { sources, lookup: Some(_root_dir.folder("folder-32")),
            matched: Vec::new(), operation_error: Some(local::LocalError::TooManyItems) }
    };
    let stale = result(runtime.local_inventory.sources.clone());
    transient::binding_result(&mut runtime, &mut app, generation - 1, Ok(stale));
    assert!(runtime.local_inventory.sources[0].descriptor().available, "stale operation cannot revoke a newer owner");
    assert!(runtime.playing.is_some());
    let current = result(runtime.local_inventory.sources.clone());
    transient::binding_result(&mut runtime, &mut app, generation, Ok(current));
    assert_eq!(runtime.local_inventory.sources.len(), 32, "failed connect cannot publish root 33");
    assert!(!runtime.local_inventory.sources[0].descriptor().available);
    assert!(runtime.transient.candidate.is_none());
    assert!(runtime.transient.accepted.is_none());
    assert!(runtime.playing.is_none());
    assert!(runtime.local_inventory.proof_cleanup == transient::ProofCleanup::Pending);
    assert!(runtime.play_at(&favorite, 0).is_err());
    assert!(runtime.resolver_next.is_none());
    assert!(plx_media::route::external_source(&app.player.session).is_none());
    app.ott = Some(runtime);
}

#[test]
fn dropping_the_inventory_withdraws_a_retained_real_root_handle() {
    let _serial = plx_base::testlock::serial();
    let root_dir = TempDir::new();
    let source = acquire_root(local::BoundSource {
        id: "drop-root".into(),
        name: "Movies".into(),
        kind: local::SourceKind::Folder,
        available: true,
        authority_ref: local::AuthorityRef(1),
        generation: 7,
        canonical_path: Some(root_dir.folder("Movies")),
    });
    let retained = source.root_handle().unwrap().clone();
    let mut inventory = transient::Inventory::default();
    inventory.install(vec![source], 7);
    assert!(retained.is_live());
    drop(inventory);
    assert!(
        !retained.is_live(),
        "the live inventory, not a borrowed clone, owns withdrawal"
    );
}

fn prepare_late_favorite_commit(runtime: &mut Runtime) -> (favorites::CommitResult, usize) {
    use plx_data::ott::favorites::Document;
    use plx_data::ott::favorites_wire::{self as wire, Context, Fx, Resolution};
    runtime.saving = false;
    let before = runtime.saved.clone();
    let mut added = before.favorites.entries()[0].clone();
    added.title = "Later imported item".into();
    added.locator = plx_data::ott::favorites::Locator::Local {
        root_alias: "Movies".into(), relative_path: "Later.mp4".into(),
    };
    let mut incoming = Document::default(); incoming.toggle(added).unwrap();
    let bytes = incoming.encode().unwrap();
    let revision = wire::revision(&before.favorites).unwrap();
    let context_revision = "b".repeat(64);
    let token = runtime.remote.runtime().to_owned();
    let now = runtime.now_ms();
    let deadline = now.saturating_add(30_000);
    let resolve = |_: &plx_data::ott::favorites::Favorite| Resolution::Missing;
    let context = Context { runtime: &token, now_ms: now, wall_now_ms: now, revision: &revision,
        context_revision: &context_revision, document: &before.favorites,
        authorized: true, write_ready: true, write_blocker: None,
        fresh_ids: ["20000000-0000-4000-8000-000000000001", "20000000-0000-4000-8000-000000000002"],
        resolve: &resolve, durable_status: &before.receipts };
    let mut step = |mut params: Value| {
        params["runtime"] = json!(&token);
        runtime.favorites.execute(&params, params.to_string().len() + 160, deadline, &context)
    };
    let reply = |result| match result { Fx::Reply(value) => {
        assert!(value.get("error").is_none(), "{value}"); value
    }, Fx::Commit(_) => panic!("unexpected early commit") };
    let described = reply(step(json!({"op":"describe"})));
    let begun = reply(step(json!({"op":"begin", "operation_id":described["admission"]["operation_id"],
        "mode":"merge", "expected_revision":described["revision"],
        "expected_context_revision":described["context_revision"],
        "document_sha256":wire::hash(&bytes),"size_bytes":bytes.len(),"chunk_count":1,
        "budget_ms":30_000,"required_kinds":["local-media"]})));
    let transaction = begun["transaction_id"].clone();
    reply(step(json!({"op":"append","transaction_id":transaction,"seq":0,"offset":0,
        "chunk_sha256":wire::hash(&bytes),"data_base64":plx_base::b64::encode(&bytes)})));
    let preview = reply(step(json!({"op":"preview","transaction_id":transaction,"page_offset":0,"max_entries":64})));
    assert_eq!(preview["ready"], true);
    let commit_params = json!({"op":"commit","transaction_id":transaction,
        "operation_id":described["admission"]["operation_id"],"plan_digest":preview["plan_digest"],"runtime":token});
    let Fx::Commit(intent) = step(commit_params.clone()) else { panic!("commit was not admitted"); };
    drop(step);
    let invocation = remote_invocation(runtime, 1, "favorites_global", commit_params);
    let operation = intent.receipt.operation_id.clone();
    let mut after = before;
    after.favorites = intent.document;
    after.local_favorite_proofs.prune(&after.favorites);
    after.receipts.push(wire::DurableStatus::verified(intent.receipt));
    let count = after.favorites.entries().len();
    // Pause between the real protocol's commit admission and delivery of its
    // already-verified after-image; no disk writer is needed by this fixture.
    runtime.saving = true;
    (favorites::CommitResult::from_parts_for_test(invocation, operation, Some(after),
        wire::Outcome::Verified, deadline), count)
}

#[test]
fn late_favorite_commit_after_observed_loss_cannot_restore_grant_even_when_cleanup_fails() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, root) = setup();
    let favorite = add_root_proof(&mut app, &root);
    install(&mut app, Some(&root), "first");
    let old_binding = app.ott.as_ref().unwrap().playing.as_ref().unwrap().local_selection.clone().unwrap();
    let old_proofs = app.ott.as_ref().unwrap().saved.local_favorite_proofs.clone();
    let (completion, new_count) = prepare_late_favorite_commit(app.ott.as_mut().unwrap());
    observed_loss(&mut app, &root);
    {
        let runtime = app.ott.as_mut().unwrap();
        assert!(runtime.current_local_root(&old_binding).is_none());
        assert!(runtime.local_inventory.proof_cleanup == transient::ProofCleanup::Pending);
        runtime.loaded = false; // suppress actual cleanup IO, not the completion path
        runtime.tx.send(Job::FavoriteCommit(completion)).unwrap_or_else(|_| panic!("receiver"));
    }
    runtime_tick(&mut app);
    {
        let runtime = app.ott.as_mut().unwrap();
        assert_eq!(runtime.saved.favorites.entries().len(), new_count);
        assert!(new_count > 1, "the late commit actually published its after-image");
        assert_eq!(runtime.saved.local_favorite_proofs, old_proofs,
            "do not pretend cleanup was durably committed");
        assert!(!runtime.saving);
        assert!(runtime.local_inventory.proof_cleanup == transient::ProofCleanup::Pending);
        assert!(!runtime.local_inventory.sources[0].descriptor().available);
        assert!(runtime.current_local_root(&old_binding).is_none());
        assert!(runtime.play_at(&favorite, 0).is_err());
        assert!(runtime.resolver_next.is_none());
        runtime.local_inventory.proof_cleanup = transient::ProofCleanup::Writing;
        runtime.saving = true;
        runtime.tx.send(Job::Saved(Err("synthetic cleanup failure"), None, false, ok(json!({}))))
            .unwrap_or_else(|_| panic!("receiver"));
    }
    runtime_tick(&mut app);
    let mut runtime = app.ott.take().unwrap();
    assert!(runtime.local_inventory.proof_cleanup == transient::ProofCleanup::Failed);
    assert!(runtime.current_local_root(&old_binding).is_none());
    assert!(runtime.play_at(&favorite, 0).is_err());
    assert!(runtime.resolver_next.is_none());
    runtime.loaded = true;
    let token = runtime.remote.runtime().to_owned();
    let retry = local_invocation(&mut runtime, 2,
        json!({"op":"connect","runtime":token,"source":root.canonical_path.clone().unwrap()}));
    let response = commands::command(&mut runtime, &mut app, &retry).unwrap();
    assert_ne!(response["data"]["state"], "ready");
    assert!(runtime.local_inventory.proof_cleanup == transient::ProofCleanup::Pending);
    assert!(runtime.transient.candidate.is_none());
    assert!(!runtime.local_inventory.sources[0].descriptor().available);
    assert!(runtime.current_local_root(&old_binding).is_none());
    assert!(runtime.play_at(&favorite, 0).is_err());
    app.ott = Some(runtime);
}

#[path = "local_lifecycle.rs"]
mod lifecycle_tests;

#[path = "local_multi_source_favorites.rs"]
mod multi_source_favorites_tests;

#[path = "local_kiosk_preparation.rs"]
mod kiosk_preparation_tests;

#[test]
fn accepted_viewer_pause_blocks_late_accepted_local_scan_continuation() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    struct RestoreState(u8);
    impl Drop for RestoreState {
        fn drop(&mut self) { plx_media::player::restore_state_for_test(self.0); }
    }
    let _state = RestoreState(plx_media::player::swap_state_for_test(
        plx_media::player::PlaybackState::Idle));
    let (_root_dir, mut app, root) = setup();
    install(&mut app, Some(&root), "first");
    mount_player(&mut app);
    app.adapters.player.install(plx_media::player::engine::engine_after_reload_for_test());
    // A repeated accepted Pause needs no host clock callback or decoder IO.
    assert!(plx_media::player::pause(&mut app.adapters.player));
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Paused);
    let mut runtime = app.ott.take().unwrap();
    runtime.saving = false;
    // Fixture initialization only: the assertion below must be driven by the
    // actual command -> viewer_transport path, never by directly pausing Local.
    transient::note_pause(&mut runtime, false);
    runtime.catalog_dirty = true;
    transient::accepted_catalog_update(&mut runtime);
    assert!(runtime.catalog_dirty, "the accepted scan starts with continuation allowed");
    let invocation = remote_invocation(&mut runtime, 71, "playback", json!({"operation":"pause"}));
    let response = commands::command(&mut runtime, &mut app, &invocation)
        .expect("actual pause command produces an acceptance receipt");
    assert_eq!(response["status"], "ok", "{response}");
    assert_eq!(response["data"]["dispatched"], true);
    assert!(lifecycle::viewer_paused());
    assert!(runtime.catalog_loading && !runtime.catalog_complete);
    runtime.catalog.entries.push(item("late-second"));
    runtime.catalog_dirty = true;
    transient::accepted_catalog_update(&mut runtime);
    let Some(transient::TransientMedia::Local(session)) = runtime.transient.accepted.as_ref() else {
        panic!("accepted Local session must remain installed");
    };
    assert_eq!(session.context.order(), &["first".to_owned(), "late-second".to_owned()],
        "the late batch is still ingested while viewer pause stands");
    assert!(!runtime.catalog_dirty,
        "accepted viewer Pause must suppress late Local scan continuation");
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    app.ott = Some(runtime);
}

#[test]
fn local_batch_and_channel_dispatch_use_global_favorites_not_private_queue_flag() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, _root) = setup();
    let mut entry = item("live");
    entry.media = MediaLocator::Http { url: "https://media.invalid/live".into() };
    let favorite = selection::capture(&permanent(), &entry).unwrap();
    let selected = format!("favorite:{}", favorite.identity());
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.favorites.toggle(favorite).unwrap();
        runtime.saved.favorites_selected = true;
    }
    install(&mut app, None, &selected);
    checkpoint(&mut app, 44000);
    let mut runtime = app.ott.take().unwrap();
    let Some(transient::TransientMedia::Local(session)) = runtime.transient.accepted.as_ref()
        else { panic!("Local session"); };
    assert!(!session.context.view_selection().1);
    assert!(session.borrowed.is_some());
    runtime.catalog.entries.push(item("later"));
    runtime.catalog_dirty = true;
    transient::accepted_catalog_update(&mut runtime);
    assert!(runtime.catalog_dirty,
        "a late Local batch must not suppress the global Favorites continuation");
    let inv = remote_invocation(&mut runtime, 101, "channels", json!({}));
    let response = commands::command(&mut runtime, &mut app, &inv).expect("synchronous query");
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["channels"].as_array().unwrap().len(), 1);
    assert_eq!(response["data"]["channels"][0]["id"], selected);
    runtime.saved.favorites_selected = false;
    runtime.catalog_dirty = true;
    transient::accepted_catalog_update(&mut runtime);
    assert!(!runtime.catalog_dirty, "direct borrow remains one-shot outside Favorites");
    app.ott = Some(runtime);
}

#[path = "local_kiosk_resume.rs"]
mod kiosk_resume_tests;

#[test]
fn global_favorite_borrowed_from_local_keeps_durable_resume_and_temporary_bookmark() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, _root) = setup();
    let mut entry = item("live");
    entry.media = MediaLocator::Http { url: "https://media.invalid/live".into() };
    let favorite = selection::capture(&permanent(), &entry).unwrap();
    let selected = format!("favorite:{}", favorite.identity());
    let permanent_profiles = app.ott.as_ref().unwrap().saved.profiles.clone();
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.favorites.toggle(favorite).unwrap();
        runtime.saved.favorites_selected = true;
    }
    install(&mut app, None, &selected);
    checkpoint(&mut app, 44_000);
    assert_eq!(borrowed_position(&app), Some(44_000));
    let runtime = app.ott.as_ref().unwrap();
    let point = &runtime.checkpoints["favorites"];
    assert_eq!(point.entry_id, selected);
    assert_eq!(point.position_ms, 44_000);
    assert_eq!(point.source_fingerprint, profile_fingerprint(&permanent()));
    assert!(!runtime.checkpoints.contains_key("permanent"));
    let mut persisted = runtime.saved.clone();
    kiosk_resume::merge(&mut persisted, &runtime.checkpoints);
    let restored: Saved = serde_json::from_slice(&serde_json::to_vec(&persisted).unwrap()).unwrap();
    assert_eq!(restored.profiles, permanent_profiles);
    assert!(restored.favorites_selected);
    assert!(restored.resume["favorites"] == *point);
}

#[test]
fn borrowed_global_favorite_cannot_checkpoint_after_selection_or_authority_changes() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    for change in ["global-off", "removed", "owner", "source", "item", "noncanonical"] {
        let (_root_dir, mut app, _root) = setup();
        let mut entry = item("live");
        entry.media = MediaLocator::Http { url: "https://media.invalid/live".into() };
        let favorite = selection::capture(&permanent(), &entry).unwrap();
        let identity = favorite.identity();
        let selected = format!("favorite:{identity}");
        {
            let runtime = app.ott.as_mut().unwrap();
            runtime.saved.favorites.toggle(favorite).unwrap();
            runtime.saved.favorites_selected = true;
        }
        install(&mut app, None, &selected);
        {
            let runtime = app.ott.as_mut().unwrap();
            match change {
                "global-off" => runtime.saved.favorites_selected = false,
                "removed" => { runtime.saved.favorites.remove(&identity); }
                "owner" => runtime.playing.as_mut().unwrap().transient_owner = Some(OwnerToken(999)),
                "source" => runtime.saved.profiles.profiles[0].source = "https://other.invalid/list.m3u".into(),
                "item" => {
                    let playing = runtime.playing.as_mut().unwrap();
                    playing.entry.title = "Different programme".into();
                    playing.entry.provider_id = Some("different-id".into());
                }
                "noncanonical" => runtime.playing.as_mut().unwrap().selection_id = format!("favorite: {identity}"),
                _ => unreachable!(),
            }
        }
        checkpoint(&mut app, 44_000);
        assert!(app.ott.as_ref().unwrap().checkpoints.is_empty(), "{change}");
    }
}

#[test]
fn favorite_without_transient_owner_keeps_existing_checkpoint_behavior() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, _root) = setup();
    let mut entry = item("live");
    entry.media = MediaLocator::Http { url: "https://media.invalid/live".into() };
    let favorite = selection::capture(&permanent(), &entry).unwrap();
    let selected = format!("favorite:{}", favorite.identity());
    app.ott.as_mut().unwrap().saved.favorites.toggle(favorite).unwrap();
    install(&mut app, None, &selected);
    let runtime = app.ott.as_mut().unwrap();
    runtime.playing.as_mut().unwrap().transient_owner = None;
    runtime.transient.accepted = None;
    checkpoint(&mut app, 12_000);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.saved.favorites_selected);
    assert_eq!(runtime.checkpoints["favorites"].entry_id, selected);
    assert_eq!(runtime.checkpoints["favorites"].position_ms, 12_000);
}

#[test]
fn global_favorite_eof_in_local_session_clears_durable_position_without_changing_provider() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_root_dir, mut app, _root) = setup();
    let mut entry = item("live");
    entry.media = MediaLocator::Http { url: "https://media.invalid/live".into() };
    let favorite = selection::capture(&permanent(), &entry).unwrap();
    let selected = format!("favorite:{}", favorite.identity());
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.favorites.toggle(favorite).unwrap();
        runtime.saved.favorites_selected = true;
    }
    install(&mut app, None, &selected);
    checkpoint(&mut app, 44_000);
    assert!(finish_catalog_playback(&mut app));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(runtime.checkpoints["favorites"].entry_id, selected);
    assert_eq!(runtime.checkpoints["favorites"].position_ms, 0);
    assert_eq!(runtime.saved.profiles.active_profile.as_deref(), Some("permanent"));
}
