//! Pure playback policy for an incrementally discovered, ordered catalogue.
//!
//! The runtime supplies its filtered order, persists resume points and executes
//! Start through its normal generation-fenced resolver. Calling `user_selected`
//! is reserved for viewer/remote intent, not Starts emitted by this policy.
use std::collections::HashSet;

/// One isolated bad item skips immediately; repeated failures yield to input.
/// A fifth consecutive failure stops automatic attempts, regardless of catalogue
/// size. Explicit viewer actions or an accepted EOF start a fresh budget.
const FAILURE_DELAYS_MS: [u64; 4] = [0, 1_000, 2_000, 4_000];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailedContinuation {
    None,
    Discovery,
    Cooldown { not_before_ms: u64 },
    SameCooldown { not_before_ms: u64 },
    Halted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResumePoint {
    pub entry_id: String,
    pub position_ms: u64,
}

/// An automatic seek may first present the preceding keyframe. That preroll
/// must not replace the bookmark which requested the seek. The runtime binds
/// this policy to one admitted playback identity and retires it on viewer seek.
pub(super) struct ResumeFloor {
    position_ms: u64,
}

impl ResumeFloor {
    pub fn new(position_ms: u64) -> Option<Self> {
        (position_ms > 0).then_some(Self { position_ms })
    }

    pub fn checkpoint(&mut self, position_ms: u64, observed_ms: Option<u64>, duration_ms: Option<u64>) -> u64 {
        if let Some(duration_ms) = duration_ms.filter(|duration| *duration > 0) {
            self.position_ms = self.position_ms.min(duration_ms);
        }
        let floor_ms = self.position_ms;
        // The initial seek seeds the published position with its target before
        // the first callback. Equality is not evidence that preroll has ended.
        if observed_ms.is_some_and(|position| position > self.position_ms) {
            self.position_ms = 0;
        }
        // The checkpoint sample can precede the observation which discharged
        // the floor. Apply this call's floor even when future calls may regress.
        position_ms.max(floor_ms)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DeferredStart {
    point: ResumePoint,
    reason: Reason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reason {
    First,
    Restore,
    Next,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Decision {
    Start {
        entry_id: String,
        position_ms: u64,
        reason: Reason,
    },
    /// Await discovery or a failed-start cooldown. Do not replay the old item.
    Wait,
    Idle,
}

/// Borrowed policy facts for the App's redacted transition observer. No IO or clock reads.
pub(super) struct PendingRetry<'a> {
    pub generation: u64,
    pub after: &'a str,
    pub failures: usize,
    pub not_before_ms: u64,
}

pub(super) struct Continuity {
    generation: u64,
    restore: Option<ResumePoint>,
    issued_restore: Option<ResumePoint>,
    deferred: Option<DeferredStart>,
    current: Option<String>,
    waiting_after: Option<String>,
    failed: HashSet<String>,
    consecutive_failures: usize,
    failed_continuation: FailedContinuation,
    complete: bool,
    enabled: bool,
    suspended: bool,
    explicitly_stopped: bool,
}

impl Continuity {
    pub fn new(generation: u64, restore: Option<ResumePoint>) -> Self {
        Self {
            generation,
            restore,
            issued_restore: None,
            deferred: None,
            current: None,
            waiting_after: None,
            failed: HashSet::new(),
            consecutive_failures: 0,
            failed_continuation: FailedContinuation::None,
            complete: false,
            enabled: true,
            suspended: false,
            explicitly_stopped: false,
        }
    }

    /// A background refresh changes the catalogue generation, not viewer intent.
    /// Keep Stop, pause, selection and failures; only the new scan can grant wrap.
    pub fn rebase(&mut self, generation: u64) {
        self.cancel_failed_continuation();
        self.generation = generation;
        self.complete = false;
    }

    /// A durable selection-domain transition can replace a previously complete
    /// Favorites list with a still-growing folder. Only its completion fact is
    /// replaced; current selection, Stop/pause and failure cooldown stay owned.
    pub fn selection_domain_changed(&mut self, generation: u64, complete: bool) {
        if generation == self.generation {
            self.complete = complete;
        }
    }

    /// Loading data without a current activation intent must not turn an idle
    /// catalogue into automatic playback. Preserve an existing selection,
    /// pending restore, or awaited successor: those carry earlier valid intent.
    pub fn suppress_unrequested_start(&mut self) {
        if self.current.is_none() && self.restore.is_none() && self.deferred.is_none()
            && self.waiting_after.is_none() {
            self.stop(self.generation);
        }
    }

    /// A temporary first-file fallback must not overwrite this durable target.
    /// It disappears after an admitted restore, scan completion, or viewer intent.
    /// A restore waiting for its source is still a pending durable target.
    pub fn pending_restore(&self) -> Option<&ResumePoint> {
        self.restore.as_ref().or_else(|| {
            self.deferred.as_ref()
                .filter(|pending| pending.reason == Reason::Restore)
                .map(|pending| &pending.point)
        })
    }

    /// The resolver may be parked and reissued across suspend. Keep the exact
    /// automatic origin until admission, independently of its worker generation.
    pub fn issued_restore(&self, generation: u64, id: &str, position_ms: u64) -> bool {
        generation == self.generation && self.current.as_deref() == Some(id)
            && self.issued_restore.as_ref().is_some_and(|point|
                point.entry_id == id && point.position_ms == position_ms)
    }

    pub fn restore_admitted(&mut self, generation: u64, id: &str, position_ms: u64) {
        if self.issued_restore(generation, id, position_ms) {
            self.issued_restore = None;
        }
    }

    /// The policy has selected this exact target, but its source capability is
    /// not ready yet. Waiting is neither an admission nor a failed attempt.
    pub fn defer_start(
        &mut self, generation: u64, entry_id: &str, position_ms: u64, reason: Reason,
    ) -> bool {
        if !self.accepts(generation) || self.current.as_deref() != Some(entry_id) {
            return false;
        }
        let pending = DeferredStart {
            point: ResumePoint { entry_id: entry_id.into(), position_ms },
            reason,
        };
        if self.deferred.as_ref().is_some_and(|old| old != &pending) { return false; }
        self.deferred = Some(pending);
        true
    }

    /// A cheap wake hint. The runner still supplies the new ordered membership
    /// and source readiness before retrying this target through ordinary play.
    /// This is an issued Start, not a failed-start cooldown; suspension preserves
    /// its target and failure budget without renewing either.
    pub fn has_deferred_start(&self, generation: u64) -> bool {
        self.accepts(generation) && self.deferred.is_some()
    }

    /// `ids` is the complete discovered order so far, never merely a new page.
    pub fn catalog_updated(&mut self, generation: u64, ids: &[&str], complete: bool) -> Decision {
        if !self.accepts(generation) {
            return Decision::Idle;
        }
        self.complete |= complete;
        // A newly discovered bookmark is an automatic start too. Neither that
        // path nor another catalogue page may bypass the failed-start budget.
        match self.failed_continuation {
            FailedContinuation::Cooldown { .. } | FailedContinuation::SameCooldown { .. } => return Decision::Wait,
            FailedContinuation::Halted => return Decision::Idle,
            _ => {}
        }
        if let Some(saved) = self.restore.as_ref() {
            if ids.contains(&saved.entry_id.as_str()) && !self.failed.contains(&saved.entry_id) {
                let saved = self.restore.take().expect("resume point was present");
                return self.start(&saved.entry_id, saved.position_ms, Reason::Restore);
            }
            if self.complete {
                self.restore = None;
            }
        }
        if let Some(pending) = self.deferred.as_ref() {
            if ids.contains(&pending.point.entry_id.as_str())
                && !self.failed.contains(&pending.point.entry_id) {
                let pending = self.deferred.take().expect("deferred start was present");
                return self.start(&pending.point.entry_id, pending.point.position_ms, pending.reason);
            }
            if self.complete {
                // A complete replacement has removed the exact target. Do not
                // reinterpret its old ordinal as a different user's selection.
                self.stop(generation);
                return Decision::Idle;
            }
            return Decision::Wait;
        }
        if let Some(after) = self.waiting_after.clone() {
            return self.advance(&after, ids);
        }
        if self.current.is_none() {
            if let Some(id) = ids.iter().find(|id| !self.failed.contains(**id)) {
                return self.start(id, 0, Reason::First);
            }
            return self.exhausted();
        }
        Decision::Idle
    }

    /// Initial native admission of a kiosk fallback preserves the pending saved
    /// target. Only the successful admission caller may use this, not user input.
    pub fn initial_admitted(&mut self, generation: u64, id: &str) {
        if generation == self.generation && self.current.is_none() {
            self.current = Some(id.into());
        }
    }

    /// A durable source change retires only its pending restore. Preserve Stop,
    /// Pause and ordinary EOF policy; an in-flight cancellation supplies the
    /// actually retained item without turning that into new viewer intent.
    pub fn cancel_restore(&mut self, generation: u64, retained: Option<&str>) {
        if generation != self.generation { return; }
        self.restore = None;
        self.issued_restore = None;
        if self.deferred.as_ref().is_some_and(|pending| pending.reason == Reason::Restore) {
            self.deferred = None;
            self.current = retained.map(str::to_owned);
        }
        if let Some(id) = retained { self.current = Some(id.into()); }
    }

    /// A user selection also prevents a later scan result from stealing playback.
    pub fn user_selected(&mut self, generation: u64, id: &str) {
        if generation != self.generation {
            return;
        }
        self.consecutive_failures = 0;
        self.retain_selection(generation, id);
    }

    /// Restore a still-owned selection after a superseded/failed request. This
    /// is not fresh viewer intent and must not renew the failure budget.
    pub fn retain_selection(&mut self, generation: u64, id: &str) {
        if generation != self.generation {
            return;
        }
        self.enabled = true;
        self.explicitly_stopped = false;
        self.restore = None;
        self.issued_restore = None;
        self.deferred = None;
        self.waiting_after = None;
        self.failed_continuation = FailedContinuation::None;
        self.failed.remove(id);
        self.current = Some(id.into());
    }

    /// Call on Stop/Back or a replacement source while discovery continues.
    /// An explicit subsequent selection re-enables ordinary EOS progression.
    pub fn stop(&mut self, generation: u64) {
        if generation != self.generation {
            return;
        }
        self.enabled = false;
        self.explicitly_stopped = true;
        self.suspended = false;
        self.restore = None;
        self.issued_restore = None;
        self.deferred = None;
        self.waiting_after = None;
        self.current = None;
        self.failed_continuation = FailedContinuation::None;
    }

    /// Pausing revokes late restoration but retains committed playback ownership.
    pub fn pause(&mut self, generation: u64) {
        if generation != self.generation {
            return;
        }
        self.enabled = false;
        self.restore = None;
        self.issued_restore = None;
        self.deferred = None;
        self.cancel_failed_continuation();
    }

    /// A window/standby pause retains the pending saved-file restore and the
    /// viewer's enabled/paused intent. It does not become a viewer Pause.
    pub fn suspend(&mut self, generation: u64) {
        if generation == self.generation {
            self.suspended = true;
            self.cancel_failed_continuation();
        }
    }

    fn cancel_failed_continuation(&mut self) {
        if matches!(self.failed_continuation,
            FailedContinuation::Discovery | FailedContinuation::Cooldown { .. } | FailedContinuation::SameCooldown { .. }) {
            // A later catalogue page or wake must not recreate the cancelled
            // attempt. Preserve the budget and any unrelated bookmark data.
            self.failed_continuation = FailedContinuation::Halted;
            self.waiting_after = None;
        }
    }

    pub fn resume_after_suspend(&mut self, generation: u64) {
        if generation == self.generation { self.suspended = false; }
    }

    // Pure state-machine tests exercise resume without a viewer selection.
    // Runtime transport commits user_selected; OS wake uses resume_after_suspend.
    #[cfg(test)]
    pub fn resume(&mut self, generation: u64) {
        if generation != self.generation || self.explicitly_stopped {
            return;
        }
        self.enabled = true;
    }

    /// An ignored EOF must not tear down a paused or superseded current item.
    /// An accepted EOF may still return Idle when no successor is available.
    pub fn accepts_end(&self, generation: u64, id: &str) -> bool {
        self.accepts(generation) && self.current.as_deref() == Some(id)
            && self.deferred.is_none()
            && self.failed_continuation == FailedContinuation::None
            && !self.failed.contains(id)
    }

    pub fn ended(&mut self, generation: u64, id: &str, ids: &[&str], complete: bool) -> Decision {
        self.ended_at(generation, id, id, ids, complete)
    }

    /// The owner may prove that the installed selection is a row in a different
    /// traversal domain. Keep EOF ownership on the installed ID; only successor
    /// position and a discovery wait use that domain's canonical row ID.
    pub fn ended_at(&mut self, generation: u64, installed_id: &str, order_id: &str,
        ids: &[&str], complete: bool) -> Decision
    {
        if !self.accepts_end(generation, installed_id) {
            return Decision::Idle;
        }
        self.issued_restore = None;
        self.complete |= complete;
        self.consecutive_failures = 0;
        self.waiting_after = Some(order_id.into());
        self.advance(order_id, ids)
    }

    /// Resolution/codec failures skip an item once per scan, rather than entering
    /// an automatic retry loop. An explicit viewer retry removes that exclusion.
    pub fn start_failed(
        &mut self,
        generation: u64,
        id: &str,
        ids: &[&str],
        complete: bool,
        now_ms: u64,
    ) -> Decision {
        if !self.accepts(generation) || self.current.as_deref() != Some(id)
            || self.failed.contains(id) || self.deferred.is_some() {
            return Decision::Idle;
        }
        self.issued_restore = None;
        self.complete |= complete;
        self.failed.insert(id.into());
        self.waiting_after = Some(id.into());
        let delay = FAILURE_DELAYS_MS.get(self.consecutive_failures).copied();
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if delay != Some(0) && self.complete && self.successor(id, ids, true).is_none() {
            self.failed_continuation = FailedContinuation::Halted;
            self.waiting_after = None;
            return Decision::Idle;
        }
        match delay {
            Some(0) => {
                self.failed_continuation = FailedContinuation::Discovery;
                self.advance(id, ids)
            }
            Some(delay) => {
                self.failed_continuation = FailedContinuation::Cooldown {
                    not_before_ms: now_ms.saturating_add(delay),
                };
                if self.successor(id, ids, self.complete).is_none() {
                    self.exhausted()
                } else {
                    Decision::Wait
                }
            }
            None => {
                self.failed_continuation = FailedContinuation::Halted;
                self.waiting_after = None;
                Decision::Idle
            }
        }
    }

    /// A live EOF is a disconnected source, never completion of a TV channel.
    /// Reuse the failure budget and frame deadline without selecting a successor.
    pub fn live_disconnected(&mut self, generation: u64, id: &str, now_ms: u64) -> Decision {
        if !self.accepts_end(generation, id) { return Decision::Idle; }
        self.issued_restore = None;
        self.waiting_after = Some(id.into());
        let delay = FAILURE_DELAYS_MS.get(self.consecutive_failures).copied();
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        match delay {
            Some(delay) => {
                self.failed_continuation = FailedContinuation::SameCooldown {
                    not_before_ms: now_ms.saturating_add(delay.max(1_000)),
                };
                Decision::Wait
            }
            None => {
                self.failed_continuation = FailedContinuation::Halted;
                self.waiting_after = None;
                Decision::Idle
            }
        }
    }

    /// The existing frame runner supplies its monotonic clock projection after
    /// handling viewer commands. No worker, wall-clock read or queued callback
    /// belongs to this policy; cancellation removes its only pending deadline.
    pub fn tick(&mut self, generation: u64, now_ms: u64, ids: &[&str], complete: bool) -> Decision {
        if !self.accepts(generation) {
            return Decision::Idle;
        }
        let (not_before_ms, same) = match self.failed_continuation {
            FailedContinuation::Cooldown { not_before_ms } => (not_before_ms, false),
            FailedContinuation::SameCooldown { not_before_ms } => (not_before_ms, true),
            _ => return Decision::Idle,
        };
        if now_ms < not_before_ms { return Decision::Wait; }
        if same {
            let Some(id) = self.waiting_after.clone().filter(|id| ids.contains(&id.as_str())) else {
                self.failed_continuation = FailedContinuation::Halted;
                return Decision::Idle;
            };
            return self.start(&id, 0, Reason::Next);
        }
        self.failed_continuation = FailedContinuation::Discovery;
        self.catalog_updated(generation, ids, complete)
    }

    pub fn pending_retry(&self) -> Option<PendingRetry<'_>> {
        let not_before_ms = match self.failed_continuation {
            FailedContinuation::Cooldown { not_before_ms } | FailedContinuation::SameCooldown { not_before_ms } => not_before_ms,
            _ => return None,
        };
        if !self.accepts(self.generation) { return None; }
        Some(PendingRetry {
            generation: self.generation,
            after: self.waiting_after.as_deref()?,
            failures: self.consecutive_failures,
            not_before_ms,
        })
    }

    /// Avoid rebuilding a large catalogue's filtered order on every idle frame.
    pub fn retry_due(&self, generation: u64, now_ms: u64) -> bool {
        self.accepts(generation) && matches!(self.failed_continuation,
            FailedContinuation::Cooldown { not_before_ms } | FailedContinuation::SameCooldown { not_before_ms } if now_ms >= not_before_ms)
    }

    fn accepts(&self, generation: u64) -> bool {
        self.enabled && !self.suspended && generation == self.generation
    }

    fn start(&mut self, id: &str, position_ms: u64, reason: Reason) -> Decision {
        self.deferred = None;
        self.issued_restore = (reason == Reason::Restore).then(|| ResumePoint {
            entry_id: id.into(), position_ms,
        });
        self.current = Some(id.into());
        self.waiting_after = None;
        self.failed_continuation = FailedContinuation::None;
        Decision::Start {
            entry_id: id.into(),
            position_ms,
            reason,
        }
    }

    fn exhausted(&self) -> Decision {
        if self.complete {
            Decision::Idle
        } else {
            Decision::Wait
        }
    }

    /// Read the same next-item rule as EOF without changing playback intent.
    /// Pause keeps the planned order; a stopped or different owner has no plan.
    pub fn peek_next<'a>(&self, generation: u64, current: &str, ids: &[&'a str], complete: bool) -> Option<&'a str> {
        self.peek_next_at(generation, current, current, ids, complete)
    }

    /// Preview the same proven traversal anchor as ended_at without changing
    /// the installed selection, its pause state or any continuation intent.
    pub fn peek_next_at<'a>(&self, generation: u64, installed_id: &str, order_id: &str,
        ids: &[&'a str], complete: bool) -> Option<&'a str>
    {
        if generation != self.generation || self.explicitly_stopped
            || self.current.as_deref() != Some(installed_id) || self.deferred.is_some() { return None; }
        self.successor(order_id, ids, self.complete || complete)
    }

    fn successor<'a>(&self, after: &str, ids: &[&'a str], complete: bool) -> Option<&'a str> {
        let index = ids.iter().position(|id| *id == after);
        if let Some(index) = index {
            if let Some(id) = ids[index + 1..].iter().find(|id| !self.failed.contains(**id)) {
                return Some(*id);
            }
        }
        // Wrapping a partial list can replay the first file indefinitely while
        // the next file is still being discovered. Completion grants wrapping.
        if complete {
            let end = index.map_or(ids.len(), |index| index + 1);
            return ids[..end].iter().find(|id| !self.failed.contains(**id)).copied();
        }
        None
    }

    fn advance(&mut self, after: &str, ids: &[&str]) -> Decision {
        match self.successor(after, ids, self.complete) {
            Some(id) => self.start(id, 0, Reason::Next),
            None => self.exhausted(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resume_floor_keeps_this_checkpoint_when_a_later_sample_releases_it() {
        let mut floor = ResumeFloor::new(30_000).unwrap();
        // The callback advances between reading the intended checkpoint and
        // reading the raw observation. Only subsequent checkpoints are free.
        assert_eq!(floor.checkpoint(29_000, Some(31_000), Some(60_000)), 30_000);
        assert_eq!(floor.checkpoint(29_000, Some(29_000), Some(60_000)), 29_000);
    }

    #[test]
    fn issued_restore_survives_suspend_but_requires_exact_unconsumed_intent() {
        let mut c = Continuity::new(1, Some(ResumePoint {
            entry_id: "saved".into(), position_ms: 30_000,
        }));
        assert_eq!(c.catalog_updated(1, &["saved"], true), start("saved", 30_000, Reason::Restore));
        c.suspend(1);
        c.rebase(2);
        c.resume_after_suspend(2);
        assert!(c.issued_restore(2, "saved", 30_000));
        assert!(!c.issued_restore(1, "saved", 30_000));
        assert!(!c.issued_restore(2, "other", 30_000));
        assert!(!c.issued_restore(2, "saved", 29_000));
        c.restore_admitted(2, "other", 30_000);
        assert!(c.issued_restore(2, "saved", 30_000));
        c.restore_admitted(2, "saved", 30_000);
        assert!(!c.issued_restore(2, "saved", 30_000));

        let _ = c.start("saved", 30_000, Reason::Restore);
        c.user_selected(2, "saved");
        assert!(!c.issued_restore(2, "saved", 30_000), "same row is still new viewer intent");
    }

    #[test]
    fn programme_peek_is_the_eof_rule_and_does_not_change_its_owner() {
        let mut c=Continuity::new(1,None);c.user_selected(1,"c");
        let ids=["c","a","b"];
        assert_eq!(c.peek_next(1,"c",&ids,false),Some("a"));
        assert_eq!(c.peek_next(1,"c",&ids,false),Some("a"));
        assert_eq!(c.ended(1,"c",&ids,false),start("a",0,Reason::Next));
        assert_eq!(c.start_failed(1,"a",&ids,false,0),start("b",0,Reason::Next));
        assert_eq!(c.peek_next(1,"b",&ids,false),None);
        assert_eq!(c.peek_next(1,"b",&ids,true),Some("c"));
        c.pause(1);assert_eq!(c.peek_next(1,"b",&ids,true),Some("c"));
        c.resume(1);assert_eq!(c.ended(1,"b",&ids,true),start("c",0,Reason::Next));
        assert_eq!(c.peek_next(1,"c",&ids,true),Some("b"));
        c.stop(1);assert_eq!(c.peek_next(1,"c",&ids,true),None);
    }


    #[test]
    fn rebase_preserves_stopped_and_paused_intents() {
        let mut stopped = Continuity::new(1, None);
        stopped.stop(1);
        stopped.rebase(2);
        stopped.resume(2);
        assert_eq!(stopped.catalog_updated(2, &["first"], true), Decision::Idle);

        let mut paused = Continuity::new(1, None);
        assert_eq!(
            paused.catalog_updated(1, &["first"], true),
            start("first", 0, Reason::First)
        );
        paused.pause(1);
        paused.rebase(2);
        assert_eq!(
            paused.catalog_updated(2, &["first", "second"], false),
            Decision::Idle
        );
        paused.resume(2);
        assert_eq!(
            paused.ended(2, "first", &["first", "second"], false),
            start("second", 0, Reason::Next)
        );
    }

    #[test]
    fn rebase_keeps_current_and_failures_but_requires_new_catalogue_completion() {
        let mut state = Continuity::new(1, None);
        assert_eq!(
            state.catalog_updated(1, &["bad", "good"], true),
            start("bad", 0, Reason::First)
        );
        assert_eq!(
            state.start_failed(1, "bad", &["bad", "good"], true, 0),
            start("good", 0, Reason::Next)
        );
        state.rebase(2);
        assert_eq!(state.catalog_updated(1, &["old"], true), Decision::Idle);
        assert_eq!(
            state.catalog_updated(2, &["bad", "good"], false),
            Decision::Idle
        );
        assert_eq!(
            state.ended(2, "good", &["bad", "good"], false),
            Decision::Wait
        );
        assert_eq!(
            state.catalog_updated(2, &["bad", "good"], true),
            start("good", 0, Reason::Next)
        );
    }
    fn start(id: &str, position_ms: u64, reason: Reason) -> Decision {
        Decision::Start {
            entry_id: id.into(),
            position_ms,
            reason,
        }
    }
    fn saved(id: &str) -> Option<ResumePoint> {
        Some(ResumePoint {
            entry_id: id.into(),
            position_ms: 42000,
        })
    }

    #[test]
    fn first_file_does_not_wait_for_saved_item_at_3450() {
        let mut state = Continuity::new(1, saved("3450"));
        assert_eq!(
            state.catalog_updated(1, &["first"], false),
            start("first", 0, Reason::First)
        );
        assert_eq!(state.pending_restore(), saved("3450").as_ref());
        assert_eq!(
            state.catalog_updated(1, &["first", "second"], false),
            Decision::Idle
        );
        assert_eq!(
            state.catalog_updated(1, &["first", "second", "3450"], false),
            start("3450", 42000, Reason::Restore)
        );
        assert_eq!(state.pending_restore(), None);
        assert_eq!(
            state.catalog_updated(1, &["first", "second", "3450"], true),
            Decision::Idle
        );
    }

    #[test]
    fn saved_item_in_first_batch_starts_once_at_its_position() {
        let mut state = Continuity::new(1, saved("saved"));
        assert_eq!(
            state.catalog_updated(1, &["first", "saved"], false),
            start("saved", 42000, Reason::Restore)
        );
        assert_eq!(
            state.catalog_updated(1, &["first", "saved"], true),
            Decision::Idle
        );
    }

    #[test]
    fn missing_bookmark_does_not_restart_fallback_on_completion() {
        let mut state = Continuity::new(1, saved("gone"));
        state.catalog_updated(1, &["first"], false);
        assert_eq!(state.catalog_updated(1, &["first"], true), Decision::Idle);
        assert_eq!(
            state.ended(1, "first", &["first"], true),
            start("first", 0, Reason::Next)
        );
    }

    #[test]
    fn viewer_selection_cancels_delayed_restoration() {
        let mut state = Continuity::new(1, saved("saved"));
        state.catalog_updated(1, &["first"], false);
        state.user_selected(1, "chosen");
        assert_eq!(
            state.catalog_updated(1, &["first", "chosen", "saved"], true),
            Decision::Idle
        );
        assert_eq!(
            state.ended(1, "first", &["first", "chosen"], true),
            Decision::Idle
        );
    }

    #[test]
    fn stop_prevents_late_batches_from_starting_anything() {
        let mut state = Continuity::new(1, saved("saved"));
        state.stop(1);
        assert_eq!(state.catalog_updated(1, &["saved"], true), Decision::Idle);
        state.user_selected(1, "saved");
        assert_eq!(
            state.ended(1, "saved", &["saved", "next"], true),
            start("next", 0, Reason::Next)
        );
    }

    #[test]
    fn pause_and_resume_retain_eos_without_late_restore() {
        let mut state = Continuity::new(1, saved("saved"));
        state.catalog_updated(1, &["first"], false);
        state.pause(1);
        assert_eq!(
            state.catalog_updated(1, &["first", "saved"], true),
            Decision::Idle
        );
        state.resume(1);
        assert_eq!(
            state.catalog_updated(1, &["first", "saved"], true),
            Decision::Idle
        );
        assert_eq!(
            state.ended(1, "first", &["first", "saved"], true),
            start("saved", 0, Reason::Next)
        );
    }

    #[test]
    fn catalog_completed_while_paused_starts_once_after_resume() {
        let mut state = Continuity::new(1, None);
        state.pause(1);
        assert_eq!(
            state.catalog_updated(1, &["first", "next"], true),
            Decision::Idle
        );
        state.resume(1);
        assert_eq!(
            state.catalog_updated(1, &["first", "next"], true),
            start("first", 0, Reason::First)
        );
        assert_eq!(
            state.catalog_updated(1, &["first", "next"], true),
            Decision::Idle
        );
    }

    #[test]
    fn obsolete_generation_cannot_change_policy() {
        let mut state = Continuity::new(2, saved("saved"));
        assert_eq!(state.catalog_updated(1, &["old"], true), Decision::Idle);
        state.stop(1);
        state.user_selected(1, "old");
        assert_eq!(
            state.catalog_updated(2, &["saved"], false),
            start("saved", 42000, Reason::Restore)
        );
        assert_eq!(
            state.start_failed(1, "saved", &["saved"], true, 0),
            Decision::Idle
        );
        assert_eq!(state.ended(1, "saved", &["saved"], true), Decision::Idle);
    }

    #[test]
    fn eof_waits_for_next_page_instead_of_replaying_first() {
        let mut state = Continuity::new(1, None);
        state.catalog_updated(1, &["first"], false);
        assert_eq!(state.ended(1, "first", &["first"], false), Decision::Wait);
        assert_eq!(state.catalog_updated(1, &["first"], false), Decision::Wait);
        assert_eq!(
            state.catalog_updated(1, &["first", "next"], false),
            start("next", 0, Reason::Next)
        );
    }

    #[test]
    fn eof_can_advance_before_scan_finishes_and_wraps_only_when_complete() {
        let mut state = Continuity::new(1, None);
        state.catalog_updated(1, &["first", "next"], false);
        assert_eq!(
            state.ended(1, "first", &["first", "next"], false),
            start("next", 0, Reason::Next)
        );
        assert_eq!(
            state.ended(1, "next", &["first", "next"], false),
            Decision::Wait
        );
        assert_eq!(
            state.catalog_updated(1, &["first", "next"], true),
            start("first", 0, Reason::Next)
        );
    }

    #[test]
    fn failed_file_is_not_retried_forever() {
        let mut state = Continuity::new(1, None);
        state.catalog_updated(1, &["bad"], false);
        assert_eq!(
            state.start_failed(1, "bad", &["bad"], false, 0),
            Decision::Wait
        );
        assert_eq!(
            state.catalog_updated(1, &["bad", "also-bad"], false),
            start("also-bad", 0, Reason::Next)
        );
        assert_eq!(
            state.start_failed(1, "also-bad", &["bad", "also-bad"], true, 0),
            Decision::Idle
        );
        assert_eq!(
            state.catalog_updated(1, &["bad", "also-bad"], true),
            Decision::Idle
        );
    }

    #[test]
    fn failed_saved_file_moves_on_without_reapplying_resume() {
        let mut state = Continuity::new(1, saved("bad"));
        state.catalog_updated(1, &["bad", "good"], false);
        assert_eq!(
            state.start_failed(1, "bad", &["bad", "good"], false, 0),
            start("good", 0, Reason::Next)
        );
        assert_eq!(
            state.catalog_updated(1, &["bad", "good"], true),
            Decision::Idle
        );
    }

    #[test]
    fn empty_catalogue_waits_then_settles() {
        let mut state = Continuity::new(1, saved("gone"));
        assert_eq!(state.catalog_updated(1, &[], false), Decision::Wait);
        assert_eq!(state.catalog_updated(1, &[], true), Decision::Idle);
    }

    #[test]
    fn explicit_stop_survives_background_until_a_new_selection() {
        let mut state = Continuity::new(1, saved("remembered"));
        assert_eq!(
            state.catalog_updated(1, &["first"], false),
            start("first", 0, Reason::First)
        );
        state.stop(1);
        state.pause(1);
        state.resume(1);
        assert_eq!(
            state.catalog_updated(1, &["first", "remembered"], true),
            Decision::Idle
        );
        assert_eq!(
            state.ended(1, "first", &["first", "remembered"], true),
            Decision::Idle
        );
        assert!(state.pending_restore().is_none());
        state.user_selected(1, "remembered");
        assert_eq!(
            state.ended(1, "remembered", &["first", "remembered"], true),
            start("first", 0, Reason::Next)
        );
    }

    #[test]
    fn passive_refresh_cannot_start_an_idle_catalogue_even_after_resume() {
        let mut state = Continuity::new(1, None);
        state.suppress_unrequested_start();
        state.rebase(2);
        state.resume(2);
        assert_eq!(state.catalog_updated(2, &["first"], true), Decision::Idle);
        state.user_selected(2, "first");
        assert_eq!(state.ended(2, "first", &["first", "next"], true), start("next", 0, Reason::Next));
    }

    #[test]
    fn passive_refresh_retains_current_playback_progression() {
        let mut state = Continuity::new(1, None);
        state.user_selected(1, "current");
        state.suppress_unrequested_start();
        state.rebase(2);
        assert_eq!(state.catalog_updated(2, &["current", "next"], true), Decision::Idle);
        assert_eq!(state.ended(2, "current", &["current", "next"], true), start("next", 0, Reason::Next));
    }

    #[test]
    fn passive_refresh_retains_an_existing_restore_intent() {
        let mut state = Continuity::new(1, Some(ResumePoint { entry_id: "saved".into(), position_ms: 47_000 }));
        state.suppress_unrequested_start();
        state.rebase(2);
        assert_eq!(state.catalog_updated(2, &["saved"], true), start("saved", 47_000, Reason::Restore));
    }

    #[test]
    fn passive_refresh_retains_an_awaited_successor() {
        let mut state = Continuity::new(1, None);
        state.user_selected(1, "finished");
        assert_eq!(state.ended(1, "finished", &["finished"], false), Decision::Wait);
        state.suppress_unrequested_start();
        state.rebase(2);
        assert_eq!(state.catalog_updated(2, &["finished", "next"], false), start("next", 0, Reason::Next));
    }
}

#[cfg(test)]
mod lifecycle_suspend_tests {
    use super::*;
    #[test]
    fn suspended_late_restore_survives_and_resumes_on_current_generation_only() {
        let mut state = Continuity::new(7, Some(ResumePoint { entry_id: "late".into(), position_ms: 42_000 }));
        assert!(matches!(state.catalog_updated(7, &["first"], false), Decision::Start { .. }));
        state.rebase(8);
        state.suspend(8);
        state.resume_after_suspend(7);
        assert_eq!(state.catalog_updated(8, &["first", "late"], true), Decision::Idle);
        assert_eq!(state.pending_restore().unwrap().entry_id, "late");
        state.resume_after_suspend(8);
        assert_eq!(state.catalog_updated(8, &["first", "late"], true), Decision::Start {
            entry_id: "late".into(), position_ms: 42_000, reason: Reason::Restore });
    }
    #[test]
    fn lifecycle_wake_does_not_undo_an_explicit_viewer_pause() {
        let mut state = Continuity::new(4, None);
        state.user_selected(4, "first");
        state.pause(4);
        state.suspend(4);
        state.resume_after_suspend(4);
        assert!(!state.accepts_end(4, "first"));
        state.resume(4);
        assert!(state.accepts_end(4, "first"));
    }
    #[test]
    fn stop_and_new_source_cannot_inherit_or_be_revived_by_old_lifecycle_state() {
        let mut state = Continuity::new(2, None);
        state.user_selected(2, "first");
        state.suspend(2);
        state.stop(2);
        state.resume_after_suspend(2);
        assert!(!state.accepts_end(2, "first"));
        state = Continuity::new(3, None);
        state.suspend(2);
        assert!(matches!(state.catalog_updated(3, &["new"], false), Decision::Start { .. }));
    }
}

#[cfg(test)]
mod failed_continuation_tests {
    use super::{Continuity, Decision, Reason, ResumePoint};

    const IDS: &[&str] = &["a", "b", "c", "d", "e", "f"];

    fn next(id: &str) -> Decision {
        Decision::Start { entry_id: id.into(), position_ms: 0, reason: Reason::Next }
    }

    fn cooling() -> Continuity {
        let mut policy = Continuity::new(1, None);
        policy.user_selected(1, "a");
        assert_eq!(policy.start_failed(1, "a", IDS, true, 100), next("b"));
        assert_eq!(policy.start_failed(1, "b", IDS, true, 100), Decision::Wait);
        policy
    }

    #[test]
    fn repeated_failures_have_fixed_delays_and_finite_total_budget() {
        let mut policy = cooling();
        assert!(!policy.retry_due(1, 1099));
        assert_eq!(policy.tick(1, 1099, IDS, true), Decision::Wait);
        assert!(policy.retry_due(1, 1100));
        assert_eq!(policy.tick(1, 1100, IDS, true), next("c"));
        assert_eq!(policy.start_failed(1, "c", IDS, true, 1100), Decision::Wait);
        assert_eq!(policy.tick(1, 3099, IDS, true), Decision::Wait);
        assert_eq!(policy.tick(1, 3100, IDS, true), next("d"));
        assert_eq!(policy.start_failed(1, "d", IDS, true, 3100), Decision::Wait);
        assert_eq!(policy.tick(1, 7099, IDS, true), Decision::Wait);
        assert_eq!(policy.tick(1, 7100, IDS, true), next("e"));
        assert_eq!(policy.start_failed(1, "e", IDS, true, 7100), Decision::Idle);
        for now in [7101, 60_000, u64::MAX] {
            assert!(!policy.retry_due(1, now));
            assert_eq!(policy.tick(1, now, IDS, true), Decision::Idle);
            assert_eq!(policy.catalog_updated(1, IDS, true), Decision::Idle);
        }
        policy.rebase(2);
        assert_eq!(policy.catalog_updated(2, IDS, true), Decision::Idle);
        assert_eq!(policy.tick(2, u64::MAX, IDS, true), Decision::Idle);
    }

    #[test]
    fn catalogue_pages_and_late_bookmark_cannot_bypass_or_extend_cooldown() {
        let mut policy = Continuity::new(1, Some(ResumePoint {
            entry_id: "saved".into(), position_ms: 99_000,
        }));
        policy.catalog_updated(1, &["a", "b"], false);
        assert_eq!(policy.start_failed(1, "a", &["a", "b"], false, 100), next("b"));
        assert_eq!(policy.start_failed(1, "b", &["a", "b"], false, 100), Decision::Wait);
        let page = &["a", "b", "saved", "c"];
        for _ in 0..100 {
            assert_eq!(policy.catalog_updated(1, page, true), Decision::Wait);
        }
        assert_eq!(policy.tick(1, 1099, page, false), Decision::Wait);
        assert_eq!(policy.tick(1, 1100, page, false), Decision::Start {
            entry_id: "saved".into(), position_ms: 99_000, reason: Reason::Restore,
        });
        // Complete=true from a gated page was retained; the third failure still
        // consumes the same budget, even though the selected item was a restore.
        assert_eq!(policy.start_failed(1, "saved", page, false, 1100), Decision::Wait);
        assert_eq!(policy.tick(1, 3099, page, false), Decision::Wait);
        assert_eq!(policy.tick(1, 3100, page, false), next("c"));
    }

    #[test]
    fn duplicate_failure_stale_generation_and_old_eof_do_not_change_budget() {
        let mut policy = cooling();
        assert_eq!(policy.start_failed(1, "b", IDS, true, 1050), Decision::Idle);
        assert_eq!(policy.start_failed(0, "b", IDS, true, 1050), Decision::Idle);
        assert_eq!(policy.ended(1, "b", IDS, true), Decision::Idle);
        assert_eq!(policy.tick(0, u64::MAX, IDS, true), Decision::Idle);
        assert_eq!(policy.tick(1, 1100, IDS, true), next("c"));
        assert_eq!(policy.start_failed(1, "c", IDS, true, 1100), Decision::Wait);
        assert_eq!(policy.tick(1, 3099, IDS, true), Decision::Wait);
    }

    #[test]
    fn stop_cancels_failure_deadline_and_discovery() {
        let mut policy = cooling();
        policy.stop(1);
        assert_eq!(policy.tick(1, u64::MAX, IDS, true), Decision::Idle);
        assert_eq!(policy.catalog_updated(1, IDS, true), Decision::Idle);
        policy.resume(1);
        assert_eq!(policy.catalog_updated(1, IDS, true), Decision::Idle);
    }

    #[test]
    fn pause_cancels_failure_deadline_even_after_resume() {
        let mut policy = cooling();
        policy.pause(1);
        policy.resume(1);
        assert_eq!(policy.tick(1, u64::MAX, IDS, true), Decision::Idle);
        assert_eq!(policy.catalog_updated(1, IDS, true), Decision::Idle);
    }

    #[test]
    fn standby_cancels_failure_deadline_even_after_wake() {
        let mut policy = cooling();
        policy.suspend(1);
        policy.resume_after_suspend(1);
        assert_eq!(policy.tick(1, u64::MAX, IDS, true), Decision::Idle);
        assert_eq!(policy.catalog_updated(1, IDS, true), Decision::Idle);
    }

    #[test]
    fn rebase_cancels_deadline_without_renewing_failure_budget() {
        let mut policy = cooling();
        policy.rebase(2);
        assert_eq!(policy.tick(1, u64::MAX, IDS, true), Decision::Idle);
        assert_eq!(policy.tick(2, u64::MAX, IDS, true), Decision::Idle);
        assert_eq!(policy.catalog_updated(2, IDS, true), Decision::Idle);
        // Keeping a prior installed owner is not a new viewer request.
        policy.retain_selection(2, "c");
        assert_eq!(policy.start_failed(2, "c", IDS, true, 2000), Decision::Wait);
        assert_eq!(policy.tick(2, 3999, IDS, true), Decision::Wait);
        assert_eq!(policy.tick(2, 4000, IDS, true), next("d"));
    }

    #[test]
    fn source_replacement_cannot_inherit_old_failure_deadline() {
        let mut old = cooling();
        old.stop(1);
        let mut replacement = Continuity::new(2, None);
        assert_eq!(replacement.tick(1, u64::MAX, IDS, true), Decision::Idle);
        assert_eq!(replacement.catalog_updated(2, &["new"], true), Decision::Start {
            entry_id: "new".into(), position_ms: 0, reason: Reason::First,
        });
    }

    #[test]
    fn same_id_user_retry_rearms_budget_and_removes_old_deadline() {
        let mut policy = cooling();
        policy.user_selected(1, "b");
        assert_eq!(policy.tick(1, 500, IDS, true), Decision::Idle);
        assert_eq!(policy.start_failed(1, "b", IDS, true, 500), next("c"));
        assert_eq!(policy.start_failed(1, "c", IDS, true, 500), Decision::Wait);
        assert_eq!(policy.tick(1, 1100, IDS, true), Decision::Wait);
        assert_eq!(policy.tick(1, 1500, IDS, true), next("d"));
    }

    #[test]
    fn retained_owner_and_automatic_start_do_not_reset_streak() {
        let mut policy = cooling();
        assert_eq!(policy.tick(1, 1100, IDS, true), next("c"));
        policy.retain_selection(1, "c");
        assert_eq!(policy.start_failed(1, "c", IDS, true, 1100), Decision::Wait);
        assert_eq!(policy.tick(1, 3099, IDS, true), Decision::Wait);
        assert_eq!(policy.tick(1, 3100, IDS, true), next("d"));
    }

    #[test]
    fn real_eof_resets_failure_budget_and_normal_eof_never_waits() {
        let mut policy = cooling();
        assert_eq!(policy.tick(1, 1100, IDS, true), next("c"));
        assert_eq!(policy.ended(1, "c", IDS, true), next("d"));
        assert_eq!(policy.ended(1, "d", IDS, true), next("e"));
        assert_eq!(policy.start_failed(1, "e", IDS, true, 1100), next("f"));
    }

    #[test]
    fn standby_also_cancels_first_failure_waiting_for_a_catalogue_page() {
        let mut policy = Continuity::new(1, None);
        policy.user_selected(1, "a");
        assert_eq!(policy.start_failed(1, "a", &["a"], false, 100), Decision::Wait);
        policy.suspend(1);
        policy.resume_after_suspend(1);
        assert_eq!(policy.catalog_updated(1, &["a", "b"], true), Decision::Idle);
    }
}

#[cfg(test)]
mod failed_catalog_burst_tests {
    use super::{Continuity, Decision};

    #[test]
    fn thousand_failing_items_stop_at_five_even_over_a_day_of_ticks() {
        let owned: Vec<_> = (0..1000).map(|i| format!("plex-{i}")).collect();
        let ids: Vec<_> = owned.iter().map(String::as_str).collect();
        let mut machine = Continuity::new(1, None);
        let mut decision = machine.catalog_updated(1, &ids, true);
        let mut attempts = 0;
        for now in [0, 1000, 3000, 7000, 60_000, 86_400_000] {
            if now > 0 { decision = machine.tick(1, now, &ids, true); }
            while let Decision::Start { entry_id, .. } = decision {
                attempts += 1;
                decision = machine.start_failed(1, &entry_id, &ids, true, now);
            }
            if now == 0 { assert_eq!(attempts, 2); }
        }
        assert_eq!(attempts, 5, "finite failure budget must not drain a whole large catalogue");
    }
}

#[cfg(test)]
mod deferred_start_tests {
    use super::{Continuity, Decision, Reason, ResumePoint};

    fn start(id: &str, position_ms: u64, reason: Reason) -> Decision {
        Decision::Start { entry_id: id.into(), position_ms, reason }
    }

    fn defer(policy: &mut Continuity, generation: u64, decision: Decision) {
        let Decision::Start { entry_id, position_ms, reason } = decision else {
            panic!("expected an automatic Start");
        };
        assert!(policy.defer_start(generation, &entry_id, position_ms, reason));
    }

    #[test]
    fn first_fallback_preserves_background_restore_and_restore_remains_pending_until_admission() {
        let point = ResumePoint { entry_id: "saved".into(), position_ms: 42_000 };
        let mut policy = Continuity::new(1, Some(point.clone()));
        let first = policy.catalog_updated(1, &["first"], false);
        assert_eq!(first, start("first", 0, Reason::First));
        defer(&mut policy, 1, first);
        policy.suppress_unrequested_start();
        assert!(policy.has_deferred_start(1));
        assert_eq!(policy.pending_restore(), Some(&point));
        let saved = policy.catalog_updated(1, &["first", "saved"], false);
        assert_eq!(saved, start("saved", 42_000, Reason::Restore));
        defer(&mut policy, 1, saved);
        assert_eq!(policy.pending_restore(), Some(&point));
        assert_eq!(policy.catalog_updated(1, &["first", "saved"], true),
            start("saved", 42_000, Reason::Restore));
        assert!(policy.pending_restore().is_none());
        assert!(!policy.has_deferred_start(1));
    }

    #[test]
    fn deferred_only_target_ignores_old_and_duplicate_eof_and_failure_even_after_singleton_wrap() {
        let mut policy = Continuity::new(1, None);
        policy.user_selected(1, "a");
        let next = policy.ended(1, "a", &["a"], true);
        assert_eq!(next, start("a", 0, Reason::Next));
        defer(&mut policy, 1, next);
        assert!(!policy.accepts_end(1, "a"));
        assert_eq!(policy.ended(1, "a", &["a"], true), Decision::Idle);
        assert_eq!(policy.start_failed(1, "a", &["a"], true, 100), Decision::Idle);
        assert!(policy.failed.is_empty());
        assert_eq!(policy.consecutive_failures, 0);
        assert_eq!(policy.peek_next(1, "a", &["a"], true), None);
        assert!(policy.has_deferred_start(1));
        assert_eq!(policy.catalog_updated(1, &["a"], true), start("a", 0, Reason::Next));
        assert!(policy.accepts_end(1, "a"));
    }

    #[test]
    fn pause_stop_and_explicit_selection_retire_deferred_start() {
        for action in ["pause", "stop", "select"] {
            let mut policy = Continuity::new(1, None);
            let decision = policy.catalog_updated(1, &["a", "b"], true);
            defer(&mut policy, 1, decision);
            match action {
                "pause" => policy.pause(1),
                "stop" => policy.stop(1),
                _ => policy.user_selected(1, "b"),
            }
            assert!(!policy.has_deferred_start(1), "{action}");
            policy.resume_after_suspend(1);
            assert_eq!(policy.catalog_updated(1, &["a", "b"], true), Decision::Idle, "{action}");
            assert!(!policy.accepts_end(1, "a"), "{action}");
        }
    }

    #[test]
    fn suspension_retains_restore_without_any_start_before_wake() {
        let point = ResumePoint { entry_id: "local".into(), position_ms: 23_000 };
        let mut policy = Continuity::new(1, Some(point.clone()));
        let decision = policy.catalog_updated(1, &["local"], true);
        defer(&mut policy, 1, decision);
        policy.suspend(1);
        assert!(!policy.has_deferred_start(1));
        assert_eq!(policy.pending_restore(), Some(&point));
        assert_eq!(policy.catalog_updated(1, &["local"], true), Decision::Idle);
        assert_eq!(policy.tick(1, 100_000, &["local"], true), Decision::Idle);
        policy.resume_after_suspend(1);
        assert!(policy.has_deferred_start(1));
        assert_eq!(policy.catalog_updated(1, &["local"], true), start("local", 23_000, Reason::Restore));
    }

    #[test]
    fn rebase_waits_for_exact_member_then_uses_new_order_and_rejects_old_generation() {
        let mut policy = Continuity::new(1, None);
        let decision = policy.catalog_updated(1, &["a", "b"], true);
        defer(&mut policy, 1, decision);
        policy.rebase(2);
        assert!(!policy.has_deferred_start(1));
        assert!(!policy.defer_start(1, "a", 0, Reason::First));
        assert_eq!(policy.catalog_updated(1, &["a"], true), Decision::Idle);
        assert_eq!(policy.catalog_updated(2, &["b"], false), Decision::Wait);
        assert!(policy.has_deferred_start(2));
        assert_eq!(policy.catalog_updated(2, &["b", "a", "c"], true), start("a", 0, Reason::First));
        assert_eq!(policy.ended(2, "a", &["b", "a", "c"], true), start("c", 0, Reason::Next));
    }

    #[test]
    fn completed_replacement_without_exact_deferred_member_never_uses_old_ordinal() {
        let mut policy = Continuity::new(1, None);
        let decision = policy.catalog_updated(1, &["a"], true);
        defer(&mut policy, 1, decision);
        policy.rebase(2);
        assert_eq!(policy.catalog_updated(2, &["replacement"], true), Decision::Idle);
        assert!(!policy.has_deferred_start(2));
        assert_eq!(policy.catalog_updated(2, &["replacement", "a"], true), Decision::Idle);
    }

    #[test]
    fn cancel_restore_retires_deferred_restore_and_restores_only_the_actual_retained_item() {
        for retained in [None, Some("playing")] {
            let mut policy = Continuity::new(1, Some(ResumePoint { entry_id: "saved".into(), position_ms: 500 }));
            let decision = policy.catalog_updated(1, &["saved"], false);
            defer(&mut policy, 1, decision);
            policy.cancel_restore(1, retained);
            assert!(policy.pending_restore().is_none());
            assert!(!policy.has_deferred_start(1));
            assert!(!policy.accepts_end(1, "saved"));
            assert_eq!(policy.current.as_deref(), retained);
        }
        let mut fallback = Continuity::new(1, Some(ResumePoint { entry_id: "saved".into(), position_ms: 500 }));
        let first = fallback.catalog_updated(1, &["first"], false);
        defer(&mut fallback, 1, first);
        fallback.cancel_restore(1, None);
        assert!(fallback.pending_restore().is_none());
        assert!(fallback.has_deferred_start(1));
        assert_eq!(fallback.catalog_updated(1, &["first"], true), start("first", 0, Reason::First));
    }

    #[test]
    fn failure_derived_deferred_next_survives_suspend_and_rebase_without_renewing_budget() {
        let ids = ["a", "b", "c", "d"];
        let mut policy = Continuity::new(1, None);
        policy.user_selected(1, "a");
        let next = policy.start_failed(1, "a", &ids, true, 100);
        assert_eq!(next, start("b", 0, Reason::Next));
        defer(&mut policy, 1, next);
        assert_eq!(policy.start_failed(1, "b", &ids, true, 100), Decision::Idle);
        assert_eq!(policy.consecutive_failures, 1);
        assert!(policy.failed.contains("a") && !policy.failed.contains("b"));
        policy.suspend(1);
        policy.rebase(2);
        assert_eq!(policy.catalog_updated(2, &ids, true), Decision::Idle);
        policy.resume_after_suspend(2);
        assert_eq!(policy.catalog_updated(2, &["c"], false), Decision::Wait);
        assert_eq!(policy.catalog_updated(2, &ids, true), start("b", 0, Reason::Next));
        assert_eq!(policy.start_failed(2, "b", &ids, true, 500), Decision::Wait);
        assert_eq!(policy.tick(2, 1499, &ids, true), Decision::Wait);
        assert_eq!(policy.tick(2, 1500, &ids, true), start("c", 0, Reason::Next));
        assert_eq!(policy.consecutive_failures, 2);
    }

    #[test]
    fn changed_deferred_identity_cannot_replace_an_already_held_start() {
        let mut policy = Continuity::new(1, None);
        let decision = policy.catalog_updated(1, &["a", "b"], true);
        defer(&mut policy, 1, decision);
        assert!(!policy.defer_start(1, "b", 0, Reason::First));
        assert!(!policy.defer_start(1, "a", 1000, Reason::Restore));
        assert_eq!(policy.catalog_updated(1, &["a", "b"], true), start("a", 0, Reason::First));
    }
}

#[cfg(test)]
#[path = "ott_continuity_selection_tests.rs"]
mod selection_domain_tests;

#[cfg(test)]
#[path = "ott_continuity_anchor_tests.rs"]
mod canonical_anchor_tests;

#[cfg(test)]
mod live_recovery_tests {
    use super::*;
    #[test]
    fn live_eof_retries_exact_channel_with_bounded_cancelable_budget() {
        let mut policy = Continuity::new(7, None);
        policy.user_selected(7, "third");
        let ids = ["third", "fourth", "fifth"];
        let mut now = 100;
        for _ in 0..4 {
            assert_eq!(policy.live_disconnected(7, "third", now), Decision::Wait);
            let due = policy.pending_retry().unwrap().not_before_ms;
            assert_eq!(policy.tick(7, due - 1, &ids, true), Decision::Wait);
            assert!(matches!(policy.tick(7, due, &ids, true),
                Decision::Start { entry_id, .. } if entry_id == "third"));
            now = due + 1;
        }
        assert_eq!(policy.live_disconnected(7, "third", now), Decision::Idle);
        assert_eq!(policy.tick(7, u64::MAX, &ids, true), Decision::Idle);
        policy.user_selected(7, "fourth");
        assert_eq!(policy.live_disconnected(7, "fourth", now), Decision::Wait);
        policy.stop(7);
        assert_eq!(policy.tick(7, u64::MAX, &ids, true), Decision::Idle);
    }
}
