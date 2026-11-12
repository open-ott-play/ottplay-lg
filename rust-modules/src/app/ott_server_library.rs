//! Read-only client for the existing OTTserver /offline-media catalogue protocol.
//! Snapshot GETs reuse the server's ongoing scan; this client never imports or refreshes files.
use plx_base::task::{time::Budget, OffFrame};
use plx_data::ott::{self, CatalogEntry, MediaLocator, ProviderKind, ProviderProfile};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const BATCH_SIZE: usize = 64;
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

fn poll_interval(rows: usize) -> Duration {
    if rows >= 10_000 {
        Duration::from_secs(2)
    } else if rows >= 1_000 {
        Duration::from_secs(1)
    } else {
        POLL_INTERVAL
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    available: bool,
    items: Vec<Item>,
    #[serde(default)]
    sources: Vec<Source>,
    #[serde(default)]
    scanning: bool,
    #[serde(default)]
    scanned: u64,
    total: Option<u64>,
    scan_error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    id: String,
    name: String,
    #[serde(default)]
    directory: String,
    #[serde(default)]
    mime: String,
    url: String,
    available: bool,
    #[serde(default)]
    source_id: String,
    relative_path: Option<String>,
    playback: Option<Playback>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Source {
    id: String,
    name: String,
    kind: String,
    available: bool,
}

#[derive(Default)]
struct Roots {
    /// Only unambiguous, available folder registrations may produce portable references.
    aliases: HashMap<String, String>,
}

impl Roots {
    fn new(sources: &[Source]) -> Self {
        let mut ids = HashMap::new();
        let mut names = HashMap::new();
        for source in sources {
            *ids.entry(source.id.as_str()).or_insert(0usize) += 1;
            if source.kind == "folder" {
                *names.entry(source.name.as_str()).or_insert(0usize) += 1;
            }
        }
        let aliases = sources
            .iter()
            .filter(|source| {
                source.kind == "folder"
                    && source.available
                    && identifier(&source.id)
                    && root_alias(&source.name)
                    && ids.get(source.id.as_str()) == Some(&1)
                    && names.get(source.name.as_str()) == Some(&1)
            })
            .map(|source| (source.id.clone(), source.name.clone()))
            .collect();
        Self { aliases }
    }

    fn portable(&self, item: &Item, source: &str) -> (Option<String>, Option<String>) {
        match (item.relative_path.as_deref(), self.aliases.get(source)) {
            (Some(path), Some(alias))
                if item.source_id == source && ott::valid_local_relative_path(path) =>
            {
                (Some(path.to_owned()), Some(alias.clone()))
            }
            _ => (None, None),
        }
    }

    fn source_for(&self, alias: &str) -> Option<&str> {
        self.aliases
            .iter()
            .find_map(|(id, name)| (name == alias).then_some(id.as_str()))
    }
}

fn root_alias(value: &str) -> bool {
    !value.is_empty()
        && value.encode_utf16().count() <= 256
        && value.trim() == value
        && !value.contains("://")
        && !value.chars().any(char::is_control)
}

#[derive(Deserialize)]
struct Playback {
    mode: String,
}

#[derive(PartialEq, Eq)]
struct Progress {
    scanning: bool,
    scanned: u64,
    total: Option<u64>,
}

#[derive(Default)]
struct State {
    rows: HashMap<String, CatalogEntry>,
    model_bytes: usize,
    progress: Option<Progress>,
    last_progress: Duration,
}

fn endpoint(profile: &ProviderProfile) -> Result<String, &'static str> {
    profile
        .validate()
        .map_err(|_| "Invalid server library profile")?;
    if profile.kind != ProviderKind::Local {
        return Err("Profile is not a local media library");
    }
    ott::http_url(&profile.source).ok_or("Server library address is invalid")
}

fn snapshot(text: &str) -> Result<Snapshot, &'static str> {
    if text.len() > ott::MAX_CATALOG_BYTES {
        return Err("Server library response is too large");
    }
    let value: Snapshot =
        serde_json::from_str(text).map_err(|_| "Server library returned an invalid catalogue")?;
    if value.items.len() > ott::MAX_CATALOG_ENTRIES
        || value.sources.len() > ott::MAX_CATALOG_ENTRIES
    {
        return Err("Server library item limit reached");
    }
    if !value.available {
        return Err("Server library is unavailable");
    }
    Ok(value)
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn item_key(item: &Item) -> Result<(&str, String), &'static str> {
    let source = if item.source_id.is_empty() {
        &item.id
    } else {
        &item.source_id
    };
    if !identifier(source) || !identifier(&item.id) {
        return Err("Server library returned an invalid file identity");
    }
    Ok((source, format!("{source}:{}", item.id)))
}

