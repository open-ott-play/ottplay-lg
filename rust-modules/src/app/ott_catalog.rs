//! Application-owned catalogue IO. Pure parsers live in plx_data::ott.
use plx_base::task::{time::{Budget, Deadline}, OffFrame};
use plx_data::ott::{self, Catalog, CatalogEntry, MediaLocator, ProviderKind, ProviderProfile};
use serde_json::Value;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{SyncSender, TrySendError},
    Arc,
};

#[path = "ott_local.rs"]
mod local;
#[path = "ott_local_connections.rs"]
mod connections;
pub(super) use connections::{Request as LocalBindingRequest, Validated as LocalBindings};

#[cfg(all(test, feature = "hostsim"))]
pub(super) use connections::validate as validate_local_bindings_for_test;
#[cfg(all(test, feature = "hostsim"))]
pub(super) fn validate_local_bindings_observed_for_test(
    off: &OffFrame, request: LocalBindingRequest, cancel: &AtomicBool,
    observe: &mut dyn FnMut(&OffFrame, &AtomicBool) -> Result<Vec<plx_platform::tv::removable::MountCandidate>, super::ott_usb::Error>,
) -> Result<LocalBindings, super::ott_local_session::LocalError> {
    connections::validate_with_mount_probe(off, request, cancel, observe)
}

#[cfg(all(test, feature = "hostsim"))]
pub(super) fn visit_local_folder_for_test(
    off: &OffFrame, profile: &ProviderProfile,
    source: &super::ott_local_root::LocalSource, cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(usize, bool), &'static str> {
    local::visit_source_report(off, profile, source, cancel, emit)
        .map(|report| (report.videos, report.cancelled))
}


#[cfg(all(test, feature = "hostsim"))]
pub(super) fn visit_single_file_for_test(
    off: &OffFrame, profile: &ProviderProfile,
    source: &super::ott_local_root::LocalSource, cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    local::visit_single_file(off, profile, source, cancel, emit)
}

pub(super) enum Task {
    Provider(ProviderProfile),
    ManagedPlex { profile: ProviderProfile, read: super::adapters::session::ManagedRead },
    ClassicPlex { profile: ProviderProfile, read: super::ott_plex::ClassicRead },
    LocalBindings(LocalBindingRequest),
    /// The runtime supplies its current exact-file grant, never a raw parent path.
    LocalFile { profile: ProviderProfile, source: super::ott_local_root::LocalSource },
    LocalFolder { profile: ProviderProfile, source: super::ott_local_root::LocalSource },
    Usb(super::ott_usb::Task),
    UsbRemote { serial: u64, task: super::ott_usb::Task, cancel: Arc<AtomicBool> },
}
impl From<ProviderProfile> for Task {
    fn from(profile: ProviderProfile) -> Self { Self::Provider(profile) }
}
#[path = "ott_server_library.rs"]
mod server_library;
#[path = "ott_vportal.rs"]
mod vportal;

pub(super) fn valid_vportal_quality_key(key: &str) -> bool {
    vportal::valid_quality_key(key)
}

pub(super) struct Resolved {
    pub url: String,
    pub lease: Option<super::ott_vportal_lease::Lease>,
}

