//! Stremio presentation and request adaptation over the existing companion worker.
//! Manifests and stream replies are proposals; only a current, explicit selection
//! may start media. No worker, persistence writer, HTTP stack or lease is owned here.
use super::*;
use crate::app::ott_provider_http::{BoundedHttp, ProviderHttp, Request as HttpRequest};
use crate::app::ott_stremio_catalog::{self as catalog, Manifest, Movie};
use crate::app::ott_stremio_settings::{Addon, Config};
use crate::app::ott_stremio_streams::{self as streams, Candidate, FileChoice, SourceProposal};
use crate::app::ott_torrent_discovery as discovery;
use plx_media::ff::ExternalSource;

const KEY: &str = "nativeStremio";

#[derive(Clone)]
struct Loaded {
    addon: Addon,
    manifest: Arc<Manifest>,
}
#[derive(Clone)]
struct Selection {
    addon: Loaded,
    catalog: String,
    query: Option<String>,
}
#[derive(Clone)]
struct BuiltinSelection {
    category: String,
    query: Option<String>,
}
#[derive(Clone)]
enum Operation {
    Catalogs,
    Page {
        selection: Selection,
        offset: usize,
    },
    Streams {
        addons: Vec<Loaded>,
        media_ids: Vec<String>,
    },
    BuiltinPage { selection: BuiltinSelection, page: u16 },
    BuiltinStreams { addons: Vec<Loaded>, stable_id: String },
}
struct Flight {
    generation: u64,
    expires: u64,
    config: Config,
}
pub(super) struct State {
    config: Config,
    observed: Option<Value>,
    loaded: Vec<Loaded>,
    selected: Option<Selection>,
    selected_builtin: Option<BuiltinSelection>,
    builtin_categories: Vec<discovery::CategoryRow>,
    builtin_next: Option<u16>,
    movies: Vec<Movie>,
    next: Option<usize>,
    candidates: Vec<Candidate>,
    movie_title: String,
    flight: Option<Flight>,
    retry: Option<Operation>,
    choice: Option<usize>,
    service_return: bool,
}
impl Default for State {
    fn default() -> Self {
        Self {
            config: Config::default(),
            observed: None,
            loaded: Vec::new(),
            selected: None,
            selected_builtin: None,
            builtin_categories: Vec::new(),
            builtin_next: None,
            movies: Vec::new(),
            next: None,
            candidates: Vec::new(),
            movie_title: String::new(),
            flight: None,
            retry: None,
            choice: None,
            service_return: false,
        }
    }
}
impl State {
    pub(super) fn cancel(&mut self) {
        self.flight = None;
    }
}
pub(super) struct FetchRequest {
    generation: u64,
    expires: u64,
    deadline: Deadline,
    config: Config,
    operation: Operation,
}
enum Payload {
    Catalogs {
        loaded: Vec<Loaded>,
        failed: usize,
        builtin: Vec<discovery::CategoryRow>,
        builtin_failed: bool,
    },
    Page {
        selection: Selection,
        offset: usize,
        page: catalog::Page,
    },
    BuiltinPage { selection: BuiltinSelection, page: u16, result: discovery::Page },
    Streams {
        metadata: Option<Movie>,
        candidates: Vec<Candidate>,
        failed: usize,
        unsupported: usize,
        duplicates: usize,
        rejected: Vec<(usize, streams::Rejection)>,
    },
}
pub(super) struct Fetched {
    generation: u64,
    expires: u64,
    config: Config,
    result: Result<Payload, &'static str>,
}

fn saved_config(runtime: &Runtime) -> Result<Config, &'static str> {
    Config::parse(runtime.saved.settings.get(KEY).unwrap_or(&Value::Null))
        .map_err(|_| "Invalid saved Stremio settings")
}
fn current_configuration(adapter: &Adapter, runtime: &Runtime) -> Result<Config, &'static str> {
    let current = runtime.saved.settings.get(KEY).unwrap_or(&Value::Null);
    if adapter.stremio.observed.as_ref() != Some(current) {
        return Err("Catalog settings changed. Reload categories.");
    }
    saved_config(runtime)
}
fn operation_configuration(config: &Config, operation: &Operation) -> Result<(), &'static str> {
    let admitted = |loaded: &Loaded| config.addons.iter().any(|addon| addon.enabled && addon == &loaded.addon);
    let valid = match operation {
        Operation::Catalogs => true,
        Operation::Page { selection, .. } => admitted(&selection.addon),
        Operation::Streams { addons, .. } => addons.iter().all(admitted),
        Operation::BuiltinPage { .. } => config.catalog_service_url.is_some(),
        Operation::BuiltinStreams { addons, .. } => config.catalog_service_url.is_some() && addons.iter().all(admitted),
    };
    if !valid { return Err("Catalog source changed. Reload categories."); }
    Ok(())
}

fn body(http: &mut dyn ProviderHttp, url: String, max: usize) -> Result<Vec<u8>, &'static str> {
    if http.cancelled() {
        return Err("Request cancelled");
    }
    let reply = http.request(&HttpRequest {
        url,
        headers: Vec::new(),
        body: None,
    })?;
    if http.cancelled() {
        return Err("Request cancelled");
    }
    if !(200..300).contains(&reply.status) {
        return Err("Add-on request failed");
    }
    if reply.body.len() > max {
        return Err("Add-on response exceeds limit");
    }
    Ok(reply.body)
}
fn discovery_response(http: &mut dyn ProviderHttp, config: &Config, relative: &str)
    -> Result<crate::app::ott_provider_http::Response, &'static str> {
    if http.cancelled() { return Err("Request cancelled"); }
    let url = config.discovery_url(relative).map_err(|_| "Configure the catalog service URL in Settings")?;
    let response = http.request(&HttpRequest { url, headers: Vec::new(), body: None })?;
    if http.cancelled() { return Err("Request cancelled"); }
    if response.body.len() > 524_288 { return Err("Catalog response exceeds limit"); }
    Ok(response)
}

