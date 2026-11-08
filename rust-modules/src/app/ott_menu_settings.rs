//! Grouped Settings above the exact initiating More surface. This owns only
//! presentation: it never loads a catalogue or adopts native playback.
use super::*;
use plx_machine::machine::{EntryId, NavOp};
use plx_screens::registry::{PageMemory, PlayerCatalogRequest};
use plx_ui::containers::modal::{Phase, Style};

const CHANGED: &str = "Playback changed before opening Settings";

#[derive(Default)]
pub(super) struct State {
    origin: Option<Origin>,
    memory: Option<PageMemory>,
}

struct Origin {
    menu: EntryId,
    owner: InstanceId,
    player: InstanceId,
    source: plx_media::route::PlaybackIdentity,
    session: u64,
    opening_epoch: u64,
    panel: Option<(EntryId, InstanceId)>,
    authorized: bool,
    returning: bool,
    abandoned: bool,
    page: &'static str,
}

pub(super) fn open(runtime: &mut Runtime, app: &mut App, request: &PlayerCatalogRequest)
    -> Result<(), &'static str>
{
    let menu = player_browse::menu_owner(runtime, app, request)?;
    input_policy::ui(runtime, &OttAction::ShowSettings)?;
    if runtime.menu_settings.origin.is_some() { return Err(CHANGED); }
    let player = app.pages.top_page().ok_or(CHANGED)?;
    let page = runtime.page;
    runtime.cancel_setup();
    settings_groups::cancel(runtime);
    runtime.reply = None;
    runtime.page = "settings";
    runtime.status.clear();
    runtime.accepted_navigation(false);
    runtime.menu_settings.origin = Some(Origin {
        menu, owner: request.owner, player, source: request.source.clone(),
        session: app.bridge.auth_read().0.flow_epoch,
        opening_epoch: runtime.navigation_epoch, panel: None, authorized: false, returning: false, abandoned: false, page,
    });
    runtime.changed();
    app.rec.refuse("source-bound player Settings navigation is not replayable");
    app.bridge.publish_ott_catalog_offer(Some(request.source.clone()));
    app.bridge.publish_ott_player_offer(None);
    app.pages.nav.next_style = Style::PlayerPanel { survives_failure: true };
    // Retain More: the shared stack captures its focus and restores that same instance.
    app.pages.request(MachineId::Instance(request.owner), NavOp::Present(AppArg::Ott));
    Ok(())
}

fn source_current(origin: &Origin, runtime: &Runtime, app: &App) -> bool {
    app.pages.top_page() == Some(origin.player)
        && matches!(app.route(), AppArg::Player)
        && app.bridge.auth_read().0.flow_epoch == origin.session
        && plx_media::route::playback_identity_matches(&app.player.session, &origin.source)
        && runtime.play_target.is_none() && runtime.pending_play.is_none()
        && !app.player.lifecycle.awaiting_load() && !app.decoder_parked
        && app.adapters.player.is_live()
        && app.pages.nav.entry(origin.menu).is_some_and(|entry|
            matches!(&entry.arg, AppArg::PlayerOverlay(arg)
                if matches!(arg.kind, plx_screens::player::overlay::OverlayKind::More { .. }))
            && entry.inst.as_ref().is_some_and(|body| body.id == origin.owner && !body.staged))
        && app.pages.nav.modals.surfaces.iter().any(|surface|
            surface.entry.id == origin.menu && matches!(surface.phase, Phase::Opening | Phase::Open))
}

