//! Independent FFmpeg software reader. No SHARED, TX, native Engine or child process.
//! All FFmpeg pointers and IO cursors belong to this worker; only copied bounded planes escape.
use super::*;
use crate::local_multiview::{ColorMatrix, ColorRange, Control, Error, FrameYuv, Limits, PcmChunk};
use plx_base::checkpoint::{Checkpoint, Flow};
use std::io::{Read, Seek, SeekFrom};
use std::time::{Duration, Instant};

pub(crate) fn available() -> bool {
    abi_ok() && unsafe { !avcodec_find_decoder_by_name(c"h264".as_ptr()).is_null() }
}
struct Check<'a>(&'a Control);
impl Checkpoint for Check<'_> {
    fn check(&mut self) -> Flow {
        if self.0.stopped() {
            Flow::Stop
        } else {
            Flow::Continue {
                next_check: Some(Instant::now() + Duration::from_millis(50)),
            }
        }
    }
}
fn playlist_header(bytes: &[u8]) -> bool {
    bytes.starts_with(b"#EXTM3U") || bytes.starts_with(b"\xef\xbb\xbf#EXTM3U")
}
const INTERRUPTED_RESPONSE: &[u8] = b"The request is interrupted by main process";
const DENIED_RESPONSE: &[u8] = b"Forbidden (No client found)";
const MISSING_STREAM_HTML: &[u8] = b"<h1>No stream id found</h1>";
const TRANSITIONAL_HTML: &[u8] = b"<h1>Forbidden</h1>";

/// Only exact known control replies are meaningful. Arbitrary HTML or media
/// bytes never authorize a retry or appear in a diagnostic.
fn provider_reply(bytes: &[u8]) -> Option<Error> {
    if bytes.len() > 128 { return None; }
    let text = bytes.trim_ascii();
    if text == INTERRUPTED_RESPONSE {
        return Some(Error::SourceInterrupted);
    }
    let text = text.strip_prefix(b"<h1>").and_then(|t| t.strip_suffix(b"</h1>")).unwrap_or(text);
    match text {
        DENIED_RESPONSE | b"Forbidden (No client session found)" => Some(Error::SourceDenied),
        b"No stream id found" => Some(Error::Source),
        _ => None,
    }
}

fn open_error(error: crate::curlio::OpenErr, c: &Control) -> Error {
    if c.stopped() { Error::Cancelled }
    else if matches!(error, crate::curlio::OpenErr::Status(401 | 403 | 429)) { Error::SourceDenied }
    else { Error::Source }
}

struct Remote {
    source: Box<crate::curlio::CurlSource>,
    deadline: Option<Instant>,
    failure: Option<crate::curlio::OpenErr>,
    startup_retries: u8,
    position: i64,
    prefix: std::collections::VecDeque<u8>,
}
impl Remote {
    fn open(url: &str, owned: bool, c: &Control) -> Result<Self, Error> {
        let deadline = Some(Instant::now() + Duration::from_secs(15));
        let mut check = Check(c);
        let source = if owned {
            crate::curlio::CurlSource::open_owned_private_checked(url, 0, deadline, &mut check)
                .map(|(source, _)| source)
        } else {
            // Each window owns a fresh private context. A sniffed master can issue a
            // cookie on its first response, before we know the source is HLS.
            crate::curlio::CurlSource::open_hls_private_checked(url, deadline, &mut check)
        }
        .map_err(|e| open_error(e, c))?;
        if !(200..300).contains(&source.status()) {
            return Err(open_error(crate::curlio::OpenErr::Status(source.status()), c));
        }
        Ok(Self {
            source,
            deadline: None,
            failure: None,
            startup_retries: 2,
            position: 0,
            prefix: Default::default(),
        })
    }
    fn reopen(&mut self, url: &str, c: &Control) -> Result<(), Error> {
        self.failure = None;
        let deadline = self.deadline();
        self.source.reopen_until(url, Some(deadline), &mut Check(c)).map_err(|e| {
            self.failure = Some(e);
            open_error(e, c)
        })?;
        if !(200..300).contains(&self.source.status()) {
            let error = crate::curlio::OpenErr::Status(self.source.status());
            self.failure = Some(error);
            return Err(open_error(error, c));
        }
        self.position = 0;
        self.prefix.clear();
        Ok(())
    }
    /// A successful lookup of an already-playing gateway session can return one of these exact
    /// HTML bodies after the provider stopped that stream. An authentication/limit status, a
    /// denial opening the source itself, and an arbitrary HTML body retain their ordinary errors.
    fn provider_error(&self, bytes: &[u8], existing: Option<&str>, c: &Control) -> Option<Error> {
        let error = provider_reply(bytes)?;
        let lost = matches!(bytes.trim_ascii(), MISSING_STREAM_HTML
            | b"<h1>Forbidden (No client found)</h1>" | b"<h1>Forbidden (No client session found)</h1>");
        if lost && existing.is_some_and(|url| self.played_gateway_resource(url, c)) {
            return Some(Error::LostProviderLease);
        }
        Some(error)
    }

    fn played_gateway_resource(&self, url: &str, c: &Control) -> bool {
        let Some(origin) = c.hls_origin.get() else { return false; };
        self.source.status() == 200 && c.video_decoded.load(Ordering::Acquire)
            && c.hls_live.load(Ordering::Acquire) && !c.paused.load(Ordering::Acquire) && !c.stopped()
            && crate::local_multiview::hls_handoff_origin(url).as_ref() == Some(origin)
            && crate::local_multiview::hls_handoff_origin(self.source.effective_url()).as_ref() == Some(origin)
    }

