//! Classic Settings uses the existing list and Saved writer, with one draft per group.
use super::*;
#[path = "ott_settings_draft.rs"]
mod draft;
#[path = "ott_settings_group_fields.rs"]
mod fields;
#[path = "ott_settings_group_colors.rs"]
mod colors;
use fields::Group;

const STALE: &str = "Settings changed; reopen this group";
const BUSY: &str = "This group is being saved";
const PREFIX: &str = "settings.draft:";

#[derive(Clone, Copy)]
enum Section { Draft(Group), Profiles, Remote, Parental }
impl Section {
    fn id(self) -> &'static str { match self {
        Self::Draft(group) => group.id(), Self::Profiles => "profiles",
        Self::Remote => "remote", Self::Parental => "parental",
    } }
    fn title(self) -> &'static str { match self {
        Self::Draft(group) => group.title(), Self::Profiles => "Profiles",
        Self::Remote => "Remote control", Self::Parental => "Parental control",
    } }
    fn find(id: &str) -> Option<Self> {
        Group::ALL.into_iter().find(|group| group.id() == id).map(Self::Draft)
            .or_else(|| match id { "profiles" => Some(Self::Profiles),
                "remote" => Some(Self::Remote), "parental" => Some(Self::Parental), _ => None })
    }
}

#[derive(Default)]
pub(super) struct State {
    draft: draft::State,
    section: Option<Section>,
    picker: Option<&'static str>,
    source: String,
    nonce: u64,
    focus: Option<String>,
    language: Option<LanguageHandoff>,
    color_editor: Option<colors::Editor>,
}

/// A presentation handoff only: the native language picker uses its own shared
/// preference writer. This does not transfer the draft's save authority.
struct LanguageHandoff {
    address: draft::Address,
    host: plx_machine::machine::EntryId,
    page: plx_machine::machine::EntryId,
    surface: Option<plx_machine::machine::EntryId>,
}

fn error(error: draft::Error) -> &'static str {
    match error { draft::Error::Busy => BUSY,
        draft::Error::InvalidValue(error) => error, _ => STALE }
}

fn source(runtime: &Runtime) -> String {
    // Drafts are tied to the source that opened them, including its descriptor.
    // Unrelated application settings can still be merged into current Saved.
    serde_json::to_string(&json!([runtime.provider, runtime.saved.profiles.active_profile,
        runtime.provider_profile(), runtime.effective_profile(), runtime.generation,
        runtime.saved.settings.get("selectedProvider")]))
        .expect("source descriptor serializes")
}

fn field(key: &'static str) -> draft::Field {
    let revision_key = match key {
        "uiColorConfig" => Some("settingRevision:uiColorConfig"),
        "uiFontScale" => Some("settingRevision:uiFontScale"),
        "volumeStep" => Some("settingRevision:volumeStep"),
        "sleepTimeout" => Some("settingRevision:sleepTimeout"),
        "infoTimeout" => Some("settingRevision:infoTimeout"),
        "osdOpacity" => Some("settingRevision:osdOpacity"),
        "permanentTime" => Some("settingRevision:permanentTime"),
        "pageSize" => Some("settingRevision:pageSize"),
        "listPosition" => Some("settingRevision:listPosition"),
        "arrowFun" => Some("settingRevision:arrowFun"),
        "seek13Duration" => Some("settingRevision:seek13Duration"),
        "showNumber" => Some("settingRevision:showNumber"),
        "showName" => Some("settingRevision:showName"),
        "showProgram" => Some("settingRevision:showProgram"),
        "showDescription" => Some("settingRevision:showDescription"),
        "showProgress" => Some("settingRevision:showProgress"),
        _ => None,
    };
    draft::Field { key, revision_key }
}

/// Logical draft IDs stay stable; only the existing settings owner defines
/// the physical storage keys and their provider scope.
fn storage_keys(runtime: &Runtime, field: draft::Field) -> Result<(String, Option<String>), &'static str> {
    if let Some([value, revision]) = runtime.osd_setting_storage_keys(field.key) {
        Ok((value, Some(revision)))
    } else if Group::Channels.fields().contains(&field.key) {
        Err(STALE)
    } else {
        Ok((field.key.into(), field.revision_key.map(str::to_owned)))
    }
}

