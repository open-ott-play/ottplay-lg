//! Retained More presentation: locale changes neither rebuild ownership nor commit actions.
use super::*;
use plx_platform::i18n::{language_on_this_thread_for_test, Preference};

fn digest(menu: &MoreMenuState) -> u64 {
    let mut c = plx_machine::machine::Canon::new();
    menu.canon(&mut c); c.finish()
}
fn row(menu: &MoreMenuState, id: MoreRow) -> &Row {
    let index = menu.form.index_of(&id).expect("retained row");
    menu.form.table.sections.iter().flat_map(|section| &section.rows).nth(index).unwrap()
}
fn menu() -> MoreMenuState {
    let ps = plx_media::route::PlaybackSession::IDLE;
    let mut menu = MoreMenuState::new(&ps);
    menu.set_catalog_available(&ps, true);
    menu.set_playback_capabilities(&ps, PlaybackMenuCapabilities { guide: true, categories: true, ..Default::default() });
    menu.visibility = Default::default();
    menu.rebuild(&ps);
    menu
}

#[test]
fn root_relabels_existing_form_without_changing_actions_or_feedback() {
    let _lock = plx_base::testlock::serial();
    let _en = language_on_this_thread_for_test(Preference::En);
    let ps = plx_media::route::PlaybackSession::IDLE;
    let mut menu = menu();
    menu.set_playback_failure(&ps, Some((PlaybackMenuAction::Guide, "retained owner reason")));
    let id = MoreRow::Act(Action::OpenSettings);
    assert!(menu.focus_key(id.key().0));
    let form = menu.root_form();
    menu.form.restore(form, Some(&id), 43.0);
    let old = row(&menu, id).label.clone();
    let before = digest(&menu);
    let before_scroll = menu.form.table.scroll_pos();
    let before_anchor = menu.anchor;
    let _ru = language_on_this_thread_for_test(Preference::Ru);
    menu.relabel();
    assert_ne!(row(&menu, id).label, old);
    assert_eq!(row(&menu, id).label, plx_platform::i18n::msg::settings_title());
    assert_eq!(digest(&menu), before);
    assert_eq!(menu.sel_id(), Some(id));
    assert_eq!(menu.form.table.scroll_pos(), before_scroll);
    assert_eq!(menu.anchor, before_anchor);
    assert_eq!(menu.playback_failure_note(), Some("retained owner reason"));
    assert_eq!(menu.on_ok(&ps), MoreOk::Action(Action::OpenSettings));
}

#[test]
fn every_open_page_relabels_in_place_and_preserves_pending_geometry() {
    let _lock = plx_base::testlock::serial();
    for page in [MorePage::Quality, MorePage::Speed, MorePage::Picture, MorePage::Zoom] {
        let _en = language_on_this_thread_for_test(Preference::En);
        let ps = plx_media::route::PlaybackSession::IDLE;
        let mut menu = menu();
        menu.rows = vec![Action::SetQuality(plx_media::route::Quality::Original), Action::SetQuality(plx_media::route::Quality::P720)];
        menu.forced = false;
        menu.speeds = plx_media::player::PLAY_RATES.to_vec();
        menu.set_geometry(&ps, Some(Transform { mode: PictureMode::Fit, zoom: Zoom::X125 }), true, false);
        let id = match page { MorePage::Quality => MoreRow::OpenQuality, MorePage::Speed => MoreRow::OpenSpeed,
            MorePage::Picture => MoreRow::OpenPicture, MorePage::Zoom => MoreRow::OpenZoom };
        assert!(menu.focus_key(id.key().0));
        menu.push(&ps, page);
        let selected = menu.sel_id();
        let form = menu.page_form(page);
        menu.form.restore(form, selected.as_ref(), 57.0);
        let before = digest(&menu);
        let old_title = menu.form.table.title().unwrap().to_string();
        let before_scroll = menu.form.table.scroll_pos();
        let _ru = language_on_this_thread_for_test(Preference::Ru);
        menu.relabel();
        assert_eq!(menu.page(), Some(page));
        assert_eq!(menu.form.table.title(), Some(page.title().as_str()));
        assert_ne!(menu.form.table.title(), Some(old_title.as_str()));
        assert_eq!(menu.form.table.scroll_pos(), before_scroll);
        assert_eq!(digest(&menu), before);
        assert!(menu.geometry_pending);
        assert_eq!(menu.geometry, Some(Transform { mode: PictureMode::Fit, zoom: Zoom::X125 }));
        assert!(menu.pop(&ps));
        assert_eq!(menu.sel_id(), Some(id), "relabel must retain the page's return target");
    }
}

