use super::*;
use plx_data::ott::{
    favorites::{Locator, Origin},
    multiview::{Scene, SceneId, SlotId},
};

fn fixture() -> (App, Runtime) {
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    app.running = true;
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.clock_sample = Some(
        plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt())
            .expect("fixture clock"),
    );
    runtime.remote = Remote::disabled_for_test("dev_mv_sources", "fixture");
    runtime.loaded = true;
    runtime.state_load = StateLoad::Idle;
    runtime.entropy = Some([1; 16]);
    runtime.resume_source_work();
    for n in 1..=2 {
        runtime.saved.profiles.profiles.push(ProviderProfile {
            id: format!("m3u-{n}"),
            name: format!("Profile {n}"),
            kind: ProviderKind::M3u,
            source: format!("https://example.invalid/{n}.m3u"),
            source_alias: format!("Source {n}"),
            enabled: true,
        });
        runtime
            .saved
            .favorites
            .toggle(Favorite {
                title: format!("Channel {n}"),
                origin: Origin {
                    provider: "m3u".into(),
                    profile: format!("Profile {n}"),
                    subprofile: None,
                    source_alias: format!("Source {n}"),
                },
                locator: Locator::ChannelName {
                    name: format!("Channel {n}"),
                    group: Some("Documentary".into()),
                },
            })
            .unwrap();
    }
    runtime.saved.profiles.active_profile = Some("m3u-1".into());
    runtime.provider = ProviderKind::M3u;
    (app, runtime)
}
fn ticket(handle: SourceHandle, slot: SlotId) -> ResolveTicket {
    let mut scene = Scene::new(SceneId::from_opaque(NonZeroU64::new(17).unwrap()));
    scene
        .set_layout(plx_data::ott::multiview::Layout::Grid4)
        .unwrap();
    scene.bind_source(slot, handle.source_ref()).unwrap();
    scene
        .begin_resolution(slot, handle.source_ref(), handle.media_ref())
        .unwrap()
}
fn entry() -> CatalogEntry {
    CatalogEntry {
        id: "resolved-fixture".into(),
        title: "Channel 2".into(),
        category: "Documentary".into(),
        description: String::new(),
        poster: String::new(),
        synopsis: None,
        protection: Some(false),
        media: MediaLocator::Http {
            url: "https://example.invalid/video.m3u8".into(),
        },
        duration_ms: None,
        provider_id: None,
        provider_id_field: None,
        live: None,
    }
}
fn result(work: SourceWork) -> PreparedMetadata {
    let mut metadata = entry();
    if let Locator::ChannelName { name, group } = &work.binding.favorite.locator {
        metadata.title = name.clone();
        metadata.category = group.clone().unwrap_or_default();
    }
    prepared_http_fixture_for_test(work, metadata).unwrap()
}

#[test]
fn inactive_favorite_binding_preserves_saved_global_playback_and_opaque_snapshot_ownership() {
    let _serial = plx_base::testlock::serial();
    let (app, mut runtime) = fixture();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let snapshot = Snapshot::capture(&runtime).unwrap();
    let handle = snapshot.rows().nth(1).unwrap().handle;
    assert_eq!(
        snapshot.rows().nth(1).unwrap().support,
        Support::HttpCandidate
    );
    assert_eq!(snapshot.rows().nth(1).unwrap().title, "Channel 2");
    assert_eq!(snapshot.rows().count(), 2);
    assert!(snapshot.current_for(&runtime));
    let descriptor = snapshot.rows().nth(1).unwrap().descriptor().unwrap();
    assert_eq!(descriptor.provider, "m3u");
    assert_eq!(descriptor.profile, "m3u-2");
    assert_eq!(descriptor.kind, "live");
    assert!(snapshot.contains(&handle));
    let work = snapshot
        .capture_work(&mut runtime, &app, &handle, ticket(handle, SlotId::SLOT_1))
        .unwrap();
    assert_eq!(work.binding.profile.id, "m3u-2");
    assert_eq!(work.ticket, ticket(handle, SlotId::SLOT_1));
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(runtime.play_target.is_none());
    assert!(runtime.resolver_next.is_none());
    assert!(runtime.active_lease.is_none());
    assert!(runtime.selected.is_none());
    let other = Snapshot::capture(&runtime).unwrap();
    assert!(!other.contains(&handle));
    assert_eq!(format!("{handle:?}"), "SourceHandle(<opaque>)");
}

