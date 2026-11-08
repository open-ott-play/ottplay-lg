//! Addressed organization drafts and projection of the ordinary provider catalog.
//! This owner never edits provider rows, Favorites, credentials or playback IDs.
use super::*;
use catalog_organization::{Direction, Document as Organization, Operation, Projection, Source};
use std::cell::RefCell;
#[path = "ott_list_order.rs"]
pub(super) mod list_order;

const STALE: &str = "The catalog changed; reopen its organization before editing";
const UNAVAILABLE: &str = "Catalog organization is unavailable for this source";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode { Categories, Category, Item, AddItem, ConfirmDelete, ConfirmReset }
#[derive(Clone)]
pub(super) struct Row { pub id: String, pub title: String }
pub(super) struct View {
    pub revision: u64,
    pub mode: Mode,
    pub categories: Vec<Row>,
    pub items: Vec<Row>,
    pub category: Option<Row>,
    pub item: Option<Row>,
    pub pending: bool,
    pub alphabetical: bool,
}
impl View {
    pub(super) fn key(&self, operation: &str) -> String {
        format!("organization.{}.{operation}", self.revision)
    }
}
#[derive(Clone)]
struct LocalContext(transient::LocalSourceRead);
impl PartialEq for LocalContext {
    fn eq(&self, other: &Self) -> bool {
        // Identity only: use the shared runtime capability check for liveness.
        self.0.selection == other.0.selection && self.0.same_grant(&other.0)
    }
}
#[derive(Clone, PartialEq)]
struct Context {
    profile: ProviderProfile,
    local: Option<LocalContext>,
    generation: u64,
    catalog_revision: u64,
    overlay: Option<Value>,
    owner: InstanceId,
    navigation: u64,
    authority: u64,
}
#[derive(Clone)]
struct Session {
    revision: u64,
    context: Context,
    projection: Arc<Projection>,
    alphabetical: bool,
    mode: Mode,
    category: Option<String>,
    item: Option<String>,
}
fn category_exists(projection: &Projection, id: &str) -> bool {
    id == catalog_organization::ALL_ID || projection.category(id).is_some()
}
fn members<'a>(projection: &'a Projection, id: &str) -> Option<&'a Vec<String>> {
    if id == catalog_organization::ALL_ID { Some(&projection.all) }
    else { projection.category(id).map(|category| &category.members) }
}
fn category_rows(projection: &Projection) -> Vec<Row> {
    std::iter::once(Row { id: catalog_organization::ALL_ID.into(), title: plx_platform::i18n::msg::browse_library_all().into() })
        .chain(projection.categories.iter().map(|category| Row { id: category.id.clone(), title: category.label.clone() })).collect()
}
#[derive(Clone, Copy)]
enum TextOperation { Create, Rename, Copy }
#[derive(Clone)]
struct Draft { session: u64, editor: EditorSession, operation: TextOperation }
struct Pending { session: Session, overlay: Option<Value> }
struct Cached {
    generation: u64,
    catalog_revision: u64,
    profile: ProviderProfile,
    overlay: Option<Value>,
    projection: Arc<Projection>,
    alphabetical: bool,
    positions: std::collections::HashMap<String, usize>,
}
#[derive(Default)]
pub(super) struct State {
    serial: u64,
    authority: u64,
    exhausted: bool,
    observed: Option<(Option<ProviderProfile>, Option<Value>)>,
    session: Option<Session>,
    draft: Option<Draft>,
    pending: Option<Pending>,
    rebase: bool,
    cache: RefCell<Option<Cached>>,
    order_menu: Option<list_order::Menu>,
    order_pending: Option<list_order::Pending>,
}
fn invalid(error: catalog_organization::Error) -> &'static str { error.message() }