#[test]
fn relabel_uses_the_accepted_facts_instead_of_polling_settings_or_playback() {
    let _lock = plx_base::testlock::serial();
    let _en = language_on_this_thread_for_test(Preference::En);
    let mut menu = menu();
    menu.current = plx_media::route::Quality::Original;
    menu.presentation = MorePresentationFacts { playing_quality: plx_media::route::PlayingQuality::Converted,
        source_decodable: false, stats_on: true };
    menu.rows = vec![Action::SetQuality(plx_media::route::Quality::Original), Action::ToggleStats];
    menu.forced = false;
    let _ru = language_on_this_thread_for_test(Preference::Ru);
    menu.relabel();
    let expected = format!("{} · {}", menu.current.short_label(), plx_platform::i18n::msg::widgets_menu_quality_converted());
    assert_eq!(row(&menu, MoreRow::OpenQuality).value.as_deref(), Some(expected.as_str()));
    // The live stats reader defaults false in this fixture; the accepted true fact must survive.
    assert_eq!(row(&menu, MoreRow::Act(Action::ToggleStats)).toggle, Some(true));
    assert_eq!(menu.presentation.playing_quality, plx_media::route::PlayingQuality::Converted);
    assert!(!menu.presentation.source_decodable);
    assert!(menu.presentation.stats_on);
}

#[test]
fn modern_and_classic_paint_the_new_title_without_the_departing_language() {
    let _lock = plx_base::testlock::serial();
    for mode in [UiMode::Modern, UiMode::Classic] {
        let _en = language_on_this_thread_for_test(Preference::En);
        let ps = plx_media::route::PlaybackSession::IDLE;
        let mut menu = menu();
        menu.rows = vec![Action::SetQuality(plx_media::route::Quality::Original),
            Action::SetQuality(plx_media::route::Quality::P720), Action::OpenSettings];
        menu.forced = false;
        menu.rebuild(&ps);
        assert!(menu.focus_key(MoreRow::OpenQuality.key().0));
        menu.push(&ps, MorePage::Quality);
        let old_title = menu.form.table.title().unwrap().to_string();
        let old_settings = plx_platform::i18n::msg::settings_title().to_string();
        let before = digest(&menu);
        let _ru = language_on_this_thread_for_test(Preference::Ru);
        menu.relabel();
        let paint = plx_ui::draw_census::capture_text(|| plx_ui::record_walk(||
            menu.draw_for_mode(1.0, mode, &plx_ui::fixture::FixtureMeasure)));
        // The shared Modern title band uppercases its text; the Classic caption does not.
        let title = plx_platform::i18n::msg::widgets_menu_quality();
        let painted_title = match mode {
            UiMode::Modern => title.to_uppercase(),
            UiMode::Classic => title.to_string(),
        };
        assert!(paint.iter().any(|(text, _)| text == &painted_title), "{mode:?}");
        let old_uppercase_title = old_title.to_uppercase();
        assert!(!paint.iter().any(|(text, _)| text == &old_title || text == &old_uppercase_title || text == &old_settings), "departing table must not paint old-language text: {mode:?}");
        assert_eq!(digest(&menu), before);
    }
}
