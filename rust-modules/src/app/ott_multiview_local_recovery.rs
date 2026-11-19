//! Reacquire a live HLS source after an explicit temporary outage or provider
//! session loss. A pending restart holds no playable URL or authority.
use super::super::{multiview, multiview_sources::SourceHandle, App, Runtime};
use super::{handoff::direct_hls, Driver, Picture};
use plx_data::ott::multiview::{ResolveTicket, SceneId, SlotId};
use plx_media::local_multiview::{Error as MediaError, Event};
use std::time::Duration;

const WINDOW: Duration = Duration::from_secs(5 * 60);
const COLD_DELAY: Duration = Duration::from_secs(8);
const FOLLOWUP_DELAY: Duration = Duration::from_secs(25);
const FOLLOWUP_LIMIT: Duration = Duration::from_secs(90);

struct Proof {
    original: ResolveTicket,
    handle: SourceHandle,
    url_hash: String,
    lost: Duration,
}

#[derive(Default)]
pub(super) struct Budget {
    attempts: [Option<Duration>; 2],
    proof: Option<Box<Proof>>,
    next: Option<(ResolveTicket, SourceHandle, Duration, Duration)>,
}

impl Budget {
    pub(super) fn schedule(
        &mut self,
        ticket: ResolveTicket,
        handle: SourceHandle,
        now: Duration,
    ) -> bool {
        self.enqueue(ticket, handle, now, now, now + FOLLOWUP_LIMIT)
    }

    fn enqueue(
        &mut self,
        ticket: ResolveTicket,
        handle: SourceHandle,
        now: Duration,
        earliest: Duration,
        expires: Duration,
    ) -> bool {
        if self.next.is_some() {
            return false;
        }
        let recent = self
            .attempts
            .iter()
            .flatten()
            .filter(|t| now.saturating_sub(**t) < WINDOW)
            .count();
        if recent >= self.attempts.len() {
            return false;
        }
        self.attempts.rotate_left(1);
        self.attempts[1] = Some(now);
        let delay = if recent == 0 {
            Duration::from_millis(400)
        } else {
            Duration::from_secs(2)
        };
        self.next = Some((ticket, handle, (now + delay).max(earliest), expires));
        true
    }

    pub(super) fn resumed(&mut self) {
        self.proof = None;
    }

    fn confirmed_loss(
        &mut self,
        ticket: ResolveTicket,
        handle: SourceHandle,
        url: &str,
        now: Duration,
    ) -> bool {
        if !self.schedule(ticket, handle, now) {
            return false;
        }
        self.proof = Some(Box::new(Proof {
            original: ticket,
            handle,
            url_hash: plx_data::ott::favorites_wire::hash(url.as_bytes()),
            lost: now,
        }));
        true
    }

    fn startup_outcome(
        &mut self,
        error: MediaError,
        ticket: ResolveTicket,
        handle: SourceHandle,
        url: &str,
        now: Duration,
    ) -> bool {
        let Some(proof) = self.proof.take() else {
            // One cold restart per selection, and only for a transport-classified
            // outage. Stagger windows recovering from the same gateway outage.
            let delay = COLD_DELAY + Duration::from_millis(400 * ticket.slot().index() as u64);
            return error == MediaError::SourceTemporarilyUnavailable
                && self.attempts.iter().all(Option::is_none)
                && self.enqueue(ticket, handle, now, now + delay, now + FOLLOWUP_LIMIT);
        };
        matches!(
            error,
            MediaError::SourceTemporarilyUnavailable | MediaError::SourceInterrupted
        ) && ticket.scene() == proof.original.scene()
            && ticket.slot() == proof.original.slot()
            && ticket != proof.original
            && handle == proof.handle
            && now < proof.lost + FOLLOWUP_LIMIT
            && proof.url_hash == plx_data::ott::favorites_wire::hash(url.as_bytes())
            && self.enqueue(
                ticket,
                handle,
                now,
                proof.lost + FOLLOWUP_DELAY,
                proof.lost + FOLLOWUP_LIMIT,
            )
    }

