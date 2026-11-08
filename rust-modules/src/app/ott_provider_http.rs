//! Bounded provider IO, called only by the catalogue/resolve worker.
//! Credentials stay inside a profile and never enter diagnostic strings.
use plx_base::task::{
    time::{Budget, Deadline},
    OffFrame,
};
use plx_data::ott::{self, CatalogEntry, MediaLocator, ProviderKind, ProviderProfile};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[path = "ott_providers/live.rs"]
mod live;
#[path = "ott_providers/stalker.rs"]
mod stalker;
#[path = "ott_providers/stalker_api.rs"]
mod stalker_api;
#[path = "ott_providers/xtream.rs"]
mod xtream;

pub(super) struct Request {
    pub url: String,
    pub headers: Vec<String>,
    pub body: Option<Vec<u8>>,
}
pub(super) struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}
pub(super) trait ProviderHttp {
    fn request(&mut self, request: &Request) -> Result<Response, &'static str>;
    fn cancelled(&self) -> bool;
    /// Optional row work has a separate budget from the selected guide owner.
    fn row_guide_expired(&self) -> bool { false }
    fn row_guide_selected(&mut self) {}
    fn stream(
        &mut self,
        request: &Request,
        consume: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<(), &'static str> {
        let response = self.request(request)?;
        if !(200..300).contains(&response.status) {
            return Err("Provider rejected request");
        }
        if self.cancelled() || !consume(&response.body) {
            return Err("Provider request cancelled");
        }
        Ok(())
    }
}

/// Small metadata GETs within a deadline captured by the operation owner.
/// The owner admits the configured authority before constructing each request;
/// this adapter cannot acquire a new deadline or follow a redirected authority.
pub(super) struct BoundedHttp<'a> {
    worker: &'a OffFrame,
    cancel: &'a AtomicBool,
    deadline: Deadline,
    max_bytes: usize,
}

impl<'a> BoundedHttp<'a> {
    pub fn new(
        worker: &'a OffFrame,
        cancel: &'a AtomicBool,
        deadline: Deadline,
        max_bytes: usize,
    ) -> Self {
        Self {
            worker,
            cancel,
            deadline,
            max_bytes,
        }
    }

    fn request_with(
        &mut self,
        request: &Request,
        fetch: impl FnOnce(
            plx_net::net::Timeouts,
            &mut dyn FnMut(&[u8]) -> bool,
        ) -> plx_net::net::StreamEnd,
    ) -> Result<Response, &'static str> {
        if self.cancelled() {
            return Err("Provider request cancelled");
        }
        if request.body.is_some()
            || !request.headers.is_empty()
            || ott::http_url(&request.url).is_none()
        {
            return Err("Invalid provider request");
        }
        if self.max_bytes == 0 || self.max_bytes > ott::MAX_CATALOG_BYTES {
            return Err("Invalid provider response limit");
        }
        // Never pass zero to curl: CURLOPT_TIMEOUT_MS=0 disables its limit.
        // Truncate sub-millisecond remainder rather than extending a deadline.
        let total_ms = self
            .deadline
            .remaining(self.worker)
            .as_millis()
            .min(std::os::raw::c_long::MAX as u128) as std::os::raw::c_long;
        if total_ms == 0 {
            return Err("Provider request timed out");
        }
        let timeouts = plx_net::net::Timeouts {
            total_ms,
            ..plx_net::net::STREAM
        };
        let mut body = Vec::new();
        let mut exceeded = false;
        let end = fetch(timeouts, &mut |bytes| {
            if self.cancelled() || self.deadline.expired(self.worker) {
                return false;
            }
            if bytes.len() > self.max_bytes.saturating_sub(body.len()) {
                exceeded = true;
                return false;
            }
            body.extend_from_slice(bytes);
            true
        });
        // Recheck ownership after IO even if cancellation arrived after its last
        // callback. An old response must never become the next metadata stage.
        if self.cancelled() {
            return Err("Provider request cancelled");
        }
        if self.deadline.expired(self.worker) {
            return Err("Provider request timed out");
        }
        if exceeded {
            return Err("Provider response exceeds limit");
        }
        let status = match end {
            plx_net::net::StreamEnd::Closed => 200,
            plx_net::net::StreamEnd::Status(status) => status,
            plx_net::net::StreamEnd::Cancelled | plx_net::net::StreamEnd::Consumer => {
                return Err("Provider request cancelled");
            }
            _ => return Err("Provider request failed or timed out"),
        };
        Ok(Response { status, body })
    }
}

