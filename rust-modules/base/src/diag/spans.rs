//! **Named sub-spans of ONE frame** (results, update and draw phases), printed on that frame's `FRAMEDROP` line as
//! `spans=<name>:<ms>,…` — the attribution the eight phase stamps cannot give.
//!
//! `FRAMEDROP … draw=61.5` says the draw phase ate the frame and nothing about which part of it:
//! the page, the host snapshot's full-screen copy, the underlay field's readback, a surface's
//! first paint. `plxnative-cpuprof` measures every `ui::profile::phase` but aggregates over sixty
//! frames, so the one slow frame of a modal's open — the frame this instrument exists for — is
//! averaged into fifty-nine quick ones. A span here is per iteration, reset at every iteration
//! and printed only beside the frame it belongs to.
//!
//! **Wall time on the render thread, no `glFinish`.** A span that contains a pipeline stall — a
//! `glReadPixels`, a framebuffer-0 command that waits on the previous frame — reports that wait as
//! its own time, and that is the point: the stall IS the frame's cost, and a span is where it was
//! paid. It is not GPU time; `plxnative-hwcnt` is.
//!
//! Armed with the rest of the frame instrument (`plxnative-framedrop`); unarmed, [`span`] is one
//! relaxed atomic load and a direct call.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

static ON: AtomicBool = AtomicBool::new(false);

/// Closed vocabulary for numeric diagnostics. Other legacy labels remain text-only.
/// Spans are inclusive and may nest (for example `ott_form` contains `ott_table`).
/// Their durations must not be added together or subtracted as exclusive CPU costs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum SpanLabel {
    OttMerge, OttOrganization, OttView, OttForm, OttTable, OttTableLayout,
    Scenarios, PlaybackPump, LandResults, DispatcherFrame,
}

impl SpanLabel {
    pub const ALL: [Self; 10] = [Self::OttMerge, Self::OttOrganization, Self::OttView,
        Self::OttForm, Self::OttTable, Self::OttTableLayout, Self::Scenarios, Self::PlaybackPump,
        Self::LandResults, Self::DispatcherFrame];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OttMerge => "ott_merge", Self::OttOrganization => "ott_organization",
            Self::OttView => "ott_view", Self::OttForm => "ott_form", Self::OttTable => "ott_table",
            Self::OttTableLayout => "ott_table_layout",
            Self::Scenarios => "scen", Self::PlaybackPump => "pump", Self::LandResults => "land",
            Self::DispatcherFrame => "dframe",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|label| label.as_str() == name)
    }
}

/// One label in one iteration. `sum_us` sums calls; `max_call_us` is ONE call's
/// maximum. Integer microseconds truncate each duration, never reinterpret the sum
/// as a single call. Invalid/overflowing observations are counted, not exported as numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpanSample {
    pub calls: u32,
    pub sum_us: u64,
    pub max_call_us: u64,
    pub invalid_samples: u32,
}

impl SpanSample {
    const EMPTY: Self = Self { calls: 0, sum_us: 0, max_call_us: 0, invalid_samples: 0 };

    fn note(&mut self, ms: f64) {
        let valid = numeric_us(ms * 1000.0).and_then(|us| {
            Some((us, self.sum_us.checked_add(us)?, self.calls.checked_add(1)?))
        });
        if let Some((us, sum, calls)) = valid {
            self.calls = calls;
            self.sum_us = sum;
            self.max_call_us = self.max_call_us.max(us);
        } else {
            self.invalid_samples = self.invalid_samples.saturating_add(1);
        }
    }
}

/// Reject non-finite, negative and out-of-range measurements before integer conversion.
/// The upper comparison is strict because `u64::MAX as f64` rounds to 2^64.
pub(super) fn numeric_us(us: f64) -> Option<u64> {
    (us.is_finite() && us >= 0.0 && us < u64::MAX as f64).then_some(us as u64)
}

