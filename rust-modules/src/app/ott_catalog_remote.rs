//! Bounded read receipts for the authenticated remote catalog protocol.
//! Read receipts also admit exact category choices through the existing Saved writer.
//! No IO, random policy or independent durable state is owned here.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const MAX_PAGE: u64 = 200;
const MAX_RESULT_BYTES: usize = 1024 * 1024;
const MAX_RECEIPTS: usize = 64;
const MAX_SNAPSHOTS: usize = 2;
const IDLE_MS: u64 = 120_000;
const MAX_INDEX_ENTRIES: usize = 200_000;

#[derive(Clone)]
enum Scope {
    Categories {
        selected: String,
    },
    Channels {
        category: String,
        search: String,
        indices: Arc<Vec<usize>>,
    },
}
#[derive(Clone)]
struct ReadReceipt {
    token: String,
    snapshot: Arc<Snapshot>,
    scope: Scope,
    expires: u64,
}
#[derive(Default)]
pub(super) struct CatalogReceipts {
    rows: Vec<ReadReceipt>,
    serial: u64,
    snapshot: Option<Arc<Snapshot>>,
}

/// The read cursor names an immutable list. Once an exact row was admitted,
/// unrelated discoveries may replace that list without replacing this target.
/// Reuse the ordinary request binder so private Local authority and Favorite
/// source proofs are checked identically to every other playback path.
pub(super) struct PendingPlay {
    invocation: String,
    runtime: String,
    favorites: bool,
    catalog_generation: Option<u64>,
    request: PlaybackRequest,
    profile: ProviderProfile,
    entry: Option<CatalogEntry>,
    favorite: Option<Favorite>,
}
impl PendingPlay {
    fn capture(runtime: &Runtime, id: &str, invocation: &Invocation) -> Result<Self, &'static str> {
        let mut request = PlaybackRequest::ordinary(id, 0);
        request.advance_on_failure = false;
        let (request, profile, entry, favorite) = runtime.prepare_play_request(request)?;
        let favorites = runtime.view_selection().1;
        Ok(Self {
            invocation: invocation.id.clone(),
            runtime: runtime.remote.runtime().into(),
            favorites,
            catalog_generation: (!favorites).then_some(runtime.generation),
            request,
            profile,
            entry,
            favorite,
        })
    }
    fn current(&self, runtime: &Runtime) -> bool {
        supported(runtime)
            && self.runtime == runtime.remote.runtime()
            && self.favorites == runtime.view_selection().1
            && self.catalog_generation.is_none_or(|generation| generation == runtime.generation)
            && runtime.play_target.as_ref() == Some(&self.request)
            && runtime.prepare_play_request(self.request.clone()).is_ok_and(
                |(request, profile, entry, favorite)| {
                    request == self.request && profile == self.profile
                        && same_playback_entry(&self.entry, &entry)
                        && favorite == self.favorite
                },
            )
    }
}

// Direct resolution chooses media through MediaLocator, not presentation/EPG
// enrichment. Provider-attested IDs are used when matching a Favorite during
// discovery; those portable identities are already in the Favorite locator.
fn same_playback_entry(expected: &Option<CatalogEntry>, current: &Option<CatalogEntry>) -> bool {
    match (expected, current) {
        (None, None) => true,
        (Some(expected), Some(current)) => expected.id == current.id && expected.media == current.media,
        _ => false,
    }
}

struct Item {
    wire: controls::Channel,
    // Never serialized: a Favorite's native selection key contains its locator.
    selection: Option<String>,
    // Private digest of the exact id + locator at read time. No URL/request copy.
    // Favorites instead bind their existing exact portable identity and source fence.
    media_binding: Option<[u8; 32]>,
}
#[derive(Clone, serde::Serialize)]
struct Category {
    id: String,
    name: String,
    count: usize,
    #[serde(skip)]
    indices: Vec<usize>,
    #[serde(skip)]
    selection: String,
}
struct Snapshot {
    runtime: String,
    provider: String,
    owner_fence: String,
    fence: String,
    loading: bool,
    categories: Vec<Category>,
    items: Vec<Item>,
    // Accounted once, including allocation capacities, not rescanned per receipt.
    bytes: usize,
}

fn media_binding(entry: &CatalogEntry) -> Result<[u8; 32], &'static str> {
    struct Digest(plx_base::sha256::Sha256);
    impl std::io::Write for Digest {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let mut digest = Digest(plx_base::sha256::Sha256::new());
    serde_json::to_writer(&mut digest, &(&entry.id, &entry.media)).map_err(|_| "unavailable")?;
    Ok(digest.0.finish())
}

fn available(runtime: &Runtime) -> Result<(), &'static str> {
    if !supported(runtime) { return Err("unavailable"); }
    if !runtime.view_selection().1 && !runtime.catalog_loading && !runtime.catalog_complete {
        return Err("catalog_incomplete");
    }
    Ok(())
}

fn read_receipt(runtime: &Runtime, params: &Value) -> Result<ReadReceipt, &'static str> {
    available(runtime)?;
    runtime.command_receipts.catalog.get(token(params)?, &owner_fence(runtime), runtime.now_ms())
}

