//! Selected-entry metadata is fetched off-frame and tagged for the runtime's generation check.
use plx_base::task::OffFrame;
use plx_data::ott::{
    favorites::Favorite, guide::{Context, Query, Request, ResultSet}, CatalogEntry, MediaLocator, ProviderKind, ProviderProfile,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::Sender,
    Arc,
};

#[path = "ott_metadata_guide.rs"]
mod guide_result;
pub(super) use guide_result::Guide;

pub(super) enum Event {
    Preview(Preview),
    Guide(Guide),
    Local { generation: u64, completion: super::ott_runtime::local_assets::Completion },
}

pub(super) struct Preview {
    pub generation: u64,
    pub profile_id: String,
    pub entry_id: String,
    pub entry: Option<CatalogEntry>,
    pub result: Result<Value, &'static str>,
}

enum Fetched {
    Preview(Result<Value, &'static str>),
    Guide {
        source_profile: ProviderProfile,
        request: Request,
        result: Result<ResultSet, &'static str>,
        row_guide: Option<super::ott_runtime::row_guide::Reply>,
    },
}

pub(super) fn start_local(
    work: super::ott_runtime::local_assets::Work,
    generation: u64,
    cancel: Arc<AtomicBool>,
    tx: Sender<Event>,
    done: Arc<AtomicBool>,
) -> bool {
    super::ott_worker::spawn("ott-local-assets", done, move |worker| {
        if cancel.load(Ordering::Acquire) { return; }
        let failed = work.clone();
        let completion = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work.run(worker, &cancel)))
            .unwrap_or_else(|_| failed.failure(worker));
        if !cancel.load(Ordering::Acquire) && tx.send(Event::Local { generation, completion }).is_ok() {
            plx_machine::idle::invalidate();
        }
    })
}

pub(super) fn start(
    profile: ProviderProfile,
    entry: CatalogEntry,
    generation: u64,
    query: Query,
    row_request: Option<super::ott_runtime::row_guide::Request>,
    cancel: Arc<AtomicBool>,
    tx: Sender<Event>,
    done: Arc<AtomicBool>,
) -> bool {
    super::ott_worker::spawn("ott-metadata", done, move |worker| {
        if run(&profile, &entry, generation, &cancel, &tx, || {
            metadata(worker, &profile, &entry, generation, query, row_request, &cancel)
        }) {
            plx_machine::idle::invalidate();
        }
    })
}

enum FavoriteSource {
    Managed(ProviderProfile, super::adapters::session::ManagedRead),
    Classic(ProviderProfile, super::ott_plex::ClassicRead),
    Profile(ProviderProfile, Option<super::ott_runtime::LocalFavoritePermit>),
    M3uAttachment(super::ott_m3u_attachment::Receipt),
}

impl FavoriteSource {
    fn owner(&self) -> &ProviderProfile {
        match self {
            Self::Profile(profile, _) | Self::Managed(profile, _) | Self::Classic(profile, _) => profile,
            Self::M3uAttachment(receipt) => receipt.parent(),
        }
    }

    fn inspect(&self, worker: &OffFrame, favorite: &Favorite, entry: &CatalogEntry,
        generation: u64, query: Query, cancel: &AtomicBool) -> Fetched {
        match self {
            Self::Profile(profile, _) | Self::Managed(profile, _) | Self::Classic(profile, _) => metadata(worker, profile, entry, generation, query, None, cancel),
            Self::M3uAttachment(receipt) => {
                match super::ott_runtime::attachment_profile(receipt, favorite) {
                    Ok(effective) => metadata(worker, &effective, entry, generation, query, None, cancel),
                    Err(error) => Fetched::Preview(Err(error)),
                }
            }
        }
    }

}

pub(super) fn start_favorite(
    profile: ProviderProfile,
    favorite: Favorite,
    local_proof: Option<super::ott_runtime::LocalFavoritePermit>,
    generation: u64,
    query: Query,
    cancel: Arc<AtomicBool>,
    tx: Sender<Event>,
    done: Arc<AtomicBool>,
) -> bool {
    start_favorite_source(FavoriteSource::Profile(profile, local_proof), favorite,
        generation, query, cancel, tx, done)
}

pub(super) fn start_attachment_favorite(
    receipt: super::ott_m3u_attachment::Receipt,
    favorite: Favorite,
    generation: u64,
    query: Query,
    cancel: Arc<AtomicBool>,
    tx: Sender<Event>,
    done: Arc<AtomicBool>,
) -> bool {
    start_favorite_source(FavoriteSource::M3uAttachment(receipt), favorite,
        generation, query, cancel, tx, done)
}

