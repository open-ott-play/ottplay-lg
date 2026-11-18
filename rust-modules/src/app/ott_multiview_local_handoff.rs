//! Some HLS gateways evict the least recently read client when a new channel
//! starts. After a decoder retires, let its surviving peers read naturally
//! before admitting another master on that gateway.
use super::super::{App, ProviderKind, Runtime};
use super::{Driver, LocalResolved, Picture};
use plx_data::ott::multiview::{AdmissionOutcome, ResolveTicket};
use plx_media::ff::ExternalSource;
use plx_media::local_multiview::{hls_handoff_barrier, hls_handoff_origin, Limits, Session};
use std::time::{Duration, Instant};

const WAIT_LIMIT: Duration = Duration::from_secs(30);
const CHECK_INTERVAL: Duration = Duration::from_millis(100);

enum PeerRead {
    Independent,
    Waiting,
    Completed(u64),
}

struct Fence {
    origin: String,
    sequence: u64,
    peers: Vec<ResolveTicket>,
}

struct Pending {
    ticket: ResolveTicket,
    receipt: LocalResolved,
    origin: String,
    deadline: Instant,
    next_check: Instant,
}

#[derive(Default)]
pub(super) struct Handoff {
    fences: Vec<Fence>,
    // Keep resolved inputs off the main thread's stack. At most one per slot.
    pending: [Option<Box<Pending>>; 9],
}

impl Handoff {
    pub(super) fn waiting(&self, slot: usize) -> bool {
        self.pending[slot].is_some()
    }

    pub(super) fn has_waiters(&self) -> bool {
        self.pending.iter().any(Option::is_some)
    }

    pub(super) fn hold(&mut self, ticket: ResolveTicket, receipt: LocalResolved, origin: String) {
        let now = Instant::now();
        let slot = ticket.slot().index();
        debug_assert!(!self.waiting(slot));
        self.pending[slot] = Some(Box::new(Pending {
            ticket,
            receipt,
            origin,
            deadline: now + WAIT_LIMIT,
            next_check: now + CHECK_INTERVAL,
        }));
    }

    fn remove_peer(&mut self, ticket: ResolveTicket) {
        for fence in &mut self.fences {
            fence.peers.retain(|peer| *peer != ticket);
        }
        self.fences.retain(|fence| !fence.peers.is_empty());
    }

    fn record(&mut self, origin: String, sequence: u64, peers: Vec<ResolveTicket>) {
        // The latest retirement supersedes every earlier fence for this origin.
        self.fences.retain(|fence| fence.origin != origin);
        if !peers.is_empty() {
            self.fences.push(Fence {
                origin,
                sequence,
                peers,
            });
        }
    }

    fn needs_read(
        &mut self,
        origin: &str,
        mut completed: impl FnMut(ResolveTicket) -> PeerRead,
    ) -> bool {
        let Some(index) = self.fences.iter().position(|f| f.origin == origin) else {
            return false;
        };
        let fence = &mut self.fences[index];
        fence.peers.retain(|p| match completed(*p) {
            PeerRead::Independent => false,
            PeerRead::Waiting => true,
            PeerRead::Completed(sequence) => sequence <= fence.sequence,
        });
        if !fence.peers.is_empty() {
            return true;
        }
        self.fences.swap_remove(index);
        false
    }
}

pub(super) fn direct_hls(input: &LocalResolved) -> Option<&str> {
    if input.input.profile().kind != ProviderKind::M3u
        || input.input.entry.duration_ms.is_some_and(|ms| ms > 0)
    {
        return None;
    }
    match &input.source {
        ExternalSource::Hls(url) => Some(url),
        _ => None,
    }
}

fn observed(picture: &Picture) -> Option<(String, u64)> {
    direct_hls(&picture.receipt)?;
    picture.decoder.hls_handoff_lease()
}

// A cancelled initial GET may have claimed a session before its response
// headers arrived. This provisional observation is only used on an origin for
// which another live reader has already established the gateway contract.
fn opening_on(picture: &Picture, origin: &str) -> bool {
    !picture.admitted
        && direct_hls(&picture.receipt)
            .and_then(hls_handoff_origin)
            .as_deref()
            == Some(origin)
}

impl Driver {
    fn retiring_origin(&self, picture: &Picture) -> Option<String> {
        if let Some((origin, _)) = observed(picture) {
            return Some(origin);
        }
        if picture.admitted {
            return None;
        }
        let origin = hls_handoff_origin(direct_hls(&picture.receipt)?)?;
        self.pictures
            .iter()
            .flatten()
            .any(|p| observed(p).is_some_and(|(o, _)| o == origin))
            .then_some(origin)
    }

