//! Native UI and authenticated remote adapter for the app-owned companion.
//! Indexer secrets stay in the helper; plugin replies contain proposals, never owned leases.
use plx_screens::player::input::HudStart;
use super::*;
#[path = "ott_stremio_adapter.rs"]
mod stremio;
use crate::app::ott_companion::protocol::{
    CreateSource, Error, IndexerCapabilities, IndexerChildren, IndexerRequest, IndexerSource, Reply, Request,
    Session, SessionState,
};
use crate::app::ott_companion::{Configuration, Event, Supervisor};
use crate::app::ott_torrent_provider as torrent_provider;
use plx_base::task::time::{ClockSample, Deadline};
use std::sync::mpsc;

const SETTINGS_KEY: &str = "nativeCompanion";
const REVISION_KEY: &str = "nativeCompanionRevision";
const LANGUAGE_KEY: &str = "torrentPreferredLanguage";
const MAX_PENDING: usize = 8;
const UI_TIMEOUT: u64 = 40_000;

pub(super) fn init() -> Supervisor {
    // app_dir was resolved by the boot preamble. No filesystem probe runs here.
    let app = plx_base::paths::app_dir().to_path_buf();
    let root = private::root().with_file_name("ott-companion");
    Supervisor::new(
        app,
        root,
        Configuration {
            enabled: false,
            ..Configuration::default()
        },
    )
}

enum Target {
    Remote {
        invocation: Invocation,
        runtime: String,
        op: String,
    },
    Sources,
    Children { source_id: String },
    Categories { source_id: String, indexer_id: Option<String> },
    Search {
        append: bool,
        plan: torrent_provider::SearchPlan,
    },
    Prepare,
    Session,
    Configured,
    Removed,
}
impl Target {
    fn is_catalog(&self) -> bool {
        matches!(self, Self::Sources | Self::Children { .. } | Self::Categories { .. }
            | Self::Search { .. } | Self::Configured | Self::Removed)
    }
}
struct Pending {
    id: Option<u64>,
    request: Request,
    target: Target,
    deadline: u64,
    transport_deadline: Deadline,
    generation: u64,
    cancelled: Arc<AtomicBool>,
}
struct Inspected {
    generation: u64,
    result: Result<
        (
            plx_media::ff::ExternalSource,
            plx_media::ff::inspect::MediaInfo,
        ),
        &'static str,
    >,
}
enum Completed {
    Media(Inspected),
    Stremio(stremio::Fetched),
}
enum Work {
    Inspect(plx_media::ff::ExternalSource),
    Stremio(stremio::FetchRequest),
}
#[derive(Default)]
struct Draft {
    id: String,
    label: String,
    endpoint: String,
    // UI-only until explicit Load; never copied into Runtime::Saved or rows.
    api_key: String,
}

pub(super) struct Adapter {
    pending: Vec<Pending>,
    sources: Vec<IndexerSource>,
    sources_current: bool,
    categories: Option<IndexerCapabilities>,
    source: String,
    children: Option<IndexerChildren>,
    indexer: Option<String>,
    catalog_generation: u64,
    query: String,
    category: Vec<u64>,
    items: Vec<torrent_provider::Row>,
    next: Option<u64>,
    session: Option<Session>,
    title: String,
    retry_media: Option<(String, Request, bool)>,
    generation: u64,
    media_generation: u64,
    route_ticket: Option<plx_media::route::WorkerTicket>,
    media_deadline: u64,
    next_poll: u64,
    inspecting: bool,
    playing: bool,
    worker_done: Arc<AtomicBool>,
    inspected_tx: mpsc::Sender<Completed>,
    inspected_rx: mpsc::Receiver<Completed>,
    cancelled: Arc<AtomicBool>,
    applied: Option<Configuration>,
    applied_revision: Option<u64>,
    invalid_configuration: bool,
    snapshot: Value,
    draft: Draft,
    stremio: stremio::State,
    media_provider: &'static str,
    direct_source: Option<plx_media::ff::ExternalSource>,
}
impl Default for Adapter {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            pending: Vec::new(),
            sources: Vec::new(),
            sources_current: false,
            categories: None,
            source: String::new(),
            children: None,
            indexer: None,
            catalog_generation: 0,
            query: String::new(),
            category: Vec::new(),
            items: Vec::new(),
            next: None,
            session: None,
            title: String::new(),
            retry_media: None,
            generation: 0,
            media_generation: 0,
            route_ticket: None,
            media_deadline: 0,
            next_poll: 0,
            inspecting: false,
            playing: false,
            worker_done: Arc::new(AtomicBool::new(true)),
            inspected_tx: tx,
            inspected_rx: rx,
            cancelled: Arc::new(AtomicBool::new(false)),
            applied: None,
            applied_revision: None,
            invalid_configuration: false,
            snapshot: Value::Null,
            draft: Draft::default(),
            stremio: stremio::State::default(),
            media_provider: "torrent",
            direct_source: None,
        }
    }
}

impl Drop for Adapter {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        for pending in &self.pending {
            pending.cancelled.store(true, Ordering::Relaxed);
        }
    }
}

pub(super) fn capabilities(runtime: &Runtime) -> Value {
    let snapshot = runtime.companion.snapshot();
    let mut operations = vec!["status", "configure", "start", "stop"];
    if snapshot.ready && snapshot.plugins {
        operations.extend(["plugins", "resolve_plugin"]);
    }
    json!({"native_companion":{"version":1,"operations":operations},
        "companion":snapshot,"torrent_indexer":if snapshot.ready && snapshot.local_torznab {
        json!({"version":1})
    } else {Value::Null},
        "torrent_indexer_children": if snapshot.ready && snapshot.local_torznab_children {
            json!({"version":1,"operations":["indexers"],"scopedOperations":["caps","search"]})
        } else { Value::Null }})
}

/// Add public media identity without leaking the loopback playback capability.
pub(super) fn decorate_state(runtime: &Runtime, state: &mut Value) {
    let Some(adapter) = runtime.companion_adapter.as_ref() else {
        return;
    };
    if adapter.playing && adapter.media_generation == runtime.play_generation {
        state["provider"] = json!(adapter.media_provider);
        state["playback"]["provider"] = json!(adapter.media_provider);
        state["playback"]["kind"] = json!("vod");
        state["playback"]["title"] = json!(adapter.title);
        state["playback"]["media_id"] = Value::Null;
    }
}

fn wire_failure(op: &str, runtime: &str, reason: &str) -> Value {
    json!({"status":if reason=="unsupported" {"unsupported"} else {"rejected"},
        "data":{"version":1,"op":op,"runtime":runtime,"reason":reason,"error":"Torrent indexer request failed."}})
}
fn wire_success(op: &str, runtime: &str, mut data: Value) -> Value {
    data["version"] = json!(1);
    data["op"] = json!(op);
    data["runtime"] = json!(runtime);
    ok(data)
}
fn clean(value: &Value, max: usize, empty: bool) -> Option<String> {
    value
        .as_str()
        .filter(|s| s.len() <= max && (empty || !s.is_empty()) && !s.chars().any(char::is_control))
        .map(str::to_owned)
}
fn exact(value: &Value, fields: &[&str]) -> bool {
    value.as_object().is_some_and(|map| {
        map.len() == fields.len() && fields.iter().all(|key| map.contains_key(*key))
    })
}
fn source_id(value: &Value) -> Option<String> {
    clean(value, 64, false).filter(|s| {
        s.as_bytes()[0].is_ascii_lowercase()
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}
fn reference(value: &Value) -> Option<String> {
    clean(value, 32, false).filter(|s| {
        s.len() == 32
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn configuration(settings: &BTreeMap<String, Value>) -> Result<(Configuration, u64), &'static str> {
    let config = settings
        .get(SETTINGS_KEY)
        .cloned()
        .map(serde_json::from_value::<Configuration>)
        .unwrap_or_else(|| Ok(Configuration::default()))
        .map_err(|_| "Invalid companion configuration")?;
    config.validate().map_err(Error::code)?;
    let revision = settings
        .get(REVISION_KEY)
        .map(|value| value.as_u64().ok_or("Invalid companion revision"))
        .transpose()?
        .unwrap_or(0);
    Ok((config, revision))
}
fn save_configuration(saved: &mut Saved, config: &Configuration) -> Result<(), &'static str> {
    let revision = saved
        .settings
        .get(REVISION_KEY)
        .map(|value| value.as_u64().ok_or("Invalid companion revision"))
        .transpose()?
        .unwrap_or(0)
        .checked_add(1)
        .ok_or("Companion revision exhausted")?;
    saved.settings.insert(SETTINGS_KEY.into(), json!(config));
    saved.settings.insert(REVISION_KEY.into(), json!(revision));
    Ok(())
}
fn operation_deadline(sample: Option<ClockSample>, expiry: u64) -> Result<Deadline, &'static str> {
    let sample = sample.ok_or("companion_not_ready")?;
    let now = sample.unix_ms();
    if expiry <= now {
        return Err("expired");
    }
    Ok(sample.deadline_at_unix_ms(expiry.min(now.saturating_add(120_000))))
}

fn indexer_request(params: &Value, current: &str) -> Result<(String, Request, bool), &'static str> {
    if params["runtime"].as_str() != Some(current) {
        return Err("runtime_changed");
    }
    if params["version"].as_u64() != Some(1) {
        return Err("invalid_request");
    }
    let op = params["op"].as_str().ok_or("invalid_request")?;
    let fields: &[&str] = match op {
        "list" => &[],
        "caps" | "remove" | "indexers" => &["sourceId"],
        "configure" => &["sourceId", "label", "endpoint", "apiKey"],
        "prepare" => &["sourceId", "resultRef"],
        "search" => &["sourceId", "query", "categoryIds", "offset", "limit"],
        _ => return Err("invalid_request"),
    };
    let mut keys = vec!["version", "runtime", "op"];
    keys.extend(fields);
    let indexer_id = if matches!(op, "caps" | "search") && params.get("indexerId").is_some() {
        keys.push("indexerId");
        Some(clean(&params["indexerId"], 128, false).ok_or("invalid_request")?)
    } else { None };
    if !exact(params, &keys) {
        return Err("invalid_request");
    }
    let id = if op == "list" {
        String::new()
    } else {
        source_id(&params["sourceId"]).ok_or("invalid_request")?
    };
    let request = match op {
        "list" => IndexerRequest::List,
        "indexers" => IndexerRequest::Indexers { source_id: id },
        "caps" => match indexer_id {
            Some(indexer_id) => IndexerRequest::ScopedCapabilities { source_id: id, indexer_id },
            None => IndexerRequest::Capabilities { id },
        },
        "remove" => IndexerRequest::Remove { id },
        "configure" => IndexerRequest::Configure {
            id,
            label: clean(&params["label"], 128, false).ok_or("invalid_request")?,
            endpoint: clean(&params["endpoint"], 4096, false).ok_or("invalid_request")?,
            api_key: clean(&params["apiKey"], 2048, true).ok_or("invalid_request")?,
        },
        "prepare" => IndexerRequest::Prepare {
            source_id: id,
            result_ref: reference(&params["resultRef"]).ok_or("invalid_request")?,
        },
        "search" => {
            let category_ids = params["categoryIds"]
                .as_array()
                .filter(|a| a.len() <= 16)
                .ok_or("invalid_request")?
                .iter()
                .map(|v| {
                    v.as_u64()
                        .filter(|v| *v > 0 && *v <= 9_007_199_254_740_991)
                        .ok_or("invalid_request")
                })
                .collect::<Result<Vec<_>, _>>()?;
            let query = clean(&params["query"], 512, true).ok_or("invalid_request")?;
            let offset = params["offset"].as_u64().filter(|n| *n <= 1_000_000).ok_or("invalid_request")?;
            let limit = params["limit"].as_u64().filter(|n| (1..=100).contains(n)).ok_or("invalid_request")? as usize;
            match indexer_id {
                Some(indexer_id) => IndexerRequest::ScopedSearch { source_id: id, indexer_id, query, category_ids, offset, limit },
                None => IndexerRequest::Search { source_id: id, query, category_ids, offset, limit },
            }
        }
        _ => unreachable!(),
    };
    let request = Request::Indexer(request);
    request.validate().map_err(Error::code)?;
    Ok((
        op.into(),
        request,
        matches!(op, "configure" | "remove" | "prepare"),
    ))
}

fn plugin_request(params: &Value, current: &str) -> Result<Request, &'static str> {
    if params["runtime"].as_str() != Some(current) {
        return Err("runtime_changed");
    }
    if params["version"].as_u64() != Some(1)
        || params["op"].as_str() != Some("resolve_plugin")
        || !exact(params, &["version", "runtime", "op", "id", "input"])
    {
        return Err("invalid_request");
    }
    let request = Request::ResolvePlugin {
        id: clean(&params["id"], 64, false).ok_or("plugin_invalid_input")?,
        input: params["input"].clone(),
    };
    request.validate().map_err(Error::code)?;
    Ok(request)
}