pub(super) enum Event {
    Batch(u64, Catalog),
    LocalBindings(u64, Result<LocalBindings, super::ott_local_session::LocalError>),
    Usb(u64, Result<super::ott_usb::Answer, super::ott_usb::Error>),
    UsbRemote(u64, Result<super::ott_usb::Answer, super::ott_usb::Error>),
    Finished(u64, Result<(), &'static str>),
    /// Only a fully exhausted Local directory walk can produce this result.
    LocalFinishedWithWarning(u64, &'static str),
}

fn fetch(url: &str, body: Option<&Value>) -> Result<String, &'static str> {
    if ott::http_url(url).is_none() {
        return Err("Invalid provider address");
    }
    let encoded = body.map(Value::to_string);
    let headers = if body.is_some() {
        vec!["Content-Type: application/json".into()]
    } else {
        Vec::new()
    };
    let response = plx_net::net::request(
        url,
        &headers,
        if body.is_some() { "POST" } else { "GET" },
        encoded.as_deref().map(str::as_bytes),
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
    )
    .ok_or("Provider request failed or timed out")?;
    if !response.ok() {
        return Err("Provider rejected the catalogue request");
    }
    String::from_utf8(response.body).map_err(|_| "Provider returned invalid text")
}

/// Called off-frame. Adopt a successful server lease before cancellation checks,
/// so a response arriving after replacement cannot leak a transcoding session.
pub(super) fn resolve_typed(
    worker: &OffFrame,
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    cancel: &AtomicBool,
) -> Result<Resolved, &'static str> {
    if cancel.load(Ordering::Acquire) {
        return Err("Playback request cancelled");
    }
    profile.validate().map_err(|_| "Invalid provider profile")?;
    let mut failure = None;
    let resolved = match &entry.media {
        MediaLocator::Http { url } => Resolved {
            url: if profile.kind == ProviderKind::Edem && entry.live.is_some() {
                let config=ott::edem::Config::parse(&profile.source).map_err(|_|"Invalid Edem profile")?;
                config.stream(entry.provider_id.as_deref().ok_or("Invalid Edem channel")?).map_err(|_|"Invalid Edem channel")?
            } else { ott::http_url(url).ok_or("Invalid media address")? },
            lease: None,
        },
        MediaLocator::VPortal { .. } => {
            let resolved = vportal::resolve_typed(profile, entry, cancel)?;
            let lease = resolved
                .session
                .map(|session| {
                    super::ott_vportal_lease::Lease::start(session.stop, session.heartbeat)
                })
                .transpose()?;
            failure = resolved.failure;
            Resolved {
                url: resolved.url,
                lease,
            }
        }
        MediaLocator::Local { .. } => Resolved {
            url: local::resolve(profile, entry)?
                .to_str()
                .ok_or("Local path cannot be represented")?
                .into(),
            lease: None,
        },
        MediaLocator::ServerLibrary { .. } => Resolved {
            url: server_library::resolve(profile, entry, cancel)?,
            lease: None,
        },
        MediaLocator::Xtream { .. } | MediaLocator::Stalker { .. } | MediaLocator::StalkerApi { .. } => Resolved {
            url: super::ott_provider_http::resolve_cancellable(worker, profile, entry, cancel)?,
            lease: None,
        },
        MediaLocator::Plex { .. } => return Err("Plex playback uses its native session"),
    };
    complete_resolution(resolved, failure, cancel)
}

/// The resolver cannot admit its replacement until cleanup has settled. Drop
/// only signals asynchronous cleanup, so a rejected result must wait here on
/// the worker while it still owns the lease's completion barrier.
fn complete_resolution(
    mut resolved: Resolved,
    failure: Option<&'static str>,
    cancel: &AtomicBool,
) -> Result<Resolved, &'static str> {
    let error = failure.or_else(|| {
        cancel
            .load(Ordering::Acquire)
            .then_some("Playback request cancelled")
    });
    if let Some(error) = error {
        if let Some(lease) = resolved.lease.take() {
            lease.finish_off_frame()?;
        }
        return Err(error);
    }
    Ok(resolved)
}

