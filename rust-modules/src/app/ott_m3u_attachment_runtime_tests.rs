//! Runtime regressions without starting workers, loading protected state or
//! constructing a native player. Each operation below is the production helper.
//! Portal-only regressions below model a Saved round trip, not successful disk IO.
use super::*;
use plx_data::ott::favorites::{Locator, Origin};
use serde_json::json;

const LINK: &str = "portal::[key:fixture-secret]https://portal.fixture.invalid/api/";
const CHANGED_LINK: &str = "portal::[key:changed-secret]https://other.fixture.invalid/api/";

fn profile(id: &str) -> ProviderProfile {
    ProviderProfile {
        id: id.into(),
        name: "TV profile".into(),
        kind: ProviderKind::M3u,
        source: "https://tv.fixture.invalid/list.m3u".into(),
        source_alias: "Portable TV".into(),
        enabled: true,
    }
}

fn tv_entry() -> CatalogEntry {
    CatalogEntry {
        synopsis: None,
        live: None,
        id: "tv-row".into(),
        title: "Current TV channel".into(),
        category: "News".into(),
        description: String::new(),
        poster: String::new(),
        media: MediaLocator::Http {
            url: "https://tv.fixture.invalid/live.m3u8".into(),
        },
        duration_ms: None,
        provider_id: None,
        provider_id_field: None,
    }
}

fn playing(profile: ProviderProfile, entry: CatalogEntry, id: &str) -> Playing {
    Playing { channel_capture: None,
        archive: None,
        kiosk_selection: None,
        transient_owner: None,
        local_selection: None,
        identity: None,
        profile,
        entry,
        selection_id: id.into(),
        source: None,
        advance_on_failure: false,
        requires_server_lease: false,
    }
}

fn fixture() -> Runtime {
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
    runtime.write_permit.revoke();
    runtime.loaded = true;
    runtime.status = "Current TV channel".into();
    runtime.page = "channels";
    let parent = profile("m3u-1");
    runtime.saved.profiles.profiles = vec![parent.clone()];
    runtime.saved.profiles.active_profile = Some(parent.id.clone());
    runtime.saved.category = "News".into();
    runtime
        .saved
        .settings
        .insert("unrelated".into(), json!("preserved"));
    runtime.catalog.entries = vec![tv_entry()];
    runtime.catalog_complete = true;
    runtime.selected = Some("tv-row".into());
    runtime.playing = Some(playing(parent, tv_entry(), "tv-row"));
    runtime
}

fn attached_fixture() -> Runtime {
    let mut runtime = fixture();
    let candidate = runtime
        .attachment_candidate(
            &runtime.saved.profiles.profiles[0],
            LINK,
            Some("attached-1"),
        )
        .unwrap();
    runtime.saved = candidate;
    runtime.observe_m3u_attachments();
    runtime
}

fn favorite(runtime: &Runtime) -> Favorite {
    Favorite {
        title: "Saved film".into(),
        origin: Origin {
            provider: "m3u".into(),
            profile: runtime.saved.profiles.profiles[0].name.clone(),
            source_alias: runtime.saved.profiles.profiles[0].name.clone(),
            subprofile: Some("Films/Family".into()),
        },
        locator: Locator::VPortal {
            id_field: "id".into(),
            id: "42".into(),
        },
    }
}

fn add_favorite(runtime: &mut Runtime) -> (Favorite, String) {
    let favorite = favorite(runtime);
    let id = format!("favorite:{}", favorite.identity());
    runtime.saved.favorites.toggle(favorite.clone()).unwrap();
    (favorite, id)
}