    fn fetch(&mut self, url: &str, cap: usize, is_segment: bool, c: &Control) -> Result<Vec<u8>, Error> {
        self.reopen(url, c)?;
        let bytes = collect(self, cap, c)?;
        if let Some(error) = self.provider_error(&bytes, is_segment.then_some(url), c) { return Err(error); }
        Ok(bytes)
    }
    fn deadline(&self) -> Instant {
        let request = Instant::now() + Duration::from_secs(15);
        self.deadline.map_or(request, |overall| request.min(overall))
    }
    fn retryable(&self) -> bool {
        self.failure.is_some_and(|e| matches!(hls_curl_open_outcome(e),
            TraceOutcome::Transport | TraceOutcome::Deadline | TraceOutcome::ServerState))
    }
    /// The root and selected variant share a retry allowance and private
    /// transfer context. Reopening preserves cookies received during redirects.
    fn startup<T>(&mut self, url: &str, mut open: bool, c: &Control,
        parse: impl Fn(&mut Self, &Control) -> Result<T, Error>) -> Result<T, Error> {
        let previous_deadline = self.deadline;
        self.deadline = Some(self.deadline());
        let result = loop {
            let result = (if open { self.reopen(url, c) } else { Ok(()) })
                .and_then(|()| parse(self, c));
            let transient = matches!(&result, Err(Error::SourceInterrupted))
                || (matches!(&result, Err(Error::Source)) && self.retryable());
            let delay = Duration::from_millis(if self.startup_retries == 2 { 400 } else { 1000 });
            if !transient || self.startup_retries == 0 || self.deadline().saturating_duration_since(Instant::now()) <= delay {
                break result;
            }
            if !c.wait(delay) { break Err(Error::Cancelled); }
            self.startup_retries -= 1;
            open = true;
        };
        self.deadline = previous_deadline;
        result
    }
    fn sniff_hls(&mut self, c: &Control) -> Result<bool, Error> {
        let mut bytes = Vec::new();
        while bytes.len() < 16 {
            let mut head = [0u8; 16];
            let n = self.read_raw(&mut head[..16 - bytes.len()], c)?;
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&head[..n]);
        }
        let hls = playlist_header(&bytes);
        // Most progressive files need just the first 16 bytes. Read a small
        // potential control body to EOF before recognizing its exact meaning.
        let head = bytes.trim_ascii_start();
        if !head.is_empty() && !hls && (INTERRUPTED_RESPONSE.starts_with(head)
            || DENIED_RESPONSE.starts_with(head) || head.starts_with(b"<h1>")) {
            while bytes.len() < 128 {
                let mut rest = [0; 128];
                let n = self.read_raw(&mut rest[..128 - bytes.len()], c)?;
                if n == 0 {
                    if let Some(error) = provider_reply(&bytes) { return Err(error); }
                    break;
                }
                bytes.extend_from_slice(&rest[..n]);
            }
        }
        self.prefix.extend(bytes);
        Ok(hls)
    }
    fn read(&mut self, b: &mut [u8], c: &Control) -> Result<usize, Error> {
        if !self.prefix.is_empty() {
            let n = b.len().min(self.prefix.len());
            for out in &mut b[..n] {
                *out = self.prefix.pop_front().ok_or(Error::Source)?;
            }
            return Ok(n);
        }
        self.read_raw(b, c)
    }
    fn read_raw(&mut self, b: &mut [u8], c: &Control) -> Result<usize, Error> {
        let n = self.source.read_until(
            b,
            Some(self.deadline()),
            &mut Check(c),
        );
        if n < 0 {
            self.failure = self.source.body_failure().map(|f| crate::curlio::OpenErr::Transport(f.rc));
            Err(if c.stopped() {
                Error::Cancelled
            } else {
                Error::Source
            })
        } else {
            self.position = self.position.saturating_add(i64::from(n));
            Ok(n as usize)
        }
    }
}
fn collect(r: &mut Remote, cap: usize, c: &Control) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let mut scratch = [0; 16 * 1024];
    let expires = r.deadline();
    loop {
        if Instant::now() >= expires {
            r.failure = Some(crate::curlio::OpenErr::Deadline);
            return Err(Error::Source);
        }
        let n = r.read(&mut scratch, c)?;
        if n == 0 {
            return Ok(out);
        }
        if out.len().checked_add(n).is_none_or(|n| n > cap) {
            return Err(Error::Budget);
        }
        out.extend_from_slice(&scratch[..n]);
    }
}
// Only a transport-validated effective URL can become a playlist's relative base.
// Kept together with its bytes through explicit URLs, sniffed responses and refreshes.
struct Manifest {
    effective_url: String,
    bytes: Vec<u8>,
}
impl Manifest {
    fn from_remote(remote: &mut Remote, c: &Control) -> Result<Self, Error> {
        Self::read(remote, None, c)
    }
    fn read(remote: &mut Remote, existing: Option<&str>, c: &Control) -> Result<Self, Error> {
        let manifest = Self::raw(remote, c)?;
        if let Some(error) = remote.provider_error(&manifest.bytes, existing, c) { return Err(error); }
        Ok(manifest)
    }
    fn raw(remote: &mut Remote, c: &Control) -> Result<Self, Error> {
        let effective_url = remote.source.effective_url().to_owned();
        resource(&effective_url)?;
        let bytes = collect(remote, 512 * 1024, c)?;
        Ok(Self { effective_url, bytes })
    }
    fn fetch(remote: &mut Remote, url: &str, c: &Control) -> Result<Self, Error> {
        remote.reopen(url, c)?;
        Self::read(remote, Some(url), c)
    }
    fn text(&self) -> Result<&str, Error> {
        std::str::from_utf8(&self.bytes).map_err(|_| Error::Playlist)
    }
    fn media(&self) -> Result<Playlist, Error> {
        parse_playlist(&self.effective_url, self.text()?)
    }
    fn variant(&self) -> Result<Option<String>, Error> {
        select_variant(&self.effective_url, self.text()?)
    }
}
#[derive(Clone)]
struct Segment {
    sequence: u64,
    url: String,
}
struct Playlist {
    segments: Vec<Segment>,
    init: Option<String>,
    ended: bool,
    target: Duration,
}
fn resource(url: &str) -> Result<crate::hls::Resource, Error> {
    let port = if url.starts_with("https://") {
        443
    } else if url.starts_with("http://") {
        80
    } else {
        return Err(Error::Playlist);
    };
    // Origin::parse deliberately defaults missing ports to Plex's 32400.
    // Generic HLS URIs use the scheme default; explicit ports remain authoritative.
    let origin = plx_plex::plex::Origin::parse_connection(url, port).ok_or(Error::Playlist)?;
    let (_, path) = plx_plex::plex::origin::split(url);
    crate::hls::Resource::new(origin, path.to_owned()).map_err(|_| Error::Playlist)
}
fn child(base: &str, reference: &str) -> Result<String, Error> {
    let base = resource(base)?;
    let absolute;
    let reference = if reference.starts_with("https://") || reference.starts_with("http://") {
        let r = resource(reference)?;
        absolute = format!("{}{}", r.origin.base(), r.path);
        absolute.as_str()
    } else {
        reference
    };
    let child = base.resolve(reference).map_err(|_| Error::Playlist)?;
    let path = match crate::hls::InheritedAuth::capture(&base) {
        Ok(auth) => auth.request_path(&child).map_err(|_| Error::Playlist)?,
        Err(_) => child.path.clone(),
    };
    Ok(format!("{}{}", child.origin.base(), path))
}
fn attr<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let mut rest = text;
    while !rest.is_empty() {
        let (name, value) = rest.split_once('=')?;
        if let Some(quoted) = value.strip_prefix('"') {
            let end = quoted.find('"')?;
            if name == key {
                return Some(&quoted[..end]);
            }
            rest = quoted[end + 1..].strip_prefix(',').unwrap_or("");
        } else {
            let (value, next) = value.split_once(',').unwrap_or((value, ""));
            if name == key {
                return Some(value);
            }
            rest = next;
        }
    }
    None
}
fn parse_playlist(base: &str, text: &str) -> Result<Playlist, Error> {
    if text
        .trim_start_matches('\u{feff}')
        .lines()
        .next()
        .map(str::trim)
        != Some("#EXTM3U")
        || text.lines().count() > 8192
    {
        return Err(Error::Playlist);
    }
    let mut out = Playlist {
        segments: Vec::new(),
        init: None,
        ended: false,
        target: Duration::from_secs(2),
    };
    let mut seq = 0u64;
    let mut duration = false;
    for line in text.lines().map(str::trim).filter(|s| !s.is_empty()) {
        if line.starts_with("#EXT-X-KEY:")
            || line.starts_with("#EXT-X-SESSION-KEY:")
            || line.starts_with("#EXT-X-BYTERANGE:")
            || line == "#EXT-X-DISCONTINUITY"
            || line.starts_with("#EXT-X-GAP")
            || line.starts_with("#EXT-X-PART:")
            || line.starts_with("#EXT-X-DEFINE:")
            || line == "#EXT-X-I-FRAMES-ONLY"
        {
            return Err(Error::Unsupported);
        }
        if let Some(value) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            seq = value.parse().map_err(|_| Error::Playlist)?;
        } else if let Some(value) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            let secs: u64 = value.parse().map_err(|_| Error::Playlist)?;
            if !(1..=60).contains(&secs) {
                return Err(Error::Playlist);
            }
            out.target = Duration::from_secs(secs);
        } else if let Some(value) = line.strip_prefix("#EXT-X-MAP:") {
            if !out.segments.is_empty() {
                return Err(Error::Unsupported);
            }
            if value.contains("BYTERANGE=") {
                return Err(Error::Unsupported);
            }
            let url = child(base, attr(value, "URI").ok_or(Error::Playlist)?)?;
            if out.init.as_ref().is_some_and(|old| old != &url) {
                return Err(Error::Unsupported);
            }
            out.init = Some(url);
        } else if line == "#EXT-X-ENDLIST" {
            out.ended = true;
        } else if let Some(value) = line.strip_prefix("#EXTINF:") {
            if duration {
                return Err(Error::Playlist);
            }
            let seconds = value
                .split(',')
                .next()
                .ok_or(Error::Playlist)?
                .parse::<f64>()
                .map_err(|_| Error::Playlist)?;
            if !seconds.is_finite() || seconds <= 0.0 || seconds > 60.0 {
                return Err(Error::Unsupported);
            }
            duration = true;
        } else if !line.starts_with('#') {
            if !duration || out.segments.len() >= 4096 {
                return Err(Error::Playlist);
            }
            out.segments.push(Segment {
                sequence: seq,
                url: child(base, line)?,
            });
            seq = seq.checked_add(1).ok_or(Error::Playlist)?;
            duration = false;
        }
    }
    if out.segments.is_empty() || duration {
        return Err(Error::Playlist);
    }
    Ok(out)
}
fn select_variant(url: &str, text: &str) -> Result<Option<String>, Error> {
    if text
        .trim_start_matches('\u{feff}')
        .lines()
        .next()
        .map(str::trim)
        != Some("#EXTM3U")
    {
        return Err(Error::Playlist);
    }
    let mut selected: Option<(u64, String)> = None;
    let mut variant = None;
    for line in text.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            if attr(value, "AUDIO").is_some() || attr(value, "VIDEO").is_some() {
                return Err(Error::Unsupported);
            }
            variant = Some(
                attr(value, "BANDWIDTH")
                    .ok_or(Error::Playlist)?
                    .parse::<u64>()
                    .map_err(|_| Error::Playlist)?,
            );
        } else if !line.is_empty() && !line.starts_with('#') {
            if let Some(bandwidth) = variant.take() {
                if selected.as_ref().is_none_or(|(b, _)| bandwidth < *b) {
                    selected = Some((bandwidth, child(url, line)?));
                }
            }
        }
    }

    Ok(selected.map(|(_, url)| url))
}
fn poll_wait(checked: Instant, target: Duration, now: Instant) -> Duration {
    // Decoding the last segment may already have spent this entire interval.
    // Only an unchanged, recently checked edge needs a delay.
    (checked + target.min(Duration::from_secs(2)) / 2).saturating_duration_since(now)
}

