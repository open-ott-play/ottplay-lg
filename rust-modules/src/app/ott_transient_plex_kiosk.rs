//! Kiosk views of the already accepted Plex queue. No queue, cursor or playback
//! owner is stored here. Explicit snapshots allocate; frame-time pin checks borrow.
use super::*;
use crate::app::ott_kiosk_stored::PinEntries;
use crate::app::ott_runtime::kiosk_source::QueuePin;
use plx_data::ott::favorites::ENTRY_COUNT;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlaybackProof {
    Healthy,
    Current,
    Transition,
}

/// `target` permits only the caller's exact pending occurrence during admission.
/// It never admits an unaccepted restore candidate; that has a separate purpose.
fn owned<'a>(
    runtime: &'a Runtime,
    app: &App,
    target: Option<&str>,
    proof: PlaybackProof,
) -> Result<Option<&'a PlexSession>, &'static str> {
    let Some(s) = session(runtime) else {
        return Ok(None);
    };
    if runtime.transient.owns_playback()
        || plx_media::route::play_pending()
        || runtime.play_target.as_ref().is_some_and(|request| {
            target != Some(request.id.as_str())
                || request.transient_owner != Some(OwnerToken(s.queue.owner().0))
        })
    {
        return Err("Queue is busy");
    }
    let playing = runtime.playing.as_ref().ok_or("Queue is unavailable")?;
    if playing_superseded(playing, app) {
        return Err("Queue changed");
    }
    if s.queue.playback().is_none()
        && playing.transient_owner == Some(OwnerToken(s.queue.owner().0))
        && playing.selection_id.starts_with("favorite:")
        && playing_matches(playing, app)
    {
        return Ok(None);
    }
    let phase = plx_media::player::state(&app.player.session);
    if s.queue.stopped()
        || !s.native_matches(playing, app)
        || (proof != PlaybackProof::Transition && s.queue.playback().is_none())
        || (proof != PlaybackProof::Transition
            && !(proof == PlaybackProof::Current
                && phase == plx_media::player::PlaybackState::Error)
            && !matches!(
                phase,
                plx_media::player::PlaybackState::Playing
                    | plx_media::player::PlaybackState::Paused
                    | plx_media::player::PlaybackState::Buffering
                    | plx_media::player::PlaybackState::Seeking
            ))
    {
        return Err("Queue is unavailable");
    }
    if s.profile.source != s.authority.machine_id
        || !s.profile.enabled
        || !runtime
            .saved
            .profiles
            .profiles
            .iter()
            .any(|p| p == &s.profile)
        || runtime
            .saved
            .profiles
            .profiles
            .iter()
            .filter(|p| {
                p.enabled && p.kind == ProviderKind::Plex && p.source == s.authority.machine_id
            })
            .count()
            != 1
        || !plx_plex::plex::client_for(s.authority.server)
            .is_some_and(|client| s.authority.matches(client))
    {
        return Err("Queue source is unavailable or ambiguous");
    }
    shape(runtime, s)?;
    Ok(Some(s))
}

fn shape(runtime: &Runtime, s: &PlexSession) -> Result<(), &'static str> {
    let records = s.queue.records();
    if records.is_empty() || records.len() > ENTRY_COUNT {
        return Err("Queue is too large");
    }
    if s.queue.index() >= records.len()
        || s.occurrences.len() != records.len()
        || runtime.catalog.entries.len() != records.len()
        || !runtime.catalog_complete
        || runtime.catalog_loading
        || s.receipt.as_ref().is_some_and(|receipt| receipt.truncated)
    {
        return Err("Queue is incomplete");
    }
    for (index, record) in records.iter().enumerate() {
        let Some(metadata) = s.metadata.get(s.occurrences[index]) else {
            return Err("Queue is incomplete");
        };
        let row = &runtime.catalog.entries[index];
        if metadata.rating_key != record.id
            || s.occurrence_index(&row.id) != Some(index)
            || !matches!(&row.media, MediaLocator::Plex {server_id,rating_key}
                if server_id == &s.authority.machine_id && rating_key == &record.id)
        {
            return Err("Queue changed");
        }
    }
    Ok(())
}

pub(in crate::app::ott_runtime) fn kiosk_queue(
    runtime: &Runtime,
    app: &App,
) -> Result<Option<QueuePin>, &'static str> {
    snapshot(runtime, app, PlaybackProof::Healthy)
}

