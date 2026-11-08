//! Remote play_catalog receipts and the shared final Job::Play binding.
//! The final-start spy proves admission only: no decoder, Plex store runner,
//! network worker or protected write starts. Local uses a real scratch-root grant.
use super::*;
use super::super::local_integration;

struct Registry;
impl Registry {
    fn new() -> Self {
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
        Self
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
    }
}

// Field order keeps the scratch directory alive until the App releases its grants.
struct SourceFixture {
    f: Fixture,
    _directory: Option<local_integration::TempDir>,
}

fn session(app: &mut App, epoch: u64) {
    let mut init = app.bridge.snapshot_session_init();
    init.epoch = epoch;
    init.phase = plx_session::auth::Phase::Ready;
    init.apply_pending = false;
    init.pending_commit = None;
    init.persistence_warning = None;
    app.bridge = bridge::Bridge::for_session_test(init);
    assert!(app.bridge.session_ready_current());
}

fn source_fixture(kind: ProviderKind) -> SourceFixture {
    let (mut f, directory) = if kind == ProviderKind::Local {
        let (directory, app, root) = local_integration::setup();
        std::fs::write(std::path::Path::new(root.canonical_path.as_deref().unwrap())
            .join("first.mp4"), b"synthetic local admission fixture").unwrap();
        (Fixture { app, serial: 0 }, Some(directory))
    } else {
        (Fixture::new(0), None)
    };
    f.app.running = true;
    let mut runtime = f.app.ott.take().unwrap();
    runtime.loaded = true;
    runtime.saving = false;
    runtime.write_permit.revoke();
    runtime.entropy = Some([5; 16]);
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.resolver_busy = true;
    runtime.plex_refresh_pending = true;
    runtime.resume_source_work();
    match kind {
        ProviderKind::M3u => {
            let profile = ProviderProfile {
                id: "progressive-m3u".into(), name: "Progressive M3U".into(),
                kind, source: "https://progressive.invalid/list.m3u".into(),
                source_alias: String::new(), enabled: true,
            };
            runtime.catalog = plx_data::ott::parse_m3u(
                "#EXTM3U\n#EXTINF:-1 tvg-id=\"selected\" group-title=\"Channels\",Selected\nhttps://progressive.invalid/selected.m3u8\n",
                &profile).unwrap();
            runtime.saved.profiles.profiles = vec![profile.clone()];
            runtime.saved.profiles.active_profile = Some(profile.id);
            runtime.provider = kind;
        }
        ProviderKind::Plex => {
            let origin = plx_plex::plex::Origin::parse("https://progressive-plex.invalid:443").unwrap();
            let sid = plx_plex::plex::register_pinned_with_client_id(
                "progressive-plex-machine", &origin, "synthetic-token", None,
                "synthetic-client", Default::default());
            assert!(plx_plex::plex::set_current(sid));
            session(&mut f.app, 10);
            assert!(classic_source::ensure_server_catalog(&mut runtime, &mut f.app, sid).unwrap());
            let profile = runtime.effective_profile().unwrap();
            let mut entry = synthetic_entry();
            entry.id = format!("{}:42", profile.id);
            entry.title = "Selected Plex item".into();
            entry.media = MediaLocator::Plex {
                server_id: profile.source.clone(), rating_key: "42".into(),
            };
            runtime.catalog.entries = vec![entry];
        }
        ProviderKind::Local => {}
        _ => unreachable!(),
    }
    runtime.catalog_loading = true;
    runtime.catalog_complete = false;
    runtime.page = "channels";
    assert_eq!(runtime.effective_profile().unwrap().kind, kind);
    f.app.ott = Some(runtime);
    SourceFixture { f, _directory: directory }
}

struct Pending {
    request: ResolveRequest,
    source: ReadySource,
}

fn queue(f: &mut Fixture) -> Pending {
    let categories = f.categories();
    let page = f.channels(categories["catalog"].as_str().unwrap(), "all", 0);
    let id = page["channels"][0]["id"].as_str().expect("first published row");
    let response = f.request("play_catalog", json!({"catalog":page["catalog"],"id":id}));
    assert!(response.is_none(), "exact partial-catalog selection: {response:?}");
    let request = f.app.ott.as_mut().unwrap().resolver_next.take().expect("held resolver");
    let entry = request.entry.as_ref().unwrap();
    let source = match &entry.media {
        MediaLocator::Http { url } => ReadySource::External(
            plx_media::ff::ExternalSource::Http(url.clone()), unsupported_info(), None),
        MediaLocator::Local { .. } => {
            let source = transient::open_video_source(&plx_base::task::OffFrame::for_test(),
                request.local_read.as_ref(), &request.profile, entry, &request.cancel).unwrap();
            assert!(matches!(&source, plx_media::ff::ExternalSource::HeldLocal(_)));
            ReadySource::External(source, unsupported_info(), None)
        }
        MediaLocator::Plex { server_id, rating_key } => ReadySource::Plex {
            server_id: server_id.clone(), rating_key: rating_key.clone(),
        },
        _ => unreachable!(),
    };
    assert!(!f.app.ott.as_ref().unwrap().play_target.as_ref().unwrap().advance_on_failure);
    Pending { request, source }
}