fn perform(
    http: &mut dyn ProviderHttp,
    config: &Config,
    op: &Operation,
) -> Result<Payload, &'static str> {
    match op {
        Operation::Catalogs => {
            let mut loaded = Vec::new();
            let mut failed = 0;
            for addon in config.addons.iter().filter(|addon| addon.enabled) {
                if http.cancelled() {
                    return Err("Request cancelled");
                }
                match body(http, addon.manifest_url.clone(), 65_536).and_then(|bytes| {
                    Manifest::parse(&bytes).map_err(|_| "Unsupported add-on manifest")
                }) {
                    Ok(manifest) => loaded.push(Loaded {
                        addon: addon.clone(),
                        manifest: Arc::new(manifest),
                    }),
                    Err(_) => failed += 1,
                }
            }
            let mut builtin = Vec::new();
            let mut builtin_failed = false;
            if config.catalog_service_url.is_some() {
                let result = discovery::CategoriesRequest::new(&config.language)
                    .map_err(|_| "Invalid catalog language")
                    .and_then(|request| {
                        let response = discovery_response(http, config, request.relative_path())?;
                        request.decode(response.status, &response.body).map_err(|_| "Could not load built-in categories")
                    });
                match result { Ok(categories) => builtin = categories, Err(_) => builtin_failed = true }
            }
            if loaded.is_empty() && builtin.is_empty() && failed > 0 {
                return Err("Could not load add-ons. Retry or check their settings.");
            }
            Ok(Payload::Catalogs { loaded, failed, builtin, builtin_failed })
        }
        Operation::Page { selection, offset } => {
            let request = selection
                .addon
                .manifest
                .catalog_request(&selection.catalog, selection.query.as_deref(), *offset)
                .map_err(|_| "This catalog does not support that request")?;
            if request.catalog_id != selection.catalog {
                return Err("Catalog identity changed");
            }
            let url = selection
                .addon
                .addon
                .catalog_resource_url(
                    &selection.addon.manifest,
                    request.relative_path(),
                    &config.language,
                )
                .map_err(|_| "Invalid add-on catalog address")?;
            let bytes = body(http, url, 524_288)?;
            let page = request
                .decode_page(&bytes)
                .map_err(|_| "Invalid catalog response")?;
            Ok(Payload::Page {
                selection: selection.clone(),
                offset: *offset,
                page,
            })
        }
        Operation::BuiltinPage { selection, page } => {
            let request = discovery::PageRequest::new(&selection.category, selection.query.as_deref(), *page, &config.language)
                .map_err(|_| "Invalid built-in catalog request")?;
            let response = discovery_response(http, config, request.relative_path())?;
            let result = request.decode(response.status, &response.body).map_err(|_| "Could not load built-in catalog")?;
            Ok(Payload::BuiltinPage { selection: selection.clone(), page: *page, result })
        }
        Operation::BuiltinStreams { addons, stable_id } => {
            let request = discovery::DetailsRequest::new(stable_id, &config.language)
                .map_err(|_| "Invalid movie identity")?;
            let response = discovery_response(http, config, request.relative_path())?;
            let movie = request.decode(response.status, &response.body).map_err(|_| "Could not load movie details")?;
            let media_ids = movie.stream_ids().map_err(|_| "Invalid movie identity")?;
            let mut result = perform(http, config, &Operation::Streams { addons: addons.clone(), media_ids })?;
            if let Payload::Streams { metadata, .. } = &mut result { *metadata = Some(movie.metadata); }
            Ok(result)
        }
        Operation::Streams { addons, media_ids } => {
            let mut candidates: Vec<Candidate> = Vec::new();
            let mut seen = std::collections::BTreeSet::new();
            let mut failed = 0;
            let mut unsupported = 0;
            let mut duplicates = 0;
            let mut rejected = Vec::new();
            for loaded in addons {
                if http.cancelled() {
                    return Err("Request cancelled");
                }
                let Some(request) = loaded
                    .manifest
                    .stream_request(&media_ids.iter().map(String::as_str).collect::<Vec<_>>())
                    .map_err(|_| "Invalid media identity")?
                else {
                    continue;
                };
                if !media_ids.iter().any(|id| id == &request.media_id) {
                    return Err("Media identity changed");
                }
                let url = loaded
                    .addon
                    .resource_url(request.relative_path())
                    .map_err(|_| "Invalid add-on stream address")?;
                match body(http, url, 524_288).and_then(|bytes| {
                    streams::parse(&bytes, &loaded.manifest.name)
                        .map_err(|_| "Invalid stream response")
                }) {
                    Ok(result) => {
                        unsupported += result.rejected.iter().filter(|item| {
                            matches!(item.reason, streams::Rejection::Unsupported(_))
                        }).count();
                        rejected.extend(
                            result
                                .rejected
                                .iter()
                                .map(|item| (item.position, item.reason)),
                        );
                        duplicates += result.duplicate_count;
                        for candidate in result.candidates {
                            if seen.insert(candidate.source.clone()) {
                                candidates.push(candidate);
                            } else {
                                duplicates += 1;
                            }
                        }
                    }
                    Err(_) => failed += 1,
                }
            }
            Ok(Payload::Streams {
                metadata: None,
                candidates,
                failed,
                unsupported,
                duplicates,
                rejected,
            })
        }
    }
}
pub(super) fn fetch(
    worker: &plx_base::task::OffFrame,
    cancel: &AtomicBool,
    request: FetchRequest,
) -> Fetched {
    let mut http = BoundedHttp::new(worker, cancel, request.deadline, 524_288);
    let result = perform(&mut http, &request.config, &request.operation);
    Fetched {
        generation: request.generation,
        expires: request.expires,
        config: request.config,
        result,
    }
}
fn begin(
    adapter: &mut Adapter,
    runtime: &mut Runtime,
    app: &mut App,
    op: Operation,
) -> Result<(), &'static str> {
    if matches!(&op, Operation::Streams { .. } | Operation::BuiltinStreams { .. }) {
        // The movie screen already names this lookup. Even an admission
        // refusal must offer its metadata retry, never the prior movie's
        // playback choice. A same-movie playback retry goes through play().
        adapter.stremio.candidates.clear();
        adapter.stremio.choice = None;
        adapter.stremio.retry = Some(op.clone());
    }
    let config = if matches!(op, Operation::Catalogs) { saved_config(runtime)? }
        else { current_configuration(adapter, runtime)? };
    operation_configuration(&config, &op)?;
    if runtime.saving || runtime.persistence_uncertain {
        return Err("Wait for settings to finish saving");
    }
    adapter.navigate(runtime, app);
    adapter.stremio.observed = Some(
        runtime
            .saved
            .settings
            .get(KEY)
            .cloned()
            .unwrap_or(Value::Null),
    );
    adapter.stremio.config = config.clone();
    match &op {
        Operation::Catalogs => {
            adapter.stremio.loaded.clear();
            adapter.stremio.builtin_categories.clear();
        },
        Operation::Page { offset: 0, .. } => {
            adapter.stremio.movies.clear();
            adapter.stremio.next = None;
        }
        Operation::BuiltinPage { page: 1, .. } => {
            adapter.stremio.movies.clear();
            adapter.stremio.builtin_next = None;
        }
        _ => {}
    }
    let expires = runtime.now_ms().saturating_add(UI_TIMEOUT);
    let deadline = operation_deadline(runtime.clock_sample, expires)?;
    adapter.stremio.flight = Some(Flight {
        generation: adapter.generation,
        expires,
        config: config.clone(),
    });
    adapter.stremio.retry = Some(op.clone());
    let request = FetchRequest {
        generation: adapter.generation,
        expires,
        deadline,
        config,
        operation: op,
    };
    if let Err(error) = adapter.launch(Work::Stremio(request)) {
        adapter.stremio.flight = None;
        return Err(error);
    }
    runtime.status = "Loading…".into();
    runtime.changed();
    Ok(())
}
pub(super) fn tick(adapter: &mut Adapter, runtime: &mut Runtime, app: &mut App) {
    if !runtime.loaded || runtime.saving || runtime.persistence_uncertain {
        return;
    }
    if adapter.stremio.flight.is_some() && !runtime.page.starts_with("stremio-") {
        // Generic navigation/provider switching can bypass this adapter's Back
        // action. Retire only the old metadata operation, without touching the
        // destination screen's status or waiting for its obsolete deadline.
        adapter.cancel_media(runtime);
    }
    let observed = runtime
        .saved
        .settings
        .get(KEY)
        .cloned()
        .unwrap_or(Value::Null);
    if adapter.stremio.observed.as_ref() != Some(&observed) {
        // A committed profile/configuration edit retires the captured authority.
        if adapter.stremio.observed.is_some()
            && (adapter.stremio.flight.is_some()
                || runtime.page.starts_with("stremio-")
                || (adapter.media_provider == "stremio"
                    && (adapter.playing
                        || adapter.inspecting
                        || adapter.session.is_some()
                        || adapter.media_deadline > 0)))
        {
            adapter.navigate(runtime, app);
        }
        let config = Config::parse(&observed);
        adapter.stremio = State {
            observed: Some(observed),
            ..State::default()
        };
        match config {
            Ok(config) => adapter.stremio.config = config,
            Err(_) if runtime.page.starts_with("stremio-") => {
                runtime.status = "Invalid saved Stremio settings".into()
            }
            Err(_) => {}
        }
        runtime.changed();
    }
    if adapter
        .stremio
        .flight
        .as_ref()
        .is_some_and(|flight| runtime.now_ms() >= flight.expires)
    {
        adapter.navigate(runtime, app);
        runtime.status = "Add-on request timed out. Retry or go back.".into();
        runtime.changed();
    }
}
pub(super) fn complete(adapter: &mut Adapter, runtime: &mut Runtime, fetched: Fetched) {
    let Some(flight) = adapter.stremio.flight.as_ref() else {
        return;
    };
    if fetched.generation != adapter.generation
        || flight.generation != fetched.generation
        || flight.expires != fetched.expires
        || runtime.now_ms() >= fetched.expires
        || flight.config != fetched.config
        || saved_config(runtime).ok().as_ref() != Some(&fetched.config)
        || adapter.cancelled.load(Ordering::Acquire)
        || !runtime.page.starts_with("stremio-")
    {
        return;
    }
    adapter.stremio.flight = None;
    // Publication retires every action rendered for the loading snapshot.
    adapter.generation = adapter.generation.wrapping_add(1);
    match fetched.result {
        Err(error) => runtime.status = error.into(),
        Ok(Payload::Catalogs { loaded, failed, builtin, builtin_failed }) => {
            adapter.stremio.loaded = loaded;
            adapter.stremio.builtin_categories = builtin;
            runtime.page = "stremio-categories";
            runtime.status = if builtin_failed {
                "Built-in catalog unavailable. Retry or check its service URL in Settings.".into()
            } else if failed > 0 {
                format!("{failed} add-on(s) unavailable; Retry to try again")
            } else if adapter.stremio.builtin_categories.is_empty()
                && !adapter.stremio.loaded.iter().any(|loaded| loaded.manifest.catalogs().iter()
                    .any(|catalog| catalog.can_browse || catalog.can_search)) {
                "Add a catalog source or configure the catalog service in Settings".into()
            } else {
                String::new()
            };
        }
        Ok(Payload::Page {
            selection,
            offset,
            page,
        }) => {
            if offset == 0 {
                adapter.stremio.movies.clear();
            }
            let before = adapter.stremio.movies.len();
            let received = page.received_count;
            for movie in page.movies {
                if adapter.stremio.movies.len() >= plx_data::ott::MAX_CATALOG_ENTRIES {
                    runtime.status = "Catalog limit reached. Narrow the search.".into();
                    adapter.stremio.next = None;
                    runtime.changed();
                    return;
                }
                if !adapter.stremio.movies.iter().any(|old| old.id == movie.id) {
                    adapter.stremio.movies.push(movie);
                }
            }
            // A repeated page must not leave Load more looping forever.
            adapter.stremio.next = if adapter.stremio.movies.len() == before
                || adapter.stremio.movies.len() >= plx_data::ott::MAX_CATALOG_ENTRIES
            {
                None
            } else {
                page.next_offset
            };
            adapter.stremio.selected = Some(selection);
            adapter.stremio.selected_builtin = None;
            adapter.stremio.builtin_next = None;
            runtime.page = "stremio-movies";
            runtime.status = if received > 0 && adapter.stremio.movies.len() == before {
                "Catalog repeated the same page. Narrow the search.".into()
            } else {
                format!("{} videos", adapter.stremio.movies.len())
            };
        }
        Ok(Payload::BuiltinPage { selection, page, result }) => {
            if page == 1 { adapter.stremio.movies.clear(); }
            let before = adapter.stremio.movies.len();
            for movie in result.movies {
                if adapter.stremio.movies.len() >= plx_data::ott::MAX_CATALOG_ENTRIES { break; }
                if !adapter.stremio.movies.iter().any(|old| old.id == movie.metadata.id) {
                    adapter.stremio.movies.push(movie.metadata);
                }
            }
            adapter.stremio.builtin_next = if adapter.stremio.movies.len() > before
                && adapter.stremio.movies.len() < plx_data::ott::MAX_CATALOG_ENTRIES { result.next_page } else { None };
            adapter.stremio.selected_builtin = Some(selection);
            adapter.stremio.selected = None;
            adapter.stremio.next = None;
            runtime.page = "stremio-movies";
            runtime.status = if adapter.stremio.movies.len() >= plx_data::ott::MAX_CATALOG_ENTRIES {
                "Catalog limit reached. Narrow the search.".into()
            } else if result.received_count > 0 && adapter.stremio.movies.len() == before {
                "Catalog repeated the same page. Narrow the search.".into()
            } else { format!("{} videos", adapter.stremio.movies.len()) };
        }
        Ok(Payload::Streams {
            metadata,
            candidates,
            failed,
            unsupported,
            duplicates,
            rejected,
        }) => {
            if let Some(metadata) = metadata {
                adapter.stremio.movie_title = metadata.title.clone();
                if let Some(movie) = adapter.stremio.movies.iter_mut().find(|movie| movie.id == metadata.id) { *movie = metadata; }
            }
            adapter.stremio.candidates = candidates;
            adapter.stremio.choice = None;
            runtime.page = "stremio-streams";
            let invalid = rejected.iter().filter(|(_, reason)| {
                !matches!(reason, streams::Rejection::Unsupported(_))
            }).count();
            runtime.status = if adapter.stremio.candidates.is_empty() {
                if failed > 0 {
                    "Stream add-ons unavailable. Retry or check Settings.".into()
                } else if invalid > 0 {
                    "Stream add-ons returned invalid sources. Retry or check Settings.".into()
                } else if unsupported > 0 {
                    "The returned sources require unsupported playback features".into()
                } else {
                    "No sources available for this video".into()
                }
            } else if invalid > 0 {
                format!("{failed} unavailable add-on(s), {unsupported} unsupported source(s), {invalid} invalid source(s)")
            } else if failed > 0 || unsupported > 0 {
                format!("{failed} unavailable add-on(s), {unsupported} unsupported source(s)")
            } else if duplicates > 0 {
                format!("Choose a source · {duplicates} duplicate(s) omitted")
            } else {
                "Choose a source".into()
            };
            if let Some((position, reason)) = rejected.first() {
                runtime.status.push_str(&format!(
                    " · Source {}: {}",
                    position + 1,
                    rejection_message(*reason)
                ));
            }
        }
    }
    runtime.changed();
}

