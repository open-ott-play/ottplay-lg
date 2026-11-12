//! Organization pages reuse the native OTT FormTable and addressed text editor.
//! Action keys capture the runtime session revision; display rows retain identity.
use super::*;
use organization_runtime::{Mode, View};
use plx_platform::i18n::msg;

fn row(id: String, title: String, action: OttAction, enabled: bool) -> OttRow {
    OttRow { current_programme: None, content_title: None, guide: None, guide_available: false, records_available: false,
        id, title, action, enabled, media: None, detail: String::new(),
        description: String::new(), poster: None, favorite: false,
    }
}

pub(super) fn entry(runtime: &Runtime) -> Option<OttRow> {
    runtime.organization_available().then(|| row(
        "organization.open".into(), msg::ott_organization_manage().into(),
        OttAction::EditSetting("organization.open".into()), true,
    ))
}

pub(super) fn view(runtime: &Runtime) -> OttView {
    if let Some(view) = runtime.organization_view() {
        return build(&view, runtime.revision, &runtime.status);
    }
    let mut rows = Vec::new();
    if let Some(open) = entry(runtime) { rows.push(open); }
    rows.push(row("organization.back".into(), msg::settings_back().into(),
        OttAction::Back, true));
    OttView {
        preview_context: None,
        order_context: None,
        page_id: "organization:unavailable".into(), revision: runtime.revision,
        title: msg::ott_organization_manage().into(),
        description: msg::ott_organization_reopen().into(), rows,
        ..OttView::default()
    }
}