fn reply_value(reply: Reply) -> Result<Value, &'static str> {
    Ok(match reply {
        Reply::Sources(sources) => {
            json!({"sources":sources.iter().map(|s|json!({"id":s.id,"label":s.label})).collect::<Vec<_>>()})
        }
        Reply::Indexers(children) => json!({"sourceId":children.source_id,"supported":children.supported,
            "indexers":children.indexers.iter().map(|i|json!({"id":i.id,"label":i.label,"language":i.language,"type":i.kind})).collect::<Vec<_>>()}),
        Reply::ScopedCategories { indexer_id, capabilities } => {
            let mut value = reply_value(Reply::Categories(capabilities))?;
            value["indexerId"] = json!(indexer_id);
            value
        }
        Reply::ScopedSearch { indexer_id, page } => {
            let mut value = reply_value(Reply::Search(page))?;
            value["indexerId"] = json!(indexer_id);
            value
        }
        Reply::Configured(source) => json!({"source":{"id":source.id,"label":source.label}}),
        Reply::Removed => json!({"removed":true}),
        Reply::Categories(caps) => {
            json!({"sourceId":caps.source_id,"searchAvailable":caps.search_available,"maxLimit":caps.max_limit,
            "categories":caps.categories.iter().map(|c|json!({"id":c.id,"name":c.name,"parentId":c.parent_id})).collect::<Vec<_>>()})
        }
        Reply::Search(page) => {
            json!({"sourceId":page.source_id,"nextOffset":page.next_offset,"total":page.total,
            "items":page.items.iter().map(|i|json!({"resultRef":i.result_ref,"title":i.title,"size":i.size,"seeders":i.seeders,
                "publishedAt":i.published_at,"categoryIds":i.category_ids,"languages":i.languages})).collect::<Vec<_>>()})
        }
        Reply::Prepared(source) => {
            json!({"sourceId":source.source_id,"sourceRef":source.source_ref})
        }
        Reply::Plugins(plugins) => json!({"plugins":plugins.iter().map(|plugin|json!({
            "id":plugin.id,"name":plugin.name,"apiVersion":plugin.api_version,
            "builtin":plugin.builtin,"network":plugin.network,"allowedHostCount":plugin.allowed_hosts.len(),
            "maxInputBytes":plugin.max_input_bytes,"maxOutputBytes":plugin.max_output_bytes,
            "maxStreams":plugin.max_streams,"memoryLimitBytes":plugin.memory_limit_bytes,
            "timeoutMs":plugin.timeout_ms,"maxNetworkCalls":plugin.max_network_calls,"maxFetchBytes":plugin.max_fetch_bytes,
        })).collect::<Vec<_>>()}),
        Reply::Resolved(streams) => json!({"streams":streams.iter().map(|stream|json!({
            "url":stream.url,"title":stream.title,
        })).collect::<Vec<_>>()}),
        _ => return Err("companion_receipt_invalid"),
    })
}

fn request_capability(ready: bool, children: bool, request: &Request) -> Result<(), &'static str> {
    if ready && matches!(request, Request::Indexer(IndexerRequest::Indexers { .. }
        | IndexerRequest::ScopedCapabilities { .. } | IndexerRequest::ScopedSearch { .. }))
        && !children {
        return Err("unsupported");
    }
    Ok(())
}

pub(super) fn command(runtime: &mut Runtime, app: &mut App, inv: &Invocation) -> Option<Value> {
    let mut adapter = runtime.companion_adapter.take().unwrap_or_default();
    let result = adapter.command(runtime, app, inv);
    runtime.companion_adapter = Some(adapter);
    result
}
impl Adapter {
    fn command(&mut self, runtime: &mut Runtime, app: &mut App, inv: &Invocation) -> Option<Value> {
        let current = runtime.remote.runtime().to_owned();
        let op = inv.params["op"]
            .as_str()
            .filter(|s| s.len() <= 32)
            .unwrap_or("");
        if inv.action == "native_companion" {
            if inv.params["runtime"].as_str() != Some(current.as_str()) {
                return Some(wire_failure(op, &current, "runtime_changed"));
            }
            if runtime.now_ms() >= inv.deadline_ms {
                return Some(wire_failure(op, &current, "expired"));
            }
            if inv.params["version"].as_u64() != Some(1) {
                return Some(wire_failure(op, &current, "invalid_request"));
            }
            if op == "status" && exact(&inv.params, &["version", "runtime", "op"]) {
                return Some(wire_success(
                    op,
                    &current,
                    json!({"companion":runtime.companion.snapshot()}),
                ));
            }
            if op == "plugins" {
                if !exact(&inv.params, &["version", "runtime", "op"]) {
                    return Some(wire_failure(op, &current, "invalid_request"));
                }
                let snapshot = runtime.companion.snapshot();
                if snapshot.ready && !snapshot.plugins {
                    return Some(wire_failure(op, &current, "unsupported"));
                }
                let target = Target::Remote {
                    invocation: inv.clone(),
                    runtime: current.clone(),
                    op: op.into(),
                };
                return match self.enqueue(runtime, Request::Plugins, target, inv.deadline_ms) {
                    Ok(()) => None,
                    Err(reason) => Some(wire_failure(op, &current, reason)),
                };
            }
            if op == "resolve_plugin" {
                let request = match plugin_request(&inv.params, &current) {
                    Ok(request) => request,
                    Err(reason) => return Some(wire_failure(op, &current, reason)),
                };
                if !playback_allowed(app) {
                    return Some(wire_failure(op, &current, "policy_restricted"));
                }
                let snapshot = runtime.companion.snapshot();
                if !snapshot.ready {
                    return Some(wire_failure(op, &current, "companion_not_ready"));
                }
                if !snapshot.plugins {
                    return Some(wire_failure(op, &current, "unsupported"));
                }
                let target = Target::Remote {
                    invocation: inv.clone(),
                    runtime: current.clone(),
                    op: op.into(),
                };
                return match self.enqueue(runtime, request, target, inv.deadline_ms) {
                    Ok(()) => None,
                    Err(reason) => Some(wire_failure(op, &current, reason)),
                };
            }
            let mut config = self.applied.clone().unwrap_or_default();
            match op {
                "configure"
                    if exact(
                        &inv.params,
                        &["version", "runtime", "op", "enabled", "cacheBytes"],
                    ) =>
                {
                    let Some(enabled) = inv.params["enabled"].as_bool() else {
                        return Some(wire_failure(op, &current, "invalid_request"));
                    };
                    let Some(cache_bytes) = inv.params["cacheBytes"].as_u64() else {
                        return Some(wire_failure(op, &current, "invalid_request"));
                    };
                    config = Configuration {
                        enabled,
                        cache_bytes,
                    };
                }
                "start" | "stop" if exact(&inv.params, &["version", "runtime", "op"]) => {
                    config.enabled = op == "start"
                }
                _ => return Some(wire_failure(op, &current, "invalid_request")),
            }
            if config.validate().is_err() {
                return Some(wire_failure(op, &current, "invalid_request"));
            }
            let mut saved = runtime.saved.clone();
            if save_configuration(&mut saved, &config).is_err() {
                return Some(wire_failure(op, &current, "storage_failed"));
            }
            // The committed value is applied by tick, never before the storage receipt.
            return match runtime.save_reply(
                saved,
                inv.clone(),
                false,
                wire_success(
                    op,
                    &current,
                    json!({"persisted":true,"configuration":config}),
                ),
            ) {
                Ok(()) => None,
                Err(_) => Some(wire_failure(op, &current, "storage_failed")),
            };
        }
        if inv.action != "torrent_indexer" {
            return Some(rejected("Unsupported companion request"));
        }
        let (op, request, _writable) = match indexer_request(&inv.params, &current) {
            Ok(value) => value,
            Err(reason) => return Some(wire_failure(op, &current, reason)),
        };
        if runtime.now_ms() >= inv.deadline_ms {
            return Some(wire_failure(&op, &current, "expired"));
        }
        if op == "prepare" && !playback_allowed(app) {
            return Some(wire_failure(&op, &current, "policy_restricted"));
        }
        let target = Target::Remote {
            invocation: inv.clone(),
            runtime: current.clone(),
            op: op.clone(),
        };
        match self.enqueue(runtime, request, target, inv.deadline_ms) {
            Ok(()) => None,
            Err(reason) => Some(wire_failure(&op, &current, reason)),
        }
    }

