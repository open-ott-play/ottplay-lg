//! Volatile catalogue binding for an already-authorized native Plex session.
//! Never writes Saved, selects an account, copies credentials or starts a worker.
use super::*;
use crate::app::ott_plex::ClassicRead;

const CHANGED: &str = "Plex source is no longer available";

/// Structural witness for the existing parental gate. Retaining it neither
/// refreshes the source nor copies its credentials into a second owner.
#[derive(Clone)]
pub(super) struct ClassicAuthority {
    epoch: u64,
    selected_server: plx_plex::plex::ServerId,
    scope: plx_plex::plex::grant::GrantScope,
    authority: Arc<crate::app::ott_plex_prepare::CapturedAuthority>,
}
impl ClassicAuthority {
    pub(super) fn same_authority(&self, other: &Self) -> bool {
        self.epoch == other.epoch
            && self.selected_server == other.selected_server
            && self.scope == other.scope
            && self.authority.server == other.authority.server
            && self.authority.machine_id == other.authority.machine_id
            && self.authority.client_key == other.authority.client_key
            && self.authority.grant_epoch == other.authority.grant_epoch
    }
}

/// Capture only a currently admitted Classic source. A stale binding is an
/// error before the caller can fall back to unrelated durable credentials.
pub(super) fn parental_authority(runtime: &Runtime)
    -> Result<Option<ClassicAuthority>, &'static str>
{
    if runtime.transient.accepted.is_some() || managed::profile(runtime).is_some() {
        return Ok(None);
    }
    let Some(binding) = runtime.classic_source.active.as_ref() else { return Ok(None); };
    if binding.generation != runtime.generation
        || binding.selected_server != plx_plex::plex::current_server()
        || !durable_current(runtime, binding)
        || runtime.effective_profile() != Some(&binding.profile)
    {
        return Err(CHANGED);
    }
    let (authority, scope) = binding.read.authority_scope().ok_or(CHANGED)?;
    Ok(Some(ClassicAuthority {
        epoch: binding.epoch, selected_server: binding.selected_server, scope, authority,
    }))
}

#[derive(Default)]
pub(super) struct State {
    // Runtime-owned namespace identifies old issued profiles after retirement.
    // The checked serial prevents an A -> B -> A session from reviving old rows.
    prefix: String,
    serial: u64,
    active: Option<Binding>,
}
struct Binding {
    profile: ProviderProfile,
    durable: bool,
    epoch: u64,
    selected_server: plx_plex::plex::ServerId,
    generation: u64,
    read: ClassicRead,
    category: String,
}
impl Binding {
    fn current(&self, app: &App) -> bool {
        app.bridge.session_ready_current()
            && app.bridge.auth_read().0.flow_epoch == self.epoch
            && self.read.current()
            && self.selected_server == plx_plex::plex::current_server()
    }
}

