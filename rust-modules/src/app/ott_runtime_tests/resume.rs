//! Cross-boundary regressions: the real Player Exit must retire the OTT intent
//! before decoder teardown loses its playhead. Only an owned scratch directory
//! is opened for the actual Local grant; no network or private storage is used,
//! and the resolver/persistence slots are deliberately held busy.
use super::*;

#[path = "supersession.rs"]
mod supersession;

#[path = "transport.rs"]
mod transport;

struct ResetMedia;
impl Drop for ResetMedia {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}

fn local_entry(id: &str) -> CatalogEntry {
    CatalogEntry { protection: None,
        synopsis: None,
        id: id.into(),
        title: format!("{id}.mp4"),
        category: "Fixture folder".into(),
        description: String::new(),
        poster: String::new(),
        media: MediaLocator::Local {
            relative_path: format!("{id}.mp4"),
        },
        duration_ms: Some(60_000),
        provider_id: None,
        provider_id_field: None,
        live: None,
    }
}

fn local_playing(app: &mut App, restore: Option<continuity::ResumePoint>, position_ms: u64) {
    // The Local admission path now requires a completed inventory and an actual
    // retained directory grant. Keep this fixture's resolver inert as before;
    // acquiring its own empty directory does not inspect or play a media file.
    let directory = plx_base::testscratch::process_dir("ott-runtime-resume-local")
        .canonicalize()
        .expect("owned Local fixture directory");
    let root = crate::app::ott_local_session::BoundSource {
        id: "local-exit-root".into(),
        name: "Fixture folder".into(),
        kind: crate::app::ott_local_session::SourceKind::Folder,
        available: true,
        authority_ref: crate::app::ott_local_session::AuthorityRef(7),
        generation: 7,
        canonical_path: Some(directory.to_str().unwrap().into()),
    };
    let local_source = crate::app::ott_local_root::LocalSource::acquire_authorized(
        &plx_base::task::OffFrame::for_test(),
        root,
        &mut || true,
    )
    .expect("owned Local fixture grant");
    assert!(local_source.is_live());
    let profile = ProviderProfile {
        id: "local-exit-fixture".into(),
        name: "Fixture folder".into(),
        kind: ProviderKind::Local,
        source: directory.to_str().unwrap().into(),
        source_alias: "Fixture folder".into(),
        enabled: true,
    };
    let first = local_entry("first");
    let source = plx_media::ff::ExternalSource::Local(directory.join("first.mp4"));
    plx_media::route::reset_player_control_for_test(&app.player.session);
    plx_media::route::request_external_source(
        &mut app.player.session,
        &plx_media::ff::inspect::MediaInfo {
            tracks: Default::default(),
            resolved_source: None,
            hls_playback: None,
            video_codec: "h264".into(),
            audio_codec: "aac".into(),
            width: 1280,
            height: 720,
            fps: 24.0,
            duration_ns: 60_000_000_000,
            audio_channels: 2,
            audio_sample_rate: 48_000,
            audio_bit_depth: 16,
            dovi: plx_data::metadata::Dovi::NONE,
        },
        source.clone(),
        &first.title,
        &first.category,
        plx_platform::devcaps::assumed_shared(),
    )
    .expect("the inert local route must be admitted before checking the Exit boundary");
    {
        let runtime = app.ott.as_mut().unwrap();
        runtime.saved.profiles.profiles = vec![profile.clone()];
        runtime.saved.profiles.active_profile = Some(profile.id.clone());
        runtime.provider = ProviderKind::Local;
        runtime.catalog.entries = vec![first.clone()];
        runtime.catalog_loading = true;
        runtime.catalog_dirty = false;
        runtime.generation = 7;
        runtime.local_inventory.install(vec![local_source], 7);
        let selected = runtime.local_profile_selection(&profile)
            .expect("durable Local profile must bind to the fixture root");
        assert!(runtime.capture_local_source(&selected).is_some(),
            "the completed inventory must retain a live Local read grant");
        runtime.continuity = continuity::Continuity::new(7, restore);
        assert!(matches!(
            runtime.continuity.catalog_updated(7, &["first"], false),
            continuity::Decision::Start {
                reason: continuity::Reason::First,
                ..
            }
        ));
        runtime.playing = Some(Playing { channel_capture: None,
            kiosk_selection: None,
        archive: None,
        transient_owner: None,
        local_selection: None,
            identity: plx_media::route::playback_identity(&app.player.session),
            profile,
            entry: first,
            selection_id: "first".into(),
            source: Some(source),
            advance_on_failure: true,
            requires_server_lease: false,
        });
        runtime.selected = Some("first".into());
        runtime.resolver_busy = true;
        runtime.saving = true;
    }
    mount_player(app);
    plx_media::player::SHARED.playpos_ns.store(
        (position_ms * 1_000_000) as i64,
        std::sync::atomic::Ordering::Relaxed,
    );
    assert!(playing_matches(
        app.ott.as_ref().unwrap().playing.as_ref().unwrap(),
        app
    ));
}