fn item_url(base: &str, item: &Item) -> Result<Option<String>, &'static str> {
    if !item.available
        || item
            .playback
            .as_ref()
            .is_some_and(|p| p.mode == "unsupported")
    {
        return Ok(None);
    }
    if item.url.is_empty() {
        return Err("Server library returned no media address");
    }
    ott::resolve_http_url(base, &item.url)
        .map(Some)
        .ok_or("Server library returned an invalid media address")
}

fn entry(
    profile: &ProviderProfile,
    item: &Item,
    source: &str,
    key: &str,
    roots: &Roots,
) -> Result<Option<CatalogEntry>, &'static str> {
    if !item.mime.starts_with("video/") && !ott::is_local_video_path(&item.name) {
        return Ok(None);
    }
    if item.name.trim().is_empty()
        || item.name.len() > 1024
        || item.directory.len() > 4096
        || item.name.chars().any(char::is_control)
        || item.directory.chars().any(char::is_control)
    {
        return Err("Server library returned an invalid file name");
    }
    let (relative_path, root_alias) = roots.portable(item, source);
    Ok(Some(CatalogEntry { protection: None,
        synopsis: None,
        id: format!("{}:server:{key}", profile.id),
        title: item.name.clone(),
        category: if item.directory.is_empty() {
            profile.name.clone()
        } else {
            item.directory.clone()
        },
        description: String::new(),
        poster: String::new(),
        duration_ms: None,
        provider_id: Some(key.to_owned()),
        provider_id_field: Some("server-library".into()),
        live: None,
        media: MediaLocator::ServerLibrary {
            source_id: source.to_owned(),
            item_id: item.id.clone(),
            relative_path,
            root_alias,
        },
    }))
}

fn row_bytes(row: &CatalogEntry) -> usize {
    let locator = match &row.media {
        MediaLocator::ServerLibrary {
            source_id,
            item_id,
            relative_path,
            root_alias,
        } => {
            source_id.len()
                + item_id.len()
                + relative_path.as_ref().map_or(0, String::len)
                + root_alias.as_ref().map_or(0, String::len)
        }
        _ => 0,
    };
    std::mem::size_of::<CatalogEntry>()
        + row.id.len()
        + row.title.len()
        + row.category.len()
        + locator
        + row.provider_id.as_ref().map_or(0, String::len)
        + row.provider_id_field.as_ref().map_or(0, String::len)
}

impl State {
    fn publish(
        &mut self,
        profile: &ProviderProfile,
        base: &str,
        snapshot: Snapshot,
        now: Duration,
        cancel: &AtomicBool,
        emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
    ) -> Result<bool, &'static str> {
        let progress = Progress {
            scanning: snapshot.scanning,
            scanned: snapshot.scanned,
            total: snapshot.total,
        };
        let mut changed = self.progress.as_ref() != Some(&progress);
        let mut batch = Vec::with_capacity(BATCH_SIZE);
        let mut identities = HashMap::new();
        let roots = Roots::new(&snapshot.sources);
        for item in snapshot.items {
            if cancel.load(Ordering::Acquire) {
                return Ok(true);
            }
            let (source, key) = item_key(&item)?;
            let url = item_url(base, &item)?;
            if identities
                .insert(key.clone(), url.clone())
                .is_some_and(|previous| previous != url)
            {
                return Err("Server library returned conflicting file identities");
            }
            if url.is_none() {
                continue;
            }
            let Some(row) = entry(profile, &item, source, &key, &roots)? else {
                continue;
            };
            if self.rows.get(&row.id) == Some(&row) {
                continue;
            }
            let old_size = self.rows.get(&row.id).map_or(0, row_bytes);
            if old_size == 0 && self.rows.len() >= ott::MAX_CATALOG_ENTRIES {
                return Err("Server library item limit reached");
            }
            let bytes = self
                .model_bytes
                .saturating_sub(old_size)
                .saturating_add(row_bytes(&row));
            if bytes > ott::MAX_CATALOG_MODEL_BYTES {
                return Err("Server library catalogue is too large");
            }
            let first = self.rows.is_empty();
            self.model_bytes = bytes;
            self.rows.insert(row.id.clone(), row.clone());
            batch.push(row);
            changed = true;
            if first || batch.len() >= BATCH_SIZE {
                emit(std::mem::take(&mut batch))?;
            }
        }
        if !batch.is_empty() && !cancel.load(Ordering::Acquire) {
            emit(batch)?;
        }
        if changed {
            self.last_progress = now;
        }
        self.progress = Some(progress);
        if cancel.load(Ordering::Acquire) {
            return Ok(true);
        }
        if snapshot
            .scan_error
            .as_ref()
            .is_some_and(|error| !error.is_empty())
        {
            return Err("Server library scan was incomplete; the available videos remain listed");
        }
        Ok(!snapshot.scanning)
    }
}

