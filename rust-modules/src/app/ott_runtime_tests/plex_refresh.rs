//! A verified background source refresh updates listings, never playback intent.
use super::*;
use crate::app::ott_import;

fn plex_row(id: &str) -> CatalogEntry {
    let mut row = local_entry(id);
    row.media = MediaLocator::Plex {
        server_id: "refresh-fixture".into(),
        rating_key: id.into(),
    };
    row
}

#[derive(Clone, Copy)]
enum Intent {
    Exit,
    ExitWithFailedStartupCatalog,
    Play,
    Back,
}

fn plex_refresh_after_intent(intent: Intent) {
    let _serial = plx_base::testlock::serial();
    struct ResetPlex;
    impl Drop for ResetPlex {
        fn drop(&mut self) {
            let _ = ott_import::set_authorized_sources(&[]);
            plx_plex::plex::reset_servers_for_test();
            plx_plex::plex::grant::reset_for_test();
        }
    }
    let _plex = ResetPlex;
    let _media = ResetMedia;
    plx_plex::plex::grant::reset_for_test();
    plx_plex::plex::reset_servers_for_test();
    ott_import::set_authorized_sources(&[]).unwrap();
    // Complete the empty Saved load before minting the import proof: that load
    // installs its own authorized source list and would revoke an earlier proof.
    let mut app = ready_app_fixture();
    let source = plx_plex::plex::session::SourceRef {
        machine_id: "refresh-fixture".into(),
        address: "127.0.0.1".into(),
        port: 1,
        origin_url: "http://127.0.0.1:1".into(),
        token: "fixture-token".into(),
        ..Default::default()
    };
    let sid = plx_plex::plex::register_for_test(
        &source.machine_id,
        &source.address,
        1,
        &source.token,
        "fixture-client",
    );
    plx_plex::plex::client_for(sid).unwrap().disable_data_io();
    // Use the real authority check and matching synthetic registered server.
    // The test proof only replaces the successful network-verification result.
    ott_import::set_authorized_sources(std::slice::from_ref(&source)).unwrap();
    let verified = ott_import::verified_source_for_test(source.clone());
    let profile = ProviderProfile {
        id: "plex-refresh-fixture".into(),
        name: "Plex fixture".into(),
        kind: ProviderKind::Plex,
        source: source.machine_id.clone(),
        source_alias: String::new(),
        enabled: true,
    };
    mount_player(&mut app);
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.profiles.profiles = vec![profile.clone()];
        runtime.saved.profiles.active_profile = Some(profile.id.clone());
        runtime
            .saved
            .settings
            .insert("nativePlexSources".into(), json!([source]));
        runtime.provider = ProviderKind::Plex;
        let startup_loading = matches!(intent, Intent::ExitWithFailedStartupCatalog);
        runtime.catalog.entries = if startup_loading {
            Vec::new()
        } else {
            vec![plex_row("first"), plex_row("second")]
        };
        runtime.catalog_loading = startup_loading;
        runtime.catalog_complete = !startup_loading;
        runtime.catalog_dirty = false;
        runtime.catalog_done.store(false, Ordering::Release);
        runtime.generation = 7;
        runtime.continuity = continuity::Continuity::new(7, None);
        runtime.continuity.user_selected(7, "first");
        runtime.playing = Some(Playing { channel_capture: None,
            kiosk_selection: None,
        archive: None,
        transient_owner: None,
        local_selection: None,
            identity: plx_media::route::playback_identity(&app.player.session),
            profile,
            entry: plex_row("first"),
            selection_id: "first".into(),
            source: None,
            advance_on_failure: true,
            requires_server_lease: false,
        });
        runtime.selected = Some("first".into());
        runtime.resolver_busy = true;
        runtime.saving = true;
        runtime.plex_refresh_pending = true;
    }
    let restore_cancel = app.ott.as_mut().unwrap().begin_plex_restore();
    plx_media::route::install_plex_identity_for_test(&mut app.player.session, sid, "first");
    plx_media::route::install_plex_request_for_test(&mut app.player.session, sid, "first");
    match intent {
        Intent::Exit | Intent::ExitWithFailedStartupCatalog => {
            exit_through_content(&mut app);
            runtime_tick(&mut app);
            assert!(app.player.pending_exit.is_none());
            assert!(app.ott.as_ref().unwrap().playing.is_none());
        }
        Intent::Play => explicit_play(&mut app, "second"),
        Intent::Back => {
            // The provider list is one Back above profiles. Channels has its
            // own intermediate profiles page; this case tests leaving that page.
            app.ott.as_mut().unwrap().page = "profiles";
            bridge::show_page(&mut app.pages, AppArg::Ott);
            settle_pages(&mut app);
            runtime_tick(&mut app);
            settle_pages(&mut app);
            assert_eq!(app.ott.as_ref().unwrap().page, "profiles");
            let tick = next_tick(&mut app);
            bridge::frame(
                &mut app.pages,
                &mut app.bridge,
                tick,
                bridge::script_key(plx_machine::machine::Key::Back, tick),
            );
            runtime_tick(&mut app);
            settle_pages(&mut app);
            assert_eq!(app.ott.as_ref().unwrap().page, "providers");
            assert!(matches!(app.route(), AppArg::Ott));
        }
    }
    let play_generation = app.ott.as_ref().unwrap().play_generation;
    let catalog_generation = app.ott.as_ref().unwrap().generation;
    if matches!(intent, Intent::ExitWithFailedStartupCatalog) {
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.catalog_loading);
        assert!(!runtime.catalog_done.load(Ordering::Acquire));
        // Both completions are ready before the same frame. The runtime drains
        // PlexRestored first, while the obsolete startup scan still says loading.
        runtime
            .catalog_tx
            .send(CatalogEvent::Finished(
                catalog_generation,
                Err("Plex server is not connected"),
            ))
            .unwrap_or_else(|_| panic!("fixture catalogue receiver disappeared"));
    }
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::PlexRestored(restore_cancel, vec![Ok(verified)]))
        .unwrap_or_else(|_| panic!("fixture restore receiver disappeared"));
    runtime_tick(&mut app);
    let runtime = app.ott.as_ref().unwrap();
    assert!(!runtime.plex_refresh_pending);
    if matches!(intent, Intent::ExitWithFailedStartupCatalog) {
        assert_ne!(
            runtime.generation, catalog_generation,
            "verified Plex restoration must supersede a failed startup scan in the same frame"
        );
        assert!(
            runtime
                .catalog_next
                .as_ref()
                .is_some_and(|profile| profile.id == "plex-refresh-fixture"),
            "the replacement catalogue must remain queued while the old worker is held"
        );
    }
    assert!(
        runtime.catalog_loading,
        "verified source must refresh its catalogue"
    );
    if matches!(intent, Intent::Back) {
        assert_eq!(
            runtime.page, "providers",
            "a background source refresh must not navigate away from the user's page"
        );
    }
    runtime
        .catalog_tx
        .send(CatalogEvent::Batch(
            runtime.generation,
            Catalog { hierarchy: None,
                entries: vec![plex_row("refreshed-first")],
            },
        ))
        .unwrap_or_else(|_| panic!("fixture catalogue receiver disappeared"));
    runtime_tick(&mut app);

    let runtime = app.ott.as_ref().unwrap();
    assert_eq!(
        runtime.play_generation, play_generation,
        "background Plex refresh must not create a replacement playback request"
    );
    if matches!(
        intent,
        Intent::Exit | Intent::ExitWithFailedStartupCatalog | Intent::Back
    ) {
        assert!(
            runtime.play_target.is_none() && runtime.resolver_next.is_none(),
            "Plex restoration must preserve an explicit Exit or Back"
        );
        assert!(!matches!(app.route(), AppArg::Player));
        if matches!(intent, Intent::Back) {
            assert_eq!(runtime.page, "providers");
        }
    } else {
        assert_eq!(runtime.play_target, Some(PlaybackRequest::ordinary("second", 0)));
        assert!(
            runtime.resolver_next.as_ref().is_some_and(|next| next
                .entry
                .as_ref()
                .is_some_and(|entry| entry.id == "second")),
            "Plex restoration must preserve the newer user selection"
        );
    }
}

#[test]
fn plex_restoration_after_exit_does_not_reenable_autoplay() {
    plex_refresh_after_intent(Intent::Exit);
}

#[test]
fn plex_restoration_preserves_a_newer_play_request() {
    plex_refresh_after_intent(Intent::Play);
}

#[test]
fn plex_restoration_after_back_preserves_providers_page_without_autoplay() {
    plex_refresh_after_intent(Intent::Back);
}

#[test]
fn plex_restoration_retries_a_same_frame_startup_failure_without_reenabling_autoplay() {
    plex_refresh_after_intent(Intent::ExitWithFailedStartupCatalog);
}