impl Runtime {
    fn organization_profile(&self) -> Option<&ProviderProfile> {
        if self.transient.accepted.is_some() || self.transient.candidate.is_some()
            || managed::profile(self).is_some() { return None; }
        let profile = self.effective_profile()?;
        // A Classic bridge can reuse a saved profile without activating it.
        // Never turn an unsaved or ambiguous effective source into a durable owner.
        let mut saved = self.saved.profiles.profiles.iter().filter(|saved| saved.id == profile.id);
        if saved.next() != Some(profile) || saved.next().is_some()
            || (classic_source::profile(self) != Some(profile) && profile.kind != self.provider)
        { return None; }
        if profile.kind == ProviderKind::Local && self.local_profile_selection(profile)
            .and_then(|selection| self.capture_local_source(&selection)).is_none() { return None; }
        (profile.enabled && Source::new(profile).is_ok()).then_some(profile)
    }
    pub(super) fn organization_available(&self) -> bool {
        self.loaded && !self.persistence_uncertain && !self.saving && !self.organization.exhausted
            && self.catalog_complete && !self.catalog_loading && !self.view_selection().1
            && self.organization_profile().is_some()
            && matches!(super::super::ott_kiosk_stored::read(&self.saved.settings), Ok(None))
    }
    pub(super) fn organization_projection(&self) -> Option<Arc<Projection>> {
        let profile = self.organization_profile()?;
        let overlay = self.saved.settings.get(catalog_organization::KEY);
        let mut cache = self.organization.cache.borrow_mut();
        if let Some(cached) = cache.as_ref().filter(|cached| cached.generation == self.generation
            && cached.catalog_revision == self.catalog_revision && &cached.profile == profile
            && cached.overlay.as_ref() == overlay) { return Some(cached.projection.clone()); }
        let source = Source::new(profile).ok()?;
        let document = Organization::read(&self.saved.settings).ok()?;
        let alphabetical = document.alphabetical(&source);
        self.catalog_diagnostics.organization_rebuilt(self.generation);
        let projection = Arc::new(plx_base::diag::spans::span("ott_organization", || document.project(&source, &self.catalog)).ok()?);
        *cache = Some(Cached { generation: self.generation, catalog_revision: self.catalog_revision,
            profile: profile.clone(), overlay: overlay.cloned(), projection: projection.clone(), alphabetical,
            positions: self.catalog.entries.iter().enumerate().map(|(index, entry)| (entry.id.clone(), index)).collect() });
        Some(projection)
    }
    fn organization_snapshot(&self) -> Option<(Arc<Projection>, bool)> {
        let projection = self.organization_projection()?;
        let alphabetical = self.organization.cache.borrow().as_ref()?.alphabetical;
        Some((projection, alphabetical))
    }
    pub(super) fn organization_categories(&self) -> Vec<Row> {
        if let Some(projection) = self.organization_projection() {
            projection.categories.iter().map(|category| Row { id: category.id.clone(), title: category.label.clone() }).collect()
        } else { self.catalog.category_choices().into_iter().map(|category| Row { id: category.id, title: category.label }).collect() }
    }
    pub(super) fn organization_category_id(&self, selection: &str) -> String {
        self.organization_projection().and_then(|projection| projection.selection_id(selection).map(str::to_owned))
            .or_else(|| self.catalog.resolve_category(selection))
            .unwrap_or_else(|| selection.to_owned())
    }
    pub(super) fn organization_rows(&self, selection: &str) -> Vec<(usize, &CatalogEntry)> {
        if let Some(transient::TransientMedia::Local(session)) = &self.transient.accepted {
            let entries: std::collections::HashMap<&str, (usize, &CatalogEntry)> = self.catalog.entries.iter()
                .enumerate().map(|(index, entry)| (entry.id.as_str(), (index, entry))).collect();
            return session.context.order().iter().filter_map(|id| entries.get(id.as_str()).copied())
                .filter(|(_, entry)| selection.is_empty() || entry.category == selection).collect();
        }
        if let Some(projection) = self.organization_projection() {
            let cache = self.organization.cache.borrow();
            let Some(cache) = cache.as_ref() else { return Vec::new(); };
            projection.ordered_ids(selection).iter().filter_map(|id| cache.positions.get(id).copied()
                .and_then(|index| self.catalog.entries.get(index).map(|entry| (index, entry)))).collect()
        } else {
            self.catalog.category_indices(selection).into_iter()
                .filter_map(|index| self.catalog.entries.get(index).map(|entry| (index, entry))).collect()
        }
    }
    fn organization_context(&self) -> Result<Context, &'static str> {
        if !self.organization_available() { return Err(UNAVAILABLE); }
        let profile = self.organization_profile().ok_or(UNAVAILABLE)?.clone();
        let local = if profile.kind == ProviderKind::Local {
            let selection = self.local_profile_selection(&profile).ok_or(UNAVAILABLE)?;
            Some(LocalContext(self.capture_local_source(&selection).ok_or(UNAVAILABLE)?))
        } else { None };
        Ok(Context { profile, local, generation: self.generation,
            catalog_revision: self.catalog_revision, overlay: self.saved.settings.get(catalog_organization::KEY).cloned(),
            owner: self.reply.ok_or(STALE)?, navigation: self.navigation_epoch, authority: self.organization.authority })
    }
    fn organization_local_current(&self, context: &Context) -> bool {
        match &context.local {
            Some(local) => context.profile.kind == ProviderKind::Local
                && self.local_source_read_is_current(&local.0)
                && self.local_profile_selection(&context.profile).as_ref() == Some(&local.0.selection),
            None => context.profile.kind != ProviderKind::Local,
        }
    }
    fn organization_current(&self, session: &Session) -> bool {
        self.page == "organization" && self.organization_local_current(&session.context)
            && self.organization_context().as_ref() == Ok(&session.context)
    }
    fn organization_next_revision(&mut self) -> Result<u64, &'static str> {
        self.organization.serial = self.organization.serial.checked_add(1).ok_or("Reopen the player before editing its catalog")?;
        Ok(self.organization.serial)
    }
    pub(super) fn observe_organization_context(&mut self) {
        let observed = (self.organization_profile().cloned(), self.saved.settings.get(catalog_organization::KEY).cloned());
        if self.organization.observed.as_ref() != Some(&observed) {
            if let Some(next) = self.organization.authority.checked_add(1) { self.organization.authority = next; }
            else { self.organization.exhausted = true; }
            self.organization.observed = Some(observed);
        }
    }
    fn organization_open(&mut self) -> Result<(), &'static str> {
        self.observe_organization_context();
        let context = self.organization_context()?;
        let (projection, alphabetical) = self.organization_snapshot().ok_or(UNAVAILABLE)?;
        let revision = self.organization_next_revision()?;
        self.cancel_setup();
        self.organization.session = Some(Session { revision, context, projection, alphabetical, mode: Mode::Categories, category: None, item: None });
        self.organization.rebase = true;
        self.page = "organization";
        self.status.clear();
        Ok(())
    }
    pub(super) fn organization_view(&self) -> Option<View> {
        let session = self.organization.session.as_ref()?;
        // The committed image remains visible while the sole writer owns a candidate.
        let pending = self.organization.pending.as_ref().is_some_and(|pending| pending.session.revision == session.revision);
        if pending {
            if self.reply != Some(session.context.owner) || self.page != "organization"
                || self.navigation_epoch != session.context.navigation || self.generation != session.context.generation
                || self.catalog_revision != session.context.catalog_revision
                || !self.organization_local_current(&session.context)
                || self.organization_profile() != Some(&session.context.profile) { return None; }
        } else if !self.organization_current(session) { return None; }
        let categories = category_rows(&session.projection);
        let category = session.category.as_deref().and_then(|id| categories.iter().find(|category| category.id == id)).cloned();
        let ids = if session.mode == Mode::AddItem { &session.projection.available }
            else { session.category.as_deref().and_then(|id| members(&session.projection, id)).unwrap_or(&session.projection.all) };
        let items: Vec<_> = if matches!(session.mode, Mode::Categories | Mode::ConfirmDelete | Mode::ConfirmReset) { Vec::new() }
            else {
                let titles: std::collections::HashMap<_, _> = self.catalog.entries.iter().map(|entry| (entry.id.as_str(), entry.title.as_str())).collect();
                ids.iter().filter_map(|id| titles.get(id.as_str()).map(|title| Row { id: id.clone(), title: (*title).into() })).collect()
            };
        let item = session.item.as_deref().and_then(|id| items.iter().find(|item| item.id == id)).cloned();
        Some(View { revision: session.revision, mode: session.mode, categories, items, category, item, pending,
            alphabetical: session.alphabetical })
    }
    pub(super) fn rebase_organization_navigation(&mut self) {
        list_order::rebase(self);
        if std::mem::take(&mut self.organization.rebase) {
            if let Some(session) = self.organization.session.as_mut() { session.context.navigation = self.navigation_epoch; }
            if let Some(pending) = self.organization.pending.as_mut() { pending.session.context.navigation = self.navigation_epoch; }
        }
    }
    pub(super) fn organization_cancel_editor(&mut self) {
        self.organization.draft = None;
    }
    fn organization_candidate(&self, session: &Session, operation: Operation) -> Result<Option<Saved>, &'static str> {
        if !self.organization_current(session) { return Err(STALE); }
        let source = Source::new(&session.context.profile).map_err(invalid)?;
        let mut document = Organization::read(&self.saved.settings).map_err(invalid)?;
        if matches!(operation, Operation::MoveMember { .. }) && document.alphabetical(&source) {
            return Err("Turn off alphabetical sorting before moving an item");
        }
        let change = document.apply(&source, &self.catalog, operation).map_err(invalid)?;
        if !change.changed { return Ok(None); }
        let after = document.project(&source, &self.catalog).map_err(invalid)?;
        if change.category.as_deref().is_some_and(|id| !id.is_empty() && !category_exists(&after, id)) {
            return Err("Invalid category organization");
        }
        let mut candidate = self.saved.clone();
        document.write_into(&mut candidate.settings).map_err(invalid)?;
        category_state::organization_remap(&mut candidate, &session.context.profile, &session.projection, &after)?;
        Ok(Some(candidate))
    }
    fn organization_apply(&mut self, session: &Session, operation: Operation) -> Result<(), &'static str> {
        let Some(candidate) = self.organization_candidate(session, operation)? else {
            self.organization.rebase = true; return Ok(());
        };
        let overlay = candidate.settings.get(catalog_organization::KEY).cloned();
        self.save(candidate, None, false)?;
        self.organization.pending = Some(Pending { session: session.clone(), overlay });
        self.organization.rebase = true;
        Ok(())
    }
    pub(super) fn finish_organization_save(&mut self, app: &App, persisted: bool) {
        list_order::saved(self, app, persisted);
        let Some(pending) = self.organization.pending.take() else { return; };
        let session = &pending.session;
        let expected_authority = session.context.authority.checked_add(u64::from(
            persisted && session.context.overlay != pending.overlay));
        let same = expected_authority == Some(self.organization.authority)
            && self.organization.session.as_ref().is_some_and(|current| current.revision == session.revision)
            && self.page == "organization" && current_ui_owner(app) == Some(session.context.owner)
            && self.reply == Some(session.context.owner)
            && self.navigation_epoch == session.context.navigation
            && self.generation == session.context.generation && self.catalog_revision == session.context.catalog_revision
            && self.organization_local_current(&session.context)
            && self.organization_profile() == Some(&session.context.profile);
        if !same { self.organization.session = None; return; }
        if !self.organization_available() || self.require_parental(parental::Target::Settings).is_err() {
            self.organization.session = None; return;
        }
        if persisted && self.saved.settings.get(catalog_organization::KEY) != pending.overlay.as_ref() {
            self.organization.session = None; return;
        }
        if !persisted { return; }
        let Some((projection, alphabetical)) = self.organization_snapshot() else { self.organization.session = None; return; };
        // Saved preferences are remapped in the same write. The Classic bridge
        // also retains its current view in memory, including for an inactive profile.
        let category = classic_source::view_selection(self)
            .and_then(|current| session.projection.selection_id(current))
            .map(|id| if projection.category(id).is_some() { id.to_owned() } else { String::new() });
        if let Some(category) = category { classic_source::set_view(self, &category); }
        let Ok(revision) = self.organization_next_revision() else { self.organization.session = None; return; };
        if let Some(current) = self.organization.session.as_mut() {
            current.revision = revision;
            current.context.overlay = pending.overlay;
            current.context.authority = self.organization.authority;
            current.projection = projection;
            current.alphabetical = alphabetical;
            if current.category.as_deref().is_some_and(|id| !category_exists(&current.projection, id)) {
                current.category = None; current.item = None; current.mode = Mode::Categories;
            } else if matches!(current.mode, Mode::ConfirmDelete | Mode::ConfirmReset) { current.mode = Mode::Categories; }
            else if current.mode == Mode::AddItem { current.mode = Mode::Category; }
            else if current.item.as_deref().is_some_and(|id| current.category.as_deref()
                .and_then(|category| members(&current.projection, category)).is_none_or(|members| !members.iter().any(|member| member == id))) {
                current.item = None; current.mode = Mode::Category;
            }
        }
        if let Some(draft) = self.organization.draft.as_mut().filter(|draft| draft.session == session.revision) {
            draft.session = revision;
        }
        self.catalog_dirty = true;
    }
    fn organization_text(&mut self, app: &mut App, session: &Session, operation: TextOperation, key: &str) -> Result<(), &'static str> {
        let category = session.category.as_deref().and_then(|id| session.projection.category(id));
        if !matches!(operation, TextOperation::Create) && category.is_none()
            && !(matches!(operation, TextOperation::Copy) && session.category.as_deref() == Some(catalog_organization::ALL_ID)) { return Err(STALE); }
        let title = match operation { TextOperation::Create => "Create category",
            TextOperation::Rename => "Rename category",
            TextOperation::Copy => "Copy category" };
        let value = match operation { TextOperation::Rename => category.map(|c| c.label.clone()).unwrap_or_default(), _ => String::new() };
        let id = self.edit_serial.checked_add(1).ok_or("Editor session limit reached; reopen the player")?;
        self.cancel_editor();
        self.edit_serial = id;
        let editor = EditorSession { id, owner: session.context.owner, key: key.into(), purpose: search::EditorPurpose::Setting };
        self.edit_session = Some(editor.clone());
        self.organization.draft = Some(Draft { session: session.revision, editor: editor.clone(), operation });
        self.organization.rebase = true;
        app.pages.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(editor.owner), Delivery::Screen(
            plx_ui::screen::ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor {
                kind: plx_screens::ott::OttEditorKind::Setting, session: id, key: key.into(),
                title: title.into(), value, secret: false, limit: 512,
                text_policy: OttTextPolicy::Predictive,
            }))))));
        Ok(())
    }
    pub(super) fn submit_organization(&mut self, request: &EditorSession, value: &str) -> Result<bool, &'static str> {
        let draft = self.organization.draft.as_ref().filter(|draft| &draft.editor == request).cloned().ok_or(STALE)?;
        let session = self.organization.session.as_ref().filter(|session| session.revision == draft.session).cloned().ok_or(STALE)?;
        let category = session.category.clone().unwrap_or_default();
        let label = value.trim().to_owned();
        let operation = match draft.operation {
            TextOperation::Create => Operation::Create { label }, TextOperation::Rename => Operation::Rename { category, label },
            TextOperation::Copy => Operation::Copy { category, label },
        };
        self.organization_apply(&session, operation)?;
        if self.organization.pending.is_some() { self.pending_editor_save = Some(request.clone()); Ok(false) }
        else { Ok(true) }
    }
}