fn draft_values(runtime: &Runtime, group: Group) -> Result<BTreeMap<String, Value>, &'static str> {
    let mut values = BTreeMap::new();
    for key in group.fields() {
        let field = field(key);
        let (stored, revision) = storage_keys(runtime, field)?;
        if let Some(value) = runtime.saved.settings.get(&stored) {
            values.insert(field.key.into(), value.clone());
        }
        if let (Some(logical), Some(stored)) = (field.revision_key, revision) {
            if let Some(value) = runtime.saved.settings.get(&stored) {
                values.insert(logical.into(), value.clone());
            }
        }
    }
    Ok(values)
}

fn address(runtime: &Runtime) -> Result<draft::Address, &'static str> {
    let state = &runtime.settings_groups;
    let address = state.draft.address().ok_or(STALE)?;
    if runtime.page != "settings" || runtime.reply != Some(address.owner)
        || state.section.map(Section::id) != state.draft.group()
        || runtime.navigation_epoch != address.navigation || source(runtime) != state.source {
        return Err(STALE);
    }
    Ok(address)
}

fn action_row(runtime: &Runtime, command: &str, title: &str, detail: String) -> OttRow {
    let key = format!("{PREFIX}{}:{command}", runtime.settings_groups.nonce);
    ui_projection::row(key.clone(), title.into(), detail, OttAction::EditSetting(key), false)
}

fn displayed(runtime: &Runtime, key: &str) -> Option<Value> {
    runtime.settings_groups.draft.get(key).cloned().or_else(|| fields::value(runtime, key))
}

fn menu_saved(runtime: &Runtime) -> Saved {
    let mut saved = runtime.saved.clone();
    if let Some(value) = runtime.settings_groups.draft.get(menu_visibility::KEY) {
        saved.settings.insert(menu_visibility::KEY.into(), value.clone());
    }
    saved
}

fn draft_rows(runtime: &Runtime, group: Group) -> Vec<OttRow> {
    let state = &runtime.settings_groups;
    let enabled = address(runtime).is_ok() && runtime.settings_write_refusal().is_none()
        && state.draft.pending().is_none();
    let mut rows = if matches!(group, Group::Colors) {
        colors::rows(runtime)
    } else if let Some(key) = state.picker {
        let current = displayed(runtime, key).and_then(|value| value.as_u64());
        fields::choices(key).into_iter().map(|(value, label)| {
            let selected = current == Some(value) || (key == "arrowFun" && current == Some(3) && value == 0);
            action_row(runtime, &format!("choose:{key}:{value}"), &label,
                if selected { "Selected".into() } else { String::new() })
        }).collect::<Vec<_>>()
    } else if matches!(group, Group::Menu) {
        let saved = menu_saved(runtime);
        let visibility = menu_visibility::Stored::read(&saved).ok().map(|stored| stored.visibility());
        let mut rows: Vec<_> = plx_appkit::menu_visibility::Item::ALL.into_iter().map(|item| {
            action_row(runtime, &format!("menu:{}", item.id()), item.title(),
                if visibility.is_some_and(|value| value.visible(item)) { "Visible" } else { "Hidden" }.into())
        }).collect();
        rows.push(action_row(runtime, "reset-menu", "Reset menu items", "Show all menu items".into()));
        rows
    } else {
        group.fields().iter().map(|key| {
            let value = displayed(runtime, key).and_then(|value| value.as_u64());
            let label = value.map(|value| fields::choices(key).into_iter()
                .find(|(candidate, _)| *candidate == value || (*key == "arrowFun" && value == 3 && *candidate == 0))
                .map(|(_, label)| label).unwrap_or_else(|| value.to_string())).unwrap_or_default();
            let mut row = action_row(runtime, &format!("pick:{key}"), fields::title(key), label);
            row.enabled = value.is_some();
            row
        }).collect()
    };
    if group == Group::Interface && state.picker.is_none() {
        rows.insert(0, action_row(runtime, "language", plx_platform::i18n::msg::settings_language_title(),
            plx_platform::i18n::saved_preference().native_name().into()));
    }
    for row in &mut rows { row.enabled &= enabled; }
    if state.picker.is_none() {
        let mut save = action_row(runtime, "save", "Save Settings", String::new());
        save.enabled = enabled;
        rows.push(save);
    }
    rows.push(action_row(runtime, "back", if state.picker.is_some() || state.draft.pending().is_some() {
        "Back to Settings"
    } else { "Cancel" }, String::new()));
    rows
}