#[test]
fn per_slot_cancellation_and_snapshot_retirement_are_isolated_and_reach_workers() {
    let _serial = plx_base::testlock::serial();
    let (app, mut runtime) = fixture();
    let snapshot = Snapshot::capture(&runtime).unwrap();
    let handle = snapshot.rows().next().unwrap().handle;
    let a = snapshot
        .capture_work(&mut runtime, &app, &handle, ticket(handle, SlotId::SLOT_0))
        .unwrap();
    let b = snapshot
        .capture_work(&mut runtime, &app, &handle, ticket(handle, SlotId::SLOT_1))
        .unwrap();
    let ca = a.cancellation();
    let cb = b.cancellation();
    assert!(!Arc::ptr_eq(&ca, &cb));
    assert!(!Arc::ptr_eq(&ca, &runtime.play_cancel));
    ca.store(true, Ordering::Release);
    assert!(!cb.load(Ordering::Acquire));
    // Actual resolver entry refuses cancellation before any network/source IO.
    assert!(matches!(
        a.prepare_metadata(&plx_base::task::OffFrame::for_test()),
        Err(Error::Cancelled)
    ));
    drop(snapshot);
    assert!(cb.load(Ordering::Acquire));
    assert!(matches!(
        b.prepare_metadata(&plx_base::task::OffFrame::for_test()),
        Err(Error::Cancelled)
    ));
}

#[test]
fn changed_source_removed_favorite_and_a_b_a_epoch_refuse_pending_completion() {
    let _serial = plx_base::testlock::serial();
    for mutation in 0..4 {
        let (app, mut runtime) = fixture();
        let snapshot = Snapshot::capture(&runtime).unwrap();
        let handle = snapshot.rows().nth(1).unwrap().handle;
        let work = snapshot
            .capture_work(&mut runtime, &app, &handle, ticket(handle, SlotId::SLOT_1))
            .unwrap();
        match mutation {
            0 => {
                runtime.saved.profiles.profiles[1].source =
                    "https://example.invalid/changed.m3u".into()
            }
            1 => {
                let f = runtime.saved.favorites.entries()[1].clone();
                runtime.saved.favorites.toggle(f).unwrap();
            }
            2 => runtime.retire_profile_edits(), // Same final profile bytes, different source authority lifetime.
            _ => runtime.generation += 1,
        }
        assert!(matches!(
            result(work).authorize_completion(&mut runtime, &app),
            Err(Error::Stale)
        ));
        assert!(runtime.play_target.is_none());
        assert!(runtime.resolver_next.is_none());
    }
}

#[test]
fn completion_uses_existing_resolved_parental_gate_and_current_snapshot() {
    let _serial = plx_base::testlock::serial();
    let (app, mut runtime) = fixture();
    let snapshot = Snapshot::capture(&runtime).unwrap();
    let handle = snapshot.rows().next().unwrap().handle;
    let work = snapshot
        .capture_work(&mut runtime, &app, &handle, ticket(handle, SlotId::SLOT_0))
        .unwrap();
    let mut resolved = result(work);
    assert_eq!(resolved.ticket(), ticket(handle, SlotId::SLOT_0));
    resolved.entry.category = "Catalog / 18+".into();
    resolved.entry.protection = Some(true);
    assert!(matches!(
        resolved.authorize_completion(&mut runtime, &app),
        Err(Error::Policy)
    ));
    let work = snapshot
        .capture_work(&mut runtime, &app, &handle, ticket(handle, SlotId::SLOT_0))
        .unwrap();
    let input = result(work)
        .authorize_completion(&mut runtime, &app)
        .unwrap();
    assert_eq!(input.ticket, ticket(handle, SlotId::SLOT_0));
    assert_eq!(input.entry.id, "resolved-fixture");
    assert!(input.current_plex_authority());
    assert!(input.current_for(&runtime));
    assert_eq!(input.identity_scheme(), 1);
    assert_eq!(
        input.identity_digest(),
        snapshot.rows().next().unwrap().identity_digest()
    );
    assert!(!input.is_cancelled());
    let active_cancel = input.cancellation();
    snapshot.retire();
    assert!(active_cancel.load(Ordering::Acquire));
    assert!(input.is_cancelled());
    assert!(!input.current_plex_authority());
    assert!(!input.current_for(&runtime));
    assert!(runtime.play_target.is_none());
    assert!(runtime.resolver_next.is_none());
}

#[test]
fn work_admission_bounds_pending_captures_and_rejects_foreign_handles() {
    let _serial = plx_base::testlock::serial();
    let (app, mut runtime) = fixture();
    let snapshot = Snapshot::capture(&runtime).unwrap();
    let handle = snapshot.rows().next().unwrap().handle;
    let foreign = Snapshot::capture(&runtime).unwrap();
    assert!(matches!(
        foreign.capture_work(&mut runtime, &app, &handle, ticket(handle, SlotId::SLOT_0)),
        Err(Error::UnknownSource)
    ));
    let mut work = Vec::new();
    for _ in 0..9 {
        work.push(
            snapshot
                .capture_work(&mut runtime, &app, &handle, ticket(handle, SlotId::SLOT_0))
                .unwrap(),
        );
    }
    assert!(matches!(
        snapshot.capture_work(&mut runtime, &app, &handle, ticket(handle, SlotId::SLOT_0)),
        Err(Error::Exhausted)
    ));
    drop(work);
    assert!(
        snapshot
            .capture_work(&mut runtime, &app, &handle, ticket(handle, SlotId::SLOT_0))
            .is_ok()
    );
}

