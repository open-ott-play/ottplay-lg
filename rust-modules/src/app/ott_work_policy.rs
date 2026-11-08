//! Playback admission from the existing foreground and standby owners.
//! Cancellation and admission of other work remain their runner's duties.

pub(super) fn playback_allowed(foreground: bool, standby: bool) -> bool {
    foreground && !standby
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_requires_foreground_and_no_standby() {
        for (foreground, standby) in [(true, true), (false, false), (false, true)] {
            assert!(!playback_allowed(foreground, standby));
        }
    }

    #[test]
    fn clearing_standby_does_not_override_the_os_foreground_authority() {
        assert!(!playback_allowed(false, false));
        assert!(playback_allowed(true, false));
    }
}