fn rejection_message(reason: streams::Rejection) -> &'static str {
    match reason {
        streams::Rejection::Unsupported(streams::Feature::ExternalPlayer) => {
            "external player required"
        }
        streams::Rejection::Unsupported(streams::Feature::Headers) => {
            "custom headers not supported"
        }
        streams::Rejection::Unsupported(streams::Feature::Drm) => "protected source not supported",
        streams::Rejection::Unsupported(streams::Feature::Archive) => {
            "archive source not supported"
        }
        streams::Rejection::InvalidFileIndex => "invalid file selection",
        streams::Rejection::InvalidTrackers => "invalid tracker list",
        streams::Rejection::SourceLimit => "source limit reached",
        streams::Rejection::InvalidShape | streams::Rejection::InvalidSource => "invalid source",
    }
}
fn key(adapter: &Adapter, verb: &str, index: usize) -> String {
    format!("stremio.{verb}:{}:{index}", adapter.generation)
}
fn index(adapter: &Adapter, key: &str, verb: &str) -> Result<usize, &'static str> {
    let suffix = key
        .strip_prefix(&format!("stremio.{verb}:"))
        .ok_or("Invalid Stremio action")?;
    let (generation, index) = suffix.split_once(':').ok_or("Invalid Stremio action")?;
    if generation.parse::<u64>().ok() != Some(adapter.generation) {
        return Err("This selection is no longer current");
    }
    index.parse().map_err(|_| "Invalid Stremio selection")
}
fn catalogs(state: &State) -> Vec<(Loaded, catalog::Catalog)> {
    state
        .loaded
        .iter()
        .flat_map(|loaded| {
            loaded
                .manifest
                .catalogs()
                .iter()
                .filter(|catalog| catalog.can_browse || catalog.can_search)
                .map(move |catalog| (loaded.clone(), catalog.clone()))
        })
        .collect()
}
pub(super) fn rows(runtime: &Runtime) -> Vec<OttRow> {
    match runtime.page {
        "providers" => vec![button("stremio.open", "Stremio")],
        "settings" => vec![button("stremio.settings", "Stremio sources")],
        _ => Vec::new(),
    }
}
pub(super) fn view(runtime: &Runtime) -> Option<OttView> {
    let adapter = runtime.companion_adapter.as_ref()?;
    let state = &adapter.stremio;
    let (title, mut rows) = match runtime.page {
        "stremio-categories" => ("Stremio categories", {
            let mut rows = state.builtin_categories.iter().enumerate().map(|(i, category)| {
                row(key(adapter,"builtin",i),category.title.clone(),"Built-in catalog".into(),
                    OttAction::EditSetting(key(adapter,"builtin",i)))
            }).collect::<Vec<_>>();
            if state.config.catalog_service_url.is_some() {
                rows.push(button(&key(adapter,"builtin-search",0),"Search built-in catalog"));
            }
            rows.extend(catalogs(state).iter().enumerate().map(|(i,(loaded,catalog))| {
                row(key(adapter,"catalog",i),catalog.title.clone(),
                    format!("{} · {}",loaded.manifest.name,loaded.manifest.id),
                    OttAction::EditSetting(key(adapter,"catalog",i)))
            }));
            rows
        }),
        "stremio-movies" => (
            "Stremio videos",
            state
                .movies
                .iter()
                .enumerate()
                .map(|(i, movie)| {
                    let mut item = row(
                        key(adapter, "movie", i),
                        format!("{}  {}", i + 1, movie.title),
                        movie.release_info.clone().unwrap_or_default(),
                        OttAction::EditSetting(key(adapter, "movie", i)),
                    );
                    item.media = Some(plx_screens::ott::OttMedia {
                        kind: plx_screens::ott::OttMediaKind::Movie,
                        duration_ms: None,
                        progress: None,
                    });
                    item.description = movie.description.clone();
                    // Keep image URI validation at the normal OTT artwork boundary.
                    item.poster = movie
                        .artwork
                        .as_ref()
                        .and_then(|art| media_url(art.candidate()))
                        .map(|url| OttPoster::Http { url });
                    item
                })
                .collect(),
        ),
        "stremio-streams" => (
            "Choose source",
            state
                .candidates
                .iter()
                .enumerate()
                .map(|(i, candidate)| {
                    let mut item = row(
                        key(adapter, "source", i),
                        candidate.title.clone(),
                        candidate.description.clone(),
                        OttAction::EditSetting(key(adapter, "source", i)),
                    );
                    if matches!(&candidate.source, SourceProposal::Torrent(_)) {
                        let snapshot = runtime.companion.snapshot();
                        item.enabled = snapshot.ready && snapshot.torrent;
                        if !item.enabled {
                            item.detail = "Enable the local torrent service in Settings".into();
                        }
                    }
                    item
                })
                .collect(),
        ),
        "stremio-settings" => ("Stremio sources", {
            let mut rows = vec![
                button("stremio.add", "Add source (manifest URL)"),
                button("stremio.catalog-service", if state.config.catalog_service_url.is_some() {
                    "Built-in catalog service: configured"
                } else { "Configure built-in catalog service" }),
                button(
                    "stremio.language",
                    &format!("Catalog language: {}", state.config.language),
                ),
            ];
            for (i, addon) in state.config.addons.iter().enumerate() {
                rows.push(button(
                    &key(adapter, "toggle", i),
                    &format!(
                        "{}: {}",
                        addon.label,
                        if addon.enabled { "Enabled" } else { "Disabled" }
                    ),
                ));
                rows.push(button(
                    &key(adapter, "remove", i),
                    &format!("Remove {}", addon.label),
                ));
            }
            rows
        }),
        _ => return None,
    };
    if runtime.page == "stremio-movies" {
        if state.next.is_some() || state.builtin_next.is_some() {
            rows.push(button(&key(adapter, "more", 0), "Load more"));
        }
        if state.selected_builtin.is_some() || state.selected.as_ref().is_some_and(|selection| {
            selection
                .addon
                .manifest
                .catalogs()
                .iter()
                .any(|catalog| catalog.id == selection.catalog && catalog.can_search)
        }) {
            rows.push(button(&key(adapter, "search-open", 0), "Search"));
        }
    }
    if runtime.page != "stremio-settings" {
        rows.push(button("stremio.settings", "Settings"));
    }
    if runtime.page == "stremio-streams" {
        rows.push(button("stremio.service", "Local torrent service"));
    }
    if state.retry.is_some() && state.flight.is_none() && runtime.page != "stremio-settings" {
        rows.push(
            if runtime.page == "stremio-streams" && state.choice.is_some() {
                button(&key(adapter, "playagain", 0), "Retry playback")
            } else {
                button(&key(adapter, "retry", 0), "Retry")
            },
        );
    }
    if state.flight.is_some() {
        for item in &mut rows {
            item.enabled = false;
        }
    }
    rows.push(button("stremio.back", "Cancel / Back"));
    Some(OttView { status_working: false, classic_preferences: Default::default(), volume_available: false, category_label: None,
        preview_context: None,
        order_context: None, random_context: None,
        catalog_progress: None,
        search: None,
        page_id: runtime.page.into(),
        revision: runtime.revision,
        title: title.into(),
        description: if runtime.page == "stremio-settings" {
            "Language applies to the built-in catalog and supported add-ons. The optional catalog service uses a separate URL ending in /tmdb/s/.".into()
        } else {
            state.movie_title.clone()
        },
        status: runtime.status.clone(),
        rows,
        selected_row: runtime.selected.clone(),
    })
}

