//! Actual fullscreen admission and sole-writer settlement. No renderer, decoder,
//! network or durable storage runs. Revoked-writer outcomes are consumed before
//! injecting a Saved completion, so these tests do not claim disk durability.
use super::*;
use crate::app::ott_runtime::playing_favorite as action;

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}

fn ready(
    local: bool,
) -> (
    local_integration::TempDir,
    Box<App>,
    crate::app::ott_local_session::BoundSource,
) {
    let (dir, mut app, root) = local_integration::setup();
    // The shared lifecycle fixture is inert by default. These admission tests
    // model a running application with an already-mounted video page.
    app.running = true;
    mount_player(&mut app);
    if local {
        install_local(&mut app, &root);
    } else {
        local_integration::install(&mut app, None, "playing-selection");
        let runtime = app.ott.as_mut().unwrap();
        let playing = runtime.playing.as_ref().unwrap();
        let complete_source = plx_data::ott::Catalog { hierarchy: None, entries: vec![playing.entry.clone()] };
        let bindings = selection::SelectionBindings::profiles_only(&runtime.saved.profiles);
        let proof = selection::capture_channel(&bindings, selection::SelectionOwner::Profile(&playing.profile),
            &complete_source.entries, &playing.entry).unwrap();
        runtime.playing.as_mut().unwrap().channel_capture = proof;
    }
    let r = app.ott.as_mut().unwrap();
    r.entropy = Some([1; 16]);
    r.write_permit.revoke();
    r.saving = false;
    r.plex_refresh_pending = true;
    r.continuity.stop(r.generation);
    assert!(app.running && playback_allowed(&app));
    assert!(matches!(app.route(), AppArg::Player));
    assert!(!app.pages.has_pending_navigation() && !app.pages.surface_up());
    assert!(playing_matches(
        app.ott.as_ref().unwrap().playing.as_ref().unwrap(),
        &app
    ));
    (dir, app, root)
}

/// Use the production no-follow open and retained media input, not a Local-path
/// or HTTP stand-in. The native route is admitted but no decoder is started.
fn install_local(app: &mut App, root: &crate::app::ott_local_session::BoundSource) {
    use crate::app::ott_local_session::SourceKind;
    if root.kind == SourceKind::Folder {
        std::fs::write(
            std::path::Path::new(root.canonical_path.as_deref().unwrap()).join("first.mp4"),
            b"local favorite fixture",
        )
        .unwrap();
    }
    let profile = transient::source_profile(root).unwrap();
    let mut entry = synthetic_entry();
    entry.id = "first".into();
    entry.title = "first".into();
    entry.category = "Movies".into();
    entry.media = MediaLocator::Local {
        relative_path: "first.mp4".into(),
    };
    let r = app.ott.as_ref().unwrap();
    let selection = r.bind_local_selection(root).unwrap();
    let read = r.capture_local_source(&selection).unwrap();
    let source = transient::open_video_source(
        &plx_base::task::OffFrame::for_test(),
        Some(&read),
        &profile,
        &entry,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(matches!(&source, plx_media::ff::ExternalSource::HeldLocal(held) if held.is_live()));
    let mut info = unsupported_info();
    info.video_codec = "h264".into();
    info.audio_codec = "aac".into();
    info.audio_channels = 2;
    info.audio_sample_rate = 48_000;
    info.audio_bit_depth = 16;
    // Installing an accepted source retains a mounted Player and its origin.
    // Exit's delayed PopTo is not part of this fixture transition.
    playback::replace_player_in_place(
        &mut app.player,
        &mut app.adapters.player,
        &mut app.repause_at,
        &mut app.pages,
    );
    plx_media::route::reset_player_control_for_test(&app.player.session);
    plx_media::route::request_external_source(
        &mut app.player.session,
        &info,
        source.clone(),
        "local-playing",
        "Movies",
        plx_platform::devcaps::assumed_shared(),
    )
    .unwrap();
    let identity = plx_media::route::playback_identity(&app.player.session);
    assert!(identity.is_some());
    assert_eq!(
        plx_media::route::external_source(&app.player.session),
        Some(source.clone())
    );
    let r = app.ott.as_mut().unwrap();
    r.playing = Some(Playing { channel_capture: None,
        kiosk_selection: None,
        archive: None,
        identity,
        profile,
        entry,
        selection_id: "local-playing".into(),
        source: Some(source),
        advance_on_failure: false,
        requires_server_lease: false,
        transient_owner: r
            .transient
            .accepted
            .as_ref()
            .map(transient::TransientMedia::owner),
        local_selection: Some(selection),
    });
}

fn ready_file() -> (
    local_integration::TempDir,
    Box<App>,
    crate::app::ott_local_session::BoundSource,
) {
    use crate::app::ott_local_session::SourceKind;
    let (dir, mut app, mut root) = ready(false);
    let path = std::path::Path::new(root.canonical_path.as_deref().unwrap()).join("first.mp4");
    std::fs::write(&path, b"single file favorite fixture").unwrap();
    root.kind = SourceKind::File;
    root.name = "first.mp4".into();
    root.canonical_path = Some(path.to_str().unwrap().into());
    let source = crate::app::ott_local_root::LocalSource::acquire_authorized(
        &plx_base::task::OffFrame::for_test(),
        root.clone(),
        &mut || true,
    )
    .unwrap();
    let r = app.ott.as_mut().unwrap();
    r.local_inventory.install(vec![source], 7);
    r.transient.accepted = None;
    install_local(&mut app, &root);
    (dir, app, root)
}

fn invoke(app: &mut App) -> Result<(), &'static str> {
    let mut runtime = app.ott.take().unwrap();
    let result = action::toggle(&mut runtime, app);
    app.ott = Some(runtime);
    result
}

