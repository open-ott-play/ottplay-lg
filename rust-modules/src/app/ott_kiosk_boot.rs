//! Addressed startup/queue preparation using the existing recovery slot.
use super::*;
use crate::app::ott_kiosk_stored::{self as stored, Stored, TargetRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::ott_runtime) enum PreparationFailure {
    MissingItem, Unavailable, Timeout, StartFailed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::ott_runtime) enum PreparationOutcome {
    NativeAccepted, Failed(PreparationFailure), Cancelled,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::ott_runtime) enum RestoreStartError {
    Busy, Unavailable, ContextChanged, Invalid,
}
#[derive(Clone)]
pub(super) struct Startup {
    policy: Stored,
    navigation: u64,
    playback: u64,
    ticket: plx_media::route::PlayRequestTicket,
    retry_at: Option<u64>,
}
/// A single synchronous Saved-install handoff, never a new retry owner.
/// The verified receipt keeps the exact opened source alive until publication.
pub(in crate::app::ott_runtime) struct UsbStartup {
    startup: Startup,
    rebased: Stored,
    source: crate::app::ott_local_root::LocalSource,
    relocation: crate::app::ott_usb::VerifiedRelocation,
}

pub(super) enum Preparation {
    Startup(Startup),
    Active {
        receipt: transient::KioskPreparation,
        outcome: Option<PreparationOutcome>,
        observed_at: u64,
    },
}

fn preparation_wait_ms(outcome: Option<PreparationOutcome>) -> u64 {
    if matches!(outcome, Some(PreparationOutcome::Failed(_))) { 10_000 } else { 60_000 }
}

impl Runtime {
    /// Called once by the successful initial Loaded boundary. Merely stages
    /// intent; source registration/revalidation may still be in progress.
    pub(in crate::app::ott_runtime) fn stage_kiosk_startup(&mut self, app: &App) -> bool {
        if !self.initial_kiosk_startup_allowed(app) || self.source_lifecycle.recovery.is_some() {
            return false;
        }
        let Ok(Some(policy)) = stored::read(&self.saved.settings) else { return false; };
        if !matches!(policy.target(), TargetRef::Queue(_) | TargetRef::LocalFolder { .. }) {
            return false;
        }
        self.continuity.suppress_unrequested_start();
        self.source_lifecycle.recovery = Some(Recovery::Preparation(Preparation::Startup(Startup {
            policy, navigation: self.navigation_epoch, playback: self.playback_epoch,
            ticket: plx_media::route::play_request_ticket(), retry_at: None,
        })));
        true
    }

    pub(in crate::app::ott_runtime) fn kiosk_preparation_started(&mut self,
        receipt: transient::KioskPreparation)
    {
        if let Some(Recovery::Preparation(Preparation::Active { receipt: current, .. })) =
            self.source_lifecycle.recovery.as_ref()
        {
            if current.same_attempt(&receipt) { return; }
        }
        self.source_lifecycle.recovery = Some(Recovery::Preparation(Preparation::Active {
            receipt, outcome: None, observed_at: self.now_ms(),
        }));
    }

    pub(in crate::app::ott_runtime) fn kiosk_preparation_finished(&mut self,
        receipt: &transient::KioskPreparation, outcome: PreparationOutcome)
    {
        let now = self.now_ms();
        let Some(Recovery::Preparation(Preparation::Active {
            receipt: current, outcome: previous, observed_at,
        })) = self.source_lifecycle.recovery.as_mut() else { return; };
        if !current.same_attempt(receipt) || *previous == Some(outcome)
            || matches!(previous, Some(PreparationOutcome::Failed(_) | PreparationOutcome::Cancelled))
        { return; }
        if outcome == PreparationOutcome::Cancelled {
            self.source_lifecycle.recovery = None;
        } else {
            // NativeAccepted may carry the exact native admission's new ticket;
            // operation identity remains the same owner/cancel-Arc pair.
            *current = receipt.clone();
            *previous = Some(outcome);
            *observed_at = now;
        }
        self.changed();
    }

    /// Retain only the current source owner's attested preparation before the
    /// existing cancel path retires it. No Saved-only target can enter here.
    pub(in crate::app::ott_runtime) fn preserve_kiosk_preparation(&mut self, app: &App) -> bool {
        if self.resume_pending.is_some() || self.playing.is_some() || lifecycle::viewer_paused() {
            return false;
        }
        let Some(Recovery::Preparation(Preparation::Active { receipt, .. })) =
            self.source_lifecycle.recovery.as_ref() else { return false; };
        // The provider owner checks an extant target against this receipt's
        // exact staged occurrence/member. A newer target is never borrowed.
        let Some(policy) = transient::kiosk_preparation_resume_policy(self, app, receipt) else {
            return false;
        };
        self.source_lifecycle.recovery = Some(Recovery::Preparation(Preparation::Startup(Startup {
            policy, navigation: self.navigation_epoch, playback: self.playback_epoch,
            ticket: plx_media::route::play_request_ticket(), retry_at: None,
        })));
        true
    }