    pub(super) fn retired_handoff(&mut self, retired: &Picture) {
        self.handoff.remove_peer(retired.ticket);
        if self.closing {
            return;
        }
        let Some(origin) = self.retiring_origin(retired) else {
            return;
        };
        // A completed request that began before this exact retirement cannot
        // establish that its server session is newer than the released one.
        let sequence = hls_handoff_barrier();
        let peers = self
            .pictures
            .iter()
            .flatten()
            .filter(|p| observed(p).map_or_else(|| opening_on(p, &origin), |(o, _)| o == origin))
            .map(|p| p.ticket)
            .collect();
        self.handoff.record(origin, sequence, peers);
    }

    pub(super) fn wait_for_handoff(&mut self, receipt: &LocalResolved) -> Option<String> {
        // Starting a cold grid never waits, and sources without a retired peer
        // or negotiated gateway never pay the cost of parsing the URL.
        if self.handoff.fences.is_empty()
            && !self.stopping.iter().any(Option::is_some)
            && !self
                .pictures
                .iter()
                .flatten()
                .any(|p| p.terminal || p.receipt.input.is_cancelled())
        {
            return None;
        }
        let origin = hls_handoff_origin(direct_hls(receipt)?)?;
        self.handoff_blocked(&origin).then_some(origin)
    }

    fn handoff_blocked(&mut self, origin: &str) -> bool {
        // A Stop in another slot can already be taking effect on this frame.
        // Do not let its successor open in the interval before worker retirement.
        if self.pictures.iter().flatten().any(|p| {
            (self.stopping[p.ticket.slot().index()].is_some()
                || p.terminal
                || p.receipt.input.is_cancelled())
                && self.retiring_origin(p).as_deref() == Some(origin)
        }) {
            return true;
        }
        self.handoff.needs_read(origin, |ticket| {
            let Some(p) = self.pictures[ticket.slot().index()]
                .as_ref()
                .filter(|p| p.ticket == ticket)
            else {
                return PeerRead::Waiting;
            };
            match observed(p) {
                Some((o, sequence)) if o == origin => PeerRead::Completed(sequence),
                None if opening_on(p, origin) => PeerRead::Waiting,
                // A provisional participant has now proved independent (for
                // example a progressive movie returned at a playlist URL).
                _ => PeerRead::Independent,
            }
        })
    }

    pub(super) fn poll_handoffs(&mut self, runtime: &mut Runtime, app: &App) {
        if !self.handoff.has_waiters() {
            return;
        }
        let now = Instant::now();
        for index in 0..self.handoff.pending.len() {
            let Some(mut wait) = self.handoff.pending[index].take() else {
                continue;
            };
            let expired = now >= wait.deadline;
            let cancelled = self.closing
                || self.cleanup_failed
                || self.stopping[index].is_some()
                || !wait.receipt.input.current_for(runtime);
            if !cancelled {
                if now >= wait.next_check || expired {
                    if !self.handoff_blocked(&wait.origin) {
                        self.media_ready_with(
                            runtime,
                            app,
                            wait.ticket,
                            Ok(wait.receipt),
                            |source| Session::open(source, Limits::default()),
                        );
                        continue;
                    }
                    wait.next_check = now + CHECK_INTERVAL;
                }
                if !expired {
                    self.handoff.pending[index] = Some(wait);
                    continue;
                }
                self.last_stop[index] = Some("provider_refresh_timeout");
            }
            self.retire_receipt(index, wait.receipt);
            let t = runtime
                .multiview
                .backend_start_completed(wait.ticket, AdmissionOutcome::Rejected);
            self.queue(runtime, t);
        }
    }
}

#[cfg(all(test, feature = "hostsim"))]
mod tests {
    use super::*;
    use plx_data::ott::multiview::{Layout, MediaRef, Scene, SceneId, SlotId, SourceRef};
    use std::num::NonZeroU64;

    fn ticket(slot: u8, id: u64) -> ResolveTicket {
        let token = NonZeroU64::new(id).unwrap();
        let mut scene = Scene::new(SceneId::from_opaque(NonZeroU64::MIN));
        scene.set_layout(Layout::Grid4).unwrap();
        let slot = SlotId::new(slot).unwrap();
        let source = SourceRef::from_opaque(token);
        scene.bind_source(slot, source).unwrap();
        scene
            .begin_resolution(slot, source, MediaRef::from_opaque(token))
            .unwrap()
    }

