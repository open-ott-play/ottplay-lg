//! Independent FFmpeg software reader. No SHARED, TX, native Engine or child process.
//! All FFmpeg pointers and IO cursors belong to this worker; only copied bounded planes escape.
use super::*;
use crate::local_multiview::{ColorMatrix, ColorRange, Control, Error, FrameYuv, Limits, PcmChunk};
use plx_base::checkpoint::{Checkpoint, Flow};
use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;
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
struct Remote {
    source: Box<crate::curlio::CurlSource>,
    _abort: Arc<crate::curlio::Abort>,
    position: i64,
    prefix: std::collections::VecDeque<u8>,
}
impl Remote {
    fn open(url: &str, owned: bool, c: &Control) -> Result<Self, Error> {
        let deadline = Some(Instant::now() + Duration::from_secs(15));
        let mut check = Check(c);
        let (source, abort) = if owned {
            crate::curlio::CurlSource::open_owned_private_checked(url, 0, deadline, &mut check)
        } else {
            crate::curlio::CurlSource::open_private_checked(url, 0, deadline, &mut check)
        }
        .map_err(|_| Error::Source)?;
        if !(200..300).contains(&source.status()) {
            return Err(Error::Source);
        }
        Ok(Self {
            source,
            _abort: abort,
            position: 0,
            prefix: Default::default(),
        })
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
            Some(Instant::now() + Duration::from_secs(15)),
            &mut Check(c),
        );
        if n < 0 {
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
fn fetch(url: &str, cap: usize, c: &Control) -> Result<Vec<u8>, Error> {
    collect(Remote::open(url, false, c)?, cap, c)
}
fn collect(mut r: Remote, cap: usize, c: &Control) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let mut scratch = [0; 16 * 1024];
    let expires = Instant::now() + Duration::from_secs(15);
    loop {
        if Instant::now() >= expires {
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
    fn from_remote(remote: Remote, c: &Control) -> Result<Self, Error> {
        let effective_url = remote.source.effective_url().to_owned();
        resource(&effective_url)?;
        Ok(Self {
            effective_url,
            bytes: collect(remote, 512 * 1024, c)?,
        })
    }
    fn fetch(url: &str, c: &Control) -> Result<Self, Error> {
        Self::from_remote(Remote::open(url, false, c)?, c)
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
struct Hls {
    url: String,
    playlist: Playlist,
    next: u64,
    bytes: Vec<u8>,
    pos: usize,
    init_sent: bool,
    refreshed: Instant,
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
        let first = match first {
            Some(remote) => Manifest::from_remote(remote, c)?,
            None => Manifest::fetch(&url, c)?,
        };
        let manifest = if let Some(child) = first.variant()? {
            Manifest::fetch(&child, c)?
        } else {
            first
        };
        url = manifest.effective_url.clone();
        let playlist = manifest.media()?;
        let start = if playlist.ended {
            0
        } else {
            playlist.segments.len().saturating_sub(2)
        };
        let next = playlist.segments[start].sequence;
        Ok(Self {
            url,
            playlist,
            next,
            bytes: Vec::new(),
            pos: 0,
            init_sent: false,
            refreshed: Instant::now(),
            cap,
        })
    }
    fn read(&mut self, out: &mut [u8], c: &Control) -> Result<usize, Error> {
        while self.pos == self.bytes.len() {
            if c.stopped() {
                return Err(Error::Cancelled);
            }
            if !self.init_sent {
                self.init_sent = true;
                if let Some(url) = &self.playlist.init {
                    self.bytes = fetch(url, self.cap, c)?;
                    self.pos = 0;
                    if !self.bytes.is_empty() {
                        break;
                    }
                }
            }
            if let Some(segment) = self
                .playlist
                .segments
                .iter()
                .find(|s| s.sequence == self.next)
            {
                self.bytes = fetch(&segment.url, self.cap, c)?;
                self.pos = 0;
                self.next = self.next.checked_add(1).ok_or(Error::Playlist)?;
                if !self.bytes.is_empty() {
                    break;
                }
                return Err(Error::Source);
            }
            if self.playlist.ended {
                return Ok(0);
            }
            if self
                .playlist
                .segments
                .first()
                .is_some_and(|s| s.sequence > self.next)
            {
                return Err(Error::Playlist);
            }
            if !c.wait(self.playlist.target.min(Duration::from_secs(2)) / 2) {
                return Err(Error::Cancelled);
            }
            let manifest = Manifest::fetch(&self.url, c)?;
            let fresh = manifest.media()?;
            if fresh.init != self.playlist.init {
                return Err(Error::Unsupported);
            }
            if fresh.segments.last().map(|s| s.sequence)
                > self.playlist.segments.last().map(|s| s.sequence)
            {
                self.refreshed = Instant::now();
            }
            if self.refreshed.elapsed() > Duration::from_secs(30) {
                return Err(Error::Source);
            }
            self.url = manifest.effective_url;
            self.playlist = fresh;
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
    input: Input,
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
    let input = match source {
        ExternalSource::HeldLocal(file) => {
            Input::Local(file_source::FileSource::held(&file).map_err(|_| Error::Source)?)
        }
        ExternalSource::Http(url) => {
            let mut remote = Remote::open(&url, false, c)?;
            if remote.sniff_hls(c)? {
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
    let seekable = !matches!(&input, Input::Hls(_));
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
    let bounded = av_dict_set(&mut opts, c"probesize".as_ptr(), c"1048576".as_ptr(), 0) >= 0
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
    c.seekable.store(seekable, Ordering::Release);
    c.mailbox.lock().unwrap_or_else(|e| e.into_inner()).ready = true;
    let mut clock: Option<(i64, Instant)> = None;
    let mut last_sent = None;
    let mut flushing = false;
    let mut fallback = 0i64;
    loop {
        if c.stopped() {
            return Err(Error::Cancelled);
        }
        while c.paused.load(Ordering::Acquire) {
            if !c.wait(Duration::from_millis(20)) {
                return Err(Error::Cancelled);
            }
            clock = None;
        }
        let requested_seek = c.seek.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(position) = requested_seek {
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
        }
        let timeline = c.timeline.load(Ordering::Acquire);
        let generation = c.audio_generation.load(Ordering::Acquire);
        if generation != audio_generation {
            audio = None;
            audio_generation = generation;
            if c.audio.load(Ordering::Acquire) && ai >= 0 {
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
        if read_result < 0 && read_result != AVERROR_EOF {
            return Err(io.error.unwrap_or(Error::Source));
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
                for (pts, bytes) in decoder.decode(d.packet, pts_ns(d.packet, ast)) {
                    if bytes.len() > 4096 {
                        return Err(Error::Budget);
                    }
                    let samples = bytes
                        .chunks_exact(2)
                        .map(|b| i16::from_le_bytes([b[0], b[1]]))
                        .collect();
                    if generation == c.audio_generation.load(Ordering::Acquire)
                        && c.audio.load(Ordering::Acquire)
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
                let received = avcodec_receive_frame(d.codec, d.frame);
                if received == AVERROR_EAGAIN || received == AVERROR_EOF {
                    break;
                }
                if received < 0 {
                    return Err(Error::Decode);
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
    fn planar_budget_includes_odd_chroma_edges() {
        let mut l = Limits::default();
        l.max_frame_bytes = 16;
        assert_eq!(dimensions(3, 3, l), Err(Error::Budget));
        l.max_frame_bytes = 17;
        assert_eq!(dimensions(3, 3, l), Ok((3, 3, 2, 2)));
        assert!(dimensions(-1, 100, l).is_err());
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