    #[cfg(all(test, feature = "hostsim"))]
    pub(in crate::app::ott_runtime) fn kiosk_preparation_receipt_for_test(&self)
        -> Option<transient::KioskPreparation>
    {
        match self.source_lifecycle.recovery.as_ref() {
            Some(Recovery::Preparation(Preparation::Active { receipt, .. })) => Some(receipt.clone()),
            _ => None,
        }
    }

    pub(super) fn kiosk_preparation_health(&self, app: &App) -> Option<KioskHealth> {
        match self.source_lifecycle.recovery.as_ref()? {
            Recovery::Preparation(Preparation::Startup(startup)) if self.startup_current(app, startup) =>
                Some(KioskHealth::Waiting),
            Recovery::Preparation(Preparation::Active { receipt, outcome, .. })
                if transient::kiosk_preparation_is_current(self, app, receipt) =>
                Some(if matches!(outcome, Some(PreparationOutcome::Failed(_))) { KioskHealth::Error }
                    else if self.source_lifecycle.retry_pending { KioskHealth::Retrying }
                    else { KioskHealth::Starting }),
            _ => None,
        }
    }

    fn startup_current(&self, app: &App, startup: &Startup) -> bool {
        self.loaded && !self.persistence_uncertain
            && self.navigation_epoch == startup.navigation
            && self.playback_epoch == startup.playback
            && plx_media::route::play_request_is_current(startup.ticket)
            && !lifecycle::viewer_paused() && app.player.pending_exit.is_none()
            && self.playing.is_none() && self.play_target.is_none() && self.resume_pending.is_none()
            && stored::read(&self.saved.settings).ok().flatten()
                .is_some_and(|now| now.same_target(&startup.policy))
    }

    /// Capture immediately before this verified USB install reconciles Saved
    /// or invalidates the old catalogue. A path or UUID alone cannot do this.
    pub(in crate::app::ott_runtime) fn capture_usb_startup(
        &self,
        app: &App,
        relocation: &crate::app::ott_usb::VerifiedRelocation,
        source: &crate::app::ott_local_root::LocalSource,
    ) -> Option<UsbStartup> {
        let Some(Recovery::Preparation(Preparation::Startup(startup))) =
            self.source_lifecycle.recovery.as_ref()
        else { return None; };
        if !self.source_work_allowed() || !self.startup_current(app, startup)
            || !relocation.is_current(source)
        { return None; }
        let current = stored::read(&self.saved.settings).ok().flatten()?;
        let TargetRef::LocalFolder { source_id, source_fingerprint } = current.target()
        else { return None; };
        let old = relocation.old_root();
        let new = relocation.new_root();
        if source_id != old.id || source_id != new.id
            || source_fingerprint != selection::local_root_fingerprint(old).ok()?
        { return None; }
        let rebased = Stored::local_folder(current.origin().clone(), current.strict(),
            new.id.clone(), selection::local_root_fingerprint(new).ok()?).ok()?;
        Some(UsbStartup {
            startup: startup.clone(), rebased, source: source.clone(),
            relocation: relocation.clone(),
        })
    }

    /// Called once after the exact Saved install and its ordinary catalogue
    /// reload. Only policy location changes; original navigation, playback,
    /// native admission and retry deadline remain authoritative.
    pub(in crate::app::ott_runtime) fn finish_usb_startup(
        &mut self, app: &App, snapshot: UsbStartup,
    ) -> bool {
        if !self.source_work_allowed() || !snapshot.relocation.is_current(&snapshot.source)
            || !self.local_inventory.sources.iter()
                .any(|source| snapshot.relocation.is_current(source))
        { return false; }
        if let Some(recovery) = self.source_lifecycle.recovery.as_ref() {
            match recovery {
                Recovery::Preparation(Preparation::Startup(current))
                    if current.navigation == snapshot.startup.navigation
                        && current.playback == snapshot.startup.playback
                        && current.ticket == snapshot.startup.ticket
                        && current.policy.same_target(&snapshot.startup.policy) => {}
                _ => return false,
            }
        }
        let Ok(Some(current)) = stored::read(&self.saved.settings) else { return false; };
        if current != snapshot.rebased { return false; }
        let mut startup = snapshot.startup;
        startup.policy = snapshot.rebased;
        if !self.startup_current(app, &startup) { return false; }
        self.continuity.suppress_unrequested_start();
        self.source_lifecycle.recovery = Some(Recovery::Preparation(Preparation::Startup(startup)));
        true
    }

