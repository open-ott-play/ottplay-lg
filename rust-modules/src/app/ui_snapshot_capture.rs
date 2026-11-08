//! One authorized UI snapshot, without a listener, path, request parser or worker.
//!
//! The adapter owns the request ID and must supply current eligibility. It polls
//! even on iterations which do not present, invalidates idle once on admission,
//! and calls `capture_before_swap` only after the actual UI draw and before swap.
//! A successful read is the CURRENT framebuffer, not `capture::cap_cycle`'s
//! previous-cycle cache. Encoding and guarded publication belong to its worker.
//!
//! The eight-second deadline rejects late admission/completion. It cannot preempt
//! a synchronous GPU call; the existing readback watchdog remains responsible for
//! a hung driver. `source_ticks` is the supplied draw/readback-start tick, not UTC.

use image::ImageEncoder;
use std::io::{self, Write};

pub(super) const DEADLINE_MS: u32 = 8_000;
pub(super) const MAX_WIDTH: u32 = 1_920;
pub(super) const MAX_HEIGHT: u32 = 1_080;
pub(super) const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Failure {
    Timeout,
    VideoBound,
    SensitiveUi,
    UnsupportedUi,
    Background,
    InvalidViewport,
    ReadbackFailed,
    Cancelled,
    EncodingFailed,
    PayloadTooLarge,
}

/// No inference from the visible route: the adapter supplies the actual plane
/// binding and its password/token-editor privacy gate immediately before capture.
#[derive(Clone, Copy, Debug)]
pub(super) struct Eligibility {
    pub foreground: bool,
    pub video_plane_bound: bool,
    pub sensitive_ui: bool,
}