/// Cleanup is narrowly addressed and runs after kiosk input policy but before
/// the Settings grant gate. Expiring a grant must not trap its own open menu.
pub(super) fn cleanup_action(runtime: &mut Runtime, app: &mut App, action: &OttAction)
    -> Option<Result<(), &'static str>> {
    if matches!(action, OttAction::Back) && runtime.page == "organization"
        && runtime.reply.is_some() && runtime.reply == current_ui_owner(app)
        && runtime.organization.session.as_ref().is_none_or(|session|
            session.context.navigation != runtime.navigation_epoch || Some(session.context.owner) != runtime.reply)
    {
        runtime.organization.session = None;
        runtime.cancel_editor();
        runtime.page = "categories";
        runtime.organization.rebase = false;
        return Some(Ok(()));
    }
    let revision = match action {
        OttAction::EditSetting(key) => {
            let tail = key.strip_prefix("organization.")?;
            let (revision, operation) = tail.split_once('.')?;
            if operation != "back" { return None; }
            Some(revision.parse::<u64>())
        },
        OttAction::Back if runtime.page == "organization" => None,
        _ => return None,
    };
    Some((|| {
        let session = runtime.organization.session.as_ref().cloned().ok_or(STALE)?;
        if revision.is_some_and(|revision| revision != Ok(session.revision))
            || runtime.page != "organization" || runtime.reply != Some(session.context.owner)
            || current_ui_owner(app) != Some(session.context.owner)
            || runtime.navigation_epoch != session.context.navigation
        { return Err(STALE); }
        let close = runtime.organization.pending.is_some() || !runtime.organization_current(&session)
            || session.mode == Mode::Categories;
        if close {
            runtime.organization.session = None;
            runtime.cancel_editor();
            runtime.page = "categories";
            runtime.organization.rebase = false;
            return Ok(());
        }
        let mut next = session;
        match next.mode {
            Mode::Category | Mode::ConfirmReset => { next.category = None; next.item = None; next.mode = Mode::Categories; },
            _ => { next.item = None; next.mode = Mode::Category; },
        }
        next.revision = runtime.organization_next_revision()?;
        runtime.organization.session = Some(next);
        runtime.organization.rebase = true;
        Ok(())
    })())
}

