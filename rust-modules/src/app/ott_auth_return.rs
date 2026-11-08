//! Ephemeral return from the existing Session-owned Plex PIN flow. No account
//! decision, credential, saved profile or cached screen is owned by this module.
use super::*;
use plx_session::auth::owner::{LoginCancelDisposition, LoginOrigin, ProfileScope, ReplyTo};
use plx_screens::registry::AppFx;

#[derive(Clone)]
struct Request {
    id: u32,
    owner: InstanceId,
    fence: Option<String>,
    origin: Option<LoginOrigin>,
    started: bool,
    stale: bool,
}

struct PendingReturn {
    to: ReplyTo,
    origin: LoginOrigin,
    epoch: u64,
    scope: ProfileScope,
    request: Request,
}

#[derive(Default)]
pub(super) struct State {
    serial: u32,
    request: Option<Request>,
    pending_return: Option<PendingReturn>,
    focus_connect: bool,
    returning: bool,
}

fn fence(runtime: &Runtime) -> String {
    // Only the digest is retained. Source credentials never enter a continuation.
    plx_data::ott::favorites_wire::hash(json!([
        runtime.page, runtime.provider, runtime.navigation_epoch, runtime.generation,
        runtime.profile_edit_epoch, runtime.remote.authority_generation(),
        runtime.saved.profiles, runtime.saved.settings.get("selectedProvider"),
        runtime.effective_profile()
    ]).to_string().as_bytes())
}

pub(super) fn request(runtime: &mut Runtime, app: &App) -> Result<(), &'static str> {
    let owner = runtime.reply.filter(|owner| Some(*owner) == current_ui_owner(app))
        .ok_or("Plex sign-in screen is no longer active")?;
    if runtime.page != "profiles" || runtime.provider != ProviderKind::Plex
        || app.pages.has_pending_navigation() || app.pages.nav.tabs.stack.is_pending() {
        return Err("Plex sign-in screen is no longer active");
    }
    let id = runtime.auth_return.serial.checked_add(1).ok_or("Reopen the player before signing in")?;
    runtime.auth_return.serial = id;
    runtime.auth_return.focus_connect = false;
    runtime.auth_return.pending_return = None;
    runtime.auth_return.request = Some(Request {
        id, owner, fence: None, origin: None, started: false, stale: false,
    });
    Ok(())
}

/// Called after accepted-navigation bookkeeping, never before its epoch changes.
pub(super) fn finalize(runtime: &mut Runtime, app: &mut App) {
    if !runtime.auth_return.request.as_ref().is_some_and(|request| request.fence.is_none()) { return; }
    let captured = fence(runtime);
    let request = runtime.auth_return.request.as_mut().unwrap();
    request.fence = Some(captured);
    app.pages.emit(MachineId::Nav, Fx::App(AppFx::Loop(
        plx_screens::registry::LoopReq::OttSignIn { origin: request.id })));
}

pub(super) fn navigation(runtime: &mut Runtime) {
    runtime.auth_return.focus_connect = false;
    if let Some(request) = &mut runtime.auth_return.request {
        // The action opening the flow has not been finalized yet.
        if request.fence.is_some() { request.stale = true; }
    }
}

pub(super) fn focus(runtime: &Runtime) -> bool {
    runtime.auth_return.focus_connect && runtime.page == "profiles" && runtime.provider == ProviderKind::Plex
}

/// A top page is not necessarily the input owner: a modal can cover it and
/// suspension can retain the entire tree. Never remove either with a return.
fn return_input_ready(app: &App, instance: u32) -> bool {
    if app.ott_standby || (!run::controlled_replay(app) && !app.window_activity.allow_present(true))
        || app.pages.nav.suspended
        || app.pages.input.keyboard || app.pages.input.keyboard_owner.is_some()
        || app.pages.has_pending_navigation() || app.pages.nav.tabs.stack.is_pending()
        || app.pages.has_pending_surface_navigation() {
        return false;
    }
    let Some(plx_machine::machine::InputOwner::Entry(id)) = app.pages.nav.input_owner() else { return false; };
    app.pages.nav.entry(id).is_some_and(|entry|
        matches!(entry.arg, AppArg::Login | AppArg::Profiles)
            && entry.inst.as_ref().is_some_and(|body| body.id.0 == instance && !body.staged))
}