/// Explicit status projection only. A current pinned queue keeps its public
/// identity during decoder failure, while creating a new pin remains Healthy.
pub(in crate::app::ott_runtime) fn kiosk_queue_for_status(
    runtime: &Runtime,
    app: &App,
) -> Result<Option<QueuePin>, &'static str> {
    snapshot(runtime, app, PlaybackProof::Current)
}

fn snapshot(
    runtime: &Runtime,
    app: &App,
    proof: PlaybackProof,
) -> Result<Option<QueuePin>, &'static str> {
    let Some(s) = owned(runtime, app, None, proof)? else {
        return Ok(None);
    };
    let bindings = selection::SelectionBindings::profiles_only(&runtime.saved.profiles);
    let mut entries = Vec::with_capacity(s.queue.records().len());
    for row in &runtime.catalog.entries {
        let entry = selection::capture_bound(
            &bindings,
            selection::SelectionOwner::Profile(&s.profile),
            row,
        )
        .map_err(|_| "Queue contains incompatible sources")?;
        if entries
            .first()
            .is_some_and(|first: &plx_data::ott::favorites::Favorite| first.origin != entry.origin)
        {
            return Err("Queue contains incompatible sources");
        }
        entries.push(entry);
    }
    let identity = runtime
        .playing
        .as_ref()
        .and_then(|p| p.identity.as_ref().cloned())
        .ok_or("Queue is unavailable")?;
    Ok(Some(QueuePin {
        owner: OwnerToken(s.queue.owner().0),
        entries,
        index: s.queue.index(),
        profile_fingerprint: profile_fingerprint(&s.profile),
        current_identity: identity,
    }))
}

pub(in crate::app::ott_runtime) fn kiosk_queue_is_current(
    runtime: &Runtime,
    app: &App,
    pin: &QueuePin,
) -> bool {
    kiosk_queue(runtime, app)
        .ok()
        .flatten()
        .is_some_and(|current| {
            current.owner == pin.owner
                && current.index == pin.index
                && current.current_identity == pin.current_identity
                && current.profile_fingerprint == pin.profile_fingerprint
                && current.entries == pin.entries
        })
}

pub(in crate::app::ott_runtime) fn kiosk_repeat_all(
    runtime: &mut Runtime,
    app: &App,
    pin: &QueuePin,
) -> Result<(), &'static str> {
    if !kiosk_queue_is_current(runtime, app, pin) {
        return Err("Queue changed");
    }
    let s = session_mut(runtime).ok_or("Queue changed")?;
    s.queue
        .set_kiosk_repeat(s.queue.owner(), true)
        .map_err(|_| "Queue changed")
}

fn pins_match(s: &PlexSession, pins: PinEntries<'_>) -> bool {
    pins.len() == s.queue.records().len()
        && pins
            .iter()
            .zip(s.queue.records())
            .all(|(pin, record)| pin.matches_plex(&s.authority.machine_id, &record.id))
}

/// Durable pin checks use identity, not a retained snapshot's old playback ID.
/// A legitimate queue advance therefore stays pinned without cloning Favorites.
pub(in crate::app::ott_runtime) fn kiosk_queue_matches_current(
    runtime: &Runtime,
    app: &App,
    pins: PinEntries<'_>,
) -> bool {
    // Decoder failure does not release an accepted queue's identity or token.
    // Keep that owner through ordinary failure reconciliation so the shared
    // kiosk watchdog can retry it. Capturing a new pin still requires Healthy.
    owned(runtime, app, None, PlaybackProof::Current)
        .ok()
        .flatten()
        .is_some_and(|s| pins_match(s, pins))
}

pub(in crate::app::ott_runtime) fn kiosk_queue_allows(
    runtime: &Runtime,
    app: &App,
    id: &str,
    pins: PinEntries<'_>,
) -> bool {
    // EOF consumes the previous queue playback token before requesting its next
    // occurrence. Native ownership plus the complete pin still authorize that
    // exact transition even after stop_bufferfeed sets Idle. This is membership,
    // not a claim that the old source is still playing; a borrowed Favorite
    // never satisfies native_matches.
    owned(runtime, app, Some(id), PlaybackProof::Transition)
        .ok()
        .flatten()
        .is_some_and(|s| s.occurrence_index(id).is_some() && pins_match(s, pins))
}