fn member(section: Section, id: &str) -> bool {
    match section {
        Section::Profiles => id.starts_with("profile."),
        Section::Remote => id.starts_with("remote."),
        Section::Parental => id.starts_with("parental."),
        Section::Draft(_) => false,
    }
}

/// Transform the existing, capability-filtered settings rows. Other pages and
/// the shared Classic renderer keep their original layout and routing.
pub(super) fn view(runtime: &Runtime, mut view: OttView) -> OttView {
    if runtime.page != "settings" { return view; }
    let state = &runtime.settings_groups;
    if let Some(section) = state.section {
        view.title = state.picker.map(fields::title).unwrap_or(section.title()).into();
        if let Section::Draft(group) = section {
            view.rows = draft_rows(runtime, group);
            view.status = runtime.protected_state_status().unwrap_or(&runtime.status).into();
            if address(runtime).is_err() { view.status = STALE.into(); }
            view.description = if state.picker.is_some() { "Return to the group without changing this value" }
                else { "Changes are saved only with Save Settings" }.into();
        } else {
            view.rows.retain(|row| member(section, &row.id));
            view.rows.push(action_row(runtime, "back", "Back to Settings", String::new()));
            if !matches!(section, Section::Remote) {
                view.status = runtime.protected_state_status().unwrap_or(&runtime.status).into();
                view.description = "Closing does not undo changes already saved".into();
            }
        }
    } else {
        let mut rows: Vec<_> = Group::ALL.into_iter().filter(|group| !group.fields().is_empty()).map(|group| {
            let key = format!("settings.open:{}", group.id());
            let mut row = ui_projection::row(key.clone(), group.title().into(), String::new(),
                OttAction::EditSetting(key), false);
            row.enabled = runtime.settings_write_refusal().is_none() && state.draft.pending().is_none();
            row
        }).collect();
        for section in [Section::Profiles, Section::Parental, Section::Remote] {
            if view.rows.iter().any(|row| member(section, &row.id)) {
                let key = format!("settings.open:{}", section.id());
                rows.push(ui_projection::row(key.clone(), section.title().into(), "These actions save separately".into(),
                    OttAction::EditSetting(key), false));
            }
        }
        rows.extend(view.rows.into_iter().filter(|row| {
            ![Section::Profiles, Section::Remote, Section::Parental].into_iter().any(|section| member(section, &row.id))
                && row.id != "menu.items" && !Group::ALL.into_iter().any(|group| group.fields().contains(&row.id.as_str()))
        }));
        view.rows = rows;
        view.description.clear();
        view.status = runtime.protected_state_status().unwrap_or(&runtime.status).into();
    }
    let focus = state.focus.as_ref().map(|command| if command.starts_with("settings.open:") {
        command.clone()
    } else { format!("{PREFIX}{}:{command}", state.nonce) });
    view.selected_row = focus.filter(|id| view.rows.iter().any(|row| row.id == *id))
        .or_else(|| state.picker.and_then(|_| view.rows.iter().find(|row| row.detail == "Selected")
            .map(|row| row.id.clone())))
        .or_else(|| view.selected_row.take().filter(|id| view.rows.iter().any(|row| row.id == *id)))
        .or_else(|| if state.section.is_none() {
            runtime.settings_focus.and_then(|key| Group::ALL.into_iter().find(|group| group.fields().contains(&key)))
                .map(|group| format!("settings.open:{}", group.id()))
        } else { None });
    view
}