fn persist(
    adapter: &mut Adapter,
    runtime: &mut Runtime,
    app: &mut App,
    config: Config,
) -> Result<(), &'static str> {
    config.validate().map_err(|_| "Invalid Stremio settings")?;
    let mut saved = runtime.saved.clone();
    saved.settings.insert(KEY.into(), config.to_value());
    runtime.save(saved, None, false)?;
    adapter.navigate(runtime, app);
    runtime.status = "Saving…".into();
    Ok(())
}
fn back(adapter: &mut Adapter, runtime: &mut Runtime, app: &mut App) {
    let page = runtime.page;
    adapter.navigate(runtime, app);
    runtime.page = match page {
        "stremio-streams" => "stremio-movies",
        "torrent-files" | "torrent-settings" => "stremio-streams",
        "stremio-movies" | "stremio-settings" => "stremio-categories",
        _ => "providers",
    };
    adapter.stremio.retry = match runtime.page {
        "stremio-movies" => match adapter.stremio.selected_builtin.clone() {
            Some(selection) => Some(Operation::BuiltinPage { selection, page: 1 }),
            None => adapter.stremio.selected.clone().map(|selection| Operation::Page { selection, offset: 0 }),
        },
        "stremio-categories" => Some(Operation::Catalogs),
        "stremio-streams" => adapter
            .stremio
            .retry
            .clone()
            .filter(|op| matches!(op, Operation::Streams { .. } | Operation::BuiltinStreams { .. })),
        _ => None,
    };
    adapter.stremio.service_return = false;
    runtime.status.clear();
    runtime.changed();
}
fn page(
    adapter: &mut Adapter,
    runtime: &mut Runtime,
    app: &mut App,
    selection: Selection,
    offset: usize,
) -> Result<(), &'static str> {
    adapter.stremio.selected = Some(selection.clone());
    adapter.stremio.selected_builtin = None;
    adapter.stremio.builtin_next = None;
    runtime.page = "stremio-movies";
    begin(adapter, runtime, app, Operation::Page { selection, offset })
}
fn builtin_page(adapter: &mut Adapter, runtime: &mut Runtime, app: &mut App,
    selection: BuiltinSelection, page: u16) -> Result<(), &'static str> {
    adapter.stremio.selected_builtin = Some(selection.clone());
    adapter.stremio.selected = None;
    adapter.stremio.next = None;
    runtime.page = "stremio-movies";
    begin(adapter, runtime, app, Operation::BuiltinPage { selection, page })
}