    fn enqueue(
        &mut self,
        runtime: &mut Runtime,
        request: Request,
        target: Target,
        deadline: u64,
    ) -> Result<(), &'static str> {
        if self.pending.len() >= MAX_PENDING {
            return Err("companion_busy");
        }
        let snapshot = runtime.companion.snapshot();
        request_capability(snapshot.ready, snapshot.local_torznab_children, &request)?;
        if runtime.persistence_uncertain {
            return Err("storage_failed");
        }
        if !runtime.loaded || self.applied.is_none() {
            return Err("companion_not_ready");
        }
        if !self.applied.as_ref().is_some_and(|c| c.enabled) {
            return Err("unsupported");
        }
        if snapshot.error.is_some() {
            return Err(snapshot.error.unwrap_or("companion_unavailable"));
        }
        let transport_deadline = operation_deadline(runtime.clock_sample, deadline)?;
        // Local or remote mutations can commit before their reply arrives.
        // Only a later successful List makes generated IDs safe to offer again.
        if matches!(&request, Request::Indexer(IndexerRequest::Configure { .. } | IndexerRequest::Remove { .. })) {
            self.sources_current = false;
            self.pending.retain(|pending| {
                let keep = !matches!(pending.target, Target::Sources);
                if !keep { pending.cancelled.store(true, Ordering::Relaxed); }
                keep
            });
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let id = if snapshot.ready {
            Some(
                runtime
                    .companion
                    .submit_scoped(request.clone(), transport_deadline, cancelled.clone())
                    .map_err(Error::code)?,
            )
        } else {
            None
        };
        self.pending.push(Pending {
            id,
            request,
            target,
            deadline,
            transport_deadline,
            generation: self.generation,
            cancelled,
        });
        Ok(())
    }
    fn ui_request(
        &mut self,
        runtime: &mut Runtime,
        request: Request,
        target: Target,
    ) -> Result<(), &'static str> {
        if matches!(target, Target::Sources) {
            self.sources_current = false;
            if self.pending.iter().any(|pending| matches!(&pending.request,
                Request::Indexer(IndexerRequest::Configure { .. } | IndexerRequest::Remove { .. }))) {
                return Err("Wait for the indexer change, then refresh sources");
            }
        }
        self.enqueue(
            runtime,
            request,
            target,
            runtime.now_ms().saturating_add(UI_TIMEOUT),
        )?;
        runtime.status = "Loading…".into();
        runtime.changed();
        Ok(())
    }
    // Requests not handed to the supervisor have no captured helper lifetime.
    // End them here when that lifetime fails or the committed configuration changes.
    fn retire_waiting(&mut self, runtime: &mut Runtime, app: &mut App, reason: &'static str) {
        let now = runtime.now_ms();
        while let Some(index) = self.pending.iter().position(|pending| pending.id.is_none()) {
            let pending = self.pending.remove(index);
            pending.cancelled.store(true, Ordering::Relaxed);
            let restricted = matches!(&pending.request,
                Request::Indexer(IndexerRequest::Prepare { .. }) | Request::Create { .. }
                    | Request::Select { .. } | Request::ResolvePlugin { .. }) && !playback_allowed(app);
            if let Target::Remote { invocation, runtime: token, op } = pending.target {
                let reason = if token != runtime.remote.runtime() { "runtime_changed" }
                    else if now >= invocation.deadline_ms { "expired" }
                    else if restricted { "policy_restricted" }
                    else { reason };
                runtime.remote.complete(invocation, wire_failure(&op, &token, reason));
            } else if pending.generation == self.generation
                && (!pending.target.is_catalog() || runtime.page.starts_with("torrent-")) {
                runtime.status = if now >= pending.deadline { "Request timed out" }
                    else if restricted { "policy_restricted" }
                    else { reason }.into();
                if matches!(pending.target, Target::Prepare | Target::Session) {
                    self.stop_owned_playback(runtime, app);
                    self.cancel_media(runtime);
                }
                runtime.changed();
            }
        }
    }

    fn cancel_media(&mut self, runtime: &mut Runtime) {
        self.draft = Draft::default();
        self.generation = self.generation.wrapping_add(1);
        self.cancelled.store(true, Ordering::Relaxed);
        self.cancelled = Arc::new(AtomicBool::new(false));
        self.session = None;
        self.direct_source = None;
        self.stremio.cancel();
        self.inspecting = false;
        self.playing = false;
        self.media_deadline = 0;
        runtime.companion.cancel_media();
        self.pending.retain(|p| {
            let keep = !matches!(p.target, Target::Prepare | Target::Session);
            if !keep {
                p.cancelled.store(true, Ordering::Relaxed);
            }
            keep
        });
    }
    fn navigate(&mut self, runtime: &mut Runtime, app: &mut App) {
        self.generation = self.generation.wrapping_add(1);
        self.pending.retain(|p| {
            let keep = matches!(p.target, Target::Remote { .. });
            if !keep {
                p.cancelled.store(true, Ordering::Relaxed);
            }
            keep
        });
        self.stop_owned_playback(runtime, app);
        self.cancel_media(runtime);
    }
    fn catalog_key(&self, key: &str, prefix: &str) -> Result<String, &'static str> {
        let (generation, value) = key.strip_prefix(prefix).and_then(|key| key.split_once(':'))
            .ok_or("Torrent selection is no longer available")?;
        if generation.parse::<u64>().ok() != Some(self.catalog_generation) {
            return Err("Torrent selection is no longer available");
        }
        Ok(value.into())
    }
    fn invalidate_catalog(&mut self) {
        self.catalog_generation = self.catalog_generation.wrapping_add(1);
        self.items.clear();
        self.next = None;
    }
    fn categories_request(&mut self, runtime: &mut Runtime) -> Result<(), &'static str> {
        let request = match &self.indexer {
            Some(indexer_id) => IndexerRequest::ScopedCapabilities { source_id: self.source.clone(), indexer_id: indexer_id.clone() },
            None => IndexerRequest::Capabilities { id: self.source.clone() },
        };
        runtime.page = "torrent-categories";
        self.ui_request(runtime, Request::Indexer(request), Target::Categories {
            source_id: self.source.clone(), indexer_id: self.indexer.clone(),
        })
    }
    fn children_request(&mut self, runtime: &mut Runtime) -> Result<(), &'static str> {
        runtime.page = "torrent-children";
        self.ui_request(runtime, Request::Indexer(IndexerRequest::Indexers { source_id: self.source.clone() }),
            Target::Children { source_id: self.source.clone() })
    }
    fn search(&mut self, runtime: &mut Runtime, append: bool) -> Result<(), &'static str> {
        let offset = if append { self.next.ok_or("No more results")? } else { 0 };
        if append && self.items.len() >= 512 {
            return Err("Search result limit reached; narrow the query");
        }
        if append && self.pending.iter().any(|p| matches!(p.target, Target::Search { .. })) {
            return Err("A search is already in progress");
        }
        let source = self.sources.iter().find(|s| s.id == self.source).ok_or("Unknown torrent source")?;
        let caps = self.categories.as_ref().ok_or("torznab_search_unavailable")?;
        let categories = match self.indexer.as_deref() {
            Some(indexer_id) => torrent_provider::categories_scoped(source, Some(caps), Some(indexer_id)),
            None => torrent_provider::categories(source, Some(caps)),
        }?;
        let category_id = self.category.first().map_or_else(|| "all".into(), u64::to_string);
        let category = categories.iter().find(|c| c.id == format!("catalog:torznab:{}:{}", self.source, category_id))
            .ok_or("Unknown torrent category")?;
        let generation = if append { self.catalog_generation } else { self.catalog_generation.wrapping_add(1) };
        let plan = torrent_provider::search_plan(category, caps, torrent_provider::SearchInput {
            query: &self.query, offset, limit: if append { (512 - self.items.len()).min(50) } else { 50 },
            preferred_language: runtime.saved.settings.get(LANGUAGE_KEY).and_then(Value::as_str).unwrap_or(""),
            generation,
        })?;
        if !append {
            self.generation = self.generation.wrapping_add(1);
            self.pending.retain(|p| {
                let keep = matches!(p.target, Target::Remote { .. });
                if !keep { p.cancelled.store(true, Ordering::Relaxed); }
                keep
            });
            self.invalidate_catalog();
        }
        runtime.page = "torrent-results";
        self.ui_request(runtime, plan.request(), Target::Search { append, plan })
    }
    fn apply_search(&mut self, runtime: &mut Runtime, append: bool, plan: torrent_provider::SearchPlan,
        page: crate::app::ott_companion::protocol::SearchPage, indexer_id: Option<&str>) -> Result<(), &'static str> {
        let page = match indexer_id {
            Some(indexer_id) => plan.project_scoped(page, Some(indexer_id), self.catalog_generation),
            None => plan.project(page, self.catalog_generation),
        }?;
        if !append { self.items.clear(); }
        let before = self.items.len();
        for item in page.rows {
            if !self.items.iter().any(|old| old.id == item.id) { self.items.push(item); }
        }
        // A successful page must make progress; repeated pages cannot keep a
        // never-ending Load more button alive or renew the catalogue budget.
        self.next = if self.items.len() > before && self.items.len() < 512 { page.next_offset } else { None };
        runtime.status = match page.total {
            Some(total) => format!("{} / {total} results", self.items.len()),
            None => format!("{} results", self.items.len()),
        };
        Ok(())
    }
    fn retry_media(&mut self, runtime: &mut Runtime, app: &mut App) -> Result<(), &'static str> {
        let (title, request, prepare) = self.retry_media.clone().ok_or("Choose a source first")?;
        self.begin_media(runtime, app, title, request, if prepare { Target::Prepare } else { Target::Session })
    }
    fn begin_media(
        &mut self,
        runtime: &mut Runtime,
        app: &mut App,
        title: String,
        request: Request,
        target: Target,
    ) -> Result<(), &'static str> {
        if !playback_allowed(app) {
            return Err("policy_restricted");
        }
        self.stop_owned_playback(runtime, app);
        self.cancel_media(runtime);
        runtime.stop_catalog(app);
        runtime.selected = None;
        self.media_generation = runtime.play_generation;
        self.route_ticket = Some(plx_media::route::worker_ticket());
        self.media_deadline = runtime.now_ms().saturating_add(120_000);
        self.retry_media = Some((title.clone(), request.clone(), matches!(target, Target::Prepare)));
        self.title = title;
        self.media_provider = "torrent";
        runtime.page = "torrent-files";
        self.ui_request(runtime, request, target)
    }
}

pub(super) fn tick(runtime: &mut Runtime, app: &mut App) {
    let mut adapter = runtime.companion_adapter.take().unwrap_or_default();
    adapter.tick(runtime, app);
    runtime.companion_adapter = Some(adapter);
}

pub(super) fn suspend(runtime: &mut Runtime, app: &mut App) {
    let Some(mut adapter) = runtime.companion_adapter.take() else {
        return;
    };
    adapter.stop_owned_playback(runtime, app);
    adapter.cancel_media(runtime);
    runtime.companion_adapter = Some(adapter);
}

/// Retire companion work at explicit user Exit, before the canonical player teardown.
/// Unlike suspension, this does not perform a second navigation or create resume intent.
pub(super) fn exit(runtime: &mut Runtime) {
    let Some(mut adapter) = runtime.companion_adapter.take() else {
        return;
    };
    adapter.cancel_media(runtime);
    adapter.route_ticket = None;
    adapter.title.clear();
    adapter.next_poll = 0;
    runtime.companion_adapter = Some(adapter);
}

