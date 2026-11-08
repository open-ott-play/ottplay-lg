//! Cross-provider regression at the second (native Plex) completion boundary.
//!
//! This injects the native terminal verdict after the OTT wrapper has admitted
//! Plex playback. It is not a network/PlayLanding end-to-end test. Unlike the
//! continuity unit tests it runs the real application tick and requires it to
//! turn the native failure into the next provider's queued resolver request.
use super::*;

fn profile(id: &str, name: &str, kind: ProviderKind, source: &str) -> ProviderProfile {
    ProviderProfile {
        id: id.into(),
        name: name.into(),
        kind,
        source: source.into(),
        source_alias: name.into(),
        enabled: true,
    }
}

fn row(id: &str, title: &str, media: MediaLocator) -> CatalogEntry {
    CatalogEntry {
        id: id.into(),
        title: title.into(),
        category: String::new(),
        description: String::new(),
        poster: String::new(),
        media,
        duration_ms: Some(60_000),
        provider_id: None,
        provider_id_field: None,
        live: None,
    }
}

#[test]
fn injected_plex_terminal_refusal_after_wrapper_admission_queues_local_favorite() {
    let _serial = plx_base::testlock::serial();
    struct RegistryCleanup;
    impl Drop for RegistryCleanup {
        fn drop(&mut self) {
            plx_media::player::SHARED.reset_session();
            plx_media::player::TX.reset();
            plx_media::route::reset_player_control_for_test(
                &plx_media::route::PlaybackSession::IDLE,
            );
            plx_plex::plex::reset_servers_for_test();
        }
    }
    let _registry_cleanup = RegistryCleanup;
    plx_plex::plex::reset_servers_for_test();
    let sid = plx_plex::plex::register_for_test(
        "fixture-machine",
        "127.0.0.1",
        1,
        "fixture-token",
        "fixture-client",
    );
    plx_plex::plex::client_for(sid).unwrap().disable_data_io();
    let mut app = ready_app_fixture();
    app.running = true;
    let mut runtime = app.ott.take().unwrap();
    assert!(runtime.play_work_allowed(), "the actual foreground tick admitted source work");
    let directory = plx_base::testscratch::process_dir("plex-failure-local-successor")
        .canonicalize().expect("owned Local fixture directory");
    let plex = profile(
        "plex-fixture",
        "Plex",
        ProviderKind::Plex,
        "fixture-machine",
    );
    let local = profile(
        "local-fixture",
        "Local",
        ProviderKind::Local,
        directory.to_str().unwrap(),
    );
    let source = crate::app::ott_local_root::LocalSource::acquire_authorized(
        &plx_base::task::OffFrame::for_test(),
        crate::app::ott_local_session::BoundSource {
            id: "plex-successor-root".into(), name: "Local".into(),
            kind: crate::app::ott_local_session::SourceKind::Folder,
            available: true, authority_ref: crate::app::ott_local_session::AuthorityRef(7),
            generation: 7, canonical_path: Some(local.source.clone()),
        }, &mut || true,
    ).expect("owned Local directory grant");
    assert!(source.is_live());
    runtime.generation = 7;
    runtime.continuity.rebase(runtime.generation);
    runtime.local_inventory.install(vec![source], runtime.generation);
    let plex_row = row(
        "plex-fixture:42",
        "Plex film",
        MediaLocator::Plex {
            server_id: plex.source.clone(),
            rating_key: "42".into(),
        },
    );
    let local_row = row(
        "local-fixture:next",
        "Local film",
        MediaLocator::Local {
            relative_path: "next.mp4".into(),
        },
    );
    let plex_favorite = selection::capture(&plex, &plex_row).unwrap();
    let local_favorite = selection::capture(&local, &local_row).unwrap();
    let plex_selection = format!("favorite:{}", plex_favorite.identity());
    let local_selection = format!("favorite:{}", local_favorite.identity());
    runtime.saved.profiles.profiles = vec![plex.clone(), local.clone()];
    let local_binding = runtime.local_profile_selection(&local).expect("successor profile binds to retained root");
    assert!(runtime.capture_local_source(&local_binding).is_some_and(|read| read.is_live()));
    runtime.saved.profiles.active_profile = Some(plex.id.clone());
    runtime.saved.favorites.toggle(plex_favorite).unwrap();
    runtime.saved.favorites.toggle(local_favorite).unwrap();
    // A receiving player's explicit local capture proves a root-level basename.
    // The successor still passes through the ordinary Favorite binder.
    let bindings = selection::SelectionBindings::profiles_only(&runtime.saved.profiles);
    runtime.saved.local_favorite_proofs.record_capture(
        &bindings, &runtime.saved.favorites, selection::SelectionOwner::Profile(&local),
        &[local_row.clone()], &local_row,
    ).unwrap();
    assert!(runtime.saved.local_favorite_proofs.authorize(
        &bindings, &runtime.saved.favorites,
        runtime.saved.favorites.entries().iter().find(|favorite|
            format!("favorite:{}", favorite.identity()) == local_selection).unwrap(), &local,
    ).unwrap().is_some());
    runtime.saved.favorites_selected = true;
    runtime.provider = ProviderKind::Plex;
    runtime.page = "favorites";
    runtime.catalog_loading = false;
    runtime.catalog_dirty = false;
    runtime.selected = Some(plex_selection.clone());

    // State committed by start_resolved(ReadySource::Plex), after Job::Play
    // consumes play_target. The inner native resolve has not succeeded yet.
    runtime
        .continuity
        .user_selected(runtime.generation, &plex_selection);
    assert!(runtime.continuity.accepts_end(runtime.generation, &plex_selection),
        "the admitted Plex item must own continuation in the current catalogue generation");
    runtime.playing = Some(Playing { channel_capture: None,
        kiosk_selection: None,
        archive: None,
        transient_owner: None,
        local_selection: None,
        identity: plx_media::route::playback_identity(&app.player.session),
        profile: plex,
        entry: plex_row,
        selection_id: plex_selection,
        source: None,
        advance_on_failure: true,
        requires_server_lease: false,
    });
    runtime.play_target = None;
    runtime.pending_play = None;
    runtime.resolver_next = None;
    // Hold the outgoing test resolver boundary. A queued next-file request is
    // observable without reading a media file or starting any network job.
    runtime.resolver_busy = true;
    app.ott = Some(runtime);

    plx_media::route::install_plex_identity_for_test(&mut app.player.session, sid, "42");
    assert!(
        playing_matches(app.ott.as_ref().unwrap().playing.as_ref().unwrap(), &app),
        "premise: the failed native source belongs to the committed OTT Plex item"
    );
    plx_media::route::refuse_for_test(
        &mut app.player.session,
        plx_media::route::PlayVerdict::DirectPlayDisabled,
    );
    assert_eq!(
        plx_media::player::state(&app.player.session),
        plx_media::player::PlaybackState::Error,
        "premise: the native stage failed after OTT admitted the Plex item"
    );

    runtime_tick(&mut app);

    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(
        runtime.play_target.as_ref().map(|target| target.id.as_str()),
        Some(local_selection.as_str()),
        "a native Plex refusal must follow the same skip policy as an outer resolver failure"
    );
    assert!(
        runtime.resolver_next.as_ref().is_some_and(|next| {
            next.profile.kind == ProviderKind::Local
                && next.local_read.as_ref().is_some_and(|read|
                    read.is_live() && read.selection == local_binding)
                && next.favorite.as_ref().is_some_and(|favorite| {
                    format!("favorite:{}", favorite.identity()) == local_selection
                })
        }),
        "the next request must be rebound through Local, not replay the failed Plex item"
    );

    // Do not let the deliberately injected verdict affect another test.
    plx_media::route::cancel_play(&mut app.player.session);
}