impl ProviderHttp for BoundedHttp<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    fn request(&mut self, request: &Request) -> Result<Response, &'static str> {
        let cancel = self.cancel;
        self.request_with(request, |timeouts, consume| {
            plx_net::net::stream_tls_with_timeouts(
                &request.url,
                &[],
                plx_net::net::Tls::Ca,
                None,
                timeouts,
                Duration::from_secs(10),
                cancel,
                consume,
            )
        })
    }
}

struct NativeHttp<'a> {
    worker: &'a OffFrame,
    cancel: &'a AtomicBool,
}
impl ProviderHttp for NativeHttp<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
    fn request(&mut self, request: &Request) -> Result<Response, &'static str> {
        if self.cancelled() {
            return Err("Provider request cancelled");
        }
        if ott::http_url(&request.url).is_none() {
            return Err("Invalid provider address");
        }
        if request.body.is_none() {
            let mut body = Vec::new();
            let mut exceeded = false;
            let result = self.stream_status(request, &mut |bytes| {
                if bytes.len() > ott::MAX_CATALOG_BYTES.saturating_sub(body.len()) {
                    exceeded = true;
                    return false;
                }
                body.extend_from_slice(bytes);
                true
            });
            if exceeded {
                return Err("Provider response exceeds limit");
            }
            return Ok(Response {
                status: result?,
                body,
            });
        }
        let response = plx_net::net::request(
            &request.url,
            &request.headers,
            "POST",
            request.body.as_deref(),
            plx_net::net::Timeouts {
                connect_s: 5,
                total_s: 15,
                total_ms: 0,
                low_speed_bps: 1,
                low_speed_s: 8,
            },
            false,
            Some(ott::MAX_CATALOG_BYTES),
            None,
        )
        .ok_or("Provider request failed")?;
        if self.cancelled() {
            return Err("Provider request cancelled");
        }
        Ok(Response {
            status: response.status,
            body: response.body,
        })
    }
    fn stream(
        &mut self,
        request: &Request,
        consume: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<(), &'static str> {
        match self.stream_status(request, consume)? {
            200..=299 => Ok(()),
            401 | 403 => Err("Provider authorization failed"),
            _ => Err("Provider rejected request"),
        }
    }
}
impl NativeHttp<'_> {
    fn stream_status(
        &mut self,
        request: &Request,
        consume: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<u16, &'static str> {
        if self.cancelled() {
            return Err("Provider request cancelled");
        }
        if request.body.is_some() || ott::http_url(&request.url).is_none() {
            return Err("Invalid provider request");
        }
        let budget = Budget::start(self.worker, Duration::from_secs(60));
        let mut total = 0usize;
        let mut limited = false;
        let end = plx_net::net::stream_tls(
            &request.url,
            &request.headers,
            plx_net::net::Tls::Ca,
            None,
            Duration::from_secs(10),
            self.cancel,
            &mut |bytes| {
                total = total.saturating_add(bytes.len());
                if total > ott::MAX_CATALOG_BYTES || budget.expired(self.worker) {
                    limited = true;
                    return false;
                }
                !self.cancel.load(Ordering::Acquire) && consume(bytes)
            },
        );
        if limited {
            return Err("Provider response exceeds limit");
        }
        match end {
            plx_net::net::StreamEnd::Closed => Ok(200),
            plx_net::net::StreamEnd::Status(status) => Ok(status),
            plx_net::net::StreamEnd::Cancelled | plx_net::net::StreamEnd::Consumer => {
                Err("Provider request cancelled")
            }
            _ => Err("Provider request failed or timed out"),
        }
    }
}

