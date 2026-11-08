use super::*;

fn row(id: &str, title: &str, action: &str, enabled: bool) -> OttRow {
    OttRow {
        current_programme: None, content_title: None,
        guide: None,
        guide_available: false, records_available: false,
        media: None,
        id: id.into(),
        title: title.into(),
        detail: String::new(),
        description: String::new(),
        poster: None,
        action: OttAction::EditSetting(action.into()),
        enabled,
        favorite: false,
    }
}
fn snapshot(revision: u64, waiting: bool, added: bool) -> Arc<OttView> {
    let mut rows = vec![row(
        "usb.cancel",
        "Cancel",
        "local.usb.browse:7:0:cancel",
        true,
    )];
    if added {
        rows.push(row(
            "usb.folder:/A",
            "A",
            "local.usb.browse:8:0:enter:0",
            !waiting,
        ));
    }
    rows.push(row(
        "usb.folder:/B",
        "B",
        "local.usb.browse:8:0:enter:1",
        !waiting,
    ));
    Arc::new(OttView {
        page_id: "usb-folders:7:".into(),
        revision,
        title: "USB folders".into(),
        status: if waiting {
            "Reading folders…".into()
        } else {
            String::new()
        },
        rows,
        ..OttView::default()
    })
}

#[test]
fn enlarged_picker_keeps_folder_key_and_pending_form_keeps_cancel_action() {
    let mut screen = OttScreen::new(EntryId(127), InstanceId(127));
    screen.install(snapshot(1, false, false));
    let key = screen.keys["item:usb.folder:/B"];
    screen.install(snapshot(2, true, false));
    assert_eq!(screen.keys["item:usb.folder:/B"], key);
    let cancel = screen
        .form
        .index_of_key(RowKey(screen.keys["item:usb.cancel"]))
        .unwrap();
    assert!(
        matches!(screen.form.activate(cancel), Some(Activation::Action(
        FormAction::Forward(OttAction::EditSetting(action)))) if action == "local.usb.browse:7:0:cancel")
    );
    let folder = screen.form.index_of_key(RowKey(key)).unwrap();
    assert!(!matches!(
        screen.form.activate(folder),
        Some(Activation::Action(FormAction::Forward(
            OttAction::EditSetting(_)
        )))
    ));
    screen.install(snapshot(3, false, true));
    assert_eq!(screen.keys["item:usb.folder:/B"], key);
    let folder = screen.form.index_of_key(RowKey(key)).unwrap();
    assert!(
        matches!(screen.form.activate(folder), Some(Activation::Action(
        FormAction::Forward(OttAction::EditSetting(action)))) if action == "local.usb.browse:8:0:enter:1")
    );
}

#[test]
fn usb_picker_controls_fit_the_existing_form_in_all_shipped_languages() {
    let mut failures = Vec::new();
    let width = RouteLayout::screen().sectioned_table().w;
    for language in plx_platform::i18n::SHIPPED {
        let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
        let cx = plx_platform::i18n::LocaleContext::resolve(
            plx_platform::i18n::Preference::from_tag(language.tag()),
            None,
            None,
            None,
            None,
        );
        let labels = [
            ("use", msg::ott_usb_browser_use_in(&cx)),
            ("parent", msg::ott_usb_browser_parent_in(&cx)),
            ("root", msg::ott_usb_browser_root_in(&cx)),
            ("previous", msg::ott_usb_browser_previous_in(&cx)),
            ("next", msg::ott_usb_browser_next_in(&cx)),
            ("more", msg::ott_usb_browser_more_in(&cx)),
            ("browse", msg::ott_usb_browser_browse_root_in(&cx)),
        ];
        let mut screen = OttScreen::new(EntryId(128), InstanceId(128));
        screen.install(Arc::new(OttView {
            page_id: "usb-folders:7:".into(),
            title: msg::ott_usb_browser_title_in(&cx).into(),
            rows: labels
                .iter()
                .map(|(id, title)| {
                    row(
                        &format!("usb.{id}"),
                        title,
                        &format!("local.usb.browse:7:0:{id}"),
                        true,
                    )
                })
                .collect(),
            ..OttView::default()
        }));
        failures.extend(screen.form.table.app_fit_failures(width, language.tag()));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
