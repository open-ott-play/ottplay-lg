//! List actions share the organization owner and ordinary Saved writer.
//! Local menus retain their held grant; other menus retain the Saved source scope.
use super::*;
use crate::app::ott_local_session::OwnerToken;
use crate::app::ott_runtime::ui_projection::row;
use plx_platform::i18n::msg;

#[path = "ott_list_order_context.rs"]
mod contextual;

const CHANGED: &str = "The list changed; reopen Actions";
const TOO_LARGE: &str = "This list is too large to save its order";
const UNAVAILABLE: &str = "List order is unavailable for this source";

#[derive(Clone)]
struct DurableScope {
    authority: u64,
    profile_epoch: u64,
    cancel: Arc<AtomicBool>,
}
impl DurableScope {
    fn same(&self, other: &Self, after_save: bool) -> bool {
        // saved() separately verifies the exact authority increment from our
        // overlay ACK; neither source epoch nor cancellation identity is waived.
        (after_save || self.authority == other.authority)
            && self.profile_epoch == other.profile_epoch
            && Arc::ptr_eq(&self.cancel, &other.cancel)
    }
}
#[derive(Clone)]
enum SourceOwner {
    Local {
        read: transient::LocalSourceRead,
        transient: Option<OwnerToken>,
        durable: Option<DurableScope>,
    },
    Saved(DurableScope),
}
impl SourceOwner {
    fn transient(&self) -> Option<OwnerToken> {
        match self {
            Self::Local { transient, .. } => *transient,
            Self::Saved(_) => None,
        }
    }
    fn durable(&self) -> Option<&DurableScope> {
        match self {
            Self::Local { durable, .. } => durable.as_ref(),
            Self::Saved(scope) => Some(scope),
        }
    }
    fn same(&self, other: &Self, after_save: bool) -> bool {
        let source = match (self, other) {
            (Self::Local { read, transient, .. }, Self::Local { read: next, transient: owner, .. }) => {
                transient == owner && read.selection == next.selection && read.same_grant(next)
            }
            (Self::Saved(_), Self::Saved(_)) => true,
            _ => false,
        };
        source && match (self.durable(), other.durable()) {
            (Some(scope), Some(next)) => scope.same(next, after_save),
            (None, None) => true,
            _ => false,
        }
    }
}

// A retained Local grant is not proof of a durable profile. Logical edits need
// the same exact Saved owner as other providers, in addition to that grant.
fn durable_scope(runtime: &Runtime, profile: &ProviderProfile) -> Option<DurableScope> {
    if runtime.transient.accepted.is_some()
        || runtime.organization_profile() != Some(profile)
        || runtime.organization.exhausted
        || runtime.profile_edit_epoch == 0
        || !runtime.catalog_complete || runtime.catalog_loading
        || runtime.cancel.load(Ordering::Acquire)
    { return None; }
    runtime.organization_projection()?;
    Some(DurableScope {
        authority: runtime.organization.authority,
        profile_epoch: runtime.profile_edit_epoch,
        cancel: runtime.cancel.clone(),
    })
}

#[derive(Clone)]
struct Context {
    profile: ProviderProfile,
    category: String,
    source: SourceOwner,
    generation: u64,
    navigation: u64,
    catalog_revision: u64,
    owner: InstanceId,
    overlay: Option<Value>,
}
#[derive(Clone)]
pub(super) struct Menu {
    revision: u64,
    context: Context,
    item: Option<String>,
    choosing_category: bool,
    rebase: bool,
}
pub(super) struct Pending {
    menu: Menu,
    overlay: Option<Value>,
    focus: Option<String>,
}

