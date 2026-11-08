//! One source-bound list order for desktop and remote next/previous actions.
use super::*;

pub(super) fn target(runtime: &Runtime, app: &App, offset: i64) -> Result<String, &'static str> {
    let playing = runtime
        .playing
        .as_ref()
        .filter(|playing| playing_matches(playing, app))
        .or_else(|| failed_playback::admissible(runtime, app))
        .ok_or("The playing source is no longer current")?;
    if runtime.play_target.is_some() {
        return Err("Another video is already opening");
    }
    // An admitted Plex occurrence queue owns its order, including repeat mode.
    if let Some(current) = transient::plex_remote::step_id(runtime, 0) {
        if current.as_deref() != Some(playing.selection_id.as_str()) {
            return Err("The playing queue changed");
        }
        return transient::plex_remote::step_id(runtime, offset)
            .flatten()
            .ok_or("No next video is available");
    }
    let favorites = runtime.view_selection().1;
    if !favorites && runtime.effective_profile() != Some(&playing.profile) {
        return Err("The selected list belongs to another source");
    }
    let ids = commands::selected_ids(runtime);
    if !ids.iter().any(|id| id == &playing.selection_id) {
        return Err("The playing video is not in the selected list");
    }
    commands::step_in_list(&ids, Some(&playing.selection_id), offset).ok_or("No next video is available")
}

/// Shared fullscreen next/previous intent. Native Plex keeps its installed
/// playback owner; its admitted Classic list supplies the ordinary Play target.
pub(super) fn perform(runtime: &mut Runtime, app: &mut App, offset: i64)
    -> Result<(), &'static str>
{
    if !matches!(offset, -1 | 1) { return Err("Invalid playback direction"); }
    input_policy::navigation(runtime)?;
    if runtime.playing.is_none() && runtime.failed_playback.is_none() {
        let source = player_browse::native_identity(runtime, app)
            .ok_or("The playing source is no longer current")?;
        return player_browse::native_list_step(runtime, app, &source, offset);
    }
    let id = target(runtime, app, offset)?;
    input_policy::ui(runtime, &OttAction::Play(id.clone()))?;
    runtime.checkpoint(app, true);
    // A refused destination must not fall forward to unrelated media.
    runtime.play_with_policy(app, &id, false)?;
    runtime.cancel_setup();
    Ok(())
}
