//! More is a presentation of the installed source, never the browse highlight.
//! Requests retain the opening source and their actual modal input owner.
use super::*;
use plx_appkit::more_menu::{PlaybackMenuAction as Action, PlaybackMenuCapabilities};
use plx_machine::machine::{EntryId, InputOwner};
use plx_screens::registry::{PlayerMenuOffer, PlayerMenuRequest, PlayerReq};
use plx_ui::containers::modal::Phase;

const CHANGED: &str = "Playback changed; reopen the menu";
const UNAVAILABLE: &str = "This action is no longer available for the playing source";

fn eligible(runtime: &Runtime, app: &App) -> bool {
    runtime.interface_mode() == plx_machine::machine::UiMode::Classic
        && app.bridge.ui_mode() == plx_machine::machine::UiMode::Classic
        && runtime.setup.is_none() && runtime.edit_session.is_none()
        && runtime.play_target.is_none() && runtime.resolver_next.is_none()
        && runtime.pending_play.is_none() && !retained_request_replaces_installed(app)
}

/// Re-resolve the retained selection through the ordinary owner binder. A row
/// with the same public id but a changed source may not borrow the old player.
fn bound_playing<'a>(runtime: &'a Runtime, app: &App) -> Result<&'a Playing, &'static str> {
    let playing = runtime.playing.as_ref().filter(|p| playing_matches(p, app)).ok_or(CHANGED)?;
    let (_, profile, entry, favorite) = runtime.prepare_play_request(
        PlaybackRequest::ordinary(&playing.selection_id, 0))?;
    if profile != playing.profile { return Err(CHANGED); }
    managed::guard_profile(runtime, app, &profile)?;
    classic_source::guard_profile(runtime, app, &profile)?;
    if let Some(entry) = entry {
        // Display enrichment is not a new source. Keep locator, provider and
        // guide/catch-up provenance exact while allowing title/art/synopsis updates.
        let old = &playing.entry;
        if entry.id != old.id || entry.media != old.media
            || entry.provider_id != old.provider_id || entry.provider_id_field != old.provider_id_field
            || entry.live != old.live || entry.duration_ms != old.duration_ms
        { return Err(CHANGED); }
    } else {
        let current = favorite.ok_or(CHANGED)?;
        let retained = playing_favorite::capture(runtime, playing)?;
        if current.identity() != retained.identity() { return Err(CHANGED); }
    }
    Ok(playing)
}

/// The playing inventory belongs to the installed Plex leaf. A retained Info
/// caption or the current browsing Detail is not evidence of that ownership.
fn installed_leaf(app: &App) -> Option<&plx_data::metadata::PlayingItem> {
    if plx_media::route::external_source(&app.player.session).is_some() { return None; }
    let leaf = app.bridge.metadata_view().playing()?;
    let sid = plx_media::route::cur_sid(&app.player.session);
    let rk = plx_media::route::cur_rk(&app.player.session);
    (!rk.is_empty() && leaf.sid == sid && leaf.rk == rk).then_some(leaf)
}

fn detail_target(app: &App) -> Option<String> {
    let leaf = installed_leaf(app)?;
    match leaf.kind.as_str() {
        "movie" => Some(leaf.rk.clone()),
        "episode" if !leaf.show_rk.is_empty() => Some(leaf.show_rk.clone()),
        _ => None,
    }
}

fn finite_seek(runtime: &Runtime, app: &App) -> bool {
    // The player exposes seek strategies (segment/native/reload), not a separate
    // seekable flag. Reuse its finite timing contract, never a live EPG duration.
    if !commands::playback_ready(app) || playback::dur() <= 0
        || plx_appkit::player_hud::transport_hidden(&app.player.session) { return false; }
    match runtime.playing.as_ref() {
        Some(_) => bound_playing(runtime, app).is_ok_and(|playing|
            playing.archive.is_some() || !guide_ui::is_live(&playing.profile, &playing.entry)),
        None => true, // offer() has already proved current native Plex ownership.
    }
}

