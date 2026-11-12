use super::{
    http_url, identity, model_bytes, resolve_http_url, text, Catalog, CatalogEntry, CatalogError,
    MediaLocator, ProviderKind, ProviderProfile, SynopsisSpan, CategoryHierarchy, CategoryMembership, CategoryNode, MAX_CATALOG_BYTES, MAX_CATALOG_ENTRIES,
    MAX_CATALOG_MODEL_BYTES, MAX_METADATA_BYTES, MAX_REQUEST_BYTES,
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, PartialEq)]
pub struct PortalLink {
    pub endpoint: String,
    pub key: String,
}
impl std::fmt::Debug for PortalLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PortalLink(<private>)")
    }
}

/// Decode only the brackets; the opaque key and URL escapes must retain their bytes.
pub fn parse_vportal_link(value: &str) -> Result<PortalLink, CatalogError> {
    let value = value.trim();
    if value.len() > 16384 || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(CatalogError::InvalidUrl);
    }
    let prefix = if value
        .get(..13)
        .is_some_and(|s| s.eq_ignore_ascii_case("portal::[key:"))
    {
        13
    } else if value
        .get(..15)
        .is_some_and(|s| s.eq_ignore_ascii_case("portal::%5bkey:"))
    {
        15
    } else {
        return Err(CatalogError::InvalidUrl);
    };
    let tail = &value[prefix..];
    let lower = tail.to_ascii_lowercase();
    let (close, end_len) = [(lower.find(']'), 1), (lower.find("%5d"), 3)]
        .into_iter()
        .filter_map(|(at, len)| at.map(|at| (at, len)))
        .min_by_key(|(at, _)| *at)
        .ok_or(CatalogError::InvalidUrl)?;
    let key = &tail[..close];
    if key.is_empty() || key.len() > 1024 || key.contains(['[', ']', '<', '>', '"', '\\']) {
        return Err(CatalogError::InvalidUrl);
    }
    let endpoint = http_url(&tail[close + end_len..]).ok_or(CatalogError::InvalidUrl)?;
    if tail[close + end_len..].contains('#') {
        return Err(CatalogError::InvalidUrl);
    }
    Ok(PortalLink {
        endpoint,
        key: key.into(),
    })
}

impl PortalLink {
    /// Validate untrusted locator parameters before adding installation credentials.
    pub fn request_body(&self, params: &Value) -> Result<Value, CatalogError> {
        let mut body = request(params)?
            .as_object()
            .cloned()
            .ok_or(CatalogError::InvalidFormat)?;
        body.insert("app".into(), Value::String("ott-play".into()));
        body.insert("key".into(), Value::String(self.key.clone()));
        Ok(Value::Object(body))
    }
}

/// Metadata that crosses a page request. Never sent back to the provider.
#[derive(Clone, Default, PartialEq)]
struct PortalContext {
    protection: Option<bool>,
    title: String,
    year: String,
    duration: String,
    age: String,
    description: String,
    landscape: String,
    poster: String,
    series: bool,
    ancestry: Vec<CategoryNode>,
}

