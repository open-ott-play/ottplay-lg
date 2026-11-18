//! In-process software pictures. Each Session owns one worker and no native player globals.
//! A grid is presentation geometry, never evidence that nine decoders fit a particular TV.
use crate::ff::ExternalSource;
use std::sync::{
    atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    InvalidLimits,
    Unsupported,
    Source,
    SourceDenied,
    LostProviderLease,
    SourceInterrupted,
    SourceTemporarilyUnavailable,
    Playlist,
    Decode,
    Budget,
    Cancelled,
    Thread,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "local video: {self:?}")
    }
}
impl std::error::Error for Error {}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorRange {
    Limited,
    Full,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMatrix {
    Bt601,
    Bt709,
}
#[derive(Debug)]
pub struct FrameYuv {
    pub width: u32,
    pub height: u32,
    /// Display width / height, including non-square pixels from the stream or decoded frame.
    pub display_aspect: f32,
    pub chroma_width: u32,
    pub chroma_height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    pub pts_us: i64,
    pub range: ColorRange,
    pub matrix: ColorMatrix,
}
#[derive(Debug)]
pub struct PcmChunk {
    pub samples: Vec<i16>,
    pub pts_us: i64,
}
#[derive(Debug)]
pub enum Event {
    Ready,
    Video(FrameYuv),
    Audio(PcmChunk),
    Ended,
    Failure(Error),
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_source_pixels: u32,
    pub max_frame_bytes: usize,
    pub max_fps: u32,
    pub max_object_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_source_pixels: 1920 * 1080,
            max_frame_bytes: 1920 * 1080 * 3 / 2 + 4096,
            max_fps: 15,
            max_object_bytes: 16 * 1024 * 1024,
        }
    }
}
impl Limits {
    pub(crate) fn valid(self) -> bool {
        self.max_source_pixels > 0
            && self.max_source_pixels <= 3840 * 2160
            && self.max_frame_bytes >= 3
            && self.max_frame_bytes <= 64 * 1024 * 1024
            && (1..=60).contains(&self.max_fps)
            && (64 * 1024..=32 * 1024 * 1024).contains(&self.max_object_bytes)
    }
}
#[derive(Default)]
pub(crate) struct Mailbox {
    pub ready: bool,
    pub video: Option<(u64, FrameYuv)>,
    pub audio: std::collections::VecDeque<(u64, PcmChunk)>,
    pub terminal: Option<Result<(), Error>>,
}
/// A completed request counts after a retirement only if it also *started* afterwards.
static HLS_REQUEST_ORDER: AtomicU64 = AtomicU64::new(0);

/// Sample after the outgoing reader has fully retired. A sibling's receipt must exceed this value.
pub fn hls_handoff_barrier() -> u64 {
    HLS_REQUEST_ORDER.load(Ordering::SeqCst)
}

pub(crate) fn hls_request_started() -> u64 {
    HLS_REQUEST_ORDER.fetch_add(1, Ordering::SeqCst) + 1
}

/// Canonical, credential-free origin shared by the scheduler and HLS transport.
pub fn hls_handoff_origin(url: &str) -> Option<String> {
    if url.contains('#') { return None; }
    let url = plx_data::ott::http_url(url)?;
    let (origin, _) = plx_net::net::origin::split(&url);
    Some(origin.base().to_ascii_lowercase())
}