impl Eligibility {
    fn failure(self) -> Option<Failure> {
        if self.sensitive_ui {
            Some(Failure::SensitiveUi)
        } else if !self.foreground {
            Some(Failure::Background)
        } else if self.video_plane_bound {
            Some(Failure::VideoBound)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Viewport {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Viewport {
    fn rgb_len(self) -> Result<usize, Failure> {
        if self.x < 0
            || self.y < 0
            || self.width <= 0
            || self.height <= 0
            || self.width as u32 > MAX_WIDTH
            || self.height as u32 > MAX_HEIGHT
            || self.x.checked_add(self.width).is_none()
            || self.y.checked_add(self.height).is_none()
        {
            return Err(Failure::InvalidViewport);
        }
        (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|n| n.checked_mul(3))
            .ok_or(Failure::InvalidViewport)
    }
}

/// Sensitive image pixels are never included in Debug or log output.
pub(super) struct Frame {
    pub rgb: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub source_ticks: u32,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("source_ticks", &self.source_ticks)
            .field("rgb_bytes", &self.rgb.len())
            .finish()
    }
}

pub(super) type Completion = Result<Frame, Failure>;

/// Owned by one admitted request. Terminal outcomes consume it even on failure;
/// retries and duplicate request handling are the adapter's responsibility.
pub(super) struct Capture {
    admitted_ticks: u32,
    pending: bool,
}

impl Capture {
    pub fn new(admitted_ticks: u32) -> Self {
        Self { admitted_ticks, pending: true }
    }

    pub fn is_pending(&self) -> bool {
        self.pending
    }

    fn expired(&self, now: u32) -> bool {
        now.wrapping_sub(self.admitted_ticks) >= DEADLINE_MS
    }

    pub fn poll(&mut self, now: u32, eligibility: Eligibility) -> Option<Completion> {
        if !self.is_pending() {
            return None;
        }
        let failure = if self.expired(now) {
            Some(Failure::Timeout)
        } else {
            eligibility.failure()
        };
        failure.map(|reason| {
            let _ = self.cancel();
            Err(reason)
        })
    }

    pub fn cancel(&mut self) -> Option<Completion> {
        if !std::mem::replace(&mut self.pending, false) {
            return None;
        }
        Some(Err(Failure::Cancelled))
    }

    /// Render-thread runner only. `completion_ticks` must sample the same tick
    /// domain AFTER readback; reusing the input tick would hide a late GPU call.
    pub fn capture_before_swap(
        &mut self,
        now: u32,
        eligibility: Eligibility,
        viewport: Viewport,
        completion_ticks: impl FnOnce() -> u32,
    ) -> Option<Completion> {
        self.capture_with(now, eligibility, viewport, read_rgb, completion_ticks)
    }

    fn capture_with(
        &mut self,
        now: u32,
        eligibility: Eligibility,
        viewport: Viewport,
        read: impl FnOnce(Viewport) -> Result<Vec<u8>, Failure>,
        completion_ticks: impl FnOnce() -> u32,
    ) -> Option<Completion> {
        if let Some(outcome) = self.poll(now, eligibility) {
            return Some(outcome);
        }
        if !std::mem::replace(&mut self.pending, false) {
            return None;
        }
        let expected = match viewport.rgb_len() {
            Ok(n) => n,
            Err(reason) => return Some(Err(reason)),
        };
        let result = read(viewport);
        if self.expired(completion_ticks()) {
            return Some(Err(Failure::Timeout));
        }
        Some(result.and_then(|rgb| {
            if rgb.len() != expected {
                return Err(Failure::ReadbackFailed);
            }
            Ok(Frame {
                rgb,
                width: viewport.width as u32,
                height: viewport.height as u32,
                source_ticks: now,
            })
        }))
    }
}

fn zeroed(len: usize) -> Result<Vec<u8>, Failure> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| Failure::ReadbackFailed)?;
    bytes.resize(len, 0);
    Ok(bytes)
}

/// Same orientation/alpha policy as the existing hostsim `shot::read_flipped`:
/// RGB already holds the UI's composited colors; exporting its alpha as ordinary
/// straight alpha would misrepresent the transparent webOS UI plane.
fn opaque_rgb(viewport: Viewport, rgba: &[u8]) -> Result<Vec<u8>, Failure> {
    let rgb_len = viewport.rgb_len()?;
    let pixels = rgb_len / 3;
    if rgba.len() != pixels * 4 {
        return Err(Failure::ReadbackFailed);
    }
    let mut rgb = zeroed(rgb_len)?;
    let w = viewport.width as usize;
    let h = viewport.height as usize;
    for y in 0..h {
        for x in 0..w {
            let source = ((h - 1 - y) * w + x) * 4;
            let target = (y * w + x) * 3;
            rgb[target..target + 3].copy_from_slice(&rgba[source..source + 3]);
        }
    }
    Ok(rgb)
}

#[cfg(all(feature = "devtriggers", target_os = "linux", target_arch = "arm", not(feature = "hostsim")))]
fn read_rgb(viewport: Viewport) -> Result<Vec<u8>, Failure> {
    use std::ffi::c_void;
    extern "C" {
        fn glGetError() -> u32;
        fn glGetIntegerv(name: u32, value: *mut i32);
        fn glPixelStorei(name: u32, value: i32);
        fn glReadPixels(x: i32, y: i32, w: i32, h: i32, format: u32, ty: u32, pixels: *mut c_void);
    }
    const GL_VIEWPORT: u32 = 0x0ba2;
    const GL_FRAMEBUFFER_BINDING: u32 = 0x8ca6;
    const GL_PACK_ALIGNMENT: u32 = 0x0d05;
    const GL_RGBA: u32 = 0x1908;
    const GL_UNSIGNED_BYTE: u32 = 0x1401;
    let rgb_len = viewport.rgb_len()?;
    let mut rgba = zeroed(rgb_len / 3 * 4)?;
    unsafe {
        if glGetError() != 0 {
            return Err(Failure::ReadbackFailed);
        }
        let mut actual = [0i32; 4];
        let mut framebuffer = -1;
        let mut pack_alignment = 0;
        glGetIntegerv(GL_VIEWPORT, actual.as_mut_ptr());
        glGetIntegerv(GL_FRAMEBUFFER_BINDING, &mut framebuffer);
        glGetIntegerv(GL_PACK_ALIGNMENT, &mut pack_alignment);
        if glGetError() != 0 {
            return Err(Failure::ReadbackFailed);
        }
        if framebuffer != 0
            || actual != [viewport.x, viewport.y, viewport.width, viewport.height]
            || !matches!(pack_alignment, 1 | 2 | 4 | 8)
        {
            return Err(Failure::InvalidViewport);
        }
        glPixelStorei(GL_PACK_ALIGNMENT, 1);
        if glGetError() != 0 {
            glPixelStorei(GL_PACK_ALIGNMENT, pack_alignment);
            return Err(Failure::ReadbackFailed);
        }
        {
            #[cfg(feature = "threadcheck")]
            let _readback = plx_base::task::watchdog::readback_scope();
            glReadPixels(viewport.x, viewport.y, viewport.width, viewport.height,
                GL_RGBA, GL_UNSIGNED_BYTE, rgba.as_mut_ptr().cast());
        }
        let failed = glGetError() != 0;
        glPixelStorei(GL_PACK_ALIGNMENT, pack_alignment);
        let restore_failed = glGetError() != 0;
        if failed || restore_failed {
            return Err(Failure::ReadbackFailed);
        }
    }
    opaque_rgb(viewport, &rgba)
}

#[cfg(not(all(feature = "devtriggers", target_os = "linux", target_arch = "arm", not(feature = "hostsim"))))]
fn read_rgb(_viewport: Viewport) -> Result<Vec<u8>, Failure> {
    Err(Failure::UnsupportedUi)
}

struct PngOutput {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl Write for PngOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.bytes.len().checked_add(bytes.len()).is_none_or(|n| n > self.limit) {
            self.exceeded = true;
            return Err(io::Error::other("snapshot payload limit"));
        }
        self.bytes.try_reserve_exact(bytes.len()).map_err(|_| io::Error::other("snapshot allocation"))?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Worker-only encoding helper. No metadata, disk access, path or log is added.
pub(super) fn encode_png(frame: Frame) -> Result<Vec<u8>, Failure> {
    let viewport = Viewport { x: 0, y: 0, width: frame.width as i32, height: frame.height as i32 };
    if viewport.rgb_len()? != frame.rgb.len() {
        return Err(Failure::ReadbackFailed);
    }
    let mut output = PngOutput { bytes: Vec::new(), limit: MAX_PNG_BYTES, exceeded: false };
    let result = image::codecs::png::PngEncoder::new_with_quality(
        &mut output, image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    ).write_image(&frame.rgb, frame.width, frame.height, image::ExtendedColorType::Rgb8);
    if output.exceeded {
        return Err(Failure::PayloadTooLarge);
    }
    result.map_err(|_| Failure::EncodingFailed)?;
    Ok(output.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    const ALLOWED: Eligibility = Eligibility { foreground: true, video_plane_bound: false, sensitive_ui: false };
    const VIEW: Viewport = Viewport { x: 0, y: 0, width: 2, height: 2 };

    fn failure(outcome: Option<Completion>) -> Failure {
        outcome.expect("terminal outcome").expect_err("refused")
    }

    #[test]
    fn one_capture_consumes_request_and_preserves_current_source_tick() {
        let mut capture = Capture::new(10);
        assert!(capture.poll(11, ALLOWED).is_none());
        let frame = capture.capture_with(12, ALLOWED, VIEW, |_| Ok(vec![5; 12]), || 13).unwrap().unwrap();
        assert_eq!((frame.width, frame.height, frame.source_ticks), (2, 2, 12));
        assert_eq!(frame.rgb, vec![5; 12]);
        assert!(!capture.is_pending());
        assert!(capture.capture_with(14, ALLOWED, VIEW, |_| panic!("second read"), || panic!("second clock")).is_none());
    }

    #[test]
    fn expiry_without_a_presented_frame_is_terminal_and_wrap_safe() {
        let start = u32::MAX - 100;
        let mut capture = Capture::new(start);
        assert!(capture.poll(start.wrapping_add(DEADLINE_MS - 1), ALLOWED).is_none());
        assert_eq!(failure(capture.poll(start.wrapping_add(DEADLINE_MS), ALLOWED)), Failure::Timeout);
        assert!(capture.poll(start.wrapping_add(DEADLINE_MS + 1), ALLOWED).is_none());
    }

    #[test]
    fn privacy_background_and_video_refuse_before_readback() {
        for (eligibility, expected) in [
            (Eligibility { sensitive_ui: true, ..ALLOWED }, Failure::SensitiveUi),
            (Eligibility { foreground: false, ..ALLOWED }, Failure::Background),
            (Eligibility { video_plane_bound: true, ..ALLOWED }, Failure::VideoBound),
        ] {
            let mut capture = Capture::new(0);
            assert_eq!(failure(capture.capture_with(1, eligibility, VIEW, |_| panic!("forbidden read"), || panic!("clock"))), expected);
            assert!(!capture.is_pending());
        }
    }

    #[test]
    fn eligibility_is_rechecked_at_the_actual_draw() {
        let mut capture = Capture::new(0);
        assert!(capture.poll(1, ALLOWED).is_none());
        assert_eq!(failure(capture.capture_with(2, Eligibility { sensitive_ui: true, ..ALLOWED }, VIEW,
            |_| panic!("newly sensitive screen"), || 3)), Failure::SensitiveUi);
    }

    #[test]
    fn late_gpu_completion_is_discarded_without_retry() {
        let mut capture = Capture::new(0);
        let mut reads = 0;
        assert_eq!(failure(capture.capture_with(7_999, ALLOWED, VIEW, |_| { reads += 1; Ok(vec![0; 12]) }, || 8_000)), Failure::Timeout);
        assert_eq!(reads, 1);
        assert!(capture.cancel().is_none());
    }

    #[test]
    fn invalid_viewports_never_reach_the_reader() {
        for viewport in [
            Viewport { width: 0, ..VIEW }, Viewport { height: -1, ..VIEW },
            Viewport { x: -1, ..VIEW }, Viewport { y: i32::MAX, ..VIEW },
            Viewport { width: 1_921, ..VIEW }, Viewport { height: 1_081, ..VIEW },
        ] {
            assert_eq!(failure(Capture::new(0).capture_with(1, ALLOWED, viewport,
                |_| panic!("unbounded read"), || 2)), Failure::InvalidViewport);
        }
        assert_eq!(Viewport { x: 0, y: 0, width: 1_920, height: 1_080 }.rgb_len(), Ok(6_220_800));
    }

    #[test]
    fn failed_or_short_read_is_terminal() {
        for result in [Err(Failure::ReadbackFailed), Ok(vec![0; 11])] {
            let mut capture = Capture::new(0);
            assert_eq!(failure(capture.capture_with(1, ALLOWED, VIEW, |_| result, || 2)), Failure::ReadbackFailed);
            assert!(!capture.is_pending());
        }
    }

    #[test]
    fn cancel_is_terminal_and_never_reads() {
        let mut capture = Capture::new(0);
        assert_eq!(failure(capture.cancel()), Failure::Cancelled);
        assert!(capture.capture_with(1, ALLOWED, VIEW, |_| panic!("cancelled read"), || 2).is_none());
    }

    #[test]
    fn rgba_rows_flip_once_and_alpha_is_not_reinterpreted() {
        let rgba = [1,2,3,0, 4,5,6,1, 7,8,9,128, 10,11,12,255];
        assert_eq!(opaque_rgb(VIEW, &rgba).unwrap(), [7,8,9,10,11,12,1,2,3,4,5,6]);
        assert_eq!(opaque_rgb(VIEW, &rgba[..15]), Err(Failure::ReadbackFailed));
    }

    #[test]
    fn png_round_trip_preserves_rgb_geometry() {
        let pixels = vec![7,8,9,10,11,12,1,2,3,4,5,6];
        let encoded = encode_png(Frame { rgb: pixels.clone(), width: 2, height: 2, source_ticks: 2 }).unwrap();
        assert!(encoded.len() <= MAX_PNG_BYTES);
        let decoded = image::load_from_memory_with_format(&encoded, image::ImageFormat::Png).unwrap().to_rgb8();
        assert_eq!(decoded.dimensions(), (2, 2));
        assert_eq!(decoded.into_raw(), pixels);
    }

    #[test]
    fn output_limit_rejects_the_whole_excess_write_without_truncation() {
        let mut output = PngOutput { bytes: Vec::new(), limit: 4, exceeded: false };
        assert_eq!(output.write(&[1,2,3]).unwrap(), 3);
        assert!(output.write(&[4,5]).is_err());
        assert_eq!(output.bytes, [1,2,3]);
        assert!(output.exceeded);
    }

    #[test]
    fn invalid_frame_cannot_be_encoded() {
        assert_eq!(encode_png(Frame { rgb: vec![0; 11], width: 2, height: 2, source_ticks: 0 }), Err(Failure::ReadbackFailed));
        assert_eq!(encode_png(Frame { rgb: Vec::new(), width: u32::MAX, height: 2, source_ticks: 0 }), Err(Failure::InvalidViewport));
    }
    #[test]
    #[cfg(not(all(feature = "devtriggers", target_os = "linux", target_arch = "arm", not(feature = "hostsim"))))]
    fn actual_host_backend_refuses_once_without_returning_pixels() {
        let mut capture = Capture::new(10);
        assert_eq!(failure(capture.capture_before_swap(11, ALLOWED, VIEW, || 12)), Failure::UnsupportedUi);
        assert!(!capture.is_pending());
        assert!(capture.capture_before_swap(13, ALLOWED, VIEW,
            || panic!("a completed request must not sample the clock or backend again")).is_none());
    }

}
