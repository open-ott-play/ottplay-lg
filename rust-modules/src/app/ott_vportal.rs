//! VPortal IO adapter. The parser retains folder metadata and page parameters;
//! this runner owns bounded traversal and publishes playable rows incrementally.
use plx_base::task::{time::Deadline, OffFrame};
use plx_data::ott::{
    self, Catalog, CatalogEntry, MediaLocator, PortalRequest, ProviderKind, ProviderProfile,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

const PAGE_LIMIT: usize = 4096;
const DEPTH_LIMIT: usize = 128;
const BATCH_SIZE: usize = 64;
const CANCELLED: &str = "VPortal request cancelled";
type Result<T> = std::result::Result<T, &'static str>;

type Fetch<'a> = dyn FnMut(&str, &Value, &AtomicBool) -> Result<String> + 'a;

fn active(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(CANCELLED)
    } else {
        Ok(())
    }
}

/// A blocking request is bounded to 25 seconds. Its result is discarded if the
/// owner cancelled while libcurl was running. Playback resolution instead hands
/// its lease to the caller for cleanup before that caller checks cancellation.
fn fetch(endpoint: &str, body: &Value, cancel: &AtomicBool) -> Result<String> {
    active(cancel)?;
    if ott::http_url(endpoint).is_none() {
        return Err("Invalid VPortal address");
    }
    let bytes = serde_json::to_vec(body).map_err(|_| "Invalid VPortal request")?;
    if bytes.len() > ott::MAX_REQUEST_BYTES + 2048 {
        return Err("VPortal request is too large");
    }
    let response = plx_net::net::request(
        endpoint,
        &["Content-Type: application/json".into()],
        "POST",
        Some(&bytes),
        plx_net::net::Timeouts {
            connect_s: 8,
            total_s: 25,
            total_ms: 0,
            low_speed_bps: 1,
            low_speed_s: 10,
        },
        false,
        Some(ott::MAX_CATALOG_BYTES),
        None,
    );
    let response = response.ok_or("VPortal request failed or timed out")?;
    if !response.ok() {
        return Err("VPortal rejected the request");
    }
    String::from_utf8(response.body).map_err(|_| "VPortal returned invalid text")
}

fn link(profile: &ProviderProfile) -> Result<ott::PortalLink> {
    profile.validate().map_err(|_| "Invalid VPortal profile")?;
    if profile.kind == ProviderKind::Edem {
        let config=ott::edem::Config::parse(&profile.source).map_err(|_| "Invalid Edem profile")?;
        return ott::parse_vportal_link(&config.portal).map_err(|_| "Configure the Edem VPortal link");
    }
    if profile.kind != ProviderKind::VPortal {
        return Err("Invalid VPortal profile");
    }
    ott::parse_vportal_link(&profile.source).map_err(|_| "Invalid VPortal link")
}

fn request(
    link: &ott::PortalLink,
    params: &Value,
    cancel: &AtomicBool,
    fetch: &mut Fetch<'_>,
) -> Result<String> {
    active(cancel)?;
    let body = link
        .request_body(params)
        .map_err(|_| "Invalid VPortal request")?;
    let text = fetch(&link.endpoint, &body, cancel)?;
    if text.len() > ott::MAX_CATALOG_BYTES {
        return Err("VPortal response is too large");
    }
    Ok(text)
}

struct Path {
    request: Arc<str>,
    parent: Option<Arc<Path>>,
    category: String,
    depth: usize,
}
impl Path {
    fn contains(&self, request: &str) -> bool {
        let mut at = Some(self);
        while let Some(path) = at {
            if path.request.as_ref() == request {
                return true;
            }
            at = path.parent.as_deref();
        }
        false
    }
}
struct Job {
    page: PortalRequest,
    parent: Option<Arc<Path>>,
}

pub(super) fn visit(
    worker: &OffFrame,
    profile: &ProviderProfile,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<()>,
) -> Result<()> {
    visit_with(worker, profile, cancel, emit, &mut fetch)
}

/// The catalogue owner receives typed folders and memberships, including empty folders.
pub(super) fn visit_catalog(
    worker: &OffFrame,
    profile: &ProviderProfile,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Catalog) -> Result<()>,
) -> Result<()> {
    visit_catalog_with_deadline(worker, profile, cancel, emit, None, &mut fetch)
}

pub(super) fn visit_before(
    worker: &OffFrame,
    profile: &ProviderProfile,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<()>,
    deadline: Deadline,
) -> Result<()> {
    visit_with_deadline(worker, profile, cancel, emit, Some(deadline), &mut fetch)
}