    pub(super) fn on_failure(
        &mut self,
        event: &Event,
        picture: &Picture,
        runtime: &Runtime,
    ) -> bool {
        let Some(url) = direct_hls(&picture.receipt) else {
            self.resumed();
            return false;
        };
        let Some(handle) = runtime.multiview.live_recovery_source(picture.ticket) else {
            self.resumed();
            return false;
        };
        let now = runtime.frame_time();
        match event {
            Event::Failure(MediaError::LostProviderLease)
                if picture.admitted && picture.decoder.hls_handoff().is_some() =>
            {
                self.confirmed_loss(picture.ticket, handle, url, now)
            }
            Event::Failure(MediaError::SourceTemporarilyUnavailable) if picture.admitted => {
                self.schedule(picture.ticket, handle, now)
            }
            Event::Failure(error) if !picture.admitted => {
                self.startup_outcome(*error, picture.ticket, handle, url, now)
            }
            _ => {
                self.resumed();
                false
            }
        }
    }

    pub(super) fn pending(&self) -> bool {
        self.next.is_some()
    }

    pub(super) fn take_due(&mut self, now: Duration) -> Option<(ResolveTicket, SourceHandle)> {
        let (ticket, handle, after, expires) = self.next?;
        if now >= expires {
            self.next = None;
            self.resumed();
            return None;
        }
        if now < after {
            return None;
        }
        self.next = None;
        Some((ticket, handle))
    }
}

impl Driver {
    pub(in crate::app::ott_runtime) fn recovering(&self, scene: SceneId, slot: SlotId) -> bool {
        self.recovery[slot.index()].next.is_some_and(|(ticket, ..)| ticket.scene() == scene)
    }

    pub(super) fn recover_drained_slot(
        &mut self,
        runtime: &mut Runtime,
        app: &App,
        slot: usize,
        now: Duration,
    ) {
        let Some((ticket, handle)) = self.recovery[slot].take_due(now) else {
            return;
        };
        // Rebind the original opaque handle. Metadata, parental authorization and
        // the gateway handoff run again for the new ticket.
        if !self.closing
            && !self.cleanup_failed
            && runtime.multiview.ready_after_retirement(ticket)
            && runtime
                .multiview
                .validate_active_owner(runtime, app)
                .is_ok()
            && self
                .start_source(runtime, app, ticket.slot(), handle)
                .is_ok()
        {
            return;
        }
        self.recovery[slot].resumed();
    }

    pub(in crate::app::ott_runtime) fn stop_slot(
        &mut self,
        runtime: &mut Runtime,
        slot: SlotId,
    ) -> Result<(), multiview::Error> {
        let i = slot.index();
        let recovering = self.recovery[i].pending() || self.recovery[i].proof.is_some();
        match runtime.multiview.close_slot(slot) {
            Ok(t) => runtime.multiview_effects.extend(t.effects),
            Err(multiview::Error::BackendPending) if recovering => {}
            Err(e) => return Err(e),
        }
        // A real user Stop supersedes a pending same-source recovery or replacement.
        self.recovery[i] = Default::default();
        self.replacements[i] = None;
        Ok(())
    }
}

#[cfg(all(test, feature = "hostsim"))]
mod tests {
    use super::*;
    use plx_data::ott::multiview::{Layout, Scene};

    fn test_tickets(
        handle: SourceHandle,
        original: ResolveTicket,
    ) -> (ResolveTicket, ResolveTicket) {
        let mut scene = Scene::new(original.scene());
        let slot = original.slot();
        scene.set_layout(Layout::Grid4).unwrap();
        scene.bind_source(slot, handle.source_ref()).unwrap();
        let first = scene
            .begin_resolution(slot, handle.source_ref(), handle.media_ref())
            .unwrap();
        scene.report_pipeline_stopped(first).unwrap();
        scene.bind_source(slot, handle.source_ref()).unwrap();
        let second = scene
            .begin_resolution(slot, handle.source_ref(), handle.media_ref())
            .unwrap();
        assert_ne!(first, second);
        (first, second)
    }