pub(super) fn start_managed_favorite(
    profile: ProviderProfile, favorite: Favorite, read: super::adapters::session::ManagedRead,
    generation: u64, query: Query, cancel: Arc<AtomicBool>, tx: Sender<Event>, done: Arc<AtomicBool>,
) -> bool {
    start_favorite_source(FavoriteSource::Managed(profile, read), favorite, generation, query, cancel, tx, done)
}

pub(super) fn start_classic_favorite(
    profile: ProviderProfile, favorite: Favorite, read: super::ott_plex::ClassicRead,
    generation: u64, query: Query, cancel: Arc<AtomicBool>, tx: Sender<Event>, done: Arc<AtomicBool>,
) -> bool {
    start_favorite_source(FavoriteSource::Classic(profile, read), favorite, generation, query, cancel, tx, done)
}

// Reject non-Plex locators before the shared lookup can fall back to scanning a catalogue.
fn classic_favorite_machine<'a>(profile: &ProviderProfile, favorite: &'a Favorite) -> Result<&'a str, &'static str> {
    profile.validate().map_err(|_| "Invalid provider profile")?;
    match &favorite.locator {
        plx_data::ott::favorites::Locator::Plex { server_id, .. }
            if profile.kind == ProviderKind::Plex && *server_id == profile.source => Ok(server_id),
        _ => Err("Favorite Plex source does not match"),
    }
}

fn start_favorite_source(
    source: FavoriteSource, favorite: Favorite, generation: u64, query: Query,
    cancel: Arc<AtomicBool>, tx: Sender<Event>, done: Arc<AtomicBool>,
) -> bool {
    super::ott_worker::spawn("ott-fav-preview", done, move |worker| {
        if run_favorite(
            source.owner(), &favorite, generation, &cancel, &tx,
            || match &source {
                FavoriteSource::Profile(profile, proof) => {
                    let entry = super::ott_catalog::find_favorite(worker, profile, &favorite, proof.as_ref(), &cancel)?;
                    hydrate_plex(profile, entry, &cancel)
                }
                FavoriteSource::Managed(profile, read) => {
                    if !read.current() { return Err("Plex source is no longer available"); }
                    let entry = super::ott_catalog::find_favorite(worker, profile, &favorite, None, &cancel)?;
                    hydrate_plex_with_read(profile, entry, &cancel, Some(PlexRead::Managed(read)))
                }
                FavoriteSource::Classic(profile, read) => {
                    let machine = classic_favorite_machine(profile, &favorite)?;
                    if !read.matches(machine) { return Err("Favorite Plex source does not match"); }
                    if !read.current() { return Err("Plex source is no longer available"); }
                    let entry = super::ott_catalog::find_favorite(worker, profile, &favorite, None, &cancel)?;
                    hydrate_plex_with_read(profile, entry, &cancel, Some(PlexRead::Classic(read)))
                }
                FavoriteSource::M3uAttachment(receipt) =>
                    super::ott_catalog::find_attachment_favorite(worker, receipt, &favorite, &cancel),
            },
            |entry| source.inspect(worker, &favorite, entry, generation, query, &cancel),
        ) {
            plx_machine::idle::invalidate();
        }
    })
}

fn metadata(
    worker: &OffFrame,
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    generation: u64,
    query: Query,
    row_request: Option<super::ott_runtime::row_guide::Request>,
    cancel: &AtomicBool,
) -> Fetched {
    let guide = match (&profile.kind, &entry.media) {
        (ProviderKind::M3u, MediaLocator::Http { .. }) => entry.duration_ms.is_none()
            && plx_data::ott::m3u_source_kind(&profile.source) == Ok(plx_data::ott::M3uSourceKind::Playlist),
        (ProviderKind::Stalker, MediaLocator::StalkerApi { .. }) => true,
        (ProviderKind::Xtream, MediaLocator::Xtream { stream_kind, .. }) => stream_kind == "live",
        (ProviderKind::Stalker, MediaLocator::Stalker { stream_kind, .. }) => stream_kind == "itv",
        _ => false,
    };
    if guide {
        let request = Request {
            context: Context {
                generation,
                profile_id: profile.id.clone(),
                entry_id: entry.id.clone(),
            },
            query,
        };
        let source_profile = profile.clone();
        let (result, row_guide) = if let Some(row_request) = row_request {
            let (result, outcome) = super::ott_provider_http::guide_rows_cancellable(
                worker, profile, entry, &request, &row_request.selectors, cancel);
            (result, Some(super::ott_runtime::row_guide::Reply {
                request: row_request, outcome,
            }))
        } else {
            (super::ott_provider_http::guide_cancellable(worker, profile, entry, &request, cancel), None)
        };
        return Fetched::Guide { source_profile, request, result, row_guide };
    }
    let now = match query {
        Query::CurrentNext { now } => now,
        Query::Day { .. } => return Fetched::Preview(Err("Day guide is unavailable for this item")),
        Query::Records { .. } => return Fetched::Preview(Err("Records are unavailable for this item")),
    };
    if matches!(
        profile.kind,
        ProviderKind::M3u | ProviderKind::Xtream | ProviderKind::Stalker
    ) {
        Fetched::Preview(super::ott_provider_http::metadata(
            worker, profile, entry, now, cancel,
        ))
    } else {
        Fetched::Preview(Ok(
            json!({"description":entry.description,"poster":entry.poster,
            "durationSeconds":entry.duration_ms.map(|ms|ms as f64/1000.0)}),
        ))
    }
}

