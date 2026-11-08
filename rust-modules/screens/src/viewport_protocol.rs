//! Addressed placement of an already admitted native player. These values are
//! process-local capabilities, never URLs, portable settings or play commands.
use plx_machine::machine::{Addr, RequestId};
use plx_media::player::viewport::{Failure, Source, Target};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerViewportAction {
    Set(Target),
    /// Only this previously submitted Set may release the placement. A delayed
    /// release cannot remove a later claim from the same screen instance.
    Release { claim: RequestId },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerViewportRequest {
    pub reply: Addr,
    pub source: Source,
    pub action: PlayerViewportAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerViewportFailure {
    Inactive,
    StaleClaim,
    Media(Failure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerViewportOutcome {
    /// The current native sink accepted the placement; not proof of pixels.
    Submitted,
    Rejected(PlayerViewportFailure),
}
