//! Player-owned output volume and its native actuator.
//!
//! `machine::Player` owns [`VolumeState`]; the adapter and each short-lived Engine do not
//! own a second copy of the viewer's choice. A Load, a seek and a temporary preroll mute
//! therefore cannot replace that choice with the pipeline's default volume.
//!
//! The native getter is deliberately not used: its ABI differs between firmware versions.
//! A setter return is dispatch evidence, not a measurement of the output or TV volume.

use super::adapter::PlayerAdapter;
use super::engine::Engine;
use super::shared::Stage;
use plx_base::task::MainThread;

/// The viewer's chosen level and mute, independently of temporary playback holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DesiredVolume {
    level: u8,
    muted: bool,
}

impl DesiredVolume {
    pub const DEFAULT: Self = Self {
        level: 100,
        muted: false,
    };

    /// Reject out-of-range requests rather than silently turning them into another request.
    pub const fn new(level: i32, muted: bool) -> Option<Self> {
        if level < 0 || level > 100 {
            None
        } else {
            Some(Self {
                level: level as u8,
                muted,
            })
        }
    }

    pub const fn level(self) -> u8 {
        self.level
    }
    pub const fn muted(self) -> bool {
        self.muted
    }
    pub const fn with_muted(self, muted: bool) -> Self {
        Self { muted, ..self }
    }

    /// Mute never destroys the level to restore when the viewer unmutes.
    pub const fn output(self) -> u8 {
        if self.muted {
            0
        } else {
            self.level
        }
    }
}

impl Default for DesiredVolume {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A setter call that reached a particular native Load. It is not hardware readback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VolumeSubmission {
    pub native_epoch: u32,
    pub output: u8,
    pub temporary_silence: bool,
    /// Raw setter result, retained as evidence without calling it measured volume.
    pub native_result: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VolumeReadback {
    pub desired: DesiredVolume,
    pub last_submission: Option<VolumeSubmission>,
    /// No supported native getter exists. Never substitute the desired level here.
    pub measured_output: Option<u8>,
}

/// Pure state with the same lifetime as `machine::Player`, not the current Engine.
#[derive(Debug)]
pub struct VolumeState {
    desired: DesiredVolume,
    last_submission: Option<VolumeSubmission>,
}

impl VolumeState {
    pub const fn new() -> Self {
        Self {
            desired: DesiredVolume::DEFAULT,
            last_submission: None,
        }
    }

    pub const fn desired(&self) -> DesiredVolume {
        self.desired
    }

    /// Store a validated preference even when no pipeline is available to apply it yet.
    /// This changes no playback, lifecycle or mute-preroll state.
    pub fn set_desired(&mut self, desired: DesiredVolume) {
        self.desired = desired;
    }

    /// A stop/reload retires native evidence, but keeps the viewer's chosen level and mute.
    pub fn forget_pipeline(&mut self) {
        self.last_submission = None;
    }