pub(super) fn presentation_action(runtime: &Runtime, action: &OttAction) -> bool {
    colors::presentation_action(runtime, action) || matches!(action, OttAction::EditSetting(key) if key.starts_with(PREFIX) || key.starts_with("settings.open:"))
        || (runtime.page == "settings" && runtime.settings_groups.section.is_some()
            && matches!(action, OttAction::Back | OttAction::ShowSettings))
}

pub(super) fn back(runtime: &Runtime, action: OttAction) -> OttAction {
    if runtime.page == "settings" && runtime.settings_groups.section.is_some() && matches!(action, OttAction::Back) {
        OttAction::ShowSettings
    } else { action }
}

/// The parental section and Cancel only navigate. They keep the existing
/// ShowSettings gate; the actual parental or save action retains its own gate.
pub(super) fn navigation_only(runtime: &Runtime, action: &OttAction) -> bool {
    if runtime.page != "settings" { return false; }
    matches!(action, OttAction::EditSetting(key) if key == "settings.open:parental"
        || (runtime.settings_groups.section.is_some()
            && key == &format!("{PREFIX}{}:back", runtime.settings_groups.nonce)))
}

pub(super) fn cancel(runtime: &mut Runtime) {
    if runtime.settings_groups.color_editor.as_ref().is_some_and(|held|
        runtime.edit_session.as_ref() == Some(&held.session)) { runtime.cancel_editor(); }
    let state = &mut runtime.settings_groups;
    if let Some(address) = state.draft.address() { state.draft.cancel(address); }
    state.section = None;
    state.picker = None;
    state.source.clear();
    state.focus = None;
    state.language = None;
    state.color_editor = None;
}

fn leave(runtime: &mut Runtime) {
    if let Some(key) = runtime.settings_groups.picker.take() {
        runtime.settings_groups.focus = Some(format!("pick:{key}"));
    } else {
        return_to_root(runtime);
    }
}

fn return_to_root(runtime: &mut Runtime) {
    let focus = runtime.settings_groups.section.map(|section| format!("settings.open:{}", section.id()));
    cancel(runtime);
    runtime.settings_groups.focus = focus;
}

fn open(runtime: &mut Runtime, section: Section) -> Result<(), &'static str> {
    if runtime.page != "settings" || runtime.settings_groups.section.is_some()
        || runtime.edit_session.is_some() || runtime.setup.is_some() { return Err(STALE); }
    if let Section::Draft(group) = section {
        if let Some(error) = runtime.settings_write_refusal() { return Err(error); }
        let inventory: Vec<_> = group.fields().iter().copied().map(field).collect();
        let owner = runtime.reply.ok_or(STALE)?;
        let source = source(runtime);
        let values = draft_values(runtime, group)?;
        runtime.settings_groups.draft.begin(group.id(), owner, runtime.navigation_epoch,
            &inventory, &values).map_err(error)?;
        runtime.settings_groups.source = source;
    }
    runtime.settings_groups.section = Some(section);
    runtime.status.clear();
    Ok(())
}

fn merge(runtime: &Runtime, address: draft::Address) -> Result<Option<Saved>, &'static str> {
    let group = match runtime.settings_groups.section { Some(Section::Draft(group)) => group, _ => return Err(STALE) };
    let values = draft_values(runtime, group)?;
    let changes = runtime.settings_groups.draft.changes(address, &values).map_err(error)?;
    if changes.is_empty() { return Ok(None); }
    let mut saved = runtime.saved.clone();
    for change in changes {
        let key = change.field.key;
        if key == menu_visibility::KEY {
            saved.settings.insert(key.into(), change.value);
            menu_visibility::Stored::read(&saved)?;
        } else {
            let candidate = if key == plx_ui::palette::KEY {
                runtime.ui_color_candidate(change.value.as_str().ok_or(STALE)?, address.id)?
            } else {
                fields::candidate(runtime, key, change.value.as_u64().ok_or(STALE)?, address.id)?
            };
            let (stored, revision) = storage_keys(runtime, change.field)?;
            saved.settings.insert(stored.clone(), candidate.settings.get(&stored).ok_or(STALE)?.clone());
            let revision = revision.ok_or(STALE)?;
            saved.settings.insert(revision.clone(), candidate.settings.get(&revision).ok_or(STALE)?.clone());
        }
    }
    Ok(Some(saved))
}

