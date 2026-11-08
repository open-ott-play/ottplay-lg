//! The existing OTT catalog presented on Player's modal stack. Navigation owns
//! the surface; the native Player and Runtime keep their existing playback state.
use super::*;
use plx_screens::registry::PlayerCatalogRequest;
use plx_ui::containers::modal::{Phase, Style};

const CHANGED: &str = "Playback changed before continuation";
const UNRECORDED: &str = "source-bound player catalog navigation is not replayable";

#[derive(Clone, PartialEq, Eq)]
struct NativeSource {
    identity: plx_media::route::PlaybackIdentity,
    sid: plx_plex::plex::ServerId,
    rk: String,
}

/// Presentation receipt only: never a playback, continuation or checkpoint owner.
pub(super) struct NativeCatalog {
    source: NativeSource,
    player: plx_machine::machine::InstanceId,
    panel: Option<plx_machine::machine::InstanceId>,
    opening_epoch: u64,
}

fn native_source(runtime: &Runtime, app: &App) -> Option<NativeSource> {
    if app.pages.has_pending_surface_navigation() { return None; }
    native_playback_source(runtime, app)
}

// Source eligibility only; callers admitting an action also reject pending navigation.
fn native_playback_source(runtime: &Runtime, app: &App) -> Option<NativeSource> {
    if runtime.playing.is_some()
        || runtime.interface_mode() != plx_machine::machine::UiMode::Classic
        || app.bridge.ui_mode() != plx_machine::machine::UiMode::Classic
        || !ready(app) || !runtime.loaded || runtime.persistence_uncertain
        || public_preferences::write_refusal(runtime).is_some()
        || !app.bridge.session_ready_current()
        || runtime.play_target.is_some() || runtime.resolver_next.is_some() || runtime.pending_play.is_some()
        || runtime.setup.is_some() || runtime.edit_session.is_some()
        || runtime.transient.accepted.is_some() || managed::profile(runtime).is_some()
        || retained_request_replaces_installed(app)
        || plx_media::route::external_source(&app.player.session).is_some()
        || plx_appkit::player_hud::transport_hidden(&app.player.session)
    { return None; }
    let sid = plx_media::route::cur_sid(&app.player.session);
    let rk = plx_media::route::cur_rk(&app.player.session);
    if rk.is_empty() || plx_plex::plex::client_for(sid).is_none_or(|client| client.data_io_disabled()) { return None; }
    // The installed route owns playback identity. A show page may retain its
    // parent Detail while an episode plays, so display metadata is not a gate.
    Some(NativeSource { identity: plx_media::route::playback_identity(&app.player.session)?, sid, rk })
}

/// The fullscreen remote shares the same installed-source proof as More.
pub(super) fn native_identity(runtime: &Runtime, app: &App)
    -> Option<plx_media::route::PlaybackIdentity>
{
    native_source(runtime, app).map(|source| source.identity)
}

