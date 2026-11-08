//! **The simulator's picture** (`plxnative-simvideo`, with the clock sink armed): the video access
//! units the clock sink already accepts are ALSO piped to a system `ffmpeg` child process, decoded
//! there, and the frame whose presentation time the sink's clock has reached is composited UNDER
//! the finished UI frame — exactly the arithmetic the television's compositor applies to its
//! hardware video plane and our transparent UI plane.
//!
//! **Why it exists.** The documentation's player figure has to show a real picture under the real
//! HUD, and the simulator otherwise shows the HUD over black (`ffi_host.rs`: nothing decodes). It
//! is a screenshot facility, nothing more: every number the clock sink's module doc disowns stays
//! disowned, and nothing here says anything about LG's decoder.
//!
//! **Why a child process, and why only in the simulator.** The host FFmpeg this crate links
//! (`ci/build-ffmpeg.sh`) is built from the television's ONE component list, which has no video
//! decoders — the TV decodes in hardware — and growing that list for a Mac would make the two
//! builds diverge. A system `ffmpeg` binary (`PLXNATIVE_SIM_FFMPEG`, else `ffmpeg` on `PATH`) keeps
//! the decode entirely outside this process and this crate's link line. `hostsim`-only: the
//! television build does not contain this file.
//!
//! **What it costs, and the bound.** One 1920x1080 RGBA frame (8 MiB) crosses a pipe per decoded
//! picture and is uploaded once when it becomes current — about what a poster upload costs, once
//! per video frame, on a Mac. The reader holds at most ONE frame ahead of the clock (it blocks
//! until the clock reaches it), so a paused player holds a frozen picture and does no work, and
//! the render thread only ever sees the latest frame.
//!
//! **Frame ↔ PTS.** The decoder is fed raw Annex-B, which carries no timestamps, and emits frames
//! in presentation order. So the n-th frame out is the n-th smallest PTS fed since the last
//! reset: the pending PTS are kept in a min-heap and popped as frames emerge. A reorder window
//! cannot break that: a frame cannot be output before every frame that precedes it in display
//! order has been fed.

use std::collections::BinaryHeap;
use std::cmp::Reverse;
use std::io::{Read, Write};
use std::os::raw::c_uint;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering::{Acquire, Release, Relaxed}};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};

#[path = "sim_video_placement.rs"]
mod placement;
#[path = "sim_seek.rs"]
mod seek;
use seek::{Controller, FrameDecision, Status};
use placement::Placement;
use super::video_geometry::{Aspect, Viewport};

/// The decoded picture's size — the canvas. Letterboxed to it, whatever the stream's shape.
const W: usize = placement::WIDTH as usize;
const H: usize = placement::HEIGHT as usize;
const FRAME_BYTES: usize = W * H * 4;

/// Is the picture armed? Read once (a trigger is a boot-time fact).
fn armed() -> bool {
    static ONCE: OnceLock<bool> = OnceLock::new();
    *ONCE.get_or_init(|| {
        let on = plx_base::devtrig::flag("simvideo");
        if on {
            plx_base::eventlog::log(
                "simvideo: ARMED — video AUs are also decoded by a system ffmpeg and composited \
                 under the UI. A screenshot facility: nothing here measures the television.",
            );
        }
        on
    })
}

/// One decode session: a child, the thread feeding it, and the PTS its frames will carry.
struct Session {
    child: Arc<Mutex<Child>>,
    feed: Option<InputSender>,
}

/// The live session, if any. Replaced on every Load/flush, dropped (and the child killed) on stop.
static SESSION: Mutex<Option<Session>> = Mutex::new(None);
/// Bumped on every reset, so a reader thread from an older session stops publishing.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// One owner for generation, output floor, readiness and publication. Reset and the reader
/// acquire this before LATEST, so an obsolete decoder cannot win a check/publish race.
struct SeekState {
    control: Controller,
    started: Option<std::time::Instant>,
    input_ended: bool,
    clock_started: bool,
}
static SEEK: Mutex<SeekState> = Mutex::new(SeekState {
    control: Controller::new(), started: None, input_ended: false, clock_started: false,
});
const PREROLL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);
const DECODER_EXIT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);
/// Count bound, in addition to the upstream AU byte bound; a decoder which stops reading
/// must not accumulate the remainder of the movie while callbacks are fenced for preroll.
const INPUT_QUEUE_AUS: usize = 8;
#[cfg(test)]
static FORCE_EXACT: AtomicBool = AtomicBool::new(false);