pub(super) fn start(
    task: impl Into<Task>,
    generation: u64,
    cancel: Arc<AtomicBool>,
    tx: SyncSender<Event>,
    done: Arc<AtomicBool>,
) -> bool {
    let task = task.into();
    super::ott_worker::spawn("ott-catalog", done, move |worker| {
        let mut completion_cancel = cancel.clone();
        let event = match task {
            Task::UsbRemote { serial, task, cancel } => {
                completion_cancel = cancel.clone();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    super::ott_usb::execute(worker, task, &cancel)
                })).unwrap_or(Err(super::ott_usb::Error::Unavailable));
                Event::UsbRemote(serial, result)
            }
            Task::Usb(task) => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    super::ott_usb::execute(worker, task, &cancel)
                })).unwrap_or(Err(super::ott_usb::Error::Unavailable));
                Event::Usb(generation, result)
            }
            Task::ManagedPlex { profile, read } => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    super::ott_plex::visit_managed(&profile, &read, &cancel, &mut |catalog| {
                        if !read.current() { return Err("Plex source is no longer available"); }
                        send_catalog(&tx, generation, catalog, &cancel)
                    })?;
                    if !read.current() { return Err("Plex source is no longer available"); }
                    Ok(())
                })).unwrap_or(Err("Catalogue worker failed"));
                Event::Finished(generation, result)
            }
            Task::ClassicPlex { profile, read } => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    super::ott_plex::visit_classic(&profile, &read, &cancel, &mut |catalog| {
                        if !read.current() { return Err("Plex source is no longer available"); }
                        send_catalog(&tx, generation, catalog, &cancel)
                    })?;
                    if !read.current() { return Err("Plex source is no longer available"); }
                    Ok(())
                })).unwrap_or(Err("Catalogue worker failed"));
                Event::Finished(generation, result)
            }
            Task::Provider(profile) => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    scan(worker, &profile, generation, &cancel, &tx)
                })).unwrap_or(Err("Catalogue worker failed"));
                scan_completion(generation, result)
            }
            Task::LocalFolder { profile, source } => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let report = local::visit_source_report(worker, &profile, &source, &cancel,
                        &mut |rows| send(&tx, generation, rows, &cancel))?;
                    local_completion(report, &cancel)
                })).unwrap_or(Err("Catalogue worker failed"));
                scan_completion(generation, result)
            }
            Task::LocalFile { profile, source } => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    local::visit_single_file(worker, &profile, &source, &cancel, &mut |rows| {
                        send(&tx, generation, rows, &cancel)
                    })
                })).unwrap_or(Err("Catalogue worker failed"));
                Event::Finished(generation, result)
            }
            Task::LocalBindings(request) => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    connections::validate(worker, request, &cancel)
                })).unwrap_or(Err(super::ott_local_session::LocalError::NativeError));
                Event::LocalBindings(generation, result)
            }
        };
        let _ = send_event(&tx, event, &completion_cancel);
        plx_machine::idle::invalidate();
    })
}

fn send_event<T>(
    tx: &SyncSender<T>,
    mut event: T,
    cancel: &AtomicBool,
) -> Result<(), &'static str> {
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err("Catalogue cancelled");
        }
        match tx.try_send(event) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => return Err("Catalogue cancelled"),
            Err(TrySendError::Full(value)) => {
                event = value;
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
}

fn send(
    tx: &SyncSender<Event>,
    generation: u64,
    rows: Vec<CatalogEntry>,
    cancel: &AtomicBool,
) -> Result<(), &'static str> {
    send_catalog(tx, generation, Catalog { entries: rows, ..Catalog::default() }, cancel)
}

fn send_catalog(
    tx: &SyncSender<Event>, generation: u64, catalog: Catalog, cancel: &AtomicBool,
) -> Result<(), &'static str> {
    if catalog.entries.is_empty() && catalog.hierarchy.is_none() { return Ok(()); }
    send_event(tx, Event::Batch(generation, catalog), cancel)?;
    plx_machine::idle::invalidate();
    Ok(())
}

fn scan_completion(generation: u64, result: Result<Option<&'static str>, &'static str>) -> Event {
    match result {
        Ok(Some(warning)) => Event::LocalFinishedWithWarning(generation, warning),
        other => Event::Finished(generation, other.map(|_| ())),
    }
}

