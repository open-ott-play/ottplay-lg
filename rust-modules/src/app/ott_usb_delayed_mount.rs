//! Finite retry policy inside the existing USB owner, not a hotplug monitor.
//! Every retry uses the ordinary Restore worker and its complete identity proof.
use super::*;

const RETRY_WINDOW_MS: u64 = 120_000;
const RETRY_BACKOFF_MS: [u64; 3] = [2_000, 5_000, 10_000];

pub(super) struct Retry {
    pub(super) deadline_ms: u64,
    next_ms: Option<u64>,
    backoff: usize,
}

impl Retry {
    pub(super) fn start(now: u64) -> Self {
        Self {
            deadline_ms: now.saturating_add(RETRY_WINDOW_MS),
            next_ms: None,
            backoff: 0,
        }
    }

    fn unavailable(&mut self, now: u64) {
        self.next_ms = Some(now.saturating_add(RETRY_BACKOFF_MS[self.backoff]));
        self.backoff = (self.backoff + 1).min(RETRY_BACKOFF_MS.len() - 1);
    }
}

impl Runtime {
    /// Only an observed unavailable mount/backend is retryable. Missing folders,
    /// permission failures, ambiguous identities and cancelled work remain errors.
    pub(super) fn usb_delayed_mount_failed(&mut self, error: usb::Error) {
        if self.local_inventory.usb.delayed_mount.is_none() {
            return;
        }
        let now = self.now_ms();
        if error != usb::Error::Unavailable || !self.usb_delayed_mount_current() {
            self.local_inventory.usb.delayed_mount = None;
            return;
        }
        if let Some(retry) = self.local_inventory.usb.delayed_mount.as_mut() {
            retry.unavailable(now);
        }
    }

    fn usb_delayed_mount_current(&self) -> bool {
        self.loaded
            && self.initial_usb_completion_current()
            && !self.persistence_uncertain
            && self.source_work_allowed()
            && !self.cancel.load(Ordering::Acquire)
            && self.local_inventory.usb.automatic
            && self.usb_context_current()
            && self.playing.is_none()
            && self.play_target.is_none()
            && self.pending_play.is_none()
            && self.resume_pending.is_none()
            && self.transient.accepted.is_none()
            && self.transient.candidate.is_none()
            && self
                .local_inventory
                .usb
                .target
                .as_ref()
                .is_some_and(|target| {
                    target.kiosk.is_none()
                        && target.current(self)
                        && self.effective_profile() == target.previous.as_ref()
                })
    }

    /// `now` is the Runtime's existing monotonic frame sample. This method only
    /// queues an addressed task; the existing catalogue launcher owns execution.
    pub(in crate::app::ott_runtime) fn usb_delayed_mount_tick_at(&mut self, app: &App, now: u64) {
        let Some(retry) = self.local_inventory.usb.delayed_mount.as_ref() else {
            return;
        };
        if now >= retry.deadline_ms
            || !playback_allowed(app)
            || !self.usb_delayed_mount_current()
            || self.local_inventory.usb.install.is_some()
        {
            self.local_inventory.usb.delayed_mount = None;
            return;
        }
        if retry.next_ms.is_none_or(|due| now < due)
            || self.local_inventory.usb.remote.busy()
            || self.local_inventory.usb.waiting
            || self.local_inventory.usb.pending.is_some()
            || self.saving
            || !self.catalog_work_allowed()
            || !self.catalog_done.load(Ordering::Acquire)
            || !self.metadata_done.load(Ordering::Acquire)
            || self.resolver_busy
            || self.catalog_next.is_some()
            || self.catalog_loading
        {
            return;
        }
        let profile = self
            .local_inventory
            .usb
            .target
            .as_ref()
            .unwrap()
            .profile
            .clone();
        let Ok((target, task)) = self.usb_restore_target(&profile) else {
            self.local_inventory.usb.delayed_mount = None;
            return;
        };
        let mut retry = self.local_inventory.usb.delayed_mount.take().unwrap();
        let initial_restore = self.local_inventory.usb.initial_restore;
        retry.next_ms = None;
        // usb_begin_mode advances the existing generation and creates its usual
        // cancel token. Retain the original window/backoff across State replacement.
        if self.usb_begin_mode(task, target, true, true).is_ok() {
            self.local_inventory.usb.delayed_mount = Some(retry);
            self.local_inventory.usb.initial_restore = initial_restore;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_backoff_is_capped_without_renewing_the_original_window() {
        let mut retry = Retry::start(1_000);
        for (now, next) in [
            (1_000, 3_000),
            (3_000, 8_000),
            (8_000, 18_000),
            (90_000, 100_000),
        ] {
            retry.unavailable(now);
            assert_eq!(retry.next_ms, Some(next));
            assert_eq!(retry.deadline_ms, 121_000);
        }
    }
}