#[test]
fn local_fallback_exit_does_not_reopen_when_the_saved_file_arrives() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = ready_app_fixture();
    local_playing(
        &mut app,
        Some(continuity::ResumePoint {
            entry_id: "late".into(),
            position_ms: 42_000,
        }),
        8_000,
    );

    exit_through_content(&mut app);
    // The producer had already queued this old-generation discovery when Exit
    // was pressed. Draining it must not stage a new media resolution.
    app.ott
        .as_ref()
        .unwrap()
        .catalog_tx
        .send(CatalogEvent::Batch(
            7,
            Catalog { hierarchy: None,
                entries: vec![local_entry("late")],
            },
        ))
        .unwrap_or_else(|_| panic!("fixture catalogue receiver disappeared"));
    runtime_tick(&mut app);

    let runtime = app.ott.as_ref().unwrap();
    assert!(
        runtime.resolver_next.is_none(),
        "Exit was bypassed by a late restore resolver"
    );
    assert!(
        runtime.play_target.is_none(),
        "Exit was bypassed by a late saved-file target"
    );
    assert!(
        !matches!(app.route(), AppArg::Player),
        "the player must remain closed"
    );
}

#[test]
fn local_exit_checkpoints_the_nonzero_playhead_before_decoder_teardown() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = ready_app_fixture();
    local_playing(&mut app, None, 23_700);
    let runtime = app.ott.as_mut().unwrap();
    let profile = runtime.active().unwrap().clone();
    runtime.checkpoints.insert(
        profile.id.clone(),
        SavedResume {
            entry_id: "first".into(),
            position_ms: 9_000,
            source_fingerprint: profile_fingerprint(&profile),
        },
    );

    exit_through_content(&mut app);
    assert_eq!(
        plx_media::player::playpos_ns(),
        0,
        "the real teardown must have reset its clock"
    );
    runtime_tick(&mut app);

    let checkpoint = app
        .ott
        .as_ref()
        .unwrap()
        .checkpoints
        .get(&profile.id)
        .unwrap();
    assert_eq!(checkpoint.entry_id, "first");
    assert_eq!(
        checkpoint.position_ms, 23_700,
        "the post-teardown zero must not replace the viewer's last position"
    );
}

#[test]
fn partial_catalogue_failure_does_not_grant_eos_wrap_or_hide_the_error() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut app = ready_app_fixture();
    local_playing(&mut app, None, 60_000);
    const FAILURE: &str = "Fixture subtree is unavailable";
    app.ott
        .as_ref()
        .unwrap()
        .catalog_tx
        .send(CatalogEvent::Finished(7, Err(FAILURE)))
        .unwrap_or_else(|_| panic!("fixture catalogue receiver disappeared"));
    runtime_tick(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().status, FAILURE);

    // This is the real application continuation called by playback_tick on
    // native EOS; the narrow fixture checks completion admission, not decoding.
    assert!(finish_catalog_playback(&mut app));

    let runtime = app.ott.as_ref().unwrap();
    assert!(
        runtime.resolver_next.is_none(),
        "a failed scan must not wrap a partial catalogue"
    );
    assert!(
        runtime.play_target.is_none(),
        "EOS must wait after an incomplete scan"
    );
    assert_eq!(
        runtime.status, FAILURE,
        "EOS must not replace the scan failure with an opening message"
    );
}


#[path = "resume_selection.rs"]
mod selection_checkpoint;