fn scan_local(worker: &OffFrame, profile: &ProviderProfile, cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<Option<&'static str>, &'static str> {
    profile.validate().map_err(|_| "Invalid provider profile")?;
    let report = local::visit_report(worker, profile, cancel, emit)?;
    local_completion(report, cancel)
}

fn local_completion(report: local::ScanReport, cancel: &AtomicBool)
    -> Result<Option<&'static str>, &'static str> {
    // emit() may cancel from its final partial batch, after visit_report's last
    // loop check. Re-read at the terminal boundary as well as in send_event.
    if report.cancelled || cancel.load(Ordering::Acquire) { return Err("Catalogue cancelled"); }
    if report.depth_limited { return Err("Local folder depth limit reached"); }
    Ok(report.warning())
}

/// Exercise the production visitor/terminal mapping with changes between batches.
#[cfg(all(test, feature = "hostsim"))]
pub(super) fn visit_local_for_test(profile: &ProviderProfile, generation: u64,
    cancel: &AtomicBool, emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Event {
    scan_completion(generation, scan_local(&OffFrame::for_test(), profile, cancel, emit))
}

/// Missing-child fixture still uses the real walker, report and terminal mapping.
#[cfg(all(test, feature = "hostsim"))]
pub(super) fn visit_local_with_lost_child_for_test(profile: &ProviderProfile, generation: u64,
    cancel: &AtomicBool, emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Event {
    let result = local::visit_with_lost_child_for_test(&OffFrame::for_test(), profile, cancel, emit)
        .and_then(|report| local_completion(report, cancel));
    scan_completion(generation, result)
}

fn scan(
    worker: &OffFrame,
    profile: &ProviderProfile,
    generation: u64,
    cancel: &AtomicBool,
    tx: &SyncSender<Event>,
) -> Result<Option<&'static str>, &'static str> {
    profile.validate().map_err(|_| "Invalid provider profile")?;
    if profile.kind == ProviderKind::Local && ott::http_url(&profile.source).is_none() {
        return scan_local(worker, profile, cancel, &mut |rows| send(tx, generation, rows, cancel));
    }
    if profile.kind == ProviderKind::VPortal {
        vportal::visit_catalog(worker, profile, cancel, &mut |catalog|
            send_catalog(tx, generation, catalog, cancel)).map(|()| None)
    } else if profile.kind == ProviderKind::Plex {
        super::ott_plex::visit_catalog(profile, cancel, &mut |catalog|
            send_catalog(tx, generation, catalog, cancel)).map(|()| None)
    } else {
        visit(worker, profile, cancel, &mut |rows| {
            send(tx, generation, rows, cancel)
        }).map(|()| None)
    }
}

fn visit(
    worker: &OffFrame,
    profile: &ProviderProfile,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    profile.validate().map_err(|_| "Invalid provider profile")?;
    match profile.kind {
        ProviderKind::M3u | ProviderKind::Xtream | ProviderKind::Stalker | ProviderKind::Edem | ProviderKind::Operator(_) => {
            super::ott_provider_http::visit(worker, profile, cancel, emit)?;
            if profile.kind == ProviderKind::Edem && ott::edem::Config::parse(&profile.source).is_ok_and(|c|!c.portal.is_empty()) {
                vportal::visit(worker,profile,cancel,emit)?;
            }
        }
        ProviderKind::VPortal => vportal::visit(worker, profile, cancel, emit)?,
        ProviderKind::Local if ott::http_url(&profile.source).is_some() => {
            server_library::visit(worker, profile, cancel, emit)?
        }
        ProviderKind::Local => local::visit(worker, profile, cancel, emit)?,
        ProviderKind::Plex => super::ott_plex::visit(profile, cancel, emit)?,
    }
    Ok(())
}

/// A bare filename is ambiguous in portable v1; only a receiving-device proof
/// may authorize it. This guard runs before filesystem or server lookup.
fn local_favorite_item<'a>(profile: &ProviderProfile, favorite: &ott::favorites::Favorite,
    proof: Option<&'a super::ott_runtime::LocalFavoritePermit>) -> Result<Option<&'a str>, &'static str> {
    favorite.validate().map_err(|_| "Invalid Favorite")?;
    if matches!(&favorite.locator, ott::favorites::Locator::Local { relative_path, .. } if !relative_path.contains('/')) {
        let proof = proof.filter(|proof| proof.matches(profile, favorite))
            .ok_or("Local Favorite filename needs proof from this player")?;
        Ok(Some(proof.item_id()))
    } else { Ok(None) }
}

/// Rebind an imported Favorite against the receiving client's own profile. No source
/// address is taken from the portable document, and duplicate names cannot pick an
/// arbitrary channel. The UI remains responsive while this worker searches.
pub(super) fn find_favorite(
    worker: &OffFrame,
    profile: &ProviderProfile,
    favorite: &ott::favorites::Favorite,
    local_proof: Option<&super::ott_runtime::LocalFavoritePermit>,
    cancel: &AtomicBool,
) -> Result<CatalogEntry, &'static str> {
    use ott::favorites::Locator;
    let expected_item = local_favorite_item(profile, favorite, local_proof)?;
    if let Locator::Local {
        root_alias,
        relative_path,
    } = &favorite.locator
    {
        if profile.kind == ProviderKind::Local && ott::http_url(&profile.source).is_some() {
            let row = server_library::find_favorite(profile, root_alias, relative_path, cancel)?;
            if expected_item.is_some_and(|id| id != row.id) { return Err("Local Favorite file identity changed"); }
            return Ok(row);
        }
    }
    let direct = match &favorite.locator {
        Locator::Local { relative_path, .. } => Some(MediaLocator::Local {
            relative_path: relative_path.clone(),
        }),
        Locator::Plex {
            server_id,
            rating_key,
        } => Some(MediaLocator::Plex {
            server_id: server_id.clone(),
            rating_key: rating_key.clone(),
        }),
        _ => None,
    };
    if let Some(media) = direct {
        return Ok(CatalogEntry {
            synopsis: None,
            id: expected_item.map(str::to_owned).unwrap_or_else(|| favorite.identity()),
            title: favorite.title.clone(),
            category: String::new(),
            description: String::new(),
            poster: String::new(),
            media,
            duration_ms: None,
            provider_id: None,
            provider_id_field: None, live: None,
        });
    }
    find_catalog_favorite(worker, favorite, cancel, false, |accept, deadline| {
        if profile.kind == ProviderKind::VPortal {
            vportal::visit_before(worker, profile, cancel, accept, deadline)
        } else {
            visit(worker, profile, cancel, accept)
        }
    })
}

/// The caller retains this typed receipt with its playback/navigation ticket
/// and revalidates it before admitting the result. The effective source never
/// becomes a registered profile or changes the portable M3U origin.
pub(super) fn find_attachment_favorite(
    worker: &OffFrame,
    receipt: &super::ott_m3u_attachment::Receipt,
    favorite: &ott::favorites::Favorite,
    cancel: &AtomicBool,
) -> Result<CatalogEntry, &'static str> {
    find_attachment_favorite_with(worker, receipt, favorite, cancel, |profile, accept, deadline| {
        vportal::visit_before(worker, profile, cancel, accept, deadline)
    })
}

