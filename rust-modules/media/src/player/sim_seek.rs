//! Pure host-picture seek policy. Compressed access units are never filtered here:
//! the caller decodes from the keyframe and applies this policy to decoded output.
//!
//! Keep this controller and the retained picture under the SAME mutex. For a first
//! eligible frame, inspect with `decide_frame`, publish its pixels, then commit
//! `mark_presented` before releasing that lock. Merely inspecting a frame must not
//! release the presentation clock. Replacement/reset uses that same lock and a
//! fresh generation, so retired readers cannot publish pixels or readiness.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Status {
    Inactive,
    Waiting,
    Ready { first_pts: i64 },
    Exhausted,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FrameDecision {
    /// A replaced generation or terminal exact seek cannot publish this frame.
    Retired,
    /// Decode-only output before the requested presentation boundary.
    Discard,
    /// Publish first, then call `mark_presented` within the same critical section.
    PresentFirst,
    /// Ordinary output: the caller applies its existing presentation-clock wait.
    Schedule,
}

#[derive(Debug)]
pub(super) struct Controller {
    generation: u64,
    target_ns: Option<i64>,
    status: Status,
}

impl Controller {
    pub(super) const fn new() -> Self {
        Self { generation: 0, target_ns: None, status: Status::Inactive }
    }

    /// The caller supplies a fresh generation and a target in the SAME timeline
    /// as decoded frame PTS. This policy performs no timestamp rebasing or rounding.
    pub(super) fn begin(&mut self, generation: u64, target_ns: i64) {
        self.generation = generation;
        self.target_ns = Some(target_ns);
        self.status = Status::Waiting;
    }

    /// Cancel any exact seek. Ordinary playback in this new generation continues
    /// through the existing clock scheduling path without a presentation floor.
    pub(super) fn reset(&mut self, generation: u64) {
        self.generation = generation;
        self.target_ns = None;
        self.status = Status::Inactive;
    }

    pub(super) const fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) const fn target_ns(&self) -> Option<i64> {
        self.target_ns
    }

    pub(super) const fn status(&self) -> Status {
        self.status
    }

    /// Does not commit readiness: the decoded pixels may still be retired or
    /// rejected before publication. Call under the caller's picture-state mutex.
    pub(super) fn decide_frame(&self, generation: u64, pts: i64) -> FrameDecision {
        if generation != self.generation {
            return FrameDecision::Retired;
        }
        match self.status {
            Status::Inactive => FrameDecision::Schedule,
            Status::Exhausted | Status::Failed => FrameDecision::Retired,
            Status::Waiting | Status::Ready { .. } => {
                if self.target_ns.is_some_and(|target| pts < target) {
                    return FrameDecision::Discard;
                }
                match self.status {
                    Status::Waiting => FrameDecision::PresentFirst,
                    _ => FrameDecision::Schedule,
                }
            }
        }
    }

    /// Commit ONLY after these pixels became the retained picture, with the same
    /// lock still held. Returns whether this committed the first eligible frame.
    /// Subsequent frames never move the seek's initial presentation boundary.
    pub(super) fn mark_presented(&mut self, generation: u64, pts: i64) -> bool {
        if self.decide_frame(generation, pts) != FrameDecision::PresentFirst {
            return false;
        }
        self.status = Status::Ready { first_pts: pts };
        true
    }

    /// Record the decoder's terminal result for this generation. `clean` means
    /// fully drained output AND a successful decoder exit, not merely input EOF.
    /// A clean end without a published eligible frame is Exhausted; a decoder
    /// failure is Failed and must never be translated into successful EOS.
    ///
    /// This policy does not emit EOS, move clocks, or alter viewer pause intent.
    /// A clean drain after Ready preserves the actual first-frame boundary.
    /// Returns false for an obsolete generation, an already-terminal seek, or
    /// ordinary playback (whose completion remains owned by the existing path).
    pub(super) fn finish_decode(&mut self, generation: u64, clean: bool) -> bool {
        if generation != self.generation {
            return false;
        }
        match self.status {
            Status::Inactive | Status::Exhausted | Status::Failed => false,
            Status::Waiting | Status::Ready { .. } => {
                if !clean {
                    self.status = Status::Failed;
                } else if self.status == Status::Waiting {
                    self.status = Status::Exhausted;
                }
                true
            }
        }
    }
}

impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{Controller, FrameDecision, Status};

    #[test]
    fn sparse_gop_decodes_before_target_but_only_published_target_releases_readiness() {
        let mut seek = Controller::new();
        seek.begin(1, 30_366_000_000);
        for pts in [20_854_000_000, 25_000_000_000, 30_365_999_999] {
            assert_eq!(seek.decide_frame(1, pts), FrameDecision::Discard);
            assert!(!seek.mark_presented(1, pts));
            assert_eq!(seek.status(), Status::Waiting);
        }
        let landed = 30_400_000_000;
        assert_eq!(seek.decide_frame(1, landed), FrameDecision::PresentFirst);
        assert_eq!(seek.status(), Status::Waiting, "inspection is not publication");
        assert!(seek.mark_presented(1, landed));
        assert_eq!(seek.status(), Status::Ready { first_pts: landed });
        assert_eq!(seek.decide_frame(1, landed + 40_000_000), FrameDecision::Schedule);
        assert!(!seek.mark_presented(1, landed + 40_000_000));
        assert_eq!(seek.status(), Status::Ready { first_pts: landed });
        assert_eq!(seek.decide_frame(1, 30_365_999_999), FrameDecision::Discard);
    }

    #[test]
    fn exact_boundary_is_eligible_without_rounding_or_overflow_arithmetic() {
        let mut seek = Controller::new();
        seek.begin(2, i64::MAX);
        assert_eq!(seek.decide_frame(2, i64::MAX - 1), FrameDecision::Discard);
        assert_eq!(seek.decide_frame(2, i64::MAX), FrameDecision::PresentFirst);
        assert!(seek.mark_presented(2, i64::MAX));
        assert_eq!(seek.status(), Status::Ready { first_pts: i64::MAX });
    }

    #[test]
    fn replacement_rejects_old_candidate_frame_and_decoder_completion() {
        let mut seek = Controller::new();
        seek.begin(10, 30_000_000_000);
        assert_eq!(seek.decide_frame(10, 30_000_000_000), FrameDecision::PresentFirst);
        seek.begin(11, 60_000_000_000);
        assert_eq!(seek.decide_frame(10, 30_000_000_000), FrameDecision::Retired);
        assert!(!seek.mark_presented(10, 30_000_000_000));
        assert!(!seek.finish_decode(10, true));
        assert!(!seek.finish_decode(10, false));
        assert_eq!(seek.generation(), 11);
        assert_eq!(seek.target_ns(), Some(60_000_000_000));
        assert_eq!(seek.status(), Status::Waiting);
    }

    #[test]
    fn backward_replacement_uses_new_floor_and_does_not_inherit_ready_state() {
        let mut seek = Controller::new();
        seek.begin(20, 60_000_000_000);
        assert!(seek.mark_presented(20, 60_010_000_000));
        seek.begin(21, 5_000_000_000);
        assert_eq!(seek.status(), Status::Waiting);
        assert_eq!(seek.decide_frame(21, 4_960_000_000), FrameDecision::Discard);
        assert!(seek.mark_presented(21, 5_000_000_000));
        assert_eq!(seek.status(), Status::Ready { first_pts: 5_000_000_000 });
        assert_eq!(seek.decide_frame(20, 61_000_000_000), FrameDecision::Retired);
        assert!(!seek.finish_decode(20, false));
    }

    #[test]
    fn cancellation_retires_waiting_and_ready_readers_and_restores_ordinary_scheduling() {
        for ready in [false, true] {
            let mut seek = Controller::new();
            seek.begin(30, 100);
            if ready {
                assert!(seek.mark_presented(30, 101));
            }
            seek.reset(31);
            assert_eq!(seek.status(), Status::Inactive);
            assert_eq!(seek.target_ns(), None);
            assert_eq!(seek.decide_frame(30, 101), FrameDecision::Retired);
            assert!(!seek.mark_presented(30, 101));
            assert!(!seek.finish_decode(30, true));
            assert_eq!(seek.decide_frame(31, 0), FrameDecision::Schedule);
        }
    }

    #[test]
    fn clean_empty_output_is_exhausted_without_synthesizing_a_presented_frame() {
        let mut seek = Controller::new();
        seek.begin(40, 100);
        assert!(seek.finish_decode(40, true));
        assert_eq!(seek.status(), Status::Exhausted);
        assert_eq!(seek.decide_frame(40, 100), FrameDecision::Retired);
        assert!(!seek.mark_presented(40, 100));
        assert!(!seek.finish_decode(40, true));
    }

    #[test]
    fn inspected_but_unpublished_frame_does_not_turn_clean_eof_into_success() {
        let mut seek = Controller::new();
        seek.begin(41, 100);
        assert_eq!(seek.decide_frame(41, 100), FrameDecision::PresentFirst);
        assert!(seek.finish_decode(41, true));
        assert_eq!(seek.status(), Status::Exhausted);
        assert!(!seek.mark_presented(41, 100));
    }

    #[test]
    fn failed_decoder_is_never_classified_as_clean_exhaustion() {
        let mut seek = Controller::new();
        seek.begin(50, 100);
        assert!(seek.finish_decode(50, false));
        assert_eq!(seek.status(), Status::Failed);
        assert!(!seek.finish_decode(50, true));
        assert_eq!(seek.decide_frame(50, 100), FrameDecision::Retired);
        assert!(!seek.mark_presented(50, 100));
    }

    #[test]
    fn clean_drain_preserves_published_boundary_but_real_decode_failure_remains_failure() {
        let mut seek = Controller::new();
        seek.begin(60, 100);
        assert!(seek.mark_presented(60, 110));
        assert!(seek.finish_decode(60, true));
        assert_eq!(seek.status(), Status::Ready { first_pts: 110 });

        seek.begin(61, 200);
        assert!(seek.mark_presented(61, 210));
        assert!(seek.finish_decode(61, false));
        assert_eq!(seek.status(), Status::Failed);
    }

    #[test]
    fn no_seek_keeps_existing_frame_and_completion_paths_unchanged() {
        let mut seek = Controller::default();
        for pts in [i64::MIN, -1, 0, 1, i64::MAX] {
            assert_eq!(seek.decide_frame(0, pts), FrameDecision::Schedule);
            assert!(!seek.mark_presented(0, pts));
        }
        assert!(!seek.finish_decode(0, true));
        assert!(!seek.finish_decode(0, false));
        assert_eq!(seek.status(), Status::Inactive);
        assert_eq!(seek.target_ns(), None);
    }
}
