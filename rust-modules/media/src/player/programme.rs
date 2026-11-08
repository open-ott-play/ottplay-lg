//! Logical playback programme clock. The app supplies presentation evidence and
//! its existing raw wall-clock sample; this value never reads a clock or player.
#[derive(Clone, Debug)]
pub struct ProgrammeClock<K> {
    identity: Option<K>,
    started_at: Option<i64>,
}
impl<K> Default for ProgrammeClock<K> {
    fn default() -> Self {
        Self {
            identity: None,
            started_at: None,
        }
    }
}
impl<K: PartialEq> ProgrammeClock<K> {
    /// Evidence must belong to the current committed load, never an outgoing
    /// frame while a new logical request is resolving. No timestamp is guessed.
    pub fn observe(&mut self, identity: Option<K>, presented: bool, unix_ms: Option<i64>) -> bool {
        let changed = self.identity != identity;
        if changed {
            self.identity = identity;
            self.started_at = None;
        }
        if self.identity.is_some() && self.started_at.is_none() && presented {
            if let Some(time) = unix_ms.filter(|time| *time > 0) {
                self.started_at = Some(time);
                return true;
            }
        }
        changed
    }
    pub fn started_at(&self, identity: &K) -> Option<i64> {
        (self.identity.as_ref() == Some(identity))
            .then_some(self.started_at)
            .flatten()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_current_presentation_starts_a_programme() {
        let mut clock = ProgrammeClock::default();
        assert!(clock.observe(Some(1), false, Some(1000)));
        assert_eq!(clock.started_at(&1), None);
        assert!(clock.observe(Some(1), true, Some(2000)));
        assert_eq!(clock.started_at(&1), Some(2000));
        assert_eq!(clock.started_at(&2), None);
        assert!(clock.observe(Some(2), false, Some(3000)));
        assert_eq!(clock.started_at(&2), None);
    }
    #[test]
    fn seek_pause_reload_and_wall_clock_adjustment_preserve_logical_start() {
        let mut clock = ProgrammeClock::default();
        clock.observe(Some(1), true, Some(2000));
        for (presented, time) in [(false, 3000), (true, 500), (true, 9000)] {
            assert!(!clock.observe(Some(1), presented, Some(time)));
            assert_eq!(clock.started_at(&1), Some(2000));
        }
        assert!(clock.observe(None, false, None));
        assert_eq!(clock.started_at(&1), None);
    }
    #[test]
    fn absent_or_invalid_time_is_not_an_epoch_placeholder() {
        let mut clock = ProgrammeClock::default();
        for time in [None, Some(0), Some(-1)] {
            clock.observe(Some(1), true, time);
            assert_eq!(clock.started_at(&1), None);
        }
        assert!(clock.observe(Some(1), true, Some(42)));
        assert_eq!(clock.started_at(&1), Some(42));
    }
}