    pub const fn readback(&self) -> VolumeReadback {
        VolumeReadback {
            desired: self.desired,
            last_submission: self.last_submission,
            measured_output: None,
        }
    }
}

impl Default for VolumeState {
    fn default() -> Self {
        Self::new()
    }
}

/// Set the viewer's choice and try the current loaded pipeline.
///
/// `false` means deferred (no eligible pipeline) or a zero setter result; the desired
/// choice is still retained for the next Load. `true` means the setter returned nonzero
/// for this epoch/output, never that a getter confirmed the sound level. The caller must
/// separately enforce foreground/standby policy. This function never starts a decoder.
pub fn set_volume(pa: &mut PlayerAdapter, state: &mut VolumeState, desired: DesiredVolume) -> bool {
    state.set_desired(desired);
    // This is the existing preroll operation's restore snapshot, not another owner
    // of the setting. Its writers and its restoration all run on the main thread.
    if super::TX.take_preroll_volume().is_some() {
        super::TX.note_preroll_muted(i32::from(desired.output()));
    }
    sync_volume(pa, state)
}

/// Apply the latest choice while respecting the existing transport's preroll ownership.
pub fn sync_volume(pa: &mut PlayerAdapter, state: &mut VolumeState) -> bool {
    sync_output(
        pa,
        state,
        super::TX.seek_preroll_active() || super::TX.preroll_muted(),
    )
}

/// Apply a caller-owned temporary silence decision without rewriting the viewer's choice.
/// The existing preroll closer calls this with `false` after retiring its mute, including
/// the Resume path that is about to close `seek_preroll_active` in the same transaction.
pub fn sync_output(pa: &mut PlayerAdapter, state: &mut VolumeState, silent: bool) -> bool {
    let (engine, mt) = pa.split();
    match engine {
        Some(engine) => apply_to_engine(engine, mt, state, silent),
        None => {
            state.forget_pipeline();
            false
        }
    }
}

/// The pump's split-borrow variant. Use after the Load-completed transition, before its
/// first Play; use again before preroll Play with `silent = true`. The native epoch fence
/// rejects a retired Engine and a Load still running on its worker.
pub(crate) fn apply_to_engine(
    engine: &Engine,
    mt: &MainThread,
    state: &mut VolumeState,
    silent: bool,
) -> bool {
    let ready = native_ready(engine);
    apply_if_ready(state, engine.native_epoch, ready, silent, |output| {
        // SAFETY: epoch-scoped Load completion was checked above; the main-thread token
        // is borrowed from the owner of this Engine. `sink` also retains the Unload fence.
        unsafe { super::sink().set_volume(mt, i32::from(output)) }
    })
}

fn native_ready(engine: &Engine) -> bool {
    engine.stage != Stage::Loading
        && !engine.preview_abandon
        && super::SHARED.native_load_returned(engine.native_epoch)
}

/// Complete the existing preroll operation at its native transport boundary. Its
/// snapshot is refreshed by `set_volume` while it is outstanding. No getter, new
/// setting owner or decoder start is introduced in this path.
pub(crate) fn restore_preroll(pa: &mut PlayerAdapter) {
    let (engine, mt) = pa.split();
    if !engine.is_some_and(|engine| native_ready(engine)) {
        return;
    }
    let Some(output) = super::TX.take_preroll_volume() else {
        return;
    };
    // SAFETY: same epoch-scoped Load and main-thread conditions as apply_to_engine.
    unsafe { super::sink().set_volume(mt, output) };
}

fn apply_if_ready(
    state: &mut VolumeState,
    native_epoch: u32,
    ready: bool,
    silent: bool,
    setter: impl FnOnce(u8) -> i32,
) -> bool {
    if !ready || native_epoch == 0 {
        state.forget_pipeline();
        return false;
    }
    let output = if silent { 0 } else { state.desired.output() };
    if state.last_submission.is_some_and(|last| {
        last.native_epoch == native_epoch
            && last.output == output
            && last.temporary_silence == silent
            && last.native_result != 0
    }) {
        return true;
    }
    let native_result = setter(output);
    state.last_submission = Some(VolumeSubmission {
        native_epoch,
        output,
        temporary_silence: silent,
        native_result,
    });
    native_result != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desired(level: i32, muted: bool) -> DesiredVolume {
        DesiredVolume::new(level, muted).unwrap()
    }

    #[test]
    fn range_and_user_mute_preserve_the_chosen_level() {
        assert!(DesiredVolume::new(-1, false).is_none());
        assert!(DesiredVolume::new(101, false).is_none());
        assert_eq!(desired(0, false).output(), 0);
        assert_eq!(desired(100, false).output(), 100);
        let volume = desired(37, false).with_muted(true);
        assert_eq!(volume.output(), 0);
        assert_eq!(volume.level(), 37);
        assert_eq!(volume.with_muted(false).output(), 37);
    }

    #[test]
    fn pending_or_retired_load_never_reaches_the_setter() {
        let mut state = VolumeState::new();
        state.set_desired(desired(28, false));
        for (epoch, ready) in [(0, true), (7, false)] {
            assert!(!apply_if_ready(
                &mut state,
                epoch,
                ready,
                false,
                |_| panic!("unsafe setter")
            ));
        }
        assert_eq!(state.desired().level(), 28);
        assert_eq!(state.readback().last_submission, None);
    }

    #[test]
    fn preroll_restores_the_latest_choice_not_its_entry_value() {
        let mut state = VolumeState::new();
        let mut issued = Vec::new();
        state.set_desired(desired(43, false));
        assert!(apply_if_ready(&mut state, 7, true, false, |v| {
            issued.push(v);
            1
        }));
        assert!(apply_if_ready(&mut state, 7, true, true, |v| {
            issued.push(v);
            1
        }));
        state.set_desired(desired(19, false));
        assert!(apply_if_ready(&mut state, 7, true, true, |_| panic!(
            "already silent"
        )));
        assert!(apply_if_ready(&mut state, 7, true, false, |v| {
            issued.push(v);
            1
        }));
        assert_eq!(issued, [43, 0, 19]);
    }

    #[test]
    fn mute_survives_preroll_and_each_new_load_reapplies_it() {
        let mut state = VolumeState::new();
        state.set_desired(desired(61, true));
        let mut issued = Vec::new();
        assert!(apply_if_ready(&mut state, 7, true, true, |v| {
            issued.push(v);
            1
        }));
        assert!(apply_if_ready(&mut state, 7, true, false, |v| {
            issued.push(v);
            1
        }));
        assert!(apply_if_ready(&mut state, 8, true, false, |v| {
            issued.push(v);
            1
        }));
        state.forget_pipeline();
        assert_eq!(state.desired(), desired(61, true));
        assert!(apply_if_ready(&mut state, 9, true, false, |v| {
            issued.push(v);
            1
        }));
        assert_eq!(issued, [0, 0, 0, 0]);
    }

    #[test]
    fn setter_results_are_evidence_and_never_measured_readback() {
        let mut state = VolumeState::new();
        assert!(!apply_if_ready(&mut state, 7, true, false, |_| 0));
        assert_eq!(state.readback().last_submission.unwrap().native_result, 0);
        assert_eq!(state.readback().measured_output, None);
        // A refusal cannot make a later synchronization incorrectly look already applied.
        assert!(apply_if_ready(&mut state, 7, true, false, |_| 1));
        assert_eq!(state.readback().measured_output, None);
    }

    #[cfg(feature = "hostsim")]
    #[test]
    fn host_sink_receives_silence_then_the_latest_target_without_a_getter() {
        use plx_platform::tv::sink::VideoSink;

        let _serial = plx_base::testlock::serial();
        super::super::ffi_host::reset_volume_probe_for_test();
        let mt = unsafe { MainThread::assume() };
        let sink = super::super::ffi_host::HostSink;
        let mut state = VolumeState::new();
        state.set_desired(desired(52, false));
        let mut issue = |output| unsafe { sink.set_volume(&mt, i32::from(output)) };
        let _ = apply_if_ready(&mut state, 7, true, false, &mut issue);
        let _ = apply_if_ready(&mut state, 7, true, true, &mut issue);
        state.set_desired(desired(24, false));
        let _ = apply_if_ready(&mut state, 7, true, false, &mut issue);
        assert_eq!(super::super::ffi_host::host_volume_for_test(), 24);
        assert_eq!(
            super::super::ffi_host::verb_order_for_test(),
            ["volume:52", "volume:0", "volume:24"]
        );
        assert_eq!(state.readback().measured_output, None);
        super::super::ffi_host::reset_volume_probe_for_test();
    }
}