fn capture(runtime: &Runtime) -> Option<Context> {
    if !matches!(runtime.page, "channels" | "actions")
        || !runtime.loaded
        || runtime.persistence_uncertain
        || runtime.saving
        || runtime.edit_session.is_some()
        || runtime.transient.candidate.is_some()
        || !runtime.play_work_allowed()
        || runtime.view_selection().1
        || runtime.entropy.is_none()
        || !matches!(
            crate::app::ott_kiosk_stored::read(&runtime.saved.settings),
            Ok(None)
        )
    {
        return None;
    }
    let profile = runtime.effective_profile()?.clone();
    if !profile.enabled {
        return None;
    }
    let source = if profile.kind == ProviderKind::Local {
        let selected = runtime.local_profile_selection(&profile)?;
        let read = runtime.capture_local_source(&selected)?;
        let transient = runtime.transient.accepted.as_ref().map(transient::TransientMedia::owner);
        if transient.is_none() && (!runtime.catalog_complete || runtime.catalog_loading) {
            return None;
        }
        let durable = durable_scope(runtime, &profile);
        SourceOwner::Local { read, transient, durable }
    } else {
        SourceOwner::Saved(durable_scope(runtime, &profile)?)
    };
    Some(Context {
        profile,
        category: runtime.effective_category().into(),
        source,
        generation: runtime.generation,
        navigation: runtime.navigation_epoch,
        catalog_revision: runtime.catalog_revision,
        owner: runtime.reply?,
        overlay: runtime
            .saved
            .settings
            .get(catalog_organization::KEY)
            .cloned(),
    })
}
fn current(runtime: &Runtime, context: &Context, after_save: bool) -> bool {
    capture(runtime).is_some_and(|now| {
        now.profile == context.profile
            && now.category == context.category
            && now.source.same(&context.source, after_save)
            && now.generation == context.generation
            && now.navigation == context.navigation
            && now.catalog_revision == context.catalog_revision
            && now.owner == context.owner
            && (after_save || now.overlay == context.overlay)
    })
}
fn category(runtime: &Runtime, context: &Context) -> Result<String, &'static str> {
    let projection = runtime.organization_projection().ok_or(UNAVAILABLE)?;
    projection
        .selection_id(&context.category)
        .map(str::to_owned)
        .ok_or(CHANGED)
}
fn reset_available(runtime: &Runtime, context: &Context) -> bool {
    if let Some(transient::TransientMedia::Local(session)) = &runtime.transient.accepted {
        session.context.can_reset_order(&context.category)
    } else {
        let Ok(source) = Source::new(&context.profile) else {
            return false;
        };
        let Ok(category) = category(runtime, context) else {
            return false;
        };
        Organization::read(&runtime.saved.settings)
            .is_ok_and(|document| document.can_reset_order(&source, &category))
    }
}

fn classic_search_visible(runtime: &Runtime) -> bool {
    runtime.interface_mode() == plx_machine::machine::UiMode::Classic
        && menu_visibility::visibility(runtime).visible(plx_appkit::menu_visibility::Item::Search)
}

impl Runtime {
    pub(in crate::app::ott_runtime) fn local_order_key(&self) -> Option<String> {
        if self.page != "channels" {
            return None;
        }
        let context = capture(self)?;
        let count = self.organization_rows(&context.category).len();
        if context.source.durable().is_none()
            && count < 2 && !reset_available(self, &context)
            && !(classic_search_visible(self)
                && self.search_view().is_some())
        {
            return None;
        }
        // This opens a menu only. Its subsequent mutation additionally requires
        // the privately captured source owner and monotonic organization revision.
        let encoded = serde_json::to_vec(&json!([
            context.profile,
            context.category,
            context.generation,
            context.navigation,
            context.catalog_revision,
            context.source.transient().map(|owner| owner.0),
            context.source.durable().map(|scope| [scope.authority, scope.profile_epoch]),
            context.overlay
        ]))
        .ok()?;
        Some(plx_data::ott::favorites_wire::hash(&encoded))
    }
    pub(in crate::app::ott_runtime) fn local_order_view(&self) -> OttView {
        let menu = self.organization.order_menu.as_ref();
        let usable = menu.filter(|menu| current(self, &menu.context, false));
        let mut rows = Vec::new();
        if let Some(menu) = menu {
            if menu.choosing_category {
                rows = contextual::destinations(self, menu, usable.is_some());
            } else {
            rows.extend(contextual::rows(self, menu, usable.is_some()));
            let mut shuffle = row(
                "order.shuffle".into(),
                msg::ott_order_shuffle().into(),
                String::new(),
                OttAction::ShuffleOrder(menu.revision),
                false,
            );
            shuffle.enabled =
                usable.is_some_and(|menu| self.organization_rows(&menu.context.category).len() > 1);
            rows.push(shuffle);
            let mut reset = row(
                "order.reset".into(),
                msg::ott_order_reset().into(),
                String::new(),
                OttAction::ResetOrder(menu.revision),
                false,
            );
            reset.enabled = usable.is_some_and(|menu| reset_available(self, &menu.context));
            rows.push(reset);
            if classic_search_visible(self) {
                let mut search = row("order.search".into(), msg::ott_search().into(), String::new(),
                    OttAction::EditSetting(format!("order.{}.search", menu.revision)), false);
                search.enabled = usable.is_some();
                rows.push(search);
            }
            }
        }
        rows.push(row(
            "order.back".into(),
            msg::settings_back().into(),
            String::new(),
            OttAction::CloseActions,
            false,
        ));
        OttView {
            page_id: "actions".into(),
            revision: self.revision,
            title: if menu.is_some_and(|menu| menu.choosing_category) {
                msg::ott_order_choose_category().into()
            } else { msg::ott_order_actions().into() },
            description: menu
                .map(|menu| menu.context.profile.name.clone())
                .unwrap_or_default(),
            status: if self.saving {
                self.status.clone()
            } else if usable.is_none() {
                msg::ott_order_changed().into()
            } else {
                self.status.clone()
            },
            rows,
            ..OttView::default()
        }
    }
}