fn valid_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
fn opaque(value: &Value) -> String {
    hash(value)
}
fn category_id(name: &str) -> String {
    if name.is_empty() {
        "all".into()
    } else {
        format!("category:{}", opaque(&json!(name)))
    }
}
pub(super) fn supported(runtime: &Runtime) -> bool {
    runtime.loaded
        && !runtime.persistence_uncertain
        && (runtime.view_selection().1
            || runtime
                .effective_profile()
                .is_some_and(|p| p.enabled && p.kind == runtime.effective_provider()))
}
// Capability discovery must not divert legacy clients into a protocol whose
// bounded metadata cannot represent their current source.
pub(super) fn capability(runtime: &mut Runtime) -> bool {
    snapshot(runtime).is_ok_and(|snapshot| selected(runtime, &snapshot).is_ok())
}
fn owner_fence(runtime: &Runtime) -> String {
    // Browsing between provider folders does not invalidate a snapshot. Moving
    // between provider and global Favorites *does*: these are distinct catalogs.
    if runtime.view_selection().1 {
        opaque(&json!([
            runtime.remote.runtime(),
            "favorites",
            runtime.saved.favorites,
            runtime.saved.profiles,
            runtime.local_inventory.generation()
        ]))
    } else {
        opaque(&json!([
            runtime.remote.runtime(),
            wire_provider(runtime.effective_provider()),
            runtime.effective_profile(),
            runtime.generation,
            runtime.saved.settings.get(catalog_organization::KEY)
        ]))
    }
}
// Fresh roots follow presentation/discovery changes; existing leases bind only
// the owner above and keep their original totals, loading flag and row order.
pub(super) fn fence(runtime: &Runtime) -> String {
    let owner = owner_fence(runtime);
    if runtime.view_selection().1 { return owner; }
    opaque(&json!([owner, runtime.catalog_revision,
        runtime.catalog_loading, runtime.catalog_complete]))
}

impl Snapshot {
    fn build(runtime: &Runtime) -> Result<Self, &'static str> {
        available(runtime)?;
        let favorites = runtime.view_selection().1;
        let mut items = Vec::new();
        if favorites {
            for (i, favorite) in runtime.saved.favorites.entries().iter().enumerate() {
                let identity = favorite.identity();
                items.push(Item {
                    wire: controls::Channel {
                        id: format!("fav:{}", opaque(&json!(identity))),
                        name: favorite.title.clone(),
                        number: i + 1,
                    },
                    selection: Some(format!("favorite:{identity}")),
                    media_binding: None,
                });
            }
        } else {
            for (wire, entry) in controls::catalog_channels(&runtime.catalog.entries, "")
                .into_iter().zip(&runtime.catalog.entries)
            {
                items.push(Item {
                    selection: None,
                    wire,
                    media_binding: Some(media_binding(entry)?),
                });
            }
        }
        let mut ids = BTreeSet::new();
        if items.len() > plx_data::ott::MAX_CATALOG_ENTRIES
            || items.iter().any(|item| {
                !valid_text(&item.wire.id, 2048)
                    || !valid_text(&item.wire.name, 512)
                    || !ids.insert(item.wire.id.as_str())
            })
        {
            return Err("unavailable");
        }
        drop(ids);
        let mut categories = vec![Category {
            id: "all".into(),
            name: "All".into(),
            count: items.len(),
            indices: (0..items.len()).collect(),
            selection: String::new(),
        }];
        if !favorites {
            // Membership and category order come from the same projection as
            // the UI and transport. One channel may belong to several groups.
            if let Some(projection) = runtime.organization_projection() {
                let positions: BTreeMap<&str, usize> = runtime.catalog.entries.iter()
                    .enumerate().map(|(index, entry)| (entry.id.as_str(), index)).collect();
                categories[0].indices = projection.all.iter().filter_map(|id| positions.get(id.as_str()).copied()).collect();
                categories[0].count = categories[0].indices.len();
                for category in &projection.categories {
                    if !valid_text(&category.label, 512) { return Err("unavailable"); }
                    let indices: Vec<_> = category.members.iter().filter_map(|id| positions.get(id.as_str()).copied()).collect();
                    categories.push(Category { id: category_id(&category.id), name: category.label.clone(),
                        count: indices.len(), indices, selection: category.id.clone() });
                }
            } else if runtime.catalog.hierarchy.is_some() {
                for category in runtime.catalog.category_groups() {
                    if !valid_text(&category.label, 512) { return Err("unavailable"); }
                    categories.push(Category { id: category_id(&category.id), name: category.label,
                        count: category.indices.len(), indices: category.indices, selection: category.id });
                }
            } else {
                let mut positions = BTreeMap::new();
                // Category order remains source-defined; only member order changes.
                for entry in &runtime.catalog.entries {
                    if entry.category.is_empty() { continue; }
                    if !valid_text(&entry.category, 512) { return Err("unavailable"); }
                    let id = category_id(&entry.category);
                    positions.entry(id.clone()).or_insert_with(|| {
                        let index = categories.len();
                        categories.push(Category { id, name: entry.category.clone(), count: 0, indices: Vec::new(), selection: entry.category.clone() });
                        index
                    });
                }
                let ordered = runtime.organization_rows("");
                categories[0].indices = ordered.iter().map(|(index, _)| *index).collect();
                categories[0].count = categories[0].indices.len();
                for (item_index, entry) in ordered {
                    if let Some(&index) = positions.get(&category_id(&entry.category)) {
                        categories[index].count += 1;
                        categories[index].indices.push(item_index);
                    }
                }
            }
        }
        Ok(Self {
            runtime: runtime.remote.runtime().into(),
            provider: if favorites {
                "favorites".into()
            } else {
                wire_provider(runtime.effective_provider()).into()
            },
            owner_fence: owner_fence(runtime),
            fence: fence(runtime),
            loading: !favorites && runtime.catalog_loading,
            categories,
            items,
            bytes: 0,
        }.accounted()?)
    }

    fn accounted(mut self) -> Result<Self, &'static str> {
        let mut bytes = std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>();
        for value in [&self.runtime, &self.provider, &self.owner_fence, &self.fence] {
            bytes = bytes.saturating_add(value.capacity());
        }
        bytes = bytes.saturating_add(self.items.capacity().saturating_mul(std::mem::size_of::<Item>()));
        for item in &self.items {
            bytes = bytes.saturating_add(item.wire.id.capacity()).saturating_add(item.wire.name.capacity())
                .saturating_add(item.selection.as_ref().map_or(0, String::capacity));
        }
        bytes = bytes.saturating_add(self.categories.capacity().saturating_mul(std::mem::size_of::<Category>()));
        for category in &self.categories {
            bytes = bytes.saturating_add(category.id.capacity()).saturating_add(category.name.capacity())
                .saturating_add(category.selection.capacity())
                .saturating_add(category.indices.capacity().saturating_mul(std::mem::size_of::<usize>()));
        }
        if bytes > plx_data::ott::MAX_CATALOG_MODEL_BYTES { return Err("unavailable"); }
        self.bytes = bytes;
        Ok(self)
    }
}

