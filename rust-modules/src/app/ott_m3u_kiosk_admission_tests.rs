//! Exact M3U attachment kiosk transaction and restart regressions. The parent
//! supplies the real foreground App and PIN-completion fixture. Preparation and
//! held resolver seams exercise production admission without worker or media IO.
use super::*;
use crate::app::ott_kiosk_stored::{self as stored, Stored};
use plx_data::ott::favorites::{Locator, Origin};

const LINK: &str = "portal::[key:fixture-secret]https://portal.fixture.invalid/api/";

fn tv_entry() -> CatalogEntry {
    CatalogEntry {
        synopsis: None,
        live: None,
        id: "home-channel".into(),
        title: "Existing TV".into(),
        category: "News".into(),
        description: String::new(),
        poster: String::new(),
        media: MediaLocator::Http {
            url: "https://tv.fixture.invalid/live.m3u8".into(),
        },
        duration_ms: None,
        provider_id: Some("home-channel".into()),
        provider_id_field: None,
    }
}

fn attached_favorite(parent: &ProviderProfile) -> Favorite {
    Favorite {
        title: "Attached film".into(),
        origin: Origin {
            provider: "m3u".into(),
            profile: parent.name.clone(),
            source_alias: parent.source_alias.clone(),
            subprofile: Some("Films/Family".into()),
        },
        locator: Locator::VPortal {
            id_field: "id".into(),
            id: "42".into(),
        },
    }
}

fn attached_app(protected: bool) -> App {
    let mut app = fixture();
    let parent = profile("home");
    let tv = profile_favorite(&parent, "Existing TV");
    let film = attached_favorite(&parent);
    {
        let runtime = app.ott.as_mut().unwrap();
        if !protected {
            runtime.saved.settings.remove("parentPin");
        }
        runtime.saved.profiles.profiles = vec![parent.clone()];
        runtime.saved.profiles.active_profile = Some(parent.id.clone());
        runtime.provider = ProviderKind::M3u;
        runtime.saved = runtime
            .attachment_candidate(&parent, LINK, Some("attachment-home"))
            .unwrap();
        runtime.observe_m3u_attachments();
        runtime.saved.favorites.toggle(tv.clone()).unwrap();
        runtime.saved.favorites.toggle(film).unwrap();
        runtime.saved.favorites_selected = true;
        runtime.saved.settings.insert(
            stored::KEY.into(),
            Stored::item(tv, true).unwrap().encode().unwrap(),
        );
        runtime.catalog.entries = vec![tv_entry()];
        runtime.catalog_complete = true;
        runtime.catalog_loading = false;
        runtime.selected = Some("home-channel".into());
        runtime.playing = Some(Playing { channel_capture: None,
            archive: None,
            kiosk_selection: None,
            transient_owner: None,
            local_selection: None,
            identity: None,
            profile: parent,
            entry: tv_entry(),
            selection_id: "home-channel".into(),
            source: None,
            advance_on_failure: false,
            requires_server_lease: false,
        });
        // If an admission assertion regresses, no resolver worker may start.
        runtime.resolver_busy = true;
    }
    // Exercise persisted-pin startup and its typed rebind, rather than treating
    // construction of an ordinary Favorite as evidence that the pin restores.
    let mut runtime = app.ott.take().unwrap();
    let mut playing = runtime.playing.take().unwrap();
    assert!(kiosk_source::stage_startup_item(&mut runtime, &app).unwrap());
    let request = runtime.resume_pending.take().unwrap();
    assert!(request.kiosk_selection.is_some());
    let (bound, parent, _, favorite) = runtime.prepare_play_request(request).unwrap();
    assert_eq!(parent, runtime.saved.profiles.profiles[0]);
    assert_eq!(favorite.as_ref(), runtime.saved.favorites.entries().first());
    playing.kiosk_selection = bound.kiosk_selection;
    runtime.playing = Some(playing);
    app.ott = Some(runtime);
    if protected {
        grant_current_profile_for_dispatch(&mut app);
    }
    let runtime = app.ott.as_mut().unwrap();
    let parent = runtime.saved.profiles.profiles[0].clone();
    assert!(runtime.require_parental_playback(&parent).is_ok());
    assert!(runtime.settings_write_refusal().is_none());
    assert_eq!(runtime.ordered_ids().len(), 2);
    // The old pin really is bindable through the same production designation
    // path. Failure below must be specific to the second, attached Favorite.
    assert!(
        kiosk_source::prepare(app.ott.as_ref().unwrap(), &app, Some("Existing TV"), true,).is_ok()
    );
    app
}