/// Bind only the real, current, immediately adjacent OTT surface after Mount.
/// No synthetic reply target can authorize the existing parental editor.
pub(super) fn bind(runtime: &mut Runtime, app: &mut App, reply: InstanceId) {
    let Some(origin) = runtime.menu_settings.origin.as_ref() else { return; };
    if origin.panel.is_some() || current_ui_owner(app) != Some(reply)
        || app.pages.has_pending_navigation() || app.pages.has_pending_surface_navigation()
    { return; }
    let surfaces = &app.pages.nav.modals.surfaces;
    let Some(index) = surfaces.iter().position(|surface| surface.entry.id == origin.menu) else { return; };
    let Some(panel) = surfaces.get(index + 1) else { return; };
    if index + 2 != surfaces.len() || !matches!(panel.entry.arg, AppArg::Ott)
        || !matches!(panel.phase, Phase::Opening | Phase::Open)
        || panel.entry.inst.as_ref().is_none_or(|body| body.id != reply || body.staged)
    { return; }
    let stale = origin.abandoned || origin.opening_epoch != runtime.navigation_epoch
        || !source_current(origin, runtime, app);
    runtime.menu_settings.origin.as_mut().unwrap().panel = Some((panel.entry.id, reply));
    if stale { retire(runtime, app, false); return; }
    // The caller has already accepted this exact Refresh as runtime.reply.
    let admission = input_policy::ui(runtime, &OttAction::ShowSettings)
        .and_then(|_| parental_runtime::ui_gate(runtime, app, &OttAction::ShowSettings));
    if let Err(error) = admission { runtime.status = error.into(); }
}

fn retire(runtime: &mut Runtime, app: &mut App, keep_menu: bool) {
    let Some(origin) = runtime.menu_settings.origin.take() else { return; };
    let owns_runtime = match origin.panel {
        Some((_, owner)) => runtime.reply == Some(owner) && current_ui_owner(app) == Some(owner),
        None => runtime.reply.is_none() && current_ui_owner(app).is_none(),
    };
    if owns_runtime {
        runtime.cancel_setup();
        settings_groups::cancel(runtime);
        runtime.reply = None;
        runtime.page = origin.page;
    }
    runtime.changed();
    if let Some((entry, owner)) = origin.panel {
        if app.pages.nav.entry_of_instance(owner) == Some(entry) {
            app.pages.request(MachineId::Nav, NavOp::Dismiss(entry));
        }
    }
    if keep_menu {
        runtime.menu_settings.origin = Some(Origin { returning: true, ..origin });
    } else {
        runtime.menu_settings.memory = None;
        if app.pages.nav.entry_of_instance(origin.owner) == Some(origin.menu) {
            app.pages.request(MachineId::Nav, NavOp::Dismiss(origin.menu));
        }
    }
}

pub(super) fn reconcile(runtime: &mut Runtime, app: &mut App) {
    let Some(origin) = runtime.menu_settings.origin.as_ref() else { return; };
    let current = source_current(origin, runtime, app);
    if origin.panel.is_none() && !app.pages.has_pending_navigation()
        && !app.pages.has_pending_surface_navigation()
        && app.pages.nav.modals.surfaces.iter().position(|surface| surface.entry.id == origin.menu)
            .is_some_and(|index| app.pages.nav.modals.surfaces.get(index + 1)
                .is_none_or(|surface| !matches!(surface.entry.arg, AppArg::Ott)))
    {
        // A newer structural request superseded Present. Keep the foreign panel
        // and the original Menu; there is no Settings surface to retire.
        let page = origin.page;
        runtime.menu_settings.origin = None;
        if runtime.reply.is_none() && current_ui_owner(app).is_none() { runtime.page = page; }
        runtime.changed();
        return;
    }
    if origin.panel.is_some_and(|(_, owner)| current_ui_owner(app).is_some_and(|current| current != owner)) {
        retire(runtime, app, false);
        return;
    }
    let panel_missing = origin.panel.is_some_and(|(entry, owner)|
        app.pages.nav.entry_of_instance(owner) != Some(entry));
    if !current || origin.abandoned {
        if origin.panel.is_none() && app.pages.top_page() == Some(origin.player)
            && app.pages.nav.entry_of_instance(origin.owner) == Some(origin.menu)
        {
            // Present may still be queued. Keep the neutral projection until its
            // real Refresh identifies the surface; never fall through to groups.
            runtime.menu_settings.origin.as_mut().unwrap().abandoned = true;
            return;
        }
        retire(runtime, app, false);
        return;
    }
    if origin.returning {
        if panel_missing { runtime.menu_settings.origin = None; }
        return;
    }
    if panel_missing { retire(runtime, app, false); return; }
    let Some((_, owner)) = origin.panel else { return; };
    if current_ui_owner(app) != Some(owner) || runtime.reply != Some(owner)
        || !player_browse::ready(app) || app.pages.has_pending_surface_navigation() { return; }
    let result = input_policy::ui(runtime, &OttAction::ShowSettings)
        .and_then(|_| runtime.require_parental(parental::Target::Settings));
    let authorized = result.is_ok();
    let previous = runtime.menu_settings.origin.as_ref().unwrap().authorized;
    if authorized != previous {
        runtime.menu_settings.origin.as_mut().unwrap().authorized = authorized;
        if !authorized { settings_groups::cancel(runtime); }
        runtime.changed();
    }
    if let Err(error) = result { runtime.status = error.into(); }
}