fn find_attachment_favorite_with(
    worker: &OffFrame,
    receipt: &super::ott_m3u_attachment::Receipt,
    favorite: &ott::favorites::Favorite,
    cancel: &AtomicBool,
    traverse: impl FnOnce(&ProviderProfile,
        &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>, Deadline) -> Result<(), &'static str>,
) -> Result<CatalogEntry, &'static str> {
    let profile = super::ott_runtime::attachment_profile(receipt, favorite)?;
    find_catalog_favorite(worker, favorite, cancel, true, |accept, deadline| {
        traverse(&profile, accept, deadline)
    })
}

fn find_catalog_favorite(
    worker: &OffFrame,
    favorite: &ott::favorites::Favorite,
    cancel: &AtomicBool,
    require_unique: bool,
    traverse: impl FnOnce(&mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
        Deadline) -> Result<(), &'static str>,
) -> Result<CatalogEntry, &'static str> {
    use ott::favorites::Locator;
    if cancel.load(Ordering::Acquire) { return Err("Playback request cancelled"); }
    let mut found: Option<CatalogEntry> = None;
    let budget = Budget::start(worker, std::time::Duration::from_secs(60));
    // A portable portal ID can appear in more than one page or folder. Only
    // complete traversal can distinguish an identical duplicate from a second
    // media row with the same locator, including for standalone VPortal.
    let require_unique = require_unique || matches!(favorite.locator, Locator::VPortal { .. });
    let mut accept = |rows: Vec<CatalogEntry>| {
        if cancel.load(Ordering::Relaxed) {
            return Err("Playback request cancelled");
        }
        if budget.expired(worker) {
            return Err("Favorite lookup timed out");
        }
        for entry in rows {
            if super::ott_runtime::legacy_id_collision(favorite, &entry) {
                return Err("Favorite identity is ambiguous");
            }
            if super::ott_runtime::matches_entry(favorite, &entry) {
                if found.as_ref().is_some_and(|old| if require_unique {
                    old != &entry
                } else {
                    !same_match(&favorite.locator, old, &entry)
                }) {
                    return Err("Favorite matches more than one item");
                }
                if found.is_none() {
                    found = Some(entry);
                }
            }
        }
        Ok(())
    };
    traverse(&mut accept, budget.deadline())?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Playback request cancelled");
    }
    found.ok_or("Favorite is not available from this profile")
}

