//! Remote track-selection admission for the existing durable kiosk state.
//! Other kiosk actions and recovery are not integrated by this module.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum State {
    Off,
    /// First accepted selection has not yet been durably pinned.
    Armed { strict: bool },
    Locked { strict: bool },
}

pub(super) fn remote_track_selection_allowed(state: State) -> bool {
    matches!(state, State::Off)
}

#[cfg(test)]
#[path = "ott_kiosk_policy_tests.rs"]
mod tests;
