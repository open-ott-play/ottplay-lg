//! Ephemeral first-choice receipt for a durably Armed kiosk. The existing
//! playback/navigation epochs identify intent; no clock/counter/store is added.
use super::kiosk_source::{self as source, SourceError};
use super::*;
use crate::app::ott_kiosk_stored::{self as stored, Stored, TargetRef};

#[derive(Clone)]
struct Intent {
    armed: Stored,
    play_generation: u64,
    playback_epoch: u64,
    navigation_epoch: u64,
    selection_id: String,
    source_fingerprint: String,
    attachment: Option<crate::app::ott_m3u_attachment::Receipt>,
}
#[derive(Default)]
pub(super) struct Capture {
    intent: Option<Intent>,
    writing: Option<Pending>,
}
struct Pending {
    target: Stored,
}
impl Capture {
    pub(super) fn pending(&self) -> bool {
        self.writing.is_some()
    }
    /// Only a pre-write intent may be discarded. A dispatched write must settle.
    pub(super) fn cancel_unwritten(&mut self) {
        self.intent = None;
    }
}

/// Call ONLY after a user/remote explicit selection has been accepted and has
/// obtained its existing play generation, never for boot, EOF, retry or resume.
pub(super) fn explicit_admitted(runtime: &mut Runtime) {
    if runtime.kiosk_capture.pending() {
        return;
    }
    runtime.kiosk_capture.intent = None;
    if !runtime.loaded || runtime.persistence_uncertain {
        return;
    }
    let Ok(Some(armed)) = stored::read(&runtime.saved.settings) else {
        return;
    };
    if !matches!(armed.target(), TargetRef::Armed) {
        return;
    }
    let Some(request) = runtime.play_target.as_ref() else {
        return;
    };
    let Ok((_, profile, _, favorite)) = runtime.prepare_play_request(request.clone()) else {
        return;
    };
    let Ok(policy_profile) = runtime.attachment_policy_profile(&profile, favorite.as_ref()) else {
        return;
    };
    if !source::arm_matches(&armed, &policy_profile) {
        return;
    }
    runtime.kiosk_capture.intent = Some(Intent {
        armed,
        play_generation: runtime.play_generation,
        playback_epoch: runtime.playback_epoch,
        navigation_epoch: runtime.navigation_epoch,
        selection_id: request.id.clone(),
        source_fingerprint: profile_fingerprint(&profile),
        attachment: runtime.play_attachment.clone(),
    });
}

fn matches_intent(
    intent: &Intent,
    pin: &Stored,
    generation: u64,
    playback: u64,
    navigation: u64,
    id: &str,
    source: &str,
) -> bool {
    intent.armed.same_target(pin)
        && intent.play_generation == generation
        && intent.playback_epoch == playback
        && intent.navigation_epoch == navigation
        && intent.selection_id == id
        && intent.source_fingerprint == source
}

/// Run after owner reconciliation in the existing frame tick. A resolved URL,
/// native request acceptance and a buffering state are not successful playback.
pub(super) fn tick(runtime: &mut Runtime, app: &App) -> Result<bool, SourceError> {
    if runtime.kiosk_capture.pending() {
        return Ok(false);
    }
    if !playback_allowed(app) || !runtime.source_work_allowed() {
        runtime.kiosk_capture.cancel_unwritten();
        return Ok(false);
    }
    // Retain the unwritten intent for a later tick; the ordinary owner/epoch
    // checks still run before any write once the track preparation settles.
    if commands::track_preparation_pending(runtime) {
        return Ok(false);
    }
    let Some(intent) = runtime.kiosk_capture.intent.as_ref().cloned() else {
        return Ok(false);
    };
    let current = stored::read(&runtime.saved.settings).map_err(|_| SourceError::Invalid)?;
    let epochs_match = current.as_ref().is_some_and(|pin| {
        intent.armed.same_target(pin)
            && runtime.play_generation == intent.play_generation
            && runtime.playback_epoch == intent.playback_epoch
            && runtime.navigation_epoch == intent.navigation_epoch
    });
    if !epochs_match {
        runtime.kiosk_capture.cancel_unwritten();
        return Ok(false);
    }
    if runtime.saving || runtime.persistence_uncertain {
        return Ok(false);
    }
    if plx_media::player::state(&app.player.session) != plx_media::player::PlaybackState::Playing
        || plx_media::player::transport::paused()
    {
        return Ok(false);
    }
    let Some(observed) = source::observe(runtime, app)? else {
        return Ok(false);
    };
    let pin = current.as_ref().ok_or(SourceError::Changed)?;
    if intent.attachment != observed.item.attachment || !matches_intent(
        &intent,
        pin,
        observed.play_generation,
        runtime.playback_epoch,
        runtime.navigation_epoch,
        &observed.item.id,
        &profile_fingerprint(&observed.item.profile),
    ) {
        runtime.kiosk_capture.cancel_unwritten();
        return Ok(false);
    }
    let prepared = source::capture_prepared(runtime, &observed, pin.strict())?;
    source::authorize_designation(runtime, app, &prepared).map_err(|_| SourceError::Unavailable)?;
    let saved = source::save_candidate(runtime, &prepared)?;
    // Enter pending only after the existing writer accepted this exact image.
    // No other source-changing admission may run until completion/uncertainty.
    runtime
        .save(saved, None, false)
        .map_err(|_| SourceError::Busy)?;
    runtime.kiosk_capture.writing = Some(Pending {
        target: prepared.stored().clone(),
    });
    runtime.kiosk_capture.intent = None;
    Ok(true)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Completion {
    Durable,
    NotApplied,
    Unknown,
}

/// Invoke after the common Saved/Uncertain publication, only for the writer
/// associated with Capture::pending. A deadline cannot erase this receipt.
pub(super) fn settled(runtime: &mut Runtime, app: &mut App, outcome: Completion) -> bool {
    let Some(pending) = runtime.kiosk_capture.writing.take() else {
        return false;
    };
    runtime.kiosk_capture.intent = None;
    match outcome {
        Completion::Durable => {
            // Common publication is authoritative BEFORE enforcement; a stale
            // source must not leave another stream playing outside a saved pin.
            if stored::read(&runtime.saved.settings)
                .ok()
                .flatten()
                .as_ref()
                != Some(&pending.target)
            {
                runtime.persistence_uncertain = true;
            }
            source::apply_durable_policy(runtime, app);
        }
        Completion::NotApplied => {}
        Completion::Unknown => {
            runtime.persistence_uncertain = true;
        }
    }
    true
}

#[cfg(test)]
#[path = "ott_kiosk_capture_tests.rs"]
mod tests;
