//! Presentation while an accepted OTT target is not yet a native route.
//! Never clear media errors or grant playback authority from a display signal.
use super::*;

fn profile_current(
    runtime: &Runtime,
    profile: &ProviderProfile,
    request: Option<&PlaybackRequest>,
) -> bool {
    if !profile.enabled {
        return false;
    }
    if runtime.effective_profile() == Some(profile)
        || runtime
            .saved
            .profiles
            .profiles
            .iter()
            .any(|saved| saved.enabled && saved == profile)
    {
        return true;
    }
    if runtime.play_attachment.as_ref().is_some_and(|receipt| {
        runtime.m3u_attachments.is_current(receipt)
            && receipt.effective_profile().ok().as_ref() == Some(profile)
    }) {
        return true;
    }
    request
        .and_then(|request| request.local_selection.as_ref())
        .and_then(|binding| runtime.current_local_root(binding))
        .is_some_and(|root| transient::source_profile(root).ok().as_ref() == Some(profile))
}

pub(super) fn pending(app: &App) -> bool {
    if !app.running
        || !playback_allowed(app)
        || app.player.pending_exit.is_some()
        || !matches!(app.route(), AppArg::Player)
    {
        return false;
    }
    let Some(runtime) = app.ott.as_ref().filter(|runtime| {
        runtime.loaded && !runtime.persistence_uncertain && runtime.play_work_allowed()
    }) else {
        return false;
    };
    if let Some(request) = runtime.play_target.as_ref() {
        let Some(profile) = runtime.pending_play_profile() else {
            return false;
        };
        if runtime.play_cancel.load(Ordering::Acquire)
            || !runtime
                .play_ticket
                .is_some_and(plx_media::route::play_request_is_current)
            || !runtime.pending_attachment_current()
            || !transient::target_current(runtime)
            || !profile_current(runtime, profile, Some(request))
            || request.continuation.as_ref().is_some_and(|identity| {
                !plx_media::route::playback_identity_matches(&app.player.session, identity)
            })
        {
            return false;
        }
        // Remote requests can expire before the worker returns. The normal
        // completion path still owns cancellation and the real failure result.
        return runtime.pending_play.as_ref().is_none_or(|invocation| {
            invocation.deadline_ms > runtime.now_ms()
                && runtime.remote.invocation_is_current(invocation)
                && commands::pending_catalog_current(runtime)
        });
    }
    // Only the exact live-source recovery receipt can cover a transient error
    // during a bounded cooldown. An exhausted budget, Pause or source change
    // immediately restores the ordinary terminal-failure presentation.
    if lifecycle::viewer_paused() {
        return false;
    }
    let Some(retry) = runtime.continuity.pending_retry() else {
        return false;
    };
    let Some(failed) = failed_playback::current(runtime, app) else {
        return false;
    };
    retry.generation == runtime.generation
        && retry.after == failed.selection_id
        && failed.archive.is_none()
        && guide_ui::is_live(&failed.profile, &failed.entry)
        && profile_current(runtime, &failed.profile, None)
}