#[test]
fn plex_and_device_local_are_not_exported_as_http_grants() {
    let _serial = plx_base::testlock::serial();
    let (app, mut runtime) = fixture();
    let profile = &mut runtime.saved.profiles.profiles[0];
    profile.kind = ProviderKind::Plex;
    profile.source = "fixture-server".into();
    let favorite = Favorite {
        title: "Film".into(),
        origin: Origin {
            provider: "plex".into(),
            profile: "Profile 1".into(),
            subprofile: None,
            source_alias: "Source 1".into(),
        },
        locator: Locator::Plex {
            server_id: "fixture-server".into(),
            rating_key: "7".into(),
        },
    };
    runtime.saved.favorites = Default::default();
    runtime.saved.favorites.toggle(favorite).unwrap();
    let snapshot = Snapshot::capture(&runtime).unwrap();
    let row = snapshot.rows().next().unwrap();
    assert_eq!(row.support, Support::PlexBrokerRequired);
    assert!(matches!(
        snapshot.capture_work(
            &mut runtime,
            &app,
            &row.handle,
            ticket(row.handle, SlotId::SLOT_0)
        ),
        Err(Error::Unsupported)
    ));
    // Classify a bound device-local source without touching its filesystem or granting a relay.
    let mut binding = snapshot.rows[0].binding.as_ref().unwrap().clone();
    binding.profile.kind = ProviderKind::Local;
    binding.profile.source = "/private/device/movies".into();
    assert_eq!(binding.support(), Support::LocalRelayRequired);
    assert!(runtime.play_target.is_none());
    assert!(runtime.resolver_next.is_none());
}

#[test]
fn mismatched_media_ticket_cannot_resolve_the_handle_item() {
    let _serial = plx_base::testlock::serial();
    let (app, mut runtime) = fixture();
    let snapshot = Snapshot::capture(&runtime).unwrap();
    let handle = snapshot.rows().next().unwrap().handle;
    let mut scene = Scene::new(SceneId::from_opaque(NonZeroU64::new(90).unwrap()));
    scene
        .bind_source(SlotId::SLOT_0, handle.source_ref())
        .unwrap();
    let wrong = plx_data::ott::multiview::MediaRef::from_opaque(NonZeroU64::new(u64::MAX).unwrap());
    let request = scene
        .begin_resolution(SlotId::SLOT_0, handle.source_ref(), wrong)
        .unwrap();
    assert!(matches!(
        snapshot.capture_work(&mut runtime, &app, &handle, request),
        Err(Error::UnknownSource)
    ));
}

#[test]
fn opaque_counter_is_checked_and_never_wraps_or_mints_zero() {
    let counter = AtomicU64::new(u64::MAX - 1);
    assert_eq!(next_id(&counter).unwrap().get(), u64::MAX - 1);
    assert_eq!(next_id(&counter), Err(Error::Exhausted));
    assert_eq!(counter.load(Ordering::Acquire), u64::MAX);
    assert_eq!(next_id(&AtomicU64::new(0)), Err(Error::Exhausted));
}

#[test]
fn accepted_favorite_a_removed_a_cannot_revive_snapshot_work_or_active_authority() {
    let _serial = plx_base::testlock::serial();
    let (app, mut runtime) = fixture();
    let snapshot = Snapshot::capture(&runtime).unwrap();
    let handle = snapshot.rows().next().unwrap().handle;
    let work = snapshot
        .capture_work(&mut runtime, &app, &handle, ticket(handle, SlotId::SLOT_0))
        .unwrap();
    let active_work = snapshot
        .capture_work(&mut runtime, &app, &handle, ticket(handle, SlotId::SLOT_1))
        .unwrap();
    let active = result(active_work)
        .authorize_completion(&mut runtime, &app)
        .unwrap();
    let initial_profile_epoch = runtime.profile_edit_epoch;
    let initial_authority_epoch = runtime.multiview_authority_epoch;
    let original = runtime.saved.clone();
    let mut removed = original.clone();
    removed
        .favorites
        .toggle(original.favorites.entries()[0].clone())
        .unwrap();
    // The real accepted-image observer must see both transitions before each Saved install.
    runtime.observe_profile_edits(&removed);
    runtime.saved = removed;
    runtime.observe_profile_edits(&original);
    runtime.saved = original;
    assert_eq!(runtime.profile_edit_epoch, initial_profile_epoch);
    assert!(runtime.multiview_authority_epoch > initial_authority_epoch);
    assert!(!snapshot.current_for(&runtime));
    assert!(!active.current_for(&runtime));
    assert!(matches!(
        result(work).authorize_completion(&mut runtime, &app),
        Err(Error::Stale)
    ));
    let fresh = Snapshot::capture(&runtime).unwrap();
    assert!(fresh.current_for(&runtime));
    assert!(!snapshot.current_for(&runtime));
    assert!(runtime.play_target.is_none());
    assert!(runtime.resolver_next.is_none());
}
