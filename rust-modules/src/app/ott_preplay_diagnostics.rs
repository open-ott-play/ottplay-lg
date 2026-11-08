//! Passive, bounded pre-play evidence. Private bindings never enter the wire snapshot.
//! This module neither authorizes playback nor changes resolve/cancel decisions.
use super::{Job, PlaybackRequest, ProviderKind, ProviderProfile, Runtime};
use serde::Serialize;
use std::sync::{mpsc::Sender, Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Phase {
    Idle,
    Resolving,
    Inspecting,
    AwaitingQuality,
    Admitting,
    Admitted,
    Failed,
    Cancelled,
}

impl Phase {
    fn terminal(self) -> bool {
        matches!(self, Self::Admitted | Self::Failed | Self::Cancelled)
    }

    fn permits(self, next: Self) -> bool {
        matches!((self, next),
            (Self::Resolving, Self::Inspecting | Self::AwaitingQuality | Self::Admitting)
            | (Self::AwaitingQuality, Self::Resolving | Self::Inspecting)
            | (Self::Inspecting, Self::Admitting)
            | (Self::Admitting, Self::Admitted))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Failure {
    ResolveFailed,
    InspectFailed,
    WorkerUnavailable,
    AdmissionFailed,
    QualitySaveFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CancelReason {
    Cancelled,
    Superseded,
    Expired,
    OwnerChanged,
    UserBack,
}

// No Debug or Serialize: these values may contain credentials and private titles.
struct Binding {
    generation: u64,
    profile: ProviderProfile,
    request: PlaybackRequest,
}

impl Binding {
    fn matches(&self, generation: u64, profile: Option<&ProviderProfile>,
        request: Option<&PlaybackRequest>) -> bool {
        self.generation == generation && profile == Some(&self.profile)
            && request == Some(&self.request)
    }
}

#[derive(Clone, Copy)]
enum Update {
    Phase(Phase),
    Failed(Failure),
    Cancelled(CancelReason),
}

/// Only a reporter captured from this module can construct an addressed event.
pub(super) struct Event {
    binding: Arc<Binding>,
    update: Update,
}

struct Attempt {
    binding: Arc<Binding>,
    phase: Phase,
    phase_started_ms: u64,
    finished_ms: Option<u64>,
    last_failure: Option<Failure>,
    cancel_reason: Option<CancelReason>,
}

#[derive(Default)]
pub(super) struct State {
    attempt: Option<Attempt>,
}

#[derive(Serialize)]
pub(super) struct Snapshot {
    version: u8,
    phase: Phase,
    attempt_generation: Option<u64>,
    phase_elapsed_ms: u64,
    last_failure: Option<Failure>,
    cancel_reason: Option<CancelReason>,
}

impl State {
    fn begin(&mut self, binding: Arc<Binding>, now_ms: u64) {
        self.attempt = Some(Attempt { binding, phase: Phase::Resolving,
            phase_started_ms: now_ms, finished_ms: None, last_failure: None,
            cancel_reason: None });
    }

    fn apply(&mut self, event: Event, now_ms: u64) {
        let Some(attempt) = self.attempt.as_mut() else { return; };
        if !Arc::ptr_eq(&attempt.binding, &event.binding) || attempt.phase.terminal() {
            return;
        }
        let now_ms = now_ms.max(attempt.phase_started_ms);
        match event.update {
            Update::Phase(next) if attempt.phase.permits(next) => {
                if next.terminal() {
                    attempt.finished_ms = Some(now_ms);
                } else {
                    attempt.phase_started_ms = now_ms;
                }
                attempt.phase = next;
            }
            Update::Phase(_) => {}
            Update::Failed(Failure::QualitySaveFailed)
                if attempt.phase == Phase::AwaitingQuality => {
                // The existing picker permits retry after a rejected write.
                attempt.last_failure = Some(Failure::QualitySaveFailed);
            }
            Update::Failed(failure) => {
                attempt.phase = Phase::Failed;
                attempt.finished_ms = Some(now_ms);
                attempt.last_failure = Some(failure);
            }
            Update::Cancelled(reason) => {
                attempt.phase = Phase::Cancelled;
                attempt.finished_ms = Some(now_ms);
                attempt.cancel_reason = Some(reason);
            }
        }
    }

    /// Active timing starts when the app accepts the phase event, not at a worker
    /// clock sample. A terminal record freezes the duration of the phase it ended.
    /// This is elapsed observation time, never a timeout or a playback decision.
    pub(super) fn snapshot(&self, now_ms: u64) -> Snapshot {
        match &self.attempt {
            None => Snapshot { version: 1, phase: Phase::Idle, attempt_generation: None,
                phase_elapsed_ms: 0, last_failure: None, cancel_reason: None },
            Some(attempt) => Snapshot {
                version: 1,
                phase: attempt.phase,
                attempt_generation: Some(attempt.binding.generation),
                phase_elapsed_ms: attempt.finished_ms.unwrap_or(now_ms)
                    .saturating_sub(attempt.phase_started_ms),
                last_failure: attempt.last_failure,
                cancel_reason: attempt.cancel_reason,
            },
        }
    }
}

fn current(runtime: &Runtime, binding: &Binding) -> bool {
    binding.matches(runtime.play_generation, runtime.pending_play_profile(),
        runtime.play_target.as_ref())
}

fn current_binding(runtime: &Runtime, generation: u64) -> Option<Arc<Binding>> {
    let binding = &runtime.preplay.attempt.as_ref()?.binding;
    (binding.generation == generation && current(runtime, binding))
        .then(|| binding.clone())
}

/// Call after the existing admission has installed play_target and play_profile.
/// Starting any other provider clears this VPortal-only observation.
pub(super) fn begin(runtime: &mut Runtime, profile: &ProviderProfile) {
    let binding = runtime.play_target.as_ref().filter(|_| profile.kind == ProviderKind::VPortal
        && runtime.pending_play_profile() == Some(profile)).map(|request| Arc::new(Binding {
            generation: runtime.play_generation, profile: profile.clone(), request: request.clone(),
        }));
    let now_ms = runtime.now_ms();
    match binding {
        Some(binding) => runtime.preplay.begin(binding, now_ms),
        None => runtime.preplay = State::default(),
    }
}

pub(super) fn receive(runtime: &mut Runtime, event: Event) {
    if current(runtime, &event.binding) {
        let now_ms = runtime.now_ms();
        runtime.preplay.apply(event, now_ms);
    }
}

pub(super) fn phase(runtime: &mut Runtime, generation: u64, phase: Phase) {
    if let Some(binding) = current_binding(runtime, generation) {
        receive(runtime, Event { binding, update: Update::Phase(phase) });
    }
}

pub(super) fn failed(runtime: &mut Runtime, generation: u64, failure: Failure) {
    if let Some(binding) = current_binding(runtime, generation) {
        receive(runtime, Event { binding, update: Update::Failed(failure) });
    }
}

/// Capture the old attempt before the caller increments generation or clears it.
/// Source authorization is deliberately not re-run here: this observes the real
/// cancellation, including one caused by a source that is no longer authorized.
pub(super) fn cancel(runtime: &mut Runtime, reason: CancelReason) {
    if let Some(binding) = current_binding(runtime, runtime.play_generation) {
        receive(runtime, Event { binding, update: Update::Cancelled(reason) });
    }
}

/// A captured reporter owns no grant, starts no work and reads no clock. An
/// inactive reporter still executes inspect's closure unchanged, without events.
#[derive(Clone)]
pub(super) struct Worker {
    binding: Option<Arc<Binding>>,
    sender: Sender<Job>,
    phase: Phase,
}

pub(super) fn worker(runtime: &Runtime, generation: u64, profile: &ProviderProfile) -> Worker {
    Worker {
        binding: current_binding(runtime, generation).filter(|binding| &binding.profile == profile),
        sender: runtime.tx.clone(),
        phase: Phase::Resolving,
    }
}

impl Worker {
    fn send(&self, update: Update) {
        if let Some(binding) = &self.binding {
            if self.sender.send(Job::Preplay(Event { binding: binding.clone(), update })).is_ok() {
                // The inspected operation can stall; expose its phase without
                // waiting for the final playback completion to wake the loop.
                plx_machine::idle::invalidate();
            }
        }
    }

    pub(super) fn phase(&mut self, phase: Phase) {
        self.phase = phase;
        self.send(Update::Phase(phase));
    }

    /// Classify at the reached operation boundary, never by provider error text.
    pub(super) fn failed(&self) {
        self.send(Update::Failed(if self.phase == Phase::Inspecting {
            Failure::InspectFailed
        } else {
            Failure::ResolveFailed
        }));
    }

    pub(super) fn inspect<T>(&mut self, operation: impl FnOnce() -> Result<T, &'static str>)
        -> Result<T, &'static str> {
        self.phase(Phase::Inspecting);
        let result = operation();
        if result.is_err() { self.failed(); }
        result
    }
}

#[cfg(test)]
#[path = "ott_preplay_diagnostics_tests.rs"]
mod tests;