pub(super) fn temporary_id(runtime: &Runtime, id: &str) -> bool {
    !runtime.classic_source.prefix.is_empty() && id.starts_with(&runtime.classic_source.prefix)
}
pub(super) fn owns_id(runtime: &Runtime, id: &str) -> bool {
    temporary_id(runtime, id)
        || runtime.classic_source.active.as_ref().is_some_and(|binding| binding.profile.id == id)
}
pub(super) fn profile(runtime: &Runtime) -> Option<&ProviderProfile> {
    runtime.classic_source.active.as_ref()
        .filter(|binding| binding.generation == runtime.generation)
        .map(|binding| &binding.profile)
}
/// Only genuinely temporary profiles supplement the durable Favorite inventory.
pub(super) fn binding_profile(runtime: &Runtime) -> Option<&ProviderProfile> {
    runtime.classic_source.active.as_ref()
        .filter(|binding| !binding.durable && binding.generation == runtime.generation)
        .map(|binding| &binding.profile)
}
/// Resolve the existing durable category owner without treating a temporary
/// Classic binding as its unrelated active Saved profile. Live source authority
/// is checked by the existing admission/reconcile path; this is a pure metadata
/// projection and refuses a replaced or retired durable binding.
pub(super) fn category_profile(runtime: &Runtime) -> Result<Option<&ProviderProfile>, &'static str> {
    let Some(binding) = runtime.classic_source.active.as_ref() else {
        return Ok(runtime.active());
    };
    if binding.generation != runtime.generation || !durable_current(runtime, binding) {
        return Err(CHANGED);
    }
    if !binding.durable { return Ok(None); }
    if !binding.profile.enabled { return Err(CHANGED); }
    Ok(Some(&binding.profile))
}
pub(super) fn view_selection(runtime: &Runtime) -> Option<&str> {
    profile(runtime)?;
    runtime.classic_source.active.as_ref().map(|binding| binding.category.as_str())
}
pub(super) fn set_view(runtime: &mut Runtime, category: &str) {
    if let Some(binding) = runtime.classic_source.active.as_mut() {
        binding.category = category.into();
    }
}
pub(super) fn guard_profile(runtime: &Runtime, app: &App, profile: &ProviderProfile)
    -> Result<(), &'static str>
{
    if !owns_id(runtime, &profile.id) { return Ok(()); }
    let binding = runtime.classic_source.active.as_ref().ok_or(CHANGED)?;
    if binding.profile != *profile || binding.generation != runtime.generation
        || !binding.current(app) || !durable_current(runtime, binding) { return Err(CHANGED); }
    Ok(())
}
pub(super) fn capture_read(runtime: &Runtime, app: &App, profile: &ProviderProfile)
    -> Result<Option<ClassicRead>, &'static str>
{
    if !owns_id(runtime, &profile.id) { return Ok(None); }
    guard_profile(runtime, app, profile)?;
    Ok(runtime.classic_source.active.as_ref().map(|binding| binding.read.clone()))
}
pub(super) fn guard_catalog_event(runtime: &Runtime, app: &App, generation: u64)
    -> Result<(), &'static str>
{
    if let Some(binding) = &runtime.classic_source.active {
        if binding.generation == generation && (!binding.current(app) || !durable_current(runtime, binding)) { return Err(CHANGED); }
    }
    Ok(())
}
pub(super) fn guard_metadata_event(runtime: &Runtime, app: &App, profile_id: &str)
    -> Result<(), &'static str>
{
    if !owns_id(runtime, profile_id) { return Ok(()); }
    let binding = runtime.classic_source.active.as_ref().ok_or(CHANGED)?;
    if binding.profile.id != profile_id || !binding.current(app) || !durable_current(runtime, binding) { return Err(CHANGED); }
    Ok(())
}
pub(super) fn rebase(runtime: &mut Runtime, old_generation: u64) {
    if let Some(binding) = runtime.classic_source.active.as_mut()
        .filter(|binding| binding.generation == old_generation)
    {
        binding.generation = runtime.generation;
    }
}
fn durable_current(runtime: &Runtime, binding: &Binding) -> bool {
    !binding.durable || runtime.saved.profiles.profiles.iter()
        .filter(|profile| profile.id == binding.profile.id).eq(std::iter::once(&binding.profile))
}
/// A regular configured-source load supersedes this browse-only binding. Its
/// caller immediately cancels the catalogue/generation, including same-ID work.
pub(super) fn release(runtime: &mut Runtime) {
    classic_bookmark::release(runtime);
    if let Some(binding) = runtime.classic_source.active.take() {
        if runtime.play_profile.as_ref().is_some_and(|(_, profile)| profile == &binding.profile) {
            runtime.cancel_play_request();
        }
    }
}
pub(super) fn reconcile(runtime: &mut Runtime, app: &App) {
    // A committed source replacement owns its own catalogue. Retire only the
    // old Classic receipt/ticket; never clear the new source's rows or queue.
    if runtime.transient.accepted.is_some() || managed::profile(runtime).is_some() {
        release(runtime);
        return;
    }
    let invalid = runtime.classic_source.active.as_ref().is_some_and(|binding|
        binding.generation != runtime.generation || !binding.current(app)
            || !durable_current(runtime, binding));
    if !invalid { return; }
    release(runtime);
    // All known retained-catalog generation changes explicitly rebase above
    // their next use. An unrecognized mismatch is a refusal, never a fallback
    // that pairs old Plex rows with a configured source of another kind.
    runtime.cancel_catalog();
    runtime.catalog = Catalog::default();
    runtime.selected = None;
    runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
    runtime.status = CHANGED.into();
    runtime.changed();
}
pub(super) fn refresh(runtime: &mut Runtime) -> bool {
    let Some(profile) = profile(runtime).cloned() else { return false; };
    runtime.load_catalog_profile(Some(profile), true);
    true
}

/// A native route names its server explicitly where possible. Discover-only
/// content has no local-server authority; the coordinator shows its safe fallback.
fn route_server(app: &App) -> Option<plx_plex::plex::ServerId> {
    use plx_screens::registry::ContentArg;
    match app.route() {
        AppArg::Content(ContentArg::Detail { sid, .. }
            | ContentArg::Person { sid, .. } | ContentArg::Filmography { sid, .. }) => Some(sid),
        AppArg::Content(ContentArg::Collection(collection)) => Some(collection.sid),
        AppArg::Content(ContentArg::Catalog { .. }) => None,
        AppArg::Home | AppArg::Library | AppArg::LibraryCollections { .. }
            | AppArg::LibraryRecommended { .. } | AppArg::Search | AppArg::Watchlist =>
            Some(plx_plex::plex::current_server()),
        _ => None,
    }
}