fn build(view: &View, revision: u64, status: &str) -> OttView {
    let mut rows = Vec::new();
    let mut action = |operation: &str, title: &str, allowed_while_pending: bool| {
        rows.push(row(format!("organization.{operation}"), title.into(),
            OttAction::EditSetting(view.key(operation)),
            (!view.pending || allowed_while_pending)
                && (!view.alphabetical || !matches!(operation, "member.up" | "member.down"))));
    };
    let (title, description) = match view.mode {
        Mode::Categories => {
            action("create", msg::ott_organization_create(), false);
            action("reset", msg::ott_organization_reset(), false);
            action("back", msg::settings_done(), true);
            (msg::ott_organization_manage().to_owned(), msg::ott_organization_local().to_owned())
        }
        Mode::Category => {
            let all = view.category.as_ref().is_some_and(|category| category.id == catalog_organization::ALL_ID);
            if !all { action("rename", msg::ott_organization_rename(), false); }
            action("copy", msg::ott_organization_copy(), false);
            if !all {
                action("up", msg::ott_organization_up(), false);
                action("down", msg::ott_organization_down(), false);
                action("delete", msg::ott_organization_delete(), false);
            }
            action("member.add", msg::ott_organization_add(), false);
            action("back", msg::settings_back(), true);
            (view.category.as_ref().map(|category| category.title.clone()).unwrap_or_default(),
                msg::ott_organization_members().to_owned())
        }
        Mode::Item => {
            action("member.up", msg::ott_organization_up(), false);
            action("member.down", msg::ott_organization_down(), false);
            action("member.remove", msg::ott_organization_remove(), false);
            action("back", msg::settings_back(), true);
            (view.item.as_ref().map(|item| item.title.clone()).unwrap_or_default(),
                view.category.as_ref().map(|category| category.title.clone()).unwrap_or_default())
        }
        Mode::AddItem => {
            action("back", msg::settings_back(), true);
            (msg::ott_organization_add().to_owned(),
                view.category.as_ref().map(|category| category.title.clone()).unwrap_or_default())
        }
        Mode::ConfirmDelete => {
            action("back", msg::settings_cancel(), true);
            action("confirm", msg::ott_organization_delete(), false);
            (msg::ott_organization_confirm_delete().to_owned(),
                msg::ott_organization_keep_media().to_owned())
        }
        Mode::ConfirmReset => {
            action("back", msg::settings_cancel(), true);
            action("confirm", msg::ott_organization_reset(), false);
            (msg::ott_organization_confirm_reset().to_owned(),
                msg::ott_organization_keep_media().to_owned())
        }
    };
    match view.mode {
        Mode::Categories => {
            for (index, category) in view.categories.iter().enumerate() {
                rows.push(row(format!("organization.category:{}", category.id), category.title.clone(),
                    OttAction::EditSetting(view.key(&format!("category:{index}"))), !view.pending));
            }
        }
        Mode::Category | Mode::AddItem => {
            let operation = if matches!(view.mode, Mode::AddItem) { "add" } else { "item" };
            for (index, item) in view.items.iter().enumerate() {
                rows.push(row(format!("organization.item:{}", item.id), item.title.clone(),
                    OttAction::EditSetting(view.key(&format!("{operation}:{index}"))), !view.pending));
            }
        }
        _ => {}
    }
    let mode = match view.mode {
        Mode::Categories => "categories", Mode::Category => "category", Mode::Item => "item",
        Mode::AddItem => "add", Mode::ConfirmDelete => "delete", Mode::ConfirmReset => "reset",
    };
    let category = view.category.as_ref().map_or("", |category| category.id.as_str());
    let item = view.item.as_ref().map_or("", |item| item.id.as_str());
    OttView { pending_play: None, pending_step: None, pending_step_sdl: false, shelf_context: None, shelves: Vec::new(), status_working: false, classic_preferences: Default::default(), volume_available: false, category_label: None,
        preview_context: None,
        page_id: format!("organization:{mode}:{category}:{item}"), revision, title, description,
        status: status.into(), selected_row: rows.first().map(|row| row.id.clone()),
        rows, search: None, catalog_progress: None, random_context: None, order_context: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use organization_runtime::Row;

    fn fixture(mode: Mode) -> View {
        View {
            revision: 41, mode, pending: false, alphabetical: false,
            categories: vec![Row { id: "provider:stable".into(), title: "Family".into() }],
            items: vec![Row { id: "m3u-1:0123456789abcdef".into(), title: "Fixture TV".into() }],
            category: Some(Row { id: "provider:stable".into(), title: "Family".into() }),
            item: Some(Row { id: "m3u-1:0123456789abcdef".into(), title: "Fixture TV".into() }),
        }
    }

    #[test]
    fn alphabetical_manage_disables_member_moves_but_keeps_category_moves_and_removal() {
        let _serial = plx_base::testlock::serial();
        for alphabetical in [false, true] {
            for pending in [false, true] {
                let mut state = fixture(Mode::Item);
                state.alphabetical = alphabetical;
                state.pending = pending;
                let view = build(&state, 1, "");
                for operation in ["member.up", "member.down", "member.remove", "back"] {
                    let row = view.rows.iter().find(|row| row.id == format!("organization.{operation}")).unwrap();
                    let enabled = operation == "back" || (!pending
                        && (!alphabetical || operation == "member.remove"));
                    assert_eq!(row.enabled, enabled, "{operation}, alpha={alphabetical}, pending={pending}");
                    assert_eq!(row.action, OttAction::EditSetting(format!("organization.41.{operation}")));
                }
                state.mode = Mode::Category;
                let view = build(&state, 2, "");
                for operation in ["up", "down", "member.add"] {
                    assert_eq!(view.rows.iter().find(|row| row.id == format!("organization.{operation}")).unwrap().enabled,
                        !pending, "category action {operation} must not inherit member sorting restrictions");
                }
            }
        }
    }

    #[test]
    fn organization_rows_keep_identity_but_actions_capture_revision() {
        let _serial = plx_base::testlock::serial();
        let mut state = fixture(Mode::Category);
        let first = build(&state, 1, "");
        state.revision += 1;
        let second = build(&state, 2, "");
        let row_id = "organization.item:m3u-1:0123456789abcdef";
        let first = first.rows.iter().find(|row| row.id == row_id).unwrap();
        let second = second.rows.iter().find(|row| row.id == row_id).unwrap();
        assert_eq!(first.action, OttAction::EditSetting("organization.41.item:0".into()));
        assert_eq!(second.action, OttAction::EditSetting("organization.42.item:0".into()));
        assert_eq!(first.title, "Fixture TV");
    }

    #[test]
    fn organization_confirmations_open_on_cancel_and_keep_media_copy() {
        let _serial = plx_base::testlock::serial();
        for mode in [Mode::ConfirmDelete, Mode::ConfirmReset] {
            let view = build(&fixture(mode), 1, "");
            assert_eq!(view.selected_row.as_deref(), Some("organization.back"));
            assert_eq!(view.rows.len(), 2);
            assert_eq!(view.rows[0].action, OttAction::EditSetting("organization.41.back".into()));
            assert_eq!(view.rows[1].action, OttAction::EditSetting("organization.41.confirm".into()));
            assert_eq!(view.description, msg::ott_organization_keep_media());
        }
    }

    #[test]
    fn organization_pending_form_keeps_exit_and_disables_mutations() {
        let _serial = plx_base::testlock::serial();
        for mode in [Mode::Categories, Mode::Category, Mode::Item, Mode::AddItem,
            Mode::ConfirmDelete, Mode::ConfirmReset]
        {
            let mut state = fixture(mode);
            state.pending = true;
            let view = build(&state, 1, "");
            let enabled: Vec<_> = view.rows.iter().filter(|row| row.enabled).map(|row| row.id.as_str()).collect();
            assert_eq!(enabled, vec!["organization.back"]);
        }
    }

    #[test]
    fn organization_member_picker_dispatches_add_without_starting_playback() {
        let _serial = plx_base::testlock::serial();
        let view = build(&fixture(Mode::AddItem), 1, "");
        let member = view.rows.iter().find(|row| row.id.starts_with("organization.item:")).unwrap();
        assert_eq!(member.action, OttAction::EditSetting("organization.41.add:0".into()));
        assert!(view.rows.iter().all(|row| !matches!(row.action, OttAction::Play(_))));
        assert!(view.search.is_none());
        assert!(view.catalog_progress.is_none());
    }

    #[test]
    fn organization_all_keeps_membership_and_copy_but_protects_category_identity() {
        let _serial = plx_base::testlock::serial();
        let mut state = fixture(Mode::Category);
        state.category = Some(Row { id: catalog_organization::ALL_ID.into(), title: "All".into() });
        let view = build(&state, 1, "");
        for operation in ["rename", "delete", "up", "down"] {
            assert!(view.rows.iter().all(|row| row.id != format!("organization.{operation}")));
        }
        for operation in ["copy", "member.add", "back"] {
            assert!(view.rows.iter().any(|row| row.id == format!("organization.{operation}") && row.enabled));
        }
    }
}