fn deliver(f: &mut Fixture, pending: Pending, expected_starts: usize) {
    let Pending { request, source } = pending;
    let entry = request.entry.unwrap();
    let expected_profile = request.profile.clone();
    let expected_entry = entry.clone();
    let ticket = plx_media::route::play_request_ticket();
    f.app.ott.as_ref().unwrap().tx.send(Job::Play(request.generation,
        request.profile, Ok((entry, source)))).unwrap_or_else(|_| panic!("resolver receiver"));
    let clock = plx_base::task::time::ClockSample::capture_main(f.app.adapters.player.mt()).unwrap();
    let mut starts = 0;
    tick_with_start(&mut f.app, clock, |_, _, profile, entry, source| {
        starts += 1;
        assert_eq!(profile, expected_profile);
        assert_eq!(entry, expected_entry, "the resolver retains its originally selected row");
        match (&entry.media, source) {
            (MediaLocator::Http { url }, ReadySource::External(plx_media::ff::ExternalSource::Http(actual), _, None)) => assert_eq!(*url, actual),
            (MediaLocator::Local { .. }, ReadySource::External(plx_media::ff::ExternalSource::HeldLocal(_), _, None)) => {}
            (MediaLocator::Plex { server_id, rating_key }, ReadySource::Plex { server_id: actual_server, rating_key: actual_key }) => {
                assert_eq!(*server_id, actual_server);
                assert_eq!(*rating_key, actual_key);
            }
            _ => panic!("provider-specific resolved source must reach admission unchanged"),
        }
        Err("fixture admission stop")
    });
    assert_eq!(starts, expected_starts);
    assert!(plx_media::route::play_request_is_current(ticket));
    assert!(!plx_media::route::has_url(&f.app.player.session));
    assert!(f.app.bridge.metadata_view().now_playing().is_none());
    assert!(!f.app.ott.as_ref().unwrap().saving);
}

fn later(entry: &CatalogEntry) -> CatalogEntry {
    let mut later = entry.clone();
    later.id.push_str("-later");
    later.title = "Later discovered item".into();
    match &mut later.media {
        MediaLocator::Http { url } => *url = "https://progressive.invalid/later.m3u8".into(),
        MediaLocator::Local { relative_path } => *relative_path = "later.mp4".into(),
        MediaLocator::Plex { rating_key, .. } => *rating_key = "99".into(),
        _ => unreachable!(),
    }
    later
}

#[test]
fn remote_progressive_append_finish_and_metadata_preserve_m3u_local_plex_selection() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let _reset = ResetMedia;
    for kind in [ProviderKind::M3u, ProviderKind::Local, ProviderKind::Plex] {
        for mutation in ["append", "finish", "metadata"] {
            let mut fixture = source_fixture(kind);
            let f = &mut fixture.f;
            let pending = queue(f);
            let selected = pending.request.entry.as_ref().unwrap().clone();
            let runtime = f.app.ott.as_ref().unwrap();
            let generation = runtime.generation;
            let revision = runtime.catalog_revision;
            let mut rows = vec![later(&selected)];
            if mutation == "metadata" {
                let mut enriched = selected.clone();
                enriched.title = "Updated display title".into();
                enriched.description = "Updated description".into();
                enriched.poster = "https://progressive.invalid/poster.jpg".into();
                rows.push(enriched);
            }
            runtime.catalog_tx.send(CatalogEvent::Batch(generation,
                Catalog { hierarchy: None, entries: rows })).unwrap_or_else(|_| panic!("catalog receiver"));
            if mutation == "finish" {
                runtime.catalog_tx.send(CatalogEvent::Finished(generation, Ok(())))
                    .unwrap_or_else(|_| panic!("catalog receiver"));
            }
            // Job::Play is drained before catalogue events: merge in a separate tick.
            runtime_tick(&mut f.app);
            let runtime = f.app.ott.as_ref().unwrap();
            assert_eq!(runtime.generation, generation);
            assert_ne!(runtime.catalog_revision, revision);
            assert_eq!(runtime.catalog.entries.len(), 2);
            let current = runtime.catalog.entries.iter().find(|row| row.id == selected.id).unwrap();
            assert_eq!(current.media, selected.media);
            if mutation == "metadata" { assert_eq!(current.description, "Updated description"); }
            assert_eq!(runtime.catalog_complete, mutation == "finish");
            assert!(commands::pending_catalog_current(runtime));
            assert!(!pending.request.cancel.load(Ordering::Acquire));
            deliver(f, pending, 1);
            let runtime = f.app.ott.as_ref().unwrap();
            assert!(runtime.pending_play.is_none() && runtime.play_target.is_none());
            assert!(runtime.resolver_next.is_none());
        }
    }
}

