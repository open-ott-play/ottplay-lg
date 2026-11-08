//! Return to an already-published managed Session. No offered context is created,
//! consumed or replayed here; catalogue ownership is separate from source identity.
use super::*;

fn same_profile(a: &ProviderProfile, b: &ProviderProfile) -> bool {
    a.id == b.id && a.kind == b.kind && a.source == b.source
}
/// Identity lookup only. A dormant catalogue does not remove an active source
/// from Favorites; each actual lookup/playback still needs the resource guard.
pub(crate) fn binding_profile(runtime: &Runtime) -> Option<&ProviderProfile> {
    runtime.managed.active.as_ref().map(|active| &active.profile)
}
pub(crate) fn owns_retained_profile(runtime: &Runtime, profile: &ProviderProfile) -> bool {
    runtime.managed.active.as_ref().is_some_and(|a| same_profile(&a.profile, profile))
        || runtime.managed.retired_profile.as_ref().is_some_and(|old| same_profile(old, profile))
}
pub(crate) fn guard_profile(runtime: &Runtime, app: &App, profile: &ProviderProfile) -> Result<(), &'static str> {
    if !owns_retained_profile(runtime, profile) { return Ok(()); }
    let active = runtime.managed.active.as_ref().filter(|a| same_profile(&a.profile, profile))
        .ok_or("Plex source is no longer available")?;
    if active.source.retired.load(Ordering::Acquire) || active.cancel.load(Ordering::Acquire)
        || !app.bridge.managed_active_current(active.operation) {
        return Err("Plex source is no longer available");
    }
    Ok(())
}
pub(crate) fn capture_read(runtime: &Runtime, app: &App, profile: &ProviderProfile)
    -> Result<Option<crate::app::adapters::session::ManagedRead>, &'static str> {
    if !owns_retained_profile(runtime, profile) { return Ok(None); }
    guard_profile(runtime, app, profile)?;
    let operation = runtime.managed.active.as_ref().ok_or("Plex source is no longer available")?.operation;
    app.bridge.managed_read(operation).map(Some).ok_or("Plex source is no longer available")
}
pub(crate) fn record_metadata(runtime: &mut Runtime, profile: &ProviderProfile,
    read: Option<&crate::app::adapters::session::ManagedRead>) {
    runtime.managed.metadata = read.map(|read| (runtime.metadata_generation, profile.clone(), read.clone()));
}
pub(crate) fn record_catalog(runtime: &mut Runtime, profile: &ProviderProfile,
    read: &crate::app::adapters::session::ManagedRead) {
    runtime.managed.catalog = Some((runtime.generation, profile.clone(), read.clone()));
}
pub(crate) fn guard_catalog_event(runtime: &Runtime, app: &App, generation: u64) -> Result<(), &'static str> {
    if let Some((captured, profile, read)) = &runtime.managed.catalog {
        if *captured == generation {
            if !read.current() { return Err("Plex source is no longer available"); }
            return guard_profile(runtime, app, profile);
        }
    }
    Ok(())
}
pub(crate) fn guard_metadata_event(runtime: &Runtime, app: &App, generation: u64,
    profile_id: &str) -> Result<(), &'static str> {
    if let Some((captured, profile, read)) = &runtime.managed.metadata {
        if *captured == generation && profile.id == profile_id {
            if !read.current() { return Err("Plex source is no longer available"); }
            return guard_profile(runtime, app, profile);
        }
    }
    Ok(())
}
pub(super) fn observe(runtime: &mut Runtime, app: &App) {
    let ready = runtime.managed.active.as_ref()
        .is_some_and(|active| guard_profile(runtime, app, &active.profile).is_ok());
    if runtime.managed.reentry_ready != ready {
        runtime.managed.reentry_ready = ready;
        runtime.changed();
    }
}
/// Generic provider navigation can expose the existing owner without starting
/// account login, replacing playback or writing an ordinary Saved profile.
pub(crate) fn browse_active(runtime: &mut Runtime, app: &App) -> bool {
    observe(runtime, app);
    if runtime.managed.reentry_ready { return true; }
    let network_only = runtime.managed.active.as_ref().is_some_and(|active|
        !active.source.retired.load(Ordering::Acquire) && !active.cancel.load(Ordering::Acquire)
            && app.bridge.snapshot_session_init().managed_operation() == Some(active.operation)
            && active.source.verified.proof.freshness() == plx_plex::plex::grant::ImportFreshness::NetworkChanged);
    if network_only { runtime.managed.refresh_requested = true; }
    network_only
}
pub(super) fn row(runtime: &Runtime) -> Option<OttRow> {
    let active = runtime.managed.active.as_ref()?;
    let mut row = ui_projection::row(format!("managed-active/{}", active.operation),
        active.profile.name.clone(), String::new(),
        OttAction::ResumeManagedPlex { operation: active.operation }, false);
    row.enabled = runtime.managed.reentry_ready;
    Some(row)
}
pub(crate) fn resume_ui(runtime: &mut Runtime, app: &mut App, operation: u64) -> Result<(), &'static str> {
    policy(runtime, app).map_err(Error::code)?;
    if runtime.managed.pending.is_some() { return Err("Plex selection is still pending"); }
    let active = runtime.managed.active.as_ref().filter(|a| a.operation == operation)
        .ok_or("Plex source is no longer available")?;
    if active.source.verified.proof.freshness() == plx_plex::plex::grant::ImportFreshness::NetworkChanged {
        runtime.managed.refresh_requested = true;
        return Err("Plex source needs refresh; select it again when ready");
    }
    guard_profile(runtime, app, &active.profile)?;
    if !app.bridge.resume_managed(operation) { return Err("Plex source is no longer available"); }
    // No checkpoint/save: this only withdraws the previous in-flight catalogue
    // and playback requests after the retained resource has passed its guard.
    runtime.retire_catalog_playback(app);
    retire_for_catalog(app);
    runtime.managed.active.as_mut().expect("checked active owner").generation = runtime.generation;
    runtime.provider = ProviderKind::Plex;
    runtime.selected = None;
    runtime.managed_catalog();
    observe(runtime, app);
    Ok(())
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_managed_reentry_tests.rs"]
mod tests;
