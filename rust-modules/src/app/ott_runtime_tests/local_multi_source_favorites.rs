//! Multiple authorized fixture roots use the real capture, proof and resolver
//! preparation paths. Dummy temporary files are never decoded; no network or
//! protected-state worker starts. No private proof is constructed by a fixture.
use super::*;
use plx_base::task::OffFrame;

fn catalogue(root: &local::BoundSource) -> Catalog {
    plx_data::ott::catalog_from_local_files(&[plx_data::ott::LocalFile {
        relative_path: "same.mp4".into(), size: 1, duration_ms: None,
    }], &transient::source_profile(root).unwrap()).unwrap()
}

fn capture(runtime: &mut Runtime, root: &local::BoundSource, catalog: &Catalog) -> String {
    let profiles = runtime.saved.profiles.clone();
    let sources = runtime.local_inventory.sources.clone();
    let bindings = selection::SelectionBindings::new(&profiles,
        sources.iter().map(LocalSource::descriptor), runtime.local_inventory.generation()).unwrap();
    let owner = selection::SelectionOwner::LocalRoot(root);
    let favorite = selection::capture_bound(&bindings, owner, &catalog.entries[0]).unwrap();
    assert!(runtime.saved.local_favorite_proofs.toggle_capture_with_pins(
        &mut runtime.saved.favorites, &[], &bindings, owner, &catalog.entries, &catalog.entries[0],
    ).unwrap());
    format!("favorite:{}", favorite.identity())
}

fn pair() -> (TempDir, Box<App>, [local::BoundSource; 2], [Catalog; 2], [String; 2]) {
    let (directory, mut app, first) = setup();
    let second = local::BoundSource {
        id: "second-root".into(), name: "Series".into(), kind: local::SourceKind::Folder,
        available: true, authority_ref: local::AuthorityRef(2), generation: 7,
        canonical_path: Some(directory.folder("Series")),
    };
    for root in [&first, &second] {
        std::fs::write(std::path::Path::new(root.canonical_path.as_deref().unwrap()).join("same.mp4"), b"x").unwrap();
    }
    let catalogs = [catalogue(&first), catalogue(&second)];
    let runtime = app.ott.as_mut().unwrap();
    let mut sources = runtime.local_inventory.sources.clone();
    sources.push(acquire_root(second.clone()));
    runtime.local_inventory.install(sources, 7);
    let ids = [capture(runtime, &first, &catalogs[0]), capture(runtime, &second, &catalogs[1])];
    runtime.saved.favorites_selected = true;
    assert_ne!(ids[0], ids[1]);
    assert_ne!(catalogs[0].entries[0].id, catalogs[1].entries[0].id);
    (directory, app, [first, second], catalogs, ids)
}

fn prepare(runtime: &mut Runtime, id: &str) -> (PlaybackRequest, ProviderProfile, transient::LocalSourceRead) {
    runtime.resolver_busy = true;
    runtime.play_at(id, 0).unwrap();
    let request = runtime.play_target.as_ref().unwrap().clone();
    assert!(transient::target_current(runtime));
    let resolve = runtime.resolver_next.as_ref().unwrap();
    assert!(resolve.entry.is_none(), "Favorite is resolved under its own source");
    assert!(resolve.local_proof.is_some(), "a root-level file requires its captured proof");
    let read = resolve.local_read.as_ref().unwrap().clone();
    let favorite = resolve.favorite.as_ref().unwrap();
    let resolved = ott_catalog::find_favorite(&OffFrame::for_test(), &resolve.profile,
        favorite, resolve.local_proof.as_ref(), &AtomicBool::new(false)).unwrap();
    assert_eq!(resolved.id, resolve.local_proof.as_ref().unwrap().item_id());
    assert!(selection::matches_entry(favorite, &resolved));
    let opened = transient::open_video_source(&OffFrame::for_test(), Some(&read),
        &resolve.profile, &resolved, &AtomicBool::new(false)).unwrap();
    assert!(matches!(opened, plx_media::ff::ExternalSource::HeldLocal(ref held) if held.is_live()));
    (request, resolve.profile.clone(), read)
}