fn stage(app: &mut App) -> Saved {
    let r = app.ott.as_ref().unwrap();
    let (candidate, _, _) = action::candidate(r, r.playing.as_ref().unwrap()).unwrap();
    let old = r.saved.favorites.clone();
    invoke(app).unwrap();
    let r = app.ott.as_mut().unwrap();
    assert!(r.saving && r.pending_playing_favorite.is_some());
    assert_eq!(r.saved.favorites, old, "no optimistic favorite publication");
    assert!(matches!(r.save_navigation, SaveNavigation::Preserve));
    assert!(
        matches!(
            r.rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Job::Saved(Err(_), None, false, _)
        ),
        "revoked writer must refuse before storage"
    );
    candidate
}

fn deliver(app: &mut App, result: Result<Saved, &'static str>) {
    app.ott
        .as_ref()
        .unwrap()
        .tx
        .send(Job::Saved(result, None, false, json!({"status":"ok"})))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(app);
}

#[test]
fn fullscreen_uses_admitted_m3u_item_not_selected_local_browse_row() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, _root) = ready(false);
    let r = app.ott.as_mut().unwrap();
    r.selected = Some("first".into());
    assert!(matches!(
        r.catalog.entries[0].media,
        MediaLocator::Local { .. }
    ));
    let profiles = r.saved.profiles.clone();
    let (candidate, _, added) = action::candidate(r, r.playing.as_ref().unwrap()).unwrap();
    assert!(added);
    assert_eq!(candidate.favorites.entries()[0].title, "live");
    assert_eq!(candidate.favorites.entries()[0].origin.provider, "m3u");
    assert_eq!(candidate.favorites.entries()[0].origin.profile, "Permanent");
    assert_eq!(candidate.profiles, profiles);
    assert!(r.saved.favorites.entries().is_empty());
}

