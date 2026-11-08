//! Menu-entry behavior, using numeric keys to remain runnable against the pre-fix base.
use super::*;

// Existing More keys 0..7 are preserved; these two formerly absent keys name the new entries.
const AUDIO: RowKey = RowKey(8);
const SUBTITLES: RowKey = RowKey(9);

#[test]
fn more_offers_both_track_tabs_without_changing_existing_opening_row() {
    let _lock = plx_base::testlock::serial();
    let ps = plx_media::route::PlaybackSession::IDLE;
    for forced in [false, true] {
        let rows = rows_for(forced);
        let mut form = MoreTable::new(plx_ui::table_screen::BAND_BASE);
        form.set_or_open(root_form_for(&ps, &rows, forced, plx_media::route::Quality::Auto,
            1000, &[1000], Default::default(), None), None);
        for (key, label) in [(AUDIO, plx_platform::i18n::msg::widgets_tracks_audio()),
            (SUBTITLES, plx_platform::i18n::msg::widgets_tracks_subtitles())] {
            let i = form.index_of_key(key).expect("More must offer both track menus");
            let Some(Activation::Action(action)) = form.activate(i) else { panic!("track entry must be actionable") };
            assert_eq!(super::label(action), label);
            assert_eq!(action.key(), key);
        }
        let expected = if forced { Action::ToggleStats.key() } else { MoreRow::OpenQuality.key() };
        assert_eq!(form.key_at(0), Some(expected), "existing opening row is preserved");
        let keys: Vec<_> = (0..form.table.n_rows() as usize).filter_map(|i| form.key_at(i)).collect();
        for (i, key) in keys.iter().enumerate() { assert!(!keys[..i].contains(key), "each row owns a distinct key"); }
    }
}

#[test]
fn more_track_visibility_is_independent_and_does_not_retarget_missing_keys() {
    let _lock = plx_base::testlock::serial();
    let ps = plx_media::route::PlaybackSession::IDLE;
    for (hidden, missing, present) in [(crate::menu_visibility::Item::Audio, AUDIO, SUBTITLES),
        (crate::menu_visibility::Item::Subtitles, SUBTITLES, AUDIO)] {
        let mut visibility = crate::menu_visibility::Visibility::default();
        visibility.hide(hidden);
        let mut form = MoreTable::new(plx_ui::table_screen::BAND_BASE);
        form.set_or_open(root_form_for(&ps, &rows_for(false), false,
            plx_media::route::Quality::Auto, 1000, &[1000], visibility, None), None);
        assert!(form.index_of_key(missing).is_none());
        assert!(form.index_of_key(present).is_some(), "hiding one tab must retain the other");
        assert!(form.index_of_key(Action::ToggleStats.key()).is_some());
    }
}
