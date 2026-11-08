//! Reversible Classic catalogue presentation over the existing navigation owner.
//! Authentication, consent, modal and playback routes retain their lifetimes.
use super::*;
use plx_machine::machine::{EntryId, InputOwner, NavOp, UiMode};

#[derive(Default)]
pub(super) struct State {
    origin: Option<Origin>,
    startup_considered: bool,
}
#[derive(Clone, Copy)]
struct Origin {
    native: EntryId,
    ott: Option<EntryId>,
    epoch: u64,
    server: plx_plex::plex::ServerId,
}

fn catalogue_page(route: &AppArg) -> bool {
    matches!(route, AppArg::Home | AppArg::Library | AppArg::LibraryCollections { .. }
        | AppArg::LibraryRecommended { .. } | AppArg::Search | AppArg::Watchlist
        | AppArg::Content(_))
}

// The existing Push owns the native return state. Remember only its entry pair,
// not a second copy of route, focus, page memory, credentials or catalogue data.
pub(super) fn observe(runtime: &mut Runtime, app: &App) {
    let Some(mut origin) = runtime.classic_navigation.origin else { return; };
    if !app.bridge.session_ready_current()
        || app.bridge.auth_read().0.flow_epoch != origin.epoch
        || plx_plex::plex::current_server() != origin.server
    {
        runtime.classic_navigation.origin = None;
        return;
    }
    let entries = &app.pages.nav.tabs.stack.entries;
    let Some(index) = entries.iter().position(|entry| entry.id == origin.native) else {
        runtime.classic_navigation.origin = None;
        return;
    };
    let above = entries.get(index + 1);
    if let Some(ott) = origin.ott {
        if above.is_none_or(|entry| entry.id != ott || !matches!(entry.arg, AppArg::Ott)) {
            runtime.classic_navigation.origin = None;
        }
    } else if let Some(entry) = above {
        runtime.classic_navigation.origin = if matches!(entry.arg, AppArg::Ott) {
            origin.ott = Some(entry.id);
            Some(origin)
        } else { None };
    } else if !app.pages.has_pending_navigation() && !app.pages.nav.tabs.stack.is_pending() {
        runtime.classic_navigation.origin = None;
    }
}

fn owned_pages(runtime: &Runtime, app: &App) -> bool {
    let Some(origin) = runtime.classic_navigation.origin else { return false; };
    let Some(ott) = origin.ott else { return false; };
    app.pages.nav.top_page().is_some_and(|entry| entry.id == ott
        && matches!(entry.arg, AppArg::Ott)
        && entry.inst.as_ref().is_some_and(|body| !body.staged))
        && app.pages.nav.tabs.stack.under_top().is_some_and(|entry| entry.id == origin.native)
}

fn owned_top(runtime: &Runtime, app: &App) -> bool {
    owned_pages(runtime, app) && runtime.classic_navigation.origin.and_then(|origin| origin.ott)
        .is_some_and(|ott| app.pages.nav.input_owner() == Some(InputOwner::Entry(ott)))
}

// A saved provider is boot intent; an incidental native Home is not a new
// selection of Plex. The App boot/input stamps already distinguish real input.
fn startup_provider(runtime: &Runtime) -> Option<ProviderKind> {
    runtime.active().map(|profile| profile.kind).or_else(|| {
        match runtime.saved.settings.get("selectedProvider").and_then(Value::as_str) {
            Some("m3u") => Some(ProviderKind::M3u),
            Some("xtream") => Some(ProviderKind::Xtream),
            Some("stalker") => Some(ProviderKind::Stalker),
            Some("vportal") => Some(ProviderKind::VPortal),
            Some("local" | "offline") => Some(ProviderKind::Local),
            _ => None,
        }
    }).filter(|provider| *provider != ProviderKind::Plex)
}