// Fresh roots and capability checks may build a current snapshot. A capability
// refresh never evicts a live lease: at capacity its temporary snapshot is dropped.
// Continuations do not enter here, so append/metadata/Finished cannot replace their list.
fn snapshot(runtime: &mut Runtime) -> Result<Arc<Snapshot>, &'static str> {
    available(runtime)?;
    let current = fence(runtime);
    let owner = owner_fence(runtime);
    let now = runtime.now_ms();
    runtime.command_receipts.catalog.prune(&owner, now);
    if let Some(snapshot) = &runtime.command_receipts.catalog.snapshot {
        if snapshot.fence == current {
            return Ok(snapshot.clone());
        }
    }
    let snapshot = Arc::new(Snapshot::build(runtime)?);
    runtime.command_receipts.catalog.cache_snapshot(snapshot.clone());
    Ok(snapshot)
}

fn selected(runtime: &Runtime, snapshot: &Snapshot) -> Result<String, &'static str> {
    let id = if runtime.view_selection().1 {
        "all".into()
    } else {
        category_id(&runtime.organization_category_id(runtime.effective_category()))
    };
    if snapshot.categories.iter().any(|row| row.id == id) {
        Ok(id)
    } else {
        Err(if snapshot.loading {
            "catalog_incomplete"
        } else {
            "category_not_found"
        })
    }
}
fn index_count(receipt: &ReadReceipt) -> usize {
    match &receipt.scope {
        Scope::Channels { indices, .. } => indices.len(),
        _ => 0,
    }
}

struct Usage {
    snapshots: usize,
    indices: usize,
    bytes: usize,
}
impl Usage {
    fn fits(&self, receipts: usize) -> bool {
        receipts <= MAX_RECEIPTS && self.snapshots <= MAX_SNAPSHOTS
            && self.indices <= MAX_INDEX_ENTRIES && self.bytes <= plx_data::ott::MAX_CATALOG_MODEL_BYTES
    }
}