/// Favorites retain portable identity, not a stale metadata snapshot. Plex's
/// direct identity lookup therefore needs its read-only metadata request here.
fn hydrate_plex(
    profile: &ProviderProfile,
    entry: CatalogEntry,
    cancel: &AtomicBool,
) -> Result<CatalogEntry, &'static str> {
    hydrate_plex_with_read(profile, entry, cancel, None)
}

enum PlexRead<'a> {
    Managed(&'a super::adapters::session::ManagedRead),
    Classic(&'a super::ott_plex::ClassicRead),
}
impl PlexRead<'_> {
    fn matches(&self, machine: &str) -> bool {
        match self { Self::Managed(read) => read.matches(machine), Self::Classic(read) => read.matches(machine) }
    }
    fn metadata(&self, rating_key: &str) -> Option<plx_plex::plex::Metadata> {
        match self { Self::Managed(read) => read.metadata(rating_key), Self::Classic(read) => read.metadata(rating_key) }
    }
}

fn hydrate_plex_with_read(
    profile: &ProviderProfile, mut entry: CatalogEntry, cancel: &AtomicBool,
    read: Option<PlexRead<'_>>,
) -> Result<CatalogEntry, &'static str> {
    let MediaLocator::Plex {
        server_id,
        rating_key,
    } = &entry.media
    else {
        return Ok(entry);
    };
    if cancel.load(Ordering::Acquire) {
        return Err("Metadata request cancelled");
    }
    if profile.kind != ProviderKind::Plex || *server_id != profile.source {
        return Err("Favorite Plex source does not match");
    }
    if rating_key.is_empty()
        || rating_key.len() > 256
        || !rating_key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err("Invalid Plex item identity");
    }
    let item = if let Some(read) = read {
        if !read.matches(server_id) { return Err("Favorite Plex source does not match"); }
        read.metadata(rating_key)
    } else {
        let sid = plx_plex::plex::id_of_machine(server_id).ok_or("Plex server is not connected")?;
        let client = plx_plex::plex::client_for(sid).ok_or("Plex server authorization is unavailable")?;
        client.metadata_bounded(rating_key)
    }.ok_or("Plex item metadata is unavailable")?;
    if cancel.load(Ordering::Acquire) {
        return Err("Metadata request cancelled");
    }
    if item.rating_key != *rating_key {
        return Err("Plex returned a different item");
    }
    if item.summary.len() > plx_data::ott::MAX_METADATA_BYTES
        || item.thumb.len() > 16384
        || item.art.len() > 16384
    {
        return Err("Plex item metadata exceeds its limit");
    }
    entry.description = item.summary;
    entry.poster = if item.thumb.is_empty() {
        item.art
    } else {
        item.thumb
    };
    entry.duration_ms = u64::try_from(item.duration).ok().filter(|ms| *ms > 0);
    Ok(entry)
}

fn run_favorite(
    profile: &ProviderProfile,
    favorite: &Favorite,
    generation: u64,
    cancel: &AtomicBool,
    tx: &Sender<Event>,
    lookup: impl FnOnce() -> Result<CatalogEntry, &'static str>,
    inspect: impl FnOnce(&CatalogEntry) -> Fetched,
) -> bool {
    publish(
        profile,
        format!("favorite:{}", favorite.identity()),
        generation,
        cancel,
        tx,
        || match lookup() {
            Ok(entry) => {
                if cancel.load(Ordering::Acquire) {
                    return (None, Fetched::Preview(Err("Metadata request cancelled")));
                }
                let result = inspect(&entry);
                (Some(entry), result)
            }
            Err(error) => (None, Fetched::Preview(Err(error))),
        },
    )
}