pub(in crate::app) fn start(app: &mut App, id: u32) {
    let Some(mut runtime) = app.ott.take() else { return; };
    let offered = runtime.auth_return.request.clone().filter(|request|
        request.id == id && !request.started && !request.stale
            && request.fence.as_deref() == Some(fence(&runtime).as_str())
            && current_ui_owner(app) == Some(request.owner) && !app.pages.has_pending_navigation()
            && !app.pages.nav.tabs.stack.is_pending());
    if let Some(request) = offered {
        runtime.auth_return.request.as_mut().unwrap().started = true;
        bridge::execute_session_command(&mut app.pages,
            plx_session::auth::SessionCmd::StartLoginFrom {
                origin: u64::from(id), reply: ReplyTo { instance: request.owner.0, correlation: id },
            });
    }
    app.ott = Some(runtime);
}

/// True while this module's fresh landing must not be overwritten by the generic
/// Login/Profiles follower. Ordinary successful sign-in still uses that follower.
pub(in crate::app) fn poll(app: &mut App) -> bool {
    // Drain even if OTT has been retired. No receipt can leak into a later runtime.
    let replies = app.bridge.take_login_origin_replies();
    let Some(mut runtime) = app.ott.take() else { return false; };
    let mut navigated = false;
    if runtime.auth_return.returning {
        if app.pages.has_pending_navigation() || app.pages.nav.tabs.stack.is_pending() {
            navigated = true;
        } else {
            runtime.auth_return.returning = false;
        }
    }
    for reply in replies {
        match reply {
            bridge::LoginOriginReply::Started { to, origin, accepted, returnable } => {
                let request = runtime.auth_return.request.clone().filter(|request|
                    request.started && u64::from(request.id) == origin.id && request.owner.0 == to.instance
                        && request.id == to.correlation && request.origin.is_none());
                if let Some(request) = request {
                    let current = app.bridge.auth_read().0.flow_epoch == origin.flow_epoch;
                    // A policy boundary may retire the return proof after Session
                    // accepted this start. Keep the ordinary Login landing, without
                    // reviving the now-ineligible shortcut back to OTT.
                    let can_return = returnable && app.bridge.auth_read().0.origin_cancel == Some(origin);
                    let destination = !request.stale && request.fence.as_deref() == Some(fence(&runtime).as_str())
                        && current_ui_owner(app) == Some(request.owner) && !app.pages.has_pending_navigation()
                        && !app.pages.nav.tabs.stack.is_pending();
                    if accepted && current && destination {
                        if can_return {
                            runtime.auth_return.request.as_mut().unwrap().origin = Some(origin);
                        } else { runtime.auth_return.request = None; }
                        // The Session accepted the start before the old screen is retired.
                        bridge::nav_root(&mut app.pages, AppArg::Login);
                        navigated = true;
                    } else {
                        runtime.auth_return.request = None;
                        if accepted && current && can_return {
                            // Retire only our superseded start, never a newer flow.
                            bridge::execute_session_command(&mut app.pages,
                                plx_session::auth::SessionCmd::CancelLoginFrom { origin, reply: to });
                        }
                    }
                }
            }
            bridge::LoginOriginReply::Cancelled { to, origin, disposition } => {
                if let Some(request) = runtime.auth_return.request.clone().filter(|request| request.origin == Some(origin)) {
                    match disposition {
                        LoginCancelDisposition::Returned { epoch, scope } => {
                            runtime.auth_return.request = None;
                            runtime.auth_return.pending_return = Some(PendingReturn { to, origin, epoch, scope, request });
                        }
                        LoginCancelDisposition::Stale => { runtime.auth_return.request = None; }
                        LoginCancelDisposition::Blocked => {}
                    }
                }
            }
        }
    }
    if let Some(pending) = runtime.auth_return.pending_return.take() {
        let current = app.bridge.login_origin_return_is_current(pending.origin, pending.epoch, pending.scope);
        let foreground = app.pages.top_page().is_some_and(|owner| owner.0 == pending.to.instance)
            && matches!(app.route(), AppArg::Login | AppArg::Profiles);
        if current && foreground {
            if !return_input_ready(app, pending.to.instance) {
                // Keep a valid receipt through this Login's entrance, suspension
                // or covering modal. A new owner/flow still wins above.
                runtime.auth_return.pending_return = Some(pending);
                navigated = true;
            } else {
                // A changed destination gets its CURRENT view, never captured
                // provider/profile/page values. Neither path activates media.
                runtime.auth_return.focus_connect = !pending.request.stale
                    && pending.request.fence.as_deref() == Some(fence(&runtime).as_str());
                runtime.auth_return.returning = true;
                runtime.reply = None;
                runtime.changed();
                bridge::nav_root(&mut app.pages, AppArg::Ott);
                navigated = true;
            }
        }
    }
    app.ott = Some(runtime);
    navigated
}