impl CatalogReceipts {
    /// An explicit order change retires read capabilities, not admitted playback.
    /// Keep the serial so a new token cannot alias a retired token in this runtime.
    pub(super) fn retire_reads(&mut self) {
        self.rows.clear();
        self.snapshot = None;
    }
    fn get(&self, token: &str, owner: &str, now: u64) -> Result<ReadReceipt, &'static str> {
        self.rows.iter()
            .find(|r| r.token == token && r.expires > now && r.snapshot.owner_fence == owner)
            .cloned().ok_or("catalog_changed")
    }
    fn candidate(&mut self, snapshot: &Arc<Snapshot>, scope: Scope, id: &str, now: u64) -> ReadReceipt {
        self.serial = self.serial.wrapping_add(1);
        ReadReceipt {
            token: format!("catalog:{}", opaque(&json!([snapshot.runtime, id, self.serial]))),
            snapshot: snapshot.clone(),
            scope,
            expires: now.saturating_add(IDLE_MS),
        }
    }
    fn prune(&mut self, owner: &str, now: u64) {
        self.rows.retain(|r| r.expires > now && r.snapshot.owner_fence == owner);
        if self.snapshot.as_ref().is_some_and(|s| s.owner_fence != owner) { self.snapshot = None; }
    }
    // Count each shared allocation once. Receipt structs reserve the full bounded
    // ledger capacity; strings and query Vec capacities are additional owned memory.
    fn usage(&self, extra: Option<&ReadReceipt>, cache: Option<&Arc<Snapshot>>) -> Usage {
        let mut usage = Usage { snapshots: 0, indices: 0,
            bytes: std::mem::size_of::<Self>() + MAX_RECEIPTS * std::mem::size_of::<ReadReceipt>() };
        let mut snapshots: Vec<&Arc<Snapshot>> = Vec::new();
        let mut indices: Vec<&Arc<Vec<usize>>> = Vec::new();
        for receipt in self.rows.iter().chain(extra) {
            usage.bytes = usage.bytes.saturating_add(receipt.token.capacity());
            usage.indices = usage.indices.saturating_add(index_count(receipt));
            if !snapshots.iter().any(|s| Arc::ptr_eq(s, &receipt.snapshot)) {
                snapshots.push(&receipt.snapshot);
                usage.bytes = usage.bytes.saturating_add(receipt.snapshot.bytes);
            }
            match &receipt.scope {
                Scope::Categories { selected } => usage.bytes = usage.bytes.saturating_add(selected.capacity()),
                Scope::Channels { category, search, indices: rows } => {
                    usage.bytes = usage.bytes.saturating_add(category.capacity()).saturating_add(search.capacity());
                    if !indices.iter().any(|i| Arc::ptr_eq(i, rows)) {
                        indices.push(rows);
                        usage.bytes = usage.bytes.saturating_add(std::mem::size_of::<Vec<usize>>()
                            + 2 * std::mem::size_of::<usize>())
                            .saturating_add(rows.capacity().saturating_mul(std::mem::size_of::<usize>()));
                    }
                }
            }
        }
        if let Some(cache) = cache {
            if !snapshots.iter().any(|s| Arc::ptr_eq(s, cache)) {
                snapshots.push(cache);
                usage.bytes = usage.bytes.saturating_add(cache.bytes);
            }
        }
        usage.snapshots = snapshots.len();
        usage
    }
    fn cache_snapshot(&mut self, snapshot: Arc<Snapshot>) -> bool {
        // The old unpinned cache is replaced, not charged alongside its successor.
        if !self.usage(None, Some(&snapshot)).fits(self.rows.len()) { return false; }
        self.snapshot = Some(snapshot);
        true
    }
    fn can_admit(receipt: &ReadReceipt) -> Result<(), &'static str> {
        if !Self::default().usage(Some(receipt), None).fits(1) { return Err("unavailable"); }
        Ok(())
    }
    fn commit_read(&mut self, mut receipt: ReadReceipt, now: u64) -> Result<(), &'static str> {
        // Refuse an oversized single read before changing any existing lease/TTL.
        Self::can_admit(&receipt)?;
        receipt.expires = now.saturating_add(IDLE_MS);
        self.rows.retain(|r| r.expires > now && r.token != receipt.token);
        if self.snapshot.as_ref().is_some_and(|cache| !Arc::ptr_eq(cache, &receipt.snapshot)
            && !self.rows.iter().any(|r| Arc::ptr_eq(&r.snapshot, cache))) {
            self.snapshot = None;
        }
        while !self.usage(Some(&receipt), self.snapshot.as_ref()).fits(self.rows.len() + 1) {
            if self.snapshot.as_ref().is_some_and(|cache| !Arc::ptr_eq(cache, &receipt.snapshot)
                && !self.rows.iter().any(|r| Arc::ptr_eq(&r.snapshot, cache))) {
                self.snapshot = None;
                continue;
            }
            let usage = self.usage(Some(&receipt), self.snapshot.as_ref());
            if usage.snapshots > MAX_SNAPSHOTS || usage.bytes > plx_data::ott::MAX_CATALOG_MODEL_BYTES {
                if let Some(family) = self.rows.iter().find(|r| !Arc::ptr_eq(&r.snapshot, &receipt.snapshot))
                    .map(|r| r.snapshot.clone()) {
                    self.rows.retain(|r| !Arc::ptr_eq(&r.snapshot, &family));
                    if self.snapshot.as_ref().is_some_and(|cache| Arc::ptr_eq(cache, &family)) {
                        self.snapshot = None;
                    }
                    continue;
                }
            }
            // Remaining pressure is query/receipt storage in this same family.
            // The admitted/renewed receipt is not in rows and cannot evict itself.
            if self.rows.is_empty() { self.snapshot = None; }
            else { self.rows.remove(0); }
        }
        self.rows.push(receipt);
        Ok(())
    }
}

fn error(runtime: &Runtime, inv: &Invocation, op: &str, code: &'static str) -> Value {
    // The controller validates the echo against its request even for mismatch.
    let requested = inv
        .params
        .get("runtime")
        .and_then(Value::as_str)
        .unwrap_or(runtime.remote.runtime());
    json!({"status":if code == "unsupported" { "unsupported" } else { "rejected" },
        "data":{"version":1,"runtime":requested,"op":op,"state":"error","error":code}})
}
fn page(params: &Value) -> Result<(usize, usize), &'static str> {
    let offset = params["offset"]
        .as_u64()
        .filter(|n| *n <= 1_000_000)
        .ok_or("invalid_request")?;
    let limit = params["limit"]
        .as_u64()
        .filter(|n| (1..=MAX_PAGE).contains(n))
        .ok_or("invalid_request")?;
    Ok((offset as usize, limit as usize))
}
fn token(params: &Value) -> Result<&str, &'static str> {
    let token = params["catalog"].as_str().ok_or("invalid_request")?;
    if token.is_empty()
        || token.len() > 128
        || !token
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.:-".contains(&c))
    {
        return Err("invalid_request");
    }
    Ok(token)
}
fn bounded(data: Value) -> Result<Value, &'static str> {
    if serde_json::to_vec(&data).map_err(|_| "unavailable")?.len() > MAX_RESULT_BYTES {
        Err("unavailable")
    } else {
        Ok(ok(data))
    }
}