/// Cancellation checks avoid publishing abandoned work. The consumer must still check
/// generation/profile/entry: selection can change between the final check and send.
fn run(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    generation: u64,
    cancel: &AtomicBool,
    tx: &Sender<Event>,
    fetch: impl FnOnce() -> Fetched,
) -> bool {
    publish(profile, entry.id.clone(), generation, cancel, tx, || {
        (Some(entry.clone()), fetch())
    })
}

fn publish(
    profile: &ProviderProfile,
    entry_id: String,
    generation: u64,
    cancel: &AtomicBool,
    tx: &Sender<Event>,
    work: impl FnOnce() -> (Option<CatalogEntry>, Fetched),
) -> bool {
    if cancel.load(Ordering::Acquire) {
        return false;
    }
    let (entry, result) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
        .unwrap_or((None, Fetched::Preview(Err("Metadata worker failed"))));
    if cancel.load(Ordering::Acquire) {
        return false;
    }
    let event = match result {
        Fetched::Preview(result) => Event::Preview(Preview {
            generation,
            profile_id: profile.id.clone(),
            entry_id,
            entry,
            result,
        }),
        Fetched::Guide { source_profile, request, result, row_guide } => {
            let Some(entry) = entry else {
                return false;
            };
            Event::Guide(Guide {
                row_guide,
                source_profile,
                generation,
                profile_id: profile.id.clone(),
                entry_id,
                entry,
                request,
                result,
            })
        }
    };
    tx.send(event).is_ok()
}