impl Adapter {
    fn tick(&mut self, runtime: &mut Runtime, app: &mut App) {
        let now = runtime.now_ms();
        if runtime.page != "torrent-configure" {
            self.draft = Draft::default();
        }
        if !runtime.companion.snapshot().ready { self.sources_current = false; }
        // Retire abandoned catalog requests without changing the media epoch:
        // remote plugin resolution and active playback have independent owners.
        if !runtime.page.starts_with("torrent-") {
            self.pending.retain(|pending| {
                let keep = !pending.target.is_catalog();
                if !keep { pending.cancelled.store(true, Ordering::Relaxed); }
                keep
            });
        }
        // Browse navigation can leave a pending start without invoking Back.
        // Stremio torrent choices use the shared torrent-files page, while
        // direct HTTP inspection remains on stremio-streams.
        let media_page = if self.direct_source.is_some() { "stremio-streams" } else { "torrent-files" };
        let abandoned_media = self.media_deadline > 0 && runtime.page != media_page;
        // An active player's periodic Session poll is not a pending UI start.
        if !self.playing && abandoned_media {
            self.navigate(runtime, app);
        }
        stremio::tick(self, runtime, app);
        if runtime.loaded && !runtime.saving && !runtime.persistence_uncertain {
            match configuration(&runtime.saved.settings) {
                Ok((config, revision))
                    if self.invalid_configuration
                        || self.applied.as_ref() != Some(&config)
                        || self.applied_revision != Some(revision) =>
                {
                    // A committed enable/retry waits for foreground; saving settings never wakes the player.
                    if !config.enabled || playback_allowed(app) {
                        let restart = config.enabled
                            && self.applied.as_ref() == Some(&config)
                            && self.applied_revision != Some(revision);
                        self.retire_waiting(runtime, app, "companion_cancelled");
                        self.stop_owned_playback(runtime, app);
                        self.cancel_media(runtime);
                        self.sources_current = false;
                        let result = runtime.companion.configure(config.clone()).and_then(|()| {
                            if restart {
                                runtime.companion.start()
                            } else {
                                Ok(())
                            }
                        });
                        // One attempt per durable revision, even if the child cannot start.
                        self.applied = Some(config);
                        self.applied_revision = Some(revision);
                        self.invalid_configuration = false;
                        if let Err(error) = result {
                            runtime.status = error.code().into();
                            runtime.changed();
                        }
                    }
                }
                Err(_) if !self.invalid_configuration => {
                    self.invalid_configuration = true;
                    self.retire_waiting(runtime, app, "companion_cancelled");
                    self.stop_owned_playback(runtime, app);
                    self.cancel_media(runtime);
                    runtime.companion.stop();
                    runtime.status = "Invalid saved companion configuration".into();
                    runtime.changed();
                }
                _ => {}
            }
        }
        if self.media_deadline > 0
            && (self.media_generation != runtime.play_generation
                || !playback_allowed(app)
                || (!self.playing
                    && (plx_media::route::play_pending()
                        || self.route_ticket.as_ref() != Some(&plx_media::route::worker_ticket())))
                || (self.session.is_some() && !runtime.companion.snapshot().ready)
                || (self.playing
                    && plx_media::route::cur_sid(&app.player.session)
                        != plx_plex::plex::ServerId::UNSET)
                || (!self.playing && now >= self.media_deadline)
                || (self.playing && !matches!(app.route(), AppArg::Player)))
        {
            self.stop_owned_playback(runtime, app);
            self.cancel_media(runtime);
            if runtime.page.starts_with("torrent") || runtime.page.starts_with("stremio-") {
                runtime.status = "Playback request cancelled".into();
                runtime.changed();
            }
        }
        let events = runtime.companion.poll();
        for event in events {
            let Event::Reply { id, result } = event else {
                continue;
            };
            let Some(index) = self.pending.iter().position(|p| p.id == Some(id)) else {
                continue;
            };
            let pending = self.pending.remove(index);
            pending.cancelled.store(true, Ordering::Relaxed);
            if let Target::Remote {
                invocation,
                runtime: token,
                op,
            } = pending.target
            {
                let response = if token != runtime.remote.runtime() {
                    wire_failure(&op, &token, "runtime_changed")
                } else if now >= invocation.deadline_ms {
                    wire_failure(&op, &token, "expired")
                } else if matches!(op.as_str(), "prepare" | "resolve_plugin")
                    && !playback_allowed(app)
                {
                    wire_failure(&op, &token, "policy_restricted")
                } else {
                    match result.map_err(Error::code).and_then(reply_value) {
                        Ok(data) => wire_success(&op, &token, data),
                        Err(reason) => wire_failure(&op, &token, reason),
                    }
                };
                runtime.remote.complete(invocation, response);
            } else {
                self.complete_ui_reply(runtime, app, pending, result, now);
            }
        }
        let mut index = 0;
        while index < self.pending.len() {
            let stale = match &self.pending[index].target {
                Target::Remote { runtime: token, .. } => token != runtime.remote.runtime(),
                _ => self.pending[index].generation != self.generation,
            };
            let expired = now >= self.pending[index].deadline;
            let restricted = matches!(
                self.pending[index].request,
                Request::Indexer(IndexerRequest::Prepare { .. })
                    | Request::Create { .. }
                    | Request::Select { .. }
                    | Request::ResolvePlugin { .. }
            ) && !playback_allowed(app);
            if stale || expired || restricted {
                let pending = self.pending.remove(index);
                pending.cancelled.store(true, Ordering::Relaxed);
                if let Target::Remote {
                    invocation,
                    runtime: token,
                    op,
                } = pending.target
                {
                    runtime.remote.complete(
                        invocation,
                        wire_failure(
                            &op,
                            &token,
                            if stale {
                                "runtime_changed"
                            } else if restricted {
                                "policy_restricted"
                            } else {
                                "expired"
                            },
                        ),
                    );
                } else if expired || restricted {
                    runtime.status = "Request timed out".into();
                    if matches!(
                        pending.request,
                        Request::Indexer(IndexerRequest::Prepare { .. })
                            | Request::Create { .. } | Request::Select { .. } | Request::Session { .. }
                    ) {
                        self.stop_owned_playback(runtime, app);
                        self.cancel_media(runtime);
                    }
                    runtime.changed();
                }
                continue;
            }
            if self.pending[index].id.is_none() && runtime.companion.snapshot().ready {
                match request_capability(true, runtime.companion.snapshot().local_torznab_children, &self.pending[index].request)
                    .and_then(|()| runtime.companion.submit_scoped(
                        self.pending[index].request.clone(),
                        self.pending[index].transport_deadline,
                        self.pending[index].cancelled.clone(),
                    ).map_err(Error::code))
                {
                    Ok(id) => self.pending[index].id = Some(id),
                    Err(error) => {
                        let pending = self.pending.remove(index);
                        pending.cancelled.store(true, Ordering::Relaxed);
                        if let Target::Remote {
                            invocation,
                            runtime: token,
                            op,
                        } = pending.target
                        {
                            runtime
                                .remote
                                .complete(invocation, wire_failure(&op, &token, error));
                        } else {
                            runtime.status = error.into();
                            runtime.changed();
                        }
                        continue;
                    }
                }
            }
            index += 1;
        }
        let snapshot = runtime.companion.snapshot();
        if !snapshot.ready {
            if let Some(reason) = snapshot.error {
                self.retire_waiting(runtime, app, reason);
            }
        }
        if let Some(session) = self.session.as_ref() {
            if !self.inspecting
                && session.state != SessionState::Select
                && now >= self.next_poll
                && !self
                    .pending
                    .iter()
                    .any(|p| matches!(p.target, Target::Session))
            {
                let id = session.id.clone();
                self.next_poll = now.saturating_add(if self.playing { 30_000 } else { 1000 });
                if let Err(error) =
                    self.ui_request(runtime, Request::Session { id }, Target::Session)
                {
                    runtime.status = error.into();
                }
            }
        }
        while let Ok(completed) = self.inspected_rx.try_recv() {
            let inspected = match completed {
                Completed::Stremio(fetched) => {
                    stremio::complete(self, runtime, fetched);
                    continue;
                }
                Completed::Media(inspected) => inspected,
            };
            if inspected.generation != self.generation
                || self.cancelled.load(Ordering::Relaxed)
                || self.media_generation != runtime.play_generation
                || !playback_allowed(app)
                || plx_media::route::play_pending()
                || self.route_ticket.as_ref() != Some(&plx_media::route::worker_ticket())
            {
                continue;
            }
            self.inspecting = false;
            match inspected
                .result
                .and_then(|(source, info)| self.start_playback(runtime, app, source, info))
            {
                Ok(()) => {
                    self.playing = true;
                    self.next_poll = now.saturating_add(30_000);
                    runtime.status.clear();
                }
                Err(error) => {
                    self.stop_owned_playback(runtime, app);
                    self.cancel_media(runtime);
                    runtime.status = error.into();
                }
            }
            runtime.changed();
        }
        let snapshot = json!(runtime.companion.snapshot());
        if snapshot != self.snapshot {
            self.snapshot = snapshot;
            runtime.changed();
        }
    }

    fn complete_ui_reply(
        &mut self, runtime: &mut Runtime, app: &mut App, pending: Pending,
        result: Result<Reply, Error>, now: u64,
    ) {
        if pending.generation != self.generation
            || (pending.target.is_catalog() && !runtime.page.starts_with("torrent-")) { return; }
        let media_reply = matches!(pending.target, Target::Prepare | Target::Session);
        // The pending entry has already been removed from the mailbox. Publish
        // its deadline failure here; the timeout sweep cannot see it any more.
        let result = if now >= pending.deadline { Err("Request timed out") }
            else if let Err(reason) = self.selected_reply(&pending.request, &result) { Err(reason) }
            else { self.ui_reply(runtime, app, pending.target, result) };
        if let Err(reason) = result {
            runtime.status = reason.into();
            if media_reply {
                self.stop_owned_playback(runtime, app);
                self.cancel_media(runtime);
            }
            runtime.changed();
        }
    }

    fn selected_reply(&self, request: &Request, result: &Result<Reply, Error>)
        -> Result<(), &'static str>
    {
        let (Request::Select { id, file_index }, Ok(Reply::Session(after))) = (request, result)
            else { return Ok(()); };
        // A backend failure retains its own diagnostic. A successful Select
        // must acknowledge the exact file offered by this live session before
        // Ready can reach Inspect; the request's index is not a list position.
        if after.state == SessionState::Error { return Ok(()); }
        let before = self.session.as_ref().filter(|session| session.id == *id)
            .ok_or("Torrent file selection changed")?;
        let expected = before.files.iter().find(|file| file.index == *file_index)
            .ok_or("Torrent file selection changed")?;
        if after.id != *id || after.selected != Some(*file_index)
            || after.files.iter().find(|file| file.index == *file_index) != Some(expected)
        { return Err("Torrent file selection changed"); }
        Ok(())
    }

    fn stop_owned_playback(&mut self, runtime: &Runtime, app: &mut App) {
        if self.playing
            && self.media_generation == runtime.play_generation
            && matches!(app.route(), AppArg::Player)
            && plx_media::route::cur_sid(&app.player.session) == plx_plex::plex::ServerId::UNSET
        {
            playback::replace_player(
                &mut app.player,
                &mut app.adapters.player,
                &mut app.repause_at,
                &mut app.refresh_hubs,
                &mut app.pages,
            );
        }
    }

    fn ui_reply(
        &mut self,
        runtime: &mut Runtime,
        app: &App,
        target: Target,
        result: Result<Reply, Error>,
    ) -> Result<(), &'static str> {
        let reply = result.map_err(Error::code)?;
        match (target, reply) {
            (Target::Sources, Reply::Sources(sources)) => {
                self.sources = sources;
                self.sources_current = !self.pending.iter().any(|pending| matches!(&pending.request,
                    Request::Indexer(IndexerRequest::Configure { .. } | IndexerRequest::Remove { .. })));
                runtime.page = "torrent-sources";
                runtime.status.clear();
            }
            (Target::Children { source_id }, Reply::Indexers(children)) => {
                if source_id != self.source || children.source_id != source_id { return Err("torznab_source_changed"); }
                self.children = Some(children);
                runtime.page = "torrent-children";
                runtime.status.clear();
            }
            (Target::Categories { source_id, indexer_id: None }, Reply::Categories(caps)) => {
                if source_id != self.source || caps.source_id != source_id || self.indexer.is_some() { return Err("torznab_source_changed"); }
                self.categories = Some(caps);
                runtime.page = "torrent-categories";
                runtime.status.clear();
            }
            (Target::Categories { source_id, indexer_id: Some(expected) }, Reply::ScopedCategories { indexer_id, capabilities }) => {
                if source_id != self.source || capabilities.source_id != source_id || expected != indexer_id
                    || self.indexer.as_deref() != Some(indexer_id.as_str()) { return Err("torznab_indexer_changed"); }
                self.categories = Some(capabilities);
                runtime.page = "torrent-categories";
                runtime.status.clear();
            }
            (Target::Search { append, plan }, Reply::Search(page)) => self.apply_search(runtime, append, plan, page, None)?,
            (Target::Search { append, plan }, Reply::ScopedSearch { indexer_id, page }) => {
                self.apply_search(runtime, append, plan, page, Some(&indexer_id))?;
            }
            (Target::Prepare, Reply::Prepared(prepared)) => {
                if !playback_allowed(app) {
                    return Err("policy_restricted");
                }
                self.ui_request(
                    runtime,
                    Request::Create {
                        source: CreateSource::LocalReference {
                            source_id: prepared.source_id,
                            source_ref: prepared.source_ref,
                        },
                        file_index: None,
                    },
                    Target::Session,
                )?;
            }
            (Target::Session, Reply::Session(session)) => {
                runtime.status = match (session.downloaded, session.total) {
                    (Some(done), Some(total)) => {
                        format!("{} / {} MiB", done / 1_048_576, total / 1_048_576)
                    }
                    (Some(done), _) => format!("{} MiB", done / 1_048_576),
                    _ => "Preparing video…".into(),
                };
                if session.state == SessionState::Error {
                    return Err(session.error.map_or("companion_unavailable", Error::code));
                }
                if session.state == SessionState::Ready && !self.inspecting && !self.playing {
                    if !playback_allowed(app) {
                        return Err("policy_restricted");
                    }
                    let url = runtime
                        .companion
                        .stream_url(&session)
                        .map_err(Error::code)?;
                    let source = plx_media::ff::ExternalSource::OwnedHttp(url);
                    self.launch(Work::Inspect(source))?;
                    self.inspecting = true;
                }
                if session.state == SessionState::Select {
                    runtime.status = "Choose a file".into();
                }
                self.session = Some(session);
            }
            (Target::Configured, Reply::Configured(source)) => {
                self.draft = Draft::default();
                // Keep the acknowledged ID reserved even if the following list
                // refresh fails; a new form must not suggest it as unused.
                if let Some(existing) = self.sources.iter_mut().find(|known| known.id == source.id) {
                    *existing = source;
                } else {
                    self.sources.push(source);
                }
                runtime.page = "torrent-sources";
                self.ui_request(runtime, Request::Indexer(IndexerRequest::List), Target::Sources)?;
            }
            (Target::Removed, Reply::Removed) => {
                self.draft = Draft::default();
                self.ui_request(
                    runtime,
                    Request::Indexer(IndexerRequest::List),
                    Target::Sources,
                )?;
            }
            _ => return Err("companion_receipt_invalid"),
        }
        runtime.changed();
        Ok(())
    }

    fn admit_playback(
        &mut self,
        runtime: &mut Runtime,
        app: &mut App,
        source: plx_media::ff::ExternalSource,
        info: plx_media::ff::inspect::MediaInfo,
    ) -> Result<(), &'static str> {
        // Owned URLs remain scoped to the current helper session. Add-on HTTP
        // proposals never acquire ownership merely by looking like a loopback URL.
        if let Some(expected) = self.direct_source.as_ref() {
            if expected != &source || !stremio::admitted_source(&source) {
                return Err("Media source changed during inspection");
            }
            if info.resolved_source.as_ref().is_some_and(|resolved| !stremio::admitted_source(resolved)) {
                return Err("Add-on returned an unsupported media destination");
            }
        } else {
            let session = self.session.as_ref().ok_or("companion_cancelled")?;
            let expected = runtime.companion.stream_url(session).map_err(Error::code)?;
            if !matches!(&source, plx_media::ff::ExternalSource::OwnedHttp(url) if *url == expected)
                || info.resolved_source.as_ref().is_some_and(|resolved|
                    !matches!(resolved, plx_media::ff::ExternalSource::OwnedHttp(url) if *url == expected)) {
                return Err("Companion media cannot resolve an external source");
            }
        }
        if plx_media::route::play_pending()
            || self.route_ticket.as_ref() != Some(&plx_media::route::worker_ticket()) {
            return Err("Playback request superseded");
        }
        // Validate the durable preference before retiring the current source.
        let subtitles = subtitle_settings::Stored::read(&runtime.saved)?;
        if plx_media::route::has_url(&app.player.session) || matches!(app.route(), AppArg::Player) {
            playback::replace_player_in_place(
                &mut app.player,
                &mut app.adapters.player,
                &mut app.repause_at,
                &mut app.pages,
            );
        } else {
            plx_media::route::cancel_play(&mut app.player.session);
        }
        runtime.playing = None;
        runtime.active_lease.take();
        app.bridge
            .metadata_run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(
                None,
            ));
        app.bridge
            .metadata_run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(
                Some(plx_data::metadata::NowPlaying {
                    ott: None,
                    is_episode: false,
                    is_real_episode: false,
                    title: self.title.clone(),
                    ep_title: String::new(),
                    season: 0,
                    index: 0,
                    summary: String::new(),
                    year: 0,
                    dur_ms: info.duration_ns.max(0) / 1_000_000,
                    rating: String::new(),
                    thumb: String::new(),
                    detail_rk: String::new(),
                }),
            ));
        plx_media::route::request_external_source(
            &mut app.player.session,
            &info,
            source,
            &self.title,
            if self.media_provider == "stremio" { "Stremio" } else { "Torrent" },
            playback::start_caps(),
        )
        .map_err(|_| "The native decoder cannot open this torrent file")?;
        subtitles.apply(app, true);
        Ok(())
    }

    fn start_playback(
        &mut self,
        runtime: &mut Runtime,
        app: &mut App,
        source: plx_media::ff::ExternalSource,
        info: plx_media::ff::inspect::MediaInfo,
    ) -> Result<(), &'static str> {
        self.admit_playback(runtime, app, source, info)?;
        if !playback::start_playback(
            &mut app.player.session,
            &mut app.adapters.player,
            0,
            playback::Origin::Here,
            HudStart::Ordinary,
            None,
            &mut app.pages,
            &mut app.bridge,
        ) {
            return Err("Native torrent playback could not start");
        }
        Ok(())
    }
}

