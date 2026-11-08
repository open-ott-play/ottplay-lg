//! Pure, bounded adapter for the existing local media companion's v1 protocol.
//! Capability URLs are generated from a supervisor-owned port and secret, never from media.
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fmt;
use std::io::Write;

/// Limits mirror the companion control plane; media bytes never enter this JSON adapter.
pub(super) const RESPONSE_BYTES: usize = 1024 * 1024;
pub(super) const REQUEST_BYTES: usize = 64 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct Endpoint {
    port: u16,
    token: String,
}

impl Endpoint {
    pub fn new(port: u16, token: String) -> Result<Self, Error> {
        if port == 0 || !(32..=128).contains(&token.len()) || !token.bytes().all(id_byte) {
            return Err(Error("companion_endpoint_invalid"));
        }
        Ok(Self { port, token })
    }

    pub(super) fn port(&self) -> u16 {
        self.port
    }

    pub(super) fn path(&self, route: &str) -> String {
        format!("/companion/{}/v1/{route}", self.token)
    }

    fn stream_url(&self, id: &str) -> String {
        format!(
            "http://127.0.0.1:{}{}",
            self.port,
            self.path(&format!("sessions/{id}/stream"))
        )
    }
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error(pub(super) &'static str);

impl Error {
    pub fn code(self) -> &'static str {
        self.0
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Error {}

/// Source URLs and indexer credentials deliberately have no value-printing Debug impl.
#[derive(Clone)]
pub enum CreateSource {
    Direct(String),
    LocalReference {
        source_id: String,
        source_ref: String,
    },
}

#[derive(Clone)]
pub enum IndexerRequest {
    List,
    Indexers {
        source_id: String,
    },
    ScopedCapabilities {
        source_id: String,
        indexer_id: String,
    },
    ScopedSearch {
        source_id: String,
        indexer_id: String,
        query: String,
        category_ids: Vec<u64>,
        offset: u64,
        limit: usize,
    },
    Configure {
        id: String,
        label: String,
        endpoint: String,
        api_key: String,
    },
    Remove {
        id: String,
    },
    Capabilities {
        id: String,
    },
    Search {
        source_id: String,
        query: String,
        category_ids: Vec<u64>,
        offset: u64,
        limit: usize,
    },
    Prepare {
        source_id: String,
        result_ref: String,
    },
}

#[derive(Clone)]
pub enum Request {
    Capabilities,
    Indexer(IndexerRequest),
    Create {
        source: CreateSource,
        file_index: Option<usize>,
    },
    Session {
        id: String,
    },
    Select {
        id: String,
        file_index: usize,
    },
    Delete {
        id: String,
    },
    Plugins,
    ResolvePlugin {
        id: String,
        input: Value,
    },
}

impl fmt::Debug for CreateSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CreateSource([redacted])")
    }
}
impl fmt::Debug for IndexerRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("IndexerRequest([redacted])")
    }
}
impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Request([redacted])")
    }
}

pub(super) struct PreparedRequest {
    pub method: &'static str,
    pub route: String,
    pub body: Vec<u8>,
}

impl Request {
    /// Apply the same bounded wire validation before an adapter admits work.
    pub(crate) fn validate(&self) -> Result<(), Error> {
        self.prepare().map(|_| ())
    }