#[test]
fn remote_progressive_changed_locator_source_generation_and_stop_refuse_old_result() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let _reset = ResetMedia;
    for kind in [ProviderKind::M3u, ProviderKind::Local, ProviderKind::Plex] {
        for mutation in ["locator", "source", "catalog-generation", "stop"] {
            let mut fixture = source_fixture(kind);
            let f = &mut fixture.f;
            let pending = queue(f);
            let mut runtime = f.app.ott.take().unwrap();
            let catalog_generation = runtime.generation;
            match mutation {
                "locator" => {
                    let selected = pending.request.entry.as_ref().unwrap();
                    let mut replacement = later(selected);
                    replacement.id = selected.id.clone();
                    runtime.catalog_tx.send(CatalogEvent::Batch(runtime.generation,
                        Catalog { hierarchy: None, entries: vec![replacement] }))
                        .unwrap_or_else(|_| panic!("catalog receiver"));
                }
                "source" => match kind {
                    ProviderKind::M3u => runtime.saved.profiles.profiles[0].source = "https://changed.invalid/list.m3u".into(),
                    ProviderKind::Local => runtime.local_inventory.sources[0].withdraw(),
                    ProviderKind::Plex => session(&mut f.app, 11),
                    _ => unreachable!(),
                },
                "catalog-generation" => runtime.cancel_catalog(),
                "stop" => {
                    runtime.stop_catalog(&mut f.app);
                    assert_ne!(runtime.play_generation, pending.request.generation);
                    assert!(pending.request.cancel.load(Ordering::Acquire));
                }
                _ => unreachable!(),
            }
            f.app.ott = Some(runtime);
            runtime_tick(&mut f.app);
            let runtime = f.app.ott.as_ref().unwrap();
            match mutation {
                "locator" => {
                    let selected = pending.request.entry.as_ref().unwrap();
                    let current = runtime.catalog.entries.iter().find(|row| row.id == selected.id).unwrap();
                    assert_ne!(current.media, selected.media, "the replacement must actually merge before completion");
                }
                "source" => match kind {
                    ProviderKind::M3u => assert_ne!(runtime.effective_profile(), Some(&pending.request.profile)),
                    ProviderKind::Local => assert!(!runtime.local_source_read_is_current(pending.request.local_read.as_ref().unwrap())),
                    ProviderKind::Plex => assert!(classic_source::guard_profile(runtime, &f.app, &pending.request.profile).is_err()),
                    _ => unreachable!(),
                },
                "catalog-generation" => assert_ne!(runtime.generation, catalog_generation),
                "stop" => {
                    assert_ne!(runtime.play_generation, pending.request.generation);
                    assert!(runtime.play_target.is_none());
                    assert!(pending.request.cancel.load(Ordering::Acquire));
                }
                _ => unreachable!(),
            }
            deliver(f, pending, 0);
            let runtime = f.app.ott.as_ref().unwrap();
            assert!(runtime.pending_play.is_none() && runtime.play_target.is_none());
            assert!(runtime.resolver_next.is_none());
        }
    }
}

#[test]
fn remote_progressive_superseded_result_preserves_new_m3u_local_plex_request() {
    let _serial = plx_base::testlock::serial();
    let _registry = Registry::new();
    let _reset = ResetMedia;
    for kind in [ProviderKind::M3u, ProviderKind::Local, ProviderKind::Plex] {
        let mut fixture = source_fixture(kind);
        let f = &mut fixture.f;
        let old = queue(f);
        let next = queue(f);
        assert!(old.request.cancel.load(Ordering::Acquire));
        assert_ne!(old.request.generation, next.request.generation);
        let target = f.app.ott.as_ref().unwrap().play_target.clone();
        let invocation = f.app.ott.as_ref().unwrap().pending_play.as_ref().unwrap().id.clone();
        deliver(f, old, 0);
        let runtime = f.app.ott.as_ref().unwrap();
        assert_eq!(runtime.play_target, target);
        assert_eq!(runtime.play_generation, next.request.generation);
        assert_eq!(runtime.pending_play.as_ref().unwrap().id, invocation);
        assert!(!next.request.cancel.load(Ordering::Acquire));
        deliver(f, next, 1);
        let runtime = f.app.ott.as_ref().unwrap();
        assert!(runtime.pending_play.is_none() && runtime.play_target.is_none());
        assert!(runtime.resolver_next.is_none());
    }
}