pub(super) fn get_json(
    http: &mut dyn ProviderHttp,
    url: String,
    headers: Vec<String>,
) -> Result<Value, &'static str> {
    if http.cancelled() {
        return Err("Provider request cancelled");
    }
    let response = http.request(&Request {
        url,
        headers,
        body: None,
    })?;
    if http.cancelled() {
        return Err("Provider request cancelled");
    }
    if !(200..300).contains(&response.status) {
        return Err("Provider rejected request");
    }
    if response.body.len() > ott::MAX_CATALOG_BYTES {
        return Err("Provider response exceeds limit");
    }
    serde_json::from_slice(&response.body).map_err(|_| "Invalid provider JSON response")
}

pub(super) fn visit(
    worker: &OffFrame,
    profile: &ProviderProfile,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    profile.validate().map_err(|_| "Invalid provider profile")?;
    visit_with(profile, &mut NativeHttp { worker, cancel }, &mut |batch| {
        if cancel.load(Ordering::Acquire) {
            return Err("Provider request cancelled");
        }
        emit(batch)
    })
}
fn visit_with(
    profile: &ProviderProfile,
    http: &mut dyn ProviderHttp,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    profile.validate().map_err(|_| "Invalid provider profile")?;
    if http.cancelled() {
        return Err("Provider request cancelled");
    }
    match profile.kind {
        ProviderKind::M3u => match ott::m3u_source_kind(&profile.source) {
            Ok(ott::M3uSourceKind::Playlist) => m3u(profile, http, emit),
            Ok(ott::M3uSourceKind::PortalOnly) => Ok(()),
            Err(_) => Err("Invalid provider profile"),
        },
        ProviderKind::Xtream => xtream::visit(profile, http, emit),
        ProviderKind::Stalker => match ott::stalker::Config::parse(&profile.source)
            .map_err(|_| "Invalid Stalker profile")?
            .dialect()
        {
            ott::stalker::Dialect::Mag => stalker::visit(profile, http, emit),
            ott::stalker::Dialect::JsonRpc => stalker_api::visit(profile, http, emit),
        },
        _ => Err("Provider uses another catalogue adapter"),
    }
}
pub(super) fn resolve_cancellable(
    worker: &OffFrame,
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    cancel: &AtomicBool,
) -> Result<String, &'static str> {
    if cancel.load(Ordering::Acquire) {
        return Err("Provider request cancelled");
    }
    profile.validate().map_err(|_| "Invalid provider profile")?;
    resolve_with(profile, entry, &mut NativeHttp { worker, cancel })
}

#[cfg(test)]
#[path = "ott_provider_tests.rs"]
mod tests;
pub(super) fn resolve_with(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    http: &mut dyn ProviderHttp,
) -> Result<String, &'static str> {
    if profile.kind == ProviderKind::M3u
        && ott::m3u_source_kind(&profile.source) != Ok(ott::M3uSourceKind::Playlist)
    {
        return Err("Provider uses another playback adapter");
    }
    match &entry.media {
        MediaLocator::Xtream { .. } => xtream::resolve(profile, entry, http),
        MediaLocator::Stalker { .. } => stalker::resolve(profile, entry, http),
        MediaLocator::StalkerApi { .. } => stalker_api::resolve(profile, entry, http),
        MediaLocator::Http { url } => ott::http_url(url).ok_or("Invalid media URL"),
        _ => Err("Provider uses another playback adapter"),
    }
}

/// Run on the selected-item worker, never on the render/input thread. The caller
/// caches this by profile/entry generation and discards stale results.
pub(super) fn metadata(
    worker: &OffFrame,
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    now_s: i64,
    cancel: &AtomicBool,
) -> Result<Value, &'static str> {
    profile.validate().map_err(|_| "Invalid provider profile")?;
    metadata_with(profile, entry, now_s, &mut NativeHttp { worker, cancel })
}

