//! A remote number addresses the retained configured catalogue, not a temporary queue.
//! One bounded moved catalogue survives a temporary chain; no settings or URL are revived.
use super::*;
use std::io::Write;

const LIMIT: usize = 8 * 1024 * 1024;
const CHANGED: &str = "The configured channel list is unavailable or changed";

#[derive(Default)]
pub(super) struct State {
    base: Option<Box<Base>>,
    pending: Option<Box<Pending>>,
}
struct Base {
    catalog: Catalog,
    profile: ProviderProfile,
    // Remote numbers are canonical catalogue indices, even in an organized view.
    indices: Vec<usize>,
    digest: [u8; 32],
    runtime: String,
}
struct Pending {
    invocation: Invocation,
    request: PlaybackRequest,
    index: usize,
    owner: OwnerToken,
    generation: u64,
    play_generation: u64,
    playback_epoch: u64,
    last_input: u32,
    ticket: plx_media::route::PlayRequestTicket,
}
struct LimitedDigest {
    bytes: usize,
    digest: plx_base::sha256::Sha256,
}
impl LimitedDigest {
    fn new() -> Self { Self { bytes: 0, digest: plx_base::sha256::Sha256::new() } }
}
impl Write for LimitedDigest {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self.bytes.checked_add(bytes.len()).filter(|n| *n <= LIMIT)
            .ok_or_else(|| std::io::Error::other("retained catalogue budget"))?;
        self.digest.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
fn digest(runtime: &Runtime) -> Option<[u8; 32]> {
    let mut writer = LimitedDigest::new();
    // Resume progress is deliberately excluded; source, organization and Favorite
    // edits are not. Streaming avoids a second serialized settings allocation.
    serde_json::to_writer(&mut writer, &(
        &runtime.saved.profiles, &runtime.saved.settings, &runtime.saved.category,
        runtime.saved.favorites_selected, &runtime.saved.favorites,
        &runtime.saved.favorite_items, &runtime.saved.local_favorite_proofs,
    )).ok()?;
    Some(writer.digest.finish())
}
fn optional(value: &Option<String>) -> usize { value.as_ref().map_or(0, String::capacity) }
fn value_heap(value: &Value) -> usize {
    match value {
        Value::String(value) => value.capacity(),
        Value::Array(values) => values.capacity().saturating_mul(std::mem::size_of::<Value>())
            .saturating_add(values.iter().map(value_heap).fold(0usize, usize::saturating_add)),
        // The pinned serde backend uses BTreeMap. Charge sparse-node/container
        // overhead separately from keys/values; this is a conservative estimate.
        Value::Object(values) => values.iter().fold(1024usize, |bytes, (key, value)|
            bytes.saturating_add(256).saturating_add(key.capacity()).saturating_add(value_heap(value))),
        _ => 0,
    }
}
fn entry_heap(entry: &CatalogEntry) -> usize {
    let mut bytes = [&entry.id, &entry.title, &entry.category, &entry.description, &entry.poster]
        .iter().map(|value| value.capacity()).fold(0usize, usize::saturating_add);
    bytes = bytes.saturating_add(optional(&entry.provider_id)).saturating_add(optional(&entry.provider_id_field));
    let media = match &entry.media {
        MediaLocator::Http { url } => url.capacity(),
        MediaLocator::Local { relative_path } => relative_path.capacity(),
        MediaLocator::VPortal { request } => value_heap(request),
        MediaLocator::Plex { server_id, rating_key } => server_id.capacity().saturating_add(rating_key.capacity()),
        MediaLocator::Xtream { stream_id, stream_kind, extension, series_id, .. } => stream_id.capacity()
            .saturating_add(stream_kind.capacity()).saturating_add(extension.capacity()).saturating_add(optional(series_id)),
        MediaLocator::Stalker { cmd, stream_kind, series } => cmd.capacity().saturating_add(stream_kind.capacity()).saturating_add(series.capacity()),
        MediaLocator::StalkerApi { channel_id } => channel_id.capacity(),
        MediaLocator::ServerLibrary { source_id, item_id, relative_path, root_alias } => source_id.capacity()
            .saturating_add(item_id.capacity()).saturating_add(optional(relative_path)).saturating_add(optional(root_alias)),
    };
    bytes = bytes.saturating_add(media);
    if let Some(live) = &entry.live {
        bytes = bytes.saturating_add(live.tvg_name.capacity()).saturating_add(live.guide_feed.capacity())
            .saturating_add(optional(&live.guide_id))
            .saturating_add(live.catchup.as_ref().map_or(0, |source| source.source.capacity()));
    }
    bytes
}
fn bounded(catalog: &Catalog, indices: &Vec<usize>, profile: &ProviderProfile) -> bool {
    if catalog.entries.len() > plx_data::ott::MAX_CATALOG_ENTRIES { return false; }
    let mut encoded = LimitedDigest::new();
    if serde_json::to_writer(&mut encoded, catalog).is_err() { return false; }
    let mut bytes = std::mem::size_of::<Base>()
        .saturating_add(catalog.entries.capacity().saturating_mul(std::mem::size_of::<CatalogEntry>()))
        .saturating_add(indices.capacity().saturating_mul(std::mem::size_of::<usize>()))
        .saturating_add(catalog.entries.iter().map(entry_heap).fold(0usize, usize::saturating_add));
    for value in [&profile.id, &profile.name, &profile.source, &profile.source_alias] {
        bytes = bytes.saturating_add(value.capacity());
    }
    if let Some(graph) = &catalog.hierarchy {
        bytes = bytes.saturating_add(graph.nodes.capacity().saturating_mul(std::mem::size_of::<plx_data::ott::CategoryNode>()))
            .saturating_add(graph.memberships.capacity().saturating_mul(std::mem::size_of::<plx_data::ott::CategoryMembership>()));
        for node in &graph.nodes {
            bytes = bytes.saturating_add(node.id.capacity()).saturating_add(node.label.capacity())
                .saturating_add(optional(&node.parent)).saturating_add(optional(&node.anonymous_fingerprint));
            if let Some(plx_data::ott::CategorySourceIdentity::Plex { source_id, .. }) = &node.source_identity {
                bytes = bytes.saturating_add(source_id.capacity());
            }
        }
        for member in &graph.memberships {
            bytes = bytes.saturating_add(member.entry_id.capacity()).saturating_add(member.category_id.capacity());
        }
    }
    // Leave room for small retained strings and allocator bookkeeping. This is
    // an admission budget, not a claim about the process's total RSS.
    bytes.saturating_add(4096) <= LIMIT
}

/// Called only at successful native temporary publication, before replacing rows.
pub(super) fn capture(runtime: &mut Runtime) {
    if runtime.transient.accepted.is_some() {
        runtime.transient.numeric.pending = None;
        return;
    }
    runtime.transient.numeric = State::default();
    if !runtime.loaded || runtime.persistence_uncertain || !runtime.catalog_complete
        || runtime.catalog_loading || runtime.catalog.entries.is_empty()
        || runtime.catalog.entries.len() > plx_data::ott::MAX_CATALOG_ENTRIES
        || runtime.saved.favorites_selected || managed::profile(runtime).is_some()
        || classic_source::profile(runtime).is_some()
    { return; }
    let Some(profile) = runtime.active().filter(|profile| profile.enabled && !profile.source.is_empty()).cloned() else { return; };
    if runtime.effective_profile() != Some(&profile) { return; }
    let indices = runtime.organization_rows("").into_iter().map(|(index, _)| index).collect();
    if !bounded(&runtime.catalog, &indices, &profile) { return; }
    let Some(digest) = digest(runtime) else { return; };
    runtime.transient.numeric.base = Some(Box::new(Base {
        catalog: std::mem::take(&mut runtime.catalog), profile, indices, digest,
        runtime: runtime.remote.runtime().into(),
    }));
}
pub(super) fn clear(runtime: &mut Runtime) { runtime.transient.numeric = State::default(); }
fn base_current(runtime: &Runtime) -> bool {
    runtime.transient.numeric.base.as_ref().is_some_and(|base|
        runtime.loaded && !runtime.persistence_uncertain && runtime.active() == Some(&base.profile)
        && runtime.remote.runtime() == base.runtime && digest(runtime) == Some(base.digest))
}
fn pending_current(runtime: &Runtime, app: Option<&App>, after_native: bool) -> bool {
    let Some(pending) = runtime.transient.numeric.pending.as_ref() else { return false; };
    base_current(runtime) && pending.generation == runtime.generation
        && pending.play_generation == runtime.play_generation && pending.playback_epoch == runtime.playback_epoch
        && runtime.transient.accepted.as_ref().map(transient::TransientMedia::owner) == Some(pending.owner)
        && runtime.transient.candidate.is_none()
        && pending.request.local_selection.as_ref().is_none_or(|selected| {
            let Some(base) = runtime.transient.numeric.base.as_ref() else { return false; };
            base.profile.kind == ProviderKind::Local
                && plx_data::ott::http_url(&base.profile.source).is_none()
                && runtime.local_profile_selection(&base.profile).as_ref() == Some(selected)
                && runtime.capture_local_source(selected).is_some_and(|read|
                    runtime.local_source_read_is_current(&read))
        })
        && runtime.play_target.as_ref() == Some(&pending.request)
        && runtime.pending_play.as_ref().is_some_and(|invocation| invocation.id == pending.invocation.id
            && invocation.action == pending.invocation.action && invocation.params == pending.invocation.params
            && invocation.deadline_ms == pending.invocation.deadline_ms)
        && runtime.remote.invocation_is_current(&pending.invocation)
        && pending.invocation.deadline_ms > runtime.now_ms()
        && (after_native || plx_media::route::play_request_is_current(pending.ticket))
        && app.is_none_or(|app| app.running && playback_allowed(app) && app.last_input == pending.last_input)
}
/// Only authenticated explicit numeric play uses this path. Physical digits,
/// queue steps and exact catalogue receipt playback keep their existing scope.
pub(super) fn handles(runtime: &Runtime, query: &str) -> bool {
    let query = query.trim();
    runtime.transient.accepted.is_some()
        && !query.is_empty() && query.bytes().all(|byte| byte.is_ascii_digit())
}
pub(super) fn command(runtime: &mut Runtime, app: &mut App, inv: &Invocation, query: &str) -> Option<Value> {
    let query = query.trim();
    let result = (|| {
        if !app.running || !playback_allowed(app) || lifecycle::viewer_paused()
            || app.player.pending_exit.is_some() || !runtime.play_work_allowed()
        { return Err("Playback is suspended"); }
        if runtime.transient.numeric.base.is_none() { return Err("No complete configured channel list was retained before temporary playback"); }
        if !base_current(runtime) || runtime.transient.candidate.is_some() { return Err(CHANGED); }
        let number = query.parse::<usize>().ok().filter(|number| *number > 0).ok_or("Choose a positive channel number")?;
        let base = runtime.transient.numeric.base.as_ref().ok_or(CHANGED)?;
        let index = number - 1;
        if !base.indices.contains(&index) { return Err("Channel number is not in the configured list"); }
        // Ordinary resolution deliberately retires a held server session before
        // opening another. A borrowed leased stream therefore cannot promise a
        // non-destructive numeric handoff; refuse before entering that path.
        if runtime.active_lease.as_ref().is_some_and(HeldLease::is_playback) {
            return Err("Stop the current server-managed stream before selecting a configured channel by number");
        }
        let entry = base.catalog.entries.get(index).cloned().ok_or(CHANGED)?;
        let profile = base.profile.clone();
        let mut request = PlaybackRequest::ordinary(&entry.id, 0);
        request.advance_on_failure = false;
        if profile.kind == ProviderKind::Local && plx_data::ott::http_url(&profile.source).is_none() {
            request.local_selection = Some(runtime.local_profile_selection(&profile).ok_or(CHANGED)?);
        }
        // The common preparation policy validates parental/kiosk/source authority
        // before queue retirement. It does not borrow the temporary queue owner.
        parental_runtime::observe_session(runtime, app);
        runtime.play_prepared_request_with_quality_policy(app, (request.clone(), profile, Some(entry), None), false)?;
        let owner = runtime.transient.accepted.as_ref().ok_or(CHANGED)?.owner();
        runtime.pending_play = Some(inv.clone());
        runtime.transient.numeric.pending = Some(Box::new(Pending {
            invocation: inv.clone(), request, index, owner, generation: runtime.generation,
            play_generation: runtime.play_generation, playback_epoch: runtime.playback_epoch,
            last_input: app.last_input, ticket: runtime.play_ticket.ok_or(CHANGED)?,
        }));
        Ok(())
    })();
    result.err().map(rejected)
}
pub(super) fn staged_binding(runtime: &Runtime, request: &PlaybackRequest)
    -> Option<Result<(ProviderProfile, CatalogEntry), &'static str>> {
    let pending = runtime.transient.numeric.pending.as_ref()?;
    if request != &pending.request { return None; }
    Some((|| {
        if !pending_current(runtime, None, false) { return Err(CHANGED); }
        let base = runtime.transient.numeric.base.as_ref().ok_or(CHANGED)?;
        let entry = base.catalog.entries.get(pending.index).cloned().ok_or(CHANGED)?;
        if entry.id != request.id { return Err(CHANGED); }
        Ok((base.profile.clone(), entry))
    })())
}
pub(super) fn before_native(runtime: &Runtime, app: &App) -> Result<(), &'static str> {
    if has_pending(runtime) && !pending_current(runtime, Some(app), false) { return Err(CHANGED); }
    Ok(())
}
pub(super) fn has_pending(runtime: &Runtime) -> bool {
    runtime.transient.numeric.pending.as_ref().is_some_and(|pending|
        runtime.play_target.as_ref() == Some(&pending.request)
            && runtime.play_generation == pending.play_generation)
}
/// Recheck the exact native admission receipt before restoring catalogue ownership.
pub(super) fn finish(runtime: &mut Runtime, app: &App, result: Result<(), &'static str>, native: Option<&NativeStartReceipt>) -> Option<Value> {
    if !has_pending(runtime) { return None; }
    // An old Job receipt must not consume a newer pending numeric intent.
    if native.is_some_and(|receipt| runtime.transient.numeric.pending.as_ref()
        .is_none_or(|pending| receipt.generation != pending.play_generation)) { return None; }
    let current = pending_current(runtime, Some(app), true);
    let pending = runtime.transient.numeric.pending.take()?;
    let admitted = current && native.is_some_and(|native| native.generation == pending.play_generation
        && native.before == pending.ticket && plx_media::route::play_request_is_current(native.after));
    if let Err(error) = result { return Some(rejected(error)); }
    if !admitted { return Some(rejected(CHANGED)); }
    let base = runtime.transient.numeric.base.take()?;
    let entry = base.catalog.entries.get(pending.index)?;
    let response = ok(json!({"dispatched":true,"channel":{"id":entry.id,"name":entry.title,"number":pending.index + 1}}));
    // Native admission accepted the fresh source. Only now may the old producer
    // retire: a progressive Local worker can still have queued Batch/Finished
    // events. The existing cancellation boundary fences those by generation,
    // without changing the admitted native/play request or Ready root grants.
    runtime.cancel_catalog();
    transient::note_stop(runtime);
    runtime.transient.accepted = None;
    runtime.catalog = base.catalog;
    runtime.catalog_complete = true;
    runtime.catalog_loading = false;
    runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
    runtime.catalog_dirty = true;
    runtime.selected = Some(pending.request.id.clone());
    runtime.provider = base.profile.kind;
    runtime.page = "channels";
    runtime.continuity = continuity::Continuity::new(runtime.generation, None);
    runtime.continuity.initial_admitted(runtime.generation, &pending.request.id);
    // Publication preserves an already validated identical source grant, while
    // the existing context fence revokes a genuinely changed source/policy.
    runtime.parental_source_installed();
    if let Some(playing) = runtime.playing.as_ref().filter(|playing| playing.profile == base.profile) {
        let (profile, entry) = (playing.profile.clone(), playing.entry.clone());
        runtime.request_metadata(profile, entry);
    }
    runtime.changed();
    Some(response)
}