fn assert_quiet(runtime: &Runtime) {
    assert!(!runtime.saving, "must not admit a Saved writer");
    assert!(!commands::kiosk_pending(runtime));
    assert!(!runtime.kiosk_capture.pending());
    assert!(runtime.pending_play.is_none());
    assert!(runtime.resume_pending.is_none());
    assert!(runtime.play_target.is_none());
    assert!(runtime.play_attachment.is_none());
    assert!(runtime.resolver_next.is_none());
    assert!(runtime.catalog_next.is_none());
    assert!(runtime.metadata_next.is_none());
    assert!(
        runtime.resolver_busy,
        "do not release an unrelated worker slot"
    );
    assert!(matches!(
        runtime.rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(matches!(
        runtime.catalog_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(matches!(
        runtime.metadata_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

/// Exercise the real transaction preparation without admitting a Saved worker.
/// Existing command tests cover writer settlement; this suite never starts IO.
fn prepared_attachment(app: &mut App, query: &str) -> (kiosk_source::Prepared, Saved) {
    let mut runtime = app.ott.take().unwrap();
    assert_quiet(&runtime);
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let selected = runtime.selected.clone();
    let playing = runtime.playing.as_ref().unwrap().clone();
    let catalog = runtime.catalog.entries.clone();
    let status = runtime.status.clone();
    let page = runtime.page;
    let context_revision = runtime.m3u_attachments.context_revision();
    let ticket = plx_media::route::play_request_ticket();
    let generation = runtime.play_generation;
    let epoch = runtime.playback_epoch;
    let receipt = runtime.attachment_for(&attached_favorite(&runtime.saved.profiles.profiles[0]))
        .unwrap().unwrap();
    let prepared = kiosk_source::prepare(&runtime, app, Some(query), true)
        .expect("an explicitly selected attached Favorite must have a safe kiosk representation");
    kiosk_source::authorize_designation(&mut runtime, app, &prepared)
        .expect("the actual M3U parent is authorized, not an unregistered projected profile");
    let candidate = kiosk_source::save_candidate(&runtime, &prepared).unwrap();
    candidate.validate().unwrap();
    let stored = stored::read(&candidate.settings).unwrap().unwrap();
    assert_eq!(stored.origin().provider, "m3u");
    assert_eq!(stored.origin().profile, receipt.parent().name);
    assert!(stored.strict());
    assert!(stored.attachment_binding() == Some(&receipt.kiosk_binding().unwrap()));
    let stored::TargetRef::Item(favorite) = stored.target() else { panic!("item pin") };
    assert_eq!(favorite, &attached_favorite(receipt.parent()));
    assert_eq!(favorite.origin.subprofile.as_deref(), Some("Films/Family"));
    let encoded = stored.encode().unwrap().to_string();
    assert!(!encoded.contains("fixture-secret"));
    assert!(!encoded.contains("portal.fixture.invalid"));
    assert_eq!(candidate.profiles, runtime.saved.profiles);
    assert_eq!(candidate.favorites, runtime.saved.favorites);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before,
        "preparing a candidate must not publish it");
    assert_eq!(runtime.selected, selected);
    assert_eq!(runtime.catalog.entries, catalog);
    assert_eq!(runtime.status, status);
    assert_eq!(runtime.page, page);
    assert_eq!(runtime.m3u_attachments.context_revision(), context_revision);
    let current = runtime.playing.as_ref().unwrap();
    assert_eq!(current.profile, playing.profile);
    assert_eq!(current.entry, playing.entry);
    assert_eq!(current.selection_id, playing.selection_id);
    assert_eq!(current.identity, playing.identity);
    assert!(current.kiosk_selection == playing.kiosk_selection);
    assert!(runtime.playing_attachment.is_none());
    assert!(plx_media::route::play_request_is_current(ticket));
    assert_eq!(runtime.play_generation, generation);
    assert_eq!(runtime.playback_epoch, epoch);
    assert_quiet(&runtime);
    app.ott = Some(runtime);
    (prepared, candidate)
}

fn designated_attachment_is_prepared(protected: bool, query: &str) {
    let mut app = attached_app(protected);
    prepared_attachment(&mut app, query);
}

#[test]
fn attached_favorite_number_prepares_exact_kiosk_save_without_protection() {
    let _serial = plx_base::testlock::serial();
    designated_attachment_is_prepared(false, "2");
}
#[test]
fn attached_favorite_title_prepares_exact_kiosk_save_without_protection() {
    let _serial = plx_base::testlock::serial();
    designated_attachment_is_prepared(false, "Attached film");
}
#[test]
fn attached_favorite_number_prepares_exact_kiosk_save_with_parent_unlocked() {
    let _serial = plx_base::testlock::serial();
    designated_attachment_is_prepared(true, "2");
}
#[test]
fn attached_favorite_title_prepares_exact_kiosk_save_with_parent_unlocked() {
    let _serial = plx_base::testlock::serial();
    designated_attachment_is_prepared(true, "Attached film");
}

fn cold_runtime(mut saved: Saved) -> Runtime {
    // A real Saved wire round trip strips every ephemeral receipt. No global
    // Favorite or prior catalog may accidentally be the cold-start authority.
    for favorite in saved.favorites.entries().to_vec() {
        saved.favorites.toggle(favorite).unwrap();
    }
    let bytes = serde_json::to_vec(&saved).unwrap();
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.saved = serde_json::from_slice(&bytes).unwrap();
    runtime.loaded = true;
    runtime.provider = ProviderKind::M3u;
    runtime.write_permit.revoke();
    runtime.resolver_busy = true;
    runtime.observe_m3u_attachments();
    runtime
}

fn cold_request(runtime: &mut Runtime, app: &App) -> PlaybackRequest {
    assert!(runtime.catalog.entries.is_empty());
    assert!(runtime.saved.favorites.entries().is_empty());
    assert!(kiosk_source::stage_startup_item(runtime, app).unwrap());
    let request = runtime.resume_pending.take().unwrap();
    assert!(request.kiosk_selection.is_some());
    assert!(request.id.starts_with("kiosk:"));
    assert!(!request.advance_on_failure);
    assert_eq!(request.position_ms, 0);
    request
}

#[test]
fn attached_pin_cold_start_rebinds_without_global_favorites_or_catalog() {
    let _serial = plx_base::testlock::serial();
    let mut app = attached_app(false);
    let (_, candidate) = prepared_attachment(&mut app, "2");
    let expected = attached_favorite(&candidate.profiles.profiles[0]);
    let mut runtime = cold_runtime(candidate);
    let request = cold_request(&mut runtime, &app);
    let (bound, effective, entry, favorite) = runtime.prepare_play_request(request).unwrap();
    assert_eq!(effective.kind, ProviderKind::VPortal);
    assert_eq!(effective.id, "attachment-home");
    assert_eq!(effective.source, LINK);
    assert!(entry.is_none(), "the existing resolver must find the exact item lazily");
    assert_eq!(favorite, Some(expected));
    let mut untyped = bound.clone();
    untyped.kiosk_selection = None;
    assert!(runtime.prepare_play_request(untyped).is_err(),
        "the public kiosk: string alone must not grant the typed pin authority");
    assert_eq!(runtime.attachment_policy_profile(&effective, favorite.as_ref()).unwrap(),
        runtime.saved.profiles.profiles[0]);
    kiosk_source::admit_bound(&runtime, &app, kiosk_source::BoundCandidate {
        request: &bound, profile: &effective, entry: None, favorite: favorite.as_ref(),
    }).unwrap();
    assert_eq!(runtime.saved.profiles.profiles.len(), 1);
    assert!(runtime.saved.favorites.entries().is_empty());
    assert_quiet(&runtime);
}

#[derive(Clone, Copy, Debug)]
enum Replacement { Link, AttachmentId, ParentId, ParentSource, ParentAlias, Disabled, Missing }
fn replace_source(runtime: &mut Runtime, replacement: Replacement) {
    use crate::app::ott_m3u_attachment::SETTINGS_KEY;
    match replacement {
        Replacement::Link => {
            let parent = runtime.saved.profiles.profiles[0].clone();
            runtime.saved = runtime.attachment_candidate(&parent,
                "portal::[key:changed]https://replacement.fixture.invalid/api/", None).unwrap();
        }
        Replacement::AttachmentId => {
            runtime.saved.settings.get_mut(SETTINGS_KEY).unwrap()["entries"][0]["id"] = json!("attachment-new");
        }
        Replacement::ParentId => {
            runtime.saved.profiles.profiles[0].id = "new-parent".into();
            runtime.saved.profiles.active_profile = Some("new-parent".into());
            runtime.saved.settings.get_mut(SETTINGS_KEY).unwrap()["entries"][0]["parent_profile_id"] = json!("new-parent");
        }
        Replacement::ParentSource => {
            runtime.saved.profiles.profiles[0].source = "https://replacement.fixture.invalid/list.m3u".into();
        }
        Replacement::ParentAlias => runtime.saved.profiles.profiles[0].source_alias = "Other source".into(),
        Replacement::Disabled => {
            runtime.saved.profiles.profiles[0].enabled = false;
            // A valid Saved image cannot keep a disabled profile active.
            runtime.saved.profiles.active_profile = None;
        },
        Replacement::Missing => { runtime.saved.settings.remove(SETTINGS_KEY); }
    }
    runtime.saved.validate().unwrap();
    runtime.observe_m3u_attachments();
}

#[test]
fn attached_pin_rejects_source_replacement_live_and_after_cold_restart() {
    let _serial = plx_base::testlock::serial();
    for replacement in [Replacement::Link, Replacement::AttachmentId, Replacement::ParentId,
        Replacement::ParentSource, Replacement::ParentAlias, Replacement::Disabled, Replacement::Missing] {
        let mut app = attached_app(false);
        let (_, candidate) = prepared_attachment(&mut app, "2");
        let mut runtime = cold_runtime(candidate);
        let request = cold_request(&mut runtime, &app);
        let bound = runtime.prepare_play_request(request).unwrap().0;
        replace_source(&mut runtime, replacement);
        assert!(runtime.prepare_play_request(bound).is_err(), "live {replacement:?}");
        // Discard the old RAM proof too: durable identity must reject a freshly
        // observed replacement rather than adopting it under the old movie ID.
        let mut cold = cold_runtime(runtime.saved);
        match kiosk_source::stage_startup_item(&mut cold, &app) {
            Ok(true) => {
                let pending = cold.resume_pending.take().unwrap();
                assert!(cold.prepare_play_request(pending).is_err(), "cold {replacement:?}");
            }
            Err(_) => {},
            Ok(false) => panic!("no newer intent explains losing the pinned request"),
        }
        assert_quiet(&cold);
    }
}

#[test]
fn attached_prepared_and_bound_receipts_reject_a_b_a_but_fresh_cold_pin_recovers_a() {
    let _serial = plx_base::testlock::serial();
    let mut app = attached_app(false);
    let (prepared, candidate) = prepared_attachment(&mut app, "2");
    let mut runtime = app.ott.take().unwrap();
    let original = runtime.saved.clone();
    replace_source(&mut runtime, Replacement::Link);
    runtime.saved = original;
    runtime.observe_m3u_attachments();
    assert!(!kiosk_source::still_current(&runtime, &app, &prepared),
        "same link bytes do not revive a retired RAM receipt");
    let mut runtime = cold_runtime(candidate);
    let request = cold_request(&mut runtime, &app);
    let bound = runtime.prepare_play_request(request).unwrap().0;
    let original = runtime.saved.clone();
    replace_source(&mut runtime, Replacement::Link);
    runtime.saved = original.clone();
    runtime.observe_m3u_attachments();
    assert!(runtime.prepare_play_request(bound).is_err());
    let mut fresh = cold_runtime(original);
    let request = cold_request(&mut fresh, &app);
    assert!(fresh.prepare_play_request(request).is_ok(),
        "a new process may bind the original exact durable source again");
    assert_quiet(&fresh);
}

#[test]
fn attached_legacy_item_without_source_binding_cannot_borrow_current_attachment() {
    let _serial = plx_base::testlock::serial();
    let mut app = attached_app(false);
    let (_, mut candidate) = prepared_attachment(&mut app, "2");
    let favorite = attached_favorite(&candidate.profiles.profiles[0]);
    candidate.settings.insert(stored::KEY.into(), Stored::item(favorite, true).unwrap().encode().unwrap());
    let mut runtime = cold_runtime(candidate);
    match kiosk_source::stage_startup_item(&mut runtime, &app) {
        Ok(true) => {
            let request = runtime.resume_pending.take().unwrap();
            assert!(runtime.prepare_play_request(request).is_err());
        }
        Err(_) => {},
        Ok(false) => panic!("missing binding is a source failure, not a newer navigation"),
    }
    assert_quiet(&runtime);
}

#[test]
fn attached_typed_pin_pending_survives_favorite_absence_but_not_changed_source() {
    let _serial = plx_base::testlock::serial();
    let mut app = attached_app(false);
    let (_, candidate) = prepared_attachment(&mut app, "2");
    let mut runtime = cold_runtime(candidate);
    // Direct dispatch needs the same sampled clock input as the frame runner.
    let sample = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap();
    let anchor = *runtime.clock_anchor.get_or_insert(sample);
    runtime.wall_clock_ms = sample.unix_ms();
    runtime.clock_sample = Some(sample.steady_from(anchor));
    runtime.resume_source_work();
    let request = cold_request(&mut runtime, &app);
    runtime.play_request_with_policy(&mut app, request).unwrap();
    assert!(runtime.play_target.as_ref().unwrap().kiosk_selection.is_some());
    assert!(runtime.play_attachment.is_some());
    assert!(runtime.pending_attachment_current());
    assert!(runtime.resolver_next.is_some());
    let cancel = runtime.play_cancel.clone();
    runtime.observe_m3u_attachments();
    assert!(!cancel.load(Ordering::Acquire));
    assert!(runtime.play_target.is_some(), "unchanged source cannot cancel a pin absent from global Favorites");
    replace_source(&mut runtime, Replacement::Link);
    assert!(cancel.load(Ordering::Acquire));
    assert!(runtime.play_target.is_none());
    assert!(runtime.play_attachment.is_none());
    assert!(runtime.resolver_next.is_none());
    assert!(runtime.saved.favorites.entries().is_empty());
    assert!(!runtime.saving);
}

#[test]
fn attached_designation_still_requires_parent_unlock_and_cannot_use_another_parent_grant() {
    let _serial = plx_base::testlock::serial();
    let mut app = attached_app(true);
    let mut runtime = app.ott.take().unwrap();
    let parent = runtime.saved.profiles.profiles[0].clone();
    let prepared = kiosk_source::prepare(&runtime, &app, Some("2"), true).unwrap();
    runtime.retire_parental_source();
    assert!(runtime.require_parental_playback(&parent).is_err());
    assert!(kiosk_source::authorize_designation(&mut runtime, &app, &prepared).is_err());
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    for mode in ["on", "set"] {
        let invocation = command_invocation(&mut runtime, "kiosk", json!({"mode": mode, "query": "2"}));
        let response = commands::command(&mut runtime, &mut app, &invocation).unwrap();
        assert_eq!(response["status"], "rejected");
        assert!(!response.to_string().contains("not support"));
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        assert_quiet(&runtime);
    }
    // A valid grant for another current profile is not an attachment grant.
    runtime.saved.profiles.profiles.push(profile("other"));
    runtime.saved.profiles.active_profile = Some("other".into());
    runtime.observe_m3u_attachments();
    app.ott = Some(runtime);
    grant_current_profile_for_dispatch(&mut app);
    let mut runtime = app.ott.take().unwrap();
    assert!(runtime.require_parental_playback(&profile("other")).is_ok());
    assert!(runtime.require_parental_playback(&parent).is_err());
    let prepared = kiosk_source::prepare(&runtime, &app, Some("Attached film"), true).unwrap();
    assert!(kiosk_source::authorize_designation(&mut runtime, &app, &prepared).is_err());
    assert_quiet(&runtime);
}

#[test]
fn kiosk_off_uses_original_parent_for_attached_playback_policy() {
    let _serial = plx_base::testlock::serial();
    let mut app = attached_app(true);
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.settings.remove(stored::KEY);
    let parent = runtime.saved.profiles.profiles[0].clone();
    let id = runtime.ordered_ids()[1].clone();
    let (_, effective, _, favorite) = runtime
        .prepare_play_request(PlaybackRequest::ordinary(&id, 0))
        .unwrap();
    assert_eq!(effective.kind, ProviderKind::VPortal);
    let policy = runtime
        .attachment_policy_profile(&effective, favorite.as_ref())
        .unwrap();
    assert_eq!(policy, parent);
    assert!(runtime.require_parental_playback(&policy).is_ok());
    let tv = &runtime.saved.favorites.entries()[0];
    assert_eq!(
        runtime
            .attachment_policy_profile(&parent, Some(tv))
            .unwrap(),
        parent,
    );
    assert_eq!(
        runtime.attachment_policy_profile(&parent, None).unwrap(),
        parent,
    );
    assert_quiet(runtime);
}

struct RouteReset(u8);
impl RouteReset {
    fn new() -> Self {
        Self(plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Idle))
    }
}
impl Drop for RouteReset {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
        plx_media::player::restore_state_for_test(self.0);
    }
}

/// Only the native completion is modeled; the route/identity and held request
/// are real. This neither runs a decoder nor claims device playback evidence.
fn install_attached_playing(runtime: &mut Runtime, app: &mut App, favorite: &Favorite) {
    let request = PlaybackRequest::ordinary(&format!("favorite:{}", favorite.identity()), 0);
    runtime.resume_source_work();
    runtime.play_request_with_policy(app, request).unwrap();
    kiosk_capture::explicit_admitted(runtime);
    let request = runtime.play_target.as_ref().unwrap().clone();
    let receipt = runtime.play_attachment.clone().unwrap();
    let effective = receipt.effective_profile().unwrap();
    let entry = CatalogEntry {
        synopsis: None,
        live: None, id: format!("{}:film:42", effective.id), title: "Resolved film title".into(),
        category: "Films/Family".into(), description: String::new(), poster: String::new(),
        media: MediaLocator::VPortal { request: json!({"id":"42"}) },
        duration_ms: Some(10_000), provider_id: Some("42".into()), provider_id_field: Some("id".into()),
    };
    let info = plx_media::ff::inspect::MediaInfo {
        hls_playback: None,
        tracks: Default::default(), resolved_source: None,
        video_codec: "h264".into(), audio_codec: "aac".into(), width: 1280, height: 720,
        fps: 24.0, duration_ns: 10_000_000_000, audio_channels: 2,
        audio_sample_rate: 48_000, audio_bit_depth: 16, dovi: Default::default(),
    };
    playback::replace_player(&mut app.player, &mut app.adapters.player,
        &mut app.repause_at, &mut app.refresh_hubs, &mut app.pages);
    plx_media::route::reset_player_control_for_test(&app.player.session);
    plx_media::route::request_external_source(&mut app.player.session, &info,
        plx_media::ff::ExternalSource::Http("https://media.fixture.invalid/film.mp4".into()),
        &favorite.title, "Attached fixture", plx_platform::devcaps::assumed_shared()).unwrap();
    let source = plx_media::route::external_source(&app.player.session).unwrap();
    let start = plx_media::route::pending_route_start().unwrap();
    let attempt = plx_media::route::claim_route_start_attempt(start).unwrap();
    assert!(plx_media::route::settle_route_start(&mut app.player.session, attempt,
        plx_media::route::RouteStartResult::Started));
    runtime.playing_attachment = Some(receipt);
    runtime.play_target = None;
    runtime.play_attachment = None;
    runtime.resolver_next = None;
    runtime.pending_play = None;
    runtime.playing = Some(Playing {
        channel_capture: None, archive: None, kiosk_selection: request.kiosk_selection,
        transient_owner: None, local_selection: None,
        identity: plx_media::route::playback_identity(&app.player.session),
        profile: effective, entry, selection_id: request.id,
        source: Some(source), advance_on_failure: false, requires_server_lease: false,
    });
    plx_media::player::swap_state_for_test(plx_media::player::PlaybackState::Playing);
    assert!(playing_matches(runtime.playing.as_ref().unwrap(), app));
}

#[test]
fn armed_observed_attachment_pins_original_parent_after_global_favorite_removal() {
    let _serial = plx_base::testlock::serial();
    let _route = RouteReset::new();
    let mut app = attached_app(false);
    let mut runtime = app.ott.take().unwrap();
    let favorite = attached_favorite(&runtime.saved.profiles.profiles[0]);
    let receipt = runtime.attachment_for(&favorite).unwrap().unwrap();
    runtime.saved.settings.insert(stored::KEY.into(), Stored::armed(Origin {
        subprofile: None, ..favorite.origin.clone()
    }, true).unwrap().encode().unwrap());
    install_attached_playing(&mut runtime, &mut app, &favorite);
    let first = kiosk_source::observe(&runtime, &app).unwrap().unwrap();
    assert_eq!(first.favorite.identity(), favorite.identity());
    assert_eq!(first.favorite.title, "Resolved film title");
    assert_ne!(first.favorite.title, favorite.title);
    runtime.saved.favorites.toggle(favorite.clone()).unwrap();
    let observed = kiosk_source::observe(&runtime, &app).unwrap().unwrap();
    assert_eq!(observed.favorite.identity(), favorite.identity(),
        "the installed attachment retains its original portable parent identity");
    assert_eq!(observed.favorite.origin, favorite.origin);
    assert_eq!(observed.favorite.locator, favorite.locator);
    assert_eq!(observed.favorite.title, "Resolved film title");
    let prepared = kiosk_source::capture_prepared(&runtime, &observed, true).unwrap();
    kiosk_source::authorize_designation(&mut runtime, &app, &prepared).unwrap();
    let candidate = kiosk_source::save_candidate(&runtime, &prepared).unwrap();
    let pin = stored::read(&candidate.settings).unwrap().unwrap();
    assert_eq!(pin.origin(), &favorite.origin);
    assert!(pin.attachment_binding() == Some(&receipt.kiosk_binding().unwrap()));
    assert!(matches!(pin.target(), stored::TargetRef::Item(entry)
        if entry.identity() == favorite.identity() && entry.title == "Resolved film title"));
    assert_eq!(candidate.favorites, runtime.saved.favorites);
    assert_eq!(candidate.profiles, runtime.saved.profiles);
    assert!(!runtime.saving);
    assert!(!runtime.kiosk_capture.pending());
    let original = runtime.saved.clone();
    replace_source(&mut runtime, Replacement::Link);
    runtime.saved = original;
    runtime.observe_m3u_attachments();
    assert!(kiosk_source::observe(&runtime, &app).is_err(),
        "old installed media cannot use a newly issued equal-looking attachment receipt");
}

#[test]
fn attached_pin_admission_rejects_other_item_category_and_projected_source() {
    let _serial = plx_base::testlock::serial();
    let mut app = attached_app(false);
    let (_, candidate) = prepared_attachment(&mut app, "2");
    let mut runtime = cold_runtime(candidate);
    let request = cold_request(&mut runtime, &app);
    let (request, effective, _, favorite) = runtime.prepare_play_request(request).unwrap();
    let favorite = favorite.unwrap();
    for which in ["item", "category", "profile"] {
        let mut other = favorite.clone();
        let mut profile = effective.clone();
        match which {
            "item" => other.locator = Locator::VPortal { id_field: "id".into(), id: "43".into() },
            "category" => other.origin.subprofile = Some("Different folder".into()),
            _ => profile.source = "portal::[key:other]https://other.fixture.invalid/api/".into(),
        }
        assert!(kiosk_source::admit_bound(&runtime, &app, kiosk_source::BoundCandidate {
            request: &request, profile: &profile, entry: None, favorite: Some(&other),
        }).is_err(), "{which}");
    }
    assert_quiet(&runtime);
}

#[test]
fn media_only_m3u_parent_uses_same_attached_pin_contract() {
    let _serial = plx_base::testlock::serial();
    let mut app = attached_app(false);
    let runtime = app.ott.as_mut().unwrap();
    runtime.saved.profiles.profiles[0].source = LINK.into();
    runtime.saved.validate().unwrap();
    runtime.observe_m3u_attachments();
    let (_, candidate) = prepared_attachment(&mut app, "Attached film");
    let mut runtime = cold_runtime(candidate);
    let request = cold_request(&mut runtime, &app);
    let (_, effective, _, favorite) = runtime.prepare_play_request(request).unwrap();
    assert_eq!(effective.kind, ProviderKind::VPortal);
    assert_eq!(effective.source, LINK);
    assert_eq!(favorite.unwrap().origin.provider, "m3u");
    assert_eq!(runtime.saved.profiles.profiles.len(), 1);
    assert_quiet(&runtime);
}