    pub(super) fn prepare(&self) -> Result<PreparedRequest, Error> {
        let (method, route, body) = match self {
            Self::Capabilities => ("GET", "capabilities".into(), None),
            Self::Plugins => ("GET", "plugins".into(), None),
            Self::ResolvePlugin { id, input } => {
                require(source_id(id) && input.is_object(), "plugin_invalid_input")?;
                // Bound the caller-owned value before json! clones it into the envelope.
                let _ = encode(input)?;
                (
                    "POST",
                    format!("plugins/{id}/resolve"),
                    Some(json!({"input":input})),
                )
            }
            Self::Create { source, file_index } => {
                require(file_index.is_none_or(|i| i < 4096), "invalid_file")?;
                let mut body = match source {
                    CreateSource::Direct(value) => {
                        require(valid_source(value), "invalid_source")?;
                        json!({"source":value})
                    }
                    CreateSource::LocalReference {
                        source_id: id,
                        source_ref,
                    } => {
                        require(source_id(id) && reference(source_ref), "invalid_source")?;
                        json!({"sourceId":id,"sourceRef":source_ref})
                    }
                };
                if let Some(index) = file_index {
                    body["fileIndex"] = json!(index);
                }
                ("POST", "sessions".into(), Some(body))
            }
            Self::Session { id } | Self::Delete { id } => {
                require(session_id(id), "session_missing")?;
                (
                    if matches!(self, Self::Delete { .. }) {
                        "DELETE"
                    } else {
                        "GET"
                    },
                    format!("sessions/{id}"),
                    None,
                )
            }
            Self::Select { id, file_index } => {
                require(session_id(id) && *file_index < 4096, "invalid_file")?;
                (
                    "POST",
                    format!("sessions/{id}/select"),
                    Some(json!({"fileIndex":file_index})),
                )
            }
            Self::Indexer(op) => match op {
                IndexerRequest::List => ("GET", "torrent/sources".into(), None),
                IndexerRequest::Indexers { source_id: id } => {
                    require(source_id(id), "torznab_invalid_source")?;
                    ("GET", format!("torrent/sources/{id}/indexers"), None)
                }
                IndexerRequest::ScopedCapabilities {
                    source_id: id,
                    indexer_id,
                } => {
                    require(source_id(id), "torznab_invalid_source")?;
                    require(child_id(indexer_id), "torznab_invalid_query")?;
                    (
                        "GET",
                        format!("torrent/sources/{id}/caps?indexerId={indexer_id}"),
                        None,
                    )
                }
                IndexerRequest::ScopedSearch {
                    source_id,
                    indexer_id,
                    query,
                    category_ids,
                    offset,
                    limit,
                } => {
                    require(child_id(indexer_id), "torznab_invalid_query")?;
                    let mut body = search_body(source_id, query, category_ids, *offset, *limit)?;
                    body["indexerId"] = json!(indexer_id);
                    ("POST", "torrent/search".into(), Some(body))
                }
                IndexerRequest::Configure {
                    id,
                    label,
                    endpoint,
                    api_key,
                } => {
                    require(
                        source_id(id)
                            && text(label.trim(), 128)
                            && clean(api_key, 2048)
                            && valid_indexer_url(endpoint),
                        "torznab_invalid_source",
                    )?;
                    (
                        "POST",
                        "torrent/sources".into(),
                        Some(
                            json!({"id":id,"label":label.trim(),"endpoint":endpoint,"apiKey":api_key}),
                        ),
                    )
                }
                IndexerRequest::Remove { id } | IndexerRequest::Capabilities { id } => {
                    require(source_id(id), "torznab_invalid_source")?;
                    if matches!(op, IndexerRequest::Remove { .. }) {
                        ("DELETE", format!("torrent/sources/{id}"), None)
                    } else {
                        ("GET", format!("torrent/sources/{id}/caps"), None)
                    }
                }
                IndexerRequest::Search {
                    source_id: id,
                    query,
                    category_ids,
                    offset,
                    limit,
                } => (
                    "POST",
                    "torrent/search".into(),
                    Some(search_body(id, query, category_ids, *offset, *limit)?),
                ),
                IndexerRequest::Prepare {
                    source_id: id,
                    result_ref,
                } => {
                    require(
                        source_id(id) && reference(result_ref),
                        "torznab_reference_missing",
                    )?;
                    (
                        "POST",
                        "torrent/prepare".into(),
                        Some(json!({"sourceId":id,"resultRef":result_ref})),
                    )
                }
            },
        };
        Ok(PreparedRequest {
            method,
            route,
            body: body.as_ref().map(encode).transpose()?.unwrap_or_default(),
        })
    }
}

