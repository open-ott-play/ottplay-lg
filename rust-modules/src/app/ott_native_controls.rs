//! Provider-independent doors to the existing native track menus. The menu is
//! presented only after the real player page mounts, against the same source.
use super::*;
use plx_screens::player::overlay::OverlayKind;

pub(super) struct Pending {
    ticket: plx_media::route::PlayRequestTicket,
    origin: Option<InstanceId>,
    expires_ms: u64,
    tab: i32,
    transitioning: bool,
}

fn policy_allows(runtime: &Runtime) -> bool {
    !runtime.saving && matches!(runtime.track_selection_kiosk(),
        Ok(super::super::ott_kiosk_policy::State::Off
            | super::super::ott_kiosk_policy::State::Armed { .. }))
}

pub(super) fn open(runtime: &mut Runtime, app: &mut App, tab: i32) -> Result<(), &'static str> {
    if !playback_allowed(app) {
        return Err("Playback is suspended");
    }
    if !plx_media::route::has_url(&app.player.session) || matches!(
        plx_media::player::state(&app.player.session),
        plx_media::player::PlaybackState::Idle | plx_media::player::PlaybackState::Error
            | plx_media::player::PlaybackState::Resolving | plx_media::player::PlaybackState::Connecting
    ) {
        return Err("Start a video to choose audio or subtitle settings");
    }
    if !policy_allows(runtime) {
        return Err("This action is restricted by kiosk policy.");
    }
    runtime.require_parental(parental::Target::Settings)?;
    runtime.native_controls = Some(Pending {
        ticket: plx_media::route::play_request_ticket(),
        origin: app.pages.top_page(),
        expires_ms: runtime.now_ms().saturating_add(5000),
        tab,
        transitioning: false,
    });
    // This changes only the visible page. It does not restart or resume media.
    playback::enter_player(&mut app.pages, &mut app.bridge, playback::Origin::Here, None);
    Ok(())
}

pub(super) fn after_tick(runtime: &mut Runtime, app: &mut App) {
    let now = runtime.now_ms();
    if runtime.native_controls.is_some() && !policy_allows(runtime) {
        runtime.native_controls = None;
        return;
    }
    // Presentation is deferred across navigation, so the original admission
    // does not authorize opening after its parental access has expired/revoked.
    if runtime.native_controls.is_some() {
        if let Err(error) = runtime.require_parental(parental::Target::Settings) {
            runtime.native_controls = None;
            runtime.status = error.into();
            runtime.changed();
            return;
        }
    }
    let Some(pending) = runtime.native_controls.as_mut() else { return };
    if !playback_allowed(app) || !plx_media::route::play_request_is_current(pending.ticket)
        || !plx_media::route::has_url(&app.player.session) || now >= pending.expires_ms
    {
        runtime.native_controls = None;
        return;
    }
    if app.pages.has_pending_navigation() {
        pending.transitioning = true;
        return;
    }
    if !matches!(app.route(), AppArg::Player) {
        if pending.transitioning || app.pages.top_page() != pending.origin {
            runtime.native_controls = None;
        }
        return;
    }
    let pending = runtime.native_controls.take().expect("checked pending track menu");
    app.bridge.publish_playback(&app.player.session, true);
    bridge::open_player_overlay(&app.player.session, app.bridge.metadata_view(), &mut app.pages,
        OverlayKind::Tracks { tab: pending.tab });
}
