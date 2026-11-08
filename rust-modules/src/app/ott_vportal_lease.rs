//! Application-owned lifetime of a validated NAS playback session.
//!
//! Only the VPortal response parser may supply URLs: it checks the exact NAS
//! origin and signed media path. The frame thread owns a Lease, never a request.
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

pub(super) struct Lease {
    release: Sender<()>,
    finished: Receiver<()>,
}

impl Lease {
    #[cfg(test)]
    pub(super) fn pending_for_test() -> (Self, Receiver<()>, Sender<()>) {
        let (release, receiver) = mpsc::channel();
        let (done, finished) = mpsc::channel();
        (Self { release, finished }, receiver, done)
    }

    /// Called by the resolver worker, after validating both lifecycle URLs.
    /// If worker creation fails, release immediately on that resolver worker.
    pub fn start(stop: String, heartbeat: Option<String>) -> Result<Self, &'static str> {
        Self::start_deferred(stop, heartbeat).map_err(|cleanup| {
            request(&cleanup);
            "Playback session worker could not start"
        })
    }
    fn start_deferred(stop: String, heartbeat: Option<String>) -> Result<Self, String> {
        let (release, receiver) = mpsc::channel();
        let (done, finished) = mpsc::channel();
        let cleanup = stop.clone();
        if !plx_base::task::spawn_off_frame("ott-vportal-session", move |_| {
            run(&stop, heartbeat.as_deref(), &receiver, request);
            let _ = done.send(());
        }) {
            return Err(cleanup);
        }
        Ok(Self { release, finished })
    }

    /// Use before resolving a replacement on a worker. Waiting for the previous
    /// stop avoids consuming two simultaneous server-side transcode sessions.
    /// Never call from the frame thread; ordinary Drop is deliberately nonblocking.
    pub fn finish_off_frame(self) -> Result<(), &'static str> {
        self.begin_release();
        // A heartbeat may already be inside its five-second request, followed by
        // the stop request's own five-second bound. No unbounded join is needed.
        self.finished
            .recv_timeout(Duration::from_secs(11))
            .map_err(|_| "Previous playback session did not finish closing")
    }

    /// The frame loop has ended. The existing session worker performs the stop;
    /// this only waits for that attempt within the caller's remaining allowance.
    /// Completion means the request settled, not that an unresponsive server
    /// confirmed releasing its resource.
    pub fn finish_after_loop(self, allowance: Duration) -> Result<(), &'static str> {
        self.begin_release();
        match self.finished.try_recv() {
            Ok(()) => return Ok(()),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err("Previous playback session did not finish closing");
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        match plx_base::task::time::drain_for(
            &self.finished,
            allowance.min(Duration::from_secs(11)),
            |_| false,
        ) {
            plx_base::task::time::DrainResult::Complete => Ok(()),
            _ => Err("Previous playback session did not finish closing"),
        }
    }

    /// Stop producing media immediately during an EOS scan gap or suspension,
    /// retaining this lease only as the next resolver's completion barrier.
    pub fn begin_release(&self) {
        let _ = self.release.send(());
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        // Unbounded channel send never waits for a network operation.
        self.begin_release();
    }
}

fn request(url: &str) {
    // No redirects: the validated session URL cannot grant another origin GETs.
    let _ = plx_net::net::request(
        url,
        &[],
        "GET",
        None,
        plx_net::net::Timeouts {
            connect_s: 3,
            total_s: 5,
            total_ms: 0,
            low_speed_bps: 1,
            low_speed_s: 3,
        },
        false,
        Some(4096),
        None,
    );
}

fn run(stop: &str, heartbeat: Option<&str>, receiver: &Receiver<()>, get: impl FnMut(&str)) {
    run_with(
        stop,
        heartbeat,
        || receiver.recv_timeout(HEARTBEAT_INTERVAL),
        get,
    );
}