fn row(id: String, title: String, detail: String, action: OttAction) -> OttRow {
    OttRow { current_programme: None, content_title: None, guide: None,
        guide_available: false, records_available: false, media: None,
        id,
        title,
        detail,
        action,
        description: String::new(),
        poster: None,
        enabled: true,
        favorite: false,
    }
}
fn button(id: &str, title: &str) -> OttRow {
    row(
        id.into(),
        title.into(),
        String::new(),
        OttAction::EditSetting(id.into()),
    )
}
pub(super) fn rows(runtime: &Runtime) -> Vec<OttRow> {
    let mut result = match runtime.page {
        "providers" => {
            let snapshot = runtime.companion.snapshot();
            let mut entry = button("torrent.open", "Torrent");
            entry.detail = if snapshot.ready && snapshot.torrent && snapshot.local_torznab {
                "Browse local indexers or open a torrent link"
            } else if !snapshot.ready {
                "Set up the local torrent service"
            } else {
                "Local torrent indexers are unavailable"
            }.into();
            vec![entry]
        }
        "settings" => vec![button("torrent.settings", "Local torrent service")],
        _ => Vec::new(),
    };
    result.extend(stremio::rows(runtime));
    result
}
pub(super) fn view(runtime: &Runtime) -> Option<OttView> {
    if let Some(view) = stremio::view(runtime) { return Some(view); }
    let adapter = runtime.companion_adapter.as_ref()?;
    let generation = adapter.catalog_generation;
    let (title, mut rows) = match runtime.page {
        "torrent-sources" => ("Torrent sources", {
            let mut rows: Vec<_> = adapter
                .sources
                .iter()
                .map(|s| button(&format!("torrent.source:{generation}:{}", s.id), &s.label))
                .collect();
            let mut add = button("torrent.add", "Add local indexer");
            add.enabled = adapter.sources_current;
            if !add.enabled { add.detail = "Refresh sources before adding an indexer".into(); }
            rows.push(add);
            rows.push(button("torrent.link", "Open torrent link"));
            rows
        }),
        "torrent-configure" => ("Add local indexer", {
            let key = |field: &str| format!("torrent.configure:{}:{field}", adapter.generation);
            let field = |name: &str, title: &str, detail: &str| row(
                key(name), title.into(), detail.into(), OttAction::EditSetting(key(name)));
            let mut rows = vec![
                field("endpoint", "Endpoint", if adapter.draft.endpoint.is_empty() { "Not set" } else { "Set" }),
                field("key", "API key (optional)", if adapter.draft.api_key.is_empty() { "Not set" } else { "Set" }),
                field("label", "Name (optional)", &adapter.draft.label),
                field("id", "Source ID", &adapter.draft.id),
                button(&key("load"), "Load"),
            ];
            if adapter.configure_pending() {
                for row in &mut rows { row.enabled = false; }
            } else {
                let snapshot = runtime.companion.snapshot();
                if let Some(load) = rows.last_mut() {
                    if !adapter.sources_current {
                        load.enabled = false;
                        load.detail = "Go back and refresh sources before another Load".into();
                    } else if !snapshot.ready || !snapshot.torrent || !snapshot.local_torznab {
                        load.enabled = false;
                        load.detail = "The local torrent service is unavailable".into();
                    }
                }
            }
            rows
        }),
        "torrent-children" => ("Torrent indexers", {
            let mut rows = Vec::new();
            if let Some(children) = &adapter.children {
                rows.push(button(&format!("torrent.indexer:{generation}:all"), "All indexers"));
                rows.extend(children.indexers.iter().map(|child| {
                    let mut row = button(&format!("torrent.indexer:{generation}:{}", child.id), &child.label);
                    row.detail = [child.language.as_deref(), child.kind.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                    row
                }));
            }
            rows
        }),
        "torrent-categories" => ("Torrent categories", {
            let mut rows = vec![
                button(&format!("torrent.category:{generation}:all"), "All"),
                button(&format!("torrent.query:{generation}:edit"), "Search"),
                button("torrent.language", "Preferred result language"),
                button(&format!("torrent.remove:{generation}:source"), "Remove this source"),
            ];
            if let Some(caps) = &adapter.categories {
                rows.extend(
                    caps.categories
                        .iter()
                        .map(|c| button(&format!("torrent.category:{generation}:{}", c.id), &c.name)),
                );
            }
            if !adapter.categories.as_ref().is_some_and(|caps| caps.search_available) {
                for row in &mut rows {
                    if row.id.starts_with("torrent.category:") || row.id.starts_with("torrent.query:") {
                        row.enabled = false;
                        row.detail = "This indexer does not offer search".into();
                    }
                }
            }
            rows
        }),
        "torrent-results" => ("Torrent results", {
            let mut rows: Vec<_> = adapter
                .items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let facts: Vec<_> = [item.facts.size_bytes.map(|n| format!("{} MiB", n / 1_048_576)),
                        item.facts.seeders.map(|n| format!("{n} seeders")),
                        (!item.facts.languages.is_empty()).then(|| item.facts.languages.join(", ")),
                        item.facts.published_at.clone()]
                        .into_iter().flatten().filter(|fact| !fact.trim().is_empty()).collect();
                    let mut result = row(
                        item.id.clone(),
                        format!("{}  {}", index + 1, item.title),
                        facts.join(" · "),
                        OttAction::Play(format!("torrent:{}", item.id)),
                    );
                    result.content_title = Some(item.title.clone());
                    result.description = facts.join("\n");
                    result
                })
                .collect();
            if adapter.next.is_some() {
                rows.push(button(&format!("torrent.more:{generation}:next"), "Load more"));
            }
            rows.push(button(&format!("torrent.query:{generation}:edit"), "Search"));
            rows
        }),
        "torrent-files" => (
            "Torrent files",
            adapter.session.as_ref().map_or_else(Vec::new, |s| {
                s.files
                    .iter()
                    .enumerate()
                    .map(|(index, f)| {
                        let key = format!("torrent.file:{}:{}:{}", adapter.generation, s.id, f.index);
                        let mut result = row(
                            key.clone(),
                            format!("{}  {}", index + 1, f.name),
                            format!("{} MiB", f.length / 1_048_576),
                            OttAction::Play(key),
                        );
                        result.content_title = Some(f.name.clone());
                        result.description = result.detail.clone();
                        result.enabled = s.state == SessionState::Select && !adapter.pending.iter().any(|p| matches!(p.request, Request::Select { .. }));
                        result
                    })
                    .collect()
            }),
        ),
        "torrent-settings" => (
            "Local torrent service",
            vec![
                button(
                    "torrent.toggle",
                    if adapter.applied.as_ref().is_some_and(|c| c.enabled) {
                        "Disable"
                    } else {
                        "Enable"
                    },
                ),
                button("torrent.cache", "Cache limit (MiB)"),
                button("torrent.start", "Start / retry service"),
                button("torrent.language", "Preferred result language"),
            ],
        ),
        _ => return None,
    };
    if !adapter.pending.iter().any(|p| !matches!(p.target, Target::Remote { .. })) && !matches!(runtime.page, "torrent-settings" | "torrent-configure") {
        rows.push(button("torrent.retry", "Refresh / retry"));
    }
    rows.push(button("torrent.back", "Back / cancel"));
    Some(OttView { pending_play: None, pending_step: None, pending_step_sdl: false, shelf_context: None, shelves: Vec::new(), status_working: false, classic_preferences: Default::default(), volume_available: false, category_label: None,
        preview_context: None,
        order_context: None, random_context: None,
        catalog_progress: None,
        search: None,
        page_id: runtime.page.into(),
        revision: runtime.revision,
        title: title.into(),
        description: [Some(adapter.source.as_str()).filter(|s| !s.is_empty()),
            adapter.indexer.as_deref(), Some(adapter.query.as_str()).filter(|s| !s.is_empty())]
            .into_iter().flatten().collect::<Vec<_>>().join(" · "),
        status: runtime.status.clone(),
        rows,
        selected_row: runtime.selected.clone(),
    })
}