fn same_match(locator: &ott::favorites::Locator, old: &CatalogEntry, next: &CatalogEntry) -> bool {
    if old.id == next.id {
        return true;
    }
    if let ott::favorites::Locator::VPortal { id_field, id } = locator {
        return old.provider_id.as_ref() == Some(id)
            && next.provider_id.as_ref() == Some(id)
            && old.provider_id_field.as_ref() == Some(id_field)
            && next.provider_id_field.as_ref() == Some(id_field);
    }
    false
}

#[cfg(test)]
#[path = "ott_catalog_attachment_tests.rs"]
mod attachment_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn failed_or_cancelled_resolution_waits_for_the_session_stop() {
        for (failure, cancelled) in [(Some("Provider failed"), false), (None, true)] {
            let (lease, release, finished) =
                super::super::ott_vportal_lease::Lease::pending_for_test();
            let (returned, result) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                let error = complete_resolution(
                    Resolved {
                        url: "https://media.test/video".into(),
                        lease: Some(lease),
                    },
                    failure,
                    &AtomicBool::new(cancelled),
                )
                .err();
                returned.send(error).unwrap();
            });
            release.recv_timeout(Duration::from_secs(1)).unwrap();
            assert_eq!(
                result.recv_timeout(Duration::from_millis(30)),
                Err(mpsc::RecvTimeoutError::Timeout),
                "resolver returned before its session stop completed"
            );
            finished.send(()).unwrap();
            assert_eq!(
                result.recv_timeout(Duration::from_secs(1)).unwrap(),
                Some(failure.unwrap_or("Playback request cancelled"))
            );
            worker.join().unwrap();
        }
    }

    #[test]
    fn failed_session_stop_is_not_hidden_by_the_original_provider_error() {
        let (lease, _release, finished) =
            super::super::ott_vportal_lease::Lease::pending_for_test();
        drop(finished);
        let result = complete_resolution(
            Resolved {
                url: String::new(),
                lease: Some(lease),
            },
            Some("Provider failed"),
            &AtomicBool::new(false),
        );
        assert_eq!(
            result.err(),
            Some("Previous playback session did not finish closing")
        );
    }

    #[test]
    fn accepted_resolution_keeps_its_live_session_owned_by_the_caller() {
        let (lease, release, _finished) =
            super::super::ott_vportal_lease::Lease::pending_for_test();
        let result = complete_resolution(
            Resolved {
                url: "https://media.test/video".into(),
                lease: Some(lease),
            },
            None,
            &AtomicBool::new(false),
        )
        .unwrap_or_else(|error| panic!("accepted resolution failed: {error}"));
        assert!(result.lease.is_some());
        assert_eq!(release.try_recv(), Err(mpsc::TryRecvError::Empty));
        drop(result);
        assert_eq!(release.recv_timeout(Duration::from_secs(1)), Ok(()));
    }

    #[test]
    fn full_channel_does_not_prevent_cancelling_old_scan() {
        let (tx, _rx) = mpsc::sync_channel(1);
        tx.send(1).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (done, received) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            done.send(send_event(&tx, 2, &worker_cancel)).unwrap();
        });
        cancel.store(true, Ordering::Release);
        assert!(received
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .is_err());
        worker.join().unwrap();
    }

    #[test]
    fn backpressure_keeps_order_and_receiver_drop_settles() {
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(1).unwrap();
        let worker = std::thread::spawn(move || send_event(&tx, 2, &AtomicBool::new(false)));
        assert_eq!(rx.recv().unwrap(), 1);
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), 2);
        assert!(worker.join().unwrap().is_ok());
        let (tx, rx) = mpsc::sync_channel::<u8>(1);
        drop(rx);
        assert!(send_event(&tx, 1, &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn vportal_folder_aliases_share_favorite_identity() {
        let item = |row: &str| CatalogEntry {
            synopsis: None,
            id: row.into(),
            title: "Film".into(),
            category: row.into(),
            description: String::new(),
            poster: String::new(),
            duration_ms: None,
            media: MediaLocator::VPortal {
                request: serde_json::json!({"id":"42"}),
            },
            provider_id: Some("42".into()),
            provider_id_field: Some("id".into()),
            live: None,
        };
        assert!(same_match(
            &ott::favorites::Locator::VPortal {
                id_field: "id".into(),
                id: "42".into()
            },
            &item("a"),
            &item("b")
        ));
        assert!(!same_match(
            &ott::favorites::Locator::ChannelName {
                name: "Film".into(),
                group: None
            },
            &item("a"),
            &item("b")
        ));
    }
}

#[cfg(test)] mod remote_usb_delivery_tests {
    use super::*;
    #[test] fn remote_usb_result_uses_its_own_cancellation_after_catalogue_back() {
        use crate::app::ott_local_session::{AuthorityRef,BoundSource,SourceKind};
        let source=crate::app::ott_local_root::LocalSource::unavailable(BoundSource{id:"unavailable".into(),name:"Unavailable".into(),kind:SourceKind::Folder,available:false,authority_ref:AuthorityRef(1),generation:1,canonical_path:None});
        let own=Arc::new(AtomicBool::new(false));let done=Arc::new(AtomicBool::new(true));let (tx,rx)=std::sync::mpsc::sync_channel(1);
        assert!(start(Task::UsbRemote{serial:9,task:crate::app::ott_usb::Task::VerifySource(source),cancel:own},1,Arc::new(AtomicBool::new(true)),tx,done));
        // VerifySource rejects this ungranted source before mount observation;
        // the actual worker completion must not inherit the retired catalog's token.
        assert!(matches!(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),Event::UsbRemote(9,Err(crate::app::ott_usb::Error::Changed))));
    }
}