fn metadata_with(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    _now_s: i64,
    http: &mut dyn ProviderHttp,
) -> Result<Value, &'static str> {
    if http.cancelled() {
        return Err("Provider request cancelled");
    }
    let result = match profile.kind {
        ProviderKind::Xtream => xtream::metadata(profile, entry, http),
        ProviderKind::Stalker => stalker::metadata(profile, entry, http),
        _ => Ok(
            serde_json::json!({"description":entry.description,"poster":entry.poster,
            "durationSeconds":entry.duration_ms.map(|ms|ms/1000)}),
        ),
    };
    if http.cancelled() {
        return Err("Provider request cancelled");
    }
    result
}

/// Addressed current/next guide result. Keep its context/query until receiver admission.
pub(super) fn guide_cancellable(
    worker: &OffFrame,
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    request: &ott::guide::Request,
    cancel: &AtomicBool,
) -> Result<ott::guide::ResultSet, &'static str> {
    guide_with(profile, entry, request, &mut NativeHttp { worker, cancel })
}

/// Optional catalogue captions share the selected request and its cancellation.
/// A sideband failure never turns a usable selected guide into partial row facts.
pub(super) fn guide_rows_cancellable(
    worker: &OffFrame, profile: &ProviderProfile, entry: &CatalogEntry,
    request: &ott::guide::Request, selectors: &[ott::guide::rows::Selector],
    cancel: &AtomicBool,
) -> (Result<ott::guide::ResultSet, &'static str>, ott::guide::rows::Outcome) {
    // Keep selected lookup's original per-source transport. Optional work shares
    // one deadline; it never cancels an unsatisfied selected-guide fallback.
    struct RowHttp<'a> { header: NativeHttp<'a>, guides: BoundedHttp<'a>, selected: bool }
    impl ProviderHttp for RowHttp<'_> {
        fn cancelled(&self) -> bool { self.header.cancelled() }
        fn row_guide_expired(&self) -> bool { self.guides.deadline.expired(self.guides.worker) }
        fn row_guide_selected(&mut self) { self.selected = true; }
        fn request(&mut self, request: &Request) -> Result<Response, &'static str> {
            // Until selected succeeds, preserve its original per-request budget
            // and fallback; optional row work may expire independently.
            if self.selected { self.guides.request(request) } else { self.header.request(request) }
        }
        fn stream(&mut self, request: &Request, consume: &mut dyn FnMut(&[u8]) -> bool) -> Result<(), &'static str> {
            self.header.stream(request, consume)
        }
    }
    let budget = Budget::start(worker, Duration::from_secs(60));
    let mut http = RowHttp { header: NativeHttp { worker, cancel },
        guides: BoundedHttp::new(worker, cancel, budget.deadline(), ott::MAX_CATALOG_BYTES), selected: false };
    live::m3u_guide_rows(profile, entry, request, selectors, &mut http)
}

fn guide_with(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    request: &ott::guide::Request,
    http: &mut dyn ProviderHttp,
) -> Result<ott::guide::ResultSet, &'static str> {
    profile.validate().map_err(|_| "Invalid provider profile")?;
    request
        .query
        .validate()
        .map_err(|_| "Invalid guide interval")?;
    if request.context.profile_id != profile.id || request.context.entry_id != entry.id {
        return Err("Guide context mismatch");
    }
    if matches!(request.query, ott::guide::Query::Records { .. })
        && !matches!((&profile.kind, &entry.media), (ProviderKind::M3u, MediaLocator::Http { .. }))
    {
        return Err("Records require an explicit M3U catch-up source");
    }
    if http.cancelled() {
        return Err("Provider request cancelled");
    }
    let result = match (&profile.kind, &entry.media) {
        (ProviderKind::M3u, MediaLocator::Http { .. }) if entry.duration_ms.is_none()
            && ott::m3u_source_kind(&profile.source) == Ok(ott::M3uSourceKind::Playlist) => {
            live::m3u_guide(profile, entry, request, http)
        }
        (ProviderKind::Stalker, MediaLocator::StalkerApi { .. }) => {
            stalker_api::guide(profile, entry, request, http)
        }
        (ProviderKind::Stalker, MediaLocator::Stalker { stream_kind, .. })
            if stream_kind == "itv" =>
        {
            if matches!(request.query, ott::guide::Query::Day { .. }) {
                Err("Provider offers only a short programme guide")
            } else {
                stalker::guide(profile, entry, request, http)
            }
        }
        (ProviderKind::Xtream, MediaLocator::Xtream { stream_kind, .. })
            if stream_kind == "live" =>
        {
            xtream::guide(profile, entry, request, http)
        }
        _ => Err("Provider uses another metadata adapter"),
    }?;
    if !result.can_publish(request, http.cancelled()) {
        return Err("Provider request cancelled");
    }
    Ok(result)
}