#[cfg_attr(test, derive(Default))]
pub(crate) struct Control {
    pub cancel: AtomicBool,
    held: Option<crate::ff::HeldLocalFile>,
    pub retired: AtomicBool,
    pub paused: AtomicBool,
    pub audio: AtomicBool,
    pub audio_generation: AtomicU64,
    pub timeline: AtomicU64,
    /// Seek identity survives pause-only publication/audio invalidations.
    pub seek_generation: AtomicU64,
    pub seekable: AtomicBool,
    pub hls_origin: OnceLock<String>,
    pub hls_live: AtomicBool,
    pub video_decoded: AtomicBool,
    pub hls_request: AtomicU64,
    pub duration_us: AtomicI64,
    pub seek: Mutex<Option<(u64, i64)>>,
    pub mailbox: Mutex<Mailbox>,
}
/// One decoder iteration's publication authority. A newer seek invalidates all
/// three epochs; no old packet may borrow a later request's audio generation.
pub(crate) struct Iteration {
    pub request: Option<(u64, i64)>,
    pub timeline: u64,
    pub audio_generation: u64,
    pub seek_generation: u64,
}
impl Control {
    pub(crate) fn take_iteration(&self) -> Iteration {
        let mut request = self.seek.lock().unwrap_or_else(|e| e.into_inner());
        Iteration {
            request: request.take(),
            timeline: self.timeline.load(Ordering::Acquire),
            audio_generation: self.audio_generation.load(Ordering::Acquire),
            seek_generation: self.seek_generation.load(Ordering::Acquire),
        }
    }
    pub(crate) fn stopped(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
            || self.held.as_ref().is_some_and(|file| !file.is_live())
    }
    pub(crate) fn wait(&self, d: Duration) -> bool {
        let until = std::time::Instant::now() + d;
        while !self.stopped() && std::time::Instant::now() < until {
            std::thread::sleep(
                Duration::from_millis(5)
                    .min(until.saturating_duration_since(std::time::Instant::now())),
            );
        }
        !self.stopped()
    }
}
/// Decoder registry availability, not hardware slot capacity and not a codec/profile grant.
pub fn available() -> bool {
    crate::ff::local_video::available()
}
// Resource ownership only: no playback clocks, route identity or decoded bytes are global.
static WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct WorkerPermit;
impl WorkerPermit {
    fn acquire() -> Result<Self, Error> {
        WORKERS
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < 9).then_some(n + 1)
            })
            .map(|_| Self)
            .map_err(|_| Error::Budget)
    }
}
impl Drop for WorkerPermit {
    fn drop(&mut self) {
        WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}
pub struct Session {
    control: Arc<Control>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Session {
    pub fn open(source: ExternalSource, limits: Limits) -> Result<Self, Error> {
        if !limits.valid() {
            return Err(Error::InvalidLimits);
        }
        // Application Local must carry a held authorization receipt, never reopen a display path.
        if matches!(&source, ExternalSource::Local(_)) {
            return Err(Error::Unsupported);
        }
        if !available() {
            return Err(Error::Unavailable);
        }
        let held = match &source {
            ExternalSource::HeldLocal(file) => Some(file.clone()),
            _ => None,
        };
        let control = Arc::new(Control {
            cancel: AtomicBool::new(false),
            held,
            retired: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            audio: AtomicBool::new(false),
            audio_generation: AtomicU64::new(1),
            timeline: AtomicU64::new(1),
            seek_generation: AtomicU64::new(1),
            seekable: AtomicBool::new(false),
            hls_origin: OnceLock::new(),
            hls_live: AtomicBool::new(false),
            video_decoded: AtomicBool::new(false),
            hls_request: AtomicU64::new(0),
            duration_us: AtomicI64::new(0),
            seek: Mutex::new(None),
            mailbox: Mutex::new(Mailbox::default()),
        });
        let permit = WorkerPermit::acquire()?;
        let worker = Arc::clone(&control);
        let handle = plx_base::task::spawn_off_frame_keeping("local-picture", move |off| {
            let _permit = permit;
            struct Retire(Arc<Control>);
            impl Drop for Retire {
                fn drop(&mut self) {
                    self.0.retired.store(true, Ordering::Release);
                }
            }
            let _retire = Retire(Arc::clone(&worker));
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::ff::local_video::run(off, source, limits, &worker)
            }))
            .unwrap_or(Err(Error::Decode));
            worker.seekable.store(false, Ordering::Release);
            let mut m = worker.mailbox.lock().unwrap_or_else(|e| e.into_inner());
            if !worker.cancel.load(Ordering::Acquire) {
                m.terminal = Some(result);
            }
        })
        .ok_or(Error::Thread)?;
        Ok(Self {
            control,
            worker: Some(handle),
        })
    }
    pub fn poll(&mut self) -> Option<Event> {
        let mut m = self
            .control
            .mailbox
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.control.cancel.load(Ordering::Acquire) {
            return None;
        }
        if self
            .control
            .held
            .as_ref()
            .is_some_and(|file| !file.is_live())
        {
            m.ready = false;
            m.video = None;
            m.audio.clear();
            return m.terminal.take().map(|_| Event::Failure(Error::Cancelled));
        }
        if m.ready {
            m.ready = false;
            return Some(Event::Ready);
        }
        if let Some((generation, f)) = m.video.take() {
            if generation == self.control.timeline.load(Ordering::Acquire) {
                self.control.video_decoded.store(true, Ordering::Release);
                return Some(Event::Video(f));
            }
        }
        if let Some((generation, a)) = m.audio.pop_front() {
            if self.control.audio.load(Ordering::Acquire)
                && generation == self.control.audio_generation.load(Ordering::Acquire)
            {
                return Some(Event::Audio(a));
            }
        }
        m.terminal.take().map(|r| match r {
            Ok(()) => Event::Ended,
            Err(e) => Event::Failure(e),
        })
    }
    /// Native demux duration for this exact input. A live or unknown timeline
    /// has no duration; catalogue metadata remains a separate caller fallback.
    pub fn duration_us(&self) -> Option<i64> {
        let duration = self.control.duration_us.load(Ordering::Acquire);
        (duration > 0).then_some(duration)
    }

    pub fn set_audio(&mut self, on: bool) {
        if self.control.audio.swap(on, Ordering::AcqRel) == on {
            return;
        }
        self.control.audio_generation.fetch_add(1, Ordering::AcqRel);
        self.control
            .mailbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .audio
            .clear();
    }
    pub fn set_paused(&mut self, on: bool) {
        if self.control.paused.swap(on, Ordering::AcqRel) == on {
            return;
        }
        self.control.timeline.fetch_add(1, Ordering::AcqRel);
        self.control.audio_generation.fetch_add(1, Ordering::AcqRel);
        if on {
            self.control
                .mailbox
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .audio
                .clear();
        }
    }
    pub fn seek(&mut self, position_us: i64) -> Result<(), Error> {
        if !(0..=24 * 60 * 60 * 1_000_000).contains(&position_us) {
            return Err(Error::Budget);
        }
        if !self.is_seekable() {
            return Err(Error::Unsupported);
        }
        // Serialize the request and its epoch with the worker's iteration snapshot.
        // A pre-seek packet must never acquire the new seek's publication epoch.
        let mut request = self.control.seek.lock().unwrap_or_else(|e| e.into_inner());
        self.control.timeline.fetch_add(1, Ordering::AcqRel);
        let seek_generation = self.control.seek_generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.control.audio_generation.fetch_add(1, Ordering::AcqRel);
        *request = Some((seek_generation, position_us));
        let mut m = self
            .control
            .mailbox
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        m.video = None;
        m.audio.clear();
        Ok(())
    }
    pub fn cancel(&mut self) {
        self.control.cancel.store(true, Ordering::Release);
        let mut m = self
            .control
            .mailbox
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        m.video = None;
        m.audio.clear();
    }
    /// Available only after this reader opened a seekable transport. HLS,
    /// unopened, withdrawn and retired readers never advertise seeking.
    pub fn is_seekable(&self) -> bool {
        !self.control.stopped()
            && !self.control.retired.load(Ordering::Acquire)
            && self.control.seekable.load(Ordering::Acquire)
    }
    /// An opted-in live HLS gateway and the latest completed leaf/segment GET's start order.
    /// Origin discovery alone has a zero receipt. No path, query or playback ID escapes the worker.
    pub fn hls_handoff(&self) -> Option<(String, u64)> {
        if !self.control.video_decoded.load(Ordering::Acquire) { return None; }
        self.hls_handoff_lease()
    }

    /// The gateway can register a confirmed master before its leaf or first picture arrives.
    /// Keep that fact available for retirement while the decoder is still starting.
    pub fn hls_handoff_lease(&self) -> Option<(String, u64)> {
        if !self.control.hls_live.load(Ordering::Acquire) { return None; }
        let origin = self.control.hls_origin.get()?;
        Some((origin.clone(), self.control.hls_request.load(Ordering::Acquire)))
    }

    pub fn is_retired(&self) -> bool {
        self.control.retired.load(Ordering::Acquire)
            && self.worker.as_ref().is_none_or(|w| w.is_finished())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.cancel();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn starting_live_gateway_has_a_retirement_lease_before_its_first_video() {
        let session = manual_session();
        assert!(session.hls_handoff_lease().is_none());
        session.control.hls_origin.set("http://gateway.invalid:8080".into()).unwrap();
        session.control.hls_live.store(true, Ordering::Release);
        assert_eq!(session.hls_handoff_lease(), Some(("http://gateway.invalid:8080".into(), 0)));
        assert!(session.hls_handoff().is_none());
        session.control.video_decoded.store(true, Ordering::Release);
        assert_eq!(session.hls_handoff(), session.hls_handoff_lease());
        session.control.hls_live.store(false, Ordering::Release);
        assert!(session.hls_handoff_lease().is_none());
        assert!(session.hls_handoff().is_none());
    }

    #[test]
    fn budgets_are_closed_and_bounded() {
        assert!(Limits::default().valid());
        let mut l = Limits::default();
        l.max_fps = 0;
        assert!(!l.valid());
        l = Limits::default();
        l.max_object_bytes = usize::MAX;
        assert!(!l.valid());
    }
    #[test]
    fn path_only_source_is_not_a_held_file_grant() {
        let source = ExternalSource::Local("/private/video.mp4".into());
        assert!(matches!(
            Session::open(source, Limits::default()),
            Err(Error::Unsupported)
        ));
    }
    fn manual_session() -> Session {
        Session {
            control: Arc::new(Control {
                cancel: AtomicBool::new(false),
                held: None,
                retired: AtomicBool::new(false),
                paused: AtomicBool::new(false),
                audio: AtomicBool::new(false),
                audio_generation: AtomicU64::new(1),
                timeline: AtomicU64::new(1),
                seek_generation: AtomicU64::new(1),
                seekable: AtomicBool::new(true),
                hls_origin: OnceLock::new(),
                hls_live: AtomicBool::new(false),
                video_decoded: AtomicBool::new(false),
                hls_request: AtomicU64::new(0),
                duration_us: AtomicI64::new(0),
                seek: Mutex::new(None),
                mailbox: Mutex::new(Mailbox::default()),
            }),
            worker: None,
        }
    }
    #[test]
    fn duration_is_private_to_the_open_decoder() {
        let first = manual_session();
        let second = manual_session();
        assert_eq!(first.duration_us(), None);
        first.control.duration_us.store(90_000_000, Ordering::Release);
        assert_eq!(first.duration_us(), Some(90_000_000));
        assert_eq!(second.duration_us(), None);
    }

    #[test]
    fn repeated_audio_owner_preserves_pcm_but_owner_change_withdraws_it() {
        let mut s = manual_session();
        s.set_audio(true);
        let epoch = s.control.audio_generation.load(Ordering::Acquire);
        s.control.mailbox.lock().unwrap().audio.push_back((
            epoch,
            PcmChunk {
                samples: vec![1, 2],
                pts_us: 123,
            },
        ));
        s.set_audio(true);
        assert_eq!(s.control.audio_generation.load(Ordering::Acquire), epoch);
        assert!(matches!(
            s.poll(),
            Some(Event::Audio(PcmChunk { pts_us: 123, .. }))
        ));
        s.control.mailbox.lock().unwrap().audio.push_back((
            epoch,
            PcmChunk {
                samples: vec![1, 2],
                pts_us: 124,
            },
        ));
        s.set_audio(false);
        assert!(s.poll().is_none());
    }
    #[test]
    fn seekability_requires_an_open_live_reader_and_disappears_on_cancel_or_retirement() {
        let mut s = manual_session();
        s.control.seekable.store(false, Ordering::Release);
        assert!(!s.is_seekable());
        assert_eq!(s.seek(0), Err(Error::Unsupported));
        s.control.seekable.store(true, Ordering::Release);
        assert!(s.is_seekable());
        s.control.retired.store(true, Ordering::Release);
        assert!(!s.is_seekable());
        s.control.retired.store(false, Ordering::Release);
        s.cancel();
        assert!(!s.is_seekable());
        assert_eq!(s.seek(0), Err(Error::Unsupported));
    }
    #[test]
    fn pause_only_epochs_cannot_retire_a_pending_seek() {
        let mut session = manual_session();
        session.set_paused(true);
        session.seek(4_300_000).unwrap();
        let control = Arc::clone(&session.control);
        let request = control.seek.lock().unwrap();
        let pending = *request;
        let generation = control.seek_generation.load(Ordering::Acquire);
        let timeline = control.timeline.load(Ordering::Acquire);
        session.set_paused(false);
        session.set_paused(true);
        assert_eq!(*request, pending);
        assert_eq!(control.seek_generation.load(Ordering::Acquire), generation);
        assert!(control.timeline.load(Ordering::Acquire) > timeline);
        drop(request);
        session.seek(2_300_000).unwrap();
        assert!(control.seek_generation.load(Ordering::Acquire) > generation);
    }
    #[test]
    fn old_iteration_cannot_borrow_a_new_seeks_publication_or_audio_epoch() {
        let mut session = manual_session();
        let old = session.control.take_iteration();
        assert!(old.request.is_none());
        session.seek(4_300_000).unwrap();
        assert_ne!(
            old.timeline,
            session.control.timeline.load(Ordering::Acquire)
        );
        assert_ne!(
            old.audio_generation,
            session.control.audio_generation.load(Ordering::Acquire)
        );
        assert_ne!(
            old.seek_generation,
            session.control.seek_generation.load(Ordering::Acquire)
        );
        let new = session.control.take_iteration();
        assert_eq!(new.request, Some((new.seek_generation, 4_300_000)));
        assert_eq!(
            new.timeline,
            session.control.timeline.load(Ordering::Acquire)
        );
        assert_eq!(
            new.audio_generation,
            session.control.audio_generation.load(Ordering::Acquire)
        );
    }
    #[test]
    fn seek_withdraws_old_audio_and_cancel_is_not_retirement() {
        let mut s = manual_session();
        let old = s.control.timeline.load(Ordering::Acquire);
        s.seek(2_000_000).unwrap();
        assert_eq!(*s.control.seek.lock().unwrap(), Some((old + 1, 2_000_000)));
        assert!(s.control.timeline.load(Ordering::Acquire) > old);
        s.cancel();
        assert!(!s.is_retired());
        assert!(s.poll().is_none());
        s.control.retired.store(true, Ordering::Release);
        assert!(s.is_retired());
    }
    #[test]
    fn held_withdrawal_stops_cached_decode_without_hiding_failure_event() {
        let mut s = manual_session();
        let path =
            std::env::temp_dir().join(format!("ott-local-cached-permit-{}", std::process::id()));
        std::fs::write(&path, b"cached input bytes").unwrap();
        let withdrawn = Arc::new(AtomicBool::new(false));
        let held = crate::ff::HeldLocalFile::from_authorized(
            &plx_base::task::OffFrame::for_test(),
            std::fs::File::open(&path).unwrap(),
            path.clone(),
            withdrawn.clone(),
        )
        .unwrap();
        Arc::get_mut(&mut s.control).unwrap().held = Some(held);
        assert!(!s.control.stopped());
        withdrawn.store(true, Ordering::Release);
        assert!(
            s.control.stopped(),
            "cached decoder must consult the held grant, not just future reads"
        );
        s.control.mailbox.lock().unwrap().terminal = Some(Err(Error::Cancelled));
        assert!(
            matches!(s.poll(), Some(Event::Failure(Error::Cancelled))),
            "withdrawal is an observable failure, not an explicit UI cancel"
        );
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "local_multiview_real_tests.rs"]
mod real_qualification;