/// Private playback candidates; neither URL nor lifecycle capability is copied to a UI row.
#[derive(Clone)]
pub(super) struct QualityOption {
    pub key: String,
    pub label: String,
    url: String,
    lease: Option<usize>,
}
pub(super) struct VPortalChoices {
    pub options: Vec<QualityOption>,
    pub selected: Option<usize>,
    fallback: QualityOption,
    leases: super::ott_vportal_lease::LeaseSet,
}
impl VPortalChoices {
    pub(super) fn retire(self) -> Vec<super::ott_vportal_lease::Lease> {
        let leases = self.leases.into_leases();
        for lease in &leases { lease.begin_release(); }
        leases
    }
    pub(super) fn choose(mut self, worker: &OffFrame, index: Option<usize>, cancel: &AtomicBool) -> Result<Resolved, &'static str> {
        let budget = plx_base::task::time::Budget::start(worker, std::time::Duration::from_secs(11));
        let selected = match index {
            Some(index) => match self.options.get(index) {
                Some(option) => option.clone(),
                None => { self.leases.finish_before(worker, budget)?; return Err("VPortal quality selection is stale"); }
            },
            None => self.fallback.clone(),
        };
        let lease = selected.lease.and_then(|index| self.leases.take(index));
        if let Err(error) = self.leases.finish_before(worker, budget) {
            if let Some(lease) = lease { let _ = lease.finish_after_loop(budget.remaining(worker)); }
            return Err(error);
        }
        if cancel.load(Ordering::Acquire) {
            if let Some(lease) = lease { lease.finish_after_loop(budget.remaining(worker))?; }
            return Err("Playback request cancelled");
        }
        Ok(Resolved { url: selected.url, lease })
    }
}
pub(super) fn vportal_choices(
    worker: &OffFrame, profile: &ProviderProfile, entry: &CatalogEntry, cancel: &AtomicBool,
    preferred: Option<&str>, interactive: bool,
) -> Result<VPortalChoices, &'static str> {
    let parsed = vportal::resolve_choices_typed(profile, entry, cancel, preferred, interactive)?;
    let index = |variant: &vportal::Variant| variant.session.as_ref()
        .and_then(|session| parsed.sessions.iter().position(|candidate| candidate.stop == session.stop));
    let option = |variant: &vportal::Variant| QualityOption { key: variant.key.clone(), label: variant.label.clone(),
        url: variant.url.clone(), lease: index(variant) };
    let options: Vec<_> = parsed.variants.iter().map(option).collect();
    let selected = options.iter().position(|option| !parsed.selected.key.is_empty() && option.key == parsed.selected.key)
        .or_else(|| options.iter().position(|option| option.url == parsed.selected.url));
    let fallback = option(&parsed.selected);
    // Even a late result owns allocations. Adopt all admitted, deduplicated controls before cancellation.
    let cleanup_budget = plx_base::task::time::Budget::start(worker, std::time::Duration::from_secs(11));
    let leases = super::ott_vportal_lease::LeaseSet::adopt(worker, cleanup_budget, parsed.sessions.into_iter()
        .map(|session| (session.stop, session.heartbeat)).collect())?;
    if let Some(error) = parsed.failure.or_else(|| cancel.load(Ordering::Acquire).then_some("Playback request cancelled")) {
        leases.finish_before(worker, cleanup_budget)?;
        return Err(error);
    }
    Ok(VPortalChoices { options, selected, fallback, leases })
}