pub(super) fn visit(
    worker: &OffFrame,
    profile: &ProviderProfile,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    // The pure state resets its stall accounting whenever catalogue progress is
    // observed. This worker sample supplies elapsed time without a frame clock.
    let elapsed = Budget::start(worker, STALL_TIMEOUT);
    visit_with(
        profile,
        cancel,
        emit,
        &mut |url| super::fetch(url, None),
        &mut || elapsed.spent(worker),
        &mut |delay| {
            Budget::start(worker, STALL_TIMEOUT).wait(
                worker,
                delay,
                Duration::from_millis(10),
                &|| cancel.load(Ordering::Acquire),
            );
        },
    )
}

fn visit_with(
    profile: &ProviderProfile,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
    fetch: &mut dyn FnMut(&str) -> Result<String, &'static str>,
    now: &mut dyn FnMut() -> Duration,
    wait: &mut dyn FnMut(Duration),
) -> Result<(), &'static str> {
    if cancel.load(Ordering::Acquire) {
        return Ok(());
    }
    let base = endpoint(profile)?;
    let mut state = State {
        last_progress: now(),
        ..State::default()
    };
    loop {
        if cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        let response = fetch(&base);
        if cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        let text = response?;
        if state.publish(profile, &base, snapshot(&text)?, now(), cancel, emit)? {
            return Ok(());
        }
        if now().saturating_sub(state.last_progress) >= STALL_TIMEOUT {
            return Err("Server library scan stopped responding");
        }
        wait(poll_interval(state.rows.len()));
    }
}

/// Bind the opaque file identity to the current profile endpoint at playback time.
/// The full snapshot is checked, so duplicate identities cannot select an arbitrary URL.
pub(super) fn resolve(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    cancel: &AtomicBool,
) -> Result<String, &'static str> {
    resolve_with(profile, entry, cancel, &mut |url| super::fetch(url, None))
}

fn resolve_with(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    cancel: &AtomicBool,
    fetch: &mut dyn FnMut(&str) -> Result<String, &'static str>,
) -> Result<String, &'static str> {
    let MediaLocator::ServerLibrary {
        source_id,
        item_id,
        relative_path,
        root_alias,
    } = &entry.media
    else {
        return Err("Media is not a server library file");
    };
    if !identifier(source_id) || !identifier(item_id) {
        return Err("Invalid server library file identity");
    }
    if cancel.load(Ordering::Acquire) {
        return Err("Server library playback cancelled");
    }
    let base = endpoint(profile)?;
    let response = fetch(&base);
    if cancel.load(Ordering::Acquire) {
        return Err("Server library playback cancelled");
    }
    let text = response?;
    let current = snapshot(&text)?;
    let roots = Roots::new(&current.sources);
    let mut matched = None;
    let mut seen = false;
    for item in current.items {
        if cancel.load(Ordering::Acquire) {
            return Err("Server library playback cancelled");
        }
        let (source, _) = item_key(&item)?;
        if source != source_id || item.id != *item_id {
            continue;
        }
        if relative_path.is_some() || root_alias.is_some() {
            let current = roots.portable(&item, source);
            if relative_path.is_none()
                || root_alias.is_none()
                || current.0.as_ref() != relative_path.as_ref()
                || current.1.as_ref() != root_alias.as_ref()
            {
                return Err("Server library file no longer matches its selected path");
            }
        }
        let url = item_url(&base, &item)?;
        if seen && matched != url {
            return Err("Server library returned conflicting file identities");
        }
        seen = true;
        matched = url;
    }
    matched.ok_or("Server library file is unavailable")
}