    #[test]
    fn waits_for_every_surviving_read_that_started_after_retirement() {
        let mut handoff = Handoff::default();
        let peers = [ticket(0, 11), ticket(1, 12), ticket(2, 13)];
        let mut reads = [70, 71, 69];
        let blocked = |state: &mut Handoff, origin, values: [u64; 3]| {
            state.needs_read(origin, |t| PeerRead::Completed(values[t.slot().index()]))
        };
        assert!(!blocked(&mut handoff, "gateway", reads));
        handoff.record("gateway".into(), 70, peers.to_vec());
        assert!(!blocked(&mut handoff, "another-port", reads));
        assert!(blocked(&mut handoff, "gateway", reads));
        reads[0] = 72;
        assert!(blocked(&mut handoff, "gateway", reads));
        reads[2] = 73;
        assert!(!blocked(&mut handoff, "gateway", reads));
        assert!(handoff.fences.is_empty());
    }

    #[test]
    fn retiring_a_second_window_raises_the_shared_fence_without_waiting_for_its_old_ticket() {
        let mut handoff = Handoff::default();
        let peers = [ticket(0, 10), ticket(1, 11), ticket(2, 12)];
        handoff.record("gateway".into(), 20, peers.to_vec());
        assert!(handoff.needs_read("gateway", |t| if t == peers[0] {
            PeerRead::Completed(21)
        } else {
            PeerRead::Waiting
        }));
        handoff.remove_peer(peers[1]);
        handoff.record("gateway".into(), 23, vec![peers[0], peers[2]]);
        // Even the first peer must make a fresh request after the second retirement.
        assert!(handoff.needs_read("gateway", |t| match t {
            t if t == peers[0] => PeerRead::Completed(21),
            t if t == peers[2] => PeerRead::Completed(24),
            _ => panic!("asked for a retired generation"),
        }));
        handoff.remove_peer(ticket(0, 30));
        assert!(handoff.needs_read("gateway", |_| PeerRead::Waiting));
        handoff.remove_peer(peers[0]);
        assert!(!handoff.needs_read("gateway", |_| panic!("no surviving peer remains")));
    }

    #[test]
    fn independent_progressive_vod_and_other_origins_do_not_wait() {
        let _serial = plx_base::testlock::serial();
        let (_app, _runtime, _snapshot, _, mut resolved) = super::super::lease_tests::receipt();
        let origin = hls_handoff_origin(direct_hls(&resolved).unwrap()).unwrap();
        let url = direct_hls(&resolved).unwrap().to_owned();
        let mut driver = Driver::default();
        assert!(driver.wait_for_handoff(&resolved).is_none());
        driver
            .handoff
            .record("https://other.invalid:443".into(), 70, vec![ticket(1, 13)]);
        assert!(driver.wait_for_handoff(&resolved).is_none());
        driver.handoff.record(origin, 71, vec![ticket(1, 13)]);
        assert!(driver.wait_for_handoff(&resolved).is_some());
        resolved.source = ExternalSource::Http(url.clone());
        assert!(driver.wait_for_handoff(&resolved).is_none());
        resolved.source = ExternalSource::Hls(url);
        resolved.input.entry.duration_ms = Some(60_000);
        assert!(driver.wait_for_handoff(&resolved).is_none());
    }

    #[test]
    fn resolved_target_waits_without_opening_and_cancellation_or_timeout_only_ends_that_attempt() {
        let _serial = plx_base::testlock::serial();
        for cancel in [true, false] {
            let (app, mut runtime, _snapshot, _, resolved) = super::super::lease_tests::receipt();
            let token = resolved.input.ticket;
            let origin = hls_handoff_origin(direct_hls(&resolved).unwrap()).unwrap();
            let signal = resolved.input.cancellation();
            let mut driver = Driver::default();
            driver
                .handoff
                .record(origin.clone(), 70, vec![ticket(1, 13)]);
            driver.media_ready_with(&mut runtime, &app, token, Ok(resolved), |_| {
                panic!("waiting for a peer must not open the new master")
            });
            let i = token.slot().index();
            assert!(driver.handoff.waiting(i) && !driver.slot_drained(i) && driver.has_work());
            if cancel {
                signal.store(true, std::sync::atomic::Ordering::Release);
            } else {
                driver.handoff.pending[i].as_mut().unwrap().deadline = Instant::now();
            }
            driver.poll_handoffs(&mut runtime, &app);
            assert!(driver.slot_drained(i) && driver.pictures.iter().all(Option::is_none));
            assert_eq!(
                driver.last_stop[i],
                (!cancel).then_some("provider_refresh_timeout")
            );
            assert!(driver.handoff.needs_read(&origin, |_| PeerRead::Waiting));
        }
    }
}