/// A completed two-choice resolver result for application event tests. This
/// constructs no worker; the real parser and lease state machine are tested separately.
#[cfg(all(test, feature = "hostsim"))]
pub(super) fn quality_result_for_test(leases: Vec<super::ott_vportal_lease::Lease>) -> VPortalChoices {
    let first = QualityOption { key: "High".into(), label: "High".into(),
        url: "https://fixture.invalid/high.mp4".into(), lease: (!leases.is_empty()).then_some(0) };
    let second = QualityOption { key: "Low".into(), label: "Low".into(),
        url: "https://fixture.invalid/low.mp4".into(), lease: None };
    VPortalChoices { options: vec![first.clone(), second], selected: Some(0), fallback: first,
        leases: super::ott_vportal_lease::LeaseSet::for_test(leases) }
}

/// Observe the actual resolver's selection; no session/quality result is fabricated.
#[cfg(all(test, feature = "hostsim"))]
pub(super) fn vportal_resolution_with_fetch_for_test(
    profile: &ProviderProfile, entry: &CatalogEntry, cancel: &AtomicBool,
    preferred: Option<&str>, interactive: bool,
    fetch: &mut dyn FnMut(&str, &Value, &AtomicBool) -> Result<String, &'static str>,
) -> Result<(String, usize, usize, Option<&'static str>), &'static str> {
    let parsed = vportal::resolve_choices_with_fetch_for_test(profile, entry, cancel, preferred, interactive, fetch)?;
    Ok((parsed.selected.url, parsed.variants.len(), parsed.sessions.len(), parsed.failure))
}

#[cfg(all(test,feature="hostsim"))]
mod edem_resolution_tests {
    use super::*;
    #[test]
    fn edem_portal_http_movie_without_duration_retains_its_resolved_address() {
        let profile=ProviderProfile{id:"edem-1".into(),name:"Edem".into(),kind:ProviderKind::Edem,source:r#"{"key":"test_key"}"#.into(),source_alias:String::new(),enabled:true};
        let template=ott::edem::Config::parse(&profile.source).unwrap().template_profile(&profile);
        let mut row=ott::parse_m3u("#EXTM3U\n#EXTINF:-1,Movie\nhttps://movies.invalid/item.mp4\n",&template).unwrap().entries.remove(0);
        row.live=None;row.duration_ms=None;row.provider_id=Some("47".into());row.provider_id_field=Some("id".into());
        let result=resolve_typed(&OffFrame::for_test(),&profile,&row,&AtomicBool::new(false)).unwrap();
        assert_eq!(result.url,"https://movies.invalid:443/item.mp4");
    }
}