fn visit_with(
    worker: &OffFrame,
    profile: &ProviderProfile,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<()>,
    fetch: &mut Fetch<'_>,
) -> Result<()> {
    visit_with_deadline(worker, profile, cancel, emit, None, fetch)
}

fn within_deadline(worker: &OffFrame, deadline: Option<Deadline>) -> Result<()> {
    if deadline.is_some_and(|at| at.expired(worker)) {
        Err("VPortal catalogue lookup timed out")
    } else {
        Ok(())
    }
}

pub(super) fn visit_with_deadline(
    worker: &OffFrame,
    profile: &ProviderProfile,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<()>,
    deadline: Option<Deadline>,
    fetch: &mut Fetch<'_>,
) -> Result<()> {
    visit_catalog_with_deadline(worker, profile, cancel, &mut |batch| {
        if batch.entries.is_empty() { Ok(()) } else { emit(batch.entries) }
    }, deadline, fetch)
}

fn visit_catalog_with_deadline(
    worker: &OffFrame,
    profile: &ProviderProfile,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Catalog) -> Result<()>,
    deadline: Option<Deadline>,
    fetch: &mut Fetch<'_>,
) -> Result<()> {
    let link = link(profile)?;
    let mut root = PortalRequest::root();
    root.request["limit"] = Value::from(300);
    let mut queued_bytes = root.byte_len();
    let mut jobs = vec![Job {
        page: root,
        parent: None,
    }];
    let mut seen = HashSet::new();
    let mut seen_bytes = 0usize;
    let mut rows_seen: HashMap<String, String> = HashMap::new();
    let mut row_bytes = 0usize;
    let mut emitted = false;
    while let Some(mut job) = jobs.pop() {
        active(cancel)?;
        within_deadline(worker, deadline)?;
        queued_bytes = queued_bytes.saturating_sub(job.page.byte_len());
        job.page.request["limit"] = Value::from(300);
        let key = job
            .page
            .identity()
            .map_err(|_| "Invalid VPortal page request")?;
        if job.parent.as_ref().is_some_and(|path| path.contains(&key)) {
            return Err("VPortal catalogue contains a page loop");
        }
        // Folder aliases are legitimate. Only the same request in the same
        // category is redundant; the ancestor check catches recursive aliases.
        let category_id = job.page.category_identity().unwrap_or("").to_owned();
        let identity = (key.clone(), category_id.clone());
        if seen.contains(&identity) {
            continue;
        }
        seen_bytes = seen_bytes.saturating_add(key.len() + category_id.len());
        if seen.len() >= PAGE_LIMIT || seen_bytes > ott::MAX_CATALOG_BYTES {
            return Err("VPortal catalogue page limit reached");
        }
        seen.insert(identity);
        let depth = job.parent.as_ref().map_or(1, |parent| {
            parent.depth + usize::from(parent.category != category_id)
        });
        if depth > DEPTH_LIMIT {
            return Err("VPortal catalogue nesting limit reached");
        }
        let path = Arc::new(Path {
            request: Arc::from(key),
            parent: job.parent,
            category: category_id,
            depth,
        });
        let text = request(&link, &job.page.request, cancel, fetch)?;
        active(cancel)?;
        within_deadline(worker, deadline)?;
        let page = job
            .page
            .parse_response(&text, profile)
            .map_err(|_| "Invalid or incomplete VPortal catalogue")?;
        active(cancel)?;
        let hierarchy = page.catalog.hierarchy;
        // Reject a conflicting page before publishing its folder descriptors.
        // Encoding once also keeps the cross-page identity check off the emit path.
        let mut entries = Vec::with_capacity(page.catalog.entries.len());
        for entry in page.catalog.entries {
            let encoded = serde_json::to_string(&entry)
                .map_err(|_| "Invalid VPortal catalogue row")?;
            if let Some(previous) = rows_seen.get(&entry.id) {
                if previous != &encoded {
                    let previous: CatalogEntry = serde_json::from_str(previous)
                        .map_err(|_| "Invalid VPortal catalogue row")?;
                    // Later pages may refine display metadata, never replace the
                    // source or provider identity already published for this row.
                    if previous.id != entry.id || previous.category != entry.category
                        || previous.provider_id != entry.provider_id
                        || previous.provider_id_field != entry.provider_id_field
                        || previous.media != entry.media || previous.live != entry.live
                    {
                        return Err("VPortal catalogue contains conflicting rows");
                    }
                }
            }
            entries.push((entry, encoded));
        }
        if let Some(graph) = &hierarchy {
            emit(Catalog { entries: Vec::new(), hierarchy: Some(graph.nodes_only()) })?;
            active(cancel)?;
        }
        let mut repeated = Vec::new();
        let mut batch = Vec::with_capacity(BATCH_SIZE);
        for (entry, encoded) in entries {
            let previous_bytes = match rows_seen.get(&entry.id) {
                Some(previous) if previous == &encoded => {
                    repeated.push(entry.id);
                    continue;
                }
                Some(previous) => previous.len(),
                None => 0,
            };
            row_bytes = row_bytes
                .saturating_sub(previous_bytes)
                .saturating_add(encoded.len());
            if row_bytes > ott::MAX_CATALOG_MODEL_BYTES {
                return Err("VPortal catalogue metadata limit reached");
            }
            rows_seen.insert(entry.id.clone(), encoded);
            if rows_seen.len() > ott::MAX_CATALOG_ENTRIES {
                return Err("VPortal catalogue entry limit reached");
            }
            batch.push(entry);
            // A playable first row must not wait for sibling folders or pages.
            if !emitted || batch.len() >= BATCH_SIZE {
                active(cancel)?;
                emit_catalog_rows(emit, std::mem::take(&mut batch), hierarchy.as_ref())?;
                active(cancel)?;
                emitted = true;
            }
        }
        if !batch.is_empty() {
            active(cancel)?;
            emit_catalog_rows(emit, batch, hierarchy.as_ref())?;
            active(cancel)?;
        }
        // A repeated leaf may acquire another structural membership without
        // becoming another row. Its original raw position remains unchanged.
        if let Some(graph) = hierarchy.filter(|_| !repeated.is_empty()) {
            let mut graph = graph.for_entries(repeated.iter().map(String::as_str));
            graph.nodes.clear();
            emit(Catalog { entries: Vec::new(), hierarchy: Some(graph) })?;
            active(cancel)?;
        }
        // Depth-first traversal reaches a playable leaf before fetching the
        // remaining root pages. Reverse push preserves provider ordering.
        for pending in page.pending.into_iter().rev() {
            queued_bytes = queued_bytes.saturating_add(pending.byte_len());
            if jobs.len() >= PAGE_LIMIT || queued_bytes > ott::MAX_CATALOG_BYTES {
                return Err("VPortal catalogue queue limit reached");
            }
            jobs.push(Job {
                page: pending,
                parent: Some(path.clone()),
            });
        }
    }
    active(cancel)
}

