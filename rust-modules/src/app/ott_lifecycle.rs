//! Runtime-owned admission and retirement of source work.
//! No additional workers, clocks, catalogue or playback identity live here.
use super::*;
#[path = "ott_kiosk_boot.rs"]
mod boot;
pub(super) use boot::{PreparationFailure, PreparationOutcome, RestoreStartError, UsbStartup};

pub(super) struct State {
    allowed: bool,
    rearm_catalog: bool,
    rearm_metadata: bool,
    plex_cancel: Arc<AtomicBool>,
    recovery: Option<Recovery>,
    retries: u32,
    retry_pending: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            allowed: false,
            rearm_catalog: false,
            rearm_metadata: true,
            plex_cancel: Arc::new(AtomicBool::new(false)),
            recovery: None,
            retries: 0,
            retry_pending: false,
        }
    }
}

impl Runtime {
    pub(super) fn source_work_allowed(&self) -> bool {
        self.source_lifecycle.allowed
    }

    pub(super) fn play_work_allowed(&self) -> bool {
        self.source_work_allowed() && !self.source_lifecycle.rearm_catalog
    }

    pub(super) fn sync_source_lifecycle(&mut self, app: &mut App) {
        let allowed = playback_allowed(app);
        if !allowed && self.source_lifecycle.allowed {
            self.suspend(app);
        } else if allowed && !self.source_lifecycle.allowed {
            self.resume_source_work();
        }
    }

    /// Preserve only a source-owner-validated failed initial request before
    /// cancellation invalidates its admission ticket. The existing resume slot
    /// carries the typed policy/source fence through the ordinary fresh binder.
    pub(super) fn preserve_admitted_resume(&mut self, app: &App) {
        if self.resume_pending.is_some() || self.play_target.is_some()
            || lifecycle::viewer_paused()
        {
            return;
        }
        let Some(Recovery::Admitted(state)) = self.source_lifecycle.recovery.as_ref() else {
            return;
        };
        self.resume_pending = kiosk_source::admitted_resume_request(self, app, &state.attempt);
    }

    pub(super) fn suspend_source_work(&mut self, app: &mut App) {
        if !std::mem::replace(&mut self.source_lifecycle.allowed, false) {
            return;
        }
        self.usb_suspend_browser();
        self.source_lifecycle.rearm_catalog = true;
        // A request can also be queued later while suspended. Always require
        // one fresh rebind after wake, even if no target exists at this edge.
        self.source_lifecycle.rearm_metadata = true;
        self.cancel.store(true, Ordering::Release);
        self.metadata_cancel.store(true, Ordering::Release);
        self.source_lifecycle.plex_cancel.store(true, Ordering::Release);

        let previous_generation = self.generation;
        let next_generation = self.generation.wrapping_add(1).max(1);
        self.rebase_catalog_return_for_lifecycle(next_generation);
        self.generation = next_generation;
        classic_source::rebase(self, previous_generation);
        self.continuity.rebase(self.generation);
        self.continuity.suspend(self.generation);
        self.metadata_generation = self.metadata_generation.wrapping_add(1);
        self.metadata_refresh_ms = 0;
        self.metadata_next = None;
        self.metadata_local_selection = None;
        self.suspend_local_assets();

        // REQUIRED EXTERNAL OWNER HOOK: retire the candidate's addressed receipt
        // and mark consumed LocalBindings work for revalidation. It must retain
        // an accepted Local source and its order; no Local state is copied here.
        transient::suspend_for_lifecycle(self, app, previous_generation);
        // Do not clear catalog, pending restore, catalog_next or the real
        // worker done flags. Old producers still own those done flags. Metadata
        // retains only its logical target: a queued permit must be rebound.
    }

    pub(super) fn resume_source_work(&mut self) {
        self.source_lifecycle.allowed = true;
        self.plex_refresh_requested = true;
        // This clears only lifecycle suspension, not an earlier viewer Pause.
        self.continuity.resume_after_suspend(self.generation);
    }