pub(super) fn rebase(runtime: &mut Runtime) {
    if let Some(menu) = runtime
        .organization
        .order_menu
        .as_mut()
        .filter(|menu| menu.rebase)
    {
        menu.context.navigation = runtime.navigation_epoch;
        menu.rebase = false;
        if let Some(pending) = runtime.organization.order_pending.as_mut()
            .filter(|pending| pending.menu.revision == menu.revision && pending.menu.rebase)
        {
            pending.menu.context.navigation = runtime.navigation_epoch;
            pending.menu.rebase = false;
        }
    }
}

fn change(runtime: &mut Runtime, revision: u64, reset: bool) -> Result<(), &'static str> {
    let menu = runtime
        .organization
        .order_menu
        .clone()
        .filter(|menu| {
            menu.revision == revision
                && !menu.choosing_category
                && runtime.page == "actions"
                && current(runtime, &menu.context, false)
        })
        .ok_or(CHANGED)?;
    runtime.require_parental(parental::Target::Settings)?;
    let entropy = runtime.entropy.ok_or(UNAVAILABLE)?;
    let mut seed = u32::from_le_bytes(entropy[..4].try_into().map_err(|_| UNAVAILABLE)?)
        ^ (menu.revision as u32).wrapping_mul(0x9e3779b9);
    if seed == 0 {
        seed = 1;
    }
    if menu.context.source.transient().is_some() {
        let ids: Vec<String> = runtime
            .organization_rows(&menu.context.category)
            .iter()
            .map(|(_, entry)| entry.id.clone())
            .collect();
        let Some(transient::TransientMedia::Local(session)) = runtime.transient.accepted.as_mut()
        else {
            return Err(CHANGED);
        };
        session
            .context
            .reorder(
                menu.context.source.transient().ok_or(CHANGED)?,
                &menu.context.category,
                &ids,
                (!reset).then_some(seed),
            )
            .map_err(|_| CHANGED)?;
        runtime.page = "channels";
        runtime.organization.order_menu = None;
        // Unlike progressive discovery, an explicit reorder retires old read
        // snapshots. Preserve the source and already admitted playback ownership.
        commands::retire_catalog_reads(runtime);
        runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
        runtime.changed();
        return Ok(());
    }
    let category = category(runtime, &menu.context)?;
    let operation = if reset {
        Operation::ResetOrder { category }
    } else {
        Operation::ShuffleOrder { category, seed }
    };
    apply_saved(runtime, menu, operation)
}

// One candidate and one ordinary Saved writer for every list operation. The
// menu owner, exact overlay ACK and navigation fences remain shared.
fn apply_saved(runtime: &mut Runtime, menu: Menu, operation: Operation) -> Result<(), &'static str> {
    if !current(runtime, &menu.context, false) { return Err(CHANGED); }
    runtime.require_parental(parental::Target::Settings)?;
    let source = Source::new(&menu.context.profile).map_err(invalid)?;
    let mut document = Organization::read(&runtime.saved.settings).map_err(invalid)?;
    if !document
        .apply(&source, &runtime.catalog, operation)
        .map_err(|error| {
            if error == catalog_organization::Error::TooLarge {
                TOO_LARGE
            } else {
                invalid(error)
            }
        })?
        .changed
    {
        runtime.page = "channels";
        runtime.organization.order_menu = None;
        restore_focus(runtime, menu.item.as_deref());
        runtime.changed();
        return Ok(());
    }
    let projection = document.project(&source, &runtime.catalog).map_err(invalid)?;
    let focus = contextual::focus_after(runtime, &menu, &projection);
    let mut candidate = runtime.saved.clone();
    document
        .write_into(&mut candidate.settings)
        .map_err(invalid)?;
    let overlay = candidate.settings.get(catalog_organization::KEY).cloned();
    runtime.save(candidate, None, false)?;
    runtime.organization.order_pending = Some(Pending { menu, overlay, focus });
    Ok(())
}

