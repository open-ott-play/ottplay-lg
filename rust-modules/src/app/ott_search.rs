//! Temporary title filtering of the Runtime's existing projected list.
//! No catalog, persistence, transport, worker or separate intent counter lives here.
use super::*;
use plx_screens::ott::{OttEditorKind, OttSearch};

pub(super) const MAX_QUERY_CHARS: usize = 256;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Context {
    profile: Option<String>,
    transient: Option<OwnerToken>,
    category: String,
    favorites: bool,
}
impl Context {
    fn page_key(&self) -> String {
        // Only an opaque digest is handed to focus restoration; no local path,
        // address, category text or search text becomes an instance identifier.
        let value = format!("{:?}|{:?}|{}:{}|{}", self.profile,
            self.transient.map(|owner| owner.0), self.category.len(), self.category, self.favorites);
        plx_data::ott::favorites_wire::hash(value.as_bytes())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum EditorPurpose {
    Setting,
    Search { context: Context, generation: u64 },
}

#[derive(Default)]
pub(super) struct Search {
    context: Option<Context>,
    query: String,
    key: String,
}
impl Search {
    fn reconcile(&mut self, context: Option<Context>) {
        if self.context != context {
            self.context = context;
            self.query.clear();
            self.key.clear();
        }
    }
    fn query(&self, context: &Context) -> &str {
        if self.context.as_ref() == Some(context) { &self.query } else { "" }
    }
    fn matches(&self, context: &Context, title: &str) -> bool {
        self.context.as_ref() != Some(context) || self.key.is_empty()
            || plx_base::unicode::caseless_key(title).contains(&self.key)
    }
    fn commit(&mut self, context: Context, value: String) -> Result<(), &'static str> {
        if value.chars().count() > MAX_QUERY_CHARS || value.chars().any(char::is_control) {
            return Err("Search must be a single line of at most 256 characters");
        }
        self.context = Some(context);
        self.key = plx_base::unicode::caseless_key(&value);
        self.query = value;
        Ok(())
    }
}

impl Runtime {
    fn search_context(&self) -> Option<Context> {
        if !self.loaded { return None; }
        let (category, favorites) = self.view_selection();
        let profile = self.effective_profile().map(profile_fingerprint);
        if !favorites && profile.is_none() { return None; }
        Some(Context {
            profile,
            transient: self.transient.accepted.as_ref().map(transient::TransientMedia::owner),
            category: if favorites { String::new() } else { category.into() },
            favorites,
        })
    }
    fn search_list_context(&self) -> Option<Context> {
        let context = self.search_context()?;
        match (self.page, context.favorites) {
            ("channels", false) | ("favorites", true) => Some(context),
            _ => None,
        }
    }
    pub(super) fn reconcile_search(&mut self) {
        let context = self.search_context();
        self.search.reconcile(context.clone());
        if self.edit_session.as_ref().is_some_and(|editor| match &editor.purpose {
            EditorPurpose::Setting => false,
            EditorPurpose::Search { context: owner, generation } =>
                context.as_ref() != Some(owner) || *generation != self.generation
                    || self.search_list_context().is_none(),
        }) { self.cancel_editor(); }
    }
    pub(super) fn search_filter(&self) -> impl Fn(&str) -> bool + '_ {
        let context = self.search_list_context();
        move |title| context.as_ref().is_none_or(|context| self.search.matches(context, title))
    }
    pub(super) fn search_view(&self) -> Option<OttSearch> {
        let context = self.search_list_context()?;
        let scope = if context.favorites {
            plx_platform::i18n::msg::ott_favorites().to_owned()
        } else {
            let name = self.transient.accepted.as_ref().and_then(transient::TransientMedia::root)
                .map(|root| root.name.as_str())
                .or_else(|| self.effective_profile().map(|p| p.name.as_str())).unwrap_or("");
            if self.effective_category().is_empty() { name.to_owned() }
            else {
                self.category_label().map_or_else(|| name.to_owned(),
                    |category| format!("{name} · {category}"))
            }
        };
        Some(OttSearch { query: self.search.query(&context).into(), scope })
    }
    pub(super) fn search_page_id(&self) -> Option<String> {
        self.search_list_context().map(|context|format!("{}:{}", self.page, context.page_key()))
    }
    fn begin_search(&mut self, app: &mut App) -> Result<(), &'static str> {
        let context = self.search_list_context().ok_or("Search is unavailable for this list")?;
        let scope = self.search_view().ok_or("Search is unavailable for this list")?.scope;
        let title = if context.favorites { String::new() } else { scope };
        let owner = self.reply.ok_or("Editor screen is no longer active")?;
        let id = self.edit_serial.checked_add(1)
            .ok_or("Editor session limit reached; reopen the player")?;
        self.cancel_editor();
        self.edit_serial = id;
        self.edit_session = Some(EditorSession {
            id, owner, key: String::new(),
            purpose: EditorPurpose::Search { context: context.clone(), generation: self.generation },
        });
        // Actions may still be the rendered page. Publish the admitted list
        // before its editor so navigation memory captures the actual source row.
        self.publish(app);
        app.pages.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(owner),
            Delivery::Screen(plx_ui::screen::ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor {
                session: id, kind: OttEditorKind::Search, key: String::new(),
                title,
                value: self.search.query(&context).into(), secret: false, limit: MAX_QUERY_CHARS,
                text_policy: OttTextPolicy::Predictive,
            }))))));
        Ok(())
    }
    fn clear_search(&mut self, app: &mut App) -> Result<(), &'static str> {
        let context = self.search_list_context().ok_or("Search is unavailable for this list")?;
        let editor = self.edit_session.as_ref().filter(|editor|
            Some(editor.owner) == self.reply && matches!(&editor.purpose,
                EditorPurpose::Search { context: owner, generation }
                    if owner == &context && *generation == self.generation)).cloned();
        self.search.commit(context, String::new())?;
        self.changed();
        // Search is an in-memory projection. Keep the catalog and its worker,
        // source selection, playback and any pending persistence untouched.
        self.publish(app);
        if let Some(editor) = editor {
            self.editor_result(app, &editor, None);
        }
        Ok(())
    }
    fn submit_search(&mut self, app: &mut App, session: u64, value: String) -> Result<(), &'static str> {
        let owner = self.reply.ok_or("Editor screen is no longer active")?;
        let request = self.edit_session.as_ref().filter(|editor|
            editor.id == session && editor.owner == owner
                && matches!(editor.purpose, EditorPurpose::Search { .. })).cloned();
        let result = (|| {
            let request = request.as_ref().ok_or("Search editor is no longer active")?;
            let EditorPurpose::Search { context, generation } = &request.purpose else { unreachable!() };
            if self.search_list_context().as_ref() != Some(context) || *generation != self.generation {
                return Err("The list changed; open Search again");
            }
            self.search.commit(context.clone(), value)?;
            self.changed();
            // Commit the projected view before acknowledging the addressed draft.
            self.publish(app);
            Ok(())
        })();
        if let Some(request) = request {
            self.editor_result(app, &request, result.as_ref().err().map(|error| (*error).into()));
        } else {
            app.pages.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(owner),
                Delivery::Screen(plx_ui::screen::ScreenEvent::App(AppMsg::OttEditorResult {
                    session, error: Some("Search editor is no longer active".into()),
                }))));
        }
        result
    }
}

pub(super) fn action(runtime: &mut Runtime, app: &mut App, action: &OttAction)
    -> Option<Result<(), &'static str>>
{
    Some(match action {
        OttAction::ShowCatalog => {
            runtime.cancel_setup();
            runtime.page = if runtime.view_selection().1 { "favorites" } else { "channels" };
            runtime.resume_returned_catalog();
            Ok(())
        }
        OttAction::BeginSearch => runtime.begin_search(app),
        OttAction::ClearSearch => runtime.clear_search(app),
        OttAction::SubmitSearch { session, value } => runtime.submit_search(app, *session, value.clone()),
        OttAction::CancelSearch { session } => {
            if runtime.edit_session.as_ref().is_some_and(|editor|
                editor.id == *session && Some(editor.owner) == runtime.reply
                    && matches!(editor.purpose, EditorPurpose::Search { .. })) {
                runtime.cancel_editor();
            }
            Ok(())
        }
        _ => return None,
    })
}