    #[test]
    fn confirmed_loss_can_bridge_a_48_second_provider_outage_on_its_new_authorized_generation() {
        let _serial = plx_base::testlock::serial();
        let (_, _, _, handle, original) = super::super::lease_tests::receipt();
        let (first, fresh) = test_tickets(handle, original.input.ticket);
        let start = Duration::ZERO;
        let lost = start + Duration::from_secs(28);
        let url = "https://fixture.invalid:443/channel.m3u8";
        let mut budget = Budget::default();
        assert!(budget.confirmed_loss(first, handle, url, lost));
        assert_eq!(
            budget.take_due(lost + Duration::from_secs(1)),
            Some((first, handle))
        );
        assert!(budget.startup_outcome(
            MediaError::SourceTemporarilyUnavailable,
            fresh,
            handle,
            url,
            lost + Duration::from_secs(2)
        ));
        assert!(budget.take_due(start + Duration::from_secs(48)).is_none());
        assert_eq!(
            budget.take_due(start + Duration::from_secs(53)),
            Some((fresh, handle))
        );
        assert!(!budget.startup_outcome(
            MediaError::SourceTemporarilyUnavailable,
            fresh,
            handle,
            url,
            start + Duration::from_secs(55)
        ));
    }

    #[test]
    fn cold_temporary_outage_allows_one_delayed_restart_per_selection() {
        let _serial = plx_base::testlock::serial();
        let (_, _, _, handle, original) = super::super::lease_tests::receipt();
        let (first, fresh) = test_tickets(handle, original.input.ticket);
        let url = "https://fixture.invalid:443/channel.m3u8";
        let now = Duration::ZERO;
        for error in [MediaError::SourceDenied, MediaError::Source, MediaError::Playlist,
            MediaError::SourceInterrupted, MediaError::LostProviderLease, MediaError::Cancelled] {
            let mut budget = Budget::default();
            assert!(!budget.startup_outcome(error, first, handle, url, now));
            assert!(!budget.pending());
        }
        let mut budget = Budget::default();
        assert!(budget.startup_outcome(MediaError::SourceTemporarilyUnavailable, first, handle, url, now));
        assert!(budget.take_due(now + Duration::from_secs(1)).is_none());
        assert_eq!(budget.take_due(now + COLD_DELAY), Some((first, handle)));
        assert!(!budget.startup_outcome(MediaError::SourceTemporarilyUnavailable, fresh, handle, url,
            now + COLD_DELAY + Duration::from_secs(1)));
        assert!(!budget.pending());
    }

    #[test]
    fn confirmed_session_followup_rejects_changed_urls_denial_and_expired_ownership() {
        let _serial = plx_base::testlock::serial();
        let (_, _, _, handle, original) = super::super::lease_tests::receipt();
        let (first, fresh) = test_tickets(handle, original.input.ticket);
        let url = "https://fixture.invalid:443/channel.m3u8";
        let now = Duration::ZERO;
        for (error, target, identity, elapsed) in [
            (MediaError::SourceDenied, fresh, url, 2),
            (MediaError::Source, fresh, url, 2),
            (MediaError::Playlist, fresh, url, 2),
            (MediaError::SourceInterrupted, first, url, 2),
            (
                MediaError::SourceTemporarilyUnavailable,
                fresh,
                "https://different.invalid/channel.m3u8",
                2,
            ),
            (MediaError::SourceInterrupted, fresh, url, 91),
        ] {
            let mut budget = Budget::default();
            assert!(budget.confirmed_loss(first, handle, url, now));
            budget.take_due(now + Duration::from_secs(1)).unwrap();
            assert!(!budget.startup_outcome(
                error,
                target,
                handle,
                identity,
                now + Duration::from_secs(elapsed)
            ));
            assert!(!budget.pending());
            assert!(budget.proof.is_none());
        }
    }
}