fn m3u(
    profile: &ProviderProfile,
    http: &mut dyn ProviderHttp,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let mut parser = ott::M3uParser::new(profile).map_err(|_| "Invalid M3U profile")?;
    let mut pending = Vec::new();
    let mut rows = Vec::new();
    let mut first = true;
    let mut failure = None;
    let result = http.stream(
        &Request {
            url: profile.source.clone(),
            headers: Vec::new(),
            body: None,
        },
        &mut |bytes| {
            for byte in bytes {
                if *byte != b'\n' {
                    if pending.len() >= ott::MAX_CATALOG_LINE_BYTES {
                        failure = Some("M3U line exceeds limit");
                        return false;
                    }
                    pending.push(*byte);
                    continue;
                }
                let parsed = std::str::from_utf8(&pending)
                    .map_err(|_| "Invalid M3U text")
                    .and_then(|line| parser.push_line(line).map_err(|_| "Invalid M3U playlist"));
                pending.clear();
                match parsed {
                    Ok(Some(row)) => rows.push(row),
                    Ok(None) => {}
                    Err(e) => {
                        failure = Some(e);
                        return false;
                    }
                }
                if !rows.is_empty() && (first || rows.len() >= 64) {
                    first = false;
                    if let Err(e) = emit(std::mem::take(&mut rows)) {
                        failure = Some(e);
                        return false;
                    }
                }
            }
            true
        },
    );
    if let Some(failure) = failure {
        return Err(failure);
    }
    result?;
    if http.cancelled() {
        return Err("Provider request cancelled");
    }
    if !pending.is_empty() {
        let line = std::str::from_utf8(&pending).map_err(|_| "Invalid M3U text")?;
        if let Some(row) = parser.push_line(line).map_err(|_| "Invalid M3U playlist")? {
            rows.push(row);
        }
    }
    if !rows.is_empty() {
        emit(rows)?;
    }
    Ok(())
}

#[cfg(all(test, feature = "hostsim"))]
mod bounded_http_tests {
    use super::*;
    use plx_net::net::StreamEnd;

    fn request() -> Request {
        Request {
            url: "https://metadata.invalid/private-token/manifest.json".into(),
            headers: Vec::new(),
            body: None,
        }
    }

    #[test]
    fn an_expired_queued_request_never_reaches_the_transport() {
        let worker = OffFrame::for_test();
        let cancel = AtomicBool::new(false);
        let deadline = Deadline::after(&worker, Duration::ZERO);
        let mut http = BoundedHttp::new(&worker, &cancel, deadline, 65_536);
        let error = http.request_with(&request(), |_, _| panic!("expired request dialled"));
        assert_eq!(error.err(), Some("Provider request timed out"));
    }

    #[test]
    fn a_cancelled_request_never_reaches_the_transport() {
        let worker = OffFrame::for_test();
        let cancel = AtomicBool::new(true);
        let deadline = Deadline::after(&worker, Duration::from_secs(60));
        let mut http = BoundedHttp::new(&worker, &cancel, deadline, 65_536);
        let error = http.request_with(&request(), |_, _| panic!("cancelled request dialled"));
        assert_eq!(error.err(), Some("Provider request cancelled"));
    }