fn save(runtime: &mut Runtime) -> Result<(), &'static str> {
    save_with(runtime, |runtime, candidate| runtime.save(candidate, None, false))
}

fn save_with(runtime: &mut Runtime,
    writer: impl FnOnce(&mut Runtime, Saved) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let address = address(runtime)?;
    if let Some(error) = runtime.settings_write_refusal() { return Err(error); }
    let Some(candidate) = merge(runtime, address)? else { return_to_root(runtime); return Ok(()); };
    writer(runtime, candidate)?;
    runtime.settings_groups.draft.mark_saving(address).map_err(error)?;
    Ok(())
}

fn stage_menu(runtime: &mut Runtime, command: &str, address: draft::Address) -> Result<(), &'static str> {
    use plx_appkit::menu_visibility::{Change, Item};
    let saved = menu_saved(runtime);
    let change = if command == "reset-menu" { Change::Reset } else {
        let item = command.strip_prefix("menu:").and_then(Item::find).ok_or(STALE)?;
        let visible = menu_visibility::Stored::read(&saved)?.visibility().visible(item);
        Change::Set { item, visible: !visible }
    };
    let candidate = menu_visibility::candidate(&saved, change)?;
    let value = candidate.settings.get(menu_visibility::KEY).ok_or(STALE)?.clone();
    runtime.settings_groups.draft.stage(address, menu_visibility::KEY, value, |_, _| Ok(())).map_err(error)?;
    runtime.settings_groups.focus = Some(command.into());
    Ok(())
}

fn edit(runtime: &mut Runtime, command: &str) -> Result<(), &'static str> {
    if command == "back" { leave(runtime); return Ok(()); }
    let address = address(runtime)?;
    if runtime.settings_groups.draft.pending().is_some() { return Err(BUSY); }
    if let Some(error) = runtime.settings_write_refusal() { return Err(error); }
    if command == "save" && runtime.settings_groups.picker.is_none() { return save(runtime); }
    let group = match runtime.settings_groups.section { Some(Section::Draft(group)) => group, _ => return Err(STALE) };
    if matches!(group, Group::Menu) { return stage_menu(runtime, command, address); }
    if matches!(group, Group::Colors) { return colors::edit(runtime, command, address); }
    if let Some(key) = command.strip_prefix("pick:") {
        if runtime.settings_groups.picker.is_some() { return Err(STALE); }
        let key = group.fields().iter().copied().find(|field| *field == key).ok_or(STALE)?;
        runtime.settings_groups.picker = Some(key);
        return Ok(());
    }
    let (key, value) = command.strip_prefix("choose:").and_then(|value| value.split_once(':')).ok_or(STALE)?;
    if runtime.settings_groups.picker != Some(key) { return Err(STALE); }
    let value = value.parse::<u64>().map_err(|_| STALE)?;
    let candidate = fields::candidate(runtime, key, value, address.id)?;
    let logical = group.fields().iter().copied().find(|field| *field == key).ok_or(STALE)?;
    let (stored, _) = storage_keys(runtime, field(logical))?;
    let value = candidate.settings.get(&stored).cloned().ok_or(STALE)?;
    runtime.settings_groups.draft.stage(address, key, value, |_, _| Ok(())).map_err(error)?;
    runtime.settings_groups.picker = None;
    runtime.settings_groups.focus = Some(format!("pick:{key}"));
    Ok(())
}