struct FrameSpans {
    text: Vec<(&'static str, u32, f64)>,
    numeric: [SpanSample; 10],
}

/// A single drain serves the legacy log and the typed reader. `text` must never be
/// used as the remote projection: only the closed `SpanLabel` numeric array is public data.
pub struct DrainedSpans {
    pub text: String,
    pub numeric: [SpanSample; 10],
}

thread_local! {
    /// This frame's spans in first-seen order, summed by name.
    static SPANS: RefCell<FrameSpans> = const { RefCell::new(FrameSpans {
        text: Vec::new(), numeric: [SpanSample::EMPTY; 10],
    }) };
}

/// Arm the instrument — `app::boot`, beside `Instruments::new`, when `plxnative-framedrop` is set.
pub fn arm() {
    ON.store(true, Relaxed);
}

#[inline]
pub fn armed() -> bool {
    ON.load(Relaxed)
}

/// Run `f`, and when armed add its wall time to this frame's `name`. Under `threadcheck` the
/// loop's own spans (`scen`, `pump`, `land`, `dframe`) also label the watchdog for the call.
#[inline]
pub fn span<R>(name: &'static str, f: impl FnOnce() -> R) -> R {
    // The main-loop spans also name the phase for the hang watchdog (label-only).
    #[cfg(feature = "threadcheck")]
    let _label = crate::task::watchdog::span_scope(name);
    if !armed() {
        return f();
    }
    let t0 = std::time::Instant::now();
    let r = f();
    note(name, t0.elapsed().as_secs_f64() * 1000.0);
    r
}

/// Add `ms` to this frame's `name` (the seam [`span`] and the tests share).
pub fn note(name: &'static str, ms: f64) {
    SPANS.with(|s| {
        let mut s = s.borrow_mut();
        match s.text.iter_mut().find(|e| e.0 == name) {
            Some(e) => {
                e.1 += 1;
                e.2 += ms;
            }
            None => s.text.push((name, 1, ms)),
        }
        if let Some(label) = SpanLabel::from_name(name) {
            s.numeric[label as usize].note(ms);
        }
    });
}

/// This frame's spans as the `FRAMEDROP` field (`""` when none ran), and a reset for the next
/// iteration. A compatibility wrapper for [`drain`], whether or not it crossed the threshold, so a span
/// never carries into a later frame's line — the same rule `FrameCounters` learned in phase 11.
pub fn take() -> String {
    drain().text
}

/// Drain once at the iteration tail, including iterations that skipped presentation.
/// Keeps legacy text order/format and releases both views together for the next iteration.
pub fn drain() -> DrainedSpans {
    SPANS.with(|s| {
        let mut s = s.borrow_mut();
        let text = format(&s.text);
        s.text.clear();
        let numeric = std::mem::take(&mut s.numeric);
        DrainedSpans { text, numeric }
    })
}

fn format(spans: &[(&'static str, u32, f64)]) -> String {
    if spans.is_empty() {
        return String::new();
    }
    let mut out = String::from("spans=");
    for (i, (name, n, ms)) in spans.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        if *n > 1 {
            out.push_str(&std::format!("{name}x{n}:{ms:.1}"));
        } else {
            out.push_str(&std::format!("{name}:{ms:.1}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_sum_by_name_in_first_seen_order_and_take_resets() {
        let _ = take();
        note("page", 3.0);
        note("field", 20.3);
        note("page", 1.0);
        assert_eq!(take(), "spans=pagex2:4.0,field:20.3");
        assert_eq!(take(), "", "taken: the next frame starts empty");
    }

    #[test]
    fn one_drain_keeps_text_and_numeric_in_step_without_exporting_unknown_labels() {
        let _ = drain();
        note("ott_view", 1.25);
        note("private-legacy-label", 7.0);
        note("ott_view", 2.5);
        let sample = drain();
        assert_eq!(sample.text, "spans=ott_viewx2:3.8,private-legacy-label:7.0");
        assert_eq!(sample.numeric[SpanLabel::OttView as usize], SpanSample {
            calls: 2, sum_us: 3750, max_call_us: 2500, invalid_samples: 0,
        });
        assert_eq!(sample.numeric.iter().filter(|s| s.calls != 0).count(), 1);
        let empty = drain();
        assert!(empty.text.is_empty());
        assert_eq!(empty.numeric, [SpanSample::default(); 10]);
        note("ott_merge", 1.0);
        assert_eq!(take(), "spans=ott_merge:1.0");
        assert_eq!(drain().numeric, [SpanSample::default(); 10], "legacy take drains both views");
    }

    #[test]
    fn approved_labels_keep_table_construction_and_layout_independent() {
        let _ = drain();
        for (index, label) in SpanLabel::ALL.into_iter().enumerate() {
            assert_eq!(label as usize, index);
            assert_eq!(SpanLabel::from_name(label.as_str()), Some(label));
        }
        note("ott_form", 8.0);
        note("ott_table", 5.0);
        note("ott_table_layout", 2.0);
        let sample = drain();
        assert_eq!(sample.numeric[SpanLabel::OttForm as usize].sum_us, 8000);
        assert_eq!(sample.numeric[SpanLabel::OttTable as usize].sum_us, 5000);
        assert_eq!(sample.numeric[SpanLabel::OttTableLayout as usize].sum_us, 2000);
        assert_eq!(sample.numeric.iter().filter(|s| s.calls != 0).count(), 3);
    }

    #[test]
    fn numeric_spans_reject_invalid_and_overflowing_samples() {
        let mut s = SpanSample::default();
        for ms in [f64::NAN, f64::INFINITY, -1.0, f64::MAX] { s.note(ms); }
        s.note(0.25);
        assert_eq!(s, SpanSample { calls: 1, sum_us: 250, max_call_us: 250, invalid_samples: 4 });
        s.sum_us = u64::MAX;
        s.note(1.0);
        assert_eq!(s.calls, 1);
        assert_eq!(s.invalid_samples, 5);
        assert_eq!(numeric_us(u64::MAX as f64), None);
        assert_eq!(numeric_us(0.0), Some(0));
    }
}