impl PortalContext {
    fn from_item(item: &Value, parent: &Self, nested: bool, base: &str) -> Result<Self, CatalogError> {
        let value = |key: &str, fallback: &str| {
            item.get(key)
                .and_then(scalar_text)
                .filter(|s| !s.trim().is_empty())
                .map(|s| text(&s, MAX_METADATA_BYTES))
                .unwrap_or_else(|| fallback.to_owned())
        };
        let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
        // Decode only an item's own prose. Parent context is already normalized:
        // inherited escaped examples must not be decoded/stripped a second time.
        let prose = |key: &str, fallback: &str| -> Result<String, CatalogError> {
            item.get(key).and_then(scalar_text).filter(|s| !s.trim().is_empty())
                .map_or_else(|| Ok(fallback.to_owned()), |s| super::vportal_text::plain_text(&s))
        };
        let own_title = value("title", &parent.title);
        let title = if !nested && parent.series && !parent.title.is_empty() {
            parent.title.clone()
        } else if nested
            && parent.series
            && !parent.title.is_empty()
            && !own_title.is_empty()
            && own_title != parent.title
        {
            text(&format!("{} - {}", parent.title, own_title), 1024)
        } else {
            text(&own_title, 1024)
        };
        let own_landscape = item
            .get("imglr")
            .and_then(Value::as_str)
            .and_then(|v| nonempty_url(base, v));
        let own_poster = item
            .get("img")
            .and_then(Value::as_str)
            .and_then(|v| nonempty_url(base, v));
        let has_own_art = own_landscape.is_some() || own_poster.is_some();
        Ok(Self {
            protection: super::protection::inherit(parent.protection, super::protection::advertised(item)),
            title,
            year: prose("year", &parent.year)?,
            duration: prose("duration", &parent.duration)?,
            age: prose("agelimit", &parent.age)?,
            description: prose("description", &parent.description)?,
            landscape: own_landscape.unwrap_or_else(|| {
                if has_own_art {
                    String::new()
                } else {
                    parent.landscape.clone()
                }
            }),
            poster: own_poster.unwrap_or_else(|| {
                if has_own_art {
                    String::new()
                } else {
                    parent.poster.clone()
                }
            }),
            series: kind == "multistream" || parent.series,
            ancestry: parent.ancestry.clone(),
        })
    }
    fn byte_len(&self) -> usize {
        self.title.len()
            + self.year.len()
            + self.duration.len()
            + self.age.len()
            + self.description.len()
            + self.landscape.len()
            + self.poster.len()
            + self.ancestry.iter().map(|node| node.id.len() + node.label.len()
                + node.parent.as_ref().map_or(0, String::len) + node.anonymous_fingerprint.as_ref().map_or(0, String::len)
                + std::mem::size_of::<CategoryNode>()).sum::<usize>()
    }
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[derive(Clone, PartialEq)]
pub struct PortalRequest {
    pub request: Value,
    pub category: String,
    context: PortalContext,
}
impl std::fmt::Debug for PortalRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortalRequest")
            .field("category", &self.category)
            .finish_non_exhaustive()
    }
}
impl PortalRequest {
    pub fn root() -> Self {
        Self {
            request: Value::Object(Map::new()),
            category: String::new(),
            context: PortalContext::default(),
        }
    }
    /// The captured structural container survives response wrappers and paging.
    pub fn category_identity(&self) -> Option<&str> {
        self.context.ancestry.last().map(|node| node.id.as_str())
    }
    /// Canonicalizes object key order even when serde_json uses preserve_order.
    pub fn identity(&self) -> Result<String, CatalogError> {
        Ok(request(&self.request)?.to_string())
    }
    /// The runner uses this to bound retained requests and inherited metadata together.
    pub fn byte_len(&self) -> usize {
        self.request.to_string().len() + self.category.len() + self.context.byte_len()
    }
    pub fn parse_response(
        &self,
        input: &str,
        profile: &ProviderProfile,
    ) -> Result<PortalPage, CatalogError> {
        parse_page(input, profile, &self.category, &self.context, &self.request)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PortalPage {
    pub catalog: Catalog,
    pub pending: Vec<PortalRequest>,
}

pub fn parse_vportal_response(
    input: &str,
    profile: &ProviderProfile,
    category: &str,
) -> Result<PortalPage, CatalogError> {
    parse_page(
        input,
        profile,
        category,
        &PortalContext::default(),
        &Value::Object(Map::new()),
    )
}

#[derive(Default)]
struct PageBudget {
    pending: usize,
    entries: usize,
    graph_bytes: usize,
    rows: BTreeMap<String, usize>,
}

fn parse_page(
    input: &str,
    profile: &ProviderProfile,
    category: &str,
    context: &PortalContext,
    params: &Value,
) -> Result<PortalPage, CatalogError> {
    profile.validate()?;
    if profile.kind != ProviderKind::VPortal {
        return Err(CatalogError::InvalidProfile);
    }
    if input.len() > MAX_CATALOG_BYTES {
        return Err(CatalogError::LimitExceeded);
    }
    let data: Value = serde_json::from_str(input).map_err(|_| CatalogError::InvalidFormat)?;
    let link = parse_vportal_link(&profile.source)?;
    let mut page = PortalPage::default();
    let mut context = context.clone();
    // The legacy parser entry point may supply an opaque existing category.
    // It is one segment: never infer a hierarchy from its display separators.
    if context.ancestry.is_empty() && !category.is_empty() {
        context.ancestry.push(CategoryNode { id: folder_hash(&profile.id, None, "legacy", &Value::String(category.into())),
            parent: None, label: category.into(), anonymous_fingerprint: None, source_identity: None });
    }
    if !context.ancestry.is_empty() {
        page.catalog.hierarchy = Some(CategoryHierarchy { nodes: context.ancestry.clone(), memberships: Vec::new() });
    }
    let mut seen = BTreeSet::new();
    walk(
        &data,
        &context,
        category,
        profile,
        &link.endpoint,
        params,
        0,
        &mut page,
        &mut seen,
        &mut PageBudget::default(),
    )?;
    let mut catalog = Catalog::default();
    catalog.merge(page.catalog)?;
    page.catalog = catalog;
    Ok(page)
}

#[allow(clippy::too_many_arguments)]
fn walk(
    item: &Value,
    parent: &PortalContext,
    category: &str,
    profile: &ProviderProfile,
    base: &str,
    params: &Value,
    depth: usize,
    page: &mut PortalPage,
    seen: &mut BTreeSet<String>,
    budget: &mut PageBudget,
) -> Result<(), CatalogError> {
    if depth > 32 || page.catalog.entries.len() + page.pending.len() > MAX_CATALOG_ENTRIES {
        return Err(CatalogError::LimitExceeded);
    }
    if !item.is_object() {
        return Err(CatalogError::InvalidFormat);
    }
    if item.get("truncated").and_then(Value::as_bool) == Some(true) {
        return Err(CatalogError::Truncated);
    }
    for field in [
        "description",
        "year",
        "duration",
        "agelimit",
        "img",
        "imglr",
    ] {
        if item
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|s| s.len() > MAX_METADATA_BYTES)
        {
            return Err(CatalogError::LimitExceeded);
        }
    }
    let kind = item
        .get("type")
        .and_then(Value::as_str)
        .ok_or(CatalogError::InvalidFormat)?;
    let title = text(
        item.get("title").and_then(Value::as_str).unwrap_or(""),
        1024,
    );
    match kind {
        "stream" => {
            // An auth-only echo is no semantic locator. Keep a direct URL's
            // identity distinct instead of assigning every empty request "{}".
            let params = item.get("request").map(response_request).transpose()?
                .filter(|p| p.as_object().is_some_and(|o| !o.is_empty()));
            // A request resolves a fresh source at play time; a catalog URL can expire.
            let media = if let Some(params) = params.as_ref() {
                MediaLocator::VPortal {
                    request: params.clone(),
                }
            } else if let Some(url) = item
                .get("url")
                .and_then(Value::as_str)
                .and_then(|url| nonempty_url(base, url))
            {
                MediaLocator::Http { url }
            } else {
                return Err(CatalogError::InvalidFormat);
            };
            let own_id = |source: &Value| {
                ["id", "fid", "stream_id", "itemId", "media_id", "episode_id"]
                    .iter()
                    .find_map(|key| {
                        source
                            .get(*key)
                            .and_then(identifier)
                            .map(|value| ((*key).to_string(), value))
                    })
            };
            let provider_id_pair = own_id(item).or_else(|| item.get("request").and_then(own_id));
            let (provider_id_field, provider_id) = provider_id_pair
                .map(|(field, id)| (Some(field), Some(id)))
                .unwrap_or((None, None));
            let key = if let (Some(field), Some(id)) = (&provider_id_field, &provider_id) {
                format!("{field}:{id}")
            } else if let Some(params) = params {
                params.to_string()
            } else {
                serde_json::to_string(&media).map_err(|_| CatalogError::InvalidFormat)?
            };
            let metadata = PortalContext::from_item(item, parent, false, base)?;
            let mut description = [
                &metadata.year,
                &metadata.duration,
                &metadata.age,
            ]
            .into_iter()
            .filter(|s| !s.is_empty())
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n");
            if !description.is_empty() && !metadata.description.is_empty() { description.push('\n'); }
            let synopsis_start = description.len();
            description.push_str(&metadata.description);
            if description.len() > MAX_METADATA_BYTES {
                return Err(CatalogError::LimitExceeded);
            }
            let synopsis = SynopsisSpan::new(&description, synopsis_start);
            // Prefer an item's portrait to inherited fanart when it supplies its own artwork.
            let poster = [
                item.get("imglr").and_then(Value::as_str),
                item.get("img").and_then(Value::as_str),
                Some(parent.landscape.as_str()),
                Some(parent.poster.as_str()),
            ]
            .into_iter()
            .flatten()
            .find_map(|value| nonempty_url(base, value))
            .unwrap_or_default();
            let title = if title.is_empty() {
                if parent.title.is_empty() {
                    "Video".into()
                } else {
                    parent.title.clone()
                }
            } else {
                title
            };
            let title = if parent.series && !parent.title.is_empty() && parent.title != title {
                text(&format!("{} - {title}", parent.title), 1024)
            } else {
                title
            };
            let duration_ms = item
                .get("duration_ms")
                .and_then(Value::as_u64)
                .or_else(|| explicit_duration_ms(&metadata.duration));
            // The same provider leaf may be intentionally present in two folder categories.
            // Its provider ID remains identical for portable Favorites; the view row is scoped.
            let entry = CatalogEntry { protection: metadata.protection,
                id: identity(&profile.id, &format!("{category}\0{key}")),
                title,
                category: text(category, 4096),
                description,
                synopsis,
                poster,
                media,
                duration_ms,
                provider_id,
                provider_id_field,
                live: None,
            };
            let entry_id = entry.id.clone();
            if let Some(&index) = budget.rows.get(&entry.id) {
                if page.catalog.entries[index] != entry { return Err(CatalogError::InvalidFormat); }
            } else {
            budget.entries = budget
                .entries
                .checked_add(model_bytes(&entry))
                .ok_or(CatalogError::LimitExceeded)?;
            if budget.entries.saturating_add(budget.graph_bytes) > MAX_CATALOG_MODEL_BYTES {
                return Err(CatalogError::LimitExceeded);
            }
            budget.rows.insert(entry.id.clone(), page.catalog.entries.len());
            page.catalog.entries.push(entry);
            }
            if let Some(folder) = parent.ancestry.last() {
                let hierarchy = page.catalog.hierarchy.get_or_insert_with(CategoryHierarchy::default);
                if hierarchy.memberships.len() >= MAX_CATALOG_ENTRIES { return Err(CatalogError::LimitExceeded); }
                budget.graph_bytes = budget.graph_bytes.checked_add(std::mem::size_of::<CategoryMembership>() + entry_id.len() + folder.id.len())
                    .ok_or(CatalogError::LimitExceeded)?;
                if budget.entries.saturating_add(budget.graph_bytes) > MAX_CATALOG_MODEL_BYTES { return Err(CatalogError::LimitExceeded); }
                hierarchy.memberships.push(CategoryMembership { entry_id, category_id: folder.id.clone() });
            }
        }
        // A next item needs its container's record count to synthesize an offset.
        "next" => return Err(CatalogError::InvalidFormat),
        "videoportal" | "category" | "multistream" => {
            let mut context = PortalContext::from_item(item, parent, depth > 0, base)?;
            let child_category = if depth == 0 || kind == "videoportal" || title.is_empty() {
                category.to_string()
            } else if category.is_empty() {
                title.clone()
            } else {
                format!("{category} / {title}")
            };
            if child_category.len() > 4096 {
                return Err(CatalogError::LimitExceeded);
            }
            if depth > 0 && kind != "videoportal" && !title.is_empty() {
                let parent_id = parent.ancestry.last().map(|node| node.id.as_str());
                let (seed, anonymous) = folder_seed(item, &title)?;
                let id = folder_hash(&profile.id, parent_id, kind, &seed);
                let anonymous_fingerprint = anonymous.then(|| child_fingerprint(item.get("items").unwrap_or(&Value::Null)));
                let node = CategoryNode { id, parent: parent_id.map(str::to_owned), label: title.clone(), anonymous_fingerprint, source_identity: None };
                context.ancestry.push(node.clone());
                let hierarchy = page.catalog.hierarchy.get_or_insert_with(CategoryHierarchy::default);
                if hierarchy.nodes.len() >= MAX_CATALOG_ENTRIES { return Err(CatalogError::LimitExceeded); }
                budget.graph_bytes = budget.graph_bytes.checked_add(std::mem::size_of::<CategoryNode>() + node.id.len() + node.label.len()
                    + node.parent.as_ref().map_or(0, String::len) + node.anonymous_fingerprint.as_ref().map_or(0, String::len)).ok_or(CatalogError::LimitExceeded)?;
                if budget.entries.saturating_add(budget.graph_bytes) > MAX_CATALOG_MODEL_BYTES { return Err(CatalogError::LimitExceeded); }
                hierarchy.nodes.push(node);
            }
            if let Some(items) = item.get("items").and_then(Value::as_array) {
                let current = item
                    .get("request")
                    .map(response_request)
                    .transpose()?
                    .unwrap_or_else(|| params.clone());
                offset(&current)?;
                let count = items
                    .iter()
                    .filter(|v| v.get("type").and_then(Value::as_str) != Some("next"))
                    .count();
                let mut has_next = false;
                for child in items
                    .iter()
                    .filter(|child| child.get("type").and_then(Value::as_str) != Some("next"))
                {
                    walk(
                        child,
                        &context,
                        &child_category,
                        profile,
                        base,
                        &current,
                        depth + 1,
                        page,
                        seen,
                        budget,
                    )?;
                }
                // Finish the current folder's playable leaves before requesting another page.
                for child in items
                    .iter()
                    .filter(|child| child.get("type").and_then(Value::as_str) == Some("next"))
                {
                    let next = next_request(&current, child.get("request"), count)?;
                    enqueue(page, seen, next, &child_category, &context, budget)?;
                    has_next = true;
                }
                let more = item.get("has_more").and_then(Value::as_bool) == Some(true)
                    || item.get("hasMore").and_then(Value::as_bool) == Some(true)
                    || item
                        .get("total")
                        .and_then(Value::as_u64)
                        .is_some_and(|total| {
                            total > offset(&current).unwrap_or(0).saturating_add(count as u64)
                        });
                if more && !has_next {
                    return Err(CatalogError::Truncated);
                }
            } else if let Some(params) = item.get("request") {
                enqueue(
                    page,
                    seen,
                    response_request(params)?,
                    &child_category,
                    &context,
                    budget,
                )?;
            } else {
                return Err(CatalogError::InvalidFormat);
            }
        }
        _ => return Err(CatalogError::InvalidFormat),
    }
    if page.catalog.entries.len() + page.pending.len() > MAX_CATALOG_ENTRIES {
        return Err(CatalogError::LimitExceeded);
    }
    Ok(())
}

fn enqueue(
    page: &mut PortalPage,
    seen: &mut BTreeSet<String>,
    request: Value,
    category: &str,
    context: &PortalContext,
    budget: &mut PageBudget,
) -> Result<(), CatalogError> {
    let key = format!("{}\0{}", request, context.ancestry.last().map(|node| node.id.as_str()).unwrap_or(""));
    if seen.insert(key) {
        let pending = PortalRequest {
            request,
            category: category.into(),
            context: context.clone(),
        };
        budget.pending = budget
            .pending
            .checked_add(pending.byte_len())
            .ok_or(CatalogError::LimitExceeded)?;
        if budget.pending > MAX_CATALOG_BYTES {
            return Err(CatalogError::LimitExceeded);
        }
        page.pending.push(pending);
    }
    Ok(())
}


fn child_fingerprint(value: &Value) -> String {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(object) => {
                let mut fields: Vec<_> = object.iter().collect();
                fields.sort_by(|(a, _), (b, _)| a.cmp(b));
                Value::Object(fields.into_iter().map(|(key, value)| (key.clone(), canonical(value))).collect())
            }
            Value::Array(array) => Value::Array(array.iter().map(canonical).collect()),
            value => value.clone(),
        }
    }
    super::favorites_wire::hash(canonical(value).to_string().as_bytes())
}