/// Only the real fullscreen Player or the checked More door may call this.
/// Catalogue loading is explicit and never lends ownership of native playback.
pub(super) fn open_native(runtime: &mut Runtime, app: &mut App,
    expected: &plx_media::route::PlaybackIdentity,
    dismiss: Option<plx_machine::machine::EntryId>, page: &'static str)
    -> Result<(), &'static str>
{
    let source = native_source(runtime, app).ok_or(CHANGED)?;
    if &source.identity != expected { return Err(CHANGED); }
    let player = app.pages.top_page().ok_or(CHANGED)?;
    if !classic_source::ensure_server_catalog(runtime, app, source.sid)? { return Err(CHANGED); }
    if native_source(runtime, app).as_ref() != Some(&source)
        || app.pages.top_page() != Some(player) { return Err(CHANGED); }
    present(runtime, app, dismiss, page);
    runtime.native_catalog = Some(NativeCatalog { source, player, panel: None,
        opening_epoch: runtime.navigation_epoch });
    Ok(())
}

/// Step the currently visible Classic list, anchored to installed media rather
/// than the browse highlight. Missing authority never falls back to a different
/// native queue. The target enters the ordinary source-bound Play admission.
pub(super) fn native_list_step(runtime: &mut Runtime, app: &mut App,
    expected: &plx_media::route::PlaybackIdentity, offset: i64)
    -> Result<(), &'static str>
{
    const OPEN: &str = "Open the channel list for the playing source before changing items";
    if !matches!(offset, -1 | 1) { return Err(CHANGED); }
    let source = native_source(runtime, app).ok_or(CHANGED)?;
    if &source.identity != expected { return Err(CHANGED); }
    input_policy::replace_source(runtime)?;
    let profile = classic_source::profile(runtime).ok_or(OPEN)?;
    if runtime.effective_profile() != Some(profile) || profile.kind != ProviderKind::Plex
        || plx_plex::plex::id_of_machine(&profile.source) != Some(source.sid)
    { return Err(OPEN); }
    classic_source::guard_profile(runtime, app, profile)?;
    if classic_source::parental_authority(runtime)?.is_none() { return Err(OPEN); }
    let ids = commands::selected_ids(runtime);
    let visible: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
    // The bound profile already proved the canonical machine ID above. Scan
    // each collection once; unrelated rows need no registry lookup or binding.
    let matches_source = |server: &str, rk: &str| {
        server == profile.source.as_str() && rk == source.rk.as_str()
    };
    let mut current = Vec::new();
    if runtime.view_selection().1 {
        for favorite in runtime.saved.favorites.entries() {
            if matches!(&favorite.locator,
                plx_data::ott::favorites::Locator::Plex { server_id, rating_key }
                if matches_source(server_id, rating_key))
            {
                let id = format!("favorite:{}", favorite.identity());
                if visible.contains(id.as_str()) { current.push(id); }
            }
        }
    } else {
        for entry in &runtime.catalog.entries {
            if visible.contains(entry.id.as_str()) && matches!(&entry.media,
                MediaLocator::Plex { server_id, rating_key } if matches_source(server_id, rating_key))
            { current.push(entry.id.clone()); }
        }
    }
    if current.len() != 1 { return Err(OPEN); }
    let current = current[0].as_str();
    let (request, bound, entry, favorite) = runtime.prepare_play_request(PlaybackRequest::ordinary(current, 0))?;
    if bound.kind != ProviderKind::Plex || bound.source != profile.source
        || request.local_selection.is_some() || request.transient_owner.is_some()
        || match (&entry, &favorite) {
            (Some(entry), None) => !matches!(&entry.media, MediaLocator::Plex { server_id, rating_key }
                if matches_source(server_id, rating_key)),
            (None, Some(favorite)) => !matches!(&favorite.locator,
                plx_data::ott::favorites::Locator::Plex { server_id, rating_key }
                if matches_source(server_id, rating_key)),
            _ => true,
        }
    { return Err(OPEN); }
    let target = commands::step_in_list(&ids, Some(current), offset).ok_or(OPEN)?;
    if target == current { return Ok(()); }
    input_policy::ui(runtime, &OttAction::Play(target.clone()))?;
    runtime.play_with_policy(app, &target, false)?;
    runtime.cancel_setup();
    Ok(())
}

/// The first addressed refresh identifies the actual mounted panel created by
/// open(). A later unrelated panel cannot inherit this presentation receipt.
pub(super) fn bind_native_panel(runtime: &mut Runtime, app: &App,
    reply: plx_machine::machine::InstanceId)
{
    let Some(source) = native_source(runtime, app) else { return; };
    if bridge::ott_catalog_surface(&app.pages).is_none()
        || bridge::ott_ui_target(&app.pages) != Some(reply) { return; }
    if let Some(receipt) = runtime.native_catalog.as_mut() {
        if receipt.panel.is_none() && receipt.opening_epoch == runtime.navigation_epoch
            && Some(receipt.player) == app.pages.top_page() && receipt.source == source
        { receipt.panel = Some(reply); }
    }
}

fn replaying(app: &App) -> bool {
    matches!(&app.rec, super::super::recorder::Recplay::Replaying(_))
        || app.bridge.is_controlled_replay()
}

pub(super) fn ready(app: &App) -> bool {
    matches!(app.route(), AppArg::Player)
        && bridge::player(&app.pages).is_some()
        && app.adapters.player.is_live()
        && !replaying(app)
        && playback_allowed(app)
        && !app.player.lifecycle.awaiting_load()
        && !app.decoder_parked && !app.pages.nav.suspended
        && !app.pages.has_pending_navigation()
        && !app.pages.nav.tabs.stack.is_pending()
        && !app.pages.nav.tabs.stack.transition.in_flight()
        && !plx_media::route::play_pending()
        && plx_media::route::has_url(&app.player.session)
}

/// Classic's current-row Enter returns to the retained Player. This is only a
/// presentation change after the same source binding and playback gates used
/// by ordinary Play; it cannot stand in for admission of a different source.
pub(super) fn play_current(runtime: &mut Runtime, app: &mut App, id: &str)
    -> Result<bool, &'static str>
{
    if runtime.interface_mode() != plx_machine::machine::UiMode::Classic
        || app.bridge.ui_mode() != plx_machine::machine::UiMode::Classic
        || !ready(app) || !runtime.loaded || runtime.persistence_uncertain
        || app.pages.has_pending_surface_navigation()
        || !runtime.play_work_allowed()
        || !matches!(runtime.page, "channels" | "favorites")
        || runtime.setup.is_some() || runtime.edit_session.is_some()
        || runtime.play_target.is_some() || runtime.resolver_next.is_some()
        || runtime.pending_play.is_some()
        || plx_appkit::player_hud::transport_hidden(&app.player.session)
    { return Ok(false); }
    let Some(surface) = bridge::ott_catalog_surface(&app.pages) else { return Ok(false); };
    if runtime.reply.is_none() || runtime.reply != bridge::ott_ui_target(&app.pages) {
        return Ok(false);
    }
    let native = if runtime.playing.is_none() {
        let Some(source) = native_source(runtime, app) else { return Ok(false); };
        let Some(receipt) = runtime.native_catalog.as_ref() else { return Ok(false); };
        if receipt.source != source || Some(receipt.player) != app.pages.top_page()
            || receipt.panel != runtime.reply { return Ok(false); }
        Some(source)
    } else { None };
    let (request, profile, entry, favorite) =
        runtime.prepare_play_request(PlaybackRequest::ordinary(id, 0))?;
    let playing_entry = if let Some(playing) = runtime.playing.as_ref() {
        if playing.archive.is_some() || playing.identity.is_none() || !playing_matches(playing, app)
            || runtime.playing_attachment.as_ref().is_some_and(|receipt| !runtime.m3u_attachments.is_current(receipt))
            || profile != playing.profile || request.local_selection != playing.local_selection
            || request.transient_owner != playing.transient_owner
            || match (&entry, &favorite) {
                (Some(entry), None) => entry.id != playing.entry.id || entry.media != playing.entry.media,
                (None, Some(favorite)) => !selection::matches_entry(favorite, &playing.entry),
                _ => true,
            }
        { return Ok(false); }
        Some(playing.entry.clone())
    } else {
        let native = native.expect("checked native presentation");
        if profile.kind != ProviderKind::Plex
            || plx_plex::plex::id_of_machine(&profile.source) != Some(native.sid)
            || request.local_selection.is_some() || request.transient_owner.is_some()
            || match (&entry, &favorite) {
                (Some(entry), None) => !matches!(&entry.media, MediaLocator::Plex { server_id, rating_key }
                    if server_id == &profile.source && rating_key == &native.rk),
                (None, Some(favorite)) => !matches!(&favorite.locator, plx_data::ott::favorites::Locator::Plex { server_id, rating_key }
                    if server_id == &profile.source && rating_key == &native.rk),
                _ => true,
            }
        { return Ok(false); }
        entry.clone()
    };
    if !transient::lifecycle_target_ready(runtime, &profile, request.local_selection.as_ref()) {
        return Err("The local source is still being validated");
    }
    managed::guard_profile(runtime, app, &profile)?;
    classic_source::guard_profile(runtime, app, &profile)?;
    let policy_profile = runtime.attachment_policy_profile(&profile, favorite.as_ref())?;
    runtime.require_parental_playback(&policy_profile)?;
    kiosk_source::admit_bound(runtime, app, kiosk_source::BoundCandidate {
        request: &request, profile: &profile, entry: playing_entry.as_ref(), favorite: favorite.as_ref(),
    }).map_err(kiosk_source::SourceError::message)?;
    runtime.reply = None;
    runtime.native_catalog = None;
    app.pages.request(MachineId::Nav, plx_machine::machine::NavOp::Dismiss(surface));
    Ok(true)
}

/// A frame publication for the More menu. At most that one menu may cover
/// Player; another modal never lends its input authority to this entry point.
pub(super) fn offer(runtime: &Runtime, app: &App) -> Option<plx_media::route::PlaybackIdentity> {
    if app.pages.has_pending_surface_navigation() { return None; }
    current_offer(runtime, app)
}

/// Display-only continuity while a queued surface reaches its mount. The More
/// screen captures its first publication and cannot acquire another source later.
/// Keep only the already-published, still-eligible identity; action admission
/// continues through offer()/native_source(), which reject pending navigation.
pub(super) fn display_offer(runtime: &Runtime, app: &App) -> Option<plx_media::route::PlaybackIdentity> {
    let source = current_offer(runtime, app)?;
    if app.pages.has_pending_surface_navigation()
        && app.bridge.ott_catalog_offer() != Some(&source) { return None; }
    Some(source)
}

fn current_offer(runtime: &Runtime, app: &App) -> Option<plx_media::route::PlaybackIdentity> {
    if !ready(app) || !runtime.loaded || runtime.persistence_uncertain
        || input_policy::navigation(runtime).is_err() { return None; }
    let mut surfaces = app.pages.nav.modals.surfaces.iter()
        .filter(|surface| surface.phase != Phase::Hidden);
    if let Some(surface) = surfaces.next() {
        if surfaces.next().is_some()
            || !matches!(surface.phase, Phase::Opening | Phase::Open)
            || !matches!(&surface.entry.arg, AppArg::PlayerOverlay(arg)
                if matches!(arg.kind, plx_screens::player::overlay::OverlayKind::More { .. }))
        { return None; }
    }
    let Some(playing) = runtime.playing.as_ref() else {
        return native_playback_source(runtime, app).map(|source| source.identity);
    };
    if !playing_matches(playing, app) { return None; }
    let source = playing.identity.as_ref()?;
    plx_media::route::playback_identity_matches(&app.player.session, source)
        .then(|| source.clone())
}

pub(super) fn menu_owner(runtime: &Runtime, app: &mut App, request: &PlayerCatalogRequest)
    -> Result<plx_machine::machine::EntryId, &'static str>
{
    if replaying(app) {
        app.rec.refuse(UNRECORDED);
        return Err(CHANGED);
    }
    input_policy::navigation(runtime)?;
    if offer(runtime, app).as_ref() != Some(&request.source)
        || app.bridge.ott_catalog_offer() != Some(&request.source)
    { return Err(CHANGED); }
    let entry = app.pages.nav.entry_of_instance(request.owner).ok_or(CHANGED)?;
    if app.pages.nav.input_owner() != Some(plx_machine::machine::InputOwner::Entry(entry)) {
        return Err(CHANGED);
    }
    let surface = app.pages.nav.modals.surfaces.iter()
        .find(|surface| surface.entry.id == entry).ok_or(CHANGED)?;
    if !matches!(surface.phase, Phase::Opening | Phase::Open)
        || !matches!(&surface.entry.arg, AppArg::PlayerOverlay(arg)
            if matches!(arg.kind, plx_screens::player::overlay::OverlayKind::More { .. }))
        || surface.entry.inst.as_ref().is_none_or(|body| body.staged || body.id != request.owner)
    { return Err(CHANGED); }

    Ok(entry)
}

pub(super) fn open(runtime: &mut Runtime, app: &mut App, request: &PlayerCatalogRequest)
    -> Result<(), &'static str>
{
    let entry = menu_owner(runtime, app, request)?;
    let page = if runtime.view_selection().1 { "favorites" } else { "channels" };
    if runtime.playing.is_none() {
        return open_native(runtime, app, &request.source, Some(entry), page);
    }
    present(runtime, app, Some(entry), page);
    Ok(())
}

/// Shared presentation after the caller has validated its distinct input owner.
/// `page` is already admitted; presenting never replaces the decoder.
pub(super) fn present(runtime: &mut Runtime, app: &mut App,
    dismiss: Option<plx_machine::machine::EntryId>, page: &'static str) {
    app.rec.refuse(UNRECORDED);
    runtime.cancel_setup();
    runtime.native_catalog = None;
    runtime.reply = None;
    runtime.page = page;
    runtime.accepted_navigation(false);
    runtime.changed();
    app.bridge.publish_ott_catalog_offer(None);
    app.bridge.publish_ott_player_offer(None);
    if let Some(entry) = dismiss {
        app.pages.request(MachineId::Nav, plx_machine::machine::NavOp::Dismiss(entry));
    }
    app.pages.nav.next_style = Style::PlayerPanel { survives_failure: true };
    app.pages.request(MachineId::Nav, plx_machine::machine::NavOp::Present(AppArg::Ott));
}

/// A committed strict policy retires inadmissible Player presentations even
/// when they were opened without the OTT catalog. Info and the HUD remain valid.
fn restricted_presentation(arg: &AppArg) -> bool {
    use plx_screens::player::overlay::OverlayKind;
    match arg {
        AppArg::Ott | AppArg::Settings(_) => true,
        AppArg::PlayerOverlay(panel) => match panel.kind {
            OverlayKind::Info => false,
            OverlayKind::Tracks { .. } | OverlayKind::Chapters
                | OverlayKind::More { .. } | OverlayKind::Timing => true,
        },
        _ => false,
    }
}

pub(super) fn reconcile_strict_presentation(runtime: &mut Runtime, app: &mut App) {
    if !matches!(app.route(), AppArg::Player)
        || !matches!(runtime.track_selection_kiosk(),
            Ok(crate::app::ott_kiosk_policy::State::Locked { strict: true }))
    { return; }
    let ids: Vec<_> = app.pages.nav.modals.surfaces.iter()
        .filter(|surface| matches!(surface.phase, Phase::Opening | Phase::Open)
            && restricted_presentation(&surface.entry.arg))
        .map(|surface| surface.entry.id).collect();
    if ids.is_empty() { return; }
    runtime.cancel_setup();
    runtime.reply = None;
    app.bridge.publish_ott_catalog_offer(None);
    // Keep each permitted surface's own instance and state; only its forbidden
    // neighbours leave through the existing normal dismissal lifecycle.
    for id in ids {
        app.pages.request(MachineId::Nav, plx_machine::machine::NavOp::Dismiss(id));
    }
}

/// Close only the outer catalog layer. The reader/editor/Browse picker handles
/// its own Back first; guide and setup retain their existing inner navigation.
pub(super) fn back(runtime: &mut Runtime, app: &mut App, action: &OttAction)
    -> Option<Result<(), &'static str>>
{
    if !matches!(action, OttAction::Back) { return None; }
    let entry = bridge::ott_catalog_surface(&app.pages)?;
    if matches!(runtime.page, "guide" | "guide-programme" | "organization") {
        return None;
    }
    // Closing read-only browsing remains available to a normal pinned source.
    // Strict/transition policies retain the common navigation fence.
    if let Err(error) = input_policy::navigation(runtime) { return Some(Err(error)); }
    if runtime.setup.is_some() {
        runtime.cancel_setup();
        runtime.page = "profiles";
    } else if runtime.page == "actions" {
        return organization_runtime::list_order::action(runtime, app, action);
    } else if runtime.page == "categories"
        && (runtime.interface_mode() != plx_machine::machine::UiMode::Classic
            || app.bridge.ui_mode() != plx_machine::machine::UiMode::Classic) {
        runtime.page = if runtime.view_selection().1 { "favorites" } else { "channels" };
    } else if matches!(runtime.page, "categories" | "channels" | "favorites" | "providers") {
        // Classic Categories closes to its retained Player, just like Channels.
        // Dismiss only this owned surface; never replace or restart playback.
        runtime.cancel_setup();
        runtime.native_catalog = None;
        runtime.cancel_editor();
        runtime.reply = None;
        app.pages.request(MachineId::Nav, plx_machine::machine::NavOp::Dismiss(entry));
    } else {
        // Settings/profiles return to the provider chooser without the ordinary
        // page Back path's catalog/play-request cancellation.
        runtime.page = "providers";
    }
    Some(Ok(()))
}