#[test]
fn local_toggle_retains_global_other_provider_entries_and_captures_exact_root_proof() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, root) = ready(false);
    let r = app.ott.as_mut().unwrap();
    r.saved = action::candidate(r, r.playing.as_ref().unwrap()).unwrap().0;
    let existing = r.saved.favorites.entries()[0].clone();
    install_local(&mut app, &root);
    let r = app.ott.as_mut().unwrap();
    let profiles = r.saved.profiles.clone();
    let (candidate, identity, added) = action::candidate(r, r.playing.as_ref().unwrap()).unwrap();
    assert!(added);
    assert_eq!(candidate.favorites.entries().len(), 2);
    assert!(candidate.favorites.entries().contains(&existing));
    assert_eq!(candidate.profiles, profiles);
    let item = candidate
        .favorites
        .entries()
        .iter()
        .find(|v| v.identity() == identity)
        .unwrap();
    let bindings = r.selection_bindings().unwrap();
    let permit = candidate
        .local_favorite_proofs
        .authorize(
            &bindings,
            &candidate.favorites,
            item,
            &r.playing.as_ref().unwrap().profile,
        )
        .unwrap()
        .unwrap();
    assert_eq!(permit.item_id(), "first");
    r.saved = candidate;
    let (removed, _, added) = action::candidate(r, r.playing.as_ref().unwrap()).unwrap();
    assert!(!added);
    assert_eq!(removed.favorites.entries(), &[existing]);
}

#[test]
fn withdrawn_local_grant_and_wrong_named_profile_cannot_capture_playing_path() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, _root) = ready(true);
    let r = app.ott.as_mut().unwrap();
    let old = r.saved.favorites.clone();
    let mut wrong = r.playing.clone().unwrap();
    wrong.profile.id = "named-local".into();
    wrong.profile.name = "Other".into();
    wrong.profile.source = "/unrelated/folder".into();
    r.saved.profiles.profiles.push(wrong.profile.clone());
    assert!(action::candidate(r, &wrong).is_err());
    r.local_inventory.sources[0].withdraw();
    assert!(invoke(&mut app).is_err());
    let r = app.ott.as_ref().unwrap();
    assert_eq!(r.saved.favorites, old);
    assert!(!r.saving && r.pending_playing_favorite.is_none());
}

#[test]
fn saved_completion_updates_once_without_changing_provider_category_or_playback() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, _root) = ready(false);
    let old_identity = plx_media::route::playback_identity(&app.player.session);
    let old_profiles = app.ott.as_ref().unwrap().saved.profiles.clone();
    let candidate = stage(&mut app);
    deliver(&mut app, Ok(candidate.clone()));
    let r = app.ott.as_ref().unwrap();
    assert_eq!(r.saved.favorites, candidate.favorites);
    assert_eq!(r.saved.profiles, old_profiles);
    assert_eq!(r.saved.category, candidate.category);
    assert!(!r.saving && r.pending_playing_favorite.is_none());
    assert_eq!(
        plx_media::route::playback_identity(&app.player.session),
        old_identity
    );
    let removed = stage(&mut app);
    assert!(removed.favorites.entries().is_empty());
    deliver(&mut app, Ok(removed));
    assert!(
        app.ott
            .as_ref()
            .unwrap()
            .saved
            .favorites
            .entries()
            .is_empty()
    );
}

#[test]
fn refused_write_never_publishes_the_optimistic_favorite() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, _root) = ready(false);
    stage(&mut app);
    deliver(&mut app, Err("Fixture write refused"));
    let r = app.ott.as_ref().unwrap();
    assert!(r.saved.favorites.entries().is_empty());
    assert!(!r.saving && r.pending_playing_favorite.is_none());
}

#[test]
fn same_url_new_native_identity_cannot_receive_old_favorite_notice() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, _root) = ready(false);
    let candidate = stage(&mut app);
    let old_identity = plx_media::route::playback_identity(&app.player.session);
    local_integration::install(&mut app, None, "replacement-selection");
    assert_ne!(
        plx_media::route::playback_identity(&app.player.session),
        old_identity
    );
    let mut r = app.ott.take().unwrap();
    r.saved = candidate;
    assert!(!action::settled(&mut r, &mut app, true));
    assert!(r.pending_playing_favorite.is_none());
    app.ott = Some(r);
}

#[test]
fn leaving_video_cannot_receive_old_favorite_notice_or_new_toggle() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, _root) = ready(false);
    let candidate = stage(&mut app);
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    let mut r = app.ott.take().unwrap();
    r.saved = candidate;
    assert!(!action::settled(&mut r, &mut app, true));
    assert!(action::toggle(&mut r, &mut app).is_err());
    assert!(r.pending_playing_favorite.is_none());
    app.ott = Some(r);
}