fn is_select(inv: &Invocation) -> bool {
    inv.action == "categories" && inv.params["op"] == "select"
}

pub(super) fn select_rejection(runtime: &Runtime, inv: &Invocation, code: &'static str) -> Option<Value> {
    is_select(inv).then(|| error(runtime, inv, "select", code))
}

pub(super) fn select_completion_allowed(runtime: &Runtime, inv: &Invocation) -> bool {
    !is_select(inv) || (runtime.loaded && !runtime.persistence_uncertain
        && runtime.now_ms() < inv.deadline_ms && runtime.remote.invocation_is_current(inv)
        && input_policy::navigation(runtime).is_ok())
}

pub(super) fn select_settled(runtime: &Runtime, inv: &Invocation, saved: bool,
    applied: bool, response: Value) -> Value {
    if !is_select(inv) { return response; }
    if !saved || runtime.now_ms() >= inv.deadline_ms || !runtime.remote.invocation_is_current(inv) {
        return error(runtime, inv, "select", "unavailable");
    }
    if input_policy::navigation(runtime).is_err() {
        return error(runtime, inv, "select", "restricted");
    }
    if !applied { return error(runtime, inv, "select", "catalog_changed"); }
    response
}

pub(super) fn select(runtime: &mut Runtime, inv: &Invocation) -> Option<Value> {
    let result = (|| {
        let p = &inv.params;
        if !fields(p, &["op", "runtime", "catalog", "category"]) || p["op"] != "select" {
            return Err("invalid_request");
        }
        if p["runtime"].as_str() != Some(runtime.remote.runtime()) { return Err("runtime_mismatch"); }
        let category = p["category"].as_str().filter(|value| valid_text(value, 1024))
            .ok_or("invalid_request")?;
        input_policy::navigation(runtime).map_err(|_| "restricted")?;
        if runtime.now_ms() >= inv.deadline_ms || !runtime.remote.invocation_is_current(inv) {
            return Err("unavailable");
        }
        let receipt = read_receipt(runtime, p)?;
        let snapshot = &receipt.snapshot;
        if !matches!(&receipt.scope, Scope::Categories { .. }) { return Err("catalog_changed"); }
        let row = snapshot.categories.iter().find(|row| row.id == category)
            .ok_or(if snapshot.loading { "catalog_incomplete" } else { "category_not_found" })?;
        if snapshot.provider != "favorites" && !row.selection.is_empty() {
            let current = runtime.organization_category_id(&row.selection);
            if current != row.selection
                || !runtime.organization_categories().iter().any(|category| category.id == current) {
                return Err("catalog_changed");
            }
        }
        let response = bounded(json!({"version":1,"runtime":snapshot.runtime,"op":"select",
            "catalog":receipt.token,"provider":snapshot.provider,"selected":category,"dispatched":true}))?;
        let selection = (snapshot.provider != "favorites").then(|| row.selection.clone());
        runtime.save_remote_category_choice(selection, inv.clone(), response)
            .map_err(|_| "unavailable")?;
        Ok(())
    })();
    result.err().map(|code| error(runtime, inv, "select", code))
}

pub(super) fn categories(runtime: &mut Runtime, inv: &Invocation) -> Value {
    let op = if inv.params["op"] == "select" {
        "select"
    } else {
        "list"
    };
    let result = (|| {
        if op != "list" {
            return Err("unsupported");
        }
        let p = &inv.params;
        if (!fields(p, &["op", "runtime", "offset", "limit"])
            && !fields(p, &["op", "runtime", "offset", "limit", "catalog"]))
            || p["op"] != "list"
        {
            return Err("invalid_request");
        }
        if p["runtime"].as_str() != Some(runtime.remote.runtime()) {
            return Err("runtime_mismatch");
        }
        let (offset, limit) = page(p)?;
        let now = runtime.now_ms();
        let fresh = p.get("catalog").is_none();
        let receipt = if !fresh {
            read_receipt(runtime, p)?
        } else {
            if offset != 0 {
                return Err("invalid_request");
            }
            let snapshot = snapshot(runtime)?;
            let selected = selected(runtime, &snapshot)?;
            runtime.command_receipts.catalog.candidate(
                &snapshot,
                Scope::Categories { selected },
                &inv.id,
                now,
            )
        };
        let Scope::Categories { selected } = &receipt.scope else {
            return Err("catalog_changed");
        };
        let snapshot = receipt.snapshot.clone();
        if offset > snapshot.categories.len() {
            return Err("invalid_request");
        }
        let rows = snapshot
            .categories
            .iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        let response = bounded(json!({"version":1,"runtime":snapshot.runtime,"op":"list",
            "catalog":receipt.token,"provider":snapshot.provider,"selected":selected,
            "offset":offset,"total":snapshot.categories.len(),"has_more":offset+rows.len()<snapshot.categories.len(),
            "loading":snapshot.loading,"categories":rows}))?;
        runtime.command_receipts.catalog.commit_read(receipt, now)?;
        if fresh { runtime.command_receipts.catalog.cache_snapshot(snapshot); }
        Ok(response)
    })();
    result.unwrap_or_else(|code| error(runtime, inv, op, code))
}