pub(super) fn color_action(runtime: &mut Runtime, app: &mut App,
    action: &OttAction) -> Option<Result<(), &'static str>> {
    colors::action(runtime, app, action)
}

/// Open the same LanguagePage used by Modern. The existing modal container
/// returns to this exact Interface group; no provider or Saved snapshot is restored.
pub(super) fn language_action(runtime: &mut Runtime, app: &mut App,
    action: &OttAction) -> Option<Result<(), &'static str>> {
    let OttAction::EditSetting(key) = action else { return None; };
    let command = key.strip_prefix(PREFIX)?.split_once(':')?;
    if command.1 != "language" { return None; }
    Some((|| {
        if command.0 != runtime.settings_groups.nonce.to_string()
            || !matches!(runtime.settings_groups.section, Some(Section::Draft(Group::Interface)))
            || runtime.settings_groups.picker.is_some()
            || runtime.settings_groups.language.is_some() { return Err(STALE); }
        let captured = address(runtime)?;
        if current_ui_owner(app) != Some(captured.owner)
            || app.pages.has_pending_surface_navigation() { return Err(STALE); }
        let host = app.pages.nav.entry_of_instance(captured.owner).ok_or(STALE)?;
        let page = app.pages.nav.top_page().ok_or(STALE)?.id;
        if runtime.settings_groups.draft.pending().is_some() { return Err(BUSY); }
        if let Some(error) = runtime.settings_write_refusal() { return Err(error); }
        let next = runtime.settings_groups.nonce.checked_add(1).ok_or(STALE)?;
        bridge::open_settings_at(&mut app.pages, plx_screens::family::SettingsPage::Language);
        runtime.settings_groups.language = Some(LanguageHandoff { address: captured, host, page, surface: None });
        runtime.settings_groups.nonce = next;
        runtime.settings_groups.focus = Some("language".into());
        Ok(())
    })())
}

/// Only the exact Language surface may cover this draft without retiring it.
/// This is presentation continuity, never the owner used to admit OTT commands.
fn language_owner(runtime: &Runtime, app: &App)
    -> Option<(InstanceId, Option<plx_machine::machine::EntryId>)>
{
    if app.pages.nav.suspended || app.pages.has_pending_navigation() { return None; }
    let held = runtime.settings_groups.language.as_ref()?;
    let captured = held.address;
    if address(runtime).ok() != Some(captured)
        || !app.pages.nav.top_page().is_some_and(|entry| entry.id == held.page)
        || !app.pages.nav.entry(held.host).is_some_and(|entry| matches!(entry.arg, AppArg::Ott)
            && entry.inst.as_ref().is_some_and(|body| body.id == captured.owner && !body.staged))
        || (held.host != held.page && !app.pages.nav.modals.surfaces.iter().any(|surface|
            surface.entry.id == held.host && matches!(surface.phase,
                plx_ui::containers::modal::Phase::Opening | plx_ui::containers::modal::Phase::Open)))
    { return None; }
    if current_ui_owner(app) == Some(captured.owner) { return Some((captured.owner, None)); }
    let plx_machine::machine::InputOwner::Entry(input) = app.pages.nav.input_owner()? else { return None; };
    let surface = app.pages.nav.modals.surfaces.iter().find(|surface|
        surface.entry.id == input
            && matches!(surface.entry.arg, AppArg::Settings(plx_screens::family::SettingsPage::Language))
            && surface.entry.inst.as_ref().is_some_and(|body| !body.staged)
            && held.surface.is_none_or(|expected| surface.entry.id == expected))?;
    Some((captured.owner, Some(surface.entry.id)))
}

pub(super) fn retained_language_owner(runtime: &Runtime, app: &App) -> Option<InstanceId> {
    language_owner(runtime, app).map(|(owner, _)| owner)
}