pub(super) fn reconcile(runtime: &mut Runtime, app: &mut App) -> bool {
    observe(runtime, app);
    // Input while state/auth is still loading also retires this one-shot claim.
    if app.last_input != app.t0 || runtime.navigation_epoch != 0 {
        runtime.classic_navigation.startup_considered = true;
    }
    let mode = app.bridge.ui_mode();
    if runtime.interface_mode() != mode
        || !runtime.loaded || runtime.persistence_uncertain
        || public_preferences::write_refusal(runtime).is_some()
        || runtime.setup.is_some() || runtime.edit_session.is_some()
        || input_policy::navigation(runtime).is_err()
        || !playback_allowed(app) || app.pages.nav.suspended
        || app.bridge.is_controlled_replay()
        || matches!(&app.rec, super::super::recorder::Recplay::Replaying(_))
        || !app.bridge.session_ready_current()
        || app.pages.surface_up() || app.pages.has_pending_surface_navigation() || app.pages.has_pending_navigation()
        || app.pages.nav.tabs.stack.is_pending()
        || app.pages.nav.tabs.stack.transition.in_flight()
    { return false; }
    let Some(entry) = app.pages.nav.top_page() else { return false; };
    if entry.inst.as_ref().is_none_or(|body| body.staged)
        || app.pages.nav.input_owner() != Some(InputOwner::Entry(entry.id))
    { return false; }
    let native = entry.id;
    let owner = entry.inst.as_ref().expect("checked mounted page").id;
    let root_home = matches!(entry.arg, AppArg::Home)
        && app.pages.nav.tabs.stack.depth() == 1;
    let native_catalogue = catalogue_page(&entry.arg);
    let startup_route_ready = native_catalogue || matches!(entry.arg, AppArg::Ott | AppArg::Player);
    if !runtime.classic_navigation.startup_considered && startup_route_ready {
        runtime.classic_navigation.startup_considered = true;
        if root_home && runtime.transient.accepted.is_none()
            && managed::profile(runtime).is_none() && classic_source::profile(runtime).is_none()
        {
            if let Some(provider) = startup_provider(runtime) {
                // A configured provider may already own a running/progressive
                // catalogue. Projection preserves its generation, selection,
                // playback and queued reads; it grants no new source authority.
                runtime.provider = provider;
                if runtime.active().is_none() {
                    runtime.page = if runtime.saved.favorites_selected { "favorites" } else { "profiles" };
                }
                runtime.reply = None;
                runtime.changed();
                // This is the initial application route, not a reversible
                // projection of a user-opened native Plex page. Keeping Home
                // underneath would make Modern Back expose the wrong provider.
                app.pages.request(MachineId::Instance(owner), NavOp::Root(AppArg::Ott));
                return true;
            }
        }
    }
    if mode == UiMode::Modern {
        if !owned_top(runtime, app) { return false; }
        let owner = app.pages.top_page().expect("owned mounted page");
        app.pages.request(MachineId::Instance(owner), NavOp::Pop);
        return true;
    }
    if !native_catalogue { return false; }
    let categories = runtime.page == "categories";
    // Source admission sees the unchanged typed route and stages a fresh read
    // for its current session. It never selects/saves a profile or starts video.
    let unavailable = match classic_source::ensure_catalog(runtime, app) {
        Ok(true) => {
            runtime.page = if categories { "categories" }
                else if runtime.view_selection().1 { "favorites" } else { "channels" };
            None
        },
        Ok(false) => {
            if let Err(error) = runtime.require_parental(parental::Target::Providers) {
                return hold(runtime, error);
            }
            Some("Plex source is no longer available")
        },
        Err(error) => return hold(runtime, error),
    };
    if let Some(error) = unavailable {
        runtime.page = "providers";
        runtime.status = error.into();
    }
    runtime.reply = None;
    runtime.changed();
    runtime.classic_navigation.origin = Some(Origin { native, ott: None,
        epoch: app.bridge.auth_read().0.flow_epoch, server: plx_plex::plex::current_server() });
    app.pages.request(MachineId::Instance(owner), NavOp::Push(AppArg::Ott));
    true
}

/// A projected Classic root retains a native mode-return page underneath it.
/// This ownership predicate is shared with the addressed OTT exit confirmation.
pub(super) fn exit_root(runtime: &Runtime, app: &App) -> bool {
    runtime.interface_mode() == UiMode::Classic && app.bridge.ui_mode() == UiMode::Classic
        && runtime.classic_navigation.origin.is_some_and(|origin|
            app.bridge.session_ready_current() && app.bridge.auth_read().0.flow_epoch == origin.epoch
        ) && owned_pages(runtime, app)
}

// A source or policy refusal is not permission to reveal another catalogue.
fn hold(runtime: &mut Runtime, error: &'static str) -> bool {
    if runtime.status != error {
        runtime.status = error.into();
        runtime.changed();
    }
    false
}
