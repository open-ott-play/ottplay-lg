//! Owned OTT-root confirmation. Presentation has no navigation or storage effects.
use super::*;
use plx_machine::machine::UiMode;

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    pending: Option<Pending>,
}
#[derive(Clone, Copy)]
struct Pending {
    token: u64,
    owner: InstanceId,
    navigation: u64,
    generation: u64,
    mode: UiMode,
    projected: bool,
}
impl State {
    pub(super) fn retire(&mut self) { self.pending = None; }
}

fn root(runtime: &Runtime, app: &App) -> bool {
    runtime.page == "providers" && runtime.setup.is_none() && runtime.edit_session.is_none()
        && !runtime.usb_browsing()
        && matches!(app.route(), AppArg::Ott)
        && runtime.reply.is_some()
        && runtime.reply == app.pages.top_page()
        && (app.pages.nav.tabs.stack.depth() == 1 || classic_navigation::exit_root(runtime, app))
}
fn settled(app: &App) -> bool {
    !app.pages.nav.suspended && !app.pages.surface_up() && !app.pages.has_pending_surface_navigation()
        && !app.pages.has_pending_navigation() && !app.pages.nav.tabs.stack.is_pending()
        && !app.pages.nav.tabs.stack.transition.in_flight()
}
fn admitted(runtime: &Runtime, app: &App) -> Result<(), &'static str> {
    if classic_navigation::exit_root(runtime, app) { input_policy::navigation(runtime) }
    else { input_policy::ui(runtime, &OttAction::Back) }
}
fn current(runtime: &Runtime, app: &App, pending: Pending) -> bool {
    root(runtime, app) && settled(app) && Some(pending.owner) == runtime.reply
        && Some(pending.owner) == current_ui_owner(app)
        && pending.navigation == runtime.navigation_epoch && pending.generation == runtime.generation
        && pending.mode == app.bridge.ui_mode() && pending.mode == runtime.interface_mode()
        && pending.projected == (app.pages.nav.tabs.stack.depth() > 1)
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
    if runtime.exit_confirmation.pending.is_some_and(|pending| !current(runtime, app, pending)) {
        withdraw(runtime, app);
    }
}
pub(super) fn handles(runtime: &mut Runtime, app: &App, action: &OttAction) -> bool {
    if !matches!(action, OttAction::Back | OttAction::ExitAnswer { .. }) { return false; }
    classic_navigation::observe(runtime, app);
    matches!(action, OttAction::ExitAnswer { .. })
        || (matches!(action, OttAction::Back) && root(runtime, app))
}
pub(super) fn action(runtime: &mut Runtime, app: &mut App, action: &OttAction)
    -> Option<Result<(), &'static str>>
{
    if matches!(action, OttAction::Back | OttAction::ExitAnswer { .. }) {
        classic_navigation::observe(runtime, app);
    }
    if let OttAction::ExitAnswer { token, accept } = action {
        let Some(pending) = runtime.exit_confirmation.pending.filter(|pending|
            pending.token == *token && Some(pending.owner) == runtime.reply
                && Some(pending.owner) == current_ui_owner(app))
        else { return Some(Err("Exit confirmation is no longer current")); };
        runtime.exit_confirmation.retire();
        if !accept { return Some(Ok(())); }
        if !current(runtime, app, pending)
        { return Some(Err("Exit confirmation is no longer current")); }
        if let Err(error) = admitted(runtime, app) { return Some(Err(error)); }
        runtime.accepted_navigation(false);
        app.pages.emit(MachineId::Nav, Fx::App(plx_screens::registry::AppFx::Loop(
            plx_screens::registry::LoopReq::BackAtRoot)));
        return Some(Ok(()));
    }
    if !matches!(action, OttAction::Back) || !root(runtime, app) { return None; }
    // A deferred Back is not navigation either: do not cancel another surface's work.
    if !settled(app) { return Some(Ok(())); }
    if let Err(error) = admitted(runtime, app) { return Some(Err(error)); }
    let owner = runtime.reply.expect("owned root");
    if runtime.exit_confirmation.pending.is_some_and(|pending| current(runtime, app, pending)) {
        return Some(Ok(()));
    }
    withdraw(runtime, app);
    let Some(token) = runtime.exit_confirmation.serial.checked_add(1)
    else { return Some(Err("Exit confirmation limit reached; reopen the player")); };
    runtime.exit_confirmation.serial = token;
    runtime.exit_confirmation.pending = Some(Pending {
        token, owner, navigation: runtime.navigation_epoch, generation: runtime.generation,
        mode: app.bridge.ui_mode(), projected: app.pages.nav.tabs.stack.depth() > 1,
    });
    app.pages.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(owner),
        Delivery::Screen(plx_ui::screen::ScreenEvent::App(AppMsg::OttExitPrompt { token }))));
    Some(Ok(()))
}