/// Plain stream proposals cannot name the process's loopback control surface or
/// impersonate a helper-owned capability. HTTP media remains an ordinary source.
fn media_url(url: &str) -> Option<String> {
    let canonical = plx_data::ott::http_url(url)?;
    let (origin, path) = plx_plex::plex::origin::split(&canonical);
    plx_media::curlio::media_request_allowed(&origin, path, &[]).then_some(canonical)
}
pub(super) fn admitted_source(source: &ExternalSource) -> bool {
    match source {
        ExternalSource::Http(url) | ExternalSource::Hls(url) => media_url(url).is_some(),
        _ => false,
    }
}
fn torrent_request(proposal: &streams::TorrentProposal) -> Result<Request, &'static str> {
    let file_index = match proposal.file {
        FileChoice::Largest => None,
        FileChoice::Index(index) if index < 4096 => Some(index as usize),
        FileChoice::Index(_) => return Err("Torrent file index exceeds the local service limit"),
    };
    Ok(Request::Create {
        source: CreateSource::Direct(proposal.magnet_candidate()),
        file_index,
    })
}
fn play(
    adapter: &mut Adapter,
    runtime: &mut Runtime,
    app: &mut App,
    i: usize,
) -> Result<(), &'static str> {
    current_configuration(adapter, runtime)?;
    if !playback_allowed(app) {
        return Err("policy_restricted");
    }
    let candidate = adapter
        .stremio
        .candidates
        .get(i)
        .ok_or("Source no longer available")?
        .clone();
    let title = adapter.stremio.movie_title.clone();
    match candidate.source {
        SourceProposal::Torrent(proposal) => {
            let snapshot = runtime.companion.snapshot();
            if !snapshot.ready || !snapshot.torrent {
                return Err("Enable the local torrent service first");
            }
            let request = torrent_request(&proposal)?;
            let result = adapter.begin_media(runtime, app, title, request, Target::Session);
            adapter.media_provider = "stremio";
            adapter.stremio.choice = Some(i);
            if let Err(error) = result {
                adapter.cancel_media(runtime);
                runtime.page = "stremio-streams";
                return Err(error);
            }
        }
        SourceProposal::Http { url } => {
            let source =
                ExternalSource::Http(media_url(&url).ok_or("Unsupported media destination")?);
            adapter.stop_owned_playback(runtime, app);
            adapter.cancel_media(runtime);
            runtime.stop_catalog(app);
            runtime.selected = None;
            adapter.media_generation = runtime.play_generation;
            adapter.route_ticket = Some(plx_media::route::worker_ticket());
            adapter.media_deadline = runtime.now_ms().saturating_add(120_000);
            adapter.title = title;
            adapter.direct_source = Some(source.clone());
            if let Err(error) = adapter.launch(Work::Inspect(source)) {
                adapter.cancel_media(runtime);
                return Err(error);
            }
            adapter.inspecting = true;
            runtime.status = "Opening video…".into();
        }
    }
    adapter.media_provider = "stremio";
    adapter.stremio.choice = Some(i);
    runtime.changed();
    Ok(())
}
pub(super) fn action(
    runtime: &mut Runtime,
    app: &mut App,
    action: &OttAction,
) -> Option<Result<(), &'static str>> {
    let claimed = match action {
        OttAction::EditSetting(key) => {
            key.starts_with("stremio.")
                || (matches!(key.as_str(), "torrent.back" | "torrent.retry")
                    && matches!(runtime.page, "torrent-files" | "torrent-settings")
                    && runtime.companion_adapter.as_ref().is_some_and(|a| {
                        a.media_provider == "stremio" && runtime.page == "torrent-files"
                            || a.stremio.service_return && runtime.page == "torrent-settings"
                    }))
        }
        OttAction::SubmitSetting { session, key, .. } => {
            *session == 0 && key.starts_with("stremio.")
        }
        OttAction::Back | OttAction::ShowCategories | OttAction::Refresh => {
            runtime.page.starts_with("stremio-")
                || (matches!(runtime.page, "torrent-files" | "torrent-settings")
                    && runtime.companion_adapter.as_ref().is_some_and(|a| {
                        a.media_provider == "stremio" && runtime.page == "torrent-files"
                            || a.stremio.service_return && runtime.page == "torrent-settings"
                    }))
        }
        _ => false,
    };
    if !claimed {
        return None;
    }
    let mut adapter = runtime.companion_adapter.take().unwrap_or_default();
    let mapped;
    let action = match action {
        OttAction::EditSetting(key) if key == "torrent.back" => {
            mapped = OttAction::Back;
            &mapped
        }
        OttAction::EditSetting(key) if key == "torrent.retry" => {
            mapped = OttAction::Refresh;
            &mapped
        }
        _ => action,
    };
    let result = handle(&mut adapter, runtime, app, action);
    runtime.companion_adapter = Some(adapter);
    Some(result)
}
fn action_page_allowed(page: &str, action: &OttAction) -> bool {
    match action {
        OttAction::EditSetting(key) => match key.as_str() {
            "stremio.open" => page == "providers",
            "stremio.settings" => matches!(page, "settings" | "stremio-categories" | "stremio-movies" | "stremio-streams"),
            "stremio.service" => page == "stremio-streams",
            "stremio.back" => page.starts_with("stremio-"),
            "stremio.add" | "stremio.language" | "stremio.catalog-service" => page == "stremio-settings",
            key if key.starts_with("stremio.catalog:") || key.starts_with("stremio.builtin:") || key.starts_with("stremio.builtin-search:") => page == "stremio-categories",
            key if key.starts_with("stremio.movie:") || key.starts_with("stremio.more:") || key.starts_with("stremio.search-open:") => page == "stremio-movies",
            key if key.starts_with("stremio.source:") || key.starts_with("stremio.playagain:") => page == "stremio-streams",
            key if key.starts_with("stremio.toggle:") || key.starts_with("stremio.remove:") => page == "stremio-settings",
            key if key.starts_with("stremio.retry:") => matches!(page, "stremio-categories" | "stremio-movies" | "stremio-streams"),
            _ => false,
        },
        OttAction::SubmitSetting { key, .. } => {
            if key.starts_with("stremio.search:") { matches!(page, "stremio-categories" | "stremio-movies") }
            else { page == "stremio-settings" && (key.starts_with("stremio.add:") || key.starts_with("stremio.language:") || key.starts_with("stremio.catalog-service:")) }
        }
        // The outer adapter already restricts these to its visible pages and
        // maps the shared torrent Back/Refresh buttons before calling handle.
        OttAction::Back | OttAction::ShowCategories | OttAction::Refresh => true,
        _ => false,
    }
}