/// Preserve only a catalogue of the same native Plex source. An unrelated saved
/// provider is never substituted for the native page, nor modified by this view.
pub(super) fn ensure_catalog(runtime: &mut Runtime, app: &mut App)
    -> Result<bool, &'static str>
{
    let server = route_server(app);
    ensure_source(runtime, app, server)
}

/// The explicit Player catalogue door validates its current native playback
/// identity and input owner before passing the corresponding server here.
/// This function admits catalogue reads only; it never creates Playing state.
pub(super) fn ensure_server_catalog(runtime: &mut Runtime, app: &mut App,
    server: plx_plex::plex::ServerId) -> Result<bool, &'static str>
{
    ensure_source(runtime, app, Some(server))
}

fn ensure_source(runtime: &mut Runtime, app: &mut App,
    server: Option<plx_plex::plex::ServerId>) -> Result<bool, &'static str>
{
    reconcile(runtime, app);
    if !runtime.loaded || runtime.persistence_uncertain
        || input_policy::navigation(runtime).is_err() { return Ok(false); }
    if !app.bridge.session_ready_current() { return Ok(false); }
    let Some(sid) = server else { return Ok(false); };
    let Some(client) = plx_plex::plex::client_for(sid) else { return Ok(false); };
    let read = match ClassicRead::capture(client) { Ok(read) => read, Err(_) => return Ok(false) };
    let source = client.machine_id();
    if let Some(existing) = profile(runtime) {
        if existing.source == source {
            guard_profile(runtime, app, existing)?;
            return Ok(true);
        }
    }
    if let Some(existing) = managed::profile(runtime) {
        if existing.kind == ProviderKind::Plex && existing.source == source {
            managed::guard_profile(runtime, app, existing)?;
            return Ok(true);
        }
    }
    // A mode projection cannot evict an independently admitted transient or
    // managed source. Keep its existing owner and the native route unchanged.
    if runtime.transient.accepted.is_some() || managed::profile(runtime).is_some() {
        return Err("The current temporary source must be closed before changing catalogue");
    }
    input_policy::replace_source(runtime)?;
    runtime.require_parental(parental::Target::Providers)?;
    let epoch = app.bridge.auth_read().0.flow_epoch;
    let mut durable = runtime.saved.profiles.profiles.iter().filter(|profile|
        profile.enabled && profile.kind == ProviderKind::Plex && profile.source == source);
    let saved_profile = durable.next().cloned();
    if durable.next().is_some() { return Err("More than one saved Plex profile matches this source"); }
    let is_durable = saved_profile.is_some();
    let profile = if let Some(profile) = saved_profile { profile } else {
        if runtime.classic_source.prefix.is_empty() {
            let hash = plx_data::ott::favorites_wire::hash(runtime.remote.runtime().as_bytes());
            runtime.classic_source.prefix = format!("classic-{}-", &hash[..16]);
        }
        if runtime.saved.profiles.profiles.iter().any(|profile|
            profile.id.starts_with(&runtime.classic_source.prefix)) {
            return Err("Classic source identity conflicts with a saved profile");
        }
        let serial = runtime.classic_source.serial.checked_add(1).ok_or("Classic source identity exhausted")?;
        runtime.classic_source.serial = serial;
        ProviderProfile {
            id: format!("{}{serial}", runtime.classic_source.prefix),
            name: "Plex".into(), kind: ProviderKind::Plex,
            source: source.into(), source_alias: String::new(), enabled: true,
        }
    };
    profile.validate().map_err(|_| CHANGED)?;
    if !read.matches(&profile.source) || !read.current() { return Ok(false); }
    let category = if is_durable {
        category_state::remembered_category(&runtime.saved, &profile)?
    } else { String::new() };
    // Retire an older binding's pending playback ticket before replacing it.
    // Running playback belongs to the player and is deliberately untouched.
    release(runtime);
    runtime.classic_source.active = Some(Binding {
        profile: profile.clone(), durable: is_durable, epoch,
        selected_server: plx_plex::plex::current_server(),
        generation: runtime.generation, read, category,
    });
    // Preexisting ordinary catalogues have no captured session receipt. Even
    // the same saved profile must obtain fresh rows under this read; cancellation
    // advances the generation before any old queued Batch/Finished can land.
    // The loader preserves selection text; category above is source-local.
    runtime.load_catalog_profile(Some(profile), true);
    runtime.parental_source_installed();
    classic_bookmark::bind(runtime, app);
    Ok(true)
}