#[cfg(test)]
#[path = "ott_metadata_attachment_tests.rs"]
mod attachment_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use plx_data::ott::{MediaLocator, ProviderKind};
    use serde_json::json;
    use std::sync::mpsc::{channel, TryRecvError};

    #[test]
    fn classic_favorite_gate_accepts_only_a_matching_plex_source_before_lookup() {
        use plx_data::ott::favorites::Locator;
        let mut profile = profile();
        profile.kind = ProviderKind::Plex;
        profile.source = "server-a".into();
        let mut favorite = favorite();
        favorite.origin.provider = "plex".into();
        favorite.locator = Locator::Plex { server_id: "server-a".into(), rating_key: "42".into() };
        assert_eq!(classic_favorite_machine(&profile, &favorite), Ok("server-a"));
        favorite.locator = Locator::Plex { server_id: "server-b".into(), rating_key: "42".into() };
        assert_eq!(classic_favorite_machine(&profile, &favorite), Err("Favorite Plex source does not match"));
        for locator in [
            Locator::VPortal { id_field: "id".into(), id: "42".into() },
            Locator::Local { root_alias: "Home".into(), relative_path: "film.mp4".into() },
            Locator::Channel { channel_id: "42".into() },
        ] {
            favorite.locator = locator;
            assert_eq!(classic_favorite_machine(&profile, &favorite), Err("Favorite Plex source does not match"),
                "non-Plex favorites must not enter the catalogue-scanning fallback");
        }
        profile.kind = ProviderKind::M3u;
        profile.source = "https://source.invalid/list.m3u".into();
        favorite.locator = Locator::Plex { server_id: profile.source.clone(), rating_key: "42".into() };
        assert_eq!(classic_favorite_machine(&profile, &favorite), Err("Favorite Plex source does not match"));
        profile.kind = ProviderKind::Plex;
        profile.id.clear();
        assert_eq!(classic_favorite_machine(&profile, &favorite), Err("Invalid provider profile"));
    }

    fn preview(event: Event) -> Preview {
        match event { Event::Preview(value) => value, _ => panic!("expected preview") }
    }

    fn profile() -> ProviderProfile {
        ProviderProfile {
            id: "profile-a".into(),
            name: "Home".into(),
            kind: ProviderKind::M3u,
            source: "https://provider.test/catalog".into(),
            source_alias: "Home".into(),
            enabled: true,
        }
    }
    fn entry() -> CatalogEntry {
        CatalogEntry {
            id: "entry-a".into(),
            title: "Video".into(),
            category: String::new(),
            description: String::new(),
            poster: String::new(),
            duration_ms: None,
            provider_id: None,
            provider_id_field: None,
            live: None,
            media: MediaLocator::Http {
                url: "https://provider.test/video".into(),
            },
        }
    }

    #[test]
    fn preserves_selection_identity_and_provider_result() {
        let (tx, rx) = channel();
        let expected = json!({"title":"Programme","description":"Full description","end":200});
        assert!(run(
            &profile(),
            &entry(),
            7,
            &AtomicBool::new(false),
            &tx,
            || Fetched::Preview(Ok(expected.clone()))
        ));
        let event = preview(rx.try_recv().unwrap());
        assert_eq!(
            (
                event.generation,
                event.profile_id.as_str(),
                event.entry_id.as_str()
            ),
            (7, "profile-a", "entry-a")
        );
        assert_eq!(event.result, Ok(expected));
        assert_eq!(event.entry, Some(entry()));
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    fn guide_result(entry: &CatalogEntry, generation: u64, query: Query) -> Fetched {
        let request = Request {
            context: Context {
                generation,
                profile_id: profile().id,
                entry_id: entry.id.clone(),
            },
            query,
        };
        let result = Ok(ResultSet {
            context: request.context.clone(),
            query,
            coverage: plx_data::ott::guide::Coverage::RequestedWindow,
            programmes: Vec::new(),
        });
        Fetched::Guide { source_profile: profile(), request, result, row_guide: None }
    }

    #[test]
    fn worker_keeps_typed_query_until_receiver_admission() {
        let (tx, rx) = channel();
        let query = Query::CurrentNext { now: 1234 };
        let row = entry();
        assert!(run(
            &profile(),
            &row,
            7,
            &AtomicBool::new(false),
            &tx,
            || guide_result(&row, 7, query)
        ));
        let Event::Guide(event) = rx.try_recv().unwrap() else {
            panic!("expected typed guide")
        };
        assert_eq!(event.entry_id, row.id);
        assert_eq!(event.source_profile, profile());
        assert_eq!(event.request.query, query);
        assert_eq!(event.result.as_ref().unwrap().query, query);
        assert!(event
            .into_preview(7, Some(&(profile().id, row.id)), Some(&query), false)
            .is_some());
    }

    #[test]
    fn favorite_worker_keeps_outer_and_resolved_identity_separate() {
        let (tx, rx) = channel();
        let query = Query::CurrentNext { now: 1234 };
        let favorite = favorite();
        let row = entry();
        assert!(run_favorite(
            &profile(),
            &favorite,
            9,
            &AtomicBool::new(false),
            &tx,
            || Ok(row.clone()),
            |entry| guide_result(entry, 9, query)
        ));
        let Event::Guide(event) = rx.try_recv().unwrap() else {
            panic!("expected typed guide")
        };
        let target = (profile().id, format!("favorite:{}", favorite.identity()));
        assert_eq!(event.entry_id, target.1);
        assert_eq!(event.request.context.entry_id, row.id);
        assert!(event
            .into_preview(9, Some(&target), Some(&query), false)
            .is_some());
    }

    #[test]
    fn guide_publication_does_not_replace_effective_source_with_outer_owner() {
        let (tx, rx) = channel();
        let mut owner = profile();
        owner.id = "outer-owner".into();
        owner.name = "Outer profile".into();
        owner.source = "https://owner.invalid/list.m3u".into();
        let source = profile();
        let row = entry();
        let query = Query::CurrentNext { now: 1234 };
        let fetched = guide_result(&row, 9, query);
        let favorite = favorite();
        assert!(run_favorite(&owner, &favorite, 9, &AtomicBool::new(false), &tx,
            || Ok(row.clone()), |_| fetched));
        let Event::Guide(event) = rx.try_recv().unwrap() else {
            panic!("expected typed guide")
        };
        assert_eq!(event.profile_id, owner.id);
        assert_eq!(event.entry_id, format!("favorite:{}", favorite.identity()));
        assert_eq!(event.request.context.entry_id, row.id);
        assert_eq!(event.source_profile, source);
        assert_ne!(event.source_profile, owner);
        // This proves provenance transport only. Outer/effective mismatches
        // do not gain permission to pass the existing Guide request gate.
    }

    #[test]
    fn favorite_guide_retains_dispatched_source_after_same_id_profile_edit() {
        for failed in [false, true] {
            let (tx, rx) = channel();
            let dispatched = profile();
            let mut current_saved_profile = dispatched.clone();
            current_saved_profile.source = "https://replacement.invalid/list.m3u".into();
            let row = entry();
            let query = Query::CurrentNext { now: 1234 };
            let Fetched::Guide { source_profile, request, result, .. } = guide_result(&row, 9, query) else {
                panic!("expected typed fixture")
            };
            let fetched = Fetched::Guide { source_profile, request, row_guide: None,
                result: if failed { Err("Provider rejected guide request") } else { result } };
            let favorite = favorite();
            assert!(run_favorite(&dispatched, &favorite, 9, &AtomicBool::new(false), &tx,
                || Ok(row.clone()), |_| fetched));
            let Event::Guide(event) = rx.try_recv().unwrap() else {
                panic!("expected typed guide")
            };
            assert_eq!(event.profile_id, current_saved_profile.id);
            assert_eq!(event.source_profile, dispatched);
            assert_ne!(event.source_profile, current_saved_profile,
                "a matching profile ID cannot erase the dispatch source change");
            assert_eq!(event.result.is_err(), failed);
        }
    }

    #[test]
    fn cancellation_before_and_during_fetch_publishes_nothing() {
        let (tx, rx) = channel();
        let cancel = AtomicBool::new(true);
        assert!(!run(&profile(), &entry(), 1, &cancel, &tx, || panic!(
            "cancelled work ran"
        )));
        cancel.store(false, Ordering::Release);
        assert!(!run(&profile(), &entry(), 2, &cancel, &tx, || {
            cancel.store(true, Ordering::Release);
            Fetched::Preview(Ok(Value::Null))
        }));
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn failed_worker_reports_error_and_closed_receiver_does_not_signal_publication() {
        let (tx, rx) = channel();
        let cancel = AtomicBool::new(false);
        assert!(run(&profile(), &entry(), 3, &cancel, &tx, || panic!(
            "fixture failure"
        )));
        assert_eq!(preview(rx.try_recv().unwrap()).result, Err("Metadata worker failed"));
        drop(rx);
        assert!(!run(&profile(), &entry(), 4, &cancel, &tx, || {
            Fetched::Preview(Ok(Value::Null))
        }));
    }

    fn favorite() -> Favorite {
        Favorite {
            title: "Film".into(),
            origin: plx_data::ott::favorites::Origin {
                provider: "vportal".into(),
                profile: "Home".into(),
                subprofile: None,
                source_alias: "Home".into(),
            },
            locator: plx_data::ott::favorites::Locator::VPortal {
                id_field: "id".into(),
                id: "42".into(),
            },
        }
    }

    #[test]
    fn favorite_preview_retains_card_and_uses_favorite_selection_identity() {
        let (tx, rx) = channel();
        let favorite = favorite();
        let mut hydrated = entry();
        hydrated.description = "First sentence. Full second sentence.".into();
        hydrated.poster = "https://provider.test/poster.jpg".into();
        assert!(run_favorite(
            &profile(),
            &favorite,
            9,
            &AtomicBool::new(false),
            &tx,
            || Ok(hydrated.clone()),
            |_| Fetched::Preview(Err("Programme is unavailable"))
        ));
        let event = preview(rx.try_recv().unwrap());
        assert_eq!(event.entry_id, format!("favorite:{}", favorite.identity()));
        assert_eq!(event.entry, Some(hydrated));
        assert_eq!(event.result, Err("Programme is unavailable"));
    }

    #[test]
    fn cancelled_favorite_lookup_never_starts_metadata_or_publishes() {
        let (tx, rx) = channel();
        let cancel = AtomicBool::new(false);
        assert!(!run_favorite(
            &profile(),
            &favorite(),
            9,
            &cancel,
            &tx,
            || {
                cancel.store(true, Ordering::Release);
                Ok(entry())
            },
            |_| panic!("cancelled lookup started metadata")
        ));
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn library_preview_keeps_full_metadata_without_a_programme_request() {
        let mut row = entry();
        row.description = "Complete description. Including the second sentence.".into();
        row.poster = "https://provider.test/cover.jpg".into();
        for kind in [
            ProviderKind::Local,
            ProviderKind::VPortal,
            ProviderKind::Plex,
        ] {
            let mut p = profile();
            p.kind = kind;
            p.source = "no network endpoint".into();
            let Fetched::Preview(result) = metadata(
                &OffFrame::for_test(), &p, &row, 1,
                Query::CurrentNext { now: 0 }, None, &AtomicBool::new(false),
            ) else { panic!("expected library metadata"); };
            let data = result.unwrap();
            assert_eq!(data["description"], row.description);
            assert_eq!(data["poster"], row.poster);
            assert!(data.get("programmes").is_none());
        }
    }
}