/// Called before ordinary action admission. A covered/obsolete Settings origin
/// cannot use a stale row to operate on the new source or bypass its PIN gate.
pub(super) fn guard(runtime: &Runtime, app: &App, action: &OttAction) -> Result<(), &'static str> {
    let Some(origin) = runtime.menu_settings.origin.as_ref() else { return Ok(()); };
    if origin.returning || origin.abandoned || !source_current(origin, runtime, app)
        || origin.panel.map(|(_, owner)| owner) != runtime.reply
        || runtime.reply != current_ui_owner(app)
        || !player_browse::ready(app) || app.pages.has_pending_surface_navigation()
    { return Err(CHANGED); }
    if origin.authorized || matches!(action, OttAction::Back | OttAction::Refresh
        | OttAction::CancelSetting { .. })
        || matches!(action, OttAction::EditSetting(key) | OttAction::SubmitSetting { key, .. }
            if key == "parental.unlock")
    { Ok(()) } else { Err("Parental access is locked. Unlock from Settings first.") }
}

pub(super) fn back(runtime: &mut Runtime, app: &mut App, action: &OttAction)
    -> Option<Result<(), &'static str>>
{
    if !matches!(action, OttAction::Back) || runtime.page != "settings"
        || runtime.menu_settings.origin.is_none() { return None; }
    if let Err(error) = guard(runtime, app, action) { return Some(Err(error)); }
    if runtime.menu_settings.origin.as_ref().unwrap().authorized {
        runtime.menu_settings.memory = Some(app.pages.return_state().memory);
    }
    retire(runtime, app, true);
    Some(Ok(()))
}

pub(super) fn view(runtime: &Runtime) -> Option<OttView> {
    let origin = runtime.menu_settings.origin.as_ref()?;
    if origin.authorized || origin.returning { return None; }
    Some(OttView {
        page_id: "settings-access".into(), title: "Settings".into(),
        description: String::new(), status: runtime.status.clone(), revision: runtime.revision,
        rows: vec![
            ui_projection::row("parental.unlock".into(), "Unlock parental access".into(),
                String::new(), OttAction::EditSetting("parental.unlock".into()), false),
            ui_projection::row("settings.return".into(), "Back".into(), String::new(), OttAction::Back, false),
        ], ..Default::default()
    })
}

/// Runs after the new grouped snapshot. Shared screen memory restores stable
/// IDs, allowing the normal focus reconciliation to discard a hidden row.
pub(super) fn restore(runtime: &mut Runtime, app: &mut App) {
    let Some(origin) = runtime.menu_settings.origin.as_ref() else { return; };
    let Some((_, owner)) = origin.panel else { return; };
    if origin.returning || !origin.authorized || runtime.edit_session.is_some() || current_ui_owner(app) != Some(owner) { return; }
    if let Some(memory) = runtime.menu_settings.memory.take() {
        app.pages.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(owner),
            Delivery::Screen(plx_ui::screen::ScreenEvent::RestoreMemory(memory))));
    }
}

/// Display continuity for the retained More, including the Settings closing fade.
/// Actual OpenSettings still passes the strict current-owner admission above.
pub(super) fn display_offer(runtime: &Runtime, app: &App) -> Option<plx_media::route::PlaybackIdentity> {
    let origin = runtime.menu_settings.origin.as_ref()?;
    (!origin.abandoned && source_current(origin, runtime, app) && runtime.loaded && !runtime.persistence_uncertain
        && !input_policy::strict(runtime)
        && playback_allowed(app) && !app.pages.nav.suspended)
        .then(|| origin.source.clone())
}