pub(super) fn exact_seek_available() -> bool {
    #[cfg(test)]
    if FORCE_EXACT.load(Relaxed) { return true; }
    armed()
}

pub(super) fn preroll_status() -> plx_platform::tv::sink::SeekPreroll {
    use plx_platform::tv::sink::SeekPreroll as S;
    let mut state = SEEK.lock().unwrap_or_else(|e| e.into_inner());
    if state.control.status() == Status::Waiting
        && state.started.is_some_and(|at| at.elapsed() >= PREROLL_DEADLINE) {
        let generation = state.control.generation();
        state.control.finish_decode(generation, false);
    }
    match state.control.status() {
        Status::Inactive => S::Inactive, Status::Waiting => S::Waiting,
        Status::Ready { .. } => S::Ready, Status::Exhausted => S::Exhausted,
        Status::Failed => S::Failed,
    }
}

/// None holds Play; Some(None) keeps the existing clock; Some(Some(pts)) starts this exact
/// seek at its first ACTUALLY published picture. A later Pause/Play cannot re-anchor it.
pub(super) fn clock_start() -> Option<Option<i64>> {
    let _ = preroll_status();
    let mut state = SEEK.lock().unwrap_or_else(|e| e.into_inner());
    match state.control.status() {
        Status::Inactive => Some(None),
        Status::Ready { first_pts } => {
            let first = !state.clock_started;
            state.clock_started = true;
            Some(if first { Some(first_pts) } else { None })
        },
        _ => None,
    }
}

pub(super) fn seek(target_ns: i64) { reset_with_target(false, Some(target_ns)); }

pub(super) fn cancel_seek() {
    let active = SEEK.lock().unwrap_or_else(|e| e.into_inner()).control.target_ns().is_some();
    if active { reset_with_target(false, None); }
}