pub(super) fn action(runtime: &mut Runtime, app: &mut App, action: &OttAction) -> Option<Result<(), &'static str>> {
    if let OttAction::CancelSetting { session } = action {
        if runtime.organization.draft.as_ref().is_some_and(|draft| draft.editor.id == *session
            && Some(draft.editor.owner) == runtime.reply) { runtime.organization.rebase = true; }
        return None;
    }
    let OttAction::EditSetting(key) = action else { return None; };
    if key == "organization.open" { return Some(runtime.organization_open()); }
    let tail = key.strip_prefix("organization.")?;
    Some((|| {
        let (revision, operation) = tail.split_once('.').ok_or(STALE)?;
        let revision: u64 = revision.parse().map_err(|_| STALE)?;
        let session = runtime.organization.session.as_ref().filter(|session| session.revision == revision).cloned().ok_or(STALE)?;
        if !runtime.organization_current(&session) { return Err(STALE); }
        if runtime.edit_session.is_some() { return Err("Close the current editor before organizing the catalog"); }
        if matches!(operation, "create" | "rename" | "copy") {
            return runtime.organization_text(app, &session, match operation { "create" => TextOperation::Create, "rename" => TextOperation::Rename, _ => TextOperation::Copy }, key);
        }
        let category = || session.category.clone().ok_or(STALE);
        let item = || session.item.clone().ok_or(STALE);
        let mutation = match operation {
            "up" | "down" => Some(Operation::MoveCategory { category: category()?, direction: if operation == "up" { Direction::Up } else { Direction::Down } }),
            "member.remove" => Some(Operation::RemoveMember { category: category()?, item: item()? }),
            "member.up" | "member.down" => Some(Operation::MoveMember { category: category()?, item: item()?, direction: if operation == "member.up" { Direction::Up } else { Direction::Down } }),
            "confirm" if session.mode == Mode::ConfirmDelete => Some(Operation::Delete { category: category()? }),
            "confirm" if session.mode == Mode::ConfirmReset => Some(Operation::Reset),
            _ if operation.starts_with("add:") && session.mode == Mode::AddItem => {
                let index: usize = operation[4..].parse().map_err(|_| STALE)?;
                Some(Operation::AddMember { category: category()?, item: session.projection.available.get(index).cloned().ok_or(STALE)? })
            }
            _ => None,
        };
        if let Some(mutation) = mutation { return runtime.organization_apply(&session, mutation); }
        let mut next = session.clone();
        match operation {
            "delete" => { category()?; next.mode = Mode::ConfirmDelete; },
            "reset" => next.mode = Mode::ConfirmReset,
            "member.add" => { category()?; next.mode = Mode::AddItem; },
            _ if operation.starts_with("category:") => {
                let index: usize = operation[9..].parse().map_err(|_| STALE)?;
                next.category = Some(category_rows(&next.projection).get(index).ok_or(STALE)?.id.clone());
                next.item = None; next.mode = Mode::Category;
            },
            _ if operation.starts_with("item:") => {
                let index: usize = operation[5..].parse().map_err(|_| STALE)?;
                next.item = Some(members(&next.projection, &category()?).ok_or(STALE)?.get(index).cloned().ok_or(STALE)?);
                next.mode = Mode::Item;
            },
            _ => return Err("Unknown catalog organization action"),
        }
        next.revision = runtime.organization_next_revision()?;
        runtime.organization.session = Some(next);
        runtime.organization.rebase = true;
        Ok(())
    })())
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_catalog_organization_runtime_tests.rs"]
mod tests;