fn admission_read_started(url: &str, c: &Control) -> Option<u64> {
    if !c.hls_live.load(Ordering::Acquire) { return None; }
    let admitted = c.hls_origin.get()?;
    (crate::local_multiview::hls_handoff_origin(url).as_ref() == Some(admitted))
        .then(crate::local_multiview::hls_request_started)
}

fn finish_admission_read(order: Option<u64>, effective_url: &str, c: &Control) {
    if let Some(order) = order {
        if !c.stopped() && crate::local_multiview::hls_handoff_origin(effective_url).as_ref() == c.hls_origin.get() {
            c.hls_request.store(order, Ordering::Release);
        }
    }
}

fn media_segment_bytes(bytes: &[u8]) -> bool {
    // A 200 HTML or text reply is not a media read. Confirm MPEG-TS sync or a fragmented MP4 box.
    (bytes.len() >= 188 * 2 && bytes[0] == 0x47 && bytes[188] == 0x47)
        || (bytes.len() >= 8 && matches!(&bytes[4..8], b"styp" | b"sidx" | b"moof" | b"mdat"))
}

struct Hls {
    remote: Remote,
    url: String,
    playlist: Playlist,
    next: u64,
    bytes: Vec<u8>,
    pos: usize,
    init_sent: bool,
    delivered_segment: bool,
    teardown_checked: bool,
    reprobe: bool,
    checked: Instant,
    cap: usize,
}
impl Hls {
    fn open(url: String, cap: usize, c: &Control) -> Result<Self, Error> {
        Self::open_response(url, cap, c, None)
    }
    fn open_response(
        mut url: String,
        cap: usize,
        c: &Control,
        first: Option<Remote>,
    ) -> Result<Self, Error> {
        // At most one master indirection. Every response carries its own validated base.
        let mut remote = match first {
            Some(remote) => remote,
            None => Remote::open(&url, false, c)?,
        };
        let first = remote.startup(&url, false, c, Manifest::from_remote)?;
        let source_origin = crate::local_multiview::hls_handoff_origin(&url);
        let advertised = remote.source.multiview_refresh_peers()
            && source_origin.is_some()
            && crate::local_multiview::hls_handoff_origin(&first.effective_url) == source_origin;
        let manifest = if let Some(child) = first.variant()? {
            if advertised {
                if let Some(origin) = &source_origin {
                    c.hls_origin.get_or_init(|| origin.clone());
                    c.hls_live.store(true, Ordering::Release);
                }
            }
            remote.startup(&child, true, c, Manifest::from_remote)?
        } else {
            first
        };
        url = manifest.effective_url.clone();
        let playlist = manifest.media()?;
        let admitted_origin = source_origin.filter(|origin| {
            !playlist.ended && (advertised || remote.source.multiview_refresh_peers())
                && crate::local_multiview::hls_handoff_origin(&url).as_ref() == Some(origin)
        });
        let admitted = admitted_origin.is_some();
        if let Some(origin) = admitted_origin { let _ = c.hls_origin.set(origin); }
        c.hls_live.store(admitted, Ordering::Release);
        let start = if playlist.ended {
            0
        } else {
            playlist.segments.len().saturating_sub(2)
        };
        let next = playlist.segments[start].sequence;
        let checked = Instant::now();
        Ok(Self {
            remote,
            url,
            playlist,
            next,
            bytes: Vec::new(),
            pos: 0,
            init_sent: false,
            delivered_segment: false,
            teardown_checked: false,
            reprobe: false,
            checked,
            cap,
        })
    }
    fn refresh(&mut self, c: &Control) -> Result<(), Error> {
        let mut order = admission_read_started(&self.url, c);
        let mut manifest = Manifest::fetch(&mut self.remote, &self.url, c)?;
        // The source can briefly report this text while retiring a previously verified leaf.
        // Check the same leaf once. Only an independent, explicit missing-stream reply can
        // authorize a new session; an HTTP denial or another generic page remains terminal.
        if manifest.bytes == TRANSITIONAL_HTML && !self.teardown_checked
            && self.checked.elapsed() < Duration::from_secs(100)
            && c.hls_request.load(Ordering::Acquire) != 0 && self.remote.played_gateway_resource(&self.url, c)
        {
            self.teardown_checked = true;
            let pause = Duration::from_millis(800);
            if self.remote.deadline().saturating_duration_since(Instant::now()) <= pause {
                return Err(Error::Playlist);
            }
            if !c.wait(pause) { return Err(Error::Cancelled); }
            if c.paused.load(Ordering::Acquire) { return Err(Error::Playlist); }
            order = admission_read_started(&self.url, c);
            self.remote.reopen(&self.url, c)?;
            manifest = Manifest::raw(&mut self.remote, c)?;
            if manifest.bytes.trim_ascii() == MISSING_STREAM_HTML && self.remote.played_gateway_resource(&self.url, c) {
                return Err(Error::LostProviderLease);
            }
            if let Some(error) = provider_reply(&manifest.bytes) { return Err(error); }
        }
        let fresh = manifest.media()?;
        if fresh.init != self.playlist.init {
            return Err(Error::Unsupported);
        }
        if fresh.ended { c.hls_live.store(false, Ordering::Release); }
        else { finish_admission_read(order, &manifest.effective_url, c); }
        self.checked = Instant::now();
        self.url = manifest.effective_url;
        self.playlist = fresh;
        Ok(())
    }

    // Budget only the current demand for more bytes. A paused decoder can
    // still be inside AVIO when the UI suspends it between manifest requests.
    fn arm_request(&mut self, c: &Control, deadline: &mut Instant) -> Result<(), Error> {
        if c.paused.load(Ordering::Acquire) {
            let paused = Instant::now();
            while c.paused.load(Ordering::Acquire) {
                if !c.wait(Duration::from_millis(10)) { return Err(Error::Cancelled); }
            }
            *deadline += paused.elapsed();
        }
        if c.stopped() { return Err(Error::Cancelled); }
        if Instant::now() >= *deadline { return Err(Error::Source); }
        self.remote.deadline = Some(*deadline);
        Ok(())
    }

    fn refresh_retry(&mut self, c: &Control, remaining: &mut u8, deadline: &mut Instant) -> Result<(), Error> {
        loop {
            self.arm_request(c, deadline)?;
            let verified_before = self.teardown_checked;
            match self.refresh(c) {
                Err(error) if *remaining > 0 && verified_before == self.teardown_checked
                    && (error == Error::SourceInterrupted
                    || (error == Error::Source && self.remote.retryable())) => {
                    *remaining -= 1;
                    if !c.wait(Duration::from_millis(200).min(deadline.saturating_duration_since(Instant::now()))) { return Err(Error::Cancelled); }
                }
                result => return result,
            }
        }
    }

    fn read(&mut self, out: &mut [u8], c: &Control) -> Result<usize, Error> {
        self.read_with_budget(out, c, Duration::from_secs(30))
    }

