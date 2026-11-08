//! Durable, installation-wide menu presentation using the sole Saved writer.
use super::*;
use plx_appkit::menu_visibility::{Change, Item, Visibility};
use std::collections::BTreeSet;

pub(super) const KEY: &str = "nativeMenuVisibility";
const INVALID: &str = "Invalid menu visibility settings";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Stored {
    version: u32,
    revision: u64,
    hidden: BTreeSet<String>,
}
impl Default for Stored {
    fn default() -> Self { Self { version: 1, revision: 0, hidden: BTreeSet::new() } }
}
impl Stored {
    pub(super) fn read(saved: &Saved) -> Result<Self, &'static str> {
        let value: Self = match saved.settings.get(KEY) {
            Some(value) => serde_json::from_value(value.clone()).map_err(|_| INVALID)?,
            None => Self::default(),
        };
        if value.version != 1 || value.hidden.len() > Item::ALL.len()
            || value.hidden.iter().any(|id| Item::find(id).is_none()) { return Err(INVALID); }
        Ok(value)
    }
    pub(super) fn visibility(&self) -> Visibility {
        let mut result = Visibility::default();
        for item in Item::ALL { if self.hidden.contains(item.id()) { result.hide(item); } }
        result
    }
}

/// Every local and remote edit advances this compound revision, including an
/// idempotent set/reset. An old A receipt cannot become valid again after A→B→A.
pub(super) fn candidate(saved: &Saved, change: Change) -> Result<Saved, &'static str> {
    let mut value = Stored::read(saved)?;
    value.revision = value.revision.checked_add(1).ok_or("Menu settings revision exhausted")?;
    match change {
        Change::Set { item, visible: true } => { value.hidden.remove(item.id()); }
        Change::Set { item, visible: false } => { value.hidden.insert(item.id().into()); }
        Change::Reset => value.hidden.clear(),
    }
    let mut candidate = saved.clone();
    candidate.settings.insert(KEY.into(), serde_json::to_value(value).map_err(|_| INVALID)?);
    Ok(candidate)
}

pub(super) fn descriptor(item: Item) -> Value {
    json!({"id":item.setting(),"type":"integer","scope":"application","sensitive":false,
        "access":{"read":true,"write":true},"default_value":1,"apply":"immediate","constraints":{"values":[0,1]},
        "choices":[{"value":1,"label":"Visible","aliases":["visible","show"]},
            {"value":0,"label":"Hidden","aliases":["hidden","hide"]}],
        "description":format!("Show {} in native menus. Does not change action permissions or keyboard shortcuts.",item.title())})
}
pub(super) fn snapshot(saved: &Saved, item: Item) -> Value {
    match Stored::read(saved) {
        Ok(value) => {
            let visible = u8::from(value.visibility().visible(item));
            let mut result = json!({"id":item.setting(),"value":visible,
                "storage":if saved.settings.contains_key(KEY) {"stored"} else {"default"}});
            if saved.settings.contains_key(KEY) { result["persisted_value"] = json!(visible); }
            result
        }
        // Report invalidity, never silently certify a default as persisted.
        Err(_) => json!({"id":item.setting(),"storage":"invalid","value":1}),
    }
}

pub(super) fn visibility(runtime: &Runtime) -> Visibility {
    if !runtime.loaded || runtime.persistence_uncertain { return Visibility::default(); }
    Stored::read(&runtime.saved).map(|value| value.visibility()).unwrap_or_default()
}
pub(super) fn view(runtime: &Runtime) -> Option<OttView> {
    if runtime.page != "menu-items" { return None; }
    let visible = visibility(runtime);
    let revision = Stored::read(&runtime.saved).map(|stored| stored.revision).unwrap_or(0);
    let enabled = runtime.settings_write_refusal().is_none();
    let mut rows: Vec<_> = Item::ALL.into_iter().map(|item| OttRow { current_programme: None, content_title: None, guide: None, guide_available: false, records_available: false,
        media: None, id: format!("menu:{}", item.id()), title: item.title().into(),
        detail: if visible.visible(item) { "Visible" } else { "Hidden" }.into(),
        description: String::new(), poster: None, enabled, favorite: false,
        action: OttAction::MenuVisibility { revision, context: runtime.navigation_epoch,
            change: Change::Set { item, visible: !visible.visible(item) } },
    }).collect();
    rows.push(OttRow { current_programme: None, content_title: None, guide: None, guide_available: false, records_available: false, media: None, id: "menu:reset".into(), title: "Reset menu items".into(),
        detail: "Show all menu items".into(), description: String::new(), poster: None,
        action: OttAction::MenuVisibility { revision, context: runtime.navigation_epoch, change: Change::Reset },
        enabled, favorite: false });
    rows.push(OttRow { current_programme: None, content_title: None, guide: None, guide_available: false, records_available: false, media: None, id: "menu:back".into(), title: "Back to Settings".into(),
        detail: String::new(), description: String::new(), poster: None,
        action: OttAction::ShowSettings, enabled: true, favorite: false });
    Some(OttView { preview_context: None, page_id: "menu-items".into(), revision: runtime.revision,
        title: "Menu items".into(), description: "Choose which items appear in menus".into(),
        status: runtime.protected_state_status().unwrap_or(&runtime.status).into(), rows,
        selected_row: runtime.selected.clone(), ..OttView::default() })
}

pub(super) fn action(runtime: &mut Runtime, selected: &OttAction) -> Option<Result<(), &'static str>> {
    if matches!(selected, OttAction::EditSetting(key) if key == "menu.items") {
        return Some((|| {
            if let Some(error) = runtime.settings_write_refusal() { return Err(error); }
            Stored::read(&runtime.saved)?;
            runtime.cancel_setup(); runtime.page = "menu-items";
            Ok(())
        })());
    }
    let OttAction::MenuVisibility { revision, context, change } = selected else { return None; };
    Some((|| {
        if runtime.page != "menu-items" || runtime.navigation_epoch != *context
            || Stored::read(&runtime.saved)?.revision != *revision {
            return Err("Menu settings changed; select the item again");
        }
        if let Some(error) = runtime.settings_write_refusal() { return Err(error); }
        // handle_action already enforced the same Settings and kiosk gates as
        // every other settings editor; no optimistic preference is published.
        let saved = candidate(&runtime.saved, *change)?;
        runtime.save(saved, None, false)?;
        runtime.selected = Some(match change {
            Change::Set { item, .. } => format!("menu:{}", item.id()),
            Change::Reset => "menu:reset".into(),
        });
        Ok(())
    })())
}

#[cfg(test)]
#[path = "ott_menu_visibility_tests.rs"]
mod tests;
