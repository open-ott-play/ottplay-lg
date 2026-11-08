//! A failed live channel retains list authority, never decoder authority.
//! No URLs or private source bindings leave this process-local receipt.
use super::*;

pub(super) struct FailedPlayback {
    playing: Playing,
    generation: u64,
    playback_epoch: u64,
    category: String,
    favorites: bool,
}

pub(super) fn capture(runtime: &Runtime, app: &App) -> Option<FailedPlayback> {
    let playing = runtime.playing.as_ref().filter(|playing| playing_matches(playing, app))?;
    if playing.archive.is_some() || !guide_ui::is_live(&playing.profile, &playing.entry)
        || playing.identity.is_none() { return None; }
    let (category, favorites) = runtime.view_selection();
    Some(FailedPlayback { playing: playing.clone(), generation: runtime.generation,
        playback_epoch: runtime.playback_epoch, category: category.into(), favorites })
}

/// Cheap ownership check used by per-frame publication after decoder cleanup.
/// Metadata/EPG enrichment is not a new source. Exact row binding is checked
/// only at an explicit command boundary, never by scanning the list each frame.
pub(super) fn current<'a>(runtime: &'a Runtime, app: &App) -> Option<&'a Playing> {
    let failed = runtime.failed_playback.as_ref()?;
    let playing = &failed.playing;
    if app.adapters.player.is_live() || runtime.playing.is_some()
        || runtime.play_target.is_some() || runtime.pending_play.is_some()
        || runtime.resolver_next.is_some() || runtime.generation != failed.generation
        || runtime.playback_epoch != failed.playback_epoch
        || runtime.view_selection() != (failed.category.as_str(), failed.favorites)
        || (!failed.favorites && runtime.effective_profile() != Some(&playing.profile))
        || plx_media::route::play_pending() || retained_request_replaces_installed(app)
        || plx_media::route::playback_identity(&app.player.session).as_ref()
            .is_some_and(|identity| Some(identity) != playing.identity.as_ref())
        || plx_media::route::external_source(&app.player.session).as_ref()
            .is_some_and(|source| Some(source) != playing.source.as_ref())
    { return None; }
    Some(playing)
}

pub(super) fn admissible<'a>(runtime: &'a Runtime, app: &App) -> Option<&'a Playing> {
    let playing = current(runtime, app)?;
    let (_, profile, entry, _) = runtime.prepare_play_request(
        PlaybackRequest::ordinary(&playing.selection_id, 0)).ok()?;
    if profile != playing.profile || entry.as_ref().is_some_and(|entry|
        entry.id != playing.entry.id || entry.media != playing.entry.media)
        || !commands::selected_ids(runtime).iter().any(|id| id == &playing.selection_id)
    { return None; }
    Some(playing)
}

pub(super) fn ready(runtime: &Runtime, app: &App) -> bool {
    current(runtime, app).is_some()
        && app.running && runtime.loaded && !runtime.persistence_uncertain
        && runtime.setup.is_none() && runtime.edit_session.is_none()
        && input_policy::navigation(runtime).is_ok()
        && matches!(app.route(), AppArg::Player) && bridge::player(&app.pages).is_some()
        && !app.pages.surface_up() && !app.pages.has_pending_navigation()
        && !app.pages.nav.tabs.stack.is_pending() && !app.pages.nav.tabs.stack.transition.in_flight()
        && !app.pages.nav.suspended && !app.decoder_parked
        && !app.player.lifecycle.awaiting_load() && playback_allowed(app)
        && !matches!(app.rec, super::super::recorder::Recplay::Replaying(_))
        && !app.bridge.is_controlled_replay()
}