    fn read_with_budget(&mut self, out: &mut [u8], c: &Control, budget: Duration) -> Result<usize, Error> {
        if c.stopped() { return Err(Error::Cancelled); }
        let mut deadline = Instant::now() + budget;
        let mut remaining = 2;
        while self.pos == self.bytes.len() {
            if self.reprobe { return Err(Error::Source); }
            self.arm_request(c, &mut deadline)?;
            if !self.init_sent {
                if let Some(url) = &self.playlist.init {
                    self.bytes = self.remote.fetch(url, self.cap, false, c)?;
                    if self.bytes.is_empty() { return Err(Error::Source); }
                    self.pos = 0;
                }
                self.init_sent = true;
                if !self.bytes.is_empty() { break; }
            }
            if let Some(segment) = self.playlist.segments.iter().find(|s| s.sequence == self.next) {
                let order = admission_read_started(&segment.url, c);
                let result = self.remote.fetch(&segment.url, self.cap, true, c);
                match result {
                    Ok(bytes) if !bytes.is_empty() => {
                        if order.is_some() && media_segment_bytes(&bytes) {
                            finish_admission_read(order, self.remote.source.effective_url(), c);
                        }
                        self.bytes = bytes;
                        self.pos = 0;
                        self.next = self.next.checked_add(1).ok_or(Error::Playlist)?;
                        self.delivered_segment = true;
                        break;
                    }
                    Err(error) if !self.playlist.ended && remaining > 0
                        && (error == Error::SourceInterrupted || (error == Error::Source
                            && (self.remote.retryable() || matches!(self.remote.failure,
                                Some(crate::curlio::OpenErr::Status(404 | 410)))))) => {
                        // Do not spend another request on a stale signed URI until
                        // the selected playlist has supplied a current segment.
                        remaining -= 1;
                        if !c.wait(Duration::from_millis(200).min(deadline.saturating_duration_since(Instant::now()))) { return Err(Error::Cancelled); }
                        self.refresh_retry(c, &mut remaining, &mut deadline)?;
                        continue;
                    }
                    Ok(_) => return Err(Error::Source),
                    Err(e) => return Err(e),
                }
            }
            if let Some(first) = self.playlist.segments.first().filter(|s| s.sequence > self.next) {
                self.next = first.sequence;
                if self.delivered_segment {
                    // A raw splice would leave reference pictures and audio
                    // clocks from the skipped interval in FFmpeg. Reopen just
                    // this decoder, retaining the selected session and cookie.
                    self.reprobe = true;
                    return Err(Error::Source);
                }
                continue;
            }
            if self.playlist.ended {
                return Ok(0);
            }
            let now = Instant::now();
            let wait = poll_wait(self.checked, self.playlist.target, now).min(deadline.saturating_duration_since(now));
            if !c.wait(wait) {
                return Err(Error::Cancelled);
            }
            self.refresh_retry(c, &mut remaining, &mut deadline)?;
        }
        let n = out.len().min(self.bytes.len() - self.pos);
        out[..n].copy_from_slice(&self.bytes[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}
enum Input {
    Local(file_source::FileSource),
    Http(Remote),
    Hls(Hls),
}
struct Io<'a> {
    input: &'a mut Input,
    control: &'a Control,
    error: Option<Error>,
}
extern "C" fn read(p: *mut c_void, out: *mut u8, len: c_int) -> c_int {
    if len <= 0 || out.is_null() || p.is_null() {
        return -1;
    }
    let io = unsafe { &mut *(p as *mut Io<'_>) };
    if io.control.stopped() {
        return -1;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let out = unsafe { std::slice::from_raw_parts_mut(out, len as usize) };
        match &mut io.input {
            Input::Local(f) => f.read(out).map_err(|_| Error::Source),
            Input::Http(r) => r.read(out, io.control),
            Input::Hls(h) => h.read(out, io.control),
        }
    }))
    .unwrap_or(Err(Error::Source));
    match result {
        Ok(0) => AVERROR_EOF,
        Ok(n) => n as c_int,
        Err(e) => {
            io.error = Some(e);
            -1
        }
    }
}
extern "C" fn seek(p: *mut c_void, offset: i64, whence: c_int) -> i64 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        seek_inner(p, offset, whence)
    }))
    .unwrap_or(-1)
}
fn seek_inner(p: *mut c_void, offset: i64, whence: c_int) -> i64 {
    if p.is_null() {
        return -1;
    }
    let io = unsafe { &mut *(p as *mut Io<'_>) };
    if io.control.stopped() {
        return -1;
    }
    match &mut io.input {
        Input::Local(f) => {
            if whence & 0x10000 != 0 {
                return f.size();
            }
            let to = match whence & !0x20000 {
                0 if offset >= 0 => SeekFrom::Start(offset as u64),
                1 => SeekFrom::Current(offset),
                2 => SeekFrom::End(offset),
                _ => return -1,
            };
            f.seek(to)
                .ok()
                .and_then(|v| i64::try_from(v).ok())
                .unwrap_or(-1)
        }
        Input::Http(r) => {
            if whence & 0x10000 != 0 {
                r.source.size()
            } else {
                let at = match whence & !0x20000 {
                    0 => Some(offset),
                    1 => r
                        .position
                        .checked_sub(r.prefix.len() as i64)
                        .and_then(|p| p.checked_add(offset)),
                    2 if r.source.size() >= 0 => r.source.size().checked_add(offset),
                    _ => None,
                };
                match at.filter(|v| *v >= 0) {
                    Some(at)
                        if r.source.seek_until(
                            at,
                            Some(Instant::now() + Duration::from_secs(15)),
                            &mut Check(io.control),
                        ) =>
                    {
                        r.position = at;
                        r.prefix.clear();
                        at
                    }
                    _ => -1,
                }
            }
        }
        Input::Hls(_) => -1,
    }
}
struct Decode {
    fmt: *mut AVFormatContext,
    avio: *mut AVIOContext,
    codec: *mut AVCodecContext,
    frame: *mut c_void,
    packet: *mut AVPacket,
}
impl Drop for Decode {
    fn drop(&mut self) {
        unsafe {
            av_packet_free(&mut self.packet);
            av_frame_free(&mut self.frame);
            avcodec_free_context(&mut self.codec);
            avformat_close_input(&mut self.fmt);
            free_avio(self.avio);
        }
    }
}
unsafe fn plane(frame: *const u8, index: usize, w: usize, h: usize) -> Result<Vec<u8>, Error> {
    let ptr = std::ptr::read_unaligned(
        frame.add(OFF_FRAME_DATA + index * std::mem::size_of::<*const u8>()) as *const *const u8,
    );
    let stride = std::ptr::read_unaligned(frame.add(OFF_FRAME_LINESIZE + index * 4) as *const i32);
    if ptr.is_null() || (stride.unsigned_abs() as usize) < w || stride.unsigned_abs() > 32768 {
        return Err(Error::Decode);
    }
    let mut out = vec![0; w.checked_mul(h).ok_or(Error::Budget)?];
    for row in 0..h {
        out[row * w..(row + 1) * w].copy_from_slice(std::slice::from_raw_parts(
            ptr.offset(row as isize * stride as isize),
            w,
        ));
    }
    Ok(out)
}
fn dimensions(w: i32, h: i32, limits: Limits) -> Result<(usize, usize, usize, usize), Error> {
    if w <= 0 || h <= 0 || (w as u64) * (h as u64) > u64::from(limits.max_source_pixels) {
        return Err(Error::Budget);
    }
    let (w, h) = (w as usize, h as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    if w * h + 2 * cw * ch > limits.max_frame_bytes {
        return Err(Error::Budget);
    }
    Ok((w, h, cw, ch))
}
/// After a seek, keep only PCM overlapping or following the target, with at
/// most one actual stereo48k chunk of overlap. Untimed packets are decoded for
/// codec continuity but dropped after a seek: a guessed zero PTS cannot prove
/// that preroll has ended. With no seek floor, normal playback is unchanged.
fn audio_reaches_seek_floor(
    pts_us: Option<i64>,
    bytes: usize,
    floor: Option<(u64, i64)>,
    generation: u64,
) -> bool {
    let Some((owner, target)) = floor else {
        return true;
    };
    if owner != generation || bytes == 0 || bytes % 4 != 0 {
        return false;
    }
    let Some(pts) = pts_us else {
        return false;
    };
    let Some(duration) = (bytes / 4)
        .checked_mul(1_000_000)
        .and_then(|n| n.checked_add(47_999))
        .map(|n| n / 48_000)
        .and_then(|n| i64::try_from(n).ok())
    else {
        return false;
    };
    pts.checked_add(duration).is_some_and(|end| end >= target)
}

pub(crate) fn run(
    _off: &plx_base::task::OffFrame,
    source: ExternalSource,
    limits: Limits,
    control: &Control,
) -> Result<(), Error> {
    unsafe { run_inner(source, limits, control) }
}
unsafe fn run_inner(source: ExternalSource, limits: Limits, c: &Control) -> Result<(), Error> {
    if !available() {
        return Err(Error::Unavailable);
    }
    let mut input = match source {
        ExternalSource::HeldLocal(file) => {
            Input::Local(file_source::FileSource::held(&file).map_err(|_| Error::Source)?)
        }
        ExternalSource::Http(url) => {
            let mut remote = Remote::open(&url, false, c)?;
            if remote.startup(&url, false, c, Remote::sniff_hls)? {
                Input::Hls(Hls::open_response(
                    url,
                    limits.max_object_bytes,
                    c,
                    Some(remote),
                )?)
            } else {
                Input::Http(remote)
            }
        }
        ExternalSource::OwnedHttp(url) => Input::Http(Remote::open(&url, true, c)?),
        ExternalSource::Hls(url) => Input::Hls(Hls::open(url, limits.max_object_bytes, c)?),
        ExternalSource::Local(_) => return Err(Error::Unsupported),
    };
    let mut reopens = std::collections::VecDeque::new();
    loop {
        let result = decode_input(&mut input, limits, c);
        let Input::Hls(hls) = &mut input else { return result };
        if !hls.reprobe || c.stopped() { return result; }
        let now = Instant::now();
        while reopens.front().is_some_and(|then: &Instant| now.duration_since(*then) >= Duration::from_secs(30)) {
            reopens.pop_front();
        }
        if reopens.len() >= 3 { return Err(Error::Source); }
        reopens.push_back(now);
        hls.reprobe = false;
        hls.delivered_segment = false;
        hls.init_sent = false;
        hls.bytes.clear();
        hls.pos = 0;
        c.timeline.fetch_add(1, Ordering::AcqRel);
        c.audio_generation.fetch_add(1, Ordering::AcqRel);
        let mut mailbox = c.mailbox.lock().unwrap_or_else(|e| e.into_inner());
        mailbox.video = None;
        mailbox.audio.clear();
    }
}

unsafe fn decode_input(input: &mut Input, limits: Limits, c: &Control) -> Result<(), Error> {
    let seekable = match &input {
        Input::Local(_) => true,
        Input::Http(remote) => remote.source.supports_byte_ranges(),
        Input::Hls(_) => false,
    };
    let has_finite_duration = !matches!(&input, Input::Hls(_));
    // On nonseekable TS the header reader consumes its full byte probe even when
    // PAT and PMT are present. Fit discovery within the downloaded live segment:
    // reading future segments here can make the next sequence expire while the
    // decoder plays its queued probe packets. Local files and fMP4 keep their budget.
    let mut probe_bytes = 1024 * 1024;
    if let Input::Hls(hls) = input {
        if !hls.playlist.ended && hls.playlist.init.is_none() {
            hls.read(&mut [], c)?;
            if hls.bytes.first() == Some(&0x47)
                && hls.bytes.get(188) == Some(&0x47)
                && hls.bytes.get(376) == Some(&0x47)
            {
                probe_bytes = (hls.bytes.len() / 2).clamp(2048, probe_bytes);
            }
        }
    }
    let probe_bytes = CString::new(probe_bytes.to_string()).map_err(|_| Error::Budget)?;
    let mut io = Box::new(Io {
        input,
        control: c,
        error: None,
    });
    let mut d = Decode {
        fmt: std::ptr::null_mut(),
        avio: std::ptr::null_mut(),
        codec: std::ptr::null_mut(),
        frame: std::ptr::null_mut(),
        packet: std::ptr::null_mut(),
    };
    let buffer = av_malloc(64 * 1024) as *mut u8;
    if buffer.is_null() {
        return Err(Error::Budget);
    }
    d.avio = avio_alloc_context(
        buffer,
        64 * 1024,
        0,
        (&mut *io as *mut Io<'_>).cast(),
        Some(read),
        None,
        if seekable { Some(seek) } else { None },
    );
    if d.avio.is_null() {
        free_ptr(buffer.cast());
        return Err(Error::Budget);
    }
    d.fmt = avformat_alloc_context();
    if d.fmt.is_null() {
        return Err(Error::Budget);
    }
    (*d.fmt).pb = d.avio;
    let mut opts = std::ptr::null_mut();
    let bounded = av_dict_set(&mut opts, c"probesize".as_ptr(), probe_bytes.as_ptr(), 0) >= 0
        && av_dict_set(
            &mut opts,
            c"analyzeduration".as_ptr(),
            c"2000000".as_ptr(),
            0,
        ) >= 0
        && av_dict_set(&mut opts, c"max_streams".as_ptr(), c"16".as_ptr(), 0) >= 0
        && av_dict_set(&mut opts, c"max_probe_packets".as_ptr(), c"128".as_ptr(), 0) >= 0;
    if !bounded {
        av_dict_free(&mut opts);
        return Err(Error::Budget);
    }
    let opened = avformat_open_input(
        &mut d.fmt,
        std::ptr::null(),
        std::ptr::null_mut(),
        &mut opts,
    );
    av_dict_free(&mut opts);
    if opened < 0 || avformat_find_stream_info(d.fmt, std::ptr::null_mut()) < 0 {
        return Err(io.error.unwrap_or(Error::Decode));
    }
    let vi = av_find_best_stream(d.fmt, 0, -1, -1, std::ptr::null_mut(), 0);
    if vi < 0 {
        return Err(Error::Unsupported);
    }
    let stream = *(*d.fmt).streams.add(vi as usize);
    let cp = stream_codecpar(stream);
    if cp.is_null() {
        return Err(Error::Decode);
    }
    dimensions((*cp).width, (*cp).height, limits)?;
    let codec = avcodec_find_decoder((*cp).codec_id);
    if codec.is_null() {
        return Err(Error::Unsupported);
    }
    d.codec = avcodec_alloc_context3(codec);
    if d.codec.is_null() {
        return Err(Error::Budget);
    }
    // One software decode thread per slot, with an allocation ceiling before decoder open.
    if avcodec_parameters_to_context(d.codec, cp) < 0 {
        return Err(Error::Decode);
    }
    let mut options = std::ptr::null_mut();
    let pixels = CString::new(limits.max_source_pixels.to_string()).map_err(|_| Error::Budget)?;
    if av_dict_set(&mut options, c"threads".as_ptr(), c"1".as_ptr(), 0) < 0
        || av_dict_set(&mut options, c"max_pixels".as_ptr(), pixels.as_ptr(), 0) < 0
    {
        av_dict_free(&mut options);
        return Err(Error::Budget);
    }
    let opened = avcodec_open2(d.codec, codec, &mut options);
    let unrecognized_limits = !options.is_null();
    av_dict_free(&mut options);
    if opened < 0 || unrecognized_limits {
        return Err(Error::Decode);
    }
    d.frame = av_frame_alloc();
    d.packet = av_packet_alloc();
    if d.frame.is_null() || d.packet.is_null() {
        return Err(Error::Budget);
    }
    let ai = av_find_best_stream(d.fmt, 1, -1, -1, std::ptr::null_mut(), 0);
    let mut audio = None;
    let mut audio_generation = 0u64;
    let range = if (*cp).color_range == 2 {
        ColorRange::Full
    } else {
        ColorRange::Limited
    };
    let matrix = if (*cp).color_space == 1 || ((*cp).color_space == 2 && (*cp).height >= 720) {
        ColorMatrix::Bt709
    } else {
        ColorMatrix::Bt601
    };
    if has_finite_duration {
        c.duration_us.store(fmt_duration(d.fmt).max(0), Ordering::Release);
    }
    c.seekable.store(seekable, Ordering::Release);
    c.mailbox.lock().unwrap_or_else(|e| e.into_inner()).ready = true;
    let mut clock: Option<(i64, Instant)> = None;
    let mut last_sent = None;
    let mut flushing = false;
    let mut fallback = 0i64;
    let mut landing: Option<(u64, i64)> = None;
    let mut seek_audio_floor: Option<(u64, i64)> = None;
    loop {
        if c.stopped() {
            return Err(Error::Cancelled);
        }
        let iteration = c.take_iteration();
        let requested_seek = iteration.request;
        let timeline = iteration.timeline;
        let generation = iteration.audio_generation;
        let seek_generation = iteration.seek_generation;
        if landing.is_some_and(|(epoch, _)| epoch != seek_generation) {
            landing = None;
        }
        if requested_seek.is_none() && landing.is_none() && c.paused.load(Ordering::Acquire) {
            if !c.wait(Duration::from_millis(20)) {
                return Err(Error::Cancelled);
            }
            clock = None;
            continue;
        }
        if let Some((epoch, position)) = requested_seek {
            let target = av_rescale_q(
                position,
                AVRational {
                    num: 1,
                    den: 1_000_000,
                },
                stream_time_base(stream),
            );
            if av_seek_frame(d.fmt, vi, target, 1) < 0 {
                return Err(Error::Source);
            }
            avcodec_flush_buffers(d.codec);
            audio = None;
            audio_generation = 0;
            clock = None;
            last_sent = None;
            flushing = false;
            fallback = position;
            landing = Some((epoch, position));
            seek_audio_floor = Some((epoch, position));
        }
        if generation != audio_generation {
            audio = None;
            audio_generation = generation;
            if c.audio.load(Ordering::Acquire) && !c.paused.load(Ordering::Acquire) && ai >= 0 {
                let ast = *(*d.fmt).streams.add(ai as usize);
                audio = SpeedDecoder::open(stream_codecpar(ast), 1000);
            }
        }
        let read_result = if flushing {
            AVERROR_EOF
        } else {
            av_read_frame(d.fmt, d.packet)
        };
        if read_result >= 0
            && ((*d.packet).size < 0 || (*d.packet).size as usize > limits.max_object_bytes)
        {
            return Err(Error::Budget);
        }
        if read_result < 0 {
            if let Some(error) = io.error { return Err(error); }
            if read_result != AVERROR_EOF { return Err(Error::Source); }
        }
        if read_result == AVERROR_EOF {
            if flushing {
                return Ok(());
            }
            flushing = true;
        }
        if !flushing && (*d.packet).stream_index == ai {
            if let Some(decoder) = audio.as_mut() {
                let ast = *(*d.fmt).streams.add(ai as usize);
                let packet_pts = pts_ns_opt(d.packet, ast);
                for (pts, bytes) in decoder.decode(d.packet, packet_pts.unwrap_or(0)) {
                    if bytes.len() > 4096 {
                        return Err(Error::Budget);
                    }
                    let samples = bytes
                        .chunks_exact(2)
                        .map(|b| i16::from_le_bytes([b[0], b[1]]))
                        .collect();
                    if generation == c.audio_generation.load(Ordering::Acquire)
                        && timeline == c.timeline.load(Ordering::Acquire)
                        && seek_generation == c.seek_generation.load(Ordering::Acquire)
                        && c.audio.load(Ordering::Acquire)
                        && !c.paused.load(Ordering::Acquire)
                        // Decode preroll for codec continuity, but its PCM must
                        // not outrun the first exact video landing frame.
                        && landing.is_none()
                        && audio_reaches_seek_floor(
                            packet_pts.filter(|_| pts != AV_NOPTS_VALUE).map(|_| pts / 1000),
                            bytes.len(), seek_audio_floor, seek_generation,
                        )
                    {
                        let mut mailbox = c.mailbox.lock().unwrap_or_else(|e| e.into_inner());
                        // At most a quarter-second of stereo48k PCM; expired UI consumption
                        // drops oldest chunks rather than growing latency or memory forever.
                        while mailbox.audio.len() >= 12 {
                            mailbox.audio.pop_front();
                        }
                        mailbox.audio.push_back((
                            generation,
                            PcmChunk {
                                samples,
                                pts_us: pts / 1000,
                            },
                        ));
                    }
                }
            }
        }
        if flushing || (*d.packet).stream_index == vi {
            let sent =
                avcodec_send_packet(d.codec, if flushing { std::ptr::null() } else { d.packet });
            if sent < 0 && sent != AVERROR_EOF {
                return Err(Error::Decode);
            }
            loop {
                if c.stopped() {
                    return Err(Error::Cancelled);
                }
                // A newer seek owns the next landing; do not finish decoding
                // this packet under a retired target before servicing it.
                if c.seek.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
                    break;
                }
                let received = avcodec_receive_frame(d.codec, d.frame);
                if received == AVERROR_EAGAIN || received == AVERROR_EOF {
                    break;
                }
                if received < 0 {
                    return Err(Error::Decode);
                }
                if c.paused.load(Ordering::Acquire) && landing.is_none() {
                    av_frame_unref(d.frame);
                    continue;
                }
                let f = d.frame as *const u8;
                let format = std::ptr::read_unaligned(f.add(OFF_FRAME_FORMAT).cast::<i32>());
                if format != 0 && format != 12 {
                    return Err(Error::Unsupported);
                }
                let w = std::ptr::read_unaligned(f.add(OFF_FRAME_WIDTH).cast::<i32>());
                let h = std::ptr::read_unaligned(f.add(OFF_FRAME_HEIGHT).cast::<i32>());
                let (w, h, cw, ch) = dimensions(w, h, limits)?;
                let raw = std::ptr::read_unaligned(f.add(OFF_FRAME_PTS).cast::<i64>());
                let pts = if raw == AV_NOPTS_VALUE {
                    fallback
                } else {
                    av_rescale_q(
                        raw,
                        stream_time_base(stream),
                        AVRational {
                            num: 1,
                            den: 1_000_000,
                        },
                    )
                };
                fallback = pts.saturating_add(1_000_000 / i64::from(limits.max_fps));
                // Backward keyframe seeking may decode preroll. Only the first
                // frame at the requested position is a paused seek's landing.
                if landing.is_some_and(|(_, target)| pts < target) {
                    av_frame_unref(d.frame);
                    continue;
                }
                let (origin, started) = *clock.get_or_insert((pts, Instant::now()));
                let elapsed = pts.saturating_sub(origin);
                if elapsed < 0 || elapsed > 24 * 60 * 60 * 1_000_000 {
                    return Err(Error::Decode);
                }
                let due = started + Duration::from_micros(elapsed as u64);
                if !c.wait(due.saturating_duration_since(Instant::now())) {
                    return Err(Error::Cancelled);
                }
                if last_sent.is_none_or(|last: i64| {
                    pts.saturating_sub(last) >= 1_000_000 / i64::from(limits.max_fps)
                }) {
                    let frame = FrameYuv {
                        width: w as u32,
                        height: h as u32,
                        chroma_width: cw as u32,
                        chroma_height: ch as u32,
                        y: plane(f, 0, w, h)?,
                        u: plane(f, 1, cw, ch)?,
                        v: plane(f, 2, cw, ch)?,
                        pts_us: pts,
                        range: if format == 12 {
                            ColorRange::Full
                        } else {
                            range
                        },
                        matrix,
                    };
                    if timeline == c.timeline.load(Ordering::Acquire) {
                        c.mailbox.lock().unwrap_or_else(|e| e.into_inner()).video =
                            Some((timeline, frame));
                        if landing.is_some() {
                            landing = None;
                            if c.paused.load(Ordering::Acquire) {
                                // Drain any remaining frames of this packet without
                                // publishing them; no pending output may block resume.
                                clock = None;
                            }
                        }
                    }
                    last_sent = Some(pts);
                }
                av_frame_unref(d.frame);
            }
        }
        av_packet_unref(d.packet);
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitional_gateway_body_rechecks_only_the_existing_recently_playing_leaf_once() {
        use crate::curlio::cookie_tests::{curl, Reply, SessionServer};
        let _serial = curl();
        let live = b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\ns.ts\n";
        let master = b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=80\nleaf.m3u8\n";
        let opt_in = "OTT-Multiview-Admission: refresh-peers-v1\r\n";
        for (confirmation, expected) in [
            (Reply::ok("", MISSING_STREAM_HTML), Err(Error::LostProviderLease)),
            (Reply::ok("", TRANSITIONAL_HTML), Err(Error::Playlist)),
            (Reply::ok("", b"<h1>Forbidden (No client found)</h1>"), Err(Error::SourceDenied)),
            (Reply { status: 403, ..Reply::ok("", MISSING_STREAM_HTML) }, Err(Error::SourceDenied)),
            (Reply { status: 503, ..Reply::ok("", b"unavailable") }, Err(Error::Source)),
            (Reply::ok("", live), Ok(())),
        ] {
            let server = SessionServer::new(vec![
                Reply::ok(opt_in, master), Reply::ok("", live), Reply::ok("", live),
                Reply::ok("", TRANSITIONAL_HTML), confirmation, Reply::ok("", TRANSITIONAL_HTML),
            ]);
            let c = Control::default();
            let mut hls = Hls::open(format!("{}/channel.m3u8", server.base), 1024, &c).unwrap();
            c.video_decoded.store(true, Ordering::Release);
            hls.refresh(&c).unwrap();
            let before = Instant::now();
            let mut deadline = before + Duration::from_secs(15);
            let mut attempts = 2;
            assert_eq!(hls.refresh_retry(&c, &mut attempts, &mut deadline), expected);
            assert_eq!(attempts, 2, "a failed verification cannot make a third request");
            assert!(before.elapsed() >= Duration::from_millis(790));
            assert_eq!(server.heads().len(), 5);
            assert_eq!(hls.refresh(&c), Err(Error::Playlist));
            let heads = server.heads();
            assert_eq!(heads.len(), 6, "one verification at most per worker");
            assert!(heads[1..].iter().all(|r| r.starts_with("GET /leaf.m3u8 ")));
        }
    }

    #[test]
    fn expired_established_leaf_is_distinct_from_denial_and_does_not_reopen_the_master() {
        use crate::curlio::cookie_tests::{curl, Reply, SessionServer};
        let _serial = curl();
        let master = b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=80\nleaf.m3u8\n";
        let live = b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\ns.ts\n";
        let opt_in = "OTT-Multiview-Admission: refresh-peers-v1\r\n";
        let missing = b"<h1>No stream id found</h1>";
        let missing_viewer = b"<h1>Forbidden (No client found)</h1>";
        let missing_session = b"<h1>Forbidden (No client session found)</h1>";
        let server = SessionServer::new(vec![
            Reply::ok(opt_in, master), Reply::ok("", live),
            Reply::ok("", missing_viewer), Reply::ok("", missing),
            Reply::ok("", missing_session), Reply::ok("", missing_viewer), Reply::ok("", missing),
            Reply::ok("", b"<h1>Forbidden</h1>"),
            Reply { status: 403, ..Reply::ok("", missing_viewer) },
            Reply { status: 429, ..Reply::ok("", missing_viewer) },
            Reply { status: 401, ..Reply::ok("", missing_viewer) },
        ]);
        let c = Control::default();
        let mut hls = Hls::open(format!("{}/source.m3u8", server.base), 1024, &c).unwrap();
        assert_eq!(hls.refresh(&c), Err(Error::SourceDenied), "never-played lease");
        c.video_decoded.store(true, Ordering::Release);
        c.paused.store(true, Ordering::Release);
        assert_eq!(hls.refresh(&c), Err(Error::Source), "paused reader");
        c.paused.store(false, Ordering::Release);
        assert_eq!(hls.refresh(&c), Err(Error::LostProviderLease));
        assert_eq!(hls.read(&mut [0], &c), Err(Error::LostProviderLease));
        assert_eq!(hls.refresh(&c), Err(Error::LostProviderLease));
        assert_eq!(hls.refresh(&c), Err(Error::Playlist), "unspecified forbidden body");
        for _ in 0..3 { assert_eq!(hls.read(&mut [0], &c), Err(Error::SourceDenied)); }
        let heads = server.heads();
        assert_eq!(heads.len(), 11, "one request per case, no retry of refusal");
        assert_eq!(heads.iter().filter(|line| line.starts_with("GET /source.m3u8 ")).count(), 1);

        let server = SessionServer::new(vec![Reply::ok("", live), Reply::ok("", missing)]);
        let c = Control::default();
        let mut hls = Hls::open(format!("{}/live.m3u8", server.base), 1024, &c).unwrap();
        c.video_decoded.store(true, Ordering::Release);
        assert_eq!(hls.refresh(&c), Err(Error::Source), "unmarked source");
        let server = SessionServer::new(vec![Reply::ok(opt_in, missing_viewer)]);
        assert!(matches!(Hls::open(format!("{}/source.m3u8", server.base), 1024, &c), Err(Error::SourceDenied)));
    }

    #[test]
    fn handoff_reports_only_real_live_reads_started_after_retirement() {
        use crate::curlio::cookie_tests::{curl, Reply, SessionServer};
        use crate::local_multiview::hls_handoff_barrier;
        let _serial = curl();
        let master = b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=25\nleaf.m3u8\n";
        let live = b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\ns.ts\n";
        let mut ts = [0; 376];
        ts[0] = 0x47;
        ts[188] = 0x47;
        let server = SessionServer::new(vec![
            Reply::ok("OTT-Multiview-Admission: refresh-peers-v1\r\n", master),
            Reply::ok("", live), Reply::ok("", &ts), Reply::ok("", live),
            Reply::ok("", b"<h1>Forbidden (No client found)</h1>"),
            Reply::ok("", &[&live[..], b"#EXT-X-ENDLIST\n"].concat()),
        ]);
        let c = Control::default();
        let mut hls = Hls::open(format!("{}/source.m3u8?private=1", server.base), 1024, &c).unwrap();
        assert_eq!(c.hls_origin.get(), Some(&server.base));
        assert_eq!(c.hls_request.load(Ordering::Acquire), 0, "opening a session is not a refresh");
        let old = admission_read_started(&hls.url, &c);
        let fence = hls_handoff_barrier();
        finish_admission_read(old, &hls.url, &c);
        assert!(c.hls_request.load(Ordering::Acquire) <= fence, "in-flight GET began before retirement");
        assert_eq!(admission_read_started("https://other.example/leaf.m3u8", &c), None);
        let mut out = [0];
        assert_eq!(hls.read(&mut out, &c), Ok(1));
        let received = c.hls_request.load(Ordering::Acquire);
        assert!(received > fence);
        assert_eq!(hls.read(&mut out, &c), Ok(1));
        assert_eq!(c.hls_request.load(Ordering::Acquire), received, "cached bytes are not another GET");
        hls.refresh(&c).unwrap();
        let refreshed = c.hls_request.load(Ordering::Acquire);
        assert!(refreshed > received);
        assert_eq!(hls.refresh(&c), Err(Error::SourceDenied));
        assert_eq!(c.hls_request.load(Ordering::Acquire), refreshed);
        hls.refresh(&c).unwrap();
        assert!(!c.hls_live.load(Ordering::Acquire));
        assert_eq!(c.hls_request.load(Ordering::Acquire), refreshed);
    }

    #[test]
    fn handoff_requires_approved_response_on_original_origin_and_live_media() {
        use crate::curlio::cookie_tests::{curl, Reply, SessionServer};
        let _serial = curl();
        let live = b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\ns.ts\n";
        let ended = [&live[..], b"#EXT-X-ENDLIST\n"].concat();
        for (header, playlist) in [
            ("", &live[..]), ("OTT-Multiview-Admission: other\r\n", &live[..]),
            ("OTT-Multiview-Admission: refresh-peers-v1\r\n", &ended[..]),
        ] {
            let server = SessionServer::new(vec![Reply::ok(header, playlist)]);
            let c = Control::default();
            Hls::open(format!("{}/master.m3u8", server.base), 1024, &c).unwrap();
            assert!(c.hls_origin.get().is_none());
        }
        let redirect = Reply {
            status: 302,
            headers: "Location: /final.m3u8\r\nOTT-Multiview-Admission: refresh-peers-v1\r\n".into(),
            body: vec![], required: None,
        };
        let server = SessionServer::new(vec![redirect, Reply::ok("", live)]);
        let c = Control::default();
        Hls::open(format!("{}/redirect.m3u8", server.base), 1024, &c).unwrap();
        assert!(c.hls_origin.get().is_none(), "a redirect cannot opt in its destination");
        let master = b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=25\nvod.m3u8\n";
        let server = SessionServer::new(vec![
            Reply::ok("OTT-Multiview-Admission: refresh-peers-v1\r\n", master), Reply::ok("", &ended),
        ]);
        let c = Control::default();
        Hls::open(format!("{}/master.m3u8", server.base), 1024, &c).unwrap();
        assert!(!c.hls_live.load(Ordering::Acquire), "a finite leaf withdraws provisional master admission");
        use crate::local_multiview::hls_handoff_origin as origin;
        assert_eq!(origin("https://Example.org:443/private/abc?token=value"), Some("https://example.org:443".to_owned()));
        for url in ["https://user:password@example.org/path", "ftp://host/file", "https://example.org/a#x"] {
            assert_eq!(origin(url), None);
        }
        assert!(!media_segment_bytes(b"<h1>No stream id found</h1>"));
    }

    #[test]
    fn local_slot_keeps_its_private_cookie_through_segment_and_playlist_refresh() {
        use crate::curlio::cookie_tests::{curl, Reply, SessionServer};
        let _serial = curl();
        let server = SessionServer::new(vec![
            Reply::ok("Set-Cookie: mv=first; Path=/\r\n", b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=25\nmedia.m3u8\n"),
            Reply::ok("", b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:10\n#EXTINF:1,\na.ts\n").requiring("mv=first"),
            Reply::ok("Set-Cookie: mv=next; Path=/\r\n", b"A").requiring("mv=first"),
            Reply::ok("", b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:11\n#EXTINF:1,\nb.ts\n").requiring("mv=next"),
            Reply::ok("", b"B").requiring("mv=next"),
            Reply::ok("", b"second window"),
        ]);
        let control = Control::default();
        let url = format!("{}/start", server.base);
        let mut first = Remote::open(&url, false, &control).unwrap();
        assert!(first.sniff_hls(&control).unwrap());
        let mut hls = Hls::open_response(url.clone(), 1024, &control, Some(first)).unwrap();
        let mut byte = [0];
        assert_eq!(hls.read(&mut byte, &control).unwrap(), 1);
        assert_eq!(byte, [b'A']);
        assert_eq!(hls.read(&mut byte, &control).unwrap(), 1);
        assert_eq!(byte, [b'B']);
        let _independent = Remote::open(&url, false, &control).unwrap();
        let requests = server.heads();
        assert_eq!(requests.len(), 6);
        assert!(!requests[5].contains("Cookie:"));
    }
    #[test]
    fn seek_audio_floor_uses_the_actual_chunk_and_fails_closed_on_unknown_or_overflow() {
        let floor = Some((7, 4_300_000));
        assert!(!audio_reaches_seek_floor(Some(4_266_666), 4096, floor, 7));
        assert!(audio_reaches_seek_floor(Some(4_278_666), 4096, floor, 7));
        assert!(!audio_reaches_seek_floor(Some(4_278_665), 4096, floor, 7));
        assert!(audio_reaches_seek_floor(Some(4_288_000), 4096, floor, 7));
        assert!(audio_reaches_seek_floor(Some(4_300_000), 4, floor, 7));
        assert!(!audio_reaches_seek_floor(Some(4_300_000), 4, floor, 6));
        assert!(!audio_reaches_seek_floor(None, 4096, floor, 7));
        assert!(!audio_reaches_seek_floor(Some(i64::MAX), 4, floor, 7));
        assert!(!audio_reaches_seek_floor(Some(0), usize::MAX, floor, 7));
        assert!(!audio_reaches_seek_floor(Some(4_300_000), 2, floor, 7));
        assert!(audio_reaches_seek_floor(None, 4096, None, 7));
        assert!(audio_reaches_seek_floor(Some(0), 4096, None, 7));
    }
    #[test]
    fn planar_budget_includes_odd_chroma_edges() {
        let mut l = Limits::default();
        l.max_frame_bytes = 16;
        assert_eq!(dimensions(3, 3, l), Err(Error::Budget));
        l.max_frame_bytes = 17;
        assert_eq!(dimensions(3, 3, l), Ok((3, 3, 2, 2)));
        assert!(dimensions(-1, 100, l).is_err());
    }
    #[test]
    fn late_live_read_refreshes_after_a_pause_or_two_long_segments() {
        use crate::curlio::cookie_tests::{curl, Reply, SessionServer};
        let _serial = curl();
        let initial = b"#EXTM3U\n#EXT-X-TARGETDURATION:20\n#EXT-X-MEDIA-SEQUENCE:41\n#EXTINF:20,\na.ts\n#EXTINF:20,\nb.ts\n";
        let fresh = b"#EXTM3U\n#EXT-X-TARGETDURATION:20\n#EXT-X-MEDIA-SEQUENCE:43\n#EXTINF:20,\nc.ts\n";
        for paused in [false, true] {
            let server = SessionServer::new(vec![Reply::ok("", initial), Reply::ok("", b"aa"),
                Reply::ok("", b"b"), Reply::ok("", fresh), Reply::ok("", b"c")]);
            let control = Control::default();
            let mut hls = Hls::open(format!("{}/live.m3u8", server.base), 1024, &control).unwrap();
            let mut data = [0];
            assert_eq!(hls.read(&mut data, &control), Ok(1));
            if paused { control.paused.store(true, Ordering::Release); }
            // Position the last real response 35s into a pause, or two full
            // 20s segments in the past, without making the test sleep.
            hls.checked = Instant::now() - Duration::from_secs(if paused { 35 } else { 40 });
            control.paused.store(false, Ordering::Release);
            for expected in [b'a', b'b', b'c'] {
                assert_eq!(hls.read(&mut data, &control), Ok(1));
                assert_eq!(data[0], expected);
            }
            let heads = server.heads();
            assert_eq!(heads.len(), 5);
            assert!(heads[3].starts_with("GET /live.m3u8 "));
        }
    }

    #[test]
    fn late_live_read_paused_inside_avio_waits_for_resume_without_using_its_request_budget() {
        use crate::curlio::cookie_tests::{curl, Reply, SessionServer};
        let _serial = curl();
        let server = SessionServer::new(vec![
            Reply::ok("", b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:1,\na.ts\n"),
            Reply::ok("", b"a"),
            Reply::ok("", b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:2\n#EXTINF:1,\nb.ts\n"),
            Reply::ok("", b"b"),
        ]);
        let control = Control::default();
        let mut hls = Hls::open(format!("{}/live.m3u8", server.base), 1024, &control).unwrap();
        let mut byte = [0];
        assert_eq!(hls.read(&mut byte, &control), Ok(1));
        hls.checked = Instant::now() - Duration::from_secs(35);
        control.paused.store(true, Ordering::Release);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(600));
                // Release before asserting so a failed test cannot strand the reader.
                let visits = server.heads().len();
                control.paused.store(false, Ordering::Release);
                assert_eq!(visits, 2);
            });
            assert_eq!(hls.read_with_budget(&mut byte, &control, Duration::from_millis(400)), Ok(1));
        });
        assert_eq!(&byte, b"b");
    }

    #[test]
    fn late_live_read_bounds_an_unchanged_edge_and_stops_on_cancel() {
        use crate::curlio::cookie_tests::{curl, Reply, SessionServer};
        let _serial = curl();
        let edge = b"#EXTM3U\n#EXT-X-TARGETDURATION:20\n#EXT-X-MEDIA-SEQUENCE:99\n#EXTINF:20,\na.ts\n";
        let server = SessionServer::new(vec![Reply::ok("", edge), Reply::ok("", b"a"), Reply::ok("", edge)]);
        let control = Control::default();
        let mut hls = Hls::open(format!("{}/live.m3u8", server.base), 1024, &control).unwrap();
        let mut data = [0];
        assert_eq!(hls.read(&mut data, &control), Ok(1));
        hls.checked = Instant::now() - Duration::from_secs(35);
        assert_eq!(hls.read_with_budget(&mut data, &control, Duration::from_millis(300)), Err(Error::Source));
        assert_eq!(server.heads().len(), 3);
        control.paused.store(true, Ordering::Release);
        control.cancel.store(true, Ordering::Release);
        assert_eq!(hls.read(&mut data, &control), Err(Error::Cancelled));
        assert_eq!(server.heads().len(), 3);
    }

    #[test]
    fn live_poll_counts_time_since_the_previous_manifest_response() {
        let checked = Instant::now();
        let target = Duration::from_secs(2);
        assert_eq!(poll_wait(checked, target, checked), Duration::from_secs(1));
        assert_eq!(poll_wait(checked, target, checked + Duration::from_millis(800)), Duration::from_millis(200));
        assert!(poll_wait(checked, target, checked + target).is_zero());
        assert_eq!(poll_wait(checked, Duration::from_secs(10), checked), Duration::from_secs(1));
    }

    #[test]
    fn playlist_rejects_cipher_ranges_and_discontinuity() {
        let base = "https://example.test/p/list.m3u8";
        for tag in [
            "#EXT-X-KEY:METHOD=AES-128,URI=\"key\"",
            "#EXT-X-BYTERANGE:12@0",
            "#EXT-X-DISCONTINUITY",
        ] {
            let text = format!("#EXTM3U\n{tag}\n#EXTINF:2,\na.ts\n");
            assert!(matches!(
                parse_playlist(base, &text),
                Err(Error::Unsupported)
            ));
        }
    }
    #[test]
    fn playlist_keeps_sequence_map_and_same_origin() {
        let p=parse_playlist("https://example.test/p/list.m3u8","#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:30\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:2,\na.m4s\n#EXT-X-ENDLIST\n").unwrap();
        assert_eq!(p.segments[0].sequence, 30);
        assert_eq!(
            p.init.as_deref(),
            Some("https://example.test:443/p/init.mp4")
        );
        assert!(p.ended);
        assert!(parse_playlist(
            "https://example.test/p/list.m3u8",
            "#EXTM3U\n#EXTINF:2,\nhttps://other.test/a.ts\n"
        )
        .is_err());
    }
    #[test]
    fn plane_copy_respects_negative_stride_and_rejects_short_stride() {
        let pixels = [1u8, 2, 0, 0, 3, 4, 0, 0];
        let mut frame = vec![0u8; OFF_FRAME_LINESIZE + 32];
        unsafe {
            std::ptr::write_unaligned(
                frame.as_mut_ptr().cast::<*const u8>(),
                pixels.as_ptr().add(4),
            );
            std::ptr::write_unaligned(frame.as_mut_ptr().add(OFF_FRAME_LINESIZE).cast::<i32>(), -4);
            assert_eq!(plane(frame.as_ptr(), 0, 2, 2).unwrap(), vec![3, 4, 1, 2]);
            std::ptr::write_unaligned(frame.as_mut_ptr().add(OFF_FRAME_LINESIZE).cast::<i32>(), 1);
            assert_eq!(plane(frame.as_ptr(), 0, 2, 2), Err(Error::Decode));
        }
    }
    #[test]
    fn generic_hls_uses_scheme_ports_and_preserves_explicit_authority() {
        assert_eq!(
            child("https://example.test/p/list.m3u8", "a.ts").unwrap(),
            "https://example.test:443/p/a.ts"
        );
        assert_eq!(
            child(
                "http://example.test/p/list.m3u8",
                "http://example.test/a.ts"
            )
            .unwrap(),
            "http://example.test:80/a.ts"
        );
        assert_eq!(
            child("https://example.test:32400/p/list.m3u8", "a.ts").unwrap(),
            "https://example.test:32400/p/a.ts"
        );
        assert!(child(
            "https://example.test/p/list.m3u8",
            "https://example.test:32400/a.ts"
        )
        .is_err());
    }
    #[test]
    fn extensionless_hls_detection_is_content_based_and_does_not_reinterpret_mp4() {
        assert!(playlist_header(b"#EXTM3U\n#EXT-X-"));
        assert!(playlist_header(b"\xef\xbb\xbf#EXTM3U\n"));
        assert!(!playlist_header(b"\x00\x00\x00\x18ftypisom"));
        assert!(!playlist_header(b"\x47\x40\x11\x10"));
    }
    #[test]
    fn redirected_master_media_and_refresh_use_each_validated_response_base() {
        let master = Manifest {
            effective_url: "https://cdn.example.test/new/master.m3u8".into(),
            bytes: b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100\nlow/list.m3u8\n".to_vec(),
        };
        assert_eq!(
            master.variant().unwrap().as_deref(),
            Some("https://cdn.example.test:443/new/low/list.m3u8")
        );
        let media = Manifest {
            effective_url: "https://edge.example.test/session/list.m3u8".into(),
            bytes: b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:4\n#EXTINF:2,\na.ts\n".to_vec(),
        };
        assert_eq!(
            media.media().unwrap().segments[0].url,
            "https://edge.example.test:443/session/a.ts"
        );
        let refresh = Manifest {
            effective_url: "https://edge2.example.test/rotated/list.m3u8".into(),
            bytes: b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:5\n#EXTINF:2,\nb.ts\n".to_vec(),
        };
        assert_eq!(
            refresh.media().unwrap().segments[0].url,
            "https://edge2.example.test:443/rotated/b.ts"
        );
        assert!(parse_playlist(
            &media.effective_url,
            "#EXTM3U\n#EXTINF:2,\nhttps://old.example.test/a.ts\n"
        )
        .is_err());
    }
}