pub(super) fn channels(
    runtime: &mut Runtime,
    inv: &Invocation,
    search_key: fn(&str) -> String,
) -> Value {
    let result = (|| {
        let p = &inv.params;
        if !fields(p, &["runtime", "catalog", "category", "offset", "limit"])
            && !fields(
                p,
                &[
                    "runtime", "catalog", "category", "offset", "limit", "search",
                ],
            )
        {
            return Err("invalid_request");
        }
        if p["runtime"].as_str() != Some(runtime.remote.runtime()) {
            return Err("runtime_mismatch");
        }
        let (offset, limit) = page(p)?;
        let category = p["category"]
            .as_str()
            .filter(|s| valid_text(s, 1024))
            .ok_or("invalid_request")?;
        let search = match p.get("search") {
            None => String::new(),
            Some(value) => search_key(text(value, 1024, true).ok_or("invalid_request")?),
        };
        let now = runtime.now_ms();
        let source = read_receipt(runtime, p)?;
        let snapshot = source.snapshot.clone();
        if !snapshot.categories.iter().any(|c| c.id == category) {
            return Err("category_not_found");
        }
        let receipt = match &source.scope {
            Scope::Categories { .. } => {
                if offset != 0 {
                    return Err("invalid_request");
                }
                let indices = snapshot.categories.iter().find(|row| row.id == category)
                    .ok_or("category_not_found")?.indices.iter().copied()
                    .filter(|index| search.is_empty() || search_key(&snapshot.items[*index].wire.name).contains(&search))
                    .collect();
                runtime.command_receipts.catalog.candidate(
                    &snapshot,
                    Scope::Channels {
                        category: category.into(),
                        search: search.clone(),
                        indices: Arc::new(indices),
                    },
                    &inv.id,
                    now,
                )
            }
            Scope::Channels {
                category: bound,
                search: bound_search,
                ..
            } => {
                if bound != category || bound_search != &search {
                    return Err("catalog_changed");
                }
                source.clone()
            }
        };
        let Scope::Channels { indices, .. } = &receipt.scope else {
            unreachable!()
        };
        if offset > indices.len() {
            return Err("invalid_request");
        }
        let page = indices
            .iter()
            .skip(offset)
            .take(limit)
            .map(|index| &snapshot.items[*index].wire)
            .collect::<Vec<_>>();
        let response = bounded(
            json!({"version":1,"runtime":snapshot.runtime,"catalog":receipt.token,
            "category":category,"offset":offset,"total":indices.len(),"has_more":offset+page.len()<indices.len(),
            "loading":snapshot.loading,"channels":page}),
        )?;
        // Validate the child before renewing its source: a failed read renews neither.
        CatalogReceipts::can_admit(&receipt)?;
        if source.token != receipt.token {
            runtime.command_receipts.catalog.commit_read(source, now)?;
        }
        runtime.command_receipts.catalog.commit_read(receipt, now)?;
        Ok(response)
    })();
    result.unwrap_or_else(|code| error(runtime, inv, "channels", code))
}

pub(super) fn play(runtime: &mut Runtime, app: &mut App, inv: &Invocation) -> Option<Value> {
    // Deliberately do not negotiate random: native has no authoritative
    // per-entry eligibility model equivalent to the current player contract.
    if inv.params.get("random").is_some() {
        return Some(unsupported("Random catalog playback is not supported"));
    }
    let result = (|| {
        if !fields(&inv.params, &["catalog", "id"]) {
            return Err("Use one catalog receipt and exact ID");
        }
        if !app.running
            || !super::super::playback_allowed(app)
            || runtime.saving
            || runtime.persistence_uncertain
        {
            return Err("Playback is suspended or settings are being committed");
        }
        let id = text(&inv.params["id"], 2048, false).ok_or("Invalid catalog ID")?;
        available(runtime).map_err(|_| "Catalog is unavailable")?;
        let receipt = read_receipt(runtime, &inv.params)?;
        let snapshot = &receipt.snapshot;
        let Scope::Channels { indices, .. } = &receipt.scope else {
            return Err("Use a channel-page receipt");
        };
        let item = indices
            .iter()
            .map(|index| &snapshot.items[*index])
            .find(|item| item.wire.id == id)
            .ok_or("The ID is not in this catalog selection")?;
        // Already-discovered exact IDs may play while discovery continues.
        // Partial title matches remain a controller-side admission decision.
        let selection = item.selection.as_deref().unwrap_or(&item.wire.id);
        let pending = PendingPlay::capture(runtime, selection, inv)?;
        if let Some(expected) = item.media_binding {
            let current = pending.entry.as_ref().ok_or("catalog_changed")?;
            if media_binding(current)? != expected { return Err("catalog_changed"); }
        }
        if let Err(error) = play_channel_reply(
            runtime,
            app,
            selection,
            &item.wire,
            inv,
        ) {
            runtime.retire_closed_source(app);
            restore_continuity(runtime, app);
            return Err(error);
        }
        runtime.command_receipts.catalog_play = Some(pending);
        Ok(())
    })();
    result.err().map(rejected)
}

