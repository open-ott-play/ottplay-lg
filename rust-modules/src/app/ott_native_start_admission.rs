//! Revalidate the actual OTT owner at the last native-start boundary.
//! Rebinding is metadata-only: no play_target, resolver or prepared media is
//! installed to reconstruct the current source and parental membership.
use super::*;

/// None means this is not the current OTT-owned native request. A recognized
/// owner which fails revalidation must never fall through to a legacy bypass.
pub(super) fn owned(runtime: &mut Runtime, app: &App) -> Option<Result<(), &'static str>> {
    let playing = runtime.playing.as_ref()?;
    let identity = playing.identity.as_ref()?;
    if !plx_media::route::playback_identity_matches(&app.player.session, identity) { return None; }
    let matches = if let Some(source) = playing.source.as_ref() {
        plx_media::route::playback_request_identity(&app.player.session).is_none()
            && plx_media::route::external_source(&app.player.session).as_ref() == Some(source)
    } else {
        matches!((&playing.entry.media,
            plx_media::route::playback_request_identity(&app.player.session)),
            (MediaLocator::Plex { server_id, rating_key }, Some((sid, key)))
                if key == rating_key && plx_plex::plex::id_of_machine(server_id) == Some(sid))
    };
    if !matches { return Some(Err("Native playback source changed before Load")); }
    if playing.requires_server_lease && !runtime.active_lease.as_ref().is_some_and(HeldLease::is_playback) {
        return Some(Err("Playback source lease expired before native Load"));
    }
    let mut request = PlaybackRequest::ordinary(&playing.selection_id, 0);
    request.archive = playing.archive.clone();
    request.kiosk_selection = playing.kiosk_selection.clone();
    request.transient_owner = playing.transient_owner;
    request.local_selection = playing.local_selection.clone();
    let profile = playing.profile.clone();
    let entry = playing.entry.clone();
    Some(revalidate(runtime, app, request, &profile, &entry))
}

fn revalidate(runtime: &mut Runtime, app: &App, expected: PlaybackRequest,
    profile: &ProviderProfile, entry: &CatalogEntry) -> Result<(), &'static str>
{
    parental_runtime::observe_session(runtime, app);
    if !runtime.loaded || runtime.persistence_uncertain || !app.running || !playback_allowed(app) {
        return Err("Native playback admission is unavailable");
    }
    profile_rename::guard(runtime, profile)?;
    managed::guard_profile(runtime, app, profile)?;
    classic_source::guard_profile(runtime, app, profile)?;
    let (request, current_profile, current, favorite) = runtime.prepare_play_request(expected.clone())?;
    if &current_profile != profile || request.id != expected.id
        || request.local_selection != expected.local_selection
        || request.transient_owner != expected.transient_owner
        || current.as_ref().is_some_and(|row| row.id != entry.id || row.media != entry.media)
    { return Err("Native playback source changed before Load"); }
    if let Some(receipt) = runtime.playing_attachment.as_ref() {
        if !runtime.m3u_attachments.is_current(receipt)
            || receipt.effective_profile().ok().as_ref() != Some(profile)
        { return Err("M3U media attachment changed before native Load"); }
    }
    // Source authority is independent of whether a PIN policy is enabled.
    // Managed accounts already validated their addressed read above; temporary
    // sessions keep the exact read/client captured by their existing owner.
    let local_temporary = if let Some(owner) = request.transient_owner {
        match runtime.transient.accepted.as_ref().filter(|session| session.owner() == owner) {
            Some(transient::TransientMedia::Local(session)) if session.profile == *profile
                && runtime.local_source_read_is_current(&session.read) => true,
            Some(transient::TransientMedia::Plex(session)) if session.profile == *profile
                && plx_plex::plex::client_for(session.authority.server)
                    .is_some_and(|client| session.authority.matches(client)) => false,
            _ => return Err("Temporary source authority changed before native Load"),
        }
    } else { false };
    if !local_temporary && !managed::owns_retained_profile(runtime, profile) {
        let _ = transient::parental_profile_authority(runtime, profile)?;
    }
    let policy_profile = runtime.attachment_policy_profile(profile, favorite.as_ref())?;
    if runtime.effective_profile() == Some(profile) {
        let mut rows = runtime.catalog.entries.iter().filter(|row| row.id == entry.id);
        let fresh = rows.next().cloned();
        if rows.next().is_some() { return Err("Native playback catalogue identity is ambiguous"); }
        if let Some(fresh) = fresh {
            if fresh.media != entry.media { return Err("Native playback media changed before Load"); }
            runtime.require_parental_entry(&policy_profile, Some(&fresh))?;
        }
    }
    runtime.require_parental_resolved(&policy_profile, profile, current.as_ref(), favorite.as_ref(), entry)?;
    // This repeats the existing pin/root admission with the actual retained
    // request fields, rather than manufacturing a new pending selection.
    kiosk_source::admit_bound(runtime, app, kiosk_source::BoundCandidate {
        request: &request, profile, entry: Some(entry), favorite: favorite.as_ref(),
    }).map_err(kiosk_source::SourceError::message)
}

/// Retire only the recognized failed owner. A newer resolver/selection keeps
/// its own cancellation ticket and may still finish; only the old playing
/// lease and attachment are retired in that case.
pub(super) fn retire_denied(runtime: &mut Runtime, app: &mut App,
    identity: &plx_media::route::PlaybackIdentity,
    ticket: plx_media::route::PlayRequestTicket)
{
    if !plx_media::route::play_request_is_current(ticket)
        || !plx_media::route::playback_identity_matches(&app.player.session, identity)
        || !runtime.playing.as_ref().is_some_and(|playing| playing.identity.as_ref() == Some(identity))
    { return; }
    if runtime.play_target.is_some() || runtime.pending_play.is_some()
        || runtime.transient.candidate.is_some()
    {
        runtime.release_active_lease();
        runtime.playing = None;
        runtime.resume_floor = None;
        runtime.changed();
    } else {
        runtime.retire_catalog_playback(app);
    }
    runtime.playing_attachment = None;
}