/// Rebind a portable Local favorite through this receiving server's root inventory.
/// Neither opaque IDs nor a basename from the sending installation can select a file.
pub(super) fn find_favorite(
    profile: &ProviderProfile,
    root_alias: &str,
    relative_path: &str,
    cancel: &AtomicBool,
) -> Result<CatalogEntry, &'static str> {
    find_favorite_with(profile, root_alias, relative_path, cancel, &mut |url| {
        super::fetch(url, None)
    })
}

fn find_favorite_with(
    profile: &ProviderProfile,
    alias: &str,
    relative_path: &str,
    cancel: &AtomicBool,
    fetch: &mut dyn FnMut(&str) -> Result<String, &'static str>,
) -> Result<CatalogEntry, &'static str> {
    if !root_alias(alias) || !ott::valid_local_relative_path(relative_path) {
        return Err("Invalid server library favorite path");
    }
    if cancel.load(Ordering::Acquire) {
        return Err("Server library playback cancelled");
    }
    let base = endpoint(profile)?;
    let response = fetch(&base);
    if cancel.load(Ordering::Acquire) {
        return Err("Server library playback cancelled");
    }
    let current = snapshot(&response?)?;
    let roots = Roots::new(&current.sources);
    let source = roots
        .source_for(alias)
        .ok_or("Favorite root is unavailable or ambiguous on this server")?;
    let mut found = None;
    for item in current.items {
        if cancel.load(Ordering::Acquire) {
            return Err("Server library playback cancelled");
        }
        if item.source_id != source || item.relative_path.as_deref() != Some(relative_path) {
            continue;
        }
        if found.is_some() {
            return Err("Favorite matches more than one server library file");
        }
        let (_, key) = item_key(&item)?;
        if item_url(&base, &item)?.is_none() {
            return Err("Server library favorite is unavailable");
        }
        found = entry(profile, &item, source, &key, &roots)?;
        if found.is_none() {
            return Err("Server library favorite is not a video");
        }
    }
    // The normal playback resolver performs the fresh URL/path check. Do not
    // fetch a third cumulative catalogue merely to discard its resolved URL.
    found.ok_or("Favorite is not available from this server library")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;

    fn profile() -> ProviderProfile {
        ProviderProfile {
            id: "server-local".into(),
            name: "Library".into(),
            kind: ProviderKind::Local,
            source: "https://library.test/offline-media/private".into(),
            source_alias: "Home".into(),
            enabled: true,
        }
    }
    fn item(id: &str) -> serde_json::Value {
        json!({"id":id,"name":format!("{id}.mp4"),"directory":"Folder",
        "url":format!("/offline-media/private/{id}"),"available":true,"mime":"video/mp4","sourceId":"folder1"})
    }
    fn document(items: Vec<serde_json::Value>, scanning: bool, scanned: u64) -> String {
        json!({"available":true,"items":items,"scanning":scanning,"scanned":scanned,"total":null})
            .to_string()
    }

    fn root(id: &str, name: &str) -> serde_json::Value {
        json!({"id":id,"name":name,"kind":"folder","available":true})
    }

    fn portable_item(id: &str, source: &str, path: &str) -> serde_json::Value {
        let mut value = item(id);
        value["name"] = json!(path.rsplit('/').next().unwrap_or(path));
        value["sourceId"] = json!(source);
        value["relativePath"] = json!(path);
        value
    }

    fn portable_document(items: Vec<serde_json::Value>, sources: Vec<serde_json::Value>) -> String {
        json!({"available":true,"items":items,"sources":sources,"scanning":false,"scanned":2})
            .to_string()
    }

    fn published(text: &str) -> Vec<CatalogEntry> {
        let p = profile();
        let mut rows = Vec::new();
        State::default()
            .publish(
                &p,
                &p.source,
                snapshot(text).unwrap(),
                Duration::ZERO,
                &AtomicBool::new(false),
                &mut |batch| {
                    rows.extend(batch);
                    Ok(())
                },
            )
            .unwrap();
        rows
    }

    #[test]
    fn cumulative_snapshots_only_emit_new_or_changed_rows_and_first_is_immediate() {
        let p = profile();
        let cancel = AtomicBool::new(false);
        let mut state = State::default();
        let mut batches = Vec::new();
        let first = snapshot(&document(vec![item("one"), item("two")], true, 2)).unwrap();
        assert!(!state
            .publish(&p, &p.source, first, Duration::ZERO, &cancel, &mut |rows| {
                batches.push(rows);
                Ok(())
            })
            .unwrap());
        assert_eq!(batches.iter().map(Vec::len).collect::<Vec<_>>(), vec![1, 1]);
        let mut changed = item("two");
        changed["name"] = json!("renamed.mp4");
        let second = snapshot(&document(
            vec![item("one"), changed, item("three")],
            false,
            3,
        ))
        .unwrap();
        assert!(state
            .publish(
                &p,
                &p.source,
                second,
                Duration::from_secs(1),
                &cancel,
                &mut |rows| {
                    batches.push(rows);
                    Ok(())
                }
            )
            .unwrap());
        assert_eq!(batches[2].len(), 2);
        assert_eq!(batches[2][0].title, "renamed.mp4");
        assert_eq!(state.rows.len(), 3);
        assert_eq!(
            batches[0][0].media,
            MediaLocator::ServerLibrary {
                source_id: "folder1".into(),
                item_id: "one".into(),
                relative_path: None,
                root_alias: None,
            }
        );
    }

    #[test]
    fn cancellation_in_first_callback_stops_processing_the_snapshot() {
        let p = profile();
        let cancel = AtomicBool::new(false);
        let mut state = State::default();
        let mut count = 0;
        let value = snapshot(&document(
            (0..200).map(|n| item(&n.to_string())).collect(),
            true,
            200,
        ))
        .unwrap();
        assert!(state
            .publish(&p, &p.source, value, Duration::ZERO, &cancel, &mut |rows| {
                count += rows.len();
                cancel.store(true, Ordering::Release);
                Ok(())
            })
            .unwrap());
        assert_eq!(count, 1);
        assert_eq!(state.rows.len(), 1);
    }

    #[test]
    fn scan_error_keeps_published_rows_but_does_not_claim_complete() {
        let p = profile();
        let mut state = State::default();
        let mut count = 0;
        let mut value = snapshot(&document(vec![item("one")], false, 1)).unwrap();
        value.scan_error = Some("folder unavailable".into());
        assert!(state
            .publish(
                &p,
                &p.source,
                value,
                Duration::ZERO,
                &AtomicBool::new(false),
                &mut |rows| {
                    count += rows.len();
                    Ok(())
                }
            )
            .is_err());
        assert_eq!(count, 1);
    }

    #[test]
    fn progress_can_continue_past_one_minute_but_unchanged_snapshot_times_out() {
        let clock = Cell::new(Duration::ZERO);
        let calls = Cell::new(0);
        let mut count = 0;
        let result = visit_with(
            &profile(),
            &AtomicBool::new(false),
            &mut |rows| {
                count += rows.len();
                Ok(())
            },
            &mut |_| {
                calls.set(calls.get() + 1);
                Ok(document(vec![item("one")], calls.get() < 5, calls.get()))
            },
            &mut || clock.get(),
            &mut |_| clock.set(clock.get() + Duration::from_secs(20)),
        );
        assert_eq!(result, Ok(()));
        assert_eq!(count, 1);
        assert_eq!(clock.get(), Duration::from_secs(80));
        clock.set(Duration::ZERO);
        let result = visit_with(
            &profile(),
            &AtomicBool::new(false),
            &mut |_| Ok(()),
            &mut |_| Ok(document(vec![], true, 0)),
            &mut || clock.get(),
            &mut |_| clock.set(clock.get() + Duration::from_secs(20)),
        );
        assert_eq!(result, Err("Server library scan stopped responding"));
    }

    #[test]
    fn playback_rebinds_to_current_endpoint_and_rejects_ambiguous_ids() {
        let mut p = profile();
        let row = entry(
            &p,
            &serde_json::from_value(item("one")).unwrap(),
            "folder1",
            "folder1:one",
            &Roots::default(),
        )
        .unwrap()
        .unwrap();
        p.source = "https://replacement.test/offline-media/new-token".into();
        let url = resolve_with(&p, &row, &AtomicBool::new(false), &mut |requested| {
            assert_eq!(
                requested,
                "https://replacement.test:443/offline-media/new-token"
            );
            let mut value = item("one");
            value["url"] = json!("/offline-media/new-token/one");
            Ok(document(vec![value], false, 1))
        })
        .unwrap();
        assert_eq!(
            url,
            "https://replacement.test:443/offline-media/new-token/one"
        );
        let mut conflicting = item("one");
        conflicting["url"] = json!("/other");
        assert_eq!(
            resolve_with(&p, &row, &AtomicBool::new(false), &mut |_| Ok(document(
                vec![item("one"), conflicting.clone()],
                false,
                2
            ))),
            Err("Server library returned conflicting file identities")
        );
    }

    #[test]
    fn cancellation_around_transport_does_not_publish_or_start_another_request() {
        let cancel = AtomicBool::new(true);
        assert!(visit_with(
            &profile(),
            &cancel,
            &mut |_| panic!("emit"),
            &mut |_| panic!("fetch"),
            &mut || Duration::ZERO,
            &mut |_| panic!("wait")
        )
        .is_ok());
        cancel.store(false, Ordering::Release);
        assert!(visit_with(
            &profile(),
            &cancel,
            &mut |_| panic!("emit"),
            &mut |_| {
                cancel.store(true, Ordering::Release);
                Ok(document(vec![item("one")], true, 1))
            },
            &mut || Duration::ZERO,
            &mut |_| panic!("wait")
        )
        .is_ok());
    }

    #[test]
    fn large_cumulative_catalogues_reduce_polling_without_slowing_initial_discovery() {
        assert_eq!(poll_interval(0), Duration::from_millis(250));
        assert_eq!(poll_interval(999), Duration::from_millis(250));
        assert_eq!(poll_interval(1_000), Duration::from_secs(1));
        assert_eq!(poll_interval(9_999), Duration::from_secs(1));
        assert_eq!(poll_interval(10_000), Duration::from_secs(2));
    }

    #[test]
    fn portable_favorite_matches_full_nested_path_not_basename() {
        let text = portable_document(
            vec![
                portable_item("first", "folder1", "Family/clip.mp4"),
                portable_item("second", "folder1", "Travel/clip.mp4"),
            ],
            vec![root("folder1", "Movies")],
        );
        let rows = published(&text);
        assert_eq!(rows.len(), 2);
        assert!(
            matches!(&rows[1].media, MediaLocator::ServerLibrary {relative_path:Some(path),root_alias:Some(alias),..}
            if path == "Travel/clip.mp4" && alias == "Movies")
        );
        let mut requests = 0;
        let found = find_favorite_with(
            &profile(),
            "Movies",
            "Travel/clip.mp4",
            &AtomicBool::new(false),
            &mut |_| {
                requests += 1;
                Ok(text.clone())
            },
        )
        .unwrap();
        assert!(
            matches!(found.media, MediaLocator::ServerLibrary {item_id,..} if item_id == "second")
        );
        assert_eq!(
            requests, 1,
            "the playback resolver owns the subsequent fresh URL check"
        );
    }

    #[test]
    fn resolved_opaque_id_cannot_substitute_a_different_relative_path() {
        let original = portable_document(
            vec![portable_item("one", "folder1", "Family/clip.mp4")],
            vec![root("folder1", "Movies")],
        );
        let selected = published(&original).remove(0);
        let replaced = portable_document(
            vec![portable_item("one", "folder1", "Travel/clip.mp4")],
            vec![root("folder1", "Movies")],
        );
        assert_eq!(
            resolve_with(&profile(), &selected, &AtomicBool::new(false), &mut |_| Ok(
                replaced.clone()
            )),
            Err("Server library file no longer matches its selected path")
        );
    }

    #[test]
    fn old_server_without_inventory_or_paths_keeps_opaque_playback_only() {
        for text in [
            document(vec![item("one")], false, 1),
            portable_document(vec![portable_item("one", "folder1", "clip.mp4")], vec![]),
            portable_document(vec![item("one")], vec![root("folder1", "Movies")]),
        ] {
            let rows = published(&text);
            assert!(matches!(
                rows[0].media,
                MediaLocator::ServerLibrary {
                    relative_path: None,
                    root_alias: None,
                    ..
                }
            ));
            assert!(
                resolve_with(&profile(), &rows[0], &AtomicBool::new(false), &mut |_| Ok(
                    text.clone()
                ))
                .is_ok()
            );
            assert!(find_favorite_with(
                &profile(),
                "Movies",
                "clip.mp4",
                &AtomicBool::new(false),
                &mut |_| Ok(text.clone())
            )
            .is_err());
        }
    }

    #[test]
    fn duplicate_root_names_or_ids_never_produce_portable_references() {
        let mut unavailable_duplicate = root("folder2", "Movies");
        unavailable_duplicate["available"] = json!(false);
        for sources in [
            vec![root("folder1", "Movies"), root("folder2", "Movies")],
            vec![root("folder1", "Movies"), unavailable_duplicate],
            vec![root("folder1", "Movies"), root("folder1", "Other")],
        ] {
            let text =
                portable_document(vec![portable_item("one", "folder1", "clip.mp4")], sources);
            let rows = published(&text);
            assert!(matches!(
                rows[0].media,
                MediaLocator::ServerLibrary {
                    relative_path: None,
                    root_alias: None,
                    ..
                }
            ));
            assert_eq!(
                find_favorite_with(
                    &profile(),
                    "Movies",
                    "clip.mp4",
                    &AtomicBool::new(false),
                    &mut |_| Ok(text.clone())
                ),
                Err("Favorite root is unavailable or ambiguous on this server")
            );
        }
    }

    #[test]
    fn invalid_paths_cannot_be_exported_or_used_for_remote_lookup() {
        for path in [
            "../clip.mp4",
            "/private/clip.mp4",
            "Family//clip.mp4",
            "Family/./clip.mp4",
            "C:/clip.mp4",
        ] {
            let text = portable_document(
                vec![portable_item("one", "folder1", path)],
                vec![root("folder1", "Movies")],
            );
            let rows = published(&text);
            assert!(matches!(
                rows[0].media,
                MediaLocator::ServerLibrary {
                    relative_path: None,
                    root_alias: None,
                    ..
                }
            ));
            assert_eq!(
                find_favorite_with(
                    &profile(),
                    "Movies",
                    path,
                    &AtomicBool::new(false),
                    &mut |_| panic!("invalid path requested")
                ),
                Err("Invalid server library favorite path")
            );
        }
    }

    #[test]
    fn receiving_server_can_use_different_opaque_ids_for_the_same_root_and_path() {
        let old = portable_document(
            vec![portable_item("old-file", "old-root", "Series/episode.mp4")],
            vec![root("old-root", "Movies")],
        );
        let original = published(&old).remove(0);
        let text = portable_document(
            vec![portable_item("new-file", "new-root", "Series/episode.mp4")],
            vec![root("new-root", "Movies")],
        );
        let mut p = profile();
        p.source = "https://receiving.test/offline-media/its-token".into();
        let found = find_favorite_with(
            &p,
            "Movies",
            "Series/episode.mp4",
            &AtomicBool::new(false),
            &mut |address| {
                assert_eq!(
                    address,
                    "https://receiving.test:443/offline-media/its-token"
                );
                Ok(text.clone())
            },
        )
        .unwrap();
        assert_ne!(original.id, found.id);
        assert!(
            matches!(found.media, MediaLocator::ServerLibrary {source_id,item_id,relative_path:Some(path),root_alias:Some(alias)}
            if source_id == "new-root" && item_id == "new-file" && path == "Series/episode.mp4" && alias == "Movies")
        );
    }

    #[test]
    fn duplicate_full_paths_are_rejected_instead_of_picking_the_first_file() {
        let text = portable_document(
            vec![
                portable_item("one", "folder1", "clip.mp4"),
                portable_item("two", "folder1", "clip.mp4"),
            ],
            vec![root("folder1", "Movies")],
        );
        assert_eq!(
            find_favorite_with(
                &profile(),
                "Movies",
                "clip.mp4",
                &AtomicBool::new(false),
                &mut |_| Ok(text.clone())
            ),
            Err("Favorite matches more than one server library file")
        );
    }

    #[test]
    fn favorite_cancellation_before_and_after_transport_suppresses_resolution() {
        let cancel = AtomicBool::new(true);
        assert_eq!(
            find_favorite_with(&profile(), "Movies", "clip.mp4", &cancel, &mut |_| panic!(
                "cancelled lookup requested"
            )),
            Err("Server library playback cancelled")
        );
        cancel.store(false, Ordering::Release);
        assert_eq!(
            find_favorite_with(&profile(), "Movies", "clip.mp4", &cancel, &mut |_| {
                cancel.store(true, Ordering::Release);
                Err("late failure")
            }),
            Err("Server library playback cancelled")
        );
    }
}