#[test]
fn stopped_application_cannot_toggle_an_admitted_favorite() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, _root) = ready(false);
    app.running = false;
    assert_eq!(invoke(&mut app), Err("Favorites require the active video page"));
    let r = app.ott.as_ref().unwrap();
    assert!(!r.saving && r.pending_playing_favorite.is_none());
    assert!(r.saved.favorites.entries().is_empty());
}

#[test]
fn no_native_identity_or_invalid_kiosk_policy_cannot_write() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, _root) = ready(false);
    app.ott.as_mut().unwrap().playing.as_mut().unwrap().identity = None;
    assert!(invoke(&mut app).is_err());
    local_integration::install(&mut app, None, "playing-again");
    app.ott
        .as_mut()
        .unwrap()
        .saved
        .settings
        .insert("nativeKiosk".into(), json!({"invalid":true}));
    assert!(invoke(&mut app).is_err());
    let r = app.ott.as_ref().unwrap();
    assert!(!r.saving && r.pending_playing_favorite.is_none());
    assert!(r.saved.favorites.entries().is_empty());
}

#[test]
fn protected_playing_source_without_a_pin_grant_is_not_favorited() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, _root) = ready(false);
    // Existing valid record fixture: parsing only, no key derivation or grant.
    app.ott.as_mut().unwrap().saved.settings.insert("parentPin".into(), json!(
        "pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
    app.ott.as_mut().unwrap().playing.as_mut().unwrap().entry.protection = Some(true);
    assert!(invoke(&mut app).is_err());
    let r = app.ott.as_ref().unwrap();
    assert!(!r.saving && r.pending_playing_favorite.is_none());
    assert!(r.saved.favorites.entries().is_empty());
}

#[test]
fn matching_saved_completion_opens_one_bounded_notice() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, _root) = ready(false);
    let candidate = stage(&mut app);
    let mut r = app.ott.take().unwrap();
    r.saved = candidate;
    assert!(action::settled(&mut r, &mut app, true));
    assert!(
        !action::settled(&mut r, &mut app, true),
        "a completion cannot announce twice"
    );
    app.ott = Some(r);
}

#[test]
fn candidate_preserves_portal_and_plex_identity_in_one_global_document() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, _root) = ready(false);
    let r = app.ott.as_mut().unwrap();
    r.saved = action::candidate(r, r.playing.as_ref().unwrap()).unwrap().0;
    let mut portal = r.playing.clone().unwrap();
    portal.profile = synthetic_profile();
    portal.entry = synthetic_entry();
    portal.entry.category = "Family / Season 2".into();
    r.saved.profiles.profiles.push(portal.profile.clone());
    r.saved = action::candidate(r, &portal).unwrap().0;
    let mut plex = portal.clone();
    plex.profile.id = "plex-source".into();
    plex.profile.kind = ProviderKind::Plex;
    plex.profile.name = "Media server".into();
    plex.profile.source = "server-123".into();
    plex.entry.media = MediaLocator::Plex {
        server_id: "server-123".into(),
        rating_key: "42".into(),
    };
    r.saved.profiles.profiles.push(plex.profile.clone());
    r.saved = action::candidate(r, &plex).unwrap().0;
    let entries = r.saved.favorites.entries();
    assert_eq!(entries.len(), 3);
    let portal = entries
        .iter()
        .find(|f| f.origin.provider == "vportal")
        .unwrap();
    assert_eq!(
        portal.origin.subprofile.as_deref(),
        Some("Family / Season 2")
    );
    let plex = entries
        .iter()
        .find(|f| f.origin.provider == "plex")
        .unwrap();
    assert_eq!(plex.origin.profile, "Plex");
    assert_eq!(plex.origin.source_alias, "server-123");
    assert!(
        matches!(&plex.locator, plx_data::ott::favorites::Locator::Plex { rating_key, .. } if rating_key == "42")
    );
    // This case tests the shared pure capture, not a synthetic native Plex admission.
}