fn run_with(
    stop: &str,
    heartbeat: Option<&str>,
    mut receive: impl FnMut() -> Result<(), RecvTimeoutError>,
    mut get: impl FnMut(&str),
) {
    loop {
        match receive() {
            Err(RecvTimeoutError::Timeout) => {
                if let Some(url) = heartbeat {
                    get(url);
                }
            }
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                get(stop);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_is_serial_and_release_runs_once() {
        let mut events = [
            Err(RecvTimeoutError::Timeout),
            Err(RecvTimeoutError::Timeout),
            Ok(()),
        ]
        .into_iter();
        let mut requests = Vec::new();
        run_with(
            "stop",
            Some("heartbeat"),
            || events.next().unwrap(),
            |url| requests.push(url.to_owned()),
        );
        assert_eq!(requests, ["heartbeat", "heartbeat", "stop"]);
        assert_eq!(HEARTBEAT_INTERVAL, Duration::from_secs(30));
    }

    #[test]
    fn drop_without_a_heartbeat_releases_on_disconnect() {
        let mut requests = Vec::new();
        run_with(
            "stop",
            None,
            || Err(RecvTimeoutError::Disconnected),
            |url| requests.push(url.to_owned()),
        );
        assert_eq!(requests, ["stop"]);
    }

    #[test]
    fn dropping_ownership_only_enqueues_cleanup() {
        let (release, receiver) = mpsc::channel();
        let (_done, finished) = mpsc::channel();
        drop(Lease { release, finished });
        assert_eq!(receiver.try_recv(), Ok(()));
        assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    }

    #[test]
    fn replacement_waits_for_stop_completion() {
        let (release, receiver) = mpsc::channel();
        let (done, finished) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            receiver.recv().unwrap();
            done.send(()).unwrap();
        });
        assert!(Lease { release, finished }.finish_off_frame().is_ok());
        worker.join().unwrap();
    }

    #[test]
    fn post_loop_wait_accepts_completed_cleanup_with_no_remaining_allowance() {
        let (lease, release, done) = Lease::pending_for_test();
        done.send(()).unwrap();
        assert!(lease.finish_after_loop(Duration::ZERO).is_ok());
        assert_eq!(release.try_recv(), Ok(()));
    }

    #[test]
    fn post_loop_wait_does_not_extend_an_exhausted_allowance() {
        let (lease, release, _done) = Lease::pending_for_test();
        assert!(lease.finish_after_loop(Duration::ZERO).is_err());
        assert_eq!(release.try_recv(), Ok(()));
    }
}

