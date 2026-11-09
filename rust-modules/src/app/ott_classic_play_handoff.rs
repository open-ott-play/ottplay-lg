//! Presentation of an admitted UI request while its source is being resolved.
//! This receipt never grants playback authority or changes catalogue focus.
use super::*;

#[derive(Default)]
pub(super) struct State(Option<Box<Pending>>);
struct Pending {
    owner: InstanceId,
    page: &'static str,
    catalog: u64,
    generation: u64,
    request: PlaybackRequest,
    profile: ProviderProfile,
    admitted: Option<plx_media::route::PlaybackIdentity>,
}

fn classic(runtime: &Runtime) -> bool {
    runtime.interface_mode() == plx_machine::machine::UiMode::Classic
}

pub(super) fn begin(runtime: &mut Runtime, app: &App) {
    if !classic(runtime) || app.bridge.ui_mode() != plx_machine::machine::UiMode::Classic
        || !runtime.loaded || runtime.persistence_uncertain
        || !matches!(runtime.page, "channels" | "favorites" | "guide" | "guide-programme")
    { return; }
    let Some(owner) = runtime.reply.filter(|owner| current_ui_owner(app) == Some(*owner)) else { return; };
    let Some(request) = runtime.play_target.clone() else { return; };
    let Some(profile) = runtime.pending_play_profile().cloned() else { return; };
    runtime.classic_play_handoff.0 = Some(Box::new(Pending {
        owner, page: runtime.page, catalog: runtime.generation,
        generation: runtime.play_generation, request, profile, admitted: None,
    }));
}

pub(super) fn selected(runtime: &Runtime) -> Option<String> {
    let pending = runtime.classic_play_handoff.0.as_ref()?;
    if !classic(runtime) || !runtime.loaded || runtime.persistence_uncertain
        || runtime.reply != Some(pending.owner) || runtime.page != pending.page
        || runtime.play_generation != pending.generation
    { return None; }
    if let Some(identity) = &pending.admitted {
        let playing = runtime.playing.as_ref()?;
        if playing.identity.as_ref() != Some(identity)
            || playing.profile != pending.profile || playing.selection_id != pending.request.id
        { return None; }
    } else if runtime.generation != pending.catalog
        || runtime.play_target.as_ref() != Some(&pending.request)
        || runtime.pending_play_profile() != Some(&pending.profile)
    { return None; }
    Some(pending.request.id.clone())
}

/// Retain the cover during the real Player push; taking the completed target
/// must not expose a one-frame catalogue between admission and navigation.
pub(super) fn completed(runtime: &mut Runtime, generation: u64, success: bool) {
    let Some(pending) = runtime.classic_play_handoff.0.as_ref() else { return; };
    if pending.generation != generation { return; }
    let identity = runtime.playing.as_ref().filter(|_| success)
        .filter(|playing| playing.profile == pending.profile
            && playing.selection_id == pending.request.id)
        .and_then(|playing| playing.identity.clone());
    if let Some(identity) = identity {
        runtime.classic_play_handoff.0.as_mut().expect("checked receipt").admitted = Some(identity);
    } else {
        runtime.classic_play_handoff = Default::default();
    }
}

pub(super) fn back(runtime: &mut Runtime, app: &App, action: &OttAction)
    -> Option<Result<(), &'static str>>
{
    if !matches!(action, OttAction::Back) || selected(runtime).is_none()
        || runtime.classic_play_handoff.0.as_ref()?.admitted.is_some()
    { return None; }
    if let Err(error) = input_policy::navigation(runtime) { return Some(Err(error)); }
    // Cancel only this attempt. No catalogue generation, row selection or
    // browse bookmark is rewritten, and a late resolver loses its generation.
    runtime.cancel_play_request();
    runtime.playback_epoch = runtime.playback_epoch.wrapping_add(1);
    if let Some(playing) = runtime.playing.as_ref().filter(|p| playing_matches(p, app)) {
        runtime.continuity.retain_selection(runtime.generation, &playing.selection_id);
    } else {
        runtime.continuity.stop(runtime.generation);
    }
    runtime.status.clear();
    Some(Ok(()))
}

/// Classic RETURN closes a catalogue to its actual installed video. Changing
/// presentation never resolves or restarts a source and cannot adopt an old,
/// untagged decoder or a pending replacement.
pub(super) fn close_catalog(runtime: &mut Runtime, app: &mut App) -> bool {
    if !classic(runtime) || app.bridge.ui_mode() != plx_machine::machine::UiMode::Classic
        || !matches!(runtime.page, "channels" | "favorites" | "categories")
        || runtime.setup.is_some() || runtime.edit_session.is_some()
        || runtime.play_target.is_some() || runtime.pending_play.is_some()
        || runtime.reply != current_ui_owner(app) || !matches!(app.route(), AppArg::Ott)
        || app.pages.surface_up() || app.pages.has_pending_navigation()
        || app.pages.nav.tabs.stack.is_pending() || app.pages.nav.suspended
        || !playback_allowed(app) || !app.adapters.player.is_live()
        || app.decoder_parked || app.player.lifecycle.awaiting_load()
        || !plx_media::route::has_url(&app.player.session)
        || plx_media::route::play_pending() || retained_request_replaces_installed(app)
        || plx_appkit::player_hud::transport_hidden(&app.player.session)
        || runtime.playing.as_ref().is_none_or(|playing|
            playing.identity.is_none() || !playing_matches(playing, app))
    { return false; }
    playback::enter_player(&mut app.pages, &mut app.bridge, playback::Origin::Here, None);
    runtime.reply = None;
    runtime.native_catalog = None;
    true
}