pub(super) fn end_input() {
    let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    SEEK.lock().unwrap_or_else(|e| e.into_inner()).input_ended = true;
    if let Some(session) = session.as_mut() { session.feed.take(); }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FeedResult { Accepted, Full, Failed }

enum InputSender {
    Ordinary(Sender<(Vec<u8>, i64)>),
    Exact(SyncSender<(Vec<u8>, i64)>),
}

fn submit(tx: &InputSender, au: &[u8], pts: i64) -> FeedResult {
    match tx {
        InputSender::Ordinary(tx) => if tx.send((au.to_vec(), pts)).is_ok() { FeedResult::Accepted } else { FeedResult::Failed },
        InputSender::Exact(tx) => match tx.try_send((au.to_vec(), pts)) {
            Ok(()) => FeedResult::Accepted,
            Err(TrySendError::Full(_)) => FeedResult::Full,
            Err(TrySendError::Disconnected(_)) => FeedResult::Failed,
        },
    }
}

fn write_packets(output: &mut impl Write, packets: Receiver<(Vec<u8>, i64)>, pending: &Mutex<BinaryHeap<Reverse<i64>>>) -> bool {
    for (au, pts) in packets {
        // A frame cannot emerge until its bytes and corresponding timestamp are visible.
        pending.lock().unwrap_or_else(|e| e.into_inner()).push(Reverse(pts));
        if output.write_all(&au).is_err() { return false; }
    }
    true
}

fn all_accepted_output_drained(writer_result: u8, pending: &Mutex<BinaryHeap<Reverse<i64>>>) -> bool {
    writer_result == 1 && pending.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
}

fn decoder_finished(generation: u64, clean_output: bool, success: bool) {
    let mut state = SEEK.lock().unwrap_or_else(|e| e.into_inner());
    let clean = state.input_ended && clean_output && success;
    state.control.finish_decode(generation, clean);
}

/// A zero-length frame read is clean EOF; EOF part-way through a frame is corruption.
fn read_frame(input: &mut impl Read, frame: &mut [u8]) -> std::io::Result<bool> {
    let mut filled = 0;
    while filled < frame.len() {
        match input.read(&mut frame[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

/// The gate and picture are committed together. `first_only` is used before the normal clock
/// wait; all other output passes through the same ownership check after waiting.
fn publish_frame(generation: u64, pts: i64, frame: &mut Vec<u8>, first_only: bool) -> FrameDecision {
    let mut state = SEEK.lock().unwrap_or_else(|e| e.into_inner());
    let decision = state.control.decide_frame(generation, pts);
    if decision == FrameDecision::PresentFirst || (!first_only && decision == FrameDecision::Schedule) {
        let seq = FRAME_SEQ.fetch_add(1, Relaxed);
        *LATEST.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((seq, Arc::new(std::mem::replace(frame, vec![0; FRAME_BYTES]))));
        state.control.mark_presented(generation, pts);
        plx_machine::idle::invalidate();
    }
    decision
}
/// The container's codec as the Load payload named it: `true` = HEVC, else H.264.
static HEVC: AtomicBool = AtomicBool::new(false);
/// The clock the frames are presented against — the sink's `Clock::position_ns`.
static CLOCK: OnceLock<fn() -> i64> = OnceLock::new();

/// Every published frame's number, process-wide, so the render thread's texture cache can never
/// mistake a new session's first frame for the last one's.
static FRAME_SEQ: AtomicU64 = AtomicU64::new(0);

/// The frame the render thread should show: `(frame number, rgba)`; `None` = nothing to show.
static LATEST: Mutex<Option<(u64, Arc<Vec<u8>>)>> = Mutex::new(None);

/// Exported-window geometry is independent of the decode session and frame identity.
/// Units are the UI's logical 1920×1080 canvas, not drawable or television pixels.
static PLACEMENT: Mutex<Option<Placement>> = Mutex::new(Some(Placement::FULL));

/// This renderer retains a picture. Current playback ownership is checked by
/// the caller; this is not fresh-source decode attestation. Clock-only playback
/// does not satisfy the query, which neither arms nor starts a decoder.
pub(super) fn has_picture() -> bool {
    LATEST.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// Current accepted image geometry, before canvas clipping, in the fixed
/// 1920x1080 logical coordinates used by `composite_under`.
pub(super) fn presentation_viewport() -> Option<Viewport> {
    let placement = (*PLACEMENT.lock().unwrap_or_else(|e| e.into_inner()))?;
    let aspect = Aspect::unpack(super::SHARED.video_aspect.load(std::sync::atomic::Ordering::Acquire));
    placement.picture(aspect)
}

/// A valid synthetic RGBA buffer for source/placement tests, not decode evidence.
/// Callers hold the existing serial test lock; no renderer or child is invoked.
#[cfg(any(test, feature = "test-support"))]
pub(super) fn picture_for_test() -> PictureForTest {
    plx_base::testlock::assert_held("sim-video picture fixture");
    let latest = LATEST.lock().unwrap_or_else(|e| e.into_inner())
        .replace((u64::MAX, Arc::new(vec![0; FRAME_BYTES])));
    let placement = *PLACEMENT.lock().unwrap_or_else(|e| e.into_inner());
    let aspect = super::SHARED.video_aspect.swap(0, Relaxed);
    set_placement(Some(Placement::FULL));
    PictureForTest { latest, placement, aspect, _thread: std::marker::PhantomData }
}

#[cfg(any(test, feature = "test-support"))]
/// Restores synthetic picture state while the caller retains the serial test lock.
pub struct PictureForTest {
    latest: Option<(u64, Arc<Vec<u8>>)>,
    placement: Option<Placement>,
    aspect: u64,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(any(test, feature = "test-support"))]
impl PictureForTest {
    /// Represent clock-only playback without leaking the absent state to another test.
    pub fn clear(&mut self) {
        plx_base::testlock::assert_held("sim-video picture fixture");
        *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for PictureForTest {
    fn drop(&mut self) {
        plx_base::testlock::assert_held("sim-video picture fixture");
        *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = self.latest.take();
        super::SHARED.video_aspect.store(self.aspect, Relaxed);
        set_placement(self.placement);
    }
}

fn set_placement(next: Option<Placement>) {
    let changed = {
        let mut current = PLACEMENT.lock().unwrap_or_else(|e| e.into_inner());
        let changed = *current != next;
        *current = next;
        changed
    };
    if changed {
        plx_machine::idle::invalidate();
    }
}

pub(super) fn create_window() {
    set_placement(Some(Placement::FULL));
}

pub(super) fn destroy_window() {
    set_placement(None);
}

/// Move the retained picture without touching the child, clock, or presentation generation.
/// A rejected rectangle leaves the last accepted placement intact.
pub(super) fn place_window(src_w: i32, src_h: i32, x: i32, y: i32, w: i32, h: i32) -> bool {
    let Some(next) = Placement::new((src_w, src_h), Viewport { x, y, width: w, height: h }) else {
        return false;
    };
    set_placement(Some(next));
    true
}


impl Drop for Session {
    fn drop(&mut self) {
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// A new stream: remember its codec and clock, and drop whatever the last one left. `payload` is
/// the Load JSON the television would have been given.
pub fn load(payload: &str, clock: fn() -> i64) {
    if !armed() {
        return;
    }
    let _ = CLOCK.set(clock);
    let lower = payload.to_ascii_lowercase();
    HEVC.store(lower.contains("h265") || lower.contains("hevc"), Relaxed);
    reset(true);
}

/// A flush (a seek restarts the fed timeline): keep the picture on screen until the next frame.
pub fn flush() {
    if exact_seek_available() { reset(false); }
}

/// Unload/destroy: no stream, no picture.
pub fn stop() {
    if exact_seek_available() { reset(true); }
}

fn reset(clear_picture: bool) { reset_with_target(clear_picture, None); }

fn reset_with_target(clear_picture: bool, target: Option<i64>) {
    let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    {
        let mut state = SEEK.lock().unwrap_or_else(|e| e.into_inner());
        let generation = GENERATION.fetch_add(1, Relaxed).wrapping_add(1);
        if let Some(target) = target { state.control.begin(generation, target); }
        else { state.control.reset(generation); }
        state.started = target.map(|_| std::time::Instant::now());
        state.input_ended = false;
        state.clock_started = false;
        if clear_picture {
            *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = None;
            plx_machine::idle::invalidate();
        }
    }
    *session = None;
}

/// One video access unit, Annex-B, at `pts` in the fed timeline. Never blocks the feeder.
pub(super) fn feed(au: &[u8], pts: i64) -> FeedResult {
    if !armed() { return FeedResult::Accepted; }
    let mut guard = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() { *guard = spawn(); }
    let Some(session) = guard.as_ref() else {
        decoder_finished(GENERATION.load(Relaxed), false, false);
        return feed_failure_policy(FeedResult::Failed);
    };
    let result = session.feed.as_ref().map_or(FeedResult::Failed, |tx| submit(tx, au, pts));
    if result == FeedResult::Failed {
        decoder_finished(GENERATION.load(Relaxed), false, false);
    }
    feed_failure_policy(result)
}

fn feed_failure_policy(result: FeedResult) -> FeedResult {
    if result == FeedResult::Failed && SEEK.lock().unwrap_or_else(|e| e.into_inner()).control.status() == Status::Inactive {
        // Ordinary screenshot decode remains best-effort; it never owned the native clock.
        FeedResult::Accepted
    } else { result }
}

fn ffmpeg_path() -> std::ffi::OsString {
    std::env::var_os("PLXNATIVE_SIM_FFMPEG").unwrap_or_else(|| "ffmpeg".into())
}

fn spawn() -> Option<Session> {
    let generation = GENERATION.load(Relaxed);
    let scale = format!(
        "scale={W}:{H}:force_original_aspect_ratio=decrease,format=rgba,pad={W}:{H}:(ow-iw)/2:(oh-ih)/2"
    );
    let spawned = Command::new(ffmpeg_path())
        .args(["-hide_banner", "-loglevel", "error", "-f"])
        .arg(if HEVC.load(Relaxed) { "hevc" } else { "h264" })
        .args(["-i", "pipe:0", "-an", "-vf"])
        .arg(scale)
        .args(["-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            plx_base::eventlog::log(&format!("simvideo: could not start ffmpeg ({e}); no picture this session"));
            return None;
        }
    };
    let (mut stdin, mut stdout) = (child.stdin.take()?, child.stdout.take()?);
    let (tx, rx) = if SEEK.lock().unwrap_or_else(|e| e.into_inner()).control.target_ns().is_some() {
        let (tx, rx) = sync_channel::<(Vec<u8>, i64)>(INPUT_QUEUE_AUS);
        (InputSender::Exact(tx), rx)
    } else {
        let (tx, rx) = channel::<(Vec<u8>, i64)>();
        (InputSender::Ordinary(tx), rx)
    };
    let child = Arc::new(Mutex::new(child));
    let pending = Arc::new(Mutex::new(BinaryHeap::new()));
    let pending_w = Arc::clone(&pending);
    // 0 pending, 1 accepted queue fully written, 2 write failed. Publication precedes closing
    // stdin, so clean child EOF cannot outrun the writer's completion proof.
    let writer_result = Arc::new(AtomicU8::new(0));
    let writer_result_w = Arc::clone(&writer_result);
    let writer = std::thread::Builder::new().name("simvideo-in".into()).spawn(move || {
        let complete = write_packets(&mut stdin, rx, &pending_w);
        writer_result_w.store(if complete { 1 } else { 2 }, Release);
        // Closing stdin after the accepted queue drains flushes FFmpeg's delayed pictures.
    });
    let pending_r = Arc::clone(&pending);
    let child_r = Arc::clone(&child);
    let reader = std::thread::Builder::new().name("simvideo-out".into()).spawn(move || {
        let mut buf = vec![0u8; FRAME_BYTES];
        let mut frames = 0u64;
        let clean_output = loop {
            match read_frame(&mut stdout, &mut buf) {
                Ok(true) => {}, Ok(false) => break true, Err(_) => break false,
            }
            let Some(Reverse(pts)) = pending_r.lock().unwrap_or_else(|e| e.into_inner()).pop() else {
                break false;
            };
            match publish_frame(generation, pts, &mut buf, true) {
                FrameDecision::Retired => return,
                FrameDecision::Discard => continue,
                FrameDecision::PresentFirst => {
                    frames += 1;
                    plx_base::eventlog::log("simvideo: first seek picture published");
                    continue;
                },
                FrameDecision::Schedule => {},
            }
            while CLOCK.get().is_some_and(|clock| clock() < pts) {
                if GENERATION.load(Relaxed) != generation { return; }
                std::thread::sleep(std::time::Duration::from_millis(4));
            }
            if publish_frame(generation, pts, &mut buf, false) == FrameDecision::Retired { return; }
            frames += 1;
            if frames == 1 { plx_base::eventlog::log("simvideo: first picture decoded"); }
        };
        if !clean_output {
            decoder_finished(generation, false, false);
            return;
        }
        // stdout EOF alone is not success. Poll without owning the Child mutex across a wait,
        // so cancellation can always kill/reap a decoder which closed stdout then stalled.
        let exit_started = std::time::Instant::now();
        let success = loop {
            if GENERATION.load(Relaxed) != generation { return; }
            if exit_started.elapsed() >= DECODER_EXIT_DEADLINE { break false; }
            let result = child_r.lock().unwrap_or_else(|e| e.into_inner()).try_wait();
            match result {
                Ok(Some(status)) => break status.success(),
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(4)),
                Err(_) => break false,
            }
        };
        let drained = all_accepted_output_drained(writer_result.load(Acquire), &pending_r);
        decoder_finished(generation, clean_output && drained, success);
    });
    if writer.is_err() || reader.is_err() {
        plx_base::eventlog::log("simvideo: could not spawn the pipe threads; no picture this session");
        let mut child = child.lock().unwrap_or_else(|e| e.into_inner());
        let _ = child.kill(); let _ = child.wait();
        return None;
    }
    Some(Session { child, feed: Some(tx) })
}

/// The render thread's half: upload the current frame if it changed and composite it UNDER what
/// the UI drew this frame. Called once per presented frame, before the capture and the swap.
pub fn composite_under() {
    thread_local! {
        /// `(texture, frame number it holds)`.
        static TEX: std::cell::Cell<(c_uint, Option<u64>)> = const { std::cell::Cell::new((0, None)) };
    }
    if !armed() {
        return;
    }
    let placement = *PLACEMENT.lock().unwrap_or_else(|e| e.into_inner());
    let Some(placement) = placement else { return };
    // Only sourceInfo's actual published display aspect may override the coded raster.
    let aspect = Aspect::unpack(super::SHARED.video_aspect.load(std::sync::atomic::Ordering::Acquire));
    let Some(draw) = placement.draw(aspect) else { return };
    let latest = LATEST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some((n, rgba)) = latest else { return };
    TEX.with(|t| {
        let (mut tex, held) = t.get();
        if held != Some(n) {
            tex = plx_gfx::gfx::upload_rgba(tex, W as i32, H as i32, rgba.as_ptr());
            t.set((tex, Some(n)));
        }
        let quad = |r: Viewport| [r.x as f32, r.y as f32, r.width as f32, r.height as f32];
        if draw.is_full_texture() {
            // Preserve the original full-canvas texture path, including its padded pixels.
            plx_gfx::gfx::draw_under(tex, 0.0, 0.0, W as f32, H as f32);
        } else {
            plx_gfx::gfx::draw_under_uv(tex, draw.crop, quad(draw.picture), Some(quad(draw.matte)));
        }
    });
}


#[cfg(test)]
mod placement_tests {
    use super::*;
    use crate::player::ffi_host::{HostSink, FORCE_ENABLED};
    use plx_base::task::MainThread;
    use plx_platform::tv::sink::VideoSink;

    struct Restore {
        enabled: bool,
        placement: Option<Placement>,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            FORCE_ENABLED.store(self.enabled, Relaxed);
            set_placement(self.placement);
        }
    }

    #[test]
    fn host_sink_places_and_hides_without_restarting_the_decode_session() {
        let _serial = plx_base::testlock::serial();
        let _restore = Restore {
            enabled: FORCE_ENABLED.swap(true, Relaxed),
            placement: *PLACEMENT.lock().unwrap_or_else(|e| e.into_inner()),
        };
        let generation = GENERATION.load(Relaxed);
        let frame_sequence = FRAME_SEQ.load(Relaxed);
        let child_id = SESSION.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.child.lock().unwrap_or_else(|e| e.into_inner()).id());
        let mt = unsafe { MainThread::assume() };
        let sink = HostSink;
        unsafe {
            assert!(!sink.create_window(&mt).is_null());
            assert_eq!(sink.place_window(&mt, 1440, 1080, 48, 120, 640, 360), 1);
        }
        let accepted = *PLACEMENT.lock().unwrap_or_else(|e| e.into_inner());
        let draw = accepted.unwrap().draw(None).unwrap();
        assert_eq!(draw.picture, Viewport { x: 128, y: 120, width: 480, height: 360 });
        assert_eq!(draw.matte, Viewport { x: 48, y: 120, width: 640, height: 360 });
        unsafe {
            assert_eq!(sink.place_window(&mt, 0, 1080, 0, 0, 640, 360), 0);
            assert_eq!(sink.place_window(&mt, 1440, 1080, 0, 0, -1, 360), 0);
        }
        assert_eq!(*PLACEMENT.lock().unwrap_or_else(|e| e.into_inner()), accepted);
        unsafe { sink.destroy_window(&mt); }
        assert_eq!(*PLACEMENT.lock().unwrap_or_else(|e| e.into_inner()), None);
        unsafe { assert!(!sink.create_window(&mt).is_null()); }
        assert_eq!(*PLACEMENT.lock().unwrap_or_else(|e| e.into_inner()), Some(Placement::FULL));
        // These are actual transport/presentation owners, not a mock decoder counter.
        assert_eq!(GENERATION.load(Relaxed), generation);
        assert_eq!(FRAME_SEQ.load(Relaxed), frame_sequence);
        assert_eq!(SESSION.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.child.lock().unwrap_or_else(|e| e.into_inner()).id()), child_id);
    }

    #[test]
    fn picture_queries_observe_placement_without_starting_or_restarting_decode() {
        let _serial = plx_base::testlock::serial();
        let mut picture = picture_for_test();
        let generation = GENERATION.load(Relaxed);
        let frame_sequence = FRAME_SEQ.load(Relaxed);
        let child_id = SESSION.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.child.lock().unwrap_or_else(|e| e.into_inner()).id());
        assert!(has_picture());
        assert!(place_window(1920, 1080, -192, -108, 2304, 1296));
        let expected = Viewport { x: -192, y: -108, width: 2304, height: 1296 };
        assert_eq!(presentation_viewport(), Some(expected));
        assert!(!place_window(0, 1080, 0, 0, 1920, 1080));
        assert_eq!(presentation_viewport(), Some(expected));
        picture.clear();
        assert!(!has_picture());
        // Geometry and picture availability are independent observations.
        assert_eq!(presentation_viewport(), Some(expected));
        set_placement(None);
        assert_eq!(presentation_viewport(), None);
        assert_eq!(GENERATION.load(Relaxed), generation);
        assert_eq!(FRAME_SEQ.load(Relaxed), frame_sequence);
        assert_eq!(SESSION.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.child.lock().unwrap_or_else(|e| e.into_inner()).id()), child_id);
    }

    #[test]
    fn presentation_query_uses_the_images_current_measured_aspect() {
        let _serial = plx_base::testlock::serial();
        let _picture = picture_for_test();
        assert!(place_window(720, 576, 0, 0, 1920, 1080));
        assert_eq!(presentation_viewport(), Some(Viewport {
            x: 285, y: 0, width: 1350, height: 1080,
        }));
        let aspect = super::super::video_geometry::source_aspect(
            br#"{"video":{"width":720,"height":576,"pixelAspectRatio":{"width":64,"height":45}}}"#,
        ).unwrap();
        super::super::SHARED.video_aspect.store(aspect.pack(), std::sync::atomic::Ordering::Release);
        let expected = Viewport { x: 0, y: 0, width: 1920, height: 1080 };
        assert_eq!(presentation_viewport(), Some(expected));
        let placement = (*PLACEMENT.lock().unwrap_or_else(|e| e.into_inner())).unwrap();
        assert_eq!(placement.draw(Some(aspect)).unwrap().picture, expected);
    }
}


/// A decoded-output fixture, never a process or a claim that FFmpeg decoded these pixels.
#[cfg(test)]
pub(super) struct ExactSeekProbe {
    previous: bool,
    picture: Option<(u64, Arc<Vec<u8>>)>,
}
#[cfg(test)]
pub(super) fn exact_seek_for_test() -> ExactSeekProbe {
    plx_base::testlock::assert_held("exact seek fixture");
    let picture = LATEST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let previous = FORCE_EXACT.swap(true, Relaxed);
    reset(false);
    ExactSeekProbe { previous, picture }
}
#[cfg(test)]
impl ExactSeekProbe {
    pub(super) fn generation(&self) -> u64 { GENERATION.load(Relaxed) }
    pub(super) fn frame(&self, generation: u64, pts: i64) -> bool {
        publish_frame(generation, pts, &mut vec![0; FRAME_BYTES], true) == FrameDecision::PresentFirst
    }
    pub(super) fn finish(&self, generation: u64, clean: bool) {
        end_input();
        decoder_finished(generation, clean, clean);
    }
}
#[cfg(test)]
impl Drop for ExactSeekProbe {
    fn drop(&mut self) {
        reset(false);
        FORCE_EXACT.store(self.previous, Relaxed);
        *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = self.picture.take();
    }
}

#[cfg(test)]
mod exact_seek_output_tests {
    use super::*;

    #[test]
    fn exact_seek_output_queue_reports_full_and_preserves_accepted_packet_order() {
        let (tx, rx) = sync_channel(1);
        let tx = InputSender::Exact(tx);
        assert_eq!(submit(&tx, &[1, 2], 20), FeedResult::Accepted);
        assert_eq!(submit(&tx, &[3], 21), FeedResult::Full);
        assert_eq!(rx.recv().unwrap(), (vec![1, 2], 20));
        assert_eq!(submit(&tx, &[3], 21), FeedResult::Accepted);
        drop(tx);
        assert_eq!(rx.recv().unwrap(), (vec![3], 21));
        assert!(rx.recv().is_err(), "EOF follows the final accepted packet");
    }

    #[test]
    fn exact_seek_output_disconnected_decoder_is_not_successful_acceptance() {
        let (tx, rx) = sync_channel(1);
        let tx = InputSender::Exact(tx);
        drop(rx);
        assert_eq!(submit(&tx, &[1], 20), FeedResult::Failed);
    }

    #[test]
    fn exact_seek_output_ordinary_decoder_failure_remains_best_effort() {
        let _serial = plx_base::testlock::serial();
        let _probe = exact_seek_for_test();
        assert_eq!(feed_failure_policy(FeedResult::Failed), FeedResult::Accepted);
        seek(30);
        assert_eq!(feed_failure_policy(FeedResult::Failed), FeedResult::Failed);
        cancel_seek();
        assert_eq!(feed_failure_policy(FeedResult::Failed), FeedResult::Accepted);
    }

    #[test]
    fn exact_seek_output_clean_stdout_without_input_eof_is_failure() {
        let _serial = plx_base::testlock::serial();
        let probe = exact_seek_for_test();
        seek(30);
        decoder_finished(probe.generation(), true, true);
        assert_eq!(preroll_status(), plx_platform::tv::sink::SeekPreroll::Failed);
    }

    #[test]
    fn exact_seek_output_successful_child_cannot_hide_unwritten_or_missing_frames() {
        let pending = Mutex::new(BinaryHeap::new());
        let (tx, rx) = channel();
        tx.send((vec![1], 20)).unwrap();
        tx.send((vec![2], 21)).unwrap();
        drop(tx);
        let mut output = Vec::new();
        assert!(write_packets(&mut output, rx, &pending));
        assert_eq!(output, vec![1, 2]);
        assert!(!all_accepted_output_drained(1, &pending), "successful writes are not decoded frames");
        assert_eq!(pending.lock().unwrap().pop(), Some(Reverse(20)));
        assert!(!all_accepted_output_drained(1, &pending), "one silent dropped frame is still a failure");
        assert_eq!(pending.lock().unwrap().pop(), Some(Reverse(21)));
        assert!(!all_accepted_output_drained(0, &pending), "a writer still draining is not proof");
        assert!(!all_accepted_output_drained(2, &pending), "a failed writer is not clean EOF");
        assert!(all_accepted_output_drained(1, &pending));
    }

    #[test]
    fn exact_seek_output_partial_rgba_is_not_clean_eof() {
        let mut frame = [0u8; 4];
        assert!(!read_frame(&mut &b""[..], &mut frame).unwrap());
        assert!(read_frame(&mut &b"abcd"[..], &mut frame).unwrap());
        assert_eq!(read_frame(&mut &b"abc"[..], &mut frame).unwrap_err().kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn exact_seek_output_retired_generation_cannot_replace_picture_or_readiness() {
        let _serial = plx_base::testlock::serial();
        let probe = exact_seek_for_test();
        seek(30);
        let first = probe.generation();
        assert!(!probe.frame(first, 20));
        seek(70);
        let sequence = FRAME_SEQ.load(Relaxed);
        assert!(!probe.frame(first, 31));
        decoder_finished(first, true, true);
        assert_eq!(preroll_status(), plx_platform::tv::sink::SeekPreroll::Waiting);
        assert_eq!(FRAME_SEQ.load(Relaxed), sequence);
        assert!(probe.frame(probe.generation(), 71));
        assert_eq!(clock_start(), Some(Some(71)));
        assert_eq!(clock_start(), Some(None), "resume cannot rewind to the first seek picture");
    }

    #[test]
    fn exact_seek_output_deadline_fails_without_publishing_a_picture() {
        let _serial = plx_base::testlock::serial();
        let probe = exact_seek_for_test();
        seek(30);
        SEEK.lock().unwrap_or_else(|e| e.into_inner()).started = Some(std::time::Instant::now() - PREROLL_DEADLINE);
        assert_eq!(preroll_status(), plx_platform::tv::sink::SeekPreroll::Failed);
        assert!(!probe.frame(probe.generation(), 31));
        assert_eq!(clock_start(), None);
    }
}
