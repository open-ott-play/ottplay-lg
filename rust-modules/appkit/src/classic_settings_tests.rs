use crate::classic_settings::Preferences;

#[test]
fn classic_preferences_defaults_and_checked_domains() {
    let p = Preferences::default();
    assert_eq!((p.page_size(), p.list_left(), p.arrow_mode(), p.seek13_seconds()), (25, false, 0, 15));
    for size in [10, 25, 30] {
        for side in 0..=1 {
            for mode in 0..=3 {
                let p = Preferences::from_values(size, side, mode, 15).unwrap();
                assert_eq!((p.page_size(), p.list_left(), p.arrow_mode()), (size, side == 1, mode));
            }
        }
    }
    for size in [0, 9, 31, usize::MAX] { assert!(Preferences::from_values(size, 0, 0, 15).is_none()); }
    for side in [2, u8::MAX] { assert!(Preferences::from_values(25, side, 0, 15).is_none()); }
    for mode in [4, u8::MAX] { assert!(Preferences::from_values(25, 0, mode, 15).is_none()); }
}

#[test]
fn seek13_duration_accepts_only_the_classic_seconds_choices() {
    for seconds in [5, 10, 15, 20, 30, 60, 120, 180, 240, 300, 600, 900, 1200, 1800, 3600] {
        assert_eq!(Preferences::from_values(25, 0, 0, seconds).unwrap().seek13_seconds(), seconds);
    }
    for seconds in [0, 1, 4, 6, 16, 59, 3601, u32::MAX] {
        assert!(Preferences::from_values(25, 0, 0, seconds).is_none());
    }
}

#[test]
fn legacy_arrow_three_is_preserved_without_changing_other_preferences() {
    let legacy = Preferences::from_values(30, 1, 3, 20).unwrap();
    assert_eq!(legacy.arrow_mode(), 3);
    assert_eq!((legacy.page_size(), legacy.list_left(), legacy.seek13_seconds()), (30, true, 20));
    assert_ne!(legacy, Preferences::from_values(30, 1, 0, 20).unwrap());
}

#[test]
fn visibility_defaults_remain_on_and_every_boolean_is_checked() {
    let all = Preferences::default();
    assert!(all.show_number() && all.show_name() && all.show_program() && all.show_description());
    for field in 0..4 {
        let mut values = [1; 4]; values[field] = 0;
        let value = all.with_channel_display(values[0], values[1], values[2], values[3]).unwrap();
        assert_eq!([value.show_number(), value.show_name(), value.show_program(), value.show_description()], values.map(|v|v==1));
        values[field] = 2;
        assert!(all.with_channel_display(values[0], values[1], values[2], values[3]).is_none());
    }
    assert_eq!(all.page_size(), 25);
}

#[test]
fn progress_visibility_is_checked_and_independent_of_programme_text() {
    let defaults = Preferences::default();
    assert!(defaults.show_progress());
    for program in 0..=1 {
        for progress in 0..=1 {
            let value = defaults.with_channel_display(1, 1, program, 1).unwrap()
                .with_show_progress(progress).unwrap();
            assert_eq!(value.show_program(), program == 1);
            assert_eq!(value.show_progress(), progress == 1);
            assert_eq!(value.with_channel_display(0, 0, 1 - program, 0).unwrap().show_progress(), progress == 1);
        }
    }
    for invalid in [2, u8::MAX] { assert!(defaults.with_show_progress(invalid).is_none()); }
}
