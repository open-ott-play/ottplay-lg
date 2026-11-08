//! Native remote actions over the existing, source-owned OTT player.
//! The publication is a one-frame affordance; every request is checked again.
use super::*;
use plx_screens::registry::{OttPlayerAction as Action, OttPlayerOffer, OttPlayerRequest, PlayerReq};
use plx_machine::machine::InputOwner;

const CHANGED: &str = "Playback changed before continuation";

fn playing<'a>(runtime: &'a Runtime, app: &App) -> Option<&'a Playing> {
    if !player_browse::ready(app) || app.pages.surface_up()
        || !runtime.loaded || runtime.persistence_uncertain
        || runtime.setup.is_some() || runtime.edit_session.is_some()
        || input_policy::navigation(runtime).is_err()
    { return None; }
    runtime.playing.as_ref().filter(|playing| playing_matches(playing, app))
}

/// A typed local/portal list really owns this occurrence. A finite HTTP URL
/// alone does not turn television arrows into VOD seek controls.
fn sequence(runtime: &Runtime, playing: &Playing) -> bool {
    if playing.archive.is_some() { return false; }
    if matches!(playing.entry.media, MediaLocator::Plex { .. }) {
        return playing.profile.kind == ProviderKind::Plex;
    }
    if let Some(receipt) = runtime.playing_attachment.as_ref() {
        return runtime.m3u_attachments.is_current(receipt)
            && receipt.effective_profile().ok().as_ref() == Some(&playing.profile);
    }
    if let Some(local) = playing.local_selection.as_ref() {
        return runtime.current_local_root(local).is_some();
    }
    matches!(playing.profile.kind, ProviderKind::Local | ProviderKind::VPortal)
        && runtime.effective_profile() == Some(&playing.profile)
}

pub(super) fn offer(runtime: &Runtime, app: &App) -> Option<OttPlayerOffer> {
    if runtime.playing.is_none() {
        if app.pages.surface_up() || input_policy::navigation(runtime).is_err() { return None; }
        let source = player_browse::native_identity(runtime, app)?;
        let accept = if runtime.view_selection().1 || !lifecycle::viewer_paused() {
            Action::OpenCatalog
        } else { Action::TogglePause };
        return Some(OttPlayerOffer { source, seek_arrows: true, accept, guide: false });
    }
    let playing = playing(runtime, app)?;
    let source = playing.identity.clone()?;
    let live = guide_ui::is_live(&playing.profile, &playing.entry);
    // Preserve the original channel binding for an admitted archive occurrence.
    let guide = live || playing.archive.is_some();
    let seek_arrows = sequence(runtime, playing);
    let accept = if runtime.view_selection().1 {
        Action::OpenCatalog
    } else if (!live || playing.archive.is_some()) && lifecycle::viewer_paused() {
        Action::TogglePause
    } else if playing.archive.is_some() {
        Action::OpenGuide
    } else { Action::OpenCatalog };
    Some(OttPlayerOffer { source, seek_arrows, accept, guide })
}

fn action_allowed(offer: &OttPlayerOffer, action: Action) -> bool {
    match action {
        Action::SeekBackward | Action::SeekForward => offer.seek_arrows,
        Action::OpenGuide => offer.guide,
        Action::TogglePause => offer.accept == Action::TogglePause,
        Action::ShowDescription => !offer.seek_arrows,
        Action::Previous | Action::Next | Action::OpenCatalog | Action::OpenCategories
            | Action::OpenMenu | Action::ShowInfo => true,
    }
}

pub(super) fn perform(runtime: &mut Runtime, app: &mut App, request: &OttPlayerRequest)
    -> Result<Option<PlayerReq>, &'static str> {
    let current = offer(runtime, app).ok_or(CHANGED)?;
    if current.source != request.source || app.bridge.ott_player_offer() != Some(&current)
        || !action_allowed(&current, request.action)
    { return Err(CHANGED); }
    let entry = app.pages.nav.entry_of_instance(request.owner).ok_or(CHANGED)?;
    if app.pages.nav.input_owner() != Some(InputOwner::Entry(entry))
        || app.pages.nav.top_page().is_none_or(|page| page.id != entry
            || !matches!(page.arg, AppArg::Player)
            || page.inst.as_ref().is_none_or(|body| body.staged || body.id != request.owner))
    { return Err(CHANGED); }
    // From here, reuse the same semantic doors as the native controls. Never
    // pass an unchecked source-changing effect to generic player dispatch.
    app.rec.refuse("source-bound OTT remote input is not replayable");
    let forward = match request.action {
        Action::Previous | Action::Next => {
            let offset = if request.action == Action::Next { 1 } else { -1 };
            playback_step::perform(runtime, app, offset)?;
            None
        }
        Action::SeekBackward | Action::SeekForward => {
            if !commands::playback_ready(app) { return Err("No active foreground playback"); }
            let delta = if request.action == Action::SeekForward { 10_000_000_000 } else { -10_000_000_000 };
            let page = bridge::player_mut(&mut app.pages).ok_or(CHANGED)?;
            let target = page.jump_by(&app.player.session, delta, false, clock::now())
                .ok_or("Seeking requires a finite video timeline")?;
            Some(PlayerReq::CommitSeek(target))
        }
        Action::OpenMenu => Some(PlayerReq::OpenOverlay(
            plx_screens::player::overlay::OverlayKind::More { page: None })),
        Action::ShowInfo => Some(PlayerReq::ExtendDefaultHud),
        Action::ShowDescription => Some(if app.bridge.ui_mode() == plx_machine::machine::UiMode::Classic
            && plx_appkit::info_panel::live_channel(app.bridge.metadata_view())
            && !plx_appkit::info_panel::description(app.bridge.metadata_view()).trim().is_empty() {
            PlayerReq::OpenOverlay(plx_screens::player::overlay::OverlayKind::Info)
        } else { PlayerReq::ExtendDefaultHud }),
        Action::TogglePause => Some(PlayerReq::Transport(None)),
        Action::OpenCatalog | Action::OpenCategories => {
            let page = if request.action == Action::OpenCategories { "categories" }
                else if runtime.view_selection().1 { "favorites" } else { "channels" };
            if runtime.playing.is_none() {
                player_browse::open_native(runtime, app, &request.source, None, page)?;
            } else {
                player_browse::present(runtime, app, None, page);
            }
            None
        }
        Action::OpenGuide => {
            let id = runtime.playing.as_ref().ok_or(CHANGED)?.selection_id.clone();
            handle_action(runtime, app, OttAction::OpenGuide(id))?;
            player_browse::present(runtime, app, None, "guide");
            None
        }
    };
    runtime.changed();
    Ok(forward)
}