    pub(in crate::app::ott_runtime) fn kiosk_preparation_tick(&mut self, app: &mut App, now: u64) {
        // A background transition is handled by existing cancellation callbacks;
        // a not-yet-admitted startup intent may simply wait for foreground.
        if !self.play_work_allowed() || !playback_allowed(app) || self.saving { return; }
        let Some(Recovery::Preparation(preparation)) = self.source_lifecycle.recovery.as_ref() else { return; };
        match preparation {
            Preparation::Startup(startup) => {
                if !self.startup_current(app, startup) {
                    self.source_lifecycle.recovery = None;
                    return;
                }
                if startup.retry_at.is_some_and(|deadline| now < deadline)
                    || self.transient.candidate.is_some() || self.resolver_next.is_some()
                { return; }
                let policy = startup.policy.clone();
                let result = match policy.target() {
                    TargetRef::Queue(entries) => transient::kiosk_restore_queue(self, app, entries),
                    TargetRef::LocalFolder { source_id, source_fingerprint } => {
                        match self.usb_prepare_kiosk_folder(source_id, source_fingerprint) {
                            Ok(true) => return, // the existing USB/catalogue slot owns this attempt
                            Ok(false) => transient::kiosk_restore_local_folder(
                                self, app, source_id, source_fingerprint),
                            Err(error) => Err(error),
                        }
                    }
                    _ => Err(RestoreStartError::Invalid),
                };
                match result {
                    Ok(()) => { /* the actual owner synchronously published started */ }
                    Err(RestoreStartError::Busy) => {}
                    Err(RestoreStartError::Unavailable) => {
                        if let Some(Recovery::Preparation(Preparation::Startup(startup))) = self.source_lifecycle.recovery.as_mut() {
                            startup.retry_at = Some(now.saturating_add(10_000));
                        }
                    }
                    Err(RestoreStartError::ContextChanged | RestoreStartError::Invalid) => {
                        self.source_lifecycle.recovery = None;
                    }
                }
            }
            Preparation::Active { receipt, outcome, observed_at } => {
                if !transient::kiosk_preparation_is_current(self, app, receipt) {
                    self.source_lifecycle.recovery = None;
                    return;
                }
                if *outcome == Some(PreparationOutcome::NativeAccepted)
                    && self.current_kiosk_pin_owned(app)
                {
                    self.source_lifecycle.recovery = None;
                    return;
                }
                let Some(elapsed) = now.checked_sub(*observed_at) else { return; };
                // A not-yet-ready native session is a bounded startup wait,
                // not evidence that physical video has frozen.
                let interval = preparation_wait_ms(*outcome);
                if elapsed < interval { return; }
                let receipt = receipt.clone();
                let result = transient::retry_kiosk_preparation(self, app, &receipt);
                match result {
                    Ok(()) => {
                        self.source_lifecycle.retries = self.source_lifecycle.retries.saturating_add(1);
                        self.source_lifecycle.retry_pending = true;
                        self.changed();
                    }
                    Err(RestoreStartError::Busy | RestoreStartError::Unavailable) => {
                        if let Some(Recovery::Preparation(Preparation::Active { receipt: current, observed_at, .. })) = self.source_lifecycle.recovery.as_mut() {
                            if current.same_attempt(&receipt) { *observed_at = now; }
                        }
                    }
                    Err(RestoreStartError::ContextChanged | RestoreStartError::Invalid) => {
                        self.source_lifecycle.recovery = None;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preparation_and_native_acceptance_share_bounded_startup_wait() {
        for outcome in [None, Some(PreparationOutcome::NativeAccepted)] {
            assert!(59_999 < preparation_wait_ms(outcome));
            assert!(60_000 >= preparation_wait_ms(outcome));
        }
        for failure in [PreparationFailure::MissingItem, PreparationFailure::Unavailable,
            PreparationFailure::Timeout, PreparationFailure::StartFailed]
        {
            let wait = preparation_wait_ms(Some(PreparationOutcome::Failed(failure)));
            assert!(9_999 < wait);
            assert!(10_000 >= wait);
        }
    }
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_usb_startup_tests.rs"]
mod usb_startup_tests;