/// An opened language modal may cover its own draft without replacing its owner.
/// Other modals, source changes and removed/replaced parent instances retain the
/// ordinary stale-draft behavior. No save action uses this presentation exception.
pub(super) fn reconcile_owned(runtime: &mut Runtime, app: &App) {
    let mut owner = current_ui_owner(app);
    if let Some((captured, surface)) = language_owner(runtime, app) {
        owner = Some(captured);
        if let Some(surface) = surface {
            runtime.settings_groups.language.as_mut().unwrap().surface = Some(surface);
        } else if !app.pages.has_pending_surface_navigation() {
            runtime.settings_groups.language = None;
        }
    }
    reconcile(runtime, owner);
}

pub(super) fn action(runtime: &mut Runtime, action: &OttAction) -> Option<Result<(), &'static str>> {
    if runtime.page == "settings" && runtime.settings_groups.section.is_some() && matches!(action, OttAction::ShowSettings) {
        let Some(next) = runtime.settings_groups.nonce.checked_add(1) else { return Some(Err(STALE)); };
        leave(runtime);
        runtime.settings_groups.nonce = next;
        return Some(Ok(()));
    }
    let OttAction::EditSetting(key) = action else { return None; };
    if let Some(id) = key.strip_prefix("settings.open:") {
        return Some((|| {
            let next = runtime.settings_groups.nonce.checked_add(1).ok_or(STALE)?;
            open(runtime, Section::find(id).ok_or(STALE)?)?;
            runtime.settings_groups.nonce = next;
            runtime.settings_groups.focus = None;
            Ok(())
        })());
    }
    let value = key.strip_prefix(PREFIX)?;
    Some((|| {
        if runtime.page != "settings" || runtime.settings_groups.section.is_none() { return Err(STALE); }
        let expected = format!("{}:", runtime.settings_groups.nonce);
        let command = value.strip_prefix(&expected).ok_or(STALE)?;
        let next = runtime.settings_groups.nonce.checked_add(1).ok_or(STALE)?;
        edit(runtime, command)?;
        runtime.settings_groups.nonce = next;
        Ok(())
    })())
}

/// A separately committed action leaves any group draft. Focus/refresh do not.
pub(super) fn external_action(runtime: &mut Runtime, action: &OttAction) {
    if !presentation_action(runtime, action)
        && !matches!(action, OttAction::FocusAt { .. } | OttAction::Refresh) {
        if runtime.settings_groups.draft.address().is_some() { cancel(runtime); }
        else { runtime.settings_groups.focus = None; }
    }
}

pub(super) fn reconcile(runtime: &mut Runtime, owner: Option<InstanceId>) {
    let Some(captured) = runtime.settings_groups.draft.address() else { return; };
    if address(runtime).is_ok() && owner == Some(captured.owner) { return; }
    let visible = runtime.page == "settings";
    cancel(runtime);
    if visible { runtime.status = STALE.into(); }
    runtime.changed();
}

pub(super) fn settled(runtime: &mut Runtime, app: &App, persisted: bool, applied: bool) {
    finish(runtime, current_ui_owner(app), persisted, applied);
}

fn finish(runtime: &mut Runtime, owner: Option<InstanceId>, persisted: bool, applied: bool) {
    let Some(pending) = runtime.settings_groups.draft.pending() else { return; };
    let current = address(runtime).ok() == Some(pending) && owner == Some(pending.owner);
    let outcome = if persisted { draft::Outcome::Applied } else { draft::Outcome::Rejected };
    runtime.settings_groups.draft.settle(pending, outcome);
    if (persisted || !current) && matches!(runtime.settings_groups.section, Some(Section::Draft(_))) {
        if persisted && applied && current { return_to_root(runtime); }
        else { cancel(runtime); }
    }
    if persisted && applied && current { runtime.status = "Settings saved".into(); }
}

pub(super) fn uncertain(runtime: &mut Runtime) {
    if let Some(pending) = runtime.settings_groups.draft.pending() {
        runtime.settings_groups.draft.settle(pending, draft::Outcome::Uncertain);
    }
    cancel(runtime);
}

#[cfg(test)]
#[path = "ott_settings_groups_tests.rs"]
mod tests;
