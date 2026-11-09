//! Common semantic input fence. Read-only inspection and lifecycle remain available.
use super::*;
use crate::app::ott_kiosk_policy::State as Kiosk;
const BUSY: &str = "A kiosk transition is being saved. Try again when it settles.";
const PINNED: &str = "This action is restricted by kiosk policy.";

pub(super) fn pending(runtime: &Runtime) -> bool {
    commands::kiosk_pending(runtime) || runtime.kiosk_capture.pending()
}
fn mode(runtime: &Runtime) -> Result<Kiosk, &'static str> {
    runtime
        .track_selection_kiosk()
        .map_err(|_| "Kiosk policy is unavailable.")
}
pub(super) fn strict(runtime: &Runtime) -> bool {
    !matches!(
        mode(runtime),
        Ok(Kiosk::Off | Kiosk::Armed { .. } | Kiosk::Locked { strict: false })
    )
}
pub(super) fn navigation(runtime: &Runtime) -> Result<(), &'static str> {
    if pending(runtime) {
        return Err(BUSY);
    }
    if strict(runtime) {
        return Err(PINNED);
    }
    Ok(())
}
pub(super) fn replace_source(runtime: &Runtime) -> Result<(), &'static str> {
    if pending(runtime) {
        return Err(BUSY);
    }
    match mode(runtime)? {
        Kiosk::Off | Kiosk::Armed { .. } => Ok(()),
        Kiosk::Locked { .. } => Err(PINNED),
    }
}
/// UI navigation can inspect a normal pinned source; profile and source mutation
/// cannot replace it. Actual item admission is additionally checked by the pin binder.
pub(super) fn ui(runtime: &Runtime, action: &OttAction) -> Result<(), &'static str> {
    if matches!(
        action,
        OttAction::CancelSetting { .. } | OttAction::CancelSearch { .. }
    ) {
        return Ok(());
    }
    navigation(runtime)?;
    match action {
        OttAction::Back if matches!(runtime.page, "guide" | "guide-programme") => Ok(()),
        OttAction::ShowActions(_)
        | OttAction::ShowActionsAt { .. }
        | OttAction::ShuffleOrder(_)
        | OttAction::ResetOrder(_)
        | OttAction::MenuVisibility { .. }
        | OttAction::SelectProvider(_)
        | OttAction::SelectProfile(_)
        | OttAction::SelectManagedPlex { .. }
        | OttAction::ResumeManagedPlex { .. }
        | OttAction::EditM3uMediaLink { .. }
        | OttAction::Back => replace_source(runtime),
        OttAction::EditProfile { .. } => replace_source(runtime),
        OttAction::EditSetting(key) if !key.starts_with("parental.") => replace_source(runtime),
        OttAction::SubmitSetting { key, .. } if !key.starts_with("parental.") => {
            replace_source(runtime)
        }
        _ => Ok(()),
    }
}
/// A normal pinned source may close its browse UI without retiring playback.
pub(super) fn browse_back(
    runtime: &mut Runtime,
    app: &mut App,
    action: &OttAction,
) -> Option<Result<(), &'static str>> {
    if !matches!(action, OttAction::Back)
        || matches!(runtime.page, "guide" | "guide-programme")
        || !matches!(mode(runtime), Ok(Kiosk::Locked { strict: false }))
    {
        return None;
    }
    if pending(runtime) {
        return Some(Err(BUSY));
    }
    runtime.cancel_setup();
    if runtime.page != "providers" {
        runtime.page = "providers";
        Some(Ok(()))
    } else if app.pages.nav.tabs.stack.depth() > 1 {
        bridge::nav_pop(&mut app.pages);
        Some(Ok(()))
    } else {
        Some(Err(PINNED))
    }
}
fn operation(inv: &Invocation) -> &str {
    inv.params
        .get("operation")
        .or_else(|| inv.params.get("op"))
        .and_then(Value::as_str)
        .unwrap_or("")
}
pub(super) fn read_only(inv: &Invocation) -> bool {
    match inv.action.as_str() {
        "state" | "status" | "capabilities" | "providers" | "channels" | "settings_compare" => true,
        "categories" => inv.params["op"] == "list",
        "kiosk" => inv.params.get("mode").and_then(Value::as_str) == Some("status"),
        "player_settings" => matches!(operation(inv), "schema" | "get"),
        "profiles" | "profile_export" => true,
        "media_tracks" => matches!(operation(inv), "list" | "get" | "status"),
        "local_media" => matches!(operation(inv), "status" | "list" | "sources"),
        "favorites_global_items" => matches!(operation(inv), "list" | "current"),
        // Recovery has no mutating operation: even ACK is an explicit
        // unsupported response before IO. Keep inspection available in standby.
        "favorites_global_recovery" => true,
        "favorites_global" => matches!(
            operation(inv),
            "describe" | "get" | "list" | "status" | "export"
        ),
        _ => false,
    }
}
/// Classify the existing Plex wire request before its preparation can cancel
/// another catalog or take the shared resolver. Parsing also prevents an extra
/// operation field from disguising a mutating `op` as a status request.
fn plex_remote(runtime: &mut Runtime, inv: &Invocation) -> Result<(), &'static str> {
    use crate::app::ott_plex_queue::{
        self as plex, LibraryRequest, PlexRequest, PrepareKind, QueueRequest,
    };
    let request = plex::parse(&inv.action, &inv.params, runtime.remote.runtime())
        .map_err(|_| "Invalid Plex request")?;
    match request {
        PlexRequest::Queue(QueueRequest::Status)
        | PlexRequest::Library(LibraryRequest::List)
        | PlexRequest::Queue(QueueRequest::Prepare {
            kind: PrepareKind::Preview,
            ..
        })
        | PlexRequest::Library(LibraryRequest::Prepare {
            kind: PrepareKind::Preview,
            ..
        }) => Ok(()),
        PlexRequest::Queue(QueueRequest::Prepare {
            kind: PrepareKind::Play,
            ..
        })
        | PlexRequest::Library(LibraryRequest::Prepare {
            kind: PrepareKind::Play,
            ..
        }) => {
            replace_source(runtime)?;
            runtime.require_parental(parental::Target::Providers)?;
            runtime.require_parental(parental::Target::Channel { marked: true })
        }
        PlexRequest::Queue(QueueRequest::Next | QueueRequest::Previous) => {
            // A normal pinned queue can move within its actual admitted members.
            // The final source binder still checks the exact member and owner.
            navigation(runtime)?;
            runtime.require_parental(parental::Target::Channel { marked: true })
        }
        PlexRequest::Queue(QueueRequest::Stop) => navigation(runtime),
        PlexRequest::Queue(QueueRequest::Repeat(_)) => {
            if pending(runtime) {
                return Err(BUSY);
            }
            // Ordinary repeat never edits or bypasses a durable kiosk policy.
            if !matches!(mode(runtime)?, Kiosk::Off) {
                return Err(PINNED);
            }
            runtime.require_parental(parental::Target::Channel { marked: true })
        }
    }
}
pub(super) fn remote(runtime: &mut Runtime, inv: &Invocation) -> Result<(), &'static str> {
    // Installation consent remains available in standby and strict kiosk. Its
    // closed parser admits no navigation or source-changing operation.
    if matches!(
        inv.action.as_str(),
        "notifications" | "native_playback_diagnostics"
    ) {
        return Ok(());
    }
    if inv.action == "plex_servers" {
        // The strict raw parser and shared UI/Remote selection handler own this
        // family's policy. No lossy op projection may bypass its restrictions.
        return Ok(());
    }
    if matches!(inv.action.as_str(), "plex_queue" | "plex_library") {
        return plex_remote(runtime, inv);
    }
    if read_only(inv) {
        return Ok(());
    }
    if inv.action == "lifecycle" {
        return Ok(());
    }
    if inv.action == "kiosk" {
        // Read-only status returned above. Do not admit a policy write while
        // a track preparation still owns its addressed publication.
        if commands::track_preparation_pending(runtime) {
            return Err("A track change is pending. Try again when it settles.");
        }
        if pending(runtime) {
            return Err(BUSY);
        }
        if inv.params.get("query").is_some() {
            return runtime.require_parental(parental::Target::Channel { marked: true });
        }
        // Off, strict-only adjustment and bare ordinary arm do not designate
        // protected media. The source owner checks queue/target/capture admission.
        return Ok(());
    }
    if pending(runtime) {
        return Err(BUSY);
    }
    if strict(runtime) {
        // Authenticated seek addresses the existing playback only; it cannot
        // change the source. All source-changing commands still hit the binder.
        let safe_seek =
            inv.action == "playback" && matches!(operation(inv), "seek" | "seek_relative");
        if !safe_seek {
            return Err(PINNED);
        }
    }
    match inv.action.as_str() {
        "provider" | "profile" | "profile_import" | "profile_save" | "provider_settings"
        | "profile_settings" | "local_media" => {
            replace_source(runtime)?;
            runtime.require_parental(parental::Target::Providers)
        }
        "local_browser" => runtime.require_parental(parental::Target::Providers),
        "player_settings" | "player_settings_patch" => {
            runtime.require_parental(parental::Target::Settings)
        }
        "play" | "play_catalog" | "favorites_global_items" => {
            runtime.require_parental(parental::Target::Channel { marked: true })
        }
        "playback"
            if matches!(
                operation(inv),
                "next_channel" | "previous_channel" | "step_channel"
            ) =>
        {
            runtime.require_parental(parental::Target::Channel { marked: true })
        }
        _ => Ok(()),
    }
}
pub(super) fn player(runtime: &Runtime, request: &plx_screens::registry::PlayerReq) -> bool {
    use plx_screens::registry::PlayerReq;
    if !pending(runtime) && !strict(runtime) {
        return true;
    }
    matches!(
        request,
        PlayerReq::ExtendHud(_) | PlayerReq::ExtendDefaultHud | PlayerReq::ArmInfoPress
    )
}
/// Source-changing effects from other native screens have no OTT item proof.
/// A locked source must never be replaced through that alternate route.
pub(super) fn foreign_content(runtime: &mut Runtime) -> Result<(), &'static str> {
    replace_source(runtime)?;
    runtime.require_parental(parental::Target::Channel { marked: true })
}