fn folder_hash(profile: &str, parent: Option<&str>, kind: &str, seed: &Value) -> String {
    let encoded = serde_json::json!(["vportal-category-v1", profile, parent, kind, seed]);
    format!("vportal:category:{}", super::favorites_wire::hash(encoded.to_string().as_bytes()))
}

fn folder_seed(item: &Value, title: &str) -> Result<(Value, bool), CatalogError> {
    for field in ["id", "fid", "folder_id", "category_id", "itemId"] {
        if let Some(id) = item.get(field).and_then(identifier) {
            return Ok((serde_json::json!(["provider-id", field, id]), false));
        }
    }
    if let Some(value) = item.get("request") {
        let value = response_request(value)?;
        if value.as_object().is_some_and(|value| !value.is_empty()) {
            return Ok((serde_json::json!(["request", value, title]), false));
        }
    }
    // No provider identity exists: unchanged raw source names can be stable,
    // but renames and indistinguishable duplicate titles cannot be guessed.
    Ok((serde_json::json!(["source-title", title]), true))
}

fn nonempty_url(base: &str, value: &str) -> Option<String> {
    if value.trim().is_empty() {
        return None;
    }
    resolve_http_url(base, value)
}

fn offset(params: &Value) -> Result<u64, CatalogError> {
    params
        .get("offset")
        .map(|v| v.as_u64().ok_or(CatalogError::InvalidFormat))
        .unwrap_or(Ok(0))
}