pub(super) fn action(
    runtime: &mut Runtime,
    app: &mut App,
    action: &OttAction,
) -> Option<Result<(), &'static str>> {
    // These session/result identities have no durable Favorite resolver. Do
    // not let the generic catalogue path reinterpret them as stored channels.
    if let OttAction::ToggleFavorite(key) = action {
        if key.starts_with("torrent.file:") || key.starts_with("torznab:") || key.starts_with("torrent:") {
            return Some(Err("Favorites are unavailable for Torrent sources"));
        }
    }
    if let Some(result) = stremio::action(runtime, app, action) { return Some(result); }
    let claimed = match action {
        OttAction::EditSetting(key) => key.starts_with("torrent."),
        OttAction::SubmitSetting { session, key, .. } => {
            *session == 0 && key.starts_with("torrent.")
        }
        OttAction::Play(key) => key.starts_with("torrent:") || key.starts_with("torrent.file:"),
        OttAction::Back | OttAction::ShowCategories | OttAction::Refresh => {
            runtime.page.starts_with("torrent-")
        }
        _ => false,
    };
    if !claimed {
        return None;
    }
    let mut adapter = runtime.companion_adapter.take().unwrap_or_default();
    let result = adapter.action(runtime, app, action);
    runtime.companion_adapter = Some(adapter);
    Some(result)
}
fn torrent_screen_action_allowed(page: &str, action: &OttAction) -> bool {
    match action {
        OttAction::Play(key) if key.starts_with("torrent.file:") => page == "torrent-files",
        OttAction::Play(_) => page == "torrent-results",
        OttAction::Back | OttAction::ShowCategories | OttAction::Refresh => {
            page.starts_with("torrent-")
        }
        OttAction::EditSetting(key) => match key.as_str() {
            "torrent.open" => page == "providers",
            "torrent.settings" => page == "settings",
            "torrent.back" | "torrent.retry" => page.starts_with("torrent-"),
            "torrent.add" | "torrent.link" => page == "torrent-sources",
            key if key.starts_with("torrent.configure:") => page == "torrent-configure",
            "torrent.language" => matches!(page, "torrent-categories" | "torrent-settings"),
            "torrent.toggle" | "torrent.start" | "torrent.cache" => page == "torrent-settings",
            key if key.starts_with("torrent.source:") => page == "torrent-sources",
            key if key.starts_with("torrent.indexer:") => page == "torrent-children",
            key if key.starts_with("torrent.category:") || key.starts_with("torrent.remove:") => {
                page == "torrent-categories"
            }
            key if key.starts_with("torrent.more:") => page == "torrent-results",
            key if key.starts_with("torrent.query:") => {
                matches!(page, "torrent-categories" | "torrent-results")
            }
            key if key.starts_with("torrent.file:") => page == "torrent-files",
            _ => false,
        },
        OttAction::SubmitSetting { session, key, .. } if *session == 0 => match key.as_str() {
            "torrent.link" => page == "torrent-sources",
            key if key.starts_with("torrent.configure:") => page == "torrent-configure",
            "torrent.language" => matches!(page, "torrent-categories" | "torrent-settings"),
            "torrent.cache" => page == "torrent-settings",
            key if key.starts_with("torrent.query:") => {
                matches!(page, "torrent-categories" | "torrent-results")
            }
            _ => false,
        },
        _ => false,
    }
}

impl Adapter {
    fn select_file(&mut self, runtime: &mut Runtime, app: &mut App, key: &str)
        -> Result<(), &'static str>
    {
        if !playback_allowed(app) { return Err("policy_restricted"); }
        // Generic Play otherwise has a weaker navigation gate than the old
        // EditSetting row. Keep both source replacement and Settings access.
        input_policy::replace_source(runtime)?;
        parental_runtime::ui_gate(runtime, app, &OttAction::EditSetting(key.into()))?;
        let mut parts = key.strip_prefix("torrent.file:").ok_or("Invalid torrent file")?.split(':');
        let generation = parts.next().and_then(|part| part.parse::<u64>().ok());
        let session_id = parts.next().ok_or("Invalid torrent file")?;
        let index = parts.next().and_then(|part| part.parse::<usize>().ok()).ok_or("Invalid torrent file")?;
        if generation != Some(self.generation) || parts.next().is_some() {
            return Err("Torrent file selection is no longer available");
        }
        let session = self.session.as_ref().ok_or("Torrent session is no longer available")?;
        if session.id != session_id || session.state != SessionState::Select
            || self.pending.iter().any(|pending| matches!(pending.request, Request::Select { .. }))
        { return Err("Torrent file selection is no longer available"); }
        let file = session.files.iter().find(|file| file.index == index).ok_or("Unknown torrent file")?;
        let request = Request::Select { id: session.id.clone(), file_index: index };
        self.title = file.name.clone();
        self.media_deadline = runtime.now_ms().saturating_add(120_000);
        self.ui_request(runtime, request, Target::Session)
    }

    fn configure_pending(&self) -> bool {
        self.pending.iter().any(|pending| matches!(pending.target, Target::Configured))
    }