fn assert_no_work_started(runtime: &Runtime) {
    assert!(!runtime.saving);
    assert!(!runtime.resolver_busy);
    assert!(!runtime.catalog_loading);
    assert!(runtime.catalog_next.is_none());
    assert!(runtime.resolver_next.is_none());
    assert!(runtime.metadata_next.is_none());
    assert!(runtime.pending_play.is_none());
    assert!(runtime.reply.is_none());
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

fn assert_tv_unchanged(runtime: &Runtime) {
    assert_eq!(runtime.catalog.entries, vec![tv_entry()]);
    assert_eq!(
        runtime.saved.profiles.active_profile.as_deref(),
        Some("m3u-1")
    );
    assert_eq!(runtime.provider, ProviderKind::M3u);
    assert_eq!(runtime.saved.category, "News");
    assert_eq!(runtime.selected.as_deref(), Some("tv-row"));
    assert_eq!(runtime.page, "channels");
    let current = runtime.playing.as_ref().unwrap();
    assert_eq!(current.profile, profile("m3u-1"));
    assert_eq!(current.entry, tv_entry());
    assert_eq!(current.selection_id, "tv-row");
    assert!(runtime.playing_attachment.is_none());
}

fn seed_owned_pending_work(runtime: &mut Runtime) -> (Arc<AtomicBool>, Arc<AtomicBool>, u64, u64) {
    let (favorite, id) = add_favorite(runtime);
    let receipt = runtime.attachment_for(&favorite).unwrap().unwrap();
    let effective = receipt.effective_profile().unwrap();
    runtime.play_generation = 41;
    runtime.play_cancel = Arc::new(AtomicBool::new(false));
    runtime.play_target = Some(PlaybackRequest::ordinary(&id, 0));
    runtime.play_attachment = Some(receipt.clone());
    runtime.resolver_next = Some(ResolveRequest {
        local_read: None,
        archive: None,
        generation: runtime.play_generation,
        profile: effective.clone(),
        entry: None,
        favorite: Some(favorite.clone()),
        local_proof: None,
        cancel: runtime.play_cancel.clone(),
        plex: None,
        plex_choice: None,
    });
    runtime.metadata_generation = 51;
    runtime.metadata_cancel = Arc::new(AtomicBool::new(false));
    runtime.metadata_target = Some((receipt.parent().id.clone(), id.clone()));
    runtime.metadata_attachment = Some(receipt);
    runtime.metadata_next = Some((effective, MetadataSelection::Favorite(favorite, None)));
    runtime.favorite_preview = Some((id, tv_entry()));
    (
        runtime.play_cancel.clone(),
        runtime.metadata_cancel.clone(),
        runtime.play_generation,
        runtime.metadata_generation,
    )
}

#[test]
fn startup_observation_and_attachment_candidate_do_not_fetch_or_replace_tv() {
    let mut runtime = fixture();
    runtime.observe_m3u_attachments();
    assert!(runtime.m3u_attachments.context_revision() > 0);
    let saved = serde_json::to_value(&runtime.saved).unwrap();
    let candidate = runtime
        .attachment_candidate(
            &runtime.saved.profiles.profiles[0],
            LINK,
            Some("attached-1"),
        )
        .unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    assert!(runtime.m3u_attachments.capture("m3u-1").is_none());
    assert_no_work_started(&runtime);
    assert_tv_unchanged(&runtime);
    runtime.saved = candidate;
    runtime.observe_m3u_attachments();
    assert!(runtime.m3u_attachments.capture("m3u-1").is_some());
    assert_eq!(runtime.saved.settings["unrelated"], json!("preserved"));
    assert_no_work_started(&runtime);
    assert_tv_unchanged(&runtime);
}

#[test]
fn prepare_nested_favorite_projects_vportal_but_keeps_parent_policy_and_tv() {
    let mut runtime = attached_fixture();
    let (favorite, id) = add_favorite(&mut runtime);
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let request = PlaybackRequest::ordinary(&id, 1200);
    let (prepared, effective, entry, bound) =
        runtime.prepare_play_request(request.clone()).unwrap();
    assert_eq!(prepared, request);
    assert_eq!(effective.kind, ProviderKind::VPortal);
    assert_eq!(effective.id, "attached-1");
    assert_eq!(effective.source, LINK);
    assert!(entry.is_none());
    assert_eq!(bound, Some(favorite.clone()));
    assert_eq!(
        runtime
            .attachment_policy_profile(&effective, Some(&favorite))
            .unwrap(),
        profile("m3u-1")
    );
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert_eq!(runtime.saved.profiles.profiles.len(), 1);
    assert_no_work_started(&runtime);
    assert_tv_unchanged(&runtime);
}

#[test]
fn malformed_candidate_preserves_saved_source_and_current_authority() {
    let runtime = attached_fixture();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let receipt = runtime.m3u_attachments.capture("m3u-1").unwrap();
    assert!(runtime
        .attachment_candidate(&runtime.saved.profiles.profiles[0], "portal::bad", None)
        .is_err());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(runtime.m3u_attachments.is_current(&receipt));
    assert_eq!(receipt.effective_profile().unwrap().source, LINK);
    assert_no_work_started(&runtime);
    assert_tv_unchanged(&runtime);
}

#[test]
fn candidate_refuses_stale_parent_without_retargeting_another_slot() {
    let mut runtime = attached_fixture();
    let captured = runtime.saved.profiles.profiles[0].clone();
    runtime.saved.profiles.profiles[0].source = "https://changed.fixture.invalid/list.m3u".into();
    runtime
        .saved
        .profiles
        .profiles
        .push(profile("different-slot"));
    runtime.observe_m3u_attachments();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    assert!(runtime
        .attachment_candidate(&captured, CHANGED_LINK, None)
        .is_err());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(runtime.m3u_attachments.capture("different-slot").is_none());
    assert_eq!(
        runtime
            .m3u_attachments
            .capture("m3u-1")
            .unwrap()
            .effective_profile()
            .unwrap()
            .source,
        LINK
    );
    assert_no_work_started(&runtime);
}

#[test]
fn parent_delete_prunes_only_its_candidate_and_retains_saved_favorites() {
    let mut runtime = attached_fixture();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let mut candidate = runtime.saved.clone();
    let favorite = favorite(&runtime);
    candidate.favorites.toggle(favorite.clone()).unwrap();
    candidate.profiles.profiles.clear();
    candidate.profiles.active_profile = None;
    runtime.prune_m3u_attachments(&mut candidate).unwrap();
    candidate.validate().unwrap();
    assert!(Registry::read(&candidate.settings, &candidate.profiles)
        .unwrap()
        .for_parent("m3u-1")
        .is_none());
    assert_eq!(candidate.favorites.entries(), &[favorite]);
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(runtime.m3u_attachments.capture("m3u-1").is_some());
    assert_no_work_started(&runtime);
    assert_tv_unchanged(&runtime);
}

#[test]
fn ordinary_save_without_attachments_does_not_insert_a_new_settings_key() {
    let mut runtime = fixture();
    let mut candidate = runtime.saved.clone();
    candidate
        .settings
        .insert("another-setting".into(), json!(true));
    let before = serde_json::to_value(&candidate).unwrap();
    runtime.prune_m3u_attachments(&mut candidate).unwrap();
    assert_eq!(serde_json::to_value(&candidate).unwrap(), before);
    assert!(!candidate.settings.contains_key(SETTINGS_KEY));
    assert_no_work_started(&runtime);
}

#[test]
fn runtime_observation_catches_parent_a_b_a_before_first_attachment() {
    let mut runtime = fixture();
    runtime.observe_m3u_attachments();
    let first = runtime.m3u_attachments.context_revision();
    let original = runtime.saved.profiles.profiles[0].clone();
    runtime.saved.profiles.profiles[0].name = "Temporary profile name".into();
    runtime.observe_m3u_attachments();
    let middle = runtime.m3u_attachments.context_revision();
    runtime.saved.profiles.profiles[0] = original;
    runtime.observe_m3u_attachments();
    assert!(middle > first);
    assert!(runtime.m3u_attachments.context_revision() > middle);
    assert!(runtime.m3u_attachments.capture("m3u-1").is_none());
    assert_no_work_started(&runtime);
    assert_tv_unchanged(&runtime);
}

#[test]
fn link_replacement_cancels_owned_resolver_and_metadata_but_not_tv() {
    let mut runtime = attached_fixture();
    let (play_cancel, metadata_cancel, play_generation, metadata_generation) =
        seed_owned_pending_work(&mut runtime);
    let receipt = runtime.play_attachment.clone().unwrap();
    let candidate = runtime
        .attachment_candidate(&runtime.saved.profiles.profiles[0], CHANGED_LINK, None)
        .unwrap();
    assert!(!play_cancel.load(Ordering::Acquire));
    assert!(!metadata_cancel.load(Ordering::Acquire));
    runtime.saved = candidate;
    runtime.observe_m3u_attachments();
    assert!(!runtime.m3u_attachments.is_current(&receipt));
    assert!(play_cancel.load(Ordering::Acquire));
    assert!(metadata_cancel.load(Ordering::Acquire));
    assert_ne!(runtime.play_generation, play_generation);
    assert_ne!(runtime.metadata_generation, metadata_generation);
    assert!(runtime.play_target.is_none());
    assert!(runtime.play_attachment.is_none());
    assert!(runtime.metadata_target.is_none());
    assert!(runtime.metadata_attachment.is_none());
    assert!(runtime.favorite_preview.is_none());
    assert_no_work_started(&runtime);
    assert_tv_unchanged(&runtime);
}

#[test]
fn unrelated_saved_settings_preserve_owned_queued_work_and_metadata() {
    let mut runtime = attached_fixture();
    let (play_cancel, metadata_cancel, play_generation, metadata_generation) =
        seed_owned_pending_work(&mut runtime);
    let receipt = runtime.play_attachment.clone().unwrap();
    let context = runtime.m3u_attachments.context_revision();
    runtime.saved.settings.insert("volume".into(), json!(65));
    runtime.observe_m3u_attachments();
    assert_eq!(runtime.m3u_attachments.context_revision(), context);
    assert!(runtime.m3u_attachments.is_current(&receipt));
    assert!(runtime.pending_attachment_current());
    assert!(!play_cancel.load(Ordering::Acquire));
    assert!(!metadata_cancel.load(Ordering::Acquire));
    assert_eq!(runtime.play_generation, play_generation);
    assert_eq!(runtime.metadata_generation, metadata_generation);
    assert!(runtime.resolver_next.is_some());
    assert!(runtime.metadata_next.is_some());
    assert!(runtime.metadata_target.is_some());
    assert!(runtime.favorite_preview.is_some());
    assert!(runtime.catalog_next.is_none());
    assert_tv_unchanged(&runtime);
}

#[test]
fn invalid_committed_registry_fails_closed_and_retires_queued_work() {
    let mut runtime = attached_fixture();
    let (play_cancel, metadata_cancel, _, _) = seed_owned_pending_work(&mut runtime);
    let receipt = runtime.play_attachment.clone().unwrap();
    runtime
        .saved
        .settings
        .insert(SETTINGS_KEY.into(), json!({"version":99,"entries":[]}));
    runtime.observe_m3u_attachments();
    assert_eq!(runtime.m3u_attachments.context_revision(), 0);
    assert!(!runtime.m3u_attachments.is_current(&receipt));
    assert!(play_cancel.load(Ordering::Acquire));
    assert!(metadata_cancel.load(Ordering::Acquire));
    assert!(!runtime.status.contains("fixture-secret"));
    assert!(!runtime.status.contains("https://"));
    assert_no_work_started(&runtime);
    assert_tv_unchanged(&runtime);
}

#[test]
fn metadata_and_policy_require_the_same_parent_and_effective_source() {
    let mut runtime = attached_fixture();
    let (favorite, id) = add_favorite(&mut runtime);
    let receipt = runtime.attachment_for(&favorite).unwrap().unwrap();
    let effective = receipt.effective_profile().unwrap();
    let attached_playing = playing(effective.clone(), tv_entry(), &id);
    runtime.metadata_attachment = Some(receipt);
    assert!(runtime.metadata_profile_matches(&attached_playing, "m3u-1"));
    assert!(!runtime.metadata_profile_matches(&attached_playing, "other-parent"));
    let mut forged = effective.clone();
    forged.source = CHANGED_LINK.into();
    assert!(runtime
        .attachment_policy_profile(&forged, Some(&favorite))
        .is_err());
    let candidate = runtime
        .attachment_candidate(&runtime.saved.profiles.profiles[0], CHANGED_LINK, None)
        .unwrap();
    runtime.saved = candidate;
    runtime.observe_m3u_attachments();
    assert!(!runtime.metadata_profile_matches(&attached_playing, "m3u-1"));
    assert!(runtime
        .attachment_policy_profile(&effective, Some(&favorite))
        .is_err());
    assert_no_work_started(&runtime);
    assert_tv_unchanged(&runtime);
}

#[test]
fn explicit_clear_keeps_unresolved_favorite_without_falling_back_to_tv() {
    let mut runtime = attached_fixture();
    let (favorite, id) = add_favorite(&mut runtime);
    let receipt = runtime.m3u_attachments.capture("m3u-1").unwrap();
    let candidate = runtime
        .attachment_candidate(&runtime.saved.profiles.profiles[0], "", None)
        .unwrap();
    runtime.saved = candidate;
    runtime.observe_m3u_attachments();
    assert!(!runtime.m3u_attachments.is_current(&receipt));
    assert!(runtime
        .prepare_play_request(PlaybackRequest::ordinary(&id, 0))
        .is_err());
    assert_eq!(runtime.saved.favorites.entries(), &[favorite]);
    let (_, tv_profile, entry, bound) = runtime
        .prepare_play_request(PlaybackRequest::ordinary("tv-row", 0))
        .unwrap();
    assert_eq!(tv_profile, profile("m3u-1"));
    assert_eq!(entry, Some(tv_entry()));
    assert!(bound.is_none());
    assert_no_work_started(&runtime);
    assert_tv_unchanged(&runtime);
}

#[test]
fn newly_ambiguous_parent_cancels_work_without_pretending_workers_drained() {
    let mut runtime = attached_fixture();
    let (play_cancel, metadata_cancel, play_generation, metadata_generation) =
        seed_owned_pending_work(&mut runtime);
    let receipt = runtime.play_attachment.clone().unwrap();
    runtime.resolver_busy = true;
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.saved.profiles.profiles.push(profile("second-m3u"));
    runtime.observe_m3u_attachments();
    assert!(
        runtime.m3u_attachments.is_current(&receipt),
        "the original source itself is unchanged"
    );
    assert!(play_cancel.load(Ordering::Acquire));
    assert!(metadata_cancel.load(Ordering::Acquire));
    assert_ne!(runtime.play_generation, play_generation);
    assert_ne!(runtime.metadata_generation, metadata_generation);
    assert!(runtime.play_attachment.is_none());
    assert!(runtime.play_target.is_none());
    assert!(runtime.resolver_next.is_none());
    assert!(runtime.metadata_attachment.is_none());
    assert!(runtime.metadata_target.is_none());
    assert!(runtime.metadata_next.is_none());
    assert!(runtime.favorite_preview.is_none());
    assert!(
        runtime.resolver_busy,
        "the retiring worker still owns its slot"
    );
    assert!(
        !runtime.metadata_done.load(Ordering::Acquire),
        "cancellation is not a drained receipt"
    );
    assert!(runtime.catalog_next.is_none());
    assert!(matches!(
        runtime.rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(matches!(
        runtime.metadata_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert_tv_unchanged(&runtime);
}

#[test]
fn completed_target_releases_pending_attachment_but_retains_playing_owner() {
    let mut runtime = attached_fixture();
    let (favorite, id) = add_favorite(&mut runtime);
    let receipt = runtime.attachment_for(&favorite).unwrap().unwrap();
    let request = PlaybackRequest::ordinary(&id, 1200);
    runtime.play_target = Some(request.clone());
    runtime.play_attachment = Some(receipt.clone());
    runtime.playing_attachment = Some(receipt.clone());
    assert!(runtime.pending_attachment_current());

    assert_eq!(runtime.take_completed_play_target(), Some(request));
    assert!(runtime.play_target.is_none());
    assert!(runtime.play_attachment.is_none());
    assert_eq!(runtime.playing_attachment.as_ref(), Some(&receipt));
    assert!(runtime.pending_attachment_current());
    assert!(
        runtime.launch_resolver().is_ok(),
        "idle launch has no pending source to invalidate"
    );
    assert_eq!(runtime.playing_attachment.as_ref(), Some(&receipt));
    assert_no_work_started(&runtime);
}

fn portal_only_fixture() -> Runtime {
    let mut runtime = fixture();
    runtime.entropy = Some([17; 16]);
    let original = runtime.saved.profiles.profiles[0].clone();
    let mut media = original.clone();
    media.source = LINK.into();
    let candidate = runtime.profile_candidate(media, Some(&original)).unwrap();
    candidate.validate().unwrap();
    runtime.saved = candidate;
    runtime.observe_m3u_attachments();
    runtime
}

#[test]
fn portal_only_source_editor_candidate_is_atomic_and_round_trips_as_m3u() {
    let mut runtime = fixture();
    runtime.entropy = Some([17; 16]);
    let (saved_favorite, _) = add_favorite(&mut runtime);
    let before = serde_json::to_value(&runtime.saved).unwrap();
    let original = runtime.saved.profiles.profiles[0].clone();
    let mut media = original.clone();
    media.source = LINK.into();
    let candidate = runtime.profile_candidate(media, Some(&original)).unwrap();
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
    assert!(runtime.m3u_attachments.capture(&original.id).is_none());
    assert_no_work_started(&runtime);

    // This checks serialization/admission only; no disk acknowledgment is faked.
    let restored: Saved = serde_json::from_slice(&serde_json::to_vec(&candidate).unwrap()).unwrap();
    restored.validate().unwrap();
    assert_eq!(restored.profiles.profiles.len(), 1);
    let parent = &restored.profiles.profiles[0];
    assert_eq!(parent.kind, ProviderKind::M3u);
    assert_eq!(parent.id, original.id);
    assert_eq!(parent.source, LINK);
    assert_eq!(parent.name, original.name);
    assert_eq!(parent.source_alias, original.source_alias);
    assert_eq!(restored.favorites.entries(), &[saved_favorite]);
    assert_eq!(restored.settings["unrelated"], json!("preserved"));
    let registry = Registry::read(&restored.settings, &restored.profiles).unwrap();
    assert_eq!(registry.for_parent(&parent.id).unwrap().project(parent).unwrap().source, LINK);
}

#[test]
fn portal_only_startup_finishes_empty_tv_catalog_without_fetching_the_portal() {
    let mut runtime = portal_only_fixture();
    runtime.playing = None;
    runtime.load_catalog();
    assert_eq!(runtime.provider, ProviderKind::M3u);
    assert!(runtime.catalog_complete);
    assert!(runtime.catalog.entries.is_empty());
    assert!(!runtime.catalog_loading);
    assert!(runtime.catalog_next.is_none());
    assert_no_work_started(&runtime);

    let (favorite, id) = add_favorite(&mut runtime);
    let (_, effective, entry, selected) = runtime.prepare_play_request(
        PlaybackRequest::ordinary(&id, 2400)).unwrap();
    assert_eq!(effective.kind, ProviderKind::VPortal);
    assert_eq!(effective.source, LINK);
    assert!(entry.is_none());
    assert_eq!(selected, Some(favorite.clone()));
    assert_eq!(runtime.attachment_policy_profile(&effective, Some(&favorite)).unwrap(),
        runtime.saved.profiles.profiles[0]);
    assert_no_work_started(&runtime);
}

#[test]
fn portal_only_attachment_change_updates_both_sources_without_changing_portable_identity() {
    let mut runtime = portal_only_fixture();
    let (favorite, id) = add_favorite(&mut runtime);
    let old = runtime.attachment_for(&favorite).unwrap().unwrap();
    let parent = runtime.saved.profiles.profiles[0].clone();
    let candidate = runtime.attachment_candidate(&parent, CHANGED_LINK, None).unwrap();
    candidate.validate().unwrap();
    assert_eq!(runtime.saved.profiles.profiles[0].source, LINK);
    assert!(runtime.m3u_attachments.is_current(&old));
    assert_eq!(candidate.profiles.profiles[0].source, CHANGED_LINK);
    assert_eq!(candidate.favorites.entries(), &[favorite.clone()]);
    assert_eq!(candidate.profiles.profiles.len(), 1);
    runtime.saved = candidate;
    runtime.observe_m3u_attachments();
    assert!(!runtime.m3u_attachments.is_current(&old));
    let new = runtime.attachment_for(&favorite).unwrap().unwrap();
    assert_eq!(old.attachment().id(), new.attachment().id());
    let (_, effective, _, selected) = runtime.prepare_play_request(
        PlaybackRequest::ordinary(&id, 0)).unwrap();
    assert_eq!(effective.source, CHANGED_LINK);
    assert_eq!(selected, Some(favorite));
    assert_no_work_started(&runtime);
}

#[test]
fn portal_only_clear_and_stale_source_editor_preserve_the_last_valid_source() {
    let mut runtime = portal_only_fixture();
    let parent = runtime.saved.profiles.profiles[0].clone();
    let before = serde_json::to_value(&runtime.saved).unwrap();
    assert!(runtime.attachment_candidate(&parent, "", None).is_err());
    assert!(runtime.attachment_candidate(&parent, "portal::bad", None).is_err());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);

    let current = runtime.attachment_candidate(&parent, CHANGED_LINK, None).unwrap();
    runtime.saved = current;
    runtime.observe_m3u_attachments();
    let after = serde_json::to_value(&runtime.saved).unwrap();
    let mut stale = parent.clone();
    stale.source = "https://tv.fixture.invalid/replaced.m3u".into();
    assert!(runtime.profile_candidate(stale, Some(&parent)).is_err());
    assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), after);
    assert_no_work_started(&runtime);
}

#[test]
fn portal_only_profile_removal_prunes_attachment_without_erasing_favorites() {
    let mut runtime = portal_only_fixture();
    let (favorite, _) = add_favorite(&mut runtime);
    let mut candidate = runtime.saved.clone();
    candidate.profiles.profiles.clear();
    candidate.profiles.active_profile = None;
    runtime.prune_m3u_attachments(&mut candidate).unwrap();
    candidate.validate().unwrap();
    assert!(Registry::read(&candidate.settings, &candidate.profiles).unwrap()
        .for_parent("m3u-1").is_none());
    assert_eq!(candidate.favorites.entries(), &[favorite]);
    assert!(runtime.m3u_attachments.capture("m3u-1").is_some());
    assert_no_work_started(&runtime);
}