#[test]
fn retained_file_grant_survives_new_inventory_receipt_but_not_same_path_reacquisition() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, root) = ready_file();
    let r = app.ott.as_mut().unwrap();
    let playing = r.playing.as_ref().unwrap().clone();
    let original_selection = playing.local_selection.as_ref().unwrap();
    assert!(action::candidate(r, &playing).is_ok());
    r.local_inventory
        .install(r.local_inventory.sources.clone(), 8);
    assert!(
        r.capture_local_source(original_selection).is_none(),
        "fixture must change receipt metadata"
    );
    assert!(
        action::candidate(r, &playing).is_ok(),
        "same retained handle permits fresh receipt"
    );
    let new_grant = crate::app::ott_local_root::LocalSource::acquire_authorized(
        &plx_base::task::OffFrame::for_test(),
        root.clone(),
        &mut || true,
    )
    .unwrap();
    assert!(!new_grant.same_grant(&r.local_inventory.sources[0]));
    r.local_inventory.install(vec![new_grant], 8);
    assert!(
        r.bind_local_selection(&root)
            .and_then(|s| r.capture_local_source(&s).ok_or("missing"))
            .is_ok()
    );
    assert!(
        matches!(playing.source.as_ref(), Some(plx_media::ff::ExternalSource::HeldLocal(held)) if !held.is_live())
    );
    assert!(
        action::candidate(r, &playing).is_err(),
        "same descriptor cannot attest old admitted file"
    );
    assert!(invoke(&mut app).is_err());
    assert!(!app.ott.as_ref().unwrap().saving);
}

#[test]
fn accepted_folder_grant_survives_clone_refresh_but_replacement_retires_held_media() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, root) = ready(true);
    let r = app.ott.as_mut().unwrap();
    let playing = r.playing.as_ref().unwrap().clone();
    r.local_inventory
        .install(r.local_inventory.sources.clone(), 8);
    assert!(action::candidate(r, &playing).is_ok());
    let reopened = crate::app::ott_local_root::LocalSource::acquire_authorized(
        &plx_base::task::OffFrame::for_test(),
        root,
        &mut || true,
    )
    .unwrap();
    r.local_inventory.install(vec![reopened], 8);
    assert!(action::candidate(r, &playing).is_err());
    assert!(
        matches!(playing.source.as_ref(), Some(plx_media::ff::ExternalSource::HeldLocal(held)) if !held.is_live())
    );
}

#[test]
fn non_session_folder_uses_original_media_withdrawal_fence() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_dir, mut app, root) = ready(true);
    let r = app.ott.as_mut().unwrap();
    r.transient.accepted = None;
    let playing = r.playing.as_ref().unwrap().clone();
    r.local_inventory
        .install(r.local_inventory.sources.clone(), 8);
    assert!(action::candidate(r, &playing).is_ok());
    let reopened = crate::app::ott_local_root::LocalSource::acquire_authorized(
        &plx_base::task::OffFrame::for_test(),
        root,
        &mut || true,
    )
    .unwrap();
    r.local_inventory.install(vec![reopened], 8);
    assert!(action::candidate(r, &playing).is_err());
}

#[test]
fn incomplete_m3u_capture_becomes_available_after_its_catalogue_finishes() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset;
    let (_dir, mut app, _) = ready(false);
    let r = app.ott.as_mut().unwrap();
    let playing = r.playing.as_ref().unwrap().clone();
    r.transient = Default::default();
    r.saved.profiles.active_profile = Some(playing.profile.id.clone());
    r.catalog.entries = vec![playing.entry.clone()];
    r.catalog_complete = false;
    r.playing.as_mut().unwrap().channel_capture = None;
    assert!(action::capture(r, r.playing.as_ref().unwrap()).is_err());
    r.catalog_complete = true;
    action::catalog_finished(r);
    assert!(r.playing.as_ref().unwrap().channel_capture.is_some());
    assert_eq!(action::capture(r, r.playing.as_ref().unwrap()).unwrap().title, playing.entry.title);
    let mut other = playing.profile.clone(); other.id = "unrelated".into(); other.name = "Unrelated".into();
    r.saved.profiles.active_profile = Some(other.id.clone()); r.saved.profiles.profiles.push(other);
    r.catalog.entries.clear();
    assert_eq!(action::capture(r, r.playing.as_ref().unwrap()).unwrap().title, playing.entry.title);
}

