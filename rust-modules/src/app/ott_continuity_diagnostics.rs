//! Numeric, App-owned observation of continuity transitions; never playback authority.
//! Row ordinals are one-based in the exact ordered input at scheduling (zero = absent).
//! Unknown failed-play generations are marked explicitly, never rebound to a newer attempt.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Pending {
    pub catalog_generation: u64,
    pub failed_play_generation: Option<u64>,
    pub row_ordinal: usize,
    pub failures: usize,
    pub not_before_ms: u64,
}

#[derive(Clone, Copy)]
pub(super) enum Reason { None, RequestedExit, Standby, Suspend, Refresh, Changed, Exhausted }
impl Reason {
    fn name(self) -> &'static str {
        match self {
            Self::None => "none", Self::RequestedExit => "requested_exit", Self::Standby => "standby",
            Self::Suspend => "suspend", Self::Refresh => "refresh", Self::Changed => "changed",
            Self::Exhausted => "exhausted",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Event { Pending, Cancelled, RetryStart, DiscoveryWait }
impl Event {
    fn name(self) -> &'static str {
        match self { Self::Pending => "pending", Self::Cancelled => "cancelled", Self::RetryStart => "retry_start", Self::DiscoveryWait => "discovery_wait" }
    }
}

impl Pending {
    pub fn line(self, event: Event, reason: Reason, now_ms: u64, new_generation: u64) -> String {
        format!("ott-continuity event={} catalog_generation={} failed_play_generation={} failed_play_generation_known={} row_ordinal={} failures={} now_ms={} not_before_ms={} new_catalog_generation={} reason={}",
            event.name(), self.catalog_generation, self.failed_play_generation.unwrap_or(0),
            u8::from(self.failed_play_generation.is_some()), self.row_ordinal, self.failures,
            now_ms, self.not_before_ms, new_generation, reason.name())
    }
    pub fn emit(self, event: Event, reason: Reason, now_ms: u64, new_generation: u64) {
        plx_base::eventlog::log(&self.line(event, reason, now_ms, new_generation));
    }
}
