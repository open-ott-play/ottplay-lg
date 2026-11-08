//! Presentation of the existing Settings actions, never a catalogue/navigation policy.
use super::{OttAction, OttRow, OttView};

pub(super) fn applies(view: &OttView) -> bool {
    // Root and grouped draft pages share "settings". Real text editors are
    // excluded by their owner; provider setup/catalogue pages stay unchanged.
    !view.rows.is_empty() && matches!(view.page_id.as_str(),
        "settings" | "interface-mode" | "osd-opacity" | "permanent-clock"
        | "classic-page-size" | "classic-list-position" | "classic-list-arrows"
        | "numeric-seek-step" | "classic-show-number" | "classic-show-name"
        | "classic-show-program" | "classic-show-progress" | "classic-show-description" | "menu-items")
}

pub(super) fn value(row: &OttRow) -> Option<&str> {
    // These owner-defined actions carry help, not a current setting value.
    // Use the command identity, never a translated title/detail comparison.
    let help = matches!(&row.action, OttAction::EditSetting(key)
        if key.starts_with("settings.open:") || key.strip_prefix("settings.draft:")
            .and_then(|rest| rest.split_once(':')).is_some_and(|(_, command)| command == "reset-menu"));
    (!help && !row.detail.is_empty()).then_some(row.detail.as_str())
}

pub(super) fn copy(selected: Option<&OttRow>, context: &str) -> String {
    let mut parts = Vec::with_capacity(3);
    if let Some(row) = selected {
        if !row.title.is_empty() { parts.push(row.title.as_str()); }
        if !row.detail.is_empty() && row.detail != row.title { parts.push(row.detail.as_str()); }
    }
    if !context.is_empty() && !parts.contains(&context) { parts.push(context); }
    parts.join("\n\n")
}

/// Root is identified by the owner's group-opening actions, not translated labels.
/// The same page_id is also used by drafts and group-specific actions.
pub(super) fn root(view: &OttView) -> bool {
    view.page_id == "settings" && view.rows.iter().any(|row|
        matches!(&row.action, OttAction::EditSetting(key)
            if key == &row.id && key.starts_with("settings.open:")))
}

pub(super) struct DraftFooter<'a> {
    pub back: &'a OttRow,
    pub save: Option<&'a OttRow>,
}

/// The owner supplies both the address and the translated label. Save admission
/// remains the remote key handler's exact current-row check, shared by its caller.
pub(super) fn draft_footer<'a>(view: &'a OttView, save_action: Option<&OttAction>) -> Option<DraftFooter<'a>> {
    if view.page_id != "settings" || root(view) { return None; }
    let back_key = |key: &str| key.starts_with("settings.draft:") && key.ends_with(":back");
    let mut rows = view.rows.iter().filter(|row| back_key(&row.id)
        || matches!(&row.action, OttAction::EditSetting(key) if back_key(key)));
    let back = rows.next()?;
    if rows.next().is_some() || !back.enabled { return None; }
    let OttAction::EditSetting(key) = &back.action else { return None; };
    if key != &back.id { return None; }
    let nonce = key.strip_prefix("settings.draft:")?.strip_suffix(":back")?;
    let value = nonce.parse::<u64>().ok()?;
    if value == 0 || value.to_string() != nonce { return None; }
    let save_id = format!("settings.draft:{nonce}:save");
    let save = save_action.and_then(|action| view.rows.iter().find(|row|
        row.enabled && row.id == save_id && &row.action == action));
    Some(DraftFooter { back, save })
}

/// A separate focus identity avoids duplicating the same key in the body and
/// footer. Including the owner's row ID retires the footer with its draft nonce.
pub(super) fn footer_id(row: &OttRow) -> String { format!("settings-footer:{}", row.id) }

pub(super) fn draft_footer_row(measure: &dyn plx_machine::machine::Measure,
    width: f32, back: (u32, &str), save: Option<(u32, &str)>, size: i32) -> plx_ui::page_header::HeaderRow {
    use plx_ui::page_header::{FooterKey, HeaderItem, HeaderRow, PageHeader, PageHeaderSpec, Slot};
    let mut controls = vec![back];
    if let Some(save) = save { controls.push(save); }
    loop {
        let page = PageHeader::new(PageHeaderSpec { left: controls.iter().map(|(_, label)| HeaderItem {
            label: (*label).into(), focusable: true, ..Default::default()
        }).collect(), ..Default::default() }, &[], measure);
        let layout = page.layout(width, measure);
        let stops = layout.items.iter().enumerate().filter_map(|(index, item)| match item.slot {
            Slot::Left(i) => controls.get(i).map(|(key, _)| (*key, index)), _ => None,
        }).collect();
        let mut hints = vec![(back.0, vec![FooterKey::plain("✖")])];
        if let Some((key, _)) = controls.get(1) { hints.push((*key, vec![FooterKey::plain("0")])); }
        let row = HeaderRow::new(page, layout, stops).with_classic_footer(width, size, &hints, measure);
        if controls.len() == 1 || row.classic_footer_fits(width) { return row; }
        // The original Save row and key handler remain available if its hint
        // cannot fit. Do not shrink the current locale or move the shared frame.
        controls.pop();
    }
}

pub(super) fn close_footer(measure: &dyn plx_machine::machine::Measure,
    width: f32, back: u32, size: i32) -> plx_ui::page_header::HeaderRow {
    use plx_ui::page_header::{FooterKey, HeaderItem, HeaderRow, PageHeader, PageHeaderSpec, Slot};
    let page = PageHeader::new(PageHeaderSpec { left: vec![HeaderItem {
        label: plx_platform::i18n::msg::settings_login_close().into(),
        focusable: true, ..Default::default()
    }], ..Default::default() }, &[], measure);
    let layout = page.layout(width, measure);
    let stops = layout.items.iter().enumerate().filter_map(|(index, item)|
        (item.slot == Slot::Left(0)).then_some((back, index))).collect();
    HeaderRow::new(page, layout, stops).with_classic_footer(width, size,
        &[(back, vec![FooterKey::plain("✖")])], measure)
}