#[test]
fn source_replacement_rejects_retained_channel_capture_without_a_write() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset;
    let (_dir, mut app, _) = ready(false);
    let r = app.ott.as_mut().unwrap(); let playing = r.playing.as_ref().unwrap().clone();
    let original = r.saved.favorites.clone();
    r.saved.profiles.profiles.iter_mut().find(|profile| profile.id == playing.profile.id).unwrap()
        .source = "https://replacement.invalid/list.m3u".into();
    assert!(action::candidate(r, &playing).is_err());
    assert_eq!(r.saved.favorites, original); assert!(!r.saving);
}

#[test]
fn selection_id_alone_cannot_attest_an_imported_playing_favorite() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset;
    let (_dir, mut app, _) = ready(false);
    let r = app.ott.as_mut().unwrap();
    let favorite = action::capture(r, r.playing.as_ref().unwrap()).unwrap();
    r.saved.favorites.toggle(favorite.clone()).unwrap();
    let playing = r.playing.as_mut().unwrap();
    playing.selection_id = format!("favorite:{}", favorite.identity());
    playing.channel_capture = None;
    assert!(action::capture(r, r.playing.as_ref().unwrap()).is_err());
    assert!(action::candidate(r, r.playing.as_ref().unwrap()).is_err());
}

// Model the ordinary row admission: unlike Favorite replay it retains the
// numbered Saved profile with the same original held File grant. No decoder,
// network or protected write runs in this fixture.
fn ready_numbered_file() -> (
    local_integration::TempDir, Box<App>, crate::app::ott_local_session::BoundSource,
) {
    let (dir, mut app, root) = ready_file();
    let r = app.ott.as_mut().unwrap();
    let mut profile = r.playing.as_ref().unwrap().profile.clone();
    profile.id = "local-1".into();
    profile.name = "local 1".into();
    profile.source_alias = String::new();
    r.saved.profiles.profiles.push(profile.clone());
    r.saved.profiles.active_profile = Some(profile.id.clone());
    r.provider = ProviderKind::Local;
    r.playing.as_mut().unwrap().profile = profile.clone();
    r.catalog.entries = vec![r.playing.as_ref().unwrap().entry.clone()];
    r.catalog_loading = false;
    r.catalog_complete = true;
    let (request, admitted, _, _) = r.prepare_play_request(PlaybackRequest::ordinary("first", 0)).unwrap();
    assert_eq!(admitted, profile, "ordinary rows keep the numbered profile");
    assert_eq!(request.local_selection, r.playing.as_ref().unwrap().local_selection);
    assert_ne!(profile, transient::source_profile(&root).unwrap());
    (dir, app, root)
}