fn next_request(
    current: &Value,
    supplied: Option<&Value>,
    count: usize,
) -> Result<Value, CatalogError> {
    let mut page = request(current)?
        .as_object()
        .cloned()
        .ok_or(CatalogError::InvalidFormat)?;
    let supplied = supplied
        .map(response_request)
        .transpose()?
        .unwrap_or_else(|| Value::Object(Map::new()));
    let supplied = supplied.as_object().ok_or(CatalogError::InvalidFormat)?;
    let before = offset(current)?;
    for (key, value) in supplied {
        page.insert(key.clone(), value.clone());
    }
    if !supplied.contains_key("offset") {
        page.insert(
            "offset".into(),
            Value::from(
                before
                    .checked_add(count as u64)
                    .ok_or(CatalogError::LimitExceeded)?,
            ),
        );
    }
    let page = Value::Object(page);
    let forward = offset(&page)? > before;
    let changed_cursor = supplied
        .iter()
        .any(|(key, value)| key != "offset" && current.get(key) != Some(value));
    if !forward && !changed_cursor {
        return Err(CatalogError::Truncated);
    }
    request(&page)
}

/// Only colon-separated clock notation has an unambiguous unit. Bare numbers and
/// provider-specific prose remain display metadata; they are not guessed as seconds.
fn explicit_duration_ms(value: &str) -> Option<u64> {
    let parts: Vec<_> = value.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let mut seconds = 0u64;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n: u64 = part.parse().ok()?;
        if index > 0 && n >= 60 {
            return None;
        }
        seconds = seconds.checked_mul(60)?.checked_add(n)?;
    }
    seconds.checked_mul(1000)
}

