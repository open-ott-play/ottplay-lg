//! Geometry presentation contracts; native submission is qualified by the application tests.
use super::*;
fn menu(value: Option<Transform>) -> MoreMenuState {
    plx_media::ff::force_speed_decoder_for_test(true);
    let mut menu = MoreMenuState::new(&plx_media::route::PlaybackSession::IDLE);
    menu.set_geometry(&plx_media::route::PlaybackSession::IDLE, value, false, false);
    menu
}
fn enter(menu: &mut MoreMenuState, row: MoreRow) {
    assert!(menu.focus_key(row.key().0));
    assert_eq!(menu.on_ok(&plx_media::route::PlaybackSession::IDLE), MoreOk::Navigated);
}
fn checked(menu: &mut MoreMenuState, row: MoreRow) -> bool {
    let i = menu.form.index_of(&row).expect("displayed geometry row");
    menu.form.table.row_mut(i as i32).unwrap().checked
}
#[test]
fn geometry_modes_and_zoom_are_independent_and_only_accepted_values_are_checked() {
    let _lock = plx_base::testlock::serial();
    let ps = &plx_media::route::PlaybackSession::IDLE;
    let old = Transform { mode: PictureMode::Fit, zoom: Zoom::X150 };
    let mut menu = menu(Some(old));
    enter(&mut menu, MoreRow::OpenPicture);
    assert!(checked(&mut menu, MoreRow::Picture(PictureMode::Fit)));
    assert!(menu.focus_key(MoreRow::Picture(PictureMode::Fill).key().0));
    let desired = Transform { mode: PictureMode::Fill, ..old };
    assert_eq!(menu.on_ok(ps), MoreOk::Geometry(desired));
    assert!(checked(&mut menu, MoreRow::Picture(PictureMode::Fit)), "asking changes no accepted check");
    menu.set_geometry(ps, Some(old), true, false);
    assert_eq!(menu.on_ok(ps), MoreOk::Inert, "a pending request is not repeated");
    assert!(checked(&mut menu, MoreRow::Picture(PictureMode::Fit)));
    menu.set_geometry(ps, Some(old), false, true);
    assert!(checked(&mut menu, MoreRow::Picture(PictureMode::Fit)), "rejection retains the previous value");
    assert_eq!(menu.on_ok(ps), MoreOk::Geometry(desired), "explicit retry is available");
    menu.set_geometry(ps, Some(desired), false, false);
    assert!(checked(&mut menu, MoreRow::Picture(PictureMode::Fill)));
    assert!(menu.pop(ps));
    assert_eq!(menu.sel_id(), Some(MoreRow::OpenPicture));
    enter(&mut menu, MoreRow::OpenZoom);
    assert!(checked(&mut menu, MoreRow::Zoom(Zoom::X150)));
    assert!(menu.focus_key(MoreRow::Zoom(Zoom::X125).key().0));
    assert_eq!(menu.on_ok(ps), MoreOk::Geometry(Transform { zoom: Zoom::X125, ..desired }));
}
#[test]
fn geometry_offer_removal_pops_the_page_and_removes_both_openers() {
    let _lock = plx_base::testlock::serial();
    for row in [MoreRow::OpenPicture, MoreRow::OpenZoom] {
        let mut menu = menu(None);
        assert!(!menu.keys().contains(&row.key().0));
        menu.set_geometry(&plx_media::route::PlaybackSession::IDLE, Some(Transform::default()), false, false);
        enter(&mut menu, row);
        menu.set_geometry(&plx_media::route::PlaybackSession::IDLE, None, false, false);
        assert_eq!(menu.page(), None);
        for absent in [MoreRow::OpenPicture, MoreRow::OpenZoom, MoreRow::Picture(PictureMode::Fit), MoreRow::Zoom(Zoom::X100)] {
            assert!(!menu.focus_key(absent.key().0));
        }
    }
}
#[test]
fn geometry_page_back_restores_each_exact_opener_and_codes_stay_distinct() {
    let _lock = plx_base::testlock::serial();
    let mut menu = menu(Some(Transform::default()));
    for (row, page) in [(MoreRow::OpenPicture, MorePage::Picture), (MoreRow::OpenZoom, MorePage::Zoom)] {
        enter(&mut menu, row);
        assert_eq!(menu.page(), Some(page));
        assert!(menu.pop(&plx_media::route::PlaybackSession::IDLE));
        assert_eq!(menu.sel_id(), Some(row));
    }
    let keys = [MoreRow::OpenPicture, MoreRow::OpenZoom, MoreRow::Picture(PictureMode::Fit),
        MoreRow::Picture(PictureMode::Fill), MoreRow::Zoom(Zoom::X100), MoreRow::Zoom(Zoom::X125),
        MoreRow::Zoom(Zoom::X150), MoreRow::Zoom(Zoom::X175)].map(|r| r.key().0);
    assert_eq!(keys.iter().copied().collect::<std::collections::BTreeSet<_>>().len(), keys.len());
    assert_eq!(MorePage::Quality.code(), 1); assert_eq!(MorePage::Speed.code(), 2);
}
#[test]
fn geometry_forms_and_statuses_fit_all_shipped_locales() {
    let _lock = plx_base::testlock::serial();
    let ps = &plx_media::route::PlaybackSession::IDLE;
    let mut failures = Vec::new();
    for language in plx_platform::i18n::SHIPPED {
        let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
        for mode in [PictureMode::Fit, PictureMode::Fill] {
            for zoom in [Zoom::X100, Zoom::X125, Zoom::X150, Zoom::X175] {
                for row in [None, Some(MoreRow::OpenPicture), Some(MoreRow::OpenZoom)] {
                    let value = Transform { mode, zoom };
                    let mut menu = menu(Some(value));
                    if let Some(row) = row { enter(&mut menu, row); }
                    for (pending, failed) in [(false, false), (true, false), (false, true)] {
                        menu.set_geometry(ps, Some(value), pending, failed);
                        let table = &menu.form.table;
                        failures.extend(table.menu_cap_failure(&plx_base::fontcov::advances::ShippedMeasure, language.tag()));
                        failures.extend(table.app_fit_failures(plx_ui::table::MENU_MAX_W, language.tag()));
                        failures.extend(table.app_fit_failures_hugged(language.tag()));
                        assert!(table.measured_height() <= NEED_H, "geometry page exceeds shared placement height: {}", language.tag());
                    }
                }
            }
        }
    }
    plx_ui::table::assert_no_fit_failures(&failures);
}