fn emit_catalog_rows(
    emit: &mut dyn FnMut(Catalog) -> Result<()>,
    entries: Vec<CatalogEntry>,
    hierarchy: Option<&ott::CategoryHierarchy>,
) -> Result<()> {
    let hierarchy = hierarchy.map(|graph| {
        let mut projected = graph.for_entries(entries.iter().map(|entry| entry.id.as_str()));
        projected.nodes.clear(); // the parent closure was emitted before the first row
        projected
    });
    emit(Catalog { entries, hierarchy })
}

#[path = "ott_vportal_choices.rs"]
mod choices;
pub(super) use choices::{ParsedChoices, Variant};
pub(super) use choices::valid_key as valid_quality_key;
pub(super) fn resolve_choices_typed(profile: &ProviderProfile, entry: &CatalogEntry,
    cancel: &AtomicBool, preferred: Option<&str>, interactive: bool) -> Result<ParsedChoices> {
    choices::resolve(profile, entry, cancel, preferred, interactive, &mut fetch)
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct SessionHints {
    pub stop: String,
    pub heartbeat: Option<String>,
}
impl std::fmt::Debug for SessionHints {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionHints(<private>)")
    }
}

#[derive(PartialEq, Eq)]
pub(super) struct Resolution {
    pub url: String,
    pub session: Option<SessionHints>,
    pub failure: Option<&'static str>,
}
impl std::fmt::Debug for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Resolution(<private>)")
    }
}

/// The caller must adopt `session` before checking its generation/cancel flag.
/// A successful response that arrived after cancellation still owns server
/// resources: dropping the adopted lease releases them without starting video.
pub(super) fn resolve_typed(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    cancel: &AtomicBool,
) -> Result<Resolution> {
    resolve_with(profile, entry, cancel, &mut fetch)
}

fn playable_url(endpoint: &str, value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?;
    if value.trim().is_empty() {
        return None;
    }
    ott::resolve_http_url(endpoint, value)
}