    fn configure_field<'a>(&self, key: &'a str) -> Result<&'a str, &'static str> {
        let (generation, field) = key.strip_prefix("torrent.configure:")
            .and_then(|value| value.split_once(':')).ok_or("Invalid indexer field")?;
        if generation.parse::<u64>().ok() != Some(self.generation) {
            return Err("This indexer form is no longer current");
        }
        if self.configure_pending() { return Err("Wait for Load or go back"); }
        Ok(field)
    }

    fn load_draft(&mut self, runtime: &mut Runtime) -> Result<(), &'static str> {
        let snapshot = runtime.companion.snapshot();
        if !snapshot.ready || !snapshot.torrent || !snapshot.local_torznab {
            return Err("The local torrent service is unavailable");
        }
        if !self.sources_current {
            return Err("Go back and refresh sources before another Load; the submitted change may already be saved");
        }
        if self.sources.iter().any(|source| source.id == self.draft.id) {
            return Err("Choose a new source ID; this one is already configured");
        }
        let request = Request::Indexer(IndexerRequest::Configure {
            id: self.draft.id.clone(),
            label: if self.draft.label.trim().is_empty() { self.draft.id.clone() } else { self.draft.label.clone() },
            endpoint: self.draft.endpoint.clone(),
            api_key: self.draft.api_key.clone(),
        });
        request.validate().map_err(Error::code)?;
        self.ui_request(runtime, request, Target::Configured)
    }

    fn editor(
        &self,
        runtime: &Runtime,
        app: &mut App,
        key: &str,
        title: &str,
        value: String,
        secret: bool,
        limit: usize,
        text_policy: OttTextPolicy,
    ) {
        if let Some(instance) = runtime.reply {
            let editor = OttEditor {
                kind: plx_screens::ott::OttEditorKind::Setting,
                // Provider fields return into the adapter's private draft.
                // Positive sessions belong to the acknowledged profile editor.
                session: 0,
                key: key.into(),
                title: title.into(),
                value,
                secret,
                limit,
                text_policy,
            };
            app.pages.emit(
                MachineId::Nav,
                Fx::Deliver(
                    MachineId::Instance(instance),
                    Delivery::Screen(plx_ui::screen::ScreenEvent::App(AppMsg::OttEditor(
                        Arc::new(editor),
                    ))),
                ),
            );
        }
    }
    fn action(
        &mut self,
        runtime: &mut Runtime,
        app: &mut App,
        action: &OttAction,
    ) -> Result<(), &'static str> {
        // Catalog generations deliberately survive Back so the result list can
        // be restored. They do not authorize actions from a different screen.
        // This gate is only for UI actions; remote requests and replies retain
        // their existing captured request/session validation.
        if !torrent_screen_action_allowed(runtime.page, action) {
            return Err("Torrent selection is no longer available on this screen");
        }
        match action {
            OttAction::Back => {
                let page = match runtime.page {
                    "torrent-files" if !self.items.is_empty() => "torrent-results",
                    "torrent-results" => "torrent-categories",
                    "torrent-categories" if self.children.is_some() => "torrent-children",
                    "torrent-categories" | "torrent-children" | "torrent-files" | "torrent-configure" => "torrent-sources",
                    "torrent-settings" => "settings",
                    _ => "providers",
                };
                self.navigate(runtime, app);
                self.retry_media = None;
                runtime.page = page;
                runtime.status = if page == "torrent-sources" && !self.sources_current {
                    "Refresh sources before adding an indexer".into()
                } else { String::new() };
                if page == "providers" { self.invalidate_catalog(); }
            }
            OttAction::ShowCategories => {
                self.navigate(runtime, app);
                runtime.page = if self.categories.is_some() { "torrent-categories" } else { "torrent-sources" };
                runtime.status.clear();
            }
            OttAction::Refresh => {
                let page = runtime.page;
                if page == "torrent-configure" { return Ok(()); }
                self.navigate(runtime, app);
                match page {
                    "torrent-files" => self.retry_media(runtime, app)?,
                    "torrent-results" if self.categories.is_some() => self.search(runtime, false)?,
                    "torrent-categories" => self.categories_request(runtime)?,
                    "torrent-children" => self.children_request(runtime)?,
                    // Refresh is not a commit gesture for an indexer draft.
                    "torrent-settings" | "torrent-configure" => {},
                    _ => { runtime.page = "torrent-sources"; self.ui_request(runtime, Request::Indexer(IndexerRequest::List), Target::Sources)?; },
                }
            }
            OttAction::Play(key) if key.starts_with("torrent.file:") => {
                self.select_file(runtime, app, key)?;
            }
            OttAction::Play(key) => {
                let id = key.strip_prefix("torrent:").ok_or("Invalid torrent selection")?;
                let item = self.items.iter().find(|i| i.id == id).ok_or("Torrent result is no longer available")?;
                let request = item.selection.prepare_request(self.catalog_generation)?;
                self.begin_media(runtime, app, item.title.clone(), request, Target::Prepare)?;
            }
            OttAction::EditSetting(key) => match key.as_str() {
                "torrent.open" => {
                    self.navigate(runtime, app);
                    let snapshot = runtime.companion.snapshot();
                    if snapshot.ready && snapshot.torrent && snapshot.local_torznab {
                        runtime.page = "torrent-sources";
                        self.ui_request(runtime, Request::Indexer(IndexerRequest::List), Target::Sources)?;
                    } else {
                        runtime.page = "torrent-settings";
                        runtime.status = if snapshot.ready {
                            "The local service does not offer torrent indexers"
                        } else {
                            "Enable the local torrent service or retry it below"
                        }.into();
                    }
                }
                "torrent.settings" => { self.navigate(runtime, app); runtime.page = "torrent-settings"; },
                "torrent.back" => self.action(runtime, app, &OttAction::Back)?,
                "torrent.retry" => self.action(runtime, app, &OttAction::Refresh)?,
                "torrent.language" => self.editor(runtime, app, key, "Preferred result language (blank: any)",
                    runtime.saved.settings.get(LANGUAGE_KEY).and_then(Value::as_str).unwrap_or("").into(), false, 32, OttTextPolicy::Predictive),
                key if key.starts_with("torrent.more:") => {
                    self.catalog_key(key, "torrent.more:")?;
                    self.search(runtime, true)?;
                }
                key if key.starts_with("torrent.query:") => {
                    self.catalog_key(key, "torrent.query:")?;
                    self.editor(runtime, app, key, "Search torrents", self.query.clone(), false, 512, OttTextPolicy::Predictive);
                }
                "torrent.link" => self.editor(
                    runtime,
                    app,
                    key,
                    "Torrent or magnet link",
                    String::new(),
                    true,
                    16384,
                    OttTextPolicy::Literal,
                ),
                "torrent.add" => {
                    let snapshot = runtime.companion.snapshot();
                    if !snapshot.ready || !snapshot.torrent || !snapshot.local_torznab {
                        return Err("The local torrent service is unavailable");
                    }
                    if !self.sources_current { return Err("Refresh sources before adding an indexer"); }
                    self.navigate(runtime, app);
                    let id = (1..=self.sources.len() + 1).map(|n| format!("source-{n}"))
                        .find(|id| !self.sources.iter().any(|source| &source.id == id))
                        .ok_or("No source ID available")?;
                    self.draft = Draft { id, ..Draft::default() };
                    runtime.page = "torrent-configure";
                    runtime.status = "Enter an endpoint, then choose Load. Name and API key are optional.".into();
                }
                key if key.starts_with("torrent.configure:") => {
                    match self.configure_field(key)? {
                        "load" => self.load_draft(runtime)?,
                        "id" => self.editor(runtime, app, key, "Source ID", self.draft.id.clone(), false, 64, OttTextPolicy::Literal),
                        "label" => self.editor(runtime, app, key, "Name (optional)", self.draft.label.clone(), false, 128, OttTextPolicy::Predictive),
                        "endpoint" => self.editor(runtime, app, key, "Local Torznab endpoint", self.draft.endpoint.clone(), true, 4096, OttTextPolicy::Literal),
                        "key" => self.editor(runtime, app, key, "API key (optional)", self.draft.api_key.clone(), true, 2048, OttTextPolicy::Literal),
                        _ => return Err("Invalid indexer field"),
                    }
                }
                key if key.starts_with("torrent.remove:") => {
                    self.catalog_key(key, "torrent.remove:")?;
                    self.ui_request(
                        runtime,
                        Request::Indexer(IndexerRequest::Remove {
                            id: self.source.clone(),
                        }),
                        Target::Removed,
                    )?;
                }
                "torrent.toggle" => {
                    let mut config = self.applied.clone().unwrap_or_default();
                    config.enabled = !config.enabled;
                    let mut saved = runtime.saved.clone();
                    save_configuration(&mut saved, &config)?;
                    runtime.save(saved, None, false)?;
                }
                "torrent.start" => {
                    let mut config = configuration(&runtime.saved.settings)?.0;
                    config.enabled = true;
                    let mut saved = runtime.saved.clone();
                    save_configuration(&mut saved, &config)?;
                    runtime.save(saved, None, false)?;
                }
                "torrent.cache" => self.editor(
                    runtime,
                    app,
                    key,
                    "Cache limit: 64–512 MiB",
                    (self.applied.clone().unwrap_or_default().cache_bytes / 1_048_576).to_string(),
                    false,
                    3,
                    OttTextPolicy::Literal,
                ),
                key if key.starts_with("torrent.source:") => {
                    let id = self.catalog_key(key, "torrent.source:")?;
                    if !self.sources.iter().any(|s| s.id == id) { return Err("Unknown torrent source"); }
                    self.navigate(runtime, app);
                    self.invalidate_catalog();
                    self.source = id;
                    self.query.clear();
                    self.category.clear();
                    self.categories = None;
                    self.children = None;
                    self.indexer = None;
                    if runtime.companion.snapshot().local_torznab_children {
                        self.children_request(runtime)?;
                    } else { self.categories_request(runtime)?; }
                }
                key if key.starts_with("torrent.indexer:") => {
                    let id = self.catalog_key(key, "torrent.indexer:")?;
                    let children = self.children.as_ref().ok_or("Torrent indexers are unavailable")?;
                    if id != "all" && !children.indexers.iter().any(|c| c.id == id) { return Err("Unknown torrent indexer"); }
                    self.navigate(runtime, app);
                    self.invalidate_catalog();
                    self.indexer = (id != "all").then_some(id);
                    self.category.clear();
                    self.categories = None;
                    self.categories_request(runtime)?;
                }
                key if key.starts_with("torrent.category:") => {
                    let id = self.catalog_key(key, "torrent.category:")?;
                    self.category = if id == "all" {
                        Vec::new()
                    } else {
                        let id = id.parse::<u64>().map_err(|_| "Invalid torrent category")?;
                        if !self
                            .categories
                            .as_ref()
                            .is_some_and(|c| c.categories.iter().any(|v| v.id == id))
                        {
                            return Err("Unknown torrent category");
                        }
                        vec![id]
                    };
                    self.search(runtime, false)?;
                }
                key if key.starts_with("torrent.file:") => {
                    self.select_file(runtime, app, key)?;
                }
                _ => return Err("Unknown torrent action"),
            },
            OttAction::SubmitSetting { key, value, .. } => {
                let v = json!(value);
                match key.as_str() {
                    key if key.starts_with("torrent.query:") => {
                        self.catalog_key(key, "torrent.query:")?;
                        self.query = clean(&v, 512, true).ok_or("Invalid search text")?;
                        self.search(runtime, false)?;
                    }
                    "torrent.language" => {
                        let input = clean(&v, 32, true).ok_or("Invalid language")?;
                        let language = if input.trim().is_empty() { String::new() } else {
                            torrent_provider::language(&input).ok_or("Use a language name or code")?.into_owned()
                        };
                        let mut saved = runtime.saved.clone();
                        saved.settings.insert(LANGUAGE_KEY.into(), json!(language));
                        runtime.save(saved, None, false)?;
                    }
                    "torrent.link" => {
                        let link = clean(&v, 16384, false).ok_or("Invalid torrent link")?;
                        self.begin_media(
                            runtime,
                            app,
                            "Torrent".into(),
                            Request::Create {
                                source: CreateSource::Direct(link),
                                file_index: None,
                            },
                            Target::Session,
                        )?;
                    }
                    key if key.starts_with("torrent.configure:") => {
                        match self.configure_field(key)? {
                            "id" => self.draft.id = source_id(&v).ok_or("Use a lowercase source ID")?,
                            "label" => self.draft.label = clean(&v, 128, true).ok_or("Invalid indexer name")?,
                            "endpoint" => self.draft.endpoint = clean(&v, 4096, true).ok_or("Invalid indexer address")?,
                            "key" => self.draft.api_key = clean(&v, 2048, true).ok_or("Invalid indexer key")?,
                            _ => return Err("Invalid indexer field"),
                        }
                        runtime.status = "Choose Load to save this indexer".into();
                    }
                    "torrent.cache" => {
                        let mib = value
                            .parse::<u64>()
                            .ok()
                            .filter(|v| (64..=512).contains(v))
                            .ok_or("Choose 64–512 MiB")?;
                        let mut config = self.applied.clone().unwrap_or_default();
                        config.cache_bytes = mib * 1_048_576;
                        let mut saved = runtime.saved.clone();
                        save_configuration(&mut saved, &config)?;
                        runtime.save(saved, None, false)?;
                    }
                    _ => return Err("Unknown torrent setting"),
                }
            }
            _ => return Err("Unknown torrent action"),
        }
        runtime.changed();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "hostsim")]
    #[test]
    fn torrent_result_preview_keeps_selection_identity_and_omits_absent_facts() {
        use crate::app::ott_companion::protocol::{SearchItem, SearchPage};
        let _serial = plx_base::testlock::serial();
        let mut runtime = exit_fixture();
        runtime.page = "torrent-results";
        let source = IndexerSource { id: "home".into(), label: "Home".into() };
        let caps = IndexerCapabilities { source_id: source.id.clone(), search_available: true,
            max_limit: 50, categories: Vec::new() };
        let category = torrent_provider::categories(&source, Some(&caps)).unwrap().remove(0);
        let plan = torrent_provider::search_plan(&category, &caps, torrent_provider::SearchInput {
            query: "film", offset: 0, limit: 50, preferred_language: "", generation: 17,
        }).unwrap();
        let page = plan.project(SearchPage { source_id: source.id, next_offset: Some(2), total: None,
            items: vec![
                SearchItem { result_ref: "a".repeat(32), title: "Original film".into(), size: Some(2_097_152),
                    seeders: Some(0), published_at: Some("2026-11-04".into()), category_ids: Vec::new(),
                    languages: vec!["en".into(), "fr".into()] },
                SearchItem { result_ref: "b".repeat(32), title: "Unknown facts".into(), size: None,
                    seeders: None, published_at: None, category_ids: Vec::new(), languages: Vec::new() },
            ],
        }, 17).unwrap();
        let expected: Vec<_> = page.rows.iter().map(|item| (item.id.clone(),
            OttAction::Play(format!("torrent:{}", item.id)))).chain([
                ("torrent.more:17:next".into(), OttAction::EditSetting("torrent.more:17:next".into())),
                ("torrent.query:17:edit".into(), OttAction::EditSetting("torrent.query:17:edit".into())),
                ("torrent.retry".into(), OttAction::EditSetting("torrent.retry".into())),
                ("torrent.back".into(), OttAction::EditSetting("torrent.back".into())),
            ]).collect();
        let adapter = runtime.companion_adapter.as_mut().unwrap();
        adapter.catalog_generation = 17;
        adapter.items = page.rows;
        adapter.next = page.next_offset;
        let rows = view(&runtime).unwrap().rows;
        assert_eq!(rows.iter().map(|row| (row.id.clone(), row.action.clone())).collect::<Vec<_>>(), expected);
        assert_eq!(rows[0].title, "1  Original film");
        assert_eq!(rows[0].content_title.as_deref(), Some("Original film"));
        assert_eq!(rows[0].detail, "2 MiB · 0 seeders · en, fr · 2026-11-04");
        assert_eq!(rows[0].description, "2 MiB\n0 seeders\nen, fr\n2026-11-04");
        assert_eq!(rows[1].content_title.as_deref(), Some("Unknown facts"));
        assert!(rows[1].description.is_empty() && rows[1].detail.is_empty());
        assert!(rows[..2].iter().all(|row| row.media.is_none() && row.poster.is_none() && row.enabled));
        assert!(rows[2..].iter().all(|row| row.content_title.is_none() && row.description.is_empty()));
    }

    #[cfg(feature = "hostsim")]
    #[test]
    fn torrent_file_preview_numbers_labels_without_changing_sparse_file_indices_or_admission() {
        let _serial = plx_base::testlock::serial();
        let mut runtime = exit_fixture();
        runtime.page = "torrent-files";
        let adapter = runtime.companion_adapter.as_mut().unwrap();
        adapter.generation = 23;
        adapter.session = Some(crate::app::ott_companion::protocol::session_for_test(json!({
            "id":"lease-1", "state":"select", "files":[
                {"index":4,"name":"First.mp4","length":1_048_576},
                {"index":19,"name":"Folder/Second.mkv","length":2_097_152}]
        })).unwrap());
        let rows = view(&runtime).unwrap().rows;
        for (row, (index, name, size)) in rows[..2].iter().zip([
            (4, "First.mp4", "1 MiB"), (19, "Folder/Second.mkv", "2 MiB")]) {
            let id = format!("torrent.file:23:lease-1:{index}");
            assert_eq!(row.id, id);
            assert_eq!(row.action, OttAction::Play(id));
            assert_eq!(row.content_title.as_deref(), Some(name));
            assert_eq!(row.detail, size);
            assert_eq!(row.description, size);
            assert!(row.enabled && row.media.is_none() && row.poster.is_none());
        }
        assert_eq!(rows[0].title, "1  First.mp4");
        assert_eq!(rows[1].title, "2  Folder/Second.mkv");
        assert_eq!(rows[2].action, OttAction::EditSetting("torrent.retry".into()));
        assert_eq!(rows[3].action, OttAction::EditSetting("torrent.back".into()));
        runtime.companion_adapter.as_mut().unwrap().session.as_mut().unwrap().state = SessionState::Preparing;
        assert!(view(&runtime).unwrap().rows[..2].iter().all(|row| !row.enabled));
        let transport_deadline = operation_deadline(runtime.clock_sample, runtime.now_ms().saturating_add(UI_TIMEOUT)).unwrap();
        let adapter = runtime.companion_adapter.as_mut().unwrap();
        adapter.session.as_mut().unwrap().state = SessionState::Select;
        adapter.pending.push(Pending { id: None, request: Request::Select { id: "lease-1".into(), file_index: 19 },
            target: Target::Session, deadline: 0, transport_deadline, generation: 23,
            cancelled: Arc::new(AtomicBool::new(false)) });
        let rows = view(&runtime).unwrap().rows;
        assert!(rows[..2].iter().all(|row| !row.enabled));
        assert_eq!(rows[2].action, OttAction::EditSetting("torrent.back".into()));
    }

    #[cfg(feature = "hostsim")]
    fn exit_fixture() -> Runtime {
        let mut runtime =
            Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
        let worker = plx_base::task::OffFrame::for_test();
        runtime.clock_sample = Some(ClockSample::capture(&worker).unwrap());
        runtime
    }

    #[cfg(feature = "hostsim")]
    #[test]
    fn explicit_exit_retires_pending_prepare_before_its_reply_can_create_a_session() {
        let _serial = plx_base::testlock::serial();
        let mut runtime = exit_fixture();
        let deadline = runtime.now_ms().saturating_add(40_000);
        let transport_deadline = operation_deadline(runtime.clock_sample, deadline).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let adapter = runtime.companion_adapter.as_mut().unwrap();
        adapter.pending.push(Pending {
            id: Some(7),
            request: Request::Indexer(IndexerRequest::Prepare {
                source_id: "home".into(),
                result_ref: "a".repeat(32),
            }),
            target: Target::Prepare,
            deadline,
            transport_deadline,
            generation: adapter.generation,
            cancelled: cancelled.clone(),
        });
        assert!(adapter.pending.iter().any(|pending| pending.id == Some(7)));

        exit(&mut runtime);

        let adapter = runtime.companion_adapter.as_ref().unwrap();
        assert!(cancelled.load(Ordering::Acquire));
        assert!(
            adapter.pending.iter().all(|pending| pending.id != Some(7)),
            "a late Prepare reply must have no target that could create a session"
        );
        assert!(adapter.session.is_none());
    }

    #[cfg(feature = "hostsim")]
    #[test]
    fn explicit_exit_discards_an_inspection_reply_already_waiting_in_the_mailbox() {
        let _serial = plx_base::testlock::serial();
        let mut app = crate::app::run::lifecycle_regression_tests::app();
        let mut runtime = exit_fixture();
        runtime.status = "Exited player".into();
        let media_generation = runtime.play_generation;
        let media_deadline = runtime.now_ms().saturating_add(120_000);
        let adapter = runtime.companion_adapter.as_mut().unwrap();
        adapter.inspecting = true;
        adapter.media_generation = media_generation;
        adapter.media_deadline = media_deadline;
        adapter.route_ticket = Some(plx_media::route::worker_ticket());
        let generation = adapter.generation;
        let cancelled = adapter.cancelled.clone();
        assert!(adapter
            .inspected_tx
            .send(Completed::Media(Inspected {
                generation,
                result: Err("late inspector must not publish after exit"),
            }))
            .is_ok());

        exit(&mut runtime);
        tick(&mut runtime, &mut app);

        let adapter = runtime.companion_adapter.as_ref().unwrap();
        assert!(cancelled.load(Ordering::Acquire));
        assert_ne!(adapter.generation, generation);
        assert!(!adapter.inspecting && !adapter.playing);
        assert_eq!(runtime.status, "Exited player");
    }

    #[cfg(feature = "hostsim")]
    #[test]
    fn explicit_exit_clears_playing_state_without_waiting_for_navigation_or_changing_settings() {
        let _serial = plx_base::testlock::serial();
        let mut runtime = exit_fixture();
        let before = json!(runtime.companion.snapshot());
        let saved = runtime.saved.settings.clone();
        let adapter = runtime.companion_adapter.as_mut().unwrap();
        adapter.playing = true;
        adapter.media_deadline = 120_000;
        adapter.next_poll = 30_000;
        adapter.route_ticket = Some(plx_media::route::worker_ticket());
        adapter.title = "Playing fixture".into();
        let cancelled = adapter.cancelled.clone();

        exit(&mut runtime);

        let adapter = runtime.companion_adapter.as_ref().unwrap();
        assert!(cancelled.load(Ordering::Acquire));
        assert!(!adapter.playing && !adapter.inspecting);
        assert!(adapter.session.is_none() && adapter.route_ticket.is_none());
        assert!(adapter.title.is_empty());
        assert_eq!(adapter.media_deadline, 0);
        assert_eq!(adapter.next_poll, 0);
        assert_eq!(runtime.saved.settings, saved);
        assert!(runtime.resume_pending.is_none());
        assert_eq!(
            json!(runtime.companion.snapshot()),
            before,
            "explicit media exit must not stop or reconfigure the helper"
        );
    }

    fn search() -> Value {
        json!({"version":1,"runtime":"native-session","op":"search","sourceId":"home","query":"film","categoryIds":[2000],"offset":0,"limit":50})
    }
    #[test]
    fn remote_search_contract_and_policy_classification() {
        let (op, request, writable) = indexer_request(&search(), "native-session").unwrap();
        assert_eq!(op, "search");
        assert!(!writable);
        assert!(matches!(
            request,
            Request::Indexer(IndexerRequest::Search { .. })
        ));
        let configure = json!({"version":1,"runtime":"native-session","op":"configure","sourceId":"home","label":"Home","endpoint":"http://127.0.0.1/api","apiKey":"secret"});
        assert!(indexer_request(&configure, "native-session").unwrap().2);
    }
    #[test]
    fn remote_rejects_stale_runtime_and_extra_or_missing_fields() {
        assert_eq!(
            indexer_request(&search(), "new-session").unwrap_err(),
            "runtime_changed"
        );
        let mut value = search();
        value["debug"] = json!(true);
        assert!(indexer_request(&value, "native-session").is_err());
        let mut value = search();
        value.as_object_mut().unwrap().remove("query");
        assert!(indexer_request(&value, "native-session").is_err());
    }
    #[test]
    fn remote_rejects_invalid_bytes_and_numbers() {
        for (key, value) in [
            ("limit", json!(0)),
            ("offset", json!(-1)),
            ("categoryIds", json!([0])),
            ("query", json!("bad\nquery")),
            ("sourceId", json!("../home")),
        ] {
            let mut params = search();
            params[key] = value;
            assert!(indexer_request(&params, "native-session").is_err());
        }
    }
    #[test]
    fn plugin_resolution_accepts_only_the_exact_bounded_contract() {
        let params = json!({"version":1,"runtime":"native-session","op":"resolve_plugin",
            "id":"direct-url","input":{"url":"https://media.example/movie.mp4"}});
        let request = plugin_request(&params, "native-session").unwrap();
        assert!(matches!(&request, Request::ResolvePlugin { id, input }
            if id == "direct-url" && input["url"] == "https://media.example/movie.mp4"));
        assert!(!format!("{request:?}").contains("media.example"));
        assert_eq!(
            plugin_request(&params, "new-session").unwrap_err(),
            "runtime_changed"
        );
        for (key, value) in [
            ("version", json!(2)),
            ("op", json!("install_plugin")),
            ("id", json!("../direct-url")),
            ("id", json!("")),
            ("input", json!([])),
            ("input", json!(null)),
            ("input", json!({"payload":"x".repeat(65_536)})),
            ("endpoint", json!("http://127.0.0.1/private")),
        ] {
            let mut invalid = params.clone();
            invalid[key] = value;
            assert!(plugin_request(&invalid, "native-session").is_err(), "{key}");
        }
        let mut missing = params;
        missing.as_object_mut().unwrap().remove("input");
        assert!(plugin_request(&missing, "native-session").is_err());
    }

    #[test]
    fn resolved_plugin_reply_returns_proposals_without_playback_identity() {
        let result = reply_value(Reply::Resolved(vec![
            crate::app::ott_companion::protocol::PluginStream {
                title: "Example film".into(),
                url: "https://media.example/movie.mp4".into(),
            },
        ]))
        .unwrap();
        assert_eq!(
            result,
            json!({"streams":[{
                "url":"https://media.example/movie.mp4","title":"Example film"
            }]})
        );
        assert!(result.get("session").is_none());
        assert!(result.get("streamPath").is_none());
        assert!(result.get("endpoint").is_none());
        assert!(!result.to_string().contains("/companion/"));
    }
    #[test]
    fn wire_sources_never_contain_endpoint_or_key() {
        let result = reply_value(Reply::Sources(vec![IndexerSource {
            id: "home".into(),
            label: "Home".into(),
        }]))
        .unwrap();
        assert_eq!(result, json!({"sources":[{"id":"home","label":"Home"}]}));
    }
    #[test]
    fn prepared_reply_is_an_opaque_reference() {
        let result = reply_value(Reply::Prepared(
            crate::app::ott_companion::protocol::PreparedSource {
                source_id: "home".into(),
                source_ref: "a".repeat(32),
            },
        ))
        .unwrap();
        assert!(result.get("sourceRef").is_some());
        assert!(result.get("streamPath").is_none());
        assert!(result.get("endpoint").is_none());
    }

    #[test]
    fn plugin_manifest_reply_contains_policy_and_limits_without_network_addresses() {
        let result = reply_value(Reply::Plugins(vec![
            crate::app::ott_companion::protocol::Plugin {
                id: "direct-url".into(),
                name: "Direct URL".into(),
                api_version: 1,
                builtin: true,
                network: false,
                allowed_hosts: vec!["private-host.invalid".into()],
                max_input_bytes: 4096,
                max_output_bytes: 8192,
                max_streams: 2,
                memory_limit_bytes: 1_048_576,
                timeout_ms: 1000,
                max_network_calls: 0,
                max_fetch_bytes: 0,
            },
        ]))
        .unwrap();
        let plugin = &result["plugins"][0];
        assert_eq!(plugin["builtin"], true);
        assert_eq!(plugin["network"], false);
        assert_eq!(plugin["apiVersion"], 1);
        assert_eq!(plugin["allowedHostCount"], 1);
        assert_eq!(plugin["maxStreams"], 2);
        assert_eq!(plugin["timeoutMs"], 1000);
        assert!(plugin.get("url").is_none());
        assert!(plugin.get("allowedHosts").is_none());
        assert!(!result.to_string().contains("private-host.invalid"));
    }

    #[test]
    fn malformed_and_semantically_invalid_configuration_are_rejected_before_effects() {
        for value in [
            json!({"enabled":true,"cacheBytes":0}),
            json!({"enabled":"yes","cacheBytes":268435456}),
            json!({"enabled":true,"cacheBytes":268435456,"unknown":true}),
        ] {
            let mut settings = BTreeMap::new();
            settings.insert(SETTINGS_KEY.into(), value);
            assert!(configuration(&settings).is_err());
        }
    }
    #[test]
    fn every_explicit_retry_has_a_new_durable_revision_even_with_unchanged_settings() {
        let mut saved = Saved::default();
        let config = Configuration::default();
        save_configuration(&mut saved, &config).unwrap();
        assert_eq!(configuration(&saved.settings).unwrap().1, 1);
        save_configuration(&mut saved, &config).unwrap();
        assert_eq!(configuration(&saved.settings).unwrap().1, 2);
    }
    #[test]
    fn sampled_clock_is_required_and_expired_request_cannot_get_an_http_deadline() {
        let worker = plx_base::task::OffFrame::for_test();
        let sample = ClockSample::capture(&worker).unwrap();
        assert!(operation_deadline(None, u64::MAX).is_err());
        assert!(operation_deadline(Some(sample), 0).is_err());
        assert!(operation_deadline(Some(sample), sample.unix_ms()).is_err());
    }
    #[test]
    fn admitted_deadline_keeps_its_original_bounded_budget() {
        let worker = plx_base::task::OffFrame::for_test();
        let sample = ClockSample::capture(&worker).unwrap();
        let admitted = operation_deadline(Some(sample), u64::MAX).unwrap();
        let queued = admitted;
        let initial = admitted.remaining(&worker);
        assert!(initial <= std::time::Duration::from_millis(120_000));
        assert!(queued.remaining(&worker) <= initial);
    }
}

impl Adapter {
    fn launch(&self, work: Work) -> Result<(), &'static str> {
        if !self.worker_done.load(Ordering::Acquire) {
            return Err("Previous provider request is finishing. Retry shortly.");
        }
        let tx = self.inspected_tx.clone();
        let cancel = self.cancelled.clone();
        let generation = self.generation;
        if !crate::app::ott_worker::spawn("ott-torrent-media", self.worker_done.clone(), move |worker| {
            let completed = match work {
                Work::Inspect(source) => Completed::Media(Inspected {
                    generation,
                    result: plx_media::ff::inspect::source(&source, cancel.clone())
                        .map(|info| (source, info)),
                }),
                Work::Stremio(request) => Completed::Stremio(stremio::fetch(worker, &cancel, request)),
            };
            let _ = tx.send(completed);
            plx_machine::idle::invalidate();
        }) { return Err("Could not start provider worker"); }
        Ok(())
    }
}
#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_companion_torrent_tests.rs"]
mod torrent_flow_tests;