    /// Called before eager launch on the existing runtime tick. Revalidation of
    /// Local bindings must wait until the old shared catalogue worker retires.
    pub(super) fn rearm_source_work(&mut self, app: &mut App) {
        classic_source::reconcile(self, app);
        if !self.source_work_allowed() {
            return;
        }
        if self.source_lifecycle.rearm_catalog && self.catalog_done.load(Ordering::Acquire) {
            self.cancel = Arc::new(AtomicBool::new(false));
            // REQUIRED EXTERNAL OWNER HOOK: requeue only the current accepted
            // Local owner/remembered inventory, never the retired invocation.
            transient::resume_after_lifecycle(self, app);
            self.usb_rearm_after_foreground();
            if self.catalog_loading && !self.catalog_complete && self.catalog_next.is_none()
                && self.transient.accepted.is_none() && self.transient.candidate.is_none()
            {
                self.catalog_next = self.effective_profile().cloned();
            }
            self.source_lifecycle.rearm_catalog = false;
            self.catalog_dirty = true;
        }
        self.rearm_metadata_work();
    }

    pub(super) fn catalog_work_allowed(&self) -> bool {
        self.source_work_allowed() && !self.source_lifecycle.rearm_catalog
    }

    pub(super) fn rearm_metadata_work(&mut self) {
        if !self.source_work_allowed() || !self.source_lifecycle.rearm_metadata
            || !self.metadata_done.load(Ordering::Acquire)
            || !transient::lifecycle_metadata_ready(self)
        {
            return;
        }
        self.metadata_cancel = Arc::new(AtomicBool::new(false));
        // Also discard a selection queued while suspended. Refresh clears the
        // same-target shortcut before requesting a Favorite and obtains a new
        // source binding/proof through the existing owner.
        self.metadata_next = None;
        self.metadata_local_selection = None;
        if self.metadata_target.is_some() {
            // Existing refresh logic resolves the target against current owners.
            // 0 is its disabled sentinel; no new deadline/clock is introduced.
            self.metadata_refresh_ms = 1;
        }
        self.source_lifecycle.rearm_metadata = false;
    }

    pub(super) fn metadata_work_allowed(&self) -> bool {
        self.source_work_allowed() && !self.source_lifecycle.rearm_metadata
            && transient::lifecycle_metadata_ready(self)
    }

    pub(super) fn begin_plex_restore(&mut self) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        self.source_lifecycle.plex_cancel = cancel.clone();
        cancel
    }

    /// A duplicate/late result cannot release a newer worker's single-flight
    /// slot. Cancellation is checked before registering any returned source.
    pub(super) fn accept_plex_restore(&mut self, cancel: &Arc<AtomicBool>) -> bool {
        if !Arc::ptr_eq(cancel, &self.source_lifecycle.plex_cancel) {
            return false;
        }
        self.plex_refresh_pending = false;
        if !self.source_work_allowed() || cancel.load(Ordering::Acquire) {
            self.plex_refresh_requested = true;
            return false;
        }
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KioskHealth { Idle, Waiting, Starting, Playing, Retrying, Error }
impl KioskHealth {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle", Self::Waiting => "waiting", Self::Starting => "starting",
            Self::Playing => "playing", Self::Retrying => "retrying", Self::Error => "error",
        }
    }
}
pub(super) struct KioskStatus {
    pub health: KioskHealth,
    pub retries: u32,
    pub retry_seconds: u8,
    pub screen_suspended: bool,
}

enum Recovery {
    Preparation(boot::Preparation),
    Installed(InstalledRecovery),
    Admitted(AdmittedRecovery),
}

struct AdmittedRecovery {
    attempt: kiosk_source::AdmittedAttempt,
    last_ms: u64,
    quiet_ms: u64,
    failed: bool,
}

struct InstalledRecovery {
    generation: u64,
    identity: plx_media::route::PlaybackIdentity,
    last_ms: u64,
    position: Option<i64>,
    quiet_ms: u64,
}

fn startup(phase: plx_media::player::PlaybackState) -> bool {
    use plx_media::player::PlaybackState as Phase;
    matches!(phase, Phase::Resolving | Phase::Connecting | Phase::Buffering | Phase::Seeking)
}