fn handle(
    adapter: &mut Adapter,
    runtime: &mut Runtime,
    app: &mut App,
    action: &OttAction,
) -> Result<(), &'static str> {
    if !action_page_allowed(runtime.page, action) { return Err("This Stremio screen is no longer active"); }
    if adapter.stremio.flight.is_some()
        && matches!(action,
        OttAction::EditSetting(key) if key != "stremio.back" && key != "stremio.settings")
    {
        return Err("Wait for the current request or go back");
    }
    match action {
        OttAction::Back => back(adapter, runtime, app),
        OttAction::ShowCategories => {
            adapter.navigate(runtime, app);
            adapter.stremio.retry = Some(Operation::Catalogs);
            runtime.page = "stremio-categories";
            runtime.status.clear();
        }
        OttAction::Refresh => {
            if runtime.page == "torrent-settings" && adapter.stremio.service_return {
                back(adapter, runtime, app);
            } else if runtime.page == "torrent-files" {
                let choice = adapter.stremio.choice.ok_or("Choose a source first")?;
                play(adapter, runtime, app, choice)?;
            } else {
                let operation = adapter.stremio.retry.clone().unwrap_or(Operation::Catalogs);
                begin(adapter, runtime, app, operation)?;
            }
        }
        OttAction::EditSetting(key_value) => match key_value.as_str() {
            "stremio.open" => {
                adapter.stremio.config = saved_config(runtime)?;
                runtime.page = "stremio-categories";
                begin(adapter, runtime, app, Operation::Catalogs)?;
            }
            "stremio.back" => back(adapter, runtime, app),
            "stremio.service" => {
                adapter.navigate(runtime, app);
                adapter.stremio.service_return = true;
                runtime.page = "torrent-settings";
                runtime.status.clear();
            }
            value if value.starts_with("stremio.playagain:") => {
                index(adapter, value, "playagain")?;
                let i = adapter.stremio.choice.ok_or("Choose a source")?;
                play(adapter, runtime, app, i)?;
            }
            "stremio.settings" => {
                adapter.navigate(runtime, app);
                adapter.stremio.config = saved_config(runtime)?;
                runtime.page = "stremio-settings";
                runtime.status.clear();
            }
            "stremio.catalog-service" => adapter.editor(runtime,app,&key(adapter,"catalog-service",0),
                "Catalog service URL ending in /tmdb/s/ (blank: disable)",
                adapter.stremio.config.catalog_service_url.clone().unwrap_or_default(),true,4096,OttTextPolicy::Literal),
            "stremio.add" => adapter.editor(
                runtime,
                app,
                &key(adapter, "add", 0),
                "Add-on manifest URL",
                String::new(),
                true,
                4096,
                OttTextPolicy::Literal,
            ),
            "stremio.language" => adapter.editor(
                runtime,
                app,
                &key(adapter, "language", 0),
                "Catalog language",
                adapter.stremio.config.language.clone(),
                false,
                35,
                OttTextPolicy::Literal,
            ),
            value if value.starts_with("stremio.more:") => {
                index(adapter, value, "more")?;
                if let Some(selection) = adapter.stremio.selected_builtin.clone() {
                    let next = adapter.stremio.builtin_next.ok_or("No more videos")?;
                    builtin_page(adapter,runtime,app,selection,next)?;
                } else {
                    let selection = adapter.stremio.selected.clone().ok_or("Choose a catalog")?;
                    let next = adapter.stremio.next.ok_or("No more videos")?;
                    page(adapter, runtime, app, selection, next)?;
                }
            }
            value if value.starts_with("stremio.search-open:") => {
                index(adapter, value, "search-open")?;
                let query = if let Some(selection) = &adapter.stremio.selected_builtin {
                    selection.query.clone()
                } else { adapter.stremio.selected.as_ref().ok_or("Choose a catalog")?.query.clone() };
                adapter.editor(runtime,app,&key(adapter,"search",0),"Search videos",query.unwrap_or_default(),false,256,OttTextPolicy::Predictive);
            }
            value if value.starts_with("stremio.retry:") => {
                index(adapter, value, "retry")?;
                let operation = adapter.stremio.retry.clone().ok_or("Nothing to retry")?;
                begin(adapter, runtime, app, operation)?;
            }
            value if value.starts_with("stremio.builtin-search:") => {
                index(adapter,value,"builtin-search")?;
                if adapter.stremio.config.catalog_service_url.is_none() { return Err("Configure the catalog service first"); }
                adapter.stremio.selected = None;
                adapter.stremio.selected_builtin = Some(BuiltinSelection { category:"search".into(),query:None });
                adapter.editor(runtime,app,&key(adapter,"search",0),"Search built-in catalog",String::new(),false,256,OttTextPolicy::Predictive);
            }
            value if value.starts_with("stremio.builtin:") => {
                let i = index(adapter,value,"builtin")?;
                let category = adapter.stremio.builtin_categories.get(i).ok_or("Category no longer available")?.id.clone();
                builtin_page(adapter,runtime,app,BuiltinSelection { category,query:None },1)?;
            }
            value if value.starts_with("stremio.catalog:") => {
                let i = index(adapter, value, "catalog")?;
                let (addon, catalog) = catalogs(&adapter.stremio)
                    .get(i)
                    .cloned()
                    .ok_or("Catalog no longer available")?;
                let selection = Selection {
                    addon,
                    catalog: catalog.id,
                    query: None,
                };
                adapter.stremio.selected = Some(selection.clone());
                adapter.stremio.selected_builtin = None;
                adapter.stremio.builtin_next = None;
                if catalog.can_browse {
                    page(adapter, runtime, app, selection, 0)?;
                } else {
                    adapter.editor(
                        runtime,
                        app,
                        &key(adapter, "search", 0),
                        "Search videos",
                        String::new(),
                        false,
                        256,
                        OttTextPolicy::Predictive,
                    );
                }
            }
            value if value.starts_with("stremio.movie:") => {
                let movie = adapter
                    .stremio
                    .movies
                    .get(index(adapter, value, "movie")?)
                    .ok_or("Video no longer available")?;
                let media_id = movie.id.clone();
                adapter.stremio.movie_title = movie.title.clone();
                let addons: Vec<Loaded> = adapter
                    .stremio
                    .loaded
                    .iter()
                    .filter(|loaded| loaded.manifest.has_movie_streams())
                    .cloned()
                    .collect();
                adapter.stremio.candidates.clear();
                runtime.page = "stremio-streams";
                if addons.is_empty() {
                    adapter.navigate(runtime, app);
                    adapter.stremio.retry = None;
                    adapter.stremio.choice = None;
                    runtime.status = "Add a stream source in Settings to play this movie".into();
                } else {
                    let operation = if adapter.stremio.selected_builtin.is_some() {
                        Operation::BuiltinStreams { addons,stable_id:media_id }
                    } else { Operation::Streams { addons,media_ids:vec![media_id] } };
                    begin(adapter,runtime,app,operation)?;
                }
            }
            value if value.starts_with("stremio.source:") => {
                let choice = index(adapter, value, "source")?;
                play(adapter, runtime, app, choice)?
            }
            value
                if value.starts_with("stremio.toggle:") || value.starts_with("stremio.remove:") =>
            {
                let remove = value.starts_with("stremio.remove:");
                let i = index(adapter, value, if remove { "remove" } else { "toggle" })?;
                let mut config = saved_config(runtime)?;
                let addon = config
                    .addons
                    .get_mut(i)
                    .ok_or("Source no longer available")?;
                if remove {
                    config.addons.remove(i);
                } else {
                    addon.enabled = !addon.enabled;
                }
                persist(adapter, runtime, app, config)?;
            }
            _ => return Err("Unsupported Stremio action"),
        },
        OttAction::SubmitSetting {
            key: key_value,
            value,
            ..
        } => {
            let mut config = saved_config(runtime)?;
            if key_value.starts_with("stremio.catalog-service:") {
                index(adapter,key_value,"catalog-service")?;
                let url = value.trim();
                config.catalog_service_url = (!url.is_empty()).then(|| url.to_owned());
                persist(adapter,runtime,app,config)?;
            } else if key_value.starts_with("stremio.add:") {
                index(adapter, key_value, "add")?;
                let n = (1..=9)
                    .find(|i| !config.addons.iter().any(|a| a.id == format!("source-{i}")))
                    .ok_or("Source limit reached")?;
                config.addons.push(Addon {
                    id: format!("source-{n}"),
                    label: format!("Source {n}"),
                    manifest_url: value.trim().into(),
                    enabled: true,
                });
                persist(adapter, runtime, app, config)?;
            } else if key_value.starts_with("stremio.language:") {
                index(adapter, key_value, "language")?;
                config.language = crate::app::ott_stremio_settings::language(value.trim())
                    .map_err(|_| "Invalid language code")?;
                persist(adapter, runtime, app, config)?;
            } else if key_value.starts_with("stremio.search:") {
                index(adapter, key_value, "search")?;
                let query = value.trim();
                if query.is_empty() || query.len() > 256 || query.chars().any(char::is_control) {
                    return Err("Enter a search query");
                }
                if adapter.stremio.selected_builtin.is_some() {
                    let selection = BuiltinSelection { category:"search".into(),query:Some(query.into()) };
                    builtin_page(adapter,runtime,app,selection,1)?;
                } else {
                    let mut selection = adapter.stremio.selected.clone().ok_or("Choose a catalog")?;
                    selection.query = Some(query.into());
                    page(adapter, runtime, app, selection, 0)?;
                }
            } else {
                return Err("Unsupported Stremio setting");
            }
        }
        _ => return Err("Unsupported Stremio action"),
    }
    runtime.changed();
    Ok(())
}

#[cfg(test)]
#[path = "ott_stremio_adapter/tests.rs"]
mod tests;