fn open(runtime: &mut Runtime, key: &str, item: Option<&str>) -> Result<(), &'static str> {
    if runtime.local_order_key().as_deref() != Some(key) { return Err(CHANGED); }
    let context = capture(runtime).ok_or(UNAVAILABLE)?;
    // The producer names its actual focused row; never infer it from playback,
    // a saved focus fallback, or a row number. Check the rendered membership too.
    if let Some(id) = item {
        if !runtime.raw_view().rows.iter().any(|row| row.enabled
            && matches!(&row.action, OttAction::Play(value) if value == id))
        { return Err(CHANGED); }
    }
    let item = if context.source.durable().is_some() {
        item.map(str::to_owned)
    } else { None };
    let revision = runtime.organization_next_revision()?;
    runtime.organization.order_menu = Some(Menu {
        revision, context, item, choosing_category: false, rebase: true,
    });
    runtime.page = "actions";
    runtime.status.clear();
    runtime.changed();
    Ok(())
}

/// Validate the addressed menu before the normal UI gates. Search is read-only,
/// so its existing BeginSearch policy applies rather than the settings-write gate.
pub(in crate::app::ott_runtime) fn resolve_action(runtime: &Runtime, action: OttAction)
    -> Result<OttAction, &'static str>
{
    let OttAction::EditSetting(key) = &action else { return Ok(action); };
    let Some(value) = key.strip_prefix("order.").and_then(|key| key.strip_suffix(".search"))
        else { return Ok(action); };
    runtime.organization.order_menu.as_ref().filter(|menu|
        value == menu.revision.to_string() && !menu.choosing_category
            && runtime.page == "actions" && current(runtime, &menu.context, false)
            && classic_search_visible(runtime)
    ).ok_or(CHANGED)?;
    Ok(OttAction::BeginSearch)
}

fn begin_search(runtime: &mut Runtime, app: &mut App) -> Result<(), &'static str> {
    runtime.organization.order_menu.as_ref().filter(|menu|
        !menu.choosing_category && current(runtime, &menu.context, false)
            && classic_search_visible(runtime)
    ).ok_or(CHANGED)?;
    // Reuse the same admitted source/category. Failed editor admission must leave
    // the existing menu intact, including its nonce and captured row.
    runtime.page = "channels";
    let result = search::action(runtime, app, &OttAction::BeginSearch).expect("handled search action");
    if result.is_err() { runtime.page = "actions"; }
    else { runtime.organization.order_menu = None; }
    result
}

pub(in crate::app::ott_runtime) fn action(
    runtime: &mut Runtime,
    app: &mut App,
    selected: &OttAction,
) -> Option<Result<(), &'static str>> {
    if let Some(result) = contextual::action(runtime, selected) { return Some(result); }
    match selected {
        OttAction::BeginSearch if runtime.page == "actions" => Some(begin_search(runtime, app)),
        OttAction::ShowActions(key) => Some(open(runtime, key, None)),
        OttAction::ShowActionsAt { context, id } => Some(open(runtime, context, Some(id))),
        OttAction::ShuffleOrder(revision) => Some(change(runtime, *revision, false)),
        OttAction::ResetOrder(revision) => Some(change(runtime, *revision, true)),
        OttAction::CloseActions | OttAction::Back if runtime.page == "actions" => {
            runtime.organization.order_menu = None;
            runtime.page = "channels";
            runtime.status.clear();
            runtime.changed();
            Some(Ok(()))
        }
        OttAction::CloseActions => Some(Err(CHANGED)),
        _ => None,
    }
}

fn restore_focus(runtime: &mut Runtime, id: Option<&str>) {
    let Some(id) = id else { return; };
    if runtime.raw_view().rows.iter().any(|row|
        row.enabled && matches!(&row.action, OttAction::Play(value) if value == id))
    {
        browse_focus::record(runtime, id);
        browse_focus::request_flush(runtime);
    }
}

pub(super) fn saved(runtime: &mut Runtime, app: &App, persisted: bool) {
    let Some(pending) = runtime.organization.order_pending.take() else {
        return;
    };
    let authority_current = pending.menu.context.source.durable().is_none_or(|scope| {
        scope.authority.checked_add(u64::from(
            persisted && pending.menu.context.overlay != pending.overlay,
        )) == Some(runtime.organization.authority)
    });
    let still_open = authority_current && runtime.page == "actions"
        && runtime
            .organization
            .order_menu
            .as_ref()
            .is_some_and(|menu| menu.revision == pending.menu.revision)
        && current(runtime, &pending.menu.context, true)
        && current_ui_owner(app) == Some(pending.menu.context.owner)
        && runtime.reply == Some(pending.menu.context.owner);
    if !still_open {
        return;
    }
    if persisted
        && runtime.saved.settings.get(catalog_organization::KEY) == pending.overlay.as_ref()
    {
        runtime.page = "channels";
        runtime.organization.order_menu = None;
        restore_focus(runtime, pending.focus.as_deref());
    }
    // Failed writes leave the original order and menu retryable. Successful
    // writes publish through the same Saved image/cache; never restart playback.
    runtime.changed();
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_list_order_runtime_tests.rs"]
mod runtime_tests;