fn search_body(
    id: &str,
    query: &str,
    category_ids: &[u64],
    offset: u64,
    limit: usize,
) -> Result<Value, Error> {
    require(
        source_id(id)
            && clean(query, 512)
            && category_ids.len() <= 16
            && category_ids.iter().all(|v| safe_integer(*v) && *v > 0)
            && offset <= 1_000_000
            && (1..=100).contains(&limit),
        "torznab_invalid_query",
    )?;
    Ok(
        json!({"sourceId":id,"query":query,"categoryIds":category_ids,"offset":offset,"limit":limit}),
    )
}

/// A literal configured child ID, never a path/filter/query or the aggregate feed.
fn child_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.eq_ignore_ascii_case("all")
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub torrent: bool,
    pub plugins: bool,
    pub local_torznab: bool,
    #[serde(default)]
    pub local_torznab_children: bool,
    pub file_selection: bool,
    pub torrent_sources: Vec<String>,
    pub max_sessions: usize,
    #[serde(default)]
    pub max_streams: usize,
    pub cache_bytes: u64,
    pub inbound_peer_listener: bool,
    pub plugin_network: bool,
    pub plugin_network_default: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct IndexerSource {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct IndexerChild {
    pub id: String,
    pub label: String,
    pub language: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexerChildren {
    pub source_id: String,
    pub supported: bool,
    pub indexers: Vec<IndexerChild>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: u64,
    pub name: String,
    pub parent_id: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexerCapabilities {
    pub source_id: String,
    pub search_available: bool,
    pub categories: Vec<Category>,
    pub max_limit: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchItem {
    pub result_ref: String,
    pub title: String,
    pub size: Option<u64>,
    pub seeders: Option<u64>,
    pub published_at: Option<String>,
    pub category_ids: Vec<u64>,
    pub languages: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    pub source_id: String,
    pub items: Vec<SearchItem>,
    pub next_offset: Option<u64>,
    pub total: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedSource {
    pub source_id: String,
    pub source_ref: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct MediaFile {
    pub index: usize,
    pub name: String,
    pub length: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Metadata,
    Select,
    Preparing,
    Ready,
    Error,
}

#[derive(Clone)]
pub struct Session {
    pub id: String,
    pub state: SessionState,
    pub files: Vec<MediaFile>,
    pub selected: Option<usize>,
    pub downloaded: Option<u64>,
    pub total: Option<u64>,
    pub error: Option<Error>,
    owner: Endpoint,
    ready_url: Option<String>,
}

impl Session {
    /// Only a ready session belonging to this live endpoint can produce a playback capability.
    pub fn stream_url(&self, endpoint: &Endpoint) -> Result<String, Error> {
        require(
            self.state == SessionState::Ready && endpoint == &self.owner,
            "companion_receipt_invalid",
        )?;
        let url = endpoint.stream_url(&self.id);
        require(
            self.ready_url.as_ref() == Some(&url),
            "companion_receipt_invalid",
        )?;
        Ok(url)
    }
}
impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("state", &self.state)
            .field("file_count", &self.files.len())
            .field("selected", &self.selected)
            .field("downloaded", &self.downloaded)
            .field("total", &self.total)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plugin {
    pub id: String,
    pub name: String,
    pub api_version: u8,
    pub builtin: bool,
    pub network: bool,
    pub allowed_hosts: Vec<String>,
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
    pub max_streams: usize,
    pub memory_limit_bytes: usize,
    pub timeout_ms: u64,
    pub max_network_calls: usize,
    pub max_fetch_bytes: usize,
}

#[derive(Clone, Deserialize)]
pub struct PluginStream {
    pub title: String,
    pub url: String,
}
impl fmt::Debug for PluginStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PluginStream([redacted])")
    }
}

#[derive(Clone, Debug)]
pub enum Reply {
    Capabilities(Capabilities),
    Sources(Vec<IndexerSource>),
    Indexers(IndexerChildren),
    ScopedCategories {
        indexer_id: String,
        capabilities: IndexerCapabilities,
    },
    ScopedSearch {
        indexer_id: String,
        page: SearchPage,
    },
    Configured(IndexerSource),
    Removed,
    Categories(IndexerCapabilities),
    Search(SearchPage),
    Prepared(PreparedSource),
    Session(Session),
    Deleted,
    Plugins(Vec<Plugin>),
    /// Plugin URLs are proposals, not owned media leases. The caller must apply source policy.
    Resolved(Vec<PluginStream>),
}

pub(super) fn decode(
    endpoint: &Endpoint,
    request: &Request,
    status: u16,
    bytes: &[u8],
) -> Result<Reply, Error> {
    require(bytes.len() <= RESPONSE_BYTES, "companion_response_limit")?;
    if status == 204 && matches!(request, Request::Delete { .. }) && bytes.is_empty() {
        return Ok(Reply::Deleted);
    }
    if status == 401 || status == 403 {
        return Err(Error("companion_access_denied"));
    }
    // Redirects must never be followed with the URL capability.
    require(!(300..400).contains(&status), "companion_redirect_denied")?;
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| Error("companion_receipt_invalid"))?;
    if !(200..300).contains(&status) {
        return Err(backend_error(
            value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ));
    }
    if !matches!(
        request,
        Request::Create { .. }
            | Request::Session { .. }
            | Request::Select { .. }
            | Request::ResolvePlugin { .. }
    ) {
        require(
            value.get("schema").and_then(Value::as_u64) == Some(1),
            "companion_receipt_invalid",
        )?;
    }
    match request {
        Request::Capabilities => {
            let caps: Capabilities = from_value(value)?;
            require(
                (!caps.local_torznab_children || caps.local_torznab)
                    && caps.torrent_sources.len() <= 8
                    && caps.torrent_sources.iter().all(|s| {
                        matches!(s.as_str(), "magnet" | "http" | "https" | "local-reference")
                    })
                    && caps.max_sessions <= 64
                    && caps.max_streams <= 64
                    && safe_integer(caps.cache_bytes),
                "companion_receipt_invalid",
            )?;
            Ok(Reply::Capabilities(caps))
        }
        Request::Create { .. } | Request::Session { .. } | Request::Select { .. } => {
            let expected = match request {
                Request::Session { id } | Request::Select { id, .. } => Some(id.as_str()),
                _ => None,
            };
            Ok(Reply::Session(parse_session(endpoint, value, expected)?))
        }
        Request::Delete { .. } => Err(Error("companion_receipt_invalid")),
        Request::Indexer(op) => decode_indexer(op, value),
        Request::Plugins => {
            let plugins: Vec<Plugin> = field(&value, "plugins")?;
            require(plugins.len() <= 64, "companion_receipt_invalid")?;
            let mut ids = HashSet::new();
            for plugin in &plugins {
                require(
                    source_id(&plugin.id)
                        && ids.insert(plugin.id.as_str())
                        && text(&plugin.name, 256)
                        && plugin.api_version == 1
                        && plugin.allowed_hosts.len() <= 64
                        && plugin.allowed_hosts.iter().all(|s| text(s, 253))
                        && plugin.max_input_bytes <= REQUEST_BYTES
                        && plugin.max_output_bytes <= RESPONSE_BYTES
                        && plugin.max_streams <= 128
                        && plugin.memory_limit_bytes <= 256 * 1024 * 1024
                        && plugin.timeout_ms <= 60_000
                        && plugin.max_network_calls <= 64
                        && plugin.max_fetch_bytes <= 16 * 1024 * 1024,
                    "companion_receipt_invalid",
                )?;
            }
            Ok(Reply::Plugins(plugins))
        }
        Request::ResolvePlugin { .. } => {
            let streams: Vec<PluginStream> = field(&value, "streams")?;
            require(
                streams.len() <= 128
                    && streams.iter().all(|s| {
                        text(&s.title, 256)
                            && valid_source(&s.url)
                            && !contains_capability(&s.title, &endpoint.token)
                            && !contains_capability(&s.url, &endpoint.token)
                    }),
                "plugin_invalid_output",
            )?;
            Ok(Reply::Resolved(streams))
        }
    }
}

fn parse_session(
    endpoint: &Endpoint,
    value: Value,
    expected: Option<&str>,
) -> Result<Session, Error> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct WireSession {
        id: String,
        state: SessionState,
        files: Vec<MediaFile>,
        selected: Option<usize>,
        stream_path: Option<String>,
        downloaded: Option<u64>,
        total: Option<u64>,
        error: Option<String>,
    }
    let wire: WireSession = from_value(value)?;
    require(
        session_id(&wire.id) && expected.is_none_or(|id| id == wire.id) && wire.files.len() <= 4096,
        "companion_receipt_invalid",
    )?;
    let mut indexes = HashSet::new();
    for file in &wire.files {
        require(
            file.index < 4096
                && indexes.insert(file.index)
                && text(&file.name, 4096)
                && safe_integer(file.length),
            "companion_receipt_invalid",
        )?;
    }
    require(
        wire.selected.is_none_or(|index| {
            wire.files
                .iter()
                .any(|file| file.index == index && file.length > 0)
        }) && wire.downloaded.is_none_or(safe_integer)
            && wire.total.is_none_or(safe_integer),
        "companion_receipt_invalid",
    )?;
    let ready = wire.state == SessionState::Ready;
    require(
        if ready {
            wire.selected.is_some()
                && wire.stream_path.as_deref()
                    == Some(format!("sessions/{}/stream", wire.id).as_str())
                && wire.error.is_none()
        } else {
            wire.stream_path.is_none()
        },
        "companion_receipt_invalid",
    )?;
    let ready_url = ready.then(|| endpoint.stream_url(&wire.id));
    Ok(Session {
        id: wire.id,
        state: wire.state,
        files: wire.files,
        selected: wire.selected,
        downloaded: wire.downloaded,
        total: wire.total,
        error: wire.error.as_deref().map(backend_error),
        owner: endpoint.clone(),
        ready_url,
    })
}

/// Remove only a validated exact echo before reusing aggregate field validation.
/// Missing, null, differently-cased or unexpected scope is never an aggregate retry.
fn take_child_echo(value: &mut Value, expected: &str) -> Result<(), Error> {
    require(
        child_id(expected) && value.get("indexerId").and_then(Value::as_str) == Some(expected),
        "companion_receipt_invalid",
    )?;
    value
        .as_object_mut()
        .ok_or(Error("companion_receipt_invalid"))?
        .remove("indexerId");
    Ok(())
}

fn decode_indexer(op: &IndexerRequest, mut value: Value) -> Result<Reply, Error> {
    match op {
        IndexerRequest::Indexers { source_id } => {
            let children: IndexerChildren = from_value(value)?;
            let mut ids = HashSet::new();
            require(
                children.source_id == *source_id
                    && children.indexers.len() <= 256
                    && (children.supported || children.indexers.is_empty())
                    && children.indexers.iter().all(|child| {
                        child_id(&child.id)
                            && ids.insert(child.id.as_str())
                            && text(&child.label, 128)
                            && child
                                .language
                                .as_deref()
                                .is_none_or(|value| text(value, 32))
                            && child.kind.as_deref().is_none_or(|value| text(value, 32))
                    }),
                "companion_receipt_invalid",
            )?;
            Ok(Reply::Indexers(children))
        }
        IndexerRequest::ScopedCapabilities {
            source_id,
            indexer_id,
        } => {
            take_child_echo(&mut value, indexer_id)?;
            let Reply::Categories(capabilities) = decode_indexer(
                &IndexerRequest::Capabilities {
                    id: source_id.clone(),
                },
                value,
            )?
            else {
                return Err(Error("companion_receipt_invalid"));
            };
            Ok(Reply::ScopedCategories {
                indexer_id: indexer_id.clone(),
                capabilities,
            })
        }
        IndexerRequest::ScopedSearch {
            source_id,
            indexer_id,
            query,
            category_ids,
            offset,
            limit,
        } => {
            take_child_echo(&mut value, indexer_id)?;
            let Reply::Search(page) = decode_indexer(
                &IndexerRequest::Search {
                    source_id: source_id.clone(),
                    query: query.clone(),
                    category_ids: category_ids.clone(),
                    offset: *offset,
                    limit: *limit,
                },
                value,
            )?
            else {
                return Err(Error("companion_receipt_invalid"));
            };
            Ok(Reply::ScopedSearch {
                indexer_id: indexer_id.clone(),
                page,
            })
        }
        IndexerRequest::List => {
            let sources: Vec<IndexerSource> = field(&value, "sources")?;
            let mut ids = HashSet::new();
            require(
                sources.len() <= 64
                    && sources
                        .iter()
                        .all(|s| valid_indexer(s) && ids.insert(s.id.as_str())),
                "companion_receipt_invalid",
            )?;
            Ok(Reply::Sources(sources))
        }
        IndexerRequest::Configure { id, .. } => {
            let source: IndexerSource = field(&value, "source")?;
            require(
                valid_indexer(&source) && source.id == *id,
                "companion_receipt_invalid",
            )?;
            Ok(Reply::Configured(source))
        }
        IndexerRequest::Remove { .. } => {
            require(
                value.get("removed").and_then(Value::as_bool) == Some(true),
                "companion_receipt_invalid",
            )?;
            Ok(Reply::Removed)
        }
        IndexerRequest::Capabilities { id } => {
            require(
                value.get("indexerId").is_none(),
                "companion_receipt_invalid",
            )?;
            let caps: IndexerCapabilities = from_value(value)?;
            let mut ids = HashSet::new();
            require(
                caps.source_id == *id
                    && (1..=100).contains(&caps.max_limit)
                    && caps.categories.len() <= 512
                    && caps.categories.iter().all(|c| {
                        c.id > 0
                            && safe_integer(c.id)
                            && ids.insert(c.id)
                            && text(&c.name, 128)
                            && c.parent_id.is_none_or(|i| i > 0 && safe_integer(i))
                    }),
                "companion_receipt_invalid",
            )?;
            Ok(Reply::Categories(caps))
        }
        IndexerRequest::Search {
            source_id,
            offset,
            limit,
            ..
        } => {
            require(
                value.get("indexerId").is_none(),
                "companion_receipt_invalid",
            )?;
            let page: SearchPage = from_value(value)?;
            require(
                page.source_id == *source_id
                    && page.items.len() <= *limit
                    && page
                        .next_offset
                        .is_none_or(|next| next > *offset && next <= 1_000_000)
                    && page.total.is_none_or(safe_integer),
                "companion_receipt_invalid",
            )?;
            for item in &page.items {
                require(
                    reference(&item.result_ref)
                        && text(&item.title, 4096)
                        && item.size.is_none_or(safe_integer)
                        && item.seeders.is_none_or(safe_integer)
                        && item.published_at.as_ref().is_none_or(|v| text(v, 128))
                        && item.category_ids.len() <= 32
                        && item.category_ids.iter().all(|i| *i > 0 && safe_integer(*i))
                        && item.languages.len() <= 16
                        && item.languages.iter().all(|l| text(l, 64)),
                    "companion_receipt_invalid",
                )?;
            }
            Ok(Reply::Search(page))
        }
        IndexerRequest::Prepare { source_id, .. } => {
            let source: PreparedSource = from_value(value)?;
            require(
                source.source_id == *source_id && reference(&source.source_ref),
                "companion_receipt_invalid",
            )?;
            Ok(Reply::Prepared(source))
        }
    }
}

fn field<T: serde::de::DeserializeOwned>(value: &Value, key: &str) -> Result<T, Error> {
    from_value(
        value
            .get(key)
            .cloned()
            .ok_or(Error("companion_receipt_invalid"))?,
    )
}
fn from_value<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Error> {
    serde_json::from_value(value).map_err(|_| Error("companion_receipt_invalid"))
}
fn require(test: bool, code: &'static str) -> Result<(), Error> {
    if test {
        Ok(())
    } else {
        Err(Error(code))
    }
}
fn safe_integer(value: u64) -> bool {
    value <= 9_007_199_254_740_991
}
fn id_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}
fn session_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(id_byte)
}
fn source_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn reference(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn clean(value: &str, limit: usize) -> bool {
    value.len() <= limit && !value.chars().any(char::is_control)
}
fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && clean(value, limit)
}
fn valid_indexer(source: &IndexerSource) -> bool {
    source_id(&source.id) && text(&source.label, 128)
}
fn valid_source(value: &str) -> bool {
    clean(value, 8192)
        && (value.starts_with("magnet:?") && value.contains("xt=urn:")
            || plx_data::ott::http_url(value).is_some())
}
// A plugin result is a source proposal, never a way to export this launch's
// private helper credential. Check URL-encoded spellings as well as plain text.
fn contains_capability(value: &str, token: &str) -> bool {
    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
    let input = value.as_bytes();
    let mut decoded = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' && index + 2 < input.len() {
            if let (Some(high), Some(low)) = (hex(input[index + 1]), hex(input[index + 2])) {
                decoded.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        decoded.push(input[index]);
        index += 1;
    }
    decoded
        .windows(token.len())
        .any(|part| part == token.as_bytes())
}
fn valid_indexer_url(value: &str) -> bool {
    if !clean(value, 4096) || value.contains(['?', '#']) || plx_data::ott::http_url(value).is_none()
    {
        return false;
    }
    let Some(origin) = plx_net::net::origin::Origin::parse(value) else {
        return false;
    };
    // DNS names are resolved and pinned by the helper. Literal public addresses fail here too.
    match origin.host().parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private(),
        Ok(std::net::IpAddr::V6(ip)) => {
            ip.is_loopback()
                || ip.is_unique_local()
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|ip| ip.is_loopback() || ip.is_private())
        }
        Err(_) => true,
    }
}