fn permit_detail(runtime: &mut Runtime, app: &App) -> Result<(), &'static str> {
    input_policy::replace_source(runtime)?;
    let sid = installed_leaf(app).ok_or(CHANGED)?.sid;
    let profile = if runtime.playing.is_some() {
        Some(bound_playing(runtime, app)?.profile.clone())
    } else {
        let policy = parental::Policy::read(&runtime.saved.settings).map_err(|_| UNAVAILABLE)?;
        if policy.protects(parental::Target::Channel { marked: true }) {
            let profile = classic_source::profile(runtime).ok_or(UNAVAILABLE)?;
            if runtime.effective_profile() != Some(profile) || profile.kind != ProviderKind::Plex
                || plx_plex::plex::id_of_machine(&profile.source) != Some(sid)
            { return Err(UNAVAILABLE); }
            classic_source::guard_profile(runtime, app, profile)?;
            if classic_source::parental_authority(runtime)?.is_none() { return Err(UNAVAILABLE); }
            Some(profile.clone())
        } else { None }
    };
    match profile {
        Some(profile) => runtime.require_parental_playback(&profile),
        None => runtime.require_parental(parental::Target::Channel { marked: true }),
    }
}

fn capabilities(runtime: &Runtime, app: &App) -> PlaybackMenuCapabilities {
    let mut actions = PlaybackMenuCapabilities {
        categories: menu_visibility::visibility(runtime).visible(plx_appkit::menu_visibility::Item::Categories),
        reload_stream: commands::restartable(runtime, app),
        ..Default::default()
    };
    actions.from_beginning = finite_seek(runtime, app);
    actions.chapters = actions.from_beginning
        && installed_leaf(app).is_some_and(|leaf| !leaf.chapters.is_empty());
    actions.go_to_detail = detail_target(app).is_some();
    if let Some(playing) = runtime.playing.as_ref() {
        let live = guide_ui::is_live(&playing.profile, &playing.entry);
        // Ordinary live pause is not a catch-up capability. Only known finite
        // media/archive (or native Plex below) advertises this transport action.
        actions.toggle_pause = commands::playback_ready(app)
            && (playing.archive.is_some() || (!live && playback::dur() > 0));
        if let Ok(bound) = bound_playing(runtime, app) {
            actions.guide = guide_ui::is_live(&bound.profile, &bound.entry);
            actions.records = actions.guide
                && guide_ui::records_available(runtime, &bound.profile, &bound.entry);
            actions.return_live = actions.guide && bound.archive.is_some();
        }
    } else {
        actions.toggle_pause = commands::playback_ready(app);
    }
    actions
}

pub(super) fn offer(runtime: &Runtime, app: &App) -> Option<PlayerMenuOffer> {
    if !eligible(runtime, app) { return None; }
    let source = player_browse::display_offer(runtime, app)?;
    Some(PlayerMenuOffer { source, actions: capabilities(runtime, app) })
}

fn owner(app: &App, request: &PlayerMenuRequest) -> Result<EntryId, &'static str> {
    let entry = app.pages.nav.entry_of_instance(request.owner).ok_or(CHANGED)?;
    if app.pages.nav.input_owner() != Some(InputOwner::Entry(entry)) { return Err(CHANGED); }
    let surface = app.pages.nav.modals.surfaces.iter()
        .find(|surface| surface.entry.id == entry).ok_or(CHANGED)?;
    if !matches!(surface.phase, Phase::Opening | Phase::Open)
        || !matches!(&surface.entry.arg, AppArg::PlayerOverlay(arg)
            if matches!(arg.kind, plx_screens::player::overlay::OverlayKind::More { .. }))
        || surface.entry.inst.as_ref().is_none_or(|body| body.staged || body.id != request.owner)
    { return Err(CHANGED); }
    if surface.entry.inst.as_ref().and_then(|body| body.screen.as_any())
        .and_then(|screen| screen.downcast_ref::<plx_screens::player::overlay::PlayerOverlayScreen>())
        .is_none_or(|screen| !screen.accepts_menu_request(request))
    { return Err(CHANGED); }
    Ok(entry)
}