#[test]
fn multi_root_favorites_with_identical_filenames_keep_their_own_grants() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, roots, catalogs, ids) = pair();
    let runtime = app.ott.as_mut().unwrap();
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    for index in [0, 1, 0] {
        let (request, profile, read) = prepare(runtime, &ids[index]);
        assert_eq!(request.local_selection.as_ref().unwrap().source_id, roots[index].id);
        assert_eq!(profile.source, roots[index].canonical_path.clone().unwrap());
        assert_eq!(read.source.descriptor().id, roots[index].id);
        assert!(read.source.same_grant(&runtime.local_inventory.sources[index]));
        assert!(!read.source.same_grant(&runtime.local_inventory.sources[1 - index]));
        let opened = transient::open_video_source(&OffFrame::for_test(), Some(&read),
            &profile, &catalogs[index].entries[0], &AtomicBool::new(false)).unwrap();
        assert!(matches!(opened, plx_media::ff::ExternalSource::HeldLocal(ref held) if held.is_live()));
        assert_eq!(runtime.transient.accepted.as_ref().unwrap().root().unwrap().id, roots[0].id,
            "borrowing a Favorite must not replace the temporary folder");
    }
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
}

#[test]
fn one_local_root_grant_cannot_open_another_roots_favorite() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, _roots, catalogs, ids) = pair();
    let runtime = app.ott.as_mut().unwrap();
    let (request_a, profile_a, read_a) = prepare(runtime, &ids[0]);
    let (_, profile_b, read_b) = prepare(runtime, &ids[1]);
    assert!(transient::capture_video_root(runtime, &request_a, &profile_b).is_err());
    assert!(transient::open_video_source(&OffFrame::for_test(), Some(&read_a),
        &profile_b, &catalogs[1].entries[0], &AtomicBool::new(false)).is_err());
    assert!(transient::open_video_source(&OffFrame::for_test(), Some(&read_b),
        &profile_a, &catalogs[0].entries[0], &AtomicBool::new(false)).is_err());
    let open = |read: &transient::LocalSourceRead, profile: &ProviderProfile, entry: &CatalogEntry| {
        let opened = transient::open_video_source(&OffFrame::for_test(), Some(read), profile,
            entry, &AtomicBool::new(false)).unwrap();
        let plx_media::ff::ExternalSource::HeldLocal(held) = opened else { panic!("held fixture file"); };
        held
    };
    let held_a = open(&read_a, &profile_a, &catalogs[0].entries[0]);
    let held_b = open(&read_b, &profile_b, &catalogs[1].entries[0]);
    runtime.local_inventory.sources[1].withdraw();
    assert!(held_a.is_live());
    assert!(!held_b.is_live());
    assert!(!transient::target_current(runtime));
    assert!(runtime.play_at(&ids[1], 0).is_err());
    prepare(runtime, &ids[0]);
}

#[test]
fn unrelated_local_inventory_refresh_fences_old_request_without_reusing_a_grant() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (directory, mut app, _roots, _catalogs, ids) = pair();
    let runtime = app.ott.as_mut().unwrap();
    let (old_request, _, old_read) = prepare(runtime, &ids[1]);
    let third = local::BoundSource {
        id: "third-root".into(), name: "Documentaries".into(), kind: local::SourceKind::Folder,
        available: true, authority_ref: local::AuthorityRef(3), generation: 8,
        canonical_path: Some(directory.folder("Documentaries")),
    };
    let mut sources = runtime.local_inventory.sources.clone();
    sources.push(acquire_root(third));
    runtime.local_inventory.install(sources, 8);
    assert!(old_read.is_live());
    assert!(runtime.local_source_read_is_current(&old_read));
    assert!(!transient::target_current(runtime), "an old delayed receipt is fenced by the inventory revision");
    let (new_request, _, new_read) = prepare(runtime, &ids[1]);
    assert!(old_request.local_selection != new_request.local_selection);
    assert!(old_read.same_grant(&new_read), "refresh uses the same live authorized root");
}