fn quiet_after(quiet: u64, elapsed: Option<u64>, before: Option<i64>, after: Option<i64>,
    phase: plx_media::player::PlaybackState) -> u64
{
    let Some(elapsed) = elapsed else { return 0; };
    // Startup is a bounded wait for readiness, not an inference of frozen video.
    // Unknown positions in established playback are never treated as a stall.
    if startup(phase) || phase == plx_media::player::PlaybackState::Error
        || matches!((before, after), (Some(before), Some(after)) if before == after)
    {
        quiet.saturating_add(elapsed)
    } else { 0 }
}

fn retry_due(phase: plx_media::player::PlaybackState, quiet: u64) -> bool {
    quiet >= if startup(phase) { 60_000 } else { 10_000 }
}

impl Runtime {
    /// Fence only this synchronous native admission. A different error branch
    /// cannot refresh an old request's ticket into a new authority.
    pub(super) fn start_resolved_with_kiosk(&mut self, app: &mut App,
        profile: ProviderProfile, entry: CatalogEntry, source: ReadySource,
    ) -> Result<(), &'static str> {
        self.start_resolved_with_kiosk_using(app, profile, entry, source, start_resolved)
    }

    pub(super) fn start_resolved_with_kiosk_using(
        &mut self, app: &mut App, profile: ProviderProfile, entry: CatalogEntry, source: ReadySource,
        start: impl FnOnce(&mut Runtime, &mut App, ProviderProfile, CatalogEntry, ReadySource) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        let pending = self.source_lifecycle.recovery.as_ref().and_then(|state| match state {
            Recovery::Admitted(state) => kiosk_source::begin_native_admission(self, app, &state.attempt)
                .map(|fence| (state.attempt.clone(), fence)),
            _ => None,
        });
        let result = start(self, app, profile, entry, source);
        if let Some((mut attempt, fence)) = pending {
            let current = kiosk_source::finish_native_admission(self, app, &mut attempt, fence);
            if let Some(Recovery::Admitted(state)) = self.source_lifecycle.recovery.as_mut() {
                if state.attempt.play_generation == attempt.play_generation {
                    if current { state.attempt = attempt; }
                    else { self.source_lifecycle.recovery = None; }
                }
            }
        }
        result
    }

    /// Only the source owner can construct this receipt, after actual request
    /// admission. No locator-only restart and no synthetic native identity.
    pub(super) fn note_kiosk_attempt(&mut self, attempt: kiosk_source::AdmittedAttempt) {
        self.note_kiosk_attempt_at(attempt, self.now_ms());
    }

    fn note_kiosk_attempt_at(&mut self, attempt: kiosk_source::AdmittedAttempt, now: u64) {
        if attempt.play_generation != self.play_generation { return; }
        if matches!(self.source_lifecycle.recovery.as_ref(), Some(Recovery::Admitted(current))
            if current.attempt.same_admission(&attempt))
        { return; }
        self.source_lifecycle.recovery = Some(Recovery::Admitted(AdmittedRecovery {
            attempt, last_ms: now, quiet_ms: 0, failed: false,
        }));
    }

    /// Called from a matching, nonexpired request's actual failure boundary.
    /// True means the authoritative pin retains retry policy rather than the
    /// ordinary catalogue advancing to a different item.
    pub(super) fn kiosk_resolve_failed(&mut self, app: &App, generation: u64) -> bool {
        let Some(Recovery::Admitted(state)) = self.source_lifecycle.recovery.as_ref() else { return false; };
        if state.attempt.play_generation != generation
            || !kiosk_source::attempt_current(self, app, &state.attempt)
        {
            return false;
        }
        let now = self.now_ms();
        let Some(Recovery::Admitted(state)) = self.source_lifecycle.recovery.as_mut() else { return false; };
        state.failed = true;
        state.last_ms = now;
        state.quiet_ms = 0;
        true
    }

    fn admitted_kiosk_recovery_tick_at(&mut self, app: &mut App, now: u64) {
        let Some(Recovery::Admitted(state)) = self.source_lifecycle.recovery.as_ref() else { return; };
        if !kiosk_source::attempt_current(self, app, &state.attempt) {
            self.source_lifecycle.recovery = None;
            return;
        }
        let Some(Recovery::Admitted(state)) = self.source_lifecycle.recovery.as_mut() else { return; };
        state.quiet_ms = now.checked_sub(state.last_ms)
            .map_or(0, |elapsed| state.quiet_ms.saturating_add(elapsed));
        state.last_ms = now;
        let interval = if state.failed { 10_000 } else { 60_000 };
        if state.quiet_ms < interval { return; }
        state.quiet_ms = 0;
        let attempt = state.attempt.clone();
        if let Ok(next) = kiosk_source::retry_attempt(self, app, &attempt) {
            self.source_lifecycle.retries = self.source_lifecycle.retries.saturating_add(1);
            self.source_lifecycle.retry_pending = true;
            self.note_kiosk_attempt_at(next, now);
            self.changed();
        }
    }

    /// A strictness-only edit takes effect in the current policy gates; it
    /// neither restarts playback nor retires an existing source-owned retry.
    pub(super) fn reconcile_kiosk_policy(&mut self, next: &Saved) {
        use crate::app::ott_kiosk_stored as stored;
        if self.saved.settings.get(stored::KEY) == next.settings.get(stored::KEY) {
            return;
        }
        if let (Ok(Some(before)), Ok(Some(after))) =
            (stored::read(&self.saved.settings), stored::read(&next.settings))
        {
            if before.with_strict(false) == after.with_strict(false) {
                return;
            }
        }
        self.reset_kiosk_recovery();
    }

    pub(super) fn reset_kiosk_recovery(&mut self) {
        self.source_lifecycle.recovery = None;
        self.source_lifecycle.retries = 0;
        self.source_lifecycle.retry_pending = false;
    }

    pub(super) fn kiosk_health(&self, app: &App) -> Result<KioskStatus, ()> {
        use crate::app::ott_kiosk_policy::State as Kiosk;
        use plx_media::player::PlaybackState as Phase;
        let policy = self.track_selection_kiosk()?;
        let screen_suspended = !playback_allowed(app);
        let admitted = self.source_lifecycle.recovery.as_ref().and_then(|state| match state {
            Recovery::Admitted(state) if kiosk_source::attempt_current(self, app, &state.attempt) => Some(state),
            _ => None,
        });
        let health = match policy {
            Kiosk::Off => KioskHealth::Idle,
            Kiosk::Armed { .. } => KioskHealth::Waiting,
            Kiosk::Locked { .. } if screen_suspended => KioskHealth::Waiting,
            Kiosk::Locked { .. } if self.kiosk_preparation_health(app).is_some() =>
                self.kiosk_preparation_health(app).unwrap(),
            Kiosk::Locked { .. } if admitted.is_some() => {
                if admitted.unwrap().failed { KioskHealth::Error }
                else if self.source_lifecycle.retry_pending { KioskHealth::Retrying }
                else { KioskHealth::Starting }
            }
            Kiosk::Locked { .. } if !self.current_kiosk_pin_owned(app) => KioskHealth::Waiting,
            Kiosk::Locked { .. } => match plx_media::player::state(&app.player.session) {
                Phase::Playing if !lifecycle::viewer_paused() => KioskHealth::Playing,
                Phase::Resolving | Phase::Connecting | Phase::Buffering | Phase::Seeking => {
                    if self.source_lifecycle.retry_pending { KioskHealth::Retrying }
                    else { KioskHealth::Starting }
                }
                Phase::Error => KioskHealth::Error,
                _ => KioskHealth::Waiting,
            },
        };
        Ok(KioskStatus { health,
            retries: if policy == Kiosk::Off { 0 } else { self.source_lifecycle.retries },
            retry_seconds: 10, screen_suspended })
    }

    /// The pin owner supplies the authoritative locator/source/proof match.
    /// The existing playback identity and generation fence the observation;
    /// this is not another pin or a diagnostic-mirror feedback controller.
    pub(super) fn kiosk_recovery_tick(&mut self, app: &mut App) {
        self.kiosk_recovery_tick_at(app, self.now_ms());
    }

    pub(super) fn kiosk_recovery_tick_at(&mut self, app: &mut App, now: u64) {
        use crate::app::ott_kiosk_policy::State as Kiosk;
        use plx_media::player::PlaybackState as Phase;
        if self.archive_playback_owned(app) {
            self.reset_kiosk_recovery();
            return;
        }
        if matches!(self.source_lifecycle.recovery, Some(Recovery::Preparation(_))) {
            self.kiosk_preparation_tick(app, now);
            return;
        }
        if !matches!(self.track_selection_kiosk(), Ok(Kiosk::Locked { .. }))
            || !self.play_work_allowed() || !playback_allowed(app) || lifecycle::viewer_paused()
        {
            self.source_lifecycle.recovery = None;
            return;
        }
        if matches!(self.source_lifecycle.recovery, Some(Recovery::Admitted(_))) {
            if self.current_kiosk_pin_owned(app) && self.play_target.is_none() && !self.resolver_busy {
                self.source_lifecycle.recovery = None;
            } else {
                self.admitted_kiosk_recovery_tick_at(app, now);
                return;
            }
        }
        if !self.current_kiosk_pin_owned(app) {
            self.source_lifecycle.recovery = None;
            return;
        }
        let Some(identity) = plx_media::route::playback_identity(&app.player.session) else {
            self.source_lifecycle.recovery = None;
            return;
        };
        let phase = plx_media::player::state(&app.player.session);
        if matches!(phase, Phase::Idle | Phase::Paused) {
            self.source_lifecycle.recovery = None;
            return;
        }
        let position = plx_media::player::playpos_ns();
        let position = (position >= 0).then_some(position);
        let fresh = !matches!(self.source_lifecycle.recovery.as_ref(), Some(Recovery::Installed(old))
            if old.generation == self.play_generation && old.identity == identity);
        if fresh {
            self.source_lifecycle.recovery = Some(Recovery::Installed(InstalledRecovery {
                generation: self.play_generation, identity, last_ms: now, position, quiet_ms: 0,
            }));
            return;
        }
        let Some(Recovery::Installed(observation)) = self.source_lifecycle.recovery.as_mut() else { return; };
        let elapsed = now.checked_sub(observation.last_ms);
        observation.last_ms = now;
        let advanced = matches!((observation.position, position), (Some(before), Some(after)) if after > before);
        observation.quiet_ms = quiet_after(observation.quiet_ms, elapsed,
            observation.position, position, phase);
        observation.position = position;
        if advanced && phase == Phase::Playing { self.source_lifecycle.retry_pending = false; }
        if !retry_due(phase, observation.quiet_ms) { return; }
        observation.quiet_ms = 0;
        // Check current owner again immediately before the existing actuator.
        if self.current_kiosk_pin_owned(app) && self.play_work_allowed()
            && commands::restart_for_recovery(self, app)
        {
            self.source_lifecycle.retries = self.source_lifecycle.retries.saturating_add(1);
            self.source_lifecycle.retry_pending = true;
            self.source_lifecycle.recovery = None;
            self.changed();
        }
    }
}

#[cfg(test)]
mod recovery_interval_tests {
    use super::*;
    use plx_media::player::PlaybackState as Phase;
    #[test]
    fn admitted_startup_with_unknown_position_has_a_bounded_readiness_wait() {
        for phase in [Phase::Resolving, Phase::Connecting, Phase::Buffering, Phase::Seeking] {
            let quiet = quiet_after(0, Some(59_999), None, None, phase);
            assert!(!retry_due(phase, quiet));
            let quiet = quiet_after(quiet, Some(1), None, None, phase);
            assert!(retry_due(phase, quiet));
        }
    }
    #[test]
    fn unknown_position_in_established_playback_is_not_a_frozen_video_signal() {
        let quiet = quiet_after(9_999, Some(600_000), None, None, Phase::Playing);
        assert_eq!(quiet, 0);
        assert!(!retry_due(Phase::Playing, quiet));
        assert_eq!(quiet_after(9_999, Some(1), Some(10), Some(11), Phase::Playing), 0);
    }
}