pub(super) fn perform(runtime: &mut Runtime, app: &mut App, request: &PlayerMenuRequest)
    -> Result<Option<PlayerReq>, &'static str>
{
    let entry = owner(app, request)?;
    input_policy::navigation(runtime)?;
    if !eligible(runtime, app)
        || player_browse::offer(runtime, app).as_ref() != Some(&request.offer.source)
        || app.bridge.playback_menu_offer().is_none_or(|offer| offer.source != request.offer.source
            || !offer.actions.contains(request.action))
    { return Err(CHANGED); }
    let current = capabilities(runtime, app);
    if !request.offer.actions.contains(request.action) || !current.contains(request.action) {
        return Err(UNAVAILABLE);
    }
    app.rec.refuse("source-bound playback menu actions are not replayable");
    let forward = match request.action {
        Action::Categories => {
            if runtime.playing.is_none() {
                player_browse::open_native(runtime, app, &request.offer.source, Some(entry), "categories")?;
            } else {
                player_browse::present(runtime, app, Some(entry), "categories");
            }
            return Ok(None);
        }
        Action::Guide | Action::Records => {
            let id = bound_playing(runtime, app)?.selection_id.clone();
            let action = if request.action == Action::Guide { OttAction::OpenGuide(id) }
                else { OttAction::OpenRecords(id) };
            handle_action(runtime, app, action)?;
            player_browse::present(runtime, app, Some(entry), "guide");
            return Ok(None);
        }
        Action::TogglePause => {
            let transport = PlayerReq::Transport(None);
            if !input_policy::player(runtime, &transport) { return Err(UNAVAILABLE); }
            Some(transport)
        }
        Action::ReloadStream => {
            // One typed implementation also backs the existing JSON command;
            // it retains the native finite resume position and paused state.
            commands::restart_stream(runtime, app)?;
            None
        }
        Action::ReturnLive => {
            let id = bound_playing(runtime, app)?.selection_id.clone();
            let mut play = PlaybackRequest::ordinary(&id, 0);
            play.advance_on_failure = false;
            runtime.play_request_with_policy(app, play)?;
            None
        }
        Action::Chapters => {
            Some(PlayerReq::OpenOverlay(plx_screens::player::overlay::OverlayKind::Chapters))
        }
        Action::FromBeginning => {
            Some(PlayerReq::Info(plx_appkit::info_panel::InfoAction::FromBeginning))
        }
        Action::GoToDetail => {
            let target = detail_target(app).ok_or(UNAVAILABLE)?;
            permit_detail(runtime, app)?;
            // Exact playing sid/rk was checked above, so the existing executor
            // reads that server and cannot enter its current_server fallback.
            Some(PlayerReq::Info(plx_appkit::info_panel::InfoAction::GoToDetail(target)))
        }
    };
    app.pages.request(MachineId::Nav, plx_machine::machine::NavOp::Dismiss(entry));
    runtime.changed();
    Ok(forward)
}

/// A stale owner/source must not put feedback on another menu. The receiving
/// overlay also checks its exact pending request before showing the shared note.
pub(super) fn refused(app: &mut App, request: &PlayerMenuRequest, reason: &'static str) {
    if owner(app, request).is_err()
        || !plx_media::route::playback_identity_matches(&app.player.session, &request.offer.source)
    { return; }
    app.pages.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(request.owner),
        Delivery::Screen(plx_ui::screen::ScreenEvent::Async(request.id,
            AppMsg::PlayerMenuResult { request: request.clone(), outcome: Err(reason) }))));
}