#[test]
fn numbered_file_current_and_toggle_use_root_identity_without_rewriting_profile() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset;
    let (_dir, mut app, root) = ready_numbered_file();
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
    let mut r = app.ott.take().unwrap();
    let before = serde_json::to_value(&r.saved).unwrap();
    let expected = Favorite {
        title: "first".into(),
        origin: plx_data::ott::favorites::Origin {
            provider: "local".into(), profile: "Local videos".into(),
            subprofile: None, source_alias: root.name.clone(),
        },
        locator: plx_data::ott::favorites::Locator::Local {
            root_alias: root.name.clone(), relative_path: "first.mp4".into(),
        },
    };
    let now = r.now_ms();
    let body = json!({"commands":[],"server_time":1000,"requests":[{
        "id":format!("{:032x}",1),"action":"favorites_global_items",
        "params":{"runtime":"fixture","op":"current","subject":"global"},"expires_at":1020
    }]}).to_string();
    let mut requests = r.remote.raw_poll_for_test(&body, now);
    assert_eq!(requests.len(), 1);
    let invocation = requests.remove(0);
    assert!(r.remote.invocation_is_current(&invocation));
    let response = commands::command(&mut r, &mut app, &invocation).unwrap();
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["entry"], serde_json::to_value(&expected).unwrap());
    assert_eq!(response["data"]["unavailable_reason"], Value::Null);
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before, "Current is read-only");
    let profiles = r.saved.profiles.clone();
    let (candidate, identity, added) = action::candidate(&r, r.playing.as_ref().unwrap()).unwrap();
    assert!(added);
    assert_eq!(candidate.profiles, profiles);
    assert_eq!(candidate.favorites.entries(), &[expected.clone()]);
    let replay_profile = transient::source_profile(&root).unwrap();
    let bindings = r.selection_bindings().unwrap();
    let permit = candidate.local_favorite_proofs.authorize(
        &bindings, &candidate.favorites, &expected, &replay_profile,
    ).unwrap().unwrap();
    assert_eq!(permit.item_id(), "first");
    r.saved = candidate;
    let (request, replay, _, favorite) = r.prepare_play_request(
        PlaybackRequest::ordinary(&format!("favorite:{identity}"), 0),
    ).unwrap();
    assert_eq!(replay, replay_profile);
    assert_eq!(favorite, Some(expected));
    assert_eq!(request.local_selection, r.playing.as_ref().unwrap().local_selection);
    let (removed, removed_identity, added) = action::candidate(&r, r.playing.as_ref().unwrap()).unwrap();
    assert!(!added);
    assert_eq!(removed_identity, identity);
    assert!(removed.favorites.entries().is_empty());
    assert_eq!(removed.profiles, profiles);
    app.ott = Some(r);
}

#[test]
fn numbered_file_capture_refuses_changed_or_ambiguous_saved_profile() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset;
    for mutation in ["removed", "disabled", "renamed", "path", "duplicate", "kind"] {
        let (_dir, mut app, _) = ready_numbered_file();
        let r = app.ott.as_mut().unwrap();
        let playing = r.playing.as_ref().unwrap().clone();
        let saved = r.saved.profiles.profiles.iter_mut().find(|p| p.id == "local-1").unwrap();
        match mutation {
            "disabled" => saved.enabled = false,
            "renamed" => saved.name = "replacement".into(),
            "path" => saved.source = "/unrelated/first.mp4".into(),
            "kind" => saved.kind = ProviderKind::M3u,
            "removed" => r.saved.profiles.profiles.retain(|p| p.id != "local-1"),
            "duplicate" => r.saved.profiles.profiles.push(playing.profile.clone()),
            _ => unreachable!(),
        }
        let before = serde_json::to_value(&r.saved).unwrap();
        assert!(action::capture(r, &playing).is_err(), "{mutation}");
        assert!(action::candidate(r, &playing).is_err(), "{mutation}");
        assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
        assert!(!r.saving);
    }
}

#[test]
fn numbered_file_capture_keeps_original_handle_and_locator_authority() {
    let _serial = plx_base::testlock::serial(); let _reset = Reset;
    let (_dir, mut app, root) = ready_numbered_file();
    let r = app.ott.as_mut().unwrap();
    let playing = r.playing.as_ref().unwrap().clone();
    assert!(action::capture(r, &playing).is_ok());
    let mut sibling = playing.clone();
    sibling.entry.media = MediaLocator::Local { relative_path: "sibling.mp4".into() };
    assert!(action::candidate(r, &sibling).is_err());
    r.local_inventory.install(r.local_inventory.sources.clone(), 8);
    assert!(action::candidate(r, &playing).is_ok(), "same retained handle permits receipt refresh");
    let reopened = crate::app::ott_local_root::LocalSource::acquire_authorized(
        &plx_base::task::OffFrame::for_test(), root, &mut || true,
    ).unwrap();
    r.local_inventory.install(vec![reopened], 9);
    assert!(action::capture(r, &playing).is_err(), "reopening same path cannot revive original handle");
    assert!(action::candidate(r, &playing).is_err());
    assert!(r.saved.favorites.entries().is_empty());
}