/// One ownership slot per already-deduplicated validated stop identity.
pub(super) struct LeaseSet { leases: Vec<Option<Lease>> }
impl LeaseSet {
    #[cfg(all(test, feature = "hostsim"))]
    pub(super) fn for_test(leases: Vec<Lease>) -> Self {
        Self { leases: leases.into_iter().map(Some).collect() }
    }
    pub(super) fn adopt(worker: &plx_base::task::OffFrame, budget: plx_base::task::time::Budget, sessions: Vec<(String, Option<String>)>) -> Result<Self, &'static str> {
        let mut leases = Vec::with_capacity(sessions.len());
        let mut fallback = Vec::new();
        for (stop, heartbeat) in sessions {
            if !fallback.is_empty() { fallback.push(stop); leases.push(None); continue; }
            match Lease::start_deferred(stop, heartbeat) {
                Ok(lease) => leases.push(Some(lease)),
                Err(stop) => { fallback.push(stop); leases.push(None); }
            }
        }
        let result = Self { leases };
        if !fallback.is_empty() {
            let live = result.into_leases();
            for lease in &live { lease.begin_release(); }
            let attempted_all = fallback_requests(&fallback, || budget.remaining(worker), request_bounded);
            let finished = finish_many_before(worker, live, budget).is_ok();
            return Err(if attempted_all && finished { "Playback session worker could not start" }
                else { "VPortal session cleanup did not finish within its allowance" });
        }
        Ok(result)
    }
    pub(super) fn take(&mut self, index: usize) -> Option<Lease> { self.leases.get_mut(index).and_then(Option::take) }
    pub(super) fn into_leases(self) -> Vec<Lease> { self.leases.into_iter().flatten().collect() }
    pub(super) fn finish_before(self, worker: &plx_base::task::OffFrame, budget: plx_base::task::time::Budget) -> Result<(), &'static str> {
        finish_many_before(worker, self.into_leases(), budget)
    }
}
pub(super) fn finish_many(worker: &plx_base::task::OffFrame, leases: Vec<Lease>, allowance: Duration) -> Result<(), &'static str> {
    finish_many_before(worker, leases, plx_base::task::time::Budget::start(worker, allowance))
}
pub(super) fn finish_many_before(worker: &plx_base::task::OffFrame, leases: Vec<Lease>, budget: plx_base::task::time::Budget) -> Result<(), &'static str> {
    for lease in &leases { lease.begin_release(); }
    let mut failure = None;
    for lease in leases {
        if let Err(error) = lease.finish_after_loop(budget.remaining(worker)) { failure.get_or_insert(error); }
    }
    failure.map_or(Ok(()), Err)
}
fn fallback_requests(stops: &[String], mut remaining: impl FnMut() -> Duration,
    mut send: impl FnMut(&str, Duration)) -> bool {
    for stop in stops {
        let allowance = remaining().min(Duration::from_secs(5));
        if allowance.as_millis() == 0 { return false; }
        send(stop, allowance);
    }
    true
}
fn request_bounded(url: &str, allowance: Duration) {
    let millis = allowance.as_millis().min(5000);
    if millis == 0 { return; }
    let _ = plx_net::net::request(url, &[], "GET", None, plx_net::net::Timeouts {
        connect_s: 3, total_s: 0, total_ms: millis as _, low_speed_bps: 1, low_speed_s: 3,
    }, false, Some(4096), None);
}
#[cfg(test)]
mod set_tests {
    use super::*;
    #[test]
    fn zero_budget_drains_every_completed_slot_and_releases_all() {
        let worker = plx_base::task::OffFrame::for_test();
        let (a, ar, ad) = Lease::pending_for_test(); let (b, br, bd) = Lease::pending_for_test();
        ad.send(()).unwrap(); bd.send(()).unwrap();
        assert!(finish_many(&worker, vec![a,b], Duration::ZERO).is_ok());
        assert_eq!(ar.try_recv(), Ok(())); assert_eq!(br.try_recv(), Ok(()));
    }
    #[test]
    fn an_unsettled_slot_fails_without_skipping_release_of_later_slots() {
        let worker = plx_base::task::OffFrame::for_test();
        let (a, ar, _ad) = Lease::pending_for_test(); let (b, br, bd) = Lease::pending_for_test(); bd.send(()).unwrap();
        assert!(finish_many(&worker, vec![a,b], Duration::ZERO).is_err());
        assert_eq!(ar.try_recv(), Ok(())); assert_eq!(br.try_recv(), Ok(()));
    }
    #[test]
    fn selecting_one_slot_moves_it_once_and_keeps_other_cleanup_owned() {
        let (a, ar, _ad) = Lease::pending_for_test(); let (b, br, _bd) = Lease::pending_for_test();
        let mut set = LeaseSet { leases: vec![Some(a),Some(b)] };
        let chosen = set.take(1).unwrap(); assert!(set.take(1).is_none());
        drop(set); assert_eq!(ar.try_recv(), Ok(())); assert!(br.try_recv().is_err());
        drop(chosen); assert_eq!(br.try_recv(), Ok(()));
    }
    #[test]
    fn spawn_failure_fallbacks_spend_one_allowance_and_never_restart_it() {
        let stops = vec!["first".into(),"second".into(),"third".into()];
        let mut remaining = [Duration::from_secs(9),Duration::from_secs(2),Duration::ZERO].into_iter();
        let mut calls = Vec::new();
        assert!(!fallback_requests(&stops, || remaining.next().unwrap(), |stop, allowance| calls.push((stop.to_owned(), allowance))));
        assert_eq!(calls, [("first".into(),Duration::from_secs(5)),("second".into(),Duration::from_secs(2))]);
    }
    #[test]
    fn sub_millisecond_budget_is_incomplete_without_claiming_a_request() {
        let stops = vec!["pending".into()];
        let mut calls = 0;
        assert!(!fallback_requests(&stops, || Duration::from_nanos(999_999), |_,_| calls += 1));
        assert_eq!(calls, 0);
    }

}
