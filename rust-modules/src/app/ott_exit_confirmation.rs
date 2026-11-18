//! The addressed exit question belongs to the current catalogue presentation.
use super::*;
use plx_machine::machine::{EntryId, UiMode};
use plx_ui::containers::modal::Phase;

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    pending: Option<Pending>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Host {
    Page,
    Projected,
    Panel { player: InstanceId, entry: EntryId },
}

struct Pending {
    token: u64,
    owner: InstanceId,
    host: Host,
    page_id: String,
    session: u64,
    navigation: u64,
    generation: u64,
    mode: UiMode,
}

impl State {
    pub(super) fn retire(&mut self) { self.pending = None; }
}

fn root(runtime: &Runtime, app: &App) -> Option<Host> {
    if runtime.setup.is_some() || runtime.edit_session.is_some() || runtime.usb_browsing()
        || boot_resume::pending(runtime) || classic_play_handoff::selected(runtime).is_some()
    { return None; }

    let owner = runtime.reply?;
    match runtime.interface_mode() {
        UiMode::Modern => (runtime.page == "providers" && matches!(app.route(), AppArg::Ott)
            && app.pages.top_page() == Some(owner) && app.pages.nav.tabs.stack.depth() == 1)
            .then_some(Host::Page),
        UiMode::Classic => {
            if !matches!(runtime.page, "channels" | "favorites") { return None; }
            match app.route() {
                AppArg::Ott if app.pages.top_page() == Some(owner) => {
                    Some(if classic_navigation::exit_root(runtime, app) { Host::Projected } else { Host::Page })
                }
                AppArg::Player if current_ui_owner(app) == Some(owner) => Some(Host::Panel {
                    player: app.pages.top_page()?, entry: bridge::ott_catalog_surface(&app.pages)?,
                }),
                _ => None,
            }
        }
    }
}

fn settled(runtime: &Runtime, app: &App, host: Host) -> bool {
    if !playback_allowed(app) || app.pages.nav.suspended || app.pages.has_pending_surface_navigation()
        || app.pages.has_pending_navigation() || app.pages.nav.tabs.stack.is_pending()
        || app.pages.nav.tabs.stack.transition.in_flight()
        || app.bridge.ui_mode() != runtime.interface_mode() || runtime.reply != current_ui_owner(app)
    { return false; }
    match host {
        Host::Page | Host::Projected => !app.pages.surface_up(),
        // A closing More, Settings or consent surface still owns its own transition.
        Host::Panel { entry, .. } => app.pages.nav.modals.surfaces.iter()
            .all(|surface| surface.phase == Phase::Hidden
                || (surface.entry.id == entry && surface.phase == Phase::Open)),
    }
}

fn admitted(runtime: &Runtime, host: Host) -> Result<(), &'static str> {
    if runtime.interface_mode() == UiMode::Classic && runtime.search_active() {
        return Err("Close the current search before leaving the channel list");
    }
    if host == Host::Projected { input_policy::navigation(runtime) }
    else { input_policy::ui(runtime, &OttAction::Back) }
}

fn page_id(runtime: &Runtime) -> String {
    runtime.search_page_id().unwrap_or_else(|| runtime.page.into())
}

fn current(runtime: &Runtime, app: &App, pending: &Pending) -> bool {
    root(runtime, app) == Some(pending.host) && settled(runtime, app, pending.host)
        && Some(pending.owner) == runtime.reply && pending.mode == runtime.interface_mode()
        && pending.session == app.bridge.auth_read().0.flow_epoch
        && pending.navigation == runtime.navigation_epoch && pending.generation == runtime.generation
        && pending.page_id == page_id(runtime) && admitted(runtime, pending.host).is_ok()
}

fn withdraw(runtime: &mut Runtime, app: &mut App) {
    if let Some(pending) = runtime.exit_confirmation.pending.take() {
        app.pages.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(pending.owner),
            Delivery::Screen(plx_ui::screen::ScreenEvent::App(AppMsg::OttExitDismiss { token: pending.token }))));
    }
}

pub(super) fn reconcile(runtime: &mut Runtime, app: &mut App) {
    if runtime.exit_confirmation.pending.is_none() { return; }
    classic_navigation::observe(runtime, app);
    if runtime.exit_confirmation.pending.as_ref().is_some_and(|pending| !current(runtime, app, pending)) {
        withdraw(runtime, app);
    }
}

pub(super) fn handles(runtime: &mut Runtime, app: &App, action: &OttAction) -> bool {
    if !matches!(action, OttAction::Back | OttAction::ExitAnswer { .. }) { return false; }
    classic_navigation::observe(runtime, app);
    matches!(action, OttAction::ExitAnswer { .. }) || root(runtime, app).is_some()
}

pub(super) fn action(runtime: &mut Runtime, app: &mut App, action: &OttAction)
    -> Option<Result<(), &'static str>>
{
    if !matches!(action, OttAction::Back | OttAction::ExitAnswer { .. }) { return None; }
    classic_navigation::observe(runtime, app);
    if let OttAction::ExitAnswer { token, accept } = action {
        let pending = runtime.exit_confirmation.pending.as_ref().filter(|pending|
            pending.token == *token && Some(pending.owner) == runtime.reply
                && Some(pending.owner) == current_ui_owner(app));
        let Some(pending) = pending else {
            // The screen can decline a late offer after a newer question owns the list.
            return Some(if *accept { Err("Exit confirmation is no longer current") } else { Ok(()) });
        };
        let valid = !accept || current(runtime, app, pending);
        runtime.exit_confirmation.retire();
        if !valid { return Some(Err("Exit confirmation is no longer current")); }
        if *accept {
            runtime.accepted_navigation(false);
            app.pages.emit(MachineId::Nav, Fx::App(plx_screens::registry::AppFx::Loop(
                plx_screens::registry::LoopReq::BackAtRoot)));
        }
        return Some(Ok(()));
    }

    let host = root(runtime, app)?;
    // Consume a deferred Back before generic navigation can cancel catalogue work.
    if !settled(runtime, app, host) { return Some(Ok(())); }
    if let Err(error) = admitted(runtime, host) { return Some(Err(error)); }
    if runtime.exit_confirmation.pending.as_ref().is_some_and(|pending| current(runtime, app, pending)) {
        return Some(Ok(()));
    }
    withdraw(runtime, app);
    let Some(token) = runtime.exit_confirmation.serial.checked_add(1)
    else { return Some(Err("Exit confirmation limit reached; reopen the player")); };
    let owner = runtime.reply.expect("owned root");
    let page_id = page_id(runtime);
    runtime.exit_confirmation.serial = token;
    runtime.exit_confirmation.pending = Some(Pending {
        token, owner, host, page_id: page_id.clone(), session: app.bridge.auth_read().0.flow_epoch,
        navigation: runtime.navigation_epoch, generation: runtime.generation, mode: runtime.interface_mode(),
    });
    app.pages.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(owner),
        Delivery::Screen(plx_ui::screen::ScreenEvent::App(AppMsg::OttExitPrompt { token, page_id }))));
    Some(Ok(()))
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_exit_confirmation_tests.rs"]
mod tests;