#[test]
fn replacing_one_authorized_root_retires_its_grant_without_affecting_the_other() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, roots, catalogs, ids) = pair();
    let runtime = app.ott.as_mut().unwrap();
    let (_, _, read_a) = prepare(runtime, &ids[0]);
    let (old_request, profile_b, old_read_b) = prepare(runtime, &ids[1]);
    let mut replacement = roots[1].clone();
    replacement.authority_ref = local::AuthorityRef(22);
    replacement.generation = 8;
    let sources = vec![runtime.local_inventory.sources[0].clone(), acquire_root(replacement)];
    runtime.local_inventory.install(sources, 8);
    assert!(read_a.is_live());
    assert!(!old_read_b.is_live(), "refresh retires the old source capability");
    assert!(!runtime.local_source_read_is_current(&old_read_b));
    assert!(!transient::target_current(runtime));
    assert!(transient::capture_video_root(runtime, &old_request, &profile_b).is_err());
    assert!(transient::open_video_source(&OffFrame::for_test(), Some(&old_read_b),
        &profile_b, &catalogs[1].entries[0], &AtomicBool::new(false)).is_err());
    let (new_request, _, new_read_b) = prepare(runtime, &ids[1]);
    assert!(new_request.local_selection.as_ref().unwrap().authority == local::AuthorityRef(22));
    assert!(!old_read_b.same_grant(&new_read_b));
    assert!(new_read_b.is_live());
    let (_, _, current_a) = prepare(runtime, &ids[0]);
    assert!(read_a.same_grant(&current_a));
}

#[test]
fn duplicate_local_alias_blocks_only_ambiguous_favorites() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (directory, mut app, roots, _catalogs, ids) = pair();
    let runtime = app.ott.as_mut().unwrap();
    let duplicate = local::BoundSource {
        id: "duplicate-root".into(), name: roots[0].name.clone(), kind: local::SourceKind::Folder,
        available: true, authority_ref: local::AuthorityRef(3), generation: 8,
        canonical_path: Some(directory.folder("DifferentMovies")),
    };
    let mut sources = runtime.local_inventory.sources.clone();
    sources.push(acquire_root(duplicate));
    runtime.local_inventory.install(sources, 8);
    let document = runtime.saved.favorites.clone();
    assert!(runtime.play_at(&ids[0], 0).is_err());
    assert_eq!(runtime.saved.favorites, document);
    prepare(runtime, &ids[1]);
}

#[test]
fn portable_multi_root_favorites_import_creates_neither_root_nor_file_proof() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_directory, mut app, roots, _catalogs, ids) = pair();
    let sender = app.ott.as_mut().unwrap();
    let portable = serde_json::to_vec(&sender.saved.favorites).unwrap();
    let mut receiver = runtime_fixture();
    receiver.saved.profiles = sender.saved.profiles.clone();
    receiver.saved.favorites = Document::parse(&portable).unwrap();
    receiver.saved.favorites_selected = true;
    for id in &ids {
        assert!(receiver.prepare_play_request(PlaybackRequest::ordinary(id, 0)).is_err());
    }
    assert!(receiver.local_inventory.sources.is_empty());
    // Even existing receiving-device root authority does not attest the legacy
    // root-level basename: the sender's private file proof was not exported.
    let receiving_sources = roots.into_iter().enumerate().map(|(index, mut root)| {
        root.authority_ref = local::AuthorityRef(11 + index as u64);
        root.generation = 17;
        let source = acquire_root(root);
        assert!(!source.same_grant(&sender.local_inventory.sources[index]));
        source
    }).collect();
    receiver.local_inventory.install(receiving_sources, 17);
    for id in &ids {
        assert!(receiver.prepare_play_request(PlaybackRequest::ordinary(id, 0)).is_err());
    }
    assert!(receiver.resolver_next.is_none());
}