fn identifier(value: &Value) -> Option<String> {
    let id = value.as_str().map(str::to_owned).or_else(|| {
        value
            .as_u64()
            .filter(|n| *n <= 9_007_199_254_740_991)
            .map(|n| n.to_string())
    })?;
    (id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && id.len() <= 256
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-')))
    .then_some(id)
}
/// A portal may echo transport credentials in a returned continuation. They are
/// not continuation authority: discard only the top-level echo, then apply the
/// same recursive validation as every retained/outgoing request. Keep the input
/// byte bound before removal so an oversized echo cannot evade it.
fn response_request(value: &Value) -> Result<Value, CatalogError> {
    if value.to_string().len() > MAX_REQUEST_BYTES {
        return Err(CatalogError::InvalidFormat);
    }
    let mut params = value.as_object().cloned().ok_or(CatalogError::InvalidFormat)?;
    params.remove("app");
    params.remove("key");
    request(&Value::Object(params))
}

fn request(value: &Value) -> Result<Value, CatalogError> {
    if !value.is_object() || value.to_string().len() > MAX_REQUEST_BYTES {
        return Err(CatalogError::InvalidFormat);
    }
    fn clean(value: &Value, depth: usize) -> Result<Value, CatalogError> {
        if depth > 16 {
            return Err(CatalogError::LimitExceeded);
        }
        Ok(match value {
            Value::Object(object) => {
                let mut result = Map::new();
                let mut fields: Vec<_> = object.iter().collect();
                fields.sort_by(|(a, _), (b, _)| a.cmp(b));
                for (key, value) in fields {
                    if matches!(
                        key.as_str(),
                        "key" | "app" | "__proto__" | "constructor" | "prototype"
                    ) {
                        return Err(CatalogError::InvalidFormat);
                    }
                    result.insert(key.clone(), clean(value, depth + 1)?);
                }
                Value::Object(result)
            }
            Value::Array(array) => Value::Array(
                array
                    .iter()
                    .map(|v| clean(v, depth + 1))
                    .collect::<Result<_, _>>()?,
            ),
            _ => value.clone(),
        })
    }
    clean(value, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile() -> ProviderProfile {
        ProviderProfile {
            id: "portal-home".into(),
            name: "Home".into(),
            kind: ProviderKind::VPortal,
            source: "portal::[key:test]https://portal.invalid/api".into(),
            source_alias: String::new(),
            enabled: true,
        }
    }
    #[test]
    fn synopsis_normalizes_markup_retains_every_paragraph_and_does_not_strip_title_prose() {
        let synopsis = format!("<b>Название</b> начинается в 2026 году.<br>Слова &amp; &#x442;&#1077;&#1082;&#1089;&#1090;.<p>{}</p><p>ПОЛНЫЙ_КОНЕЦ</p>",
            "Длинное описание. ".repeat(300));
        let body = serde_json::json!({"type":"stream", "title":"Название", "id":"film1", "url":"/one",
            "year":"<b>2026</b>", "duration":"90 мин", "agelimit":"16+", "description":synopsis});
        let page = parse_vportal_response(&body.to_string(), &profile(), "").unwrap();
        let row = &page.catalog.entries[0];
        assert!(row.description.starts_with("2026\n90 мин\n16+\nНазвание начинается в 2026 году.\nСлова & текст.\n\n"));
        assert!(row.description.ends_with("\n\nПОЛНЫЙ_КОНЕЦ"));
        assert_eq!(row.description.matches("Длинное описание.").count(), 300);
        let span = row.synopsis.as_ref().unwrap().validated_for(&row.description).unwrap();
        assert!(span.get(&row.description).unwrap().starts_with("Название начинается в 2026 году."));
        assert_eq!(serde_json::from_str::<CatalogEntry>(&serde_json::to_string(row).unwrap()).unwrap(), *row);
        let mut plain = body.clone();
        plain["description"] = "different metadata".into();
        assert_eq!(parse_vportal_response(&plain.to_string(), &profile(), "").unwrap().catalog.entries[0].id, row.id,
            "metadata normalization does not change media identity");
    }

    #[test]
    fn inherited_markup_decodes_once_and_keeps_http_poster_paths_and_complete_text() {
        let root = PortalRequest::root().parse_response(r#"{"type":"videoportal","items":[{"type":"multistream","title":"Series","year":2026,"duration":"45 мин","description":"<p>История &amp; рассказ.</p><p>&lt;b&gt;literal&lt;/b&gt; &amp;lt;b&amp;gt;escaped&amp;lt;/b&amp;gt;\nПоследний абзац.</p>","img":"/posters/h/series.jpg","imglr":"/posters/l/series.jpg","request":{"id":"series"}}]}"#, &profile()).unwrap();
        let page = root.pending[0].parse_response(r#"{"type":"multistream","items":[{"type":"stream","title":"One","request":{"id":"one"}},{"type":"next","request":{}}]}"#, &profile()).unwrap();
        let first = &page.catalog.entries[0];
        assert_eq!(first.poster, "https://portal.invalid:443/posters/l/series.jpg");
        assert_eq!(first.description, "2026\n45 мин\nИстория & рассказ.\n\nliteral &lt;b&gt;escaped&lt;/b&gt;\nПоследний абзац.");
        let next = page.pending[0].parse_response(r#"{"type":"multistream","items":[{"type":"stream","title":"Two","img":"/posters/h/two.jpg","request":{"id":"two"}}]}"#, &profile()).unwrap();
        let second = &next.catalog.entries[0];
        assert_eq!(second.poster, "https://portal.invalid:443/posters/h/two.jpg");
        assert_eq!(second.description, first.description);
        for row in [first, second] {
            let span = row.synopsis.as_ref().unwrap().validated_for(&row.description).unwrap();
            assert_eq!(span.get(&row.description), Some("История & рассказ.\n\nliteral &lt;b&gt;escaped&lt;/b&gt;\nПоследний абзац."));
        }
    }

    #[test]
    fn facts_without_provider_synopsis_remain_full_description_and_oversize_is_rejected() {
        let mut body = serde_json::json!({"type":"stream", "title":"Year 2026", "url":"/one", "year":2026, "duration":"90 min"});
        let page = parse_vportal_response(&body.to_string(), &profile(), "").unwrap();
        let row = &page.catalog.entries[0];
        assert_eq!(row.description, "2026\n90 min");
        assert_eq!(row.synopsis.as_ref().unwrap().validated_for(&row.description).unwrap().get(&row.description), Some(""));
        body["description"] = "я".repeat(MAX_METADATA_BYTES / 2).into();
        assert_eq!(parse_vportal_response(&body.to_string(), &profile(), ""), Err(CatalogError::LimitExceeded),
            "facts plus a full admitted synopsis must fit; never trim the synopsis");
        body["description"] = "я".repeat(MAX_METADATA_BYTES / 2 + 1).into();
        assert_eq!(parse_vportal_response(&body.to_string(), &profile(), ""), Err(CatalogError::LimitExceeded));
    }
    #[test]
    fn link_keeps_opaque_key_and_url_escapes() {
        let parsed =
            parse_vportal_link("portal::[key:a%20b]https://portal.invalid/api?q=%2f").unwrap();
        assert_eq!(parsed.key, "a%20b");
        assert!(parsed.endpoint.ends_with("?q=%2f"));
        assert!(parse_vportal_link("portal::%5Bkey:x%5Dhttps://portal.invalid/api").is_ok());
        assert!(parse_vportal_link("portal::[key:x]https://user:pass@portal.invalid").is_err());
    }
    #[test]
    fn inline_folders_flatten_and_paged_folders_become_jobs() {
        let page=parse_vportal_response(r#"{"type":"videoportal","items":[{"type":"category","title":"Series","items":[{"type":"stream","title":"One","id":12,"url":"/play/12","description":"First. Second.","imglr":"/12.jpg"}]},{"type":"category","title":"Movies","request":{"section":"movies"}},{"type":"next","request":{"offset":100}}]}"#,&profile(),"").unwrap();
        assert_eq!(page.catalog.entries.len(), 1);
        assert_eq!(page.catalog.entries[0].category, "Series");
        assert_eq!(page.catalog.entries[0].description, "First. Second.");
        assert_eq!(
            page.catalog.entries[0].poster,
            "https://portal.invalid:443/12.jpg"
        );
        assert_eq!(page.pending.len(), 2);
        assert_eq!(page.pending[0].category, "Movies");
    }
    #[test]
    fn truncated_and_credential_injection_responses_are_refused() {
        assert_eq!(
            parse_vportal_response(
                r#"{"type":"category","items":[],"has_more":true}"#,
                &profile(),
                ""
            ),
            Err(CatalogError::Truncated)
        );
        assert_eq!(
            parse_vportal_response(
                r#"{"type":"stream","title":"X","request":{"key":"stolen"}}"#,
                &profile(),
                ""
            ),
            Err(CatalogError::InvalidFormat)
        );
    }
    #[test]
    fn lazy_series_inherit_full_description_and_nearest_art_on_every_page() {
        let p = profile();
        let page = PortalRequest::root().parse_response(r#"{"type":"videoportal","imglr":"/global.jpg","items":[{"type":"multistream","title":"Series","description":"First sentence. Second sentence.\nLast paragraph.","img":"/series.jpg","year":2026,"request":{"cmd":"show","id":8}}]}"#, &p).unwrap();
        let series = &page.pending[0];
        assert_eq!(series.category, "Series");
        let episodes = series.parse_response(r#"{"type":"multistream","items":[{"type":"stream","title":"Episode 1","request":{"episode_id":1}},{"type":"next","request":{}}]}"#, &p).unwrap();
        let row = &episodes.catalog.entries[0];
        assert_eq!(row.title, "Series - Episode 1");
        assert_eq!(row.poster, "https://portal.invalid:443/series.jpg");
        assert_eq!(
            row.description,
            "2026\nFirst sentence. Second sentence.\nLast paragraph."
        );
        assert_eq!(
            episodes.pending[0].request,
            serde_json::json!({"cmd":"show","id":8,"offset":1})
        );
        let final_page = episodes.pending[0].parse_response(r#"{"type":"multistream","items":[{"type":"stream","title":"Episode 2","request":{"episode_id":2}}]}"#, &p).unwrap();
        assert_eq!(final_page.catalog.entries[0].poster, row.poster);
        assert_eq!(final_page.catalog.entries[0].description, row.description);
        assert_eq!(final_page.catalog.entries[0].category, "Series");
    }
    #[test]
    fn pagination_never_silently_claims_complete_or_loses_folder() {
        let mut root = PortalRequest::root();
        root.request = serde_json::json!({"cmd":"folder","id":"abc","offset":3,"limit":300});
        let page = root.parse_response(r#"{"type":"category","items":[{"type":"stream","title":"One","url":"/one"},{"type":"next","request":{"offset":4}}]}"#, &profile()).unwrap();
        assert_eq!(
            page.pending[0].request,
            serde_json::json!({"cmd":"folder","id":"abc","offset":4,"limit":300})
        );
        for response in [
            r#"{"type":"category","items":[],"total":4}"#,
            r#"{"type":"category","items":[],"has_more":false,"hasMore":true}"#,
            r#"{"type":"category","items":[{"type":"next","request":{"offset":3}}]}"#,
        ] {
            assert_eq!(
                root.parse_response(response, &profile()),
                Err(CatalogError::Truncated)
            );
        }
    }
    #[test]
    fn art_fallback_and_request_resolution_do_not_reuse_expired_urls() {
        let page = parse_vportal_response(r#"{"type":"category","items":[{"type":"stream","title":"A","url":"/expired","request":{"id":1},"imglr":"javascript:bad","img":"/poster.jpg"},{"type":"stream","title":"B","url":"/play"}]}"#, &profile(), "").unwrap();
        assert!(matches!(
            page.catalog.entries[0].media,
            MediaLocator::VPortal { .. }
        ));
        assert_eq!(
            page.catalog.entries[0].poster,
            "https://portal.invalid:443/poster.jpg"
        );
        assert!(page.catalog.entries[1].poster.is_empty());
    }
    #[test]
    fn identity_preserves_id_field_and_folder_aliases() {
        let page = parse_vportal_response(r#"{"type":"videoportal","items":[{"type":"category","title":"A","items":[{"type":"stream","title":"One","id":1,"url":"/one"},{"type":"stream","title":"Two","fid":1,"url":"/two"}]},{"type":"category","title":"B","items":[{"type":"stream","title":"One","id":1,"url":"/one"}]}]}"#, &profile(), "").unwrap();
        assert_eq!(page.catalog.entries.len(), 3);
        assert_ne!(page.catalog.entries[0].id, page.catalog.entries[1].id);
        assert_ne!(page.catalog.entries[0].id, page.catalog.entries[2].id);
        assert_eq!(
            page.catalog.entries[0].provider_id,
            page.catalog.entries[2].provider_id
        );
    }
    #[test]
    fn duration_is_not_guessed_and_request_credentials_cannot_be_overridden() {
        assert_eq!(explicit_duration_ms("01:30"), Some(90_000));
        assert_eq!(explicit_duration_ms("1:02:03"), Some(3_723_000));
        for duration in ["90", "90 minutes", "1:99", "-1:30"] {
            assert_eq!(explicit_duration_ms(duration), None);
        }
        let link = parse_vportal_link(&profile().source).unwrap();
        let body = link.request_body(&serde_json::json!({"id":3})).unwrap();
        assert_eq!(body["app"], "ott-play");
        assert_eq!(body["key"], "test");
        assert!(link
            .request_body(&serde_json::json!({"nested":{"key":"bad"}}))
            .is_err());
        assert!(parse_vportal_link("PORTAL::%5Bkey:opaque]https://portal.invalid/api").is_ok());
        assert!(parse_vportal_link("portal::[key:opaque%5Dhttps://portal.invalid/api").is_ok());
    }
    #[test]
    fn fetched_page_cannot_replace_series_title_or_choose_request_id_over_item_id() {
        let root = PortalRequest::root().parse_response(r#"{"type":"videoportal","items":[{"type":"multistream","title":"Show Season 1","request":{"id":4}}]}"#, &profile()).unwrap();
        let page = root.pending[0].parse_response(r#"{"type":"category","title":"Streams","items":[{"type":"stream","title":"Episode 1","fid":12,"request":{"id":5}}]}"#, &profile()).unwrap();
        assert_eq!(page.catalog.entries[0].title, "Show Season 1 - Episode 1");
        assert_eq!(
            page.catalog.entries[0].provider_id_field.as_deref(),
            Some("fid")
        );
        assert_eq!(page.catalog.entries[0].provider_id.as_deref(), Some("12"));
    }

    #[test]
    fn root_key_only_echo_keeps_anonymous_empty_categories_valid() {
        // Match the observed root structure without any private values: eleven
        // categories and a request containing only the transport key echo.
        let items: Vec<Value> = (0..11).map(|index| serde_json::json!({
            "type":"category", "title":format!("Synthetic folder {index}"), "items":[]
        })).collect();
        let clean = serde_json::json!({"type":"videoportal","items":items});
        let mut echoed = clean.clone();
        echoed["request"] = serde_json::json!({"key":"synthetic-root-echo"});
        let expected = PortalRequest::root().parse_response(&clean.to_string(), &profile()).unwrap();
        let got = PortalRequest::root().parse_response(&echoed.to_string(), &profile()).unwrap();
        assert_eq!(got, expected);
        assert!(got.catalog.entries.is_empty());
        assert!(got.pending.is_empty());
        assert_eq!(got.catalog.hierarchy.as_ref().unwrap().nodes.len(), 11);
    }

    #[test]
    fn response_auth_echo_preserves_folder_row_and_cursor_identity() {
        let clean = serde_json::json!({"type":"videoportal","request":{"limit":300},"items":[
            {"type":"category","title":"Lazy","request":{"folder":1}},
            {"type":"category","title":"Inline","request":{"folder":2},"items":[
                {"type":"stream","title":"One","request":{"id":7,"filter":{"quality":"auto"}}}]},
            {"type":"next","request":{"offset":2,"limit":300}}]});
        let mut echoed = clean.clone();
        fn echo(value: &mut Value) {
            value["request"]["app"] = Value::String("echoed-app".into());
            value["request"]["key"] = Value::String("echoed-key".into());
        }
        echo(&mut echoed);
        echo(&mut echoed["items"][0]);
        echo(&mut echoed["items"][1]);
        echo(&mut echoed["items"][1]["items"][0]);
        echo(&mut echoed["items"][2]);
        let expected = PortalRequest::root().parse_response(&clean.to_string(), &profile()).unwrap();
        let got = PortalRequest::root().parse_response(&echoed.to_string(), &profile()).unwrap();
        assert_eq!(got, expected);
        assert_eq!(got.catalog.entries.len(), 1);
        assert_eq!(got.pending.len(), 2);
        let MediaLocator::VPortal { request } = &got.catalog.entries[0].media else {
            panic!("expected retained portal locator");
        };
        assert_eq!(request, &serde_json::json!({"id":7,"filter":{"quality":"auto"}}));
        let link = parse_vportal_link(&profile().source).unwrap();
        for pending in got.pending {
            let body = link.request_body(&pending.request).unwrap();
            assert_eq!(body["app"], "ott-play");
            assert_eq!(body["key"], "test");
        }
    }

    #[test]
    fn auth_only_response_requests_preserve_distinct_direct_url_identities() {
        let clean = serde_json::json!({"type":"category","items":[
            {"type":"stream","title":"One","url":"/one"},
            {"type":"stream","title":"Two","url":"/two"}]});
        let mut echoed = clean.clone();
        echoed["items"][0]["request"] = serde_json::json!({"app":"echo","key":"first"});
        echoed["items"][1]["request"] = serde_json::json!({"app":"echo","key":"second"});
        let expected = parse_vportal_response(&clean.to_string(), &profile(), "").unwrap();
        let got = parse_vportal_response(&echoed.to_string(), &profile(), "").unwrap();
        assert_eq!(got, expected);
        assert_eq!(got.catalog.entries.len(), 2);
        assert_ne!(got.catalog.entries[0].id, got.catalog.entries[1].id);
        assert!(got.catalog.entries.iter().all(|row| matches!(&row.media, MediaLocator::Http { .. })));
    }

    #[test]
    fn response_echo_never_weakens_nested_or_outgoing_request_validation() {
        let link = parse_vportal_link(&profile().source).unwrap();
        for field in ["app", "key", "__proto__", "constructor", "prototype"] {
            let mut params = serde_json::json!({"id":7,"nested":{}});
            params["nested"][field] = Value::String("not-authority".into());
            let response = serde_json::json!({"type":"stream","request":params});
            assert_eq!(parse_vportal_response(&response.to_string(), &profile(), ""), Err(CatalogError::InvalidFormat));
            assert!(link.request_body(&params).is_err());
            let mut top = serde_json::json!({"id":7});
            top[field] = Value::String("not-authority".into());
            assert!(link.request_body(&top).is_err());
            if field != "app" && field != "key" {
                let response = serde_json::json!({"type":"stream","request":top});
                assert_eq!(parse_vportal_response(&response.to_string(), &profile(), ""), Err(CatalogError::InvalidFormat));
            }
        }
        for invalid in [Value::Null, serde_json::json!([]), Value::String("invalid".into())] {
            let response = serde_json::json!({"type":"stream","request":invalid});
            assert_eq!(parse_vportal_response(&response.to_string(), &profile(), ""), Err(CatalogError::InvalidFormat));
        }
    }

    #[test]
    fn response_echo_retains_raw_request_size_and_depth_limits() {
        let oversized = serde_json::json!({"type":"stream","request":{
            "id":7,"key":"x".repeat(MAX_REQUEST_BYTES)}});
        assert_eq!(parse_vportal_response(&oversized.to_string(), &profile(), ""), Err(CatalogError::InvalidFormat));
        let mut deep = Value::String("leaf".into());
        for _ in 0..17 { deep = serde_json::json!({"nested":deep}); }
        let response = serde_json::json!({"type":"stream","request":{
            "id":7,"key":"echo","app":"echo","filter":deep}});
        assert_eq!(parse_vportal_response(&response.to_string(), &profile(), ""), Err(CatalogError::LimitExceeded));
    }
}

#[cfg(test)]
#[path = "vportal_ancestor_tests.rs"]
mod ancestor_tests;
