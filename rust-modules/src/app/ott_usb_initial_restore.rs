//! One cold-load scheduling preference inside the existing USB operation.
//! Saved metadata chooses what to verify first; only Restore grants a root.
use super::*;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Phase {
    #[default]
    None,
    Armed,
    Owned,
    // Keep the existing staged writer visible to ACK handling, but never revive it.
    Retired,
}

impl Runtime {
    /// Called only for the first decoded Saved image, before its load_catalog.
    /// The expected generation spans that one synchronous catalog reset only.
    pub(in crate::app::ott_runtime) fn arm_initial_usb_restore(&mut self, app: &App) {
        if !self.initial_kiosk_startup_allowed(app)
            || !playback_allowed(app)
            || !self.source_work_allowed()
            || self.cancel.load(Ordering::Acquire)
            || self.saving
            || self.pending_play.is_some()
            || self.transient.accepted.is_some()
            || !self.local_inventory.sources.is_empty()
            || !matches!(
                self.local_inventory.phase,
                transient::InventoryPhase::Unloaded
            )
            || self.saved.favorites_selected
            || !crate::app::ott_kiosk_stored::read(&self.saved.settings)
                .is_ok_and(|policy| policy.is_none())
            || self.local_inventory.usb.waiting
            || self.local_inventory.usb.pending.is_some()
            || self.local_inventory.usb.install.is_some()
            || self.usb_browsing()
        {
            return;
        }
        let Some(profile) = self.active().filter(|profile| {
            profile.enabled
                && profile.kind == ProviderKind::Local
                && self.usb_folder_path(&profile.source).unwrap_or(false)
                && profile.validate().is_ok()
        }) else {
            return;
        };
        let Ok((target, usb::Task::Restore { binding, .. })) = self.usb_restore_target(profile)
        else {
            return;
        };
        if binding.validate().is_err()
            || target
                .folders
                .folders()
                .iter()
                .filter(|folder| folder.path == profile.source)
                .count()
                != 1
        {
            return;
        }
        let remote = std::mem::take(&mut self.local_inventory.usb.remote);
        self.local_inventory.usb = State {
            remote,
            generation: self.generation.wrapping_add(1),
            navigation: self.navigation_epoch,
            playback: self.playback_epoch,
            ticket: Some(self.initial_native_ticket),
            automatic: true,
            initial_restore: Phase::Armed,
            target: Some(target),
            ..Default::default()
        };
    }

    pub(super) fn initial_usb_context_current(&self) -> bool {
        self.loaded
            && !self.persistence_uncertain
            && self.source_work_allowed()
            && !self.cancel.load(Ordering::Acquire)
            && self.usb_context_current()
            && plx_media::route::play_request_is_current(self.initial_native_ticket)
            && !plx_media::route::play_pending()
            && self.playing.is_none()
            && self.play_target.is_none()
            && self.pending_play.is_none()
            && self.resume_pending.is_none()
            && self.transient.accepted.is_none()
            && self.transient.candidate.is_none()
            && self.local_inventory.sources.is_empty()
            && matches!(
                self.local_inventory.phase,
                transient::InventoryPhase::Unloaded
            )
            && !self.saved.favorites_selected
            && crate::app::ott_kiosk_stored::read(&self.saved.settings)
                .is_ok_and(|policy| policy.is_none())
            && self
                .local_inventory
                .usb
                .target
                .as_ref()
                .is_some_and(|target| {
                    target.kiosk.is_none()
                        && target.current(self)
                        && self.active() == Some(&target.profile)
                })
    }

    /// Completions can arrive before the task selector runs again. A cold
    /// proposal must still pass its full intent fence at result and Saved ACK.
    pub(super) fn initial_usb_completion_current(&self) -> bool {
        match self.local_inventory.usb.initial_restore {
            Phase::None => true,
            Phase::Retired => false,
            Phase::Armed | Phase::Owned => self.initial_usb_context_current(),
        }
    }

    /// Some(None) retains this initial operation's turn while it has no task:
    /// an absent drive must not fall through to unrelated all-root validation.
    /// None relinquishes only this preference; normal intent handling continues.
    pub(in crate::app::ott_runtime) fn take_initial_usb_catalog_task(
        &mut self,
    ) -> Option<Option<ott_catalog::Task>> {
        let phase = self.local_inventory.usb.initial_restore;
        if matches!(phase, Phase::None | Phase::Retired) {
            return None;
        }
        if !self.initial_usb_context_current() {
            // Retire only this old USB work. Falling straight through would let
            // take_usb_task admit its pending task with weaker generic guards.
            self.retire_usb_work();
            return None;
        }
        if phase == Phase::Armed {
            let profile = self
                .local_inventory
                .usb
                .target
                .as_ref()
                .unwrap()
                .profile
                .clone();
            if self.catalog_next.as_ref() != Some(&profile) {
                self.retire_usb_work();
                return None;
            }
            if self.saving {
                return Some(None);
            }
            let result = self
                .usb_restore_target(&profile)
                .and_then(|(target, task)| self.usb_begin_mode(task, target, true, true));
            self.local_inventory.usb.initial_restore = Phase::Owned;
            if let Err(error) = result {
                self.catalog_next = None;
                self.catalog_loading = false;
                self.local_inventory.usb.error = Some(error);
                self.status = error.into();
                self.changed();
                return Some(None);
            }
        }
        Some(self.take_usb_task())
    }
}
