use super::*;
fn fixture() -> (Runtime, Favorite) {
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.loaded = true;
    let profile = ProviderProfile {
        id: "m3u-fixture".into(),
        name: "Fixture".into(),
        kind: ProviderKind::M3u,
        source: "https://media.invalid/fixture.m3u".into(),
        source_alias: String::new(),
        enabled: true,
    };
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id);
    runtime.provider = ProviderKind::M3u;
    let favorite = Favorite {
        title: "Pinned channel".into(),
        origin: Origin {
            provider: "m3u".into(),
            profile: "Fixture".into(),
            source_alias: "Fixture".into(),
            subprofile: None,
        },
        locator: Locator::Channel {
            channel_id: "fixture-channel".into(),
        },
    };
    runtime.saved.settings.insert(
        stored::KEY.into(),
        Stored::item(favorite.clone(), true)
            .unwrap()
            .encode()
            .unwrap(),
    );
    (runtime, favorite)
}
#[test]
fn typed_item_rebinds_after_global_favorite_removal_without_inserting_one() {
    let (mut runtime, favorite) = fixture();
    runtime.saved.favorites.toggle(favorite.clone()).unwrap();
    let prepared = prepare(&runtime).unwrap();
    let Activation::Pinned(selection) = prepared.activation else {
        panic!("typed item")
    };
    runtime.saved.favorites.toggle(favorite.clone()).unwrap();
    let request = selection.request();
    let (_, profile, entry, bound) = runtime.prepare_play_request(request).unwrap();
    assert_eq!(profile.name, "Fixture");
    assert!(entry.is_none());
    assert_eq!(bound, Some(favorite));
    assert!(runtime.saved.favorites.entries().is_empty());
    assert!(!runtime.saving);
    assert!(runtime.resolver_next.is_none());
}
#[test]
fn provider_controlled_collision_remains_an_ordinary_catalog_item() {
    let (mut runtime, _) = fixture();
    let prepared = prepare(&runtime).unwrap();
    let Activation::Pinned(selection) = prepared.activation else {
        panic!("typed item")
    };
    let id = selection.request().id;
    runtime.catalog.entries.push(CatalogEntry {
        synopsis: None,
        live: None,
        id: id.clone(),
        title: "Untrusted catalog title".into(),
        category: String::new(),
        description: String::new(),
        poster: String::new(),
        duration_ms: None,
        provider_id: None,
        provider_id_field: None,
        media: MediaLocator::Http {
            url: "https://media.invalid/untrusted".into(),
        },
    });
    let ordinary = PlaybackRequest::ordinary(&id, 0);
    assert!(bind_request(&runtime, &ordinary).unwrap().is_none());
    let (_, _, entry, favorite) = runtime.prepare_play_request(ordinary).unwrap();
    assert_eq!(entry.unwrap().title, "Untrusted catalog title");
    assert!(favorite.is_none());
}
#[test]
fn policy_or_source_replacement_revokes_a_prepared_pin_before_launch() {
    let (mut runtime, _) = fixture();
    let prepared = prepare(&runtime).unwrap();
    let Activation::Pinned(selection) = prepared.activation else {
        panic!("typed item")
    };
    runtime.saved.settings.remove(stored::KEY);
    assert!(runtime.prepare_play_request(selection.request()).is_err());
    assert!(runtime.resolver_next.is_none());
    assert!(!runtime.saving);
    runtime.saved.settings.insert(
        stored::KEY.into(),
        Stored::item(fixture().1, true).unwrap().encode().unwrap(),
    );
    runtime.saved.profiles.profiles[0].enabled = false;
    assert!(runtime.prepare_play_request(selection.request()).is_err());
}
#[test]
fn source_configuration_change_is_detected_by_prepared_proof() {
    let (mut runtime, _) = fixture();
    let prepared = prepare(&runtime).unwrap();
    let Proof::Pinned(selection, profile, local) = &prepared.proof else {
        panic!("proof")
    };
    assert_eq!(
        bind(&runtime, selection).unwrap(),
        (profile.clone(), local.clone())
    );
    runtime.saved.profiles.profiles[0].source = "https://replacement.invalid/other.m3u".into();
    assert!(
        bind(&runtime, selection).is_err(),
        "a changed source cannot borrow the old pin admission"
    );
}

#[cfg(feature = "hostsim")]
#[test]
fn initial_loaded_stages_typed_item_without_global_favorites_or_io() {
    let _serial = plx_base::testlock::serial();
    let app = crate::app::run::lifecycle_regression_tests::app();
    let (mut runtime, _) = fixture();
    assert!(stage_startup_item(&mut runtime, &app).unwrap());
    let request = runtime.resume_pending.as_ref().unwrap().clone();
    assert!(request.kiosk_selection.is_some());
    assert!(!runtime.saving);
    assert!(runtime.resolver_next.is_none());
    assert!(runtime.saved.favorites.entries().is_empty());
    assert!(runtime.prepare_play_request(request).is_ok());
    assert!(
        !stage_startup_item(&mut runtime, &app).unwrap(),
        "one existing pending owner"
    );
}
#[cfg(feature = "hostsim")]
#[test]
fn newer_navigation_prevents_startup_from_claiming_playback() {
    let _serial = plx_base::testlock::serial();
    let app = crate::app::run::lifecycle_regression_tests::app();
    let (mut runtime, _) = fixture();
    runtime.navigation_epoch = 1;
    assert!(!stage_startup_item(&mut runtime, &app).unwrap());
    assert!(runtime.resume_pending.is_none());
}

#[test]
fn pinned_marker_does_not_bypass_exact_archive_request_validation() {
    use plx_data::ott::{archive::{ArchiveInterval, CatchupMode, CatchupSource}, guide::ChannelMetadata};
    let (runtime, _) = fixture();
    let prepared = prepare(&runtime).unwrap();
    let Activation::Pinned(selection) = prepared.activation else { panic!("typed item") };
    let profile = runtime.active().unwrap();
    let entry = CatalogEntry {
        synopsis: None,
        id: "fixture-channel".into(), title: "Pinned channel".into(), category: String::new(),
        description: String::new(), poster: String::new(), duration_ms: None,
        provider_id: Some("fixture-channel".into()), provider_id_field: Some("tvg-id".into()),
        media: MediaLocator::Http { url: "https://media.invalid/live.m3u8".into() },
        live: Some(ChannelMetadata { tvg_name: String::new(), guide_shift_seconds: 0,
            catchup: Some(CatchupSource { mode: CatchupMode::Append,
                source: "?start={utc}&end={utcend}".into(), retention_seconds: 1000, correction_seconds: 0 }) }),
    };
    let mut request = selection.request();
    request.archive = Some(archive_play::Intent::new(profile, &entry,
        ArchiveInterval { start: 1000, end: 1100 }, 1500).unwrap());
    request.advance_on_failure = true;
    assert_eq!(runtime.prepare_play_request(request.clone()).err(),
        Some("Archive playback must be an exact request"));
    request.advance_on_failure = false;
    request.position_ms = 1000;
    assert_eq!(runtime.prepare_play_request(request).err(),
        Some("Archive playback must be an exact request"));
}