fn resolve_with(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    cancel: &AtomicBool,
    fetch: &mut Fetch<'_>,
) -> Result<Resolution> {
    active(cancel)?;
    let link = link(profile)?;
    match &entry.media {
        MediaLocator::Http { url } => Ok(Resolution {
            url: ott::http_url(url).ok_or("Invalid VPortal playback address")?,
            session: None,
            failure: None,
        }),
        MediaLocator::VPortal { request: params } => {
            let text = request(&link, params, cancel, fetch)?;
            let data: Value =
                serde_json::from_str(&text).map_err(|_| "Invalid VPortal playback response")?;
            if data.get("type").and_then(Value::as_str) == Some("error") {
                return failed_resolution(
                    &link.endpoint,
                    &data,
                    "VPortal could not resolve this video",
                );
            }
            let selected = data
                .get("url")
                .filter(|value| playable_url(&link.endpoint, Some(value)).is_some())
                .or_else(|| {
                    data.get("variants")
                        .and_then(Value::as_object)
                        .and_then(|variants| {
                            variants
                                .values()
                                .find(|value| playable_url(&link.endpoint, Some(value)).is_some())
                        })
                });
            let Some(selected) = selected else {
                return failed_resolution(
                    &link.endpoint,
                    &data,
                    "VPortal returned no playable source",
                );
            };
            let url = playable_url(&link.endpoint, Some(selected))
                .ok_or("Invalid VPortal playback address")?;
            let session =
                session_hints(&link.endpoint, &data, selected.as_str().unwrap_or(""), &url);
            Ok(Resolution {
                url,
                session,
                failure: None,
            })
        }
        _ => Err("Invalid VPortal media locator"),
    }
}

fn failed_resolution(endpoint: &str, data: &Value, message: &'static str) -> Result<Resolution> {
    // A failed playback response may still own the top-level allocated session.
    // Do not adopt arbitrary unselected entries from the variants session map.
    let session = data
        .get("stop")
        .and_then(Value::as_str)
        .and_then(|stop| session_url(endpoint, stop))
        .map(|stop| SessionHints {
            stop,
            heartbeat: None,
        });
    if session.is_none() {
        return Err(message);
    }
    Ok(Resolution {
        url: String::new(),
        session,
        failure: Some(message),
    })
}

fn session_hints(
    endpoint: &str,
    data: &Value,
    selected: &str,
    normalized: &str,
) -> Option<SessionHints> {
    let hints = data
        .get("sessions")
        .and_then(Value::as_object)
        .and_then(|sessions| sessions.get(selected).or_else(|| sessions.get(normalized)))
        .or_else(|| {
            (playable_url(endpoint, data.get("url")).as_deref() == Some(normalized)).then_some(data)
        })?;
    let stop = session_url(endpoint, hints.get("stop")?.as_str()?)?;
    let heartbeat = hints
        .get("heartbeat")
        .and_then(Value::as_str)
        .and_then(|value| session_url(endpoint, value));
    Some(SessionHints { stop, heartbeat })
}

/// Session control exists only for an explicitly selected NAS API origin.
/// The narrow path grammar cannot turn a provider response into an arbitrary GET.
fn session_url(endpoint: &str, value: &str) -> Option<String> {
    if endpoint.contains(['?', '#']) || value.contains(['?', '#']) {
        return None;
    }
    let endpoint = ott::http_url(endpoint)?;
    let origin = plx_net::net::origin::Origin::parse(&endpoint)?.base();
    if !endpoint
        .strip_prefix(&origin)?
        .eq_ignore_ascii_case("/nas/api")
    {
        return None;
    }
    let url = ott::http_url(value)?; // No relative or scheme-relative control URLs.
    let control_origin = plx_net::net::origin::Origin::parse(&url)?.base();
    if !control_origin.eq_ignore_ascii_case(&origin) {
        return None;
    }
    let path = url.strip_prefix(&control_origin)?;
    if !path.get(..12)?.eq_ignore_ascii_case("/nas/stream/") {
        return None;
    }
    let path = &path[12..];
    let (token, file) = path.split_once('/')?;
    let (session, secret) = token.split_once('.')?;
    let token_part = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    };
    if !token_part(session) || !token_part(secret) {
        return None;
    }
    let extension = file
        .get(..6)
        .filter(|prefix| prefix.eq_ignore_ascii_case("media."))
        .map(|_| &file[6..])?;
    if extension.is_empty()
        || extension.len() > 8
        || !extension.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(url)
}

#[cfg(test)]
#[path = "ott_vportal/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "ott_vportal_ancestor_tests.rs"]
mod ancestor_tests;

/// Exercise the real owned playback resolver with a hermetic transport.
#[cfg(all(test, feature = "hostsim"))]
pub(super) fn resolve_choices_with_fetch_for_test(
    profile: &ProviderProfile, entry: &CatalogEntry, cancel: &AtomicBool,
    preferred: Option<&str>, interactive: bool, fetch: &mut Fetch<'_>,
) -> Result<ParsedChoices> {
    choices::resolve(profile, entry, cancel, preferred, interactive, fetch)
}