fn encode(value: &Value) -> Result<Vec<u8>, Error> {
    struct Bounded(Vec<u8>);
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > REQUEST_BYTES.saturating_sub(self.0.len()) {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut out = Bounded(Vec::new());
    serde_json::to_writer(&mut out, value).map_err(|_| Error("companion_request_limit"))?;
    Ok(out.0)
}

/// Never return arbitrary helper text (which can contain an indexer URL or API key).
fn backend_error(value: &str) -> Error {
    macro_rules! known { ($($code:literal),* $(,)?) => { match value { $($code => Error($code),)* _ => Error("companion_request_failed") } }; }
    known!(
        "busy",
        "cleanup_pending",
        "cache_limit",
        "cache_unavailable",
        "service_stopped",
        "session_missing",
        "session_stopped",
        "session_busy",
        "session_limit",
        "stream_limit",
        "stream_timeout",
        "webseed_unavailable",
        "webseed_timeout",
        "webseed_invalid_range",
        "webseed_hash_mismatch",
        "webseed_network_denied",
        "invalid_request",
        "invalid_source",
        "invalid_file",
        "invalid_metadata",
        "invalid_tracker",
        "tracker_limit",
        "file_selection_required",
        "file_limit",
        "engine_unavailable",
        "metadata_failed",
        "metadata_timeout",
        "metadata_limit",
        "no_files",
        "torrent_failed",
        "initialization_timeout",
        "range_failed",
        "torznab_invalid_source",
        "torznab_invalid_link",
        "torznab_invalid_query",
        "torznab_invalid_category",
        "torznab_invalid_xml",
        "torznab_invalid_page",
        "torznab_local_only",
        "torznab_search_unavailable",
        "torznab_source_missing",
        "torznab_source_changed",
        "torznab_source_limit",
        "torznab_reference_missing",
        "torznab_busy",
        "torznab_cancelled",
        "torznab_timeout",
        "torznab_network_error",
        "torznab_http_error",
        "torznab_response_limit",
        "torznab_xml_limit",
        "torznab_api_error",
        "torznab_store_unavailable",
        "torznab_store_busy",
        "torznab_store_limit",
        "plugin_invalid_manifest",
        "plugin_invalid_script",
        "plugin_not_found",
        "plugin_invalid_input",
        "plugin_invalid_output",
        "plugin_busy",
        "plugin_cancelled",
        "plugin_timed_out",
        "plugin_execution_failed",
        "plugin_network_denied",
        "plugin_network_failed",
        "plugin_network_limit"
    )
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "hostsim"))]
pub(crate) use tests::session as session_for_test;