    #[test]
    fn metadata_get_refuses_headers_bodies_bad_urls_and_invalid_caps_without_io() {
        let worker = OffFrame::for_test();
        let cancel = AtomicBool::new(false);
        let deadline = Deadline::after(&worker, Duration::from_secs(60));
        let mut http = BoundedHttp::new(&worker, &cancel, deadline, 65_536);
        let mut header = request();
        header.headers.push("Authorization: private".into());
        let mut body = request();
        body.body = Some(Vec::new());
        let mut url = request();
        url.url = "file:///private/manifest.json".into();
        for request in [header, body, url] {
            assert_eq!(
                http.request_with(&request, |_, _| panic!("invalid request dialled"))
                    .err(),
                Some("Invalid provider request")
            );
        }
        for cap in [0, ott::MAX_CATALOG_BYTES + 1] {
            let mut http = BoundedHttp::new(&worker, &cancel, deadline, cap);
            assert_eq!(
                http.request_with(&request(), |_, _| panic!("invalid cap dialled"))
                    .err(),
                Some("Invalid provider response limit")
            );
        }
    }

    #[test]
    fn metadata_stages_share_a_finite_nonrenewing_request_budget() {
        let worker = OffFrame::for_test();
        let cancel = AtomicBool::new(false);
        let deadline = Deadline::after(&worker, Duration::from_secs(60));
        let mut http = BoundedHttp::new(&worker, &cancel, deadline, 65_536);
        let mut first = 0;
        http.request_with(&request(), |timeouts, _| {
            first = timeouts.total_ms;
            assert!(first > 0 && first <= 60_000);
            StreamEnd::Closed
        })
        .unwrap();
        http.request_with(&request(), |timeouts, _| {
            assert!(timeouts.total_ms > 0 && timeouts.total_ms <= first);
            StreamEnd::Closed
        })
        .unwrap();
    }

    #[test]
    fn a_response_cannot_cross_its_bound_or_publish_after_cancellation() {
        let worker = OffFrame::for_test();
        let cancel = AtomicBool::new(false);
        let deadline = Deadline::after(&worker, Duration::from_secs(60));
        let mut http = BoundedHttp::new(&worker, &cancel, deadline, 3);
        let response = http
            .request_with(&request(), |_, consume| {
                assert!(consume(b"abc"));
                StreamEnd::Closed
            })
            .unwrap();
        assert_eq!(response.body, b"abc");
        let overflow = http.request_with(&request(), |_, consume| {
            assert!(consume(b"ab"));
            assert!(!consume(b"cd"));
            StreamEnd::Consumer
        });
        assert_eq!(overflow.err(), Some("Provider response exceeds limit"));
        let late = http.request_with(&request(), |_, consume| {
            assert!(consume(b"abc"));
            cancel.store(true, Ordering::Release);
            StreamEnd::Closed
        });
        assert_eq!(late.err(), Some("Provider request cancelled"));
    }

    #[test]
    fn status_and_failed_fetch_are_returned_without_response_text_or_urls() {
        let worker = OffFrame::for_test();
        let cancel = AtomicBool::new(false);
        let deadline = Deadline::after(&worker, Duration::from_secs(60));
        let mut http = BoundedHttp::new(&worker, &cancel, deadline, 65_536);
        let redirect = http
            .request_with(&request(), |_, _| StreamEnd::Status(302))
            .unwrap();
        assert_eq!(redirect.status, 302);
        assert!(redirect.body.is_empty());
        let failed = http.request_with(&request(), |_, _| StreamEnd::Idle);
        assert_eq!(failed.err(), Some("Provider request failed or timed out"));
        let retried = http
            .request_with(&request(), |_, consume| {
                assert!(consume(b"{}"));
                StreamEnd::Closed
            })
            .unwrap();
        assert_eq!(retried.status, 200);
        assert_eq!(retried.body, b"{}");
    }
}