pub(super) fn pending_current(runtime: &Runtime) -> bool {
    let Some(inv) = &runtime.pending_play else {
        return true;
    };
    runtime
        .command_receipts
        .catalog_play
        .as_ref()
        .is_none_or(|pending| inv.id != pending.invocation || pending.current(runtime))
}

/// Run before removing the owned playback target. Exact failure remains terminal
/// through managed resume even after its original remote response is gone.
pub(super) fn play_failed(runtime: &mut Runtime, app: &mut App) -> bool {
    if !runtime
        .play_target
        .as_ref()
        .is_some_and(|request| !request.advance_on_failure)
    {
        return false;
    }
    runtime.retire_closed_source(app);
    restore_continuity(runtime, app);
    true
}

fn restore_continuity(runtime: &mut Runtime, app: &App) {
    let retained = runtime
        .playing
        .as_ref()
        .filter(|playing| playing_matches(playing, app))
        .map(|playing| playing.selection_id.clone());
    if let Some(id) = retained {
        runtime.continuity.user_selected(runtime.generation, &id);
    } else {
        runtime.continuity.stop(runtime.generation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_fixture() -> Arc<Snapshot> {
        snapshot_with_capacity("revision-1", 0)
    }
    fn snapshot_with_capacity(revision: &str, capacity: usize) -> Arc<Snapshot> {
        Arc::new(Snapshot {
            runtime: "fixture".into(),
            provider: "vportal".into(),
            owner_fence: "source-1".into(),
            fence: revision.into(),
            loading: false,
            categories: vec![],
            items: vec![Item {
                wire: controls::Channel { id: "row".into(), name: String::with_capacity(capacity), number: 1 },
                selection: None,
                media_binding: None,
            }],
            bytes: 0,
        }.accounted().unwrap())
    }
    fn scope() -> Scope {
        Scope::Categories { selected: "all".into() }
    }
    fn add(ledger: &mut CatalogReceipts, snapshot: &Arc<Snapshot>, id: &str) -> String {
        let receipt = ledger.candidate(snapshot, scope(), id, 1);
        let token = receipt.token.clone();
        ledger.commit_read(receipt, 1).unwrap();
        token
    }

    #[test]
    fn catalog_read_retirement_releases_families_without_reusing_tokens() {
        let mut ledger = CatalogReceipts::default();
        let snapshot = snapshot_fixture();
        let weak = Arc::downgrade(&snapshot);
        let old = add(&mut ledger, &snapshot, "same-request");
        assert!(ledger.cache_snapshot(snapshot));
        let serial = ledger.serial;
        ledger.retire_reads();
        assert!(ledger.rows.is_empty());
        assert!(ledger.snapshot.is_none());
        assert!(weak.upgrade().is_none(), "retirement must release the cached family too");
        assert_eq!(ledger.serial, serial);
        let fresh = add(&mut ledger, &snapshot_fixture(), "same-request");
        assert_ne!(fresh, old, "same request and runtime must not resurrect a retired token");
        assert!(ledger.get(&old, "source-1", 2).is_err());
        assert!(ledger.get(&fresh, "source-1", 2).is_ok());
    }

    #[test]
    fn catalog_receipts_only_successful_reads_renew_idle_expiry() {
        let snapshot = snapshot_fixture();
        let mut ledger = CatalogReceipts::default();
        let first = ledger.candidate(&snapshot, scope(), "request-1", 100);
        let token = first.token.clone();
        ledger.commit_read(first, 100).unwrap();
        assert!(ledger.get(&token, &snapshot.owner_fence, 120_099).is_ok());
        assert!(ledger.get(&token, &snapshot.owner_fence, 120_100).is_err(), "lookup/play do not renew");
        let refreshed = ledger.get(&token, &snapshot.owner_fence, 119_000).unwrap();
        ledger.commit_read(refreshed, 119_000).unwrap();
        assert!(ledger.get(&token, &snapshot.owner_fence, 238_999).is_ok());
        assert!(ledger.get(&token, &snapshot.owner_fence, 239_000).is_err());
        assert!(ledger.get(&token, "source-2", 120_000).is_err());
    }

    #[test]
    fn catalog_receipts_bound_both_number_and_query_index_memory() {
        let snapshot = snapshot_fixture();
        let mut ledger = CatalogReceipts::default();
        for i in 0..100 { add(&mut ledger, &snapshot, &format!("request-{i}")); }
        assert_eq!(ledger.rows.len(), MAX_RECEIPTS);
        for i in 0..10 {
            let receipt = ledger.candidate(&snapshot, Scope::Channels {
                category: "all".into(), search: String::new(),
                indices: Arc::new((0..50_000).collect()),
            }, &format!("large-{i}"), 2);
            ledger.commit_read(receipt, 2).unwrap();
            assert!(ledger.rows.iter().map(index_count).sum::<usize>() <= MAX_INDEX_ENTRIES);
            assert!(ledger.rows.len() <= MAX_RECEIPTS);
            assert!(ledger.usage(None, ledger.snapshot.as_ref()).bytes <= plx_data::ott::MAX_CATALOG_MODEL_BYTES);
        }
    }

    #[test]
    fn catalog_receipt_refresh_preserves_token_and_shared_indices() {
        let snapshot = snapshot_fixture();
        let mut ledger = CatalogReceipts::default();
        let indices = Arc::new(vec![1, 3, 5]);
        let first = ledger.candidate(&snapshot, Scope::Channels {
            category: "films".into(), search: "title".into(), indices: indices.clone(),
        }, "request", 1);
        let token = first.token.clone();
        ledger.commit_read(first, 1).unwrap();
        let second = ledger.get(&token, &snapshot.owner_fence, 2).unwrap();
        let Scope::Channels { indices: same, .. } = &second.scope else { panic!("wrong scope"); };
        assert!(Arc::ptr_eq(&indices, same));
        assert!(Arc::ptr_eq(&snapshot, &second.snapshot));
        ledger.commit_read(second, 2).unwrap();
        assert_eq!(ledger.rows[0].token, token);
    }

    #[test]
    fn catalog_cache_refresh_preserves_two_live_snapshot_families() {
        let mut ledger = CatalogReceipts::default();
        let first = snapshot_fixture();
        let first_token = add(&mut ledger, &first, "first");
        let second = snapshot_with_capacity("revision-2", 0);
        let second_token = add(&mut ledger, &second, "second");
        assert!(ledger.cache_snapshot(second));
        for revision in 3..20 {
            let candidate = snapshot_with_capacity(&format!("revision-{revision}"), 0);
            let weak = Arc::downgrade(&candidate);
            assert!(!ledger.cache_snapshot(candidate));
            assert!(weak.upgrade().is_none(), "an uncacheable capability snapshot must be released");
            assert!(ledger.get(&first_token, "source-1", 2).is_ok());
            assert!(ledger.get(&second_token, "source-1", 2).is_ok());
            assert_eq!(ledger.usage(None, ledger.snapshot.as_ref()).snapshots, 2);
        }
    }

    #[test]
    fn catalog_new_read_evicts_oldest_snapshot_family_and_releases_it() {
        let mut ledger = CatalogReceipts::default();
        let first = snapshot_fixture();
        let weak = Arc::downgrade(&first);
        let first_token = add(&mut ledger, &first, "first");
        let sibling = add(&mut ledger, &first, "sibling");
        assert!(ledger.cache_snapshot(first.clone()));
        drop(first);
        let second = snapshot_with_capacity("revision-2", 0);
        let second_token = add(&mut ledger, &second, "second");
        let third = snapshot_with_capacity("revision-3", 0);
        let third_token = add(&mut ledger, &third, "third");
        assert!(ledger.get(&first_token, "source-1", 2).is_err());
        assert!(ledger.get(&sibling, "source-1", 2).is_err());
        assert!(weak.upgrade().is_none(), "cache must not retain an evicted family");
        assert!(ledger.get(&second_token, "source-1", 2).is_ok());
        assert!(ledger.get(&third_token, "source-1", 2).is_ok());
        assert_eq!(ledger.usage(None, ledger.snapshot.as_ref()).snapshots, 2);
    }

    #[test]
    fn catalog_snapshot_and_query_capacity_share_one_byte_budget() {
        let budget = plx_data::ott::MAX_CATALOG_MODEL_BYTES;
        let mut ledger = CatalogReceipts::default();
        let first = snapshot_with_capacity("revision-1", budget / 2);
        let weak = Arc::downgrade(&first);
        let first_token = add(&mut ledger, &first, "first");
        drop(first);
        let second = snapshot_with_capacity("revision-2", budget / 2);
        assert!(!ledger.cache_snapshot(second.clone()), "two half-budget strings plus metadata exceed the aggregate budget");
        let second_token = add(&mut ledger, &second, "second");
        assert!(weak.upgrade().is_none());
        assert!(ledger.get(&first_token, "source-1", 2).is_err());
        assert!(ledger.get(&second_token, "source-1", 2).is_ok());
        let mut indices = Vec::with_capacity(budget / std::mem::size_of::<usize>());
        indices.push(0);
        let oversized = ledger.candidate(&second, Scope::Channels {
            category: "all".into(), search: String::new(), indices: Arc::new(indices),
        }, "oversized", 2);
        assert_eq!(ledger.commit_read(oversized, 2), Err("unavailable"));
        assert!(ledger.get(&second_token, "source-1", 120_000).is_ok(), "a refused admission must not evict its source");
        assert!(ledger.get(&second_token, "source-1", 120_001).is_err(), "a refused admission must not renew its source");
        assert!(ledger.usage(None, ledger.snapshot.as_ref()).bytes <= budget);
    }

    #[test]
    fn catalog_protocol_bounds_reject_before_serialized_delivery() {
        assert!(page(&json!({"offset":0,"limit":200})).is_ok());
        for bad in [
            json!({"offset":0,"limit":201}),
            json!({"offset":-1,"limit":1}),
            json!({"offset":1_000_001,"limit":1}),
            json!({"offset":0,"limit":0}),
        ] {
            assert!(page(&bad).is_err());
        }
        assert!(bounded(json!({"data":"x".repeat(MAX_RESULT_BYTES)})).is_err());
        assert!(token(&json!({"catalog":"https://invalid.example"})).is_err());
        assert!(valid_text("Épisode", 512));
        assert!(!valid_text("line\nfeed", 512));
    }
}
