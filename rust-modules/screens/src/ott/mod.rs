//! Native OTT catalogue surface. The application supplies immutable display
//! snapshots and receives semantic actions; this screen performs no IO.
mod metadata;
mod classic;
mod classic_settings_list;
mod viewport;
mod media;
mod memory;
mod search_return;
pub use memory::Memory;
mod navigation;
mod remote;
mod exit_prompt;
pub mod ui_text;

use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::convert::Infallible;
use std::sync::Arc;
use plx_platform::i18n::msg;
use crate::registry::{AppFx, AppLike, AppMsg, PageMemory};
use plx_machine::machine::{Canon, Cx, Delivery, Edge, Effects, EntryId, FocusKey, Fx, GroupId,
    Handled, InputKind, InstanceId, Key, LogicalState, Machine, MachineId, TextEdit};
use plx_ui::form::{Activation, Form, FormSection, FormTable, RowKey, RowKind};
use plx_ui::frame::Budget;
use plx_ui::route_screen::RouteLayout;
use plx_ui::screen::{Activate, At, Dir, DrawFrame, EdgeRule, ElemKind, FocusSource, Focusable, GroupSpec,
    HitSource, Hover, Link, Part, Placed, RenderStrategy, Screen, ScreenEvent, Step, Stop};
use plx_ui::table::Row;
use plx_ui::table_screen::{Header, TableScreen};
use plx_ui::text_buffer::TextBuffer;
use plx_ui::document_reader::DocumentReader;
use plx_ui::table_screen::{DocumentFocus, DocumentPart};
use plx_ui::{theme, Rect};
use plx_ui::widgets::{CtlPop, TabStrip};

pub const SHAPE: &str = "Ott{classic:bool,visible:bool,guide_clock:{sample:Option<i64>,anchor:u32,elapsed_s:u32},viewport:{source:bool,next:Option<u32>,pending:Option<Request>,submitted:Option<Request>,blocked:Option<Target>},category_label:Option<str>,classic_preferences:{page_size:u32,list_left:bool,arrow_mode:u32,seek13_seconds:u32,show_number:bool,show_name:bool,show_program:bool,show_progress:bool,show_description:bool},volume_available:bool,page:str,revision:u64,rows:[{id:str,enabled:bool,favorite:bool,guide_available:bool,records_available:bool}],shelves:[{id:str,rows:[u64]}],shelf_context:Option<str>,card_focus:Option<u32>,preview_context:Option<str>,random_context:Option<str>,order_context:Option<str>,editor:bool,editing:bool,editor_session:Option<u64>,editor_search:bool,editor_literal:bool,browse:bool,browse_return:Option<str>,focus_request:Option<u32>,editor_origin:Option<str>,editor_focus:Option<str>,pending:bool,error:bool,media:Option<str>,reading:bool,read_target:u32,preview:Option<str>,preview_token:Option<str>,previewed:Option<str>,preview_dwell:f32,selected:u32,table:TableViewMotion,header_capsules:TabStrip{sel:Capsule{x:Spring{pos:f32,vel:f32},w:Spring{pos:f32,vel:f32},a:Spring{pos:f32,vel:f32},at:u32},foc:Capsule{x:Spring{pos:f32,vel:f32},w:Spring{pos:f32,vel:f32},a:Spring{pos:f32,vel:f32},at:u32}},header_pop:{sp:[Spring{pos:f32,vel:f32};1],focused:Option<u32>},cards:Stack{scroll:{pos:f32,vel:f32},target:f32,sections:[Grid{bands,pop}|Shelf{motion}]},restore_memory:Option<OttMemory>,search_return:{origin:Option<SearchOrigin{memory:OttMemory,context:Option<str>}>,draft:Option<SearchOrigin{memory:OttMemory,context:Option<str>}>,from_actions:Option<SearchOrigin{memory:OttMemory,context:Option<str>}>,pending:Option<u64>},number:{until:Option<u32>,notice:bool},exit:{token:Option<u64>,open:bool,affirm:bool,scroll:u32,return_focus:Option<u32>}}";

/// Presentation order for the already loaded single-channel guide window.
/// This does not request or claim a complete archive/recordings history.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OttGuideOrder {
    #[default]
    Chronological,
    Alphabetical,
}

/// IDs are opaque local identities, never URLs or credentials.
#[derive(Clone, PartialEq, Eq)]
pub enum OttAction {
    ShowProviders, ShowProfiles, ShowCategories, ShowFavorites, ShowSettings, ShowCatalog, BeginSearch, ClearSearch,
    SelectProvider(String), SelectProfile(String), SelectCategory(String),
    SelectManagedPlex { server_id: String, context: String },
    ResumeManagedPlex { operation: u64 },
    EditProfile { context: String, target: String },
    SelectCategoryAt { context: String, category: String },
    SelectVPortalQuality { context: String, variant: String },
    ShowM3uMediaLinks,
    EditM3uMediaLink { profile: String, context: String },
    ShowActions(String), ShowActionsAt { context: String, id: String }, ShuffleOrder(u64), ResetOrder(u64), CloseActions,
    Random(String), Play(String), Preview(String), PreviewAt { context: String, id: String }, FocusAt { context: String, id: String }, ToggleFavorite(String), EditSetting(String),
    SubmitSetting { session: u64, key: String, value: String },
    CancelSetting { session: u64 },
    ExitAnswer { token: u64, accept: bool },
    MenuVisibility { revision: u64, context: u64, change: plx_appkit::menu_visibility::Change },
    SubmitSearch { session: u64, value: String },
    CancelSearch { session: u64 },
    OpenGuide(String),
    OpenRecords(String),
    GuideDay { generation: u64, day_start_s: i64 },
    GuideOrder { generation: u64, order: OttGuideOrder },
    GuideProgramme { generation: u64, programme_id: String },
    PlayArchive { generation: u64, programme_id: String },
    ReturnLive { generation: u64 },
    ToggleReminder { generation: u64, programme_id: String },
    ReminderAnswer { token: plx_data::ott::reminders::PromptToken, accept: bool },
    VolumeStep(i8),
    Back, Refresh,
}

impl std::fmt::Debug for OttAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SubmitSetting { key, .. } => f.debug_struct("SubmitSetting").field("key", key).field("value", &"<redacted>").finish(),
            Self::SubmitSearch { session, .. } => f.debug_struct("SubmitSearch").field("session", session).field("value", &"<redacted>").finish(),
            Self::CancelSearch { session } => f.debug_struct("CancelSearch").field("session", session).finish(),
            Self::MenuVisibility { revision, context, change } => f.debug_struct("MenuVisibility").field("revision", revision).field("context", context).field("change", change).finish(),
            Self::CancelSetting { session } => f.debug_struct("CancelSetting").field("session", session).finish(),
            Self::ExitAnswer { token, accept } => f.debug_struct("ExitAnswer").field("token", token).field("accept", accept).finish(),
            Self::SelectProvider(id) => f.debug_tuple("SelectProvider").field(id).finish(),
            Self::ResumeManagedPlex { .. } => f.write_str("ResumeManagedPlex"),
            Self::SelectManagedPlex { .. } => f.write_str("SelectManagedPlex(<context>)"),
            Self::SelectProfile(id) => f.debug_tuple("SelectProfile").field(id).finish(),
            Self::EditProfile { .. } => f.write_str("EditProfile(<context>, <target>)"),
            Self::SelectCategory(id) => f.debug_tuple("SelectCategory").field(id).finish(),
            Self::SelectCategoryAt { category, .. } => f.debug_tuple("SelectCategoryAt").field(category).finish(),
            Self::SelectVPortalQuality { .. } => f.write_str("SelectVPortalQuality(<context>, <variant>)"),
            Self::ShowActions(_) => f.write_str("ShowActions(<context>)"),
            Self::ShowActionsAt { .. } => f.write_str("ShowActionsAt(<context>, <item>)"),
            Self::ShuffleOrder(_) => f.write_str("ShuffleOrder"),
            Self::ResetOrder(_) => f.write_str("ResetOrder"),
            Self::CloseActions => f.write_str("CloseActions"),
            Self::Random(_) => f.write_str("Random(<context>)"),
            Self::Play(id) => f.debug_tuple("Play").field(id).finish(),
            Self::Preview(id) => f.debug_tuple("Preview").field(id).finish(),
            Self::PreviewAt { id, .. } => f.debug_tuple("PreviewAt").field(id).finish(),
            Self::FocusAt { id, .. } => f.debug_tuple("FocusAt").field(id).finish(),
            Self::ToggleFavorite(id) => f.debug_tuple("ToggleFavorite").field(id).finish(),
            Self::ShowM3uMediaLinks => f.write_str("ShowM3uMediaLinks"),
            Self::EditM3uMediaLink { .. } => f.write_str("EditM3uMediaLink(<private>)"),
            Self::EditSetting(id) => f.debug_tuple("EditSetting").field(id).finish(),
            Self::OpenGuide(_) => f.write_str("OpenGuide"),
            Self::OpenRecords(_) => f.write_str("OpenRecords"),
            Self::VolumeStep(direction) => f.debug_tuple("VolumeStep").field(direction).finish(),
            Self::GuideDay { generation, .. } => f.debug_tuple("GuideDay").field(generation).finish(),
            Self::GuideOrder { generation, order } => f.debug_tuple("GuideOrder").field(generation).field(order).finish(),
            Self::GuideProgramme { generation, .. } => f.debug_tuple("GuideProgramme").field(generation).finish(),
            Self::PlayArchive { generation, .. } => f.debug_tuple("PlayArchive").field(generation).finish(),
            Self::ReturnLive { generation } => f.debug_tuple("ReturnLive").field(generation).finish(),
            Self::ToggleReminder { generation, .. } => f.debug_tuple("ToggleReminder").field(generation).finish(),
            Self::ReminderAnswer { accept, .. } => f.debug_tuple("ReminderAnswer").field(accept).finish(),
            Self::ShowProviders => f.write_str("ShowProviders"), Self::ShowProfiles => f.write_str("ShowProfiles"),
            Self::ShowCategories => f.write_str("ShowCategories"), Self::ShowFavorites => f.write_str("ShowFavorites"),
            Self::ShowCatalog => f.write_str("ShowCatalog"), Self::BeginSearch => f.write_str("BeginSearch"),
            Self::ClearSearch => f.write_str("ClearSearch"),
            Self::ShowSettings => f.write_str("ShowSettings"), Self::Back => f.write_str("Back"), Self::Refresh => f.write_str("Refresh"),
        }
    }
}

#[derive(Clone)]
pub enum OttOut {
    Refresh { reply: InstanceId },
    Action { reply: InstanceId, action: OttAction },
}

/// Distinguishes a durable setting from a temporary catalogue filter. Both use
/// the same addressed editor, but their actions never share a persistence path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OttEditorKind { Setting, Search }

/// Commit semantics chosen by the field owner, independently of its key or secrecy.
/// Literal fields insert each normalized commit; predictive fields retain word replacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OttTextPolicy { Predictive, Literal }

/// An addressed settings or search request. Values stay in the screen's draft until Save;
/// sensitive values are never included in its logical-state diagnostics.
#[derive(Clone)]
pub struct OttEditor {
    pub session: u64,
    pub kind: OttEditorKind,
    pub text_policy: OttTextPolicy,
    pub key: String,
    /// Raw application fallback for Setting; captured unformatted scope for Search.
    pub title: String,
    pub value: String,
    pub secret: bool,
    pub limit: usize,
}

#[derive(Clone)]
enum FormAction { Forward(OttAction), Edit, Save, Cancel, Details, CloseBrowse }

struct Editor {
    spec: Arc<OttEditor>,
    // Presentation only: rebuilt from the immutable request when the locale changes.
    title: String,
    draft: TextBuffer,
    pending: bool,
    error: String,
    return_page: String,
    return_focus: Option<String>,
}

impl Editor {
    fn display_title(spec: &OttEditor, return_page: &str) -> String {
        match spec.kind {
            OttEditorKind::Setting => ui_text::setting_text(&spec.key, &spec.title).to_owned(),
            OttEditorKind::Search => search_title(&spec.title, favorites_page(return_page)),
        }
    }

    fn cancel_action(&self) -> OttAction {
        match self.spec.kind {
            OttEditorKind::Setting => OttAction::CancelSetting { session: self.spec.session },
            OttEditorKind::Search => OttAction::CancelSearch { session: self.spec.session },
        }
    }
}

/// Shared owned artwork; draw layers borrow it as `widgets::Art`.
pub type OttPoster = plx_data::metadata::MediaArt;

/// Backend facts for a known item. Absence means unknown or non-media; neither
/// the provider name nor artwork is used to guess its media subtype.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OttMediaKind {
    LiveChannel, Movie, Series, Episode, LocalVideo,
    /// A playable video with no preserved live/movie/episode classification.
    Video,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OttMediaProgress { pub position_ms: u64, pub duration_ms: u64 }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OttMedia {
    pub kind: OttMediaKind,
    pub duration_ms: Option<u64>,
    pub progress: Option<OttMediaProgress>,
    pub synopsis: Option<plx_data::ott::SynopsisSpan>,
}

/// Read-only progress. Backends append through the same catalogue snapshot;
/// these facts do not advertise a user-triggered paging command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OttCatalogProgress {
    pub loaded: usize,
    pub total: Option<usize>,
    pub loading: bool,
    pub complete: bool,
}

pub use plx_data::metadata::{OttGuideProgramme, OttNowNext};


#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OttRow {
    /// Compact current M3U catalogue facts; independent of the selected full guide.
    pub current_programme: Option<plx_data::ott::guide::rows::CurrentProgramme>,
    /// Original content title, independent of the numbered row label.
    pub content_title: Option<String>,
    /// Current selected metadata owner only; None does not mean an empty guide.
    pub guide: Option<OttNowNext>,
    /// Explicit backend capability, independent of whether media subtype is known.
    pub guide_available: bool,
    /// Explicit full-retention M3U catch-up capability, not a DVR inventory.
    pub records_available: bool,
    pub media: Option<OttMedia>,
    pub id: String,
    pub title: String,
    pub detail: String,
    pub description: String,
    pub poster: Option<OttPoster>,
    pub action: OttAction,
    pub enabled: bool,
    pub favorite: bool,
}

/// The adapter owns these ephemeral result/file identities. Both the Classic
/// list and physical-number projection count this same sequence, including
/// disabled content rows, while leaving the adapter's controls unnumbered.
pub fn torrent_catalogue_row(page: &str, row: &OttRow) -> bool {
    let OttAction::Play(id) = &row.action else { return false; };
    match page {
        "torrent-results" => row.id.starts_with("torznab:")
            && id.strip_prefix("torrent:") == Some(row.id.as_str()),
        "torrent-files" => row.id.starts_with("torrent.file:") && id == &row.id,
        _ => false,
    }
}

/// Present only when the runtime supports searching this projected list. The
/// runtime owns the committed query and context; the screen owns only a draft.
#[derive(Clone, PartialEq, Eq)]
pub struct OttSearch {
    pub query: String,
    pub scope: String,
}
fn favorites_page(page: &str) -> bool { page.split(':').next() == Some("favorites") }

impl OttSearch {
    /// Shared search caption; the scope is display text, never a source selector.
    pub fn title(&self, favorites: bool) -> String {
        search_title(&self.scope, favorites)
    }
}
fn search_title(scope: &str, favorites: bool) -> String {
    if favorites { msg::ott_search_favorites().into() }
    else if scope.is_empty() { msg::ott_search_this_list().into() }
    else { msg::ott_search_scope_title(scope) }
}
impl std::fmt::Debug for OttSearch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OttSearch").field("query", &"<redacted>").field("scope", &self.scope).finish()
    }
}

/// A real provider category, referencing the one canonical row table. Repeated
/// membership does not duplicate metadata or create another playback identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OttShelf {
    pub id: String,
    pub title: String,
    pub rows: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OttView {
    pub classic_preferences: plx_appkit::classic_settings::Preferences,
    pub volume_available: bool,
    /// Display label from the effective catalogue selection; never inferred from a page token.
    pub category_label: Option<String>,
    /// Opaque source/view owner for a debounced UI Preview; absent means no preview request.
    pub preview_context: Option<String>,
    /// Present only for a current authorized Local list.
    pub order_context: Option<String>,
    /// Opaque source/view fence for an internal random Play action; not a wire receipt.
    pub random_context: Option<String>,
    pub catalog_progress: Option<OttCatalogProgress>,
    /// Changes when navigating; a new page opens at its requested/first item.
    pub page_id: String,
    pub revision: u64,
    pub title: String,
    pub description: String,
    pub status: String,
    /// The status describes pending work; accounted only where its text is drawn.
    pub status_working: bool,
    pub rows: Vec<OttRow>,
    /// Empty preserves the flat catalogue presentation. Runtime supplies these
    /// only for its unfiltered All list; Classic continues to use `rows`.
    pub shelves: Vec<OttShelf>,
    /// Presentation-only source identity for RAM shelf memory. Never authorizes
    /// an action and deliberately survives navigation/action-token rotation.
    pub shelf_context: Option<String>,
    pub selected_row: Option<String>,
    pub search: Option<OttSearch>,
}

impl Default for OttView {
    fn default() -> Self {
        Self { classic_preferences: Default::default(), volume_available: false, category_label: None, order_context: None, preview_context: None, random_context: None, catalog_progress: None, page_id: "loading".into(), revision: 0, title: msg::ott_brand().into(),
            description: msg::ott_intro().into(), status: String::new(), status_working: false,
            rows: Vec::new(), shelves: Vec::new(), shelf_context: None, selected_row: None, search: None }
    }
}

pub struct OttScreen {
    entry: EntryId,
    instance: InstanceId,
    snapshot: Arc<OttView>,
    classic_mode: bool,
    classic_visible: bool,
    // Decorative application observation, like Player clock labels: not canonical state.
    classic_clock: Option<String>,
    classic_layout: std::cell::Cell<Option<plx_ui::classic::Layout>>,
    guide_clock: classic::GuideClock,
    viewport: viewport::ViewportState,
    form: FormTable<String, FormAction, Infallible>,
    keys: HashMap<String, u32>,
    next_key: u32,
    copy: String,
    status: String,
    card_stack: plx_ui::cards::Stack<media::Section>,
    card_presentation: Option<bool>,
    shelves: Vec<media::Shelf>,
    shelf_index: HashMap<u32, usize>,
    card_focus: Option<u32>,
    catalog_keys: Vec<u32>,
    restore_memory: Option<Memory>,
    search_return: search_return::State,
    editor: Option<Editor>,
    editing: bool,
    browse: bool,
    browse_return: Option<String>,
    focus_request: Option<u32>,
    menu_visibility: plx_appkit::menu_visibility::Visibility,
    header_cache: plx_ui::page_header::SharedRow<plx_ui::page_header::HeaderRow>,
    header_capsules: TabStrip,
    header_pop: CtlPop<1>,
    media: Option<usize>,
    media_keys: HashMap<u32, usize>,
    reading: bool,
    reader: DocumentReader,
    read_target: u32,
    preview: Option<String>,
    preview_token: Option<String>,
    previewed: Option<String>,
    preview_dwell: f32,
    number_notice: plx_ui::decision_alert::DecisionAlert,
    number_notice_until: Option<u32>,
    exit: exit_prompt::State,
}

impl OttScreen {
    /// Provider URLs may embed credentials even when displayed as plain text.
    /// The runner must suppress raw text recording for every native editor.
    pub fn handles_private_text(&self) -> bool { self.editor.is_some() }

    /// Private screenshot capture is limited to audited navigation/settings UI.
    /// Pairing codes may be visible in this private diagnostic artifact; credential
    /// editors and other text-bearing pages are never admitted by this predicate.
    pub fn ui_snapshot_allowed(&self) -> bool {
        self.editor.is_none() && !self.editing && !self.reading && !self.browse
            && matches!(self.snapshot.page_id.as_str(), "providers" | "settings")
    }

    /// Numbering applies to the canonical catalogue, never an editor or reader.
    pub fn accepts_channel_number(&self) -> bool {
        self.classic_visible && self.editor.is_none() && !self.reading && !self.browse
            && (matches!(self.snapshot.page_id.split(':').next(), Some("channels" | "favorites"))
                || (self.classic_mode && self.form.table.is_classic_catalogue()
                    && matches!(self.snapshot.page_id.as_str(), "torrent-results" | "torrent-files")))
    }
    /// Only a rendered Classic catalogue uses positions in its current visible list.
    /// Bare-player and Modern number entry retain the canonical channel numbers.
    pub fn channel_number_uses_list_order(&self) -> bool {
        self.accepts_channel_number() && self.classic_mode
            && self.form.table.is_classic_catalogue()
            && self.snapshot.rows.iter().all(|row| matches!(row.action, OttAction::Play(_)))
    }
    fn category_numbered(row: &OttRow) -> bool {
        matches!(row.action, OttAction::SelectCategory(_) | OttAction::SelectCategoryAt { .. }
            | OttAction::ShowFavorites)
    }
    /// The same first nine category rows own both their badge and physical digit.
    /// Form activation retains disabled-row behavior; auxiliary actions never count.
    pub fn category_number_action(&self, number: u8) -> Option<OttAction> {
        if !(1..=9).contains(&number) || !self.classic_visible || !self.classic_mode
            || self.editor.is_some() || self.editing || self.reading || self.browse
            || self.snapshot.page_id.split(':').next() != Some("categories")
            || !self.form.table.is_classic_catalogue()
        { return None; }
        let row = self.snapshot.rows.iter().filter(|row| Self::category_numbered(row))
            .nth(usize::from(number - 1))?;
        let key = *self.keys.get(&format!("item:{}", row.id))?;
        match self.form.index_of_key(RowKey(key)).and_then(|index| self.form.activate(index)) {
            Some(Activation::Action(FormAction::Forward(action))) => Some(action),
            _ => None,
        }
    }
    /// Reject a key for a new Runtime list until that list actually reached this screen.
    pub fn channel_number_context(&self, page: &str, revision: u64) -> bool {
        self.snapshot.page_id == page && self.snapshot.revision == revision
    }
    fn clear_number_notice(&mut self) {
        if self.number_notice.visible() || self.number_notice_until.is_some() {
            self.number_notice.close(); self.number_notice_until = None;
        }
    }
    pub fn set_number_notice(&mut self, value: &Option<(String, String)>, now: u32) {
        let Some((number, title)) = value else { self.clear_number_notice(); return; };
        if !self.accepts_channel_number() { self.clear_number_notice(); return; }
        let body = vec![number.clone().into(), title.clone().into()];
        if self.number_notice.is_open() {
            self.number_notice.reconcile_card(plx_platform::i18n::msg::ott_channel_list_c(), body, plx_ui::decision_alert::Answers::One);
        } else { self.number_notice.open_notice(plx_platform::i18n::msg::ott_channel_list_c(), body); }
        self.number_notice_until = Some(now.wrapping_add(2_000));
        plx_machine::idle::invalidate();
    }

    pub fn new(entry: EntryId, instance: InstanceId) -> Self {
        let mut screen = Self { entry, instance, snapshot: Arc::new(OttView::default()),
            classic_mode: false, classic_visible: false, classic_clock: None, classic_layout: std::cell::Cell::new(None), guide_clock: Default::default(), viewport: Default::default(),
            form: FormTable::new_hashed(crate::registry::BAND), keys: HashMap::new(), next_key: 1,
            copy: String::new(), status: String::new(),
            card_stack: plx_ui::cards::Stack::new(entry).clipped(navigation::route().content),
            card_presentation: None, shelves: Vec::new(), shelf_index: HashMap::new(), card_focus: None, catalog_keys: Vec::new(), restore_memory: None, search_return: Default::default(), editor: None, editing: false, browse: false, browse_return: None,
            menu_visibility: Default::default(), focus_request: None, header_cache: plx_ui::page_header::SharedRow::new(), header_capsules: TabStrip::new(), header_pop: CtlPop::new(), media: None,
            media_keys: HashMap::new(), reading: false, reader: DocumentReader::new(), read_target: 0,
            preview: None, preview_token: None, previewed: None, preview_dwell: 0.0,
            number_notice: plx_ui::decision_alert::DecisionAlert::new(), number_notice_until: None,
            exit: exit_prompt::State::new() };
        screen.install(Arc::new(OttView::default()));
        screen
    }

    fn row_key(&mut self, id: &str) -> RowKey {
        if let Some(key) = self.keys.get(id) { return RowKey(*key); }
        let key = self.next_key;
        self.next_key += 1;
        self.keys.insert(id.to_owned(), key);
        RowKey(key)
    }

    // Explicit content metadata may describe a choice whose action belongs to
    // another adapter. This admits display/reading only, never a Play or preview.
    fn metadata_row(&self, row: &OttRow) -> bool {
        matches!(row.action, OttAction::Play(_) | OttAction::GuideProgramme { .. })
            || (self.classic_mode && row.content_title.as_deref().is_some_and(|title| !title.trim().is_empty()))
    }

    fn install(&mut self, snapshot: Arc<OttView>) {
        self.install_restoring(snapshot, None);
    }

    fn install_restoring(&mut self, snapshot: Arc<OttView>, return_to: Option<(&str, Option<&String>)>) {
        if snapshot.revision < self.snapshot.revision { return; }
        plx_base::diag::spans::span("ott_form", || self.install_restoring_measured(snapshot, return_to));
    }

    fn install_restoring_measured(&mut self, snapshot: Arc<OttView>, return_to: Option<(&str, Option<&String>)>) {
        let changed_source = if self.snapshot.shelf_context.is_some() && !self.classic_mode {
            self.snapshot.shelf_context != snapshot.shelf_context
        } else {
            // Existing flat/Classic source admission stays independent of shelf memory.
            self.snapshot.preview_context.is_some() && self.snapshot.preview_context != snapshot.preview_context
        };
        let restore = self.restore_memory.take().filter(|memory| memory.page_id == snapshot.page_id
            && memory.source.as_ref().is_none_or(|source| snapshot.shelf_context.as_ref() == Some(source))
            && !changed_source)
            .or_else(|| {
                let changed_source = self.snapshot.page_id != snapshot.page_id || changed_source
                    || (self.snapshot.shelf_context.is_none() && self.snapshot.preview_context != snapshot.preview_context);
                let id = snapshot.selected_row.as_ref()?;
                if !changed_source || !snapshot.catalog_progress.is_some_and(|progress| progress.loading && !progress.complete)
                    || snapshot.rows.iter().any(|row| row.id == *id) { return None; }
                // Reuse navigation memory's progressive restore and explicit-input
                // cancellation; the temporary first row is not a new user choice.
                Some(Memory { page_id: snapshot.page_id.clone(), source: (!self.classic_mode).then(|| snapshot.shelf_context.clone()).flatten(), shelves: Vec::new(), focus: Some(format!("item:{id}")),
                    selected: Some(id.clone()), scroll: 0.0, keys: self.keys.clone(), next_key: self.next_key })
            });
        if let Some(memory) = &restore {
            self.keys = memory.keys.clone();
            // Retired shelf identities must never be issued again after a RAM restore.
            self.next_key = self.next_key.max(memory.next_key);
        }
        let awaiting_restore = restore.as_ref().is_some_and(|memory| {
            let missing = |id: &str| !snapshot.rows.iter().any(|row| row.id == id);
            snapshot.catalog_progress.is_some_and(|progress| progress.loading && !progress.complete)
                && (memory.focus.as_deref().and_then(|id| id.strip_prefix("item:")).is_some_and(missing)
                    || memory.selected.as_deref().is_some_and(missing))
        });
        let previous_presentation = self.card_presentation;
        let previous_shelves = !self.shelves.is_empty();
        let same_page = !changed_source && return_to.map_or(self.snapshot.page_id == snapshot.page_id,
            |(page, _)| page == snapshot.page_id);
        if self.snapshot.preview_context != snapshot.preview_context {
            self.preview = None; self.preview_token = None; self.previewed = None; self.preview_dwell = 0.0;
        }
        let prior = self.media.and_then(|i| self.snapshot.rows.get(i)).map(|r| r.id.clone());
        let retired_reader = self.reading && (!same_page || !snapshot.rows.iter().any(|row|
            prior.as_ref() == Some(&row.id) && row.enabled && self.metadata_row(row)
                && !self.description(row).is_empty()));
        let restore_target = restore.as_ref().and_then(|memory| memory.selected.as_ref()).and_then(|id|
            snapshot.rows.iter().position(|row| &row.id == id && row.enabled && self.metadata_row(row)));
        let wanted = restore.as_ref().and_then(|memory| memory.selected.as_ref()).or_else(||
            if same_page { prior.as_ref().or(snapshot.selected_row.as_ref()) } else { snapshot.selected_row.as_ref() });
        let held = if self.browse { self.browse_return.as_ref() }
            else { return_to.and_then(|(_, focus)| focus).or_else(|| self.form.selected_id()) };
        let held_id = held.and_then(|id| id.strip_prefix("item:"));
        let held_row = held_id.and_then(|id| snapshot.rows.iter().position(|row| row.id == id));
        self.media = if same_page && self.browse {
            // Browse's item actions belong to the item with which it opened.
            // A refresh may remove it; it must never retarget Play to a neighbour.
            held_row.filter(|index| snapshot.rows[*index].enabled && self.metadata_row(&snapshot.rows[*index]))
        } else if restore_target.is_some() {
            // Explicit pending restoration outranks the interim first item. A
            // deliberate item selection already cleared restore_memory.
            restore_target
        } else if same_page && held_row.is_some_and(|index|
            !snapshot.rows[index].enabled && self.metadata_row(&snapshot.rows[index])) {
            None
        } else {
            snapshot.rows.iter().position(|r| r.enabled && self.metadata_row(r) && wanted == Some(&r.id))
                .or_else(|| snapshot.rows.iter().position(|r| r.enabled && self.metadata_row(r)))
        };
        if retired_reader {
            // An open description belongs to one enabled item. Returning to Browse
            // makes disappearance explicit instead of opening a neighbour's text.
            self.reading = false; self.reader.reset(); self.read_target = 0;
            self.media = None; self.preview = None; self.preview_token = None; self.preview_dwell = 0.0;
        }
        if !same_page {
            self.browse = false; self.browse_return = None; self.focus_request = None;
            self.reading = false; self.reader.reset(); self.read_target = 0;
            self.preview = None; self.preview_token = None; self.previewed = None; self.preview_dwell = 0.0;
        }
        if !same_page { self.card_focus = None; }
        self.media_keys.clear();
        self.catalog_keys.clear();
        self.card_presentation = media::presentation(&snapshot);
        self.install_shelves(&snapshot);
        let reset_cards = (restore.is_some() && !awaiting_restore) || !same_page || self.card_presentation != previous_presentation || previous_shelves != !self.shelves.is_empty();
        if reset_cards {
            self.card_stack = plx_ui::cards::Stack::new(self.entry).clipped(navigation::route().content);
        }
        self.row_key("nav:browse");
        let visible = plx_appkit::menu_visibility::projected();
        self.menu_visibility = visible;
        let mut navigation = FormSection::new(msg::ott_browse());
        // Allocate this small stable key vocabulary once even when a capability is absent.
        // Availability is checked against the current selected row, never a stale action cache.
        for id in navigation::CONTEXT_IDS { self.row_key(id); }
        let actions = navigation::available_at(&snapshot, self.media, visible,
            self.guide_clock.now_for(self.media.and_then(|index| snapshot.rows.get(index)).and_then(|row| row.guide.as_ref())));
        for item in actions {
            let key = self.row_key(item.id);
            navigation = navigation.item_keyed(item.id.to_owned(), key, RowKind::Button, item.action, item.row);
        }
        let category_page = snapshot.page_id.split(':').next().is_some_and(|page| page == "categories");
        let torrent_catalogue = self.classic_mode && !self.browse && classic::torrent_catalogue(&snapshot);
        let torrent_page = snapshot.page_id == "torrent-results" || snapshot.page_id == "torrent-files";
        let classic_catalogue = self.classic_mode && !self.browse && snapshot.rows.iter().any(|row| row.enabled)
            && (torrent_catalogue || (!torrent_page
            && snapshot.rows.iter().all(|row| match &row.action {
                OttAction::Play(_) | OttAction::SelectCategory(_) | OttAction::SelectCategoryAt { .. } => true,
                OttAction::ShowFavorites => category_page,
                OttAction::EditSetting(key) => category_page && row.id == "organization.open" && key == "organization.open",
                _ => false,
            })));
        let classic_choices = self.classic_mode && !self.browse && !classic_catalogue
            && classic_settings_list::applies(&snapshot);
        // The full-width Classic caption already names either compact list.
        let mut content = FormSection::new(if classic_catalogue || classic_choices { String::new() } else { snapshot.title.clone() });
        let mut torrent_controls = FormSection::new("");
        let mut catalogue_number = 0usize;
        let mut category_number = 0u32;
        for (i, row) in snapshot.rows.iter().enumerate() {
            let id = format!("item:{}", row.id);
            let key = self.row_key(&id);
            let torrent_item = torrent_catalogue && torrent_catalogue_row(&snapshot.page_id, row);
            if !torrent_catalogue || torrent_item { self.catalog_keys.push(key.0); }
            if self.metadata_row(row) { self.media_keys.insert(key.0, i); }
            let catalogue_item = classic_catalogue && matches!(row.action, OttAction::Play(_));
            let mut presentation = Row::new(if catalogue_item {
                classic::list_label(row, snapshot.classic_preferences, self.guide_clock.now_for(row.guide.as_ref()))
            } else { row.title.clone() }).server_label()
                .detail(if classic_catalogue { String::new() } else { row.detail.clone() }).server_detail().checked(row.favorite);
            if row.favorite && matches!(&row.action, OttAction::GuideProgramme { .. }) {
                // Guide rows use the accepted checked state as a timer marker,
                // before their date rather than in the title's checkbox slot.
                presentation = presentation.checked(false).detail_marker(theme::CLASSIC_REMOTE_YELLOW);
            }
            if catalogue_item {
                catalogue_number += 1;
                presentation.set_secondary_label_start(classic::list_secondary_start(row,
                    snapshot.classic_preferences, &presentation.label));
            }
            if classic_choices {
                // Values share the title's row. Explanatory text stays in the
                // selected item's narrative; no binding or draft is replaced.
                presentation.detail.clear();
                if let Some(value) = classic_settings_list::value(row) {
                    presentation = presentation.value(value).server_value();
                }
            }
            if catalogue_item && snapshot.classic_preferences.show_progress() {
                if let Some((start, end)) = classic::list_progress_interval(row, self.guide_clock.now_for(row.guide.as_ref())) {
                    presentation = presentation.progress_interval(start, end);
                }
            }
            if catalogue_item && snapshot.classic_preferences.show_number() {
                presentation = presentation.catalogue_number(catalogue_number.to_string());
            }
            if classic_catalogue && category_page && Self::category_numbered(row) {
                category_number += 1;
                if category_number <= 9 {
                    presentation = presentation.catalogue_number(category_number.to_string());
                }
            }
            if torrent_catalogue && !torrent_item {
                torrent_controls = torrent_controls.item_keyed(id, key, RowKind::Button,
                    FormAction::Forward(row.action.clone()), presentation).disabled(!row.enabled);
            } else {
                content = content.item_keyed(id, key, RowKind::Button, FormAction::Forward(row.action.clone()), presentation)
                    .disabled(!row.enabled);
            }
        }
        // These owner-built pages edit static settings; Refresh would reload
        // the active catalog rather than refresh a choice on this page.
        let static_settings = matches!(snapshot.page_id.as_str(),
            "settings" | "interface-mode" | "osd-opacity" | "permanent-clock" | "classic-page-size" | "classic-list-position" | "classic-list-arrows" | "numeric-seek-step" | "classic-show-number" | "classic-show-name" | "classic-show-program" | "classic-show-progress" | "classic-show-description" | "menu-items" | "torrent-settings" | "torrent-configure" | "sleep-timer" | "m3u-media-links")
            || snapshot.page_id.strip_prefix("profile-setup:").is_some()
            || snapshot.page_id.starts_with("vportal-quality:");
        let owner_back = !self.browse && snapshot.rows.iter().any(|row| row.enabled && match
            (snapshot.page_id.as_str(), row.id.as_str(), &row.action) {
                ("sleep-timer", "sleep:back", OttAction::ShowSettings)
                | ("interface-mode", "interface:back", OttAction::ShowSettings)
                | ("osd-opacity" | "permanent-clock" | "classic-page-size" | "classic-list-position" | "classic-list-arrows" | "numeric-seek-step" | "classic-show-number" | "classic-show-name" | "classic-show-program" | "classic-show-progress" | "classic-show-description", "osd:back", OttAction::ShowSettings)
                | ("menu-items", "menu:back", OttAction::ShowSettings) => true,
                ("settings", id, OttAction::EditSetting(key)) if id == key => id.strip_prefix("settings.draft:")
                    .and_then(|rest| rest.split_once(':')).is_some_and(|(nonce, command)| command == "back"
                        && nonce.parse::<u64>().ok().is_some_and(|value| value > 0 && value.to_string() == nonce)),
                ("m3u-media-links", "m3u-media.back", OttAction::ShowProfiles) => true,
                ("torrent-settings" | "torrent-configure", "torrent.back", OttAction::EditSetting(key)) => key == "torrent.back",
                _ => false,
            });
        let mut footer = FormSection::new("");
        if !static_settings && visible.navigation("nav:refresh") {
            footer = footer.item_keyed("nav:refresh".to_owned(), self.row_key("nav:refresh"), RowKind::Button,
                FormAction::Forward(OttAction::Refresh), Row::new(msg::browse_home_refresh()));
        }
        if !owner_back {
            let key = self.row_key("nav:back");
            let action = if self.browse { FormAction::CloseBrowse } else { FormAction::Forward(OttAction::Back) };
            if classic_choices && classic_settings_list::root(&snapshot) {
                // Root Settings exposes this same Back action only in its Close footer.
            } else if classic_choices {
                content = content.item_keyed("nav:back".to_owned(), key, RowKind::Button,
                    action, Row::new(msg::settings_back()));
            } else {
                footer = footer.item_keyed("nav:back".to_owned(), key, RowKind::Button,
                    action, Row::new(msg::settings_back()));
            }
        }
        let form = if self.browse || snapshot.rows.is_empty() { Form::new().section(navigation).section(footer) }
            else if torrent_catalogue { Form::new().section(content).section(torrent_controls) }
            else if classic_catalogue || classic_choices { Form::new().section(content) }
            else { Form::new().section(content).section(footer) };
        let page_action = navigation::page_action(&snapshot.page_id);
        let selected = if self.browse { page_action.to_owned() } else { snapshot.rows.iter()
            .find(|row| row.enabled && snapshot.selected_row.as_ref() == Some(&row.id))
            .or_else(|| snapshot.rows.iter().find(|row| row.enabled))
            .map(|row| format!("item:{}", row.id))
            .unwrap_or_else(|| if snapshot.search.as_ref().is_some_and(|search| !search.query.is_empty()) {
                "nav:search".to_owned()
            } else { match snapshot.page_id.split(':').next().unwrap_or("") {
                "profiles" | "usb-folders" => "nav:profiles",
                "channels" => "nav:catalog",
                "categories" => "nav:categories",
                "favorites" => "nav:categories",
                "settings" => "nav:settings",
                _ => "nav:providers",
            }.to_owned() }) };
        // Header focus and selected media are independent. Keep the body selection
        // available beneath that header instead of substituting its first row.
        let restored_body = restore.as_ref().filter(|memory| memory.focus.as_deref().is_some_and(|id|
            id == "nav:browse" || id == "nav:search" || id == "nav:favorite" || id == "nav:description" || (self.classic_mode && navigation::is_context_control(id))))
            .and_then(|_| self.media.and_then(|index| snapshot.rows.get(index)))
            .map(|row| format!("item:{}", row.id));
        plx_base::diag::spans::span("ott_table", || {
            if let Some(memory) = &restore { self.form.open(form, restored_body.as_ref().or(memory.focus.as_ref()).or(Some(&selected))); }
            else if same_page {
                if let Some((_, focus)) = return_to { self.form.refresh_with(form, focus, Some(&selected)); }
                else { self.form.refresh_with(form, None, Some(&selected)); }
            }
            else { self.form.open(form, Some(&selected)); }
        });
        if self.card_presentation.is_some() && reset_cards && !self.browse {
            self.focus_request = self.form.selected_key().map(|key| key.0);
        }
        if let Some(memory) = &restore {
            if self.card_presentation.is_some() && !awaiting_restore {
                self.card_stack.restore(&plx_ui::cards::StackMemory { scroll: memory.scroll, shelves: memory.shelves.clone() });
            }
            self.focus_request = memory.focus.as_ref().filter(|id|
                (id.starts_with("nav:") && visible.navigation(id)) || id.starts_with("shelf-item:"))
                .and_then(|id| self.keys.get(id)).copied()
                .or_else(|| self.form.selected_key().map(|key| key.0));
            if awaiting_restore {
                let mut pending = memory.clone();
                pending.keys = self.keys.clone(); pending.next_key = self.next_key;
                self.restore_memory = Some(pending);
            }
        }
        if retired_reader { self.focus_request = self.keys.get("nav:browse").copied(); }
        self.copy = snapshot.description.clone();
        self.status = media::status(&snapshot);
        if !self.status.is_empty() {
            if !self.copy.is_empty() { self.copy.push_str("\n\n"); }
            self.copy.push_str(&self.status);
        }
        self.snapshot = snapshot;
        if !self.classic_mode && !self.browse {
            self.focus_request = self.focus_request.map(|key| self.card_target(key));
        }
        self.form.table.set_classic_catalogue(classic_catalogue);
        self.form.table.set_classic_choices(classic_choices);
        if let Some(draft) = self.classic_settings_draft_footer() {
            let ids: Vec<_> = std::iter::once(draft.back).chain(draft.save)
                .map(classic_settings_list::footer_id).collect();
            for id in ids { self.row_key(&id); }
        }
    }

    fn selected_snapshot_row(&self) -> Option<&OttRow> {
        let id = self.form.selected_id()?.strip_prefix("item:")?;
        self.snapshot.rows.iter().find(|row| row.id == id)
    }

    fn status_working(&self) -> bool {
        self.snapshot.status_working || self.snapshot.catalog_progress
            .is_some_and(|progress| progress.loading && !progress.complete)
    }

    fn grid_active(&self) -> bool {
        !self.classic_mode && self.card_presentation.is_some() && !self.browse && self.editor.is_none() && !self.reading
    }

    fn grid_view(&self) -> media::View<'_> {
        media::View { stack: &self.card_stack, page: media::Page {
            presentation: self.card_presentation, snapshot: &self.snapshot,
            keys: &self.catalog_keys, index: &self.media_keys, shelves: &self.shelves, shelf_index: &self.shelf_index,
        } }
    }

    fn grid_event<H: AppLike<Memory = PageMemory>>(&mut self, ev: &ScreenEvent<H>, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>)
        -> Option<plx_ui::cards::CardEvent<u32>> {
        if self.classic_mode || self.editor.is_some() { return None; }
        let page = media::Page { presentation: self.card_presentation, snapshot: &self.snapshot,
            keys: &self.catalog_keys, index: &self.media_keys, shelves: &self.shelves, shelf_index: &self.shelf_index };
        match self.card_stack.on(&page, ev, cx, fx) {
            Some(plx_ui::cards::StackEvent::Card(_, card)) if self.grid_active() => Some(card),
            _ => None,
        }
    }

    fn refresh_presentation(&mut self, mode: plx_machine::machine::UiMode, measure: &dyn plx_machine::machine::Measure) {
        let classic = mode == plx_machine::machine::UiMode::Classic;
        let changed = self.classic_mode != classic;
        self.classic_mode = classic;
        if changed { self.search_return = Default::default(); }
        self.form.table.set_classic(classic);
        self.classic_layout.set(Some(plx_ui::classic::Layout::new(Rect::FULL, measure,
            self.editor.is_none() && !self.reading).with_list_left(self.snapshot.classic_preferences.list_left())));
        if changed && self.editor.is_none() { self.install(self.snapshot.clone()); }
        self.form.table.set_classic(classic);
        self.form.table.set_classic_page_size(
            (classic && self.form.table.is_classic_list()).then_some(self.snapshot.classic_preferences.page_size()),
            self.table_frame().h);
        self.form.table.fit_layout(measure);
    }

    fn header_scroll(&self) -> f32 {
        if self.classic_mode { self.classic_layout.get().map_or(0.0, |layout| -layout.header_offset().1) } else { 0.0 }
    }

    fn document_frame(&self) -> Rect {
        if self.classic_mode { self.classic_layout.get().map_or(Rect::FULL, |layout| layout.body) }
        else { RouteLayout::screen().document(true) }
    }

    fn table_frame(&self) -> Rect {
        if self.classic_mode { self.route().content } else { self.route().sectioned_table() }
    }

    fn route(&self) -> RouteLayout {
        if self.classic_mode { if let Some(layout) = self.classic_layout.get() { return layout.route(); } }
        if self.editor.is_some() || self.reading { RouteLayout::screen() } else { navigation::route() }
    }

    fn classic_settings_root(&self) -> bool {
        self.classic_mode && !self.browse && self.editor.is_none() && !self.reading
            && self.form.table.is_classic_choices() && classic_settings_list::root(&self.snapshot)
    }

    fn classic_settings_draft_footer(&self) -> Option<classic_settings_list::DraftFooter<'_>> {
        if !self.classic_mode || self.browse || self.editor.is_some() || self.reading
            || !self.form.table.is_classic_choices() { return None; }
        let save = self.settings_save_action();
        classic_settings_list::draft_footer(&self.snapshot, save.as_ref())
    }

    fn header(&self, measure: &dyn plx_machine::machine::Measure) -> Option<std::rc::Rc<plx_ui::page_header::HeaderRow>> {
        if self.editor.is_some() || self.reading { return None; }
        let browse = self.keys["nav:browse"];
        if self.classic_mode {
            if self.classic_settings_root() {
                let back = self.keys["nav:back"];
                let size = self.classic_layout.get().map_or(theme::size::BODY, |layout|
                    plx_ui::classic::chrome_text_size(layout.list.h, self.snapshot.classic_preferences.page_size()));
                let mut key = std::collections::hash_map::DefaultHasher::new();
                ("classic-settings-close", back, size, measure.font_scale()).hash(&mut key);
                msg::settings_login_close_c().to_bytes().hash(&mut key);
                measure.width(msg::settings_login_close_c(), size, false).to_bits().hash(&mut key);
                return Some(self.header_cache.get(key.finish(), ||
                    classic_settings_list::close_footer(measure, Rect::FULL.w, back, size)));
            }
            if let Some(draft) = self.classic_settings_draft_footer() {
                let back = (self.keys[&classic_settings_list::footer_id(draft.back)], draft.back.title.as_str());
                let save = draft.save.map(|row| (self.keys[&classic_settings_list::footer_id(row)], row.title.as_str()));
                let size = self.classic_layout.get().map_or(theme::size::BODY, |layout|
                    plx_ui::classic::chrome_text_size(layout.list.h, self.snapshot.classic_preferences.page_size()));
                let mut key = std::collections::hash_map::DefaultHasher::new();
                ("classic-settings-draft", back, save, size, measure.font_scale()).hash(&mut key);
                for (_, label) in std::iter::once(back).chain(save) {
                    measure.width_str(label, size, false).to_bits().hash(&mut key);
                }
                return Some(self.header_cache.get(key.finish(), ||
                    classic_settings_list::draft_footer_row(measure, Rect::FULL.w, back, save, size)));
            }
            let actions = if self.browse || self.snapshot.rows.is_empty() { Vec::new() }
                else { navigation::available_at(&self.snapshot, self.media, self.menu_visibility,
                    self.guide_clock.now_for(self.media.and_then(|index| self.snapshot.rows.get(index)).and_then(|row| row.guide.as_ref()))) };
            let controls = navigation::classic_controls(&self.snapshot, &actions, &self.keys);
            let shortcuts = navigation::classic_footer_keys(&self.snapshot, &controls, &self.keys);
            let text_size = self.classic_layout.get().map_or(theme::size::BODY, |layout|
                plx_ui::classic::chrome_text_size(layout.list.h, self.snapshot.classic_preferences.page_size()));
            let mut key = std::collections::hash_map::DefaultHasher::new();
            (true, browse, self.browse, text_size, measure.font_scale(), &self.snapshot.page_id).hash(&mut key);
            for (key_id, label) in &controls {
                key_id.hash(&mut key); label.hash(&mut key);
                measure.width_str(label, theme::size::BODY, false).to_bits().hash(&mut key);
            }
            for (key_id, hints) in &shortcuts {
                key_id.hash(&mut key);
                for hint in hints { hint.text.hash(&mut key); (hint.tone as u8).hash(&mut key); }
            }
            msg::ott_browse_c().to_bytes().hash(&mut key);
            measure.width(msg::ott_browse_c(), theme::size::BODY, false).to_bits().hash(&mut key);
            return Some(self.header_cache.get(key.finish(), ||
                navigation::context_header_classic(measure, Rect::FULL.w, browse, &controls, self.browse, &shortcuts, text_size)));
        }
        // Empty catalogues keep the full recovery form. Its Search row must not
        // also appear under the same focus identity in the header group.
        let content = !self.snapshot.rows.is_empty();
        let search = (content && self.menu_visibility.navigation("nav:search") && self.snapshot.search.is_some()).then(|| self.keys.get("nav:search").copied()).flatten();
        let favorite = (content && !self.browse).then(|| navigation::available_at(&self.snapshot,
            self.media, self.menu_visibility, self.guide_clock.now_for(self.media
                .and_then(|index| self.snapshot.rows.get(index)).and_then(|row| row.guide.as_ref())))
            .into_iter().find(|item| item.id == "nav:favorite")
            .map(|item| (self.keys["nav:favorite"], item.row.label))).flatten();
        let description = (content && self.menu_visibility.navigation("nav:description") && self.has_description()).then(|| self.keys.get("nav:description").copied()).flatten();
        let caption = if self.grid_active() { navigation::caption(&self.snapshot) } else { String::new() };
        let mut key = std::collections::hash_map::DefaultHasher::new();
        (browse, search, &favorite, description, self.browse, &caption).hash(&mut key);
        measure.width_str(&caption, theme::size::BODY, false).to_bits().hash(&mut key);
        if let Some((_, label)) = &favorite {
            measure.width_str(label, theme::size::BODY, false).to_bits().hash(&mut key);
        }
        for label in [msg::ott_browse_c(), msg::ott_search_c(), msg::ott_description_c()] {
            label.to_bytes().hash(&mut key);
            measure.width(label, theme::size::BODY, false).to_bits().hash(&mut key);
        }
        Some(self.header_cache.get(key.finish(), ||
            navigation::header(measure, Rect::FULL.w, browse, search,
                favorite.as_ref().map(|(key, label)| (*key, label.as_str())), description, self.browse, &caption)))
    }

    /// Update only the row whose guide clock changed (label and progress interval); never rebuild or scan
    /// the whole catalogue each second, and do not alter focus/action identity.
    fn refresh_classic_programme(&mut self) {
        if !self.form.table.is_classic_catalogue() { return; }
        let Some(index) = self.media else { return; };
        let Some(row) = self.snapshot.rows.get(index) else { return; };
        if !matches!(row.action, OttAction::Play(_)) { return; }
        let now = self.guide_clock.now_for(row.guide.as_ref());
        let label = classic::list_label(row, self.snapshot.classic_preferences, now);
        let secondary = classic::list_secondary_start(row, self.snapshot.classic_preferences, &label);
        let progress = self.snapshot.classic_preferences.show_progress()
            .then(|| classic::list_progress_interval(row, now)).flatten();
        let Some(key) = self.keys.get(&format!("item:{}", row.id)) else { return; };
        // A Classic catalogue contains exactly its content rows. Confirm that
        // invariant with an O(1) binding lookup; never search a large catalogue
        // for the selected key on every clock tick.
        if self.form.key_at(index) != Some(RowKey(*key)) { return; }
        let mut offset = index;
        let changed = self.form.table.sections.iter().find_map(|section| {
            if offset < section.rows.len() { Some(section.rows[offset].label != label || section.rows[offset].progress_bounds() != progress || section.rows[offset].secondary_label_start() != secondary) }
            else { offset -= section.rows.len(); None }
        }).unwrap_or(false);
        if changed {
            if let Ok(index) = i32::try_from(index) {
                if let Some(row) = self.form.table.row_mut(index) {
                    row.label = label;
                    row.set_secondary_label_start(secondary);
                    row.set_progress_interval(progress);
                }
            }
        }
    }

    fn open_browse<H: AppLike>(&mut self, fx: &mut Effects<'_, H>) {
        if self.browse { self.close_browse(fx); return; }
        self.browse_return = self.form.selected_id().cloned();
        self.browse = true;
        self.preview = None; self.preview_token = None; self.preview_dwell = 0.0;
        self.install(self.snapshot.clone());
        self.focus_request = self.form.selected_key().map(|key| self.card_target(key.0));
        fx.invalidate(plx_machine::present::Provenance::Input);
    }

    fn close_browse<H: AppLike>(&mut self, fx: &mut Effects<'_, H>) {
        self.browse = false;
        let return_focus = self.browse_return.take();
        let page = self.snapshot.page_id.clone();
        self.install_restoring(self.snapshot.clone(), Some((&page, return_focus.as_ref())));
        self.focus_request = self.form.selected_key().map(|key| self.card_target(key.0));
        fx.invalidate(plx_machine::present::Provenance::Input);
    }

    fn editor_form(&mut self, opening: bool) {
        self.form.table.set_classic_catalogue(false);
        self.form.table.set_classic_choices(false);
        let Some(editor) = &self.editor else { return; };
        let pending = editor.pending;
        let search = editor.spec.kind == OttEditorKind::Search;
        let value = if editor.spec.secret { "•".repeat(editor.draft.text().chars().count().min(24)) }
            else { editor.draft.text().to_owned() };
        // The draft keeps its opening list even if a newer snapshot arrives behind it.
        let empty_hint = if !search { msg::ott_value_empty() }
            else if favorites_page(&editor.return_page) { msg::ott_search_empty_favorites() }
            else { msg::ott_search_empty() };
        let value_row = if value.is_empty() { Row::new(empty_hint) }
            else { Row::new(value).server_label() };
        let section = FormSection::new(if search { msg::ott_search() } else { msg::ott_value() })
            .item_keyed("edit:value".to_owned(), self.row_key("edit:value"), RowKind::Button,
                FormAction::Edit, value_row).disabled(pending)
            .item_keyed("edit:save".to_owned(), self.row_key("edit:save"), RowKind::Button, FormAction::Save, Row::new(if search { msg::ott_search() } else { msg::ott_save() })).disabled(pending)
            .item_keyed("edit:cancel".to_owned(), self.row_key("edit:cancel"), RowKind::Button, FormAction::Cancel, Row::new(msg::settings_cancel()));
        if opening { self.form.open(Form::new().section(section), Some(&"edit:value".to_owned())); }
        else { self.form.refresh(Form::new().section(section)); }
    }

    fn keyboard<H: AppLike>(&mut self, up: bool, fx: &mut Effects<'_, H>) {
        if self.editing == up { return; }
        self.editing = up;
        fx.push(Fx::Deliver(MachineId::Instance(self.instance), Delivery::Keyboard { up }));
        fx.invalidate(plx_machine::present::Provenance::Input);
    }

    fn close_editor<H: AppLike>(&mut self, fx: &mut Effects<'_, H>) {
        self.keyboard(false, fx);
        self.search_return.draft = None;
        if let Some(editor) = self.editor.take() {
            self.install_restoring(self.snapshot.clone(), Some((&editor.return_page, editor.return_focus.as_ref())));
            if editor.return_page == self.snapshot.page_id {
                if let Some(id) = editor.return_focus.filter(|id| self.menu_visibility.navigation(id) && matches!(id.as_str(), "nav:browse" | "nav:search" | "nav:description")) {
                    self.focus_request = self.keys.get(&id).copied();
                }
            }
        }
        fx.invalidate(plx_machine::present::Provenance::Input);
    }

    fn cancel_editor<H: AppLike>(&mut self, fx: &mut Effects<'_, H>) {
        if let Some(editor) = self.editor.as_ref().filter(|editor| editor.spec.session != 0) {
            self.send(editor.cancel_action(), fx);
        }
        self.close_editor(fx);
    }

    fn edit<H: AppLike>(&mut self, edit: &TextEdit, fx: &mut Effects<'_, H>) {
        let Some(editor) = &mut self.editor else { return; };
        if editor.pending { return; }
        let mut next = editor.draft.clone();
        match edit {
            TextEdit::Commit(text) if editor.spec.text_policy == OttTextPolicy::Literal => next.insert(text),
            _ => next.edit(edit),
        }
        let cap = if editor.spec.kind == OttEditorKind::Setting && editor.spec.key == "profile.media" {
            16384
        } else { 8192 };
        // A restored value can exceed this editor's insertion limit. Cursor
        // movement and deletion cannot grow it, and must remain available so
        // the user can repair the value without clearing the whole field.
        if matches!(edit, TextEdit::Backspace | TextEdit::Clear | TextEdit::Left | TextEdit::Right)
            || next.text().chars().count() <= editor.spec.limit.min(cap) {
            editor.draft = next;
            self.editor_form(false);
            fx.invalidate(plx_machine::present::Provenance::Input);
        }
    }

    /// Only app-owned labels constrain the editor's action column. A populated
    /// value is user text: the visible row elides it while the full draft stays
    /// intact. Derive labels from the actual form and reuse its table metrics.
    fn editor_control_width(&self, measure: &dyn plx_machine::machine::Measure) -> f32 {
        let mut controls = self.form.table.blank_like();
        controls.sections = self.form.table.sections.iter().map(|section| {
            let mut control_section = plx_ui::table::Section::new(section.header.clone());
            control_section.rows = section.rows.iter()
                .filter(|row| row.label_origin == plx_ui::table::Origin::App)
                .map(|row| Row::new(row.label.clone())).collect();
            control_section
        }).collect();
        controls.measured_width(measure)
    }

    fn editor_copy(&self) -> &str {
        self.editor.as_ref().map_or(self.copy.as_str(), |editor| {
            if editor.pending {
                if editor.spec.kind == OttEditorKind::Search { msg::ott_search_pending() } else { msg::settings_playback_saving() }
            } else if !editor.error.is_empty() { ui_text::setting_text(&editor.spec.key, &editor.error) }
            else if editor.spec.kind == OttEditorKind::Search {
                if favorites_page(&editor.return_page) { msg::ott_search_help_favorites() }
                else { msg::ott_search_help() }
            }
            else { msg::ott_editor_help() }
        })
    }

    fn view(&self, measure: &dyn plx_machine::machine::Measure) -> TableScreen<'_> {
        let title = self.editor.as_ref().map_or(self.snapshot.title.as_str(), |editor| editor.title.as_str());
        let copy = self.editor_copy();
        let (title, copy) = if self.editor.is_none() && self.browse { (msg::ott_browse(), msg::ott_intro()) } else { (title, copy) };
        let (title, copy) = if self.editor.is_none() && !self.browse && self.media.is_some() { ("", "") } else { (title, copy) };
        let crumb = if self.editor.is_none() && !self.browse && self.media.is_some() { None } else { Some(msg::ott_brand()) };
        let layout = if self.classic_mode { self.route() } else if self.editor.is_some() {
            // Preserve the selected type scale and the complete editor help.
            // Reclaim spare form width before letting a long heading consume
            // the narrative column's height. Focus and paint share this layout.
            RouteLayout::screen_for_reading(0.0, self.editor_control_width(measure), |layout| {
                let body = plx_ui::text_view::TextView::new(copy, theme::size::LABEL, plx_ui::palette::color(plx_ui::palette::Role::Text, theme::TEXT_READING))
                    .leading(theme::size::LABEL as f32 + theme::space::XS)
                    .max_lines(12).with_measure(measure);
                let room = layout.narrative_copy_frame(crumb.is_some(), title, layout.action.y, measure);
                !body.truncates(room.w)
                    && room.y + body.measure_h(room.w) <= layout.action.y - theme::space::XL
            })
        } else { self.route() };
        let (crumb, title, copy) = if self.classic_mode { (None, "", "") } else { (crumb, title, copy) };
        let view = TableScreen::new(Header::new(layout, crumb,
            title, copy), &self.form.table, GroupId(0), self.entry).keyed(&self.form);
        if self.classic_mode { view.with_frame(layout.content) } else { view }
    }

    fn document_focus(&self) -> DocumentFocus<'_> {
        DocumentFocus { reader: &self.reader, frame: self.document_frame(), group: GroupId(0), entry: self.entry }
    }

    fn description<'a>(&self, row: &'a OttRow) -> &'a str {
        metadata::description(row, self.guide_clock.now_for(row.guide.as_ref()))
    }

    fn has_description(&self) -> bool {
        self.media.and_then(|i| self.snapshot.rows.get(i)).is_some_and(|row| !self.description(row).is_empty())
    }

    fn show_details<H: AppLike>(&mut self, fx: &mut Effects<'_, H>) {
        if self.has_description() {
            self.reading = true; self.reader.reset(); self.read_target = 0;
            fx.invalidate(plx_machine::present::Provenance::Input);
        }
    }

    fn send<H: AppLike>(&self, action: OttAction, fx: &mut Effects<'_, H>) {
        fx.push(Fx::App(AppFx::Ott(OttOut::Action { reply: self.instance, action })));
    }

    fn preview_focus(&mut self, key: u32) {
        let id = self.media_keys.get(&key).and_then(|index| self.snapshot.rows.get(*index))
            .filter(|row| row.enabled)
            .and_then(|row| match &row.action { OttAction::Play(id) => Some(id), _ => None });
        let id = id.filter(|_| self.snapshot.preview_context.is_some());
        if id == self.preview.as_ref() { return; }
        self.preview = id.filter(|id| Some(*id) != self.previewed.as_ref()).cloned();
        self.preview_token = self.preview.as_ref().and(self.snapshot.preview_context.as_ref()).cloned();
        self.preview_dwell = 0.0;
    }

    fn preview_tick<H: AppLike>(&mut self, dt: f32, fx: &mut Effects<'_, H>) {
        if self.preview.is_none() || self.editor.is_some() { return; }
        plx_ui::dwell::accumulate(&mut self.preview_dwell, dt, &mut |event| fx.note(event));
        if self.preview_dwell >= 0.2 {
            let context = self.preview_token.take().filter(|context|
                self.snapshot.preview_context.as_ref() == Some(context));
            if let (Some(context), Some(id)) = (context, self.preview.take().filter(|id| self.snapshot.rows.iter().any(|row|
                row.enabled && matches!(&row.action, OttAction::Play(current) if current == id)))) {
                self.previewed = Some(id.clone());
                self.send(OttAction::PreviewAt { context, id }, fx);
            }
            self.preview_dwell = 0.0;
        }
    }

    /// Activate a currently admitted Browse or Classic action-bar item.
    pub fn activate_context<H: AppLike>(&mut self, id: &str, fx: &mut Effects<'_, H>) -> bool {
        self.activate_context_with_visibility(id, self.menu_visibility, fx)
    }

    /// Menu visibility is presentation-only. Physical shortcuts resolve the same
    /// current capabilities and selection without requiring a visible menu item.
    pub(super) fn activate_shortcut<H: AppLike>(&mut self, id: &str, fx: &mut Effects<'_, H>) -> bool {
        self.activate_context_with_visibility(id, Default::default(), fx)
    }

    fn activate_context_with_visibility<H: AppLike>(&mut self, id: &str,
        visible: plx_appkit::menu_visibility::Visibility, fx: &mut Effects<'_, H>) -> bool {
        if self.editor.is_some() || self.reading { return false; }
        let actions = navigation::available_at(&self.snapshot, self.media, visible,
            self.guide_clock.now_for(self.media.and_then(|index| self.snapshot.rows.get(index)).and_then(|row| row.guide.as_ref())));
        let Some(item) = actions.into_iter().find(|item| item.id == id) else { return false; };
        self.restore_memory = None;
        match item.action {
            FormAction::Forward(action) => {
                if self.browse && !matches!(action, OttAction::BeginSearch | OttAction::Refresh) { self.close_browse(fx); }
                self.send(action, fx);
            }
            FormAction::Details => {
                if self.browse { self.close_browse(fx); }
                self.show_details(fx);
            }
            _ => return false,
        }
        true
    }

    fn activate<H: AppLike>(&mut self, key: u32, favorite: bool, fx: &mut Effects<'_, H>) {
        self.restore_memory = None;
        if !favorite && self.classic_settings_root() && self.keys.get("nav:back") == Some(&key) {
            self.send(OttAction::Back, fx);
            return;
        }
        if !favorite {
            if let Some(draft) = self.classic_settings_draft_footer() {
                if let Some(row) = std::iter::once(draft.back).chain(draft.save)
                    .find(|row| self.keys.get(&classic_settings_list::footer_id(row)) == Some(&key)) {
                    let action = row.action.clone();
                    self.send(action, fx);
                    return;
                }
            }
        }
        if self.editor.is_none() && !self.reading && !favorite {
            if self.keys.get("nav:browse") == Some(&key) { self.open_browse(fx); return; }
            if !self.classic_mode && !self.browse {
                if let Some(id) = ["nav:search", "nav:favorite", "nav:description"].into_iter()
                    .find(|id| self.keys.get(*id) == Some(&key)) {
                    self.activate_context(id, fx);
                    return;
                }
            }
        }
        if self.classic_mode && !self.browse && !favorite {
            if let Some(id) = navigation::CONTEXT_IDS.into_iter().find(|id|
                navigation::is_context_control(id) && self.keys.get(*id) == Some(&key)) {
                self.activate_context(id, fx);
                return;
            }
        }
        let key = self.canonical_card_key(key);
        if let Some(Activation::Action(action)) = self.form.index_of_key(RowKey(key)).and_then(|i| self.form.activate(i)) {
            if self.classic_mode && favorite && !self.browse && matches!(&action, FormAction::Forward(OttAction::Play(_))) {
                self.open_browse(fx);
                return;
            }
            match action {
                FormAction::Forward(OttAction::Play(id)) if favorite => {
                    if navigation::favorite_available(&self.snapshot.page_id, &id) {
                        self.send(OttAction::ToggleFavorite(id), fx);
                    }
                },
                FormAction::Forward(OttAction::Back) if !favorite && !self.browse
                    && self.editor.is_none() && !self.reading => {
                    if !self.back_from_search(fx) { self.send(OttAction::Back, fx); }
                },
                FormAction::Forward(action) if !favorite => {
                    if self.browse && !matches!(action, OttAction::BeginSearch | OttAction::Refresh) { self.close_browse(fx); }
                    self.send(action, fx);
                },
                FormAction::Edit if !favorite => self.keyboard(true, fx),
                FormAction::Save if !favorite => {
                    let Some(editor) = &mut self.editor else { return; };
                    if editor.pending { return; }
                    let action = match editor.spec.kind {
                        OttEditorKind::Setting => OttAction::SubmitSetting { session: editor.spec.session,
                            key: editor.spec.key.clone(), value: editor.draft.text().to_owned() },
                        OttEditorKind::Search => OttAction::SubmitSearch { session: editor.spec.session,
                            value: editor.draft.text().to_owned() },
                    };
                    if editor.spec.session == 0 && editor.spec.kind == OttEditorKind::Setting {
                        self.send(action, fx);
                        self.close_editor(fx);
                        return;
                    }
                    editor.pending = true;
                    editor.error.clear();
                    self.keyboard(false, fx);
                    self.editor_form(false);
                    self.send(action, fx);
                    fx.invalidate(plx_machine::present::Provenance::Input);
                }
                FormAction::Cancel if !favorite => self.cancel_editor(fx),
                FormAction::Details if !favorite => {
                    if self.browse { self.close_browse(fx); }
                    self.show_details(fx);
                },
                FormAction::CloseBrowse if !favorite => self.close_browse(fx),
                _ => {}
            }
        }
    }
}

impl<H: AppLike<Memory = PageMemory>> Machine<H> for OttScreen {
    type Ev = ScreenEvent<H>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Handled {
        // Bypass all form, guide, focus and viewport machinery for this paint-only delta.
        if let ScreenEvent::App(AppMsg::OttClock(sample)) = ev {
            let time_changed = self.form.table.set_classic_clock(sample.utc_s);
            if self.classic_clock != sample.label || time_changed {
                self.classic_clock = sample.label.clone();
                if self.classic_visible && H::ui_mode(&cx.views) == plx_machine::machine::UiMode::Classic {
                    fx.invalidate(plx_machine::present::Provenance::Resource(plx_machine::present::ResourceKind::Text));
                }
            }
            return Handled::Yes;
        }
        let handled = self.step_inner(ev, cx, fx);
        // The prompt owns focus and presentation, but video-source and placement
        // acknowledgements must still settle their existing addressed lease.
        if !self.exit.prompt.visible() {
            self.refresh_presentation(H::ui_mode(&cx.views), cx.measure);
            self.refresh_viewport(cx.measure);
        }
        let viewport = self.viewport_event(ev, fx);
        self.sync_viewport(fx);
        if viewport { Handled::Yes } else { handled }
    }
}
impl OttScreen {
    fn step_inner<H: AppLike<Memory = PageMemory>>(&mut self, ev: &ScreenEvent<H>, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Handled {
        if let Some(handled) = self.exit_event(ev, cx, fx) { return handled; }
        self.refresh_presentation(H::ui_mode(&cx.views), cx.measure);
        if !self.accepts_channel_number() { self.clear_number_notice(); }
        let guide_changed = self.guide_clock.update(self.media.and_then(|i| self.snapshot.rows.get(i)).and_then(|row| row.guide.as_ref()), cx.tick.ms);
        if guide_changed && self.browse && self.editor.is_none() {
            let listed = self.form.index_of_key(RowKey(self.keys["nav:description"])).is_some();
            let wanted = self.menu_visibility.navigation("nav:description") && self.has_description();
            if listed != wanted {
                // Only an availability boundary changes the menu. Keep the
                // captured item and let the existing form retire a lost row.
                self.install(self.snapshot.clone());
                fx.invalidate(plx_machine::present::Provenance::Resource(plx_machine::present::ResourceKind::Text));
            }
        }
        if guide_changed && self.classic_visible && self.editor.is_none() && !self.browse {
            self.refresh_classic_programme();
            fx.invalidate(plx_machine::present::Provenance::Resource(plx_machine::present::ResourceKind::Text));
        }
        if matches!(ev, ScreenEvent::Input(input) if matches!(input.kind,
            InputKind::Key { key: Key::Back, edge: Edge::Down, .. })) { self.restore_memory = None; }
        if matches!(ev, ScreenEvent::Activate(_) | ScreenEvent::PressCommit(_) | ScreenEvent::PressHold(_))
        {
            self.search_return.pending = None;
            if self.snapshot.page_id != "actions" { self.search_return.from_actions = None; }
        }
        if let ScreenEvent::Input(input) = ev {
            if let Some(handled) = self.classic_arrow_input(input, cx, fx) { return handled; }
            if let Some(handled) = self.remote_input(input, cx, fx) { return handled; }
        }
        if !matches!(ev, ScreenEvent::App(AppMsg::OttSnapshot(_))) {
            match self.grid_event(ev, cx, fx) {
                Some(plx_ui::cards::CardEvent::Activate(key)) => { self.activate(key, false, fx); return Handled::Yes; }
                Some(plx_ui::cards::CardEvent::Hold(key)) => { self.activate(key, true, fx); return Handled::Yes; }
                _ => {}
            }
        }
        match ev {
            ScreenEvent::RestoreMemory(PageMemory::Ott(memory)) => {
                self.restore_memory = Some(memory.clone());
                if self.snapshot.revision > 0 { self.install(self.snapshot.clone()); }
                Handled::Yes
            }
            ScreenEvent::Enter(_) => {
                self.classic_visible = true;
                fx.push(Fx::App(AppFx::Ott(OttOut::Refresh { reply: self.instance })));
                Handled::Yes
            }
            ScreenEvent::App(AppMsg::OttSnapshot(snapshot)) => {
                self.accept_search_return(snapshot);
                if self.editor.is_some() {
                    if snapshot.revision >= self.snapshot.revision { self.snapshot = snapshot.clone(); }
                } else { self.install(snapshot.clone()); }
                self.grid_event(ev, cx, fx);
                fx.invalidate(plx_machine::present::Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::App(AppMsg::OttEditor(spec)) => {
                self.prepare_search_return(spec);
                self.keyboard(false, fx);
                self.preview = None; self.preview_token = None; self.preview_dwell = 0.0;
                if let Some(editor) = self.editor.as_ref().filter(|editor|
                    editor.spec.session != 0 && (editor.spec.session != spec.session || editor.spec.kind != spec.kind)) {
                    self.send(editor.cancel_action(), fx);
                }
                let (return_page, return_focus) = match &self.editor {
                    Some(editor) if editor.return_page == self.snapshot.page_id =>
                        (editor.return_page.clone(), editor.return_focus.clone()),
                    Some(_) => (self.snapshot.page_id.clone(), None),
                    None => {
                        let header_focus = cx.focus.current.and_then(|key| {
                            self.header(cx.measure).filter(|header| header.stops.iter().any(|(elem, _)| *elem == key.elem))
                                .and_then(|_| self.keys.iter().find(|(_, elem)| **elem == key.elem).map(|(id, _)| id.clone()))
                        });
                        (self.snapshot.page_id.clone(), header_focus.or_else(|| self.form.selected_id().cloned()))
                    },
                };
                let title = Editor::display_title(spec, &return_page);
                self.editor = Some(Editor { draft: TextBuffer::new(spec.value.clone(), spec.value.len()),
                    spec: spec.clone(), title, pending: false, error: String::new(), return_page, return_focus });
                self.editor_form(true);
                fx.invalidate(plx_machine::present::Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::App(AppMsg::OttEditorRetired { session }) => {
                if *session != 0 && self.editor.as_ref().is_some_and(|editor|
                    editor.spec.session == *session && editor.spec.kind == OttEditorKind::Setting)
                {
                    self.close_editor(fx);
                }
                Handled::Yes
            }
            ScreenEvent::App(AppMsg::OttEditorResult { session, error }) => {
                if self.editor.as_ref().is_some_and(|editor| editor.spec.session == *session && editor.pending) {
                    if let Some(error) = error {
                        if let Some(editor) = &mut self.editor {
                            editor.pending = false;
                            editor.error = error.clone();
                        }
                        self.editor_form(false);
                    } else {
                        if self.editor.as_ref().is_some_and(|editor| editor.spec.kind == OttEditorKind::Search) {
                            self.commit_search_return();
                            self.browse = false; self.browse_return = None;
                        }
                        self.close_editor(fx);
                    }
                    fx.invalidate(plx_machine::present::Provenance::Input);
                }
                Handled::Yes
            }
            ScreenEvent::WillLeave(_) | ScreenEvent::Unmount => {
                self.search_return = Default::default();
                self.classic_visible = false;
                self.preview = None; self.preview_token = None; self.preview_dwell = 0.0;
                self.cancel_editor(fx); Handled::Yes
            }
            ScreenEvent::Cover | ScreenEvent::Suspend => {
                self.classic_visible = false;
                self.preview = None; self.preview_token = None; self.preview_dwell = 0.0;
                self.keyboard(false, fx); Handled::Yes
            }
            ScreenEvent::Resume | ScreenEvent::Uncover => {
                self.classic_visible = true;
                fx.push(Fx::App(AppFx::Ott(OttOut::Refresh { reply: self.instance })));
                Handled::Yes
            }
            ScreenEvent::Tick(tick) => {
                if self.number_notice_until.is_some_and(|until| tick.ms.wrapping_sub(until) < (1 << 31)) {
                    self.number_notice.dismiss(); self.number_notice_until = None;
                }
                self.number_notice.update(tick.dt());
                if let Some(header) = self.header(cx.measure) {
                    header.step(self.entry, cx.focus.current, &mut self.header_capsules, &mut self.header_pop, tick.dt());
                }
                let table_frame = self.table_frame();
                plx_base::diag::spans::span("ott_table_layout", || {
                    self.form.table.fit_notes(table_frame.w, cx.measure);
                    self.form.table.update(tick.dt(), table_frame.h);
                });
                self.reader.update(tick.dt());
                // A same-ID source replacement may not move focus. Re-evaluate
                // only the engine's actual focus, never an inferred first row.
                if let Some(key) = cx.focus.current.filter(|key| key.entry == self.entry) {
                    self.preview_focus(key.elem);
                } else {
                    self.preview = None; self.preview_token = None; self.preview_dwell = 0.0;
                }
                self.preview_tick(tick.dt(), fx);
                Handled::Yes
            }
            ScreenEvent::FocusMoved { to, by, .. } => {
                let header_focus = self.header(cx.measure).is_some_and(|header| header.stops.iter().any(|(key, _)| *key == to.elem));
                if matches!(by, plx_ui::screen::By::Dir | plx_ui::screen::By::Pointer) {
                    self.search_return.pending = None;
                    if self.snapshot.page_id != "actions" { self.search_return.from_actions = None; }
                    if header_focus {
                        let focus = self.keys.iter().find(|(_, key)| **key == to.elem).map(|(id, _)| id.clone());
                        if let Some(memory) = &mut self.restore_memory { memory.focus = focus; }
                    } else { self.restore_memory = None; }
                }
                if self.reading { return Handled::Yes; }
                self.focus_request = None;
                if header_focus {
                    self.form.note_engine_key(None);
                    self.preview = None; self.preview_token = None; self.preview_dwell = 0.0;
                    return Handled::Yes;
                }
                let had_description = self.has_description();
                if let Some(index) = self.media_keys.get(&to.elem) {
                    self.media = self.snapshot.rows[*index].enabled.then_some(*index);
                }
                if matches!(by, plx_ui::screen::By::Dir | plx_ui::screen::By::Pointer) {
                    if let (Some(context), Some(row)) = (self.snapshot.preview_context.as_ref(),
                        self.media_keys.get(&to.elem).and_then(|index| self.snapshot.rows.get(*index))) {
                        if let OttAction::Play(id) = &row.action {
                            if row.enabled { self.send(OttAction::FocusAt { context: context.clone(), id: id.clone() }, fx); }
                        }
                    }
                }
                self.preview_focus(to.elem);
                if self.shelves.iter().any(|shelf| shelf.index.contains_key(&to.elem)) { self.card_focus = Some(to.elem); }
                let canonical = self.canonical_card_key(to.elem);
                self.form.note_engine_key(Some(RowKey(canonical)));
                if let Some(index) = self.form.index_of_key(RowKey(canonical)) { self.form.table.sel = index as i32; }
                if had_description != self.has_description() { self.install(self.snapshot.clone()); }
                Handled::Yes
            }
            ScreenEvent::Activate(key) if !self.reading => { self.activate(*key, false, fx); Handled::Yes }
            ScreenEvent::PressCommit(_) => {
                if !self.reading { if let Some(key) = cx.focus.current { self.activate(key.elem, false, fx); } }
                Handled::Yes
            }
            ScreenEvent::PressHold(_) => {
                if !self.reading { if let Some(key) = cx.focus.current { self.activate(key.elem, true, fx); } }
                Handled::Yes
            }
            ScreenEvent::Input(input) => match &input.kind {
                InputKind::Key { key, edge, .. } if self.reading && matches!(key, Key::Up | Key::Down) && *edge != Edge::Up => {
                    if let Some(row) = self.media.and_then(|i| self.snapshot.rows.get(i)) {
                        let document_frame = self.document_frame();
                        let body = self.description(row);
                        self.read_target = self.reader.move_measured(if *key == Key::Up { -1 } else { 1 },
                            body, document_frame, cx.measure).0;
                        fx.invalidate(plx_machine::present::Provenance::Input);
                    }
                    Handled::Yes
                }
                InputKind::SystemKeyboard(up) if self.editor.is_some() => {
                    self.editing = *up && self.editor.as_ref().is_some_and(|editor| !editor.pending);
                    Handled::Yes
                }
                InputKind::Text(edit) if self.editor.is_some() => { self.edit(edit, fx); Handled::Yes }
                InputKind::Key { key, sym, edge, .. } if self.editing && *edge != Edge::Up => {
                    let edit = match (*key, *sym) {
                        (Key::Left, _) => Some(TextEdit::Left), (Key::Right, _) => Some(TextEdit::Right),
                        (_, plx_ui::consts::SDLK_BACKSPACE) => Some(TextEdit::Backspace),
                        (_, plx_ui::consts::SDLK_CLEAR) => Some(TextEdit::Clear), _ => None,
                    };
                    if let Some(edit) = edit { self.edit(&edit, fx); }
                    else if matches!(key, Key::Back | Key::Ok | Key::Down) { self.keyboard(false, fx); }
                    Handled::Yes
                }
                // A horizontal focus boundary is not parent navigation. The shared
                // focus engine drops an unhandled at_edge direction; catalogue arrows
                // and text editing have already handled their own Left semantics.
                InputKind::Key { key: Key::Back, edge: Edge::Down, .. } => {
                    if self.reading { self.reading = false; fx.invalidate(plx_machine::present::Provenance::Input); }
                    else if self.editor.is_some() {
                        if matches!(input.kind, InputKind::Key { key: Key::Back, .. }) { self.cancel_editor(fx); }
                    } else if self.browse { self.close_browse(fx); }
                    else if !self.back_from_search(fx) { self.send(OttAction::Back, fx); }
                    Handled::Yes
                }
                InputKind::Key { key: Key::Right, edge: Edge::Down, at_edge: true, .. }
                    if self.editor.is_none() && !self.reading && cx.focus.current.is_some_and(|k| self.media_keys.get(&k.elem)
                        .is_some_and(|index| self.snapshot.rows[*index].enabled)) => {
                    self.show_details(fx); Handled::Yes
                }
                _ => Handled::No,
            },
            _ => Handled::No,
        }
    }
}

impl<H: AppLike<Memory = PageMemory>> Focusable<H> for OttScreen {
    fn groups(&self, cx: &Cx<'_, H>, out: &mut Vec<GroupSpec>) {
        if self.exit.prompt.groups(out, cx.measure) { return; }
        let first = out.len();
        if self.reading { Focusable::<H>::groups(&self.document_focus(), cx, out); }
        else if self.grid_active() { Focusable::<H>::groups(&self.grid_view(), cx, out); }
        else { Focusable::<H>::groups(&self.view(cx.measure), cx, out); }
        for group in &mut out[first..] {
            let edge = if self.grid_active() && !self.shelves.is_empty() { EdgeRule::Stop } else { EdgeRule::Screen };
            group.edge[2] = edge; group.edge[3] = edge;
        }
        if let Some(header) = self.header(cx.measure) {
            if let Some(group) = header.focus(self.entry, navigation::GROUP, (self.header_scroll(), self.header_scroll()), navigation::clip()).group_spec() {
                out.push(group);
            }
        }
    }
    fn group_of(&self, key: &u32, cx: &Cx<'_, H>) -> Option<GroupId> {
        if let Some(group) = self.exit.prompt.group_of(*key) { return group; }
        if self.header(cx.measure).is_some_and(|header| header.stops.iter().any(|(elem, _)| elem == key)) {
            return Some(navigation::GROUP);
        }
        if self.reading { Focusable::<H>::group_of(&self.document_focus(), key, cx) } else if self.grid_active() { Focusable::<H>::group_of(&self.grid_view(), key, cx) } else { Focusable::<H>::group_of(&self.view(cx.measure), key, cx) }
    }
    fn press_kind(&self, key: &u32, _cx: &Cx<'_, H>) -> Option<ElemKind> {
        if self.exit.prompt.visible() { return None; }
        if self.reading || self.editor.is_some() || self.browse { return None; }
        self.media_keys.get(key).and_then(|index| self.snapshot.rows.get(*index))
            .filter(|row| row.enabled && matches!(row.action, OttAction::Play(_) | OttAction::GuideProgramme { .. })).map(|_| ElemKind::Card)
    }
    fn neighbour(&self, key: FocusKey<u32>, dir: Dir, cx: &Cx<'_, H>) -> Step<u32> {
        if let Some(step) = self.exit.prompt.neighbour(key, dir) { return step; }
        if let Some(header) = self.header(cx.measure) {
            let focus = header.focus(self.entry, navigation::GROUP, (self.header_scroll(), self.header_scroll()), navigation::clip());
            if focus.contains(key.elem) { return focus.neighbour(key, dir); }
        }
        if self.reading { Focusable::<H>::neighbour(&self.document_focus(), key, dir, cx) } else if self.grid_active() { Focusable::<H>::neighbour(&self.grid_view(), key, dir, cx) } else { Focusable::<H>::neighbour(&self.view(cx.measure), key, dir, cx) }
    }
    fn place(&self, key: &u32, cx: &Cx<'_, H>, at: At) -> Option<Placed> {
        if let Some(placed) = self.exit.prompt.place(*key, cx.measure) { return placed; }
        if let Some(header) = self.header(cx.measure) {
            if let Some(placed) = header.focus(self.entry, navigation::GROUP, (self.header_scroll(), self.header_scroll()), navigation::clip()).place(*key, at) { return Some(placed); }
        }
        if self.reading { Focusable::<H>::place(&self.document_focus(), key, cx, at) } else if self.grid_active() { Focusable::<H>::place(&self.grid_view(), key, cx, at) } else { Focusable::<H>::place(&self.view(cx.measure), key, cx, at) }
    }
    fn reconcile(&self, want: FocusKey<u32>, cx: &Cx<'_, H>) -> FocusKey<u32> {
        if let Some(key) = self.exit.prompt.reconcile(want) { return key; }
        let want = self.focus_request.map_or(want, |elem| FocusKey { entry: self.entry, elem });
        let want = if self.grid_active() { FocusKey { elem: self.card_target(want.elem), ..want } } else { want };
        if let Some(header) = self.header(cx.measure) {
            if let Some(key) = header.focus(self.entry, navigation::GROUP, (self.header_scroll(), self.header_scroll()), navigation::clip()).reconcile(want) { return key; }
        }
        if self.reading { Focusable::<H>::reconcile(&self.document_focus(), want, cx) } else if self.grid_active() { Focusable::<H>::reconcile(&self.grid_view(), want, cx) } else { Focusable::<H>::reconcile(&self.view(cx.measure), want, cx) }
    }
    fn seat(&self, group: GroupId, from: Placed, cx: &Cx<'_, H>) -> FocusKey<u32> {
        if self.exit.prompt.is_open() {
            if let Some(key) = self.exit.prompt.seat(group, self.entry) { return key; }
        }
        if group == navigation::GROUP {
            if let Some(header) = self.header(cx.measure) {
                return header.focus(self.entry, navigation::GROUP, (self.header_scroll(), self.header_scroll()), navigation::clip()).seat(from);
            }
        }
        if self.reading { Focusable::<H>::seat(&self.document_focus(), group, from, cx) } else if self.grid_active() { Focusable::<H>::seat(&self.grid_view(), group, from, cx) } else { Focusable::<H>::seat(&self.view(cx.measure), group, from, cx) }
    }
}

impl LogicalState for OttScreen {
    fn write(&self, out: &mut Canon) {
        out.bool(self.classic_mode).bool(self.classic_visible);
        self.guide_clock.write(out);
        self.viewport.write(out);
        out.option(self.snapshot.category_label.as_ref(), |out, label| { out.str(label); });
        let preferences = self.snapshot.classic_preferences;
        out.u32(preferences.page_size() as u32).bool(preferences.list_left())
            .u32(u32::from(preferences.arrow_mode())).u32(preferences.seek13_seconds());
        out.bool(preferences.show_number()).bool(preferences.show_name()).bool(preferences.show_program()).bool(preferences.show_progress()).bool(preferences.show_description());
        out.bool(self.snapshot.volume_available);
        out.str(&self.snapshot.page_id).u64(self.snapshot.revision).seq(self.snapshot.rows.len());
        for row in &self.snapshot.rows { out.str(&row.id).bool(row.enabled).bool(row.favorite).bool(row.guide_available).bool(row.records_available); }
        out.seq(self.snapshot.shelves.len());
        for shelf in &self.snapshot.shelves {
            out.str(&shelf.id).seq(shelf.rows.len());
            for &row in &shelf.rows { out.u64(row as u64); }
        }
        out.option(self.snapshot.shelf_context.as_ref(), |out, source| { out.str(source); });
        out.option(self.card_focus, |out, key| { out.u32(key); });
        out.option(self.snapshot.preview_context.as_ref(), |out, context| { out.str(context); });
        out.option(self.snapshot.random_context.as_ref(), |out, context| { out.str(context); });
        out.option(self.snapshot.order_context.as_ref(), |out, context| { out.str(context); });
        out.bool(self.editor.is_some()).bool(self.editing);
        out.option(self.editor.as_ref(), |out, editor| { out.u64(editor.spec.session); });
        out.bool(self.editor.as_ref().is_some_and(|editor| editor.spec.kind == OttEditorKind::Search));
        out.bool(self.editor.as_ref().is_some_and(|editor| editor.spec.text_policy == OttTextPolicy::Literal));
        out.bool(self.browse);
        out.option(self.browse_return.as_ref(), |out, id| { out.str(id); });
        out.option(self.focus_request, |out, key| { out.u32(key); });
        out.option(self.editor.as_ref(), |out, editor| { out.str(&editor.return_page); });
        out.option(self.editor.as_ref().and_then(|editor| editor.return_focus.as_ref()), |out, focus| { out.str(focus); });
        out.bool(self.editor.as_ref().is_some_and(|editor| editor.pending));
        out.bool(self.editor.as_ref().is_some_and(|editor| !editor.error.is_empty()));
        let selected = self.media.and_then(|i| self.snapshot.rows.get(i));
        out.option(selected, |out, row| { out.str(&row.id); });
        out.bool(self.reading).u32(self.read_target);
        out.option(self.preview.as_ref(), |out, id| { out.str(id); });
        out.option(self.preview_token.as_ref(), |out, context| { out.str(context); });
        out.option(self.previewed.as_ref(), |out, id| { out.str(id); });
        out.f32(self.preview_dwell);
        out.u32(self.form.selected_key().map_or(0, |key| key.0));
        self.form.table.write_motion(out);
        self.header_capsules.write_motion(out);
        self.header_pop.write_motion(out);
        self.card_stack.write(out);
        out.option(self.restore_memory.as_ref(), |out, memory| memory.write(out));
        self.search_return.write(out);
        out.option(self.number_notice_until.as_ref(), |out, until| { out.u32(*until); });
        out.bool(self.number_notice.is_open());
        self.exit.write(out);
    }
    fn probe(&self, out: &mut String) { out.push_str("ott"); }
}

impl<H: AppLike<Memory = PageMemory>> Screen<H> for OttScreen {
    fn locale_changed(&mut self, _epoch: u64) {
        self.exit.prompt.relabel(msg::ott_exit_question_c(), "", msg::settings_cancel_c(), msg::ott_exit_confirm_c());
        if let Some(editor) = &mut self.editor {
            editor.title = Editor::display_title(&editor.spec, &editor.return_page);
            self.editor_form(false);
            return;
        }
        // Re-label the already admitted navigation only. Do not run install_restoring:
        // it owns progressive restore, preview and media selection, not presentation.
        let actions = navigation::available_at(&self.snapshot, self.media, self.menu_visibility,
            self.guide_clock.now_for(self.media.and_then(|i| self.snapshot.rows.get(i)).and_then(|r| r.guide.as_ref())));
        for action in actions {
            if let Some(index) = self.form.index_of(&action.id.to_owned()) {
                if let Some(row) = self.form.table.row_mut(index as i32) { *row = action.row; }
            }
        }
        for (id, label) in [("nav:refresh", msg::browse_home_refresh()), ("nav:back", msg::settings_back())] {
            if let Some(index) = self.form.index_of(&id.to_owned()) {
                if let Some(row) = self.form.table.row_mut(index as i32) { row.label = label.to_owned(); }
            }
        }
        if self.browse {
            if let Some(section) = self.form.table.sections.first_mut() { section.header = msg::ott_browse().to_owned(); }
        }
        self.status = media::status(&self.snapshot);
        self.copy = self.snapshot.description.clone();
        if !self.status.is_empty() {
            if !self.copy.is_empty() { self.copy.push_str("\n\n"); }
            self.copy.push_str(&self.status);
        }
    }

    fn memory_at(&self, focus: Option<FocusKey<u32>>) -> PageMemory {
        PageMemory::Ott(Memory::capture(self, focus))
    }
    fn name(&self) -> &'static str { "ott" }
    fn state(&self) -> &dyn LogicalState { self }
    fn crumb(&self, _cx: &Cx<'_, H>) -> Option<Cow<'_, str>> { Some(Cow::Borrowed("OTTPlay")) }
    fn prepare(&mut self, _budget: &mut Budget, _cx: &Cx<'_, H>) {}
    fn draw(&mut self, frame: &mut DrawFrame<'_, '_, H>) {
        // Use the same mode-specific surface with or without an admitted preview.
        let ground = if self.classic_mode { plx_ui::palette::color(plx_ui::palette::Role::Background, theme::CLASSIC_BACKGROUND) } else { plx_ui::palette::color(plx_ui::palette::Role::Background, theme::SURFACE_APP) };
        // This owned page paints the ground before its foreground-only parts.
        if let Some(aperture) = self.viewport_aperture() {
            plx_gfx::gfx::frame_clear_through();
            plx_ui::classic::ground_around(frame.painter, Rect::FULL, aperture, ground);
        } else { plx_gfx::gfx::frame_clear(ground[0], ground[1], ground[2]); }
        if self.classic_mode {
            if let Some(layout) = self.classic_layout.get() {
                let title = self.editor.as_ref().map_or_else(|| navigation::classic_caption(&self.snapshot),
                    |editor| editor.title.clone());
                plx_ui::classic::caption_styled(frame.painter, layout, &title, self.snapshot.classic_preferences.page_size(), self.classic_clock.as_deref(), frame.measure);
            }
        }
        if self.reading {
            if let Some(row) = self.media.and_then(|i| self.snapshot.rows.get(i)) {
                if !self.classic_mode { metadata::paint(frame.painter, frame.measure, row, &self.status, self.status_working(), true, false, self.route(), None, self.guide_clock.now_for(row.guide.as_ref())); }
                let rect = self.document_frame();
                let body = self.description(row);
                Part::<H>::draw(&mut DocumentPart { reader: &mut self.reader, frame: rect, body,
                    group: GroupId(0), entry: self.entry }, frame, rect);
            }
        } else {
            let mut view = self.view(frame.measure);
            let initial_copy;
            if !self.classic_mode && self.editor.is_none() && self.snapshot.page_id == "loading" {
                // The initial wait belongs to the rendered frame, not the model
                // constructor: discovery and hidden snapshots must not accrue debt.
                let loading = msg::browse_library_loading();
                plx_ui::placeholder::note(frame.painter, plx_ui::placeholder::Reason::WorkingReadout, loading);
                initial_copy = format!("{}\n\n{loading}", self.copy);
                view.header.copy = &initial_copy;
            }
            if self.classic_mode && self.snapshot.page_id == "loading" {
                use plx_ui::View;
                let loading = std::ffi::CString::new(msg::browse_library_loading()).unwrap_or_default();
                plx_ui::widgets::StatusOverlay::new(self.route().narrative,
                    &loading, plx_ui::widgets::StatusKind::Working)
                    .phase(frame.cx.tick.ms).draw(&plx_ui::Env::inert(), frame.painter);
            }
            if self.grid_active() { Part::<H>::draw(&mut self.grid_view(), frame, self.route().content); }
            else {
                if !self.classic_mode && self.editor.is_none() && !self.browse && self.media.is_none() && self.status_working() {
                    view.header.working_suffix = Some((&self.status, "ott-status"));
                }
                Part::<H>::draw(&mut view, frame, Rect::FULL);
            }
            if let Some(header) = self.header(frame.measure) {
                header.draw(frame.painter, self.header_scroll(), &self.header_capsules, self.header_pop.scale_with(0, frame.press.scale), None);
                if frame.records_stops() {
                    let focus = header.focus(self.entry, navigation::GROUP, (self.header_scroll(), self.header_scroll()), navigation::clip());
                    for &(elem, _) in &header.stops {
                        let Some(placed) = focus.place(elem, At::Drawn) else { continue };
                        frame.stop(frame.painter, Stop {
                            key: FocusKey { entry: self.entry, elem },
                            rect: placed.rect, rest_rect: placed.rest_rect, clip: placed.clip,
                            hover: Hover::Focus, activate: Activate::Press,
                        });
                    }
                }
            }
            if self.classic_mode && self.snapshot.page_id != "loading" && (self.editor.is_some() || self.browse || self.media.is_none()) {
                let selection_copy;
                let copy = if self.form.table.is_classic_choices() && self.editor.is_none() && !self.browse && self.status.is_empty() {
                    selection_copy = classic_settings_list::copy(self.selected_snapshot_row(), &self.copy);
                    selection_copy.as_str()
                } else { self.editor_copy() };
                let working = (self.editor.is_none() && self.status_working()).then_some((self.status.as_str(), "ott-status"));
                plx_ui::text_view::TextView::new(copy, theme::size::BODY, plx_ui::palette::color(plx_ui::palette::Role::Muted, theme::TEXT_SECONDARY))
                    .with_measure(frame.measure).draw_working_suffix(frame.painter, self.route().narrative, working);
            }
            if self.editor.is_none() && !self.browse { if let Some(row) = self.media.and_then(|i| self.snapshot.rows.get(i)) {
                if self.classic_mode {
                    if let Some(layout) = self.classic_layout.get() {
                        classic::paint_left(frame.painter, layout, row, &self.status, self.status_working(), self.guide_clock.now_for(row.guide.as_ref()), self.snapshot.classic_preferences, frame.measure);
                    }
                } else { metadata::paint(frame.painter, frame.measure, row, &self.status, self.status_working(), false, self.grid_active(), self.route(), self.viewport_aperture(), self.guide_clock.now_for(row.guide.as_ref())); }
            } }
        }
        if self.number_notice.visible() && self.accepts_channel_number() {
            self.number_notice.draw(msg::settings_cancel_c(), msg::settings_cancel_c(), frame.measure);
        }
        self.exit.prompt.draw(frame, self.entry);
    }
    fn links(&self, out: &mut Vec<Link>) {
        if self.exit.prompt.visible() { return; }
        if self.editor.is_none() && !self.reading {
            let first = if self.grid_active() { self.shelves.first().map_or(GroupId(0), |shelf| shelf.group) } else { GroupId(0) };
            out.push(Link { from: first, dir: Dir::Up, to: navigation::GROUP });
            out.push(Link { from: navigation::GROUP, dir: Dir::Down, to: first });
            if self.grid_active() {
                for pair in self.shelves.windows(2) {
                    out.push(Link { from: pair[0].group, dir: Dir::Down, to: pair[1].group });
                    out.push(Link { from: pair[1].group, dir: Dir::Up, to: pair[0].group });
                }
            }
        }
    }
    fn render(&self) -> RenderStrategy { RenderStrategy::Page }
    fn focus_source(&self) -> FocusSource { FocusSource::Engine }
    fn hit_source(&self) -> HitSource { HitSource::Engine }
    fn requires_presented_activation(&self, key: &u32) -> bool {
        self.form.index_of_key(RowKey(*key)).and_then(|i| self.form.activate(i)).is_some_and(|activation|
            matches!(activation, Activation::Action(FormAction::Forward(OttAction::EditSetting(key)))
                if key.starts_with("remote.choice:")))
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> { Some(self) }
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> { Some(self) }
}

/// Paint-only standby treatment. The application owns playback parking and the
/// first-key/click wake; this creates no screen, focus stop or input trap.
pub fn draw_standby(painter: plx_ui::Painter) {
    use plx_ui::View;
    let black = plx_ui::theme::scrim_black(1.0);
    painter.rect(plx_ui::Rect::FULL, 0.0, black, black, 0.0);
    plx_ui::widgets::StatusOverlay::new(plx_ui::Rect::FULL, msg::ott_standby_c(), plx_ui::widgets::StatusKind::Empty)
        .draw(&plx_ui::Env::inert(), painter);
}

#[cfg(test)]
mod input_tests;

#[cfg(test)]
mod synopsis_tests;

#[cfg(test)]
pub(crate) mod cards_harness;

#[cfg(test)]
mod tests {
    #[test]
    fn locale_refresh_keeps_native_editor_draft_and_owner() {
        use plx_platform::i18n::{language_on_this_thread_for_test, Preference};
        let mut screen = {
            let _english = language_on_this_thread_for_test(Preference::En);
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            editor(&mut screen, "retained draft");
            screen
        };
        let owner = screen.editor.as_ref().unwrap().spec.clone();
        let snapshot = screen.snapshot.clone();
        let hash = screen.hash();
        let selected = screen.form.selected_key();
        let _spanish = language_on_this_thread_for_test(Preference::Es);
        <OttScreen as Screen<TestHost>>::locale_changed(&mut screen, 1);
        assert_eq!(screen.editor.as_ref().unwrap().draft.text(), "retained draft");
        assert!(Arc::ptr_eq(&owner, &screen.editor.as_ref().unwrap().spec));
        assert!(Arc::ptr_eq(&snapshot, &screen.snapshot));
        assert_eq!(screen.hash(), hash);
        assert_eq!(screen.form.selected_key(), selected);
        let index = screen.form.index_of(&"edit:cancel".to_owned()).unwrap();
        assert_eq!(screen.form.table.row_mut(index as i32).unwrap().label, msg::settings_cancel());
    }

    mod remote;
    mod search;
    mod navigation;
    mod reader;
    mod media;
    mod editor_layout;
    mod editor_repair;
    mod editor_locale;
    mod classic_presentation;
    mod classic_clock;
    mod classic_metadata;
    mod modern_viewport;
    mod modern_caption;
    mod settings_navigation;
    mod classic_settings_list;
    mod working_status;
    mod torrent_classic;
    use super::*;
    use plx_machine::machine::{Host, Stamped};
    struct TestHost;
    impl Host for TestHost {
        type Arg = crate::registry::AppArg;
        type Fx = AppFx;
        type Msg = AppMsg;
        type Elem = u32;
        type Views<'a> = ();
        type Init = crate::registry::AppArg;
        type Memory = PageMemory;
    }
    #[test]
    fn private_snapshot_allows_pairing_but_refuses_editors_and_other_pages() {
        for page in ["providers", "settings"] {
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            screen.snapshot = Arc::new(OttView { page_id: page.into(),
                status: "Pairing code: ABCD1234".into(), ..Default::default() });
            assert!(screen.ui_snapshot_allowed());
            editor(&mut screen, "private field");
            assert!(!screen.ui_snapshot_allowed());
        }
        for page in ["", "profile-setup", "profiles:m3u", "channels", "settings:credentials"] {
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            screen.snapshot = Arc::new(OttView { page_id: page.into(), ..Default::default() });
            assert!(!screen.ui_snapshot_allowed());
        }
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        screen.snapshot = Arc::new(OttView { page_id: "settings".into(), ..Default::default() });
        screen.reading = true;
        assert!(!screen.ui_snapshot_allowed());
    }

    fn effects(run: impl FnOnce(&mut Effects<'_, TestHost>)) -> Vec<Stamped<TestHost>> {
        let mut output = Vec::new();
        let mut present = plx_machine::present::Present::new();
        run(&mut Effects::new(&mut output, MachineId::Instance(InstanceId(1)), &mut present));
        output
    }
    fn editor(screen: &mut OttScreen, value: &str) {
        editor_session(screen, value, 7);
    }
    fn editor_session(screen: &mut OttScreen, value: &str, session: u64) {
        let spec = Arc::new(OttEditor { session, kind: OttEditorKind::Setting, text_policy: OttTextPolicy::Literal, key: "profile.source".into(), title: "Source".into(),
            value: value.into(), secret: true, limit: 64 });
        deliver(screen, ScreenEvent::App(AppMsg::OttEditor(spec)));
    }
    fn deliver(screen: &mut OttScreen, event: ScreenEvent<TestHost>) {
        let _ = deliver_recorded(screen, event);
    }
    fn deliver_recorded(screen: &mut OttScreen, event: ScreenEvent<TestHost>) -> Vec<Stamped<TestHost>> {
        let measure = plx_ui::fixture::FixtureMeasure;
        let cx = Cx { views: (), tick: Default::default(), measure: &measure,
            focus: Default::default(), press: Default::default(),
            owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
        effects(|fx| { screen.step(&event, &cx, fx); })
    }
    fn key_event(key: Key) -> ScreenEvent<TestHost> {
        ScreenEvent::Input(plx_machine::machine::InputEvent { at: Default::default(), source: plx_machine::machine::Source::Script,
            kind: InputKind::Key { key, sym: 0, wcode: 0, edge: Edge::Down, at_edge: true } })
    }
    fn snapshot(revision: u64, ids: &[&str]) -> Arc<OttView> {
        Arc::new(OttView { shelf_context: None, shelves: Vec::new(), status_working: false, classic_preferences: Default::default(), volume_available: false, category_label: None, order_context: None, preview_context: Some("source-fixture".into()), random_context: None, catalog_progress: None, page_id: "channels".into(), revision, title: "Channels".into(),
            description: String::new(), status: String::new(), selected_row: None, search: None,
            rows: ids.iter().map(|id| OttRow { current_programme: None, content_title: None, guide: None, guide_available: false, records_available: false, media: None, id: id.to_string(), title: id.to_string(), detail: String::new(),
                description: "Complete description. A second sentence remains available.".into(), poster: None,
                action: OttAction::Play(id.to_string()), enabled: true, favorite: false }).collect() })
    }
    #[test]
    fn numeric_notice_preserves_catalogue_selection_and_expires() {
        let _lock = plx_base::testlock::serial();
        for classic in [false, true] {
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            screen.install(snapshot(7, &["one", "two"]));
            screen.classic_visible = true;
            screen.classic_mode = classic;
            let selected = screen.form.selected_key();
            let keys = screen.keys.clone();
            screen.set_number_notice(&Some(("2".into(), "two".into())), 100);
            assert!(screen.number_notice.is_open());
            assert_eq!(screen.number_notice.body_for_test()[0].as_ref(), "2");
            assert_eq!(screen.number_notice.body_for_test()[1].as_ref(), "two");
            assert_eq!(screen.form.selected_key(), selected);
            assert_eq!(screen.keys, keys, "passive feedback adds no focus targets");
            deliver(&mut screen, ScreenEvent::Tick(plx_machine::machine::Tick { ms: 2_100, dt_us: 16_000 }));
            assert!(!screen.number_notice.is_open());
            assert!(screen.number_notice_until.is_none());
        }
    }

    #[test]
    fn numeric_notice_refuses_editor_reader_and_unrendered_context() {
        let _lock = plx_base::testlock::serial();
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        screen.install(snapshot(7, &["one"])); screen.classic_visible = true;
        assert!(screen.accepts_channel_number());
        assert!(screen.channel_number_context("channels", 7));
        assert!(!screen.channel_number_context("channels", 8));
        assert!(!screen.channel_number_context("favorites", 7));
        screen.reading = true;
        screen.set_number_notice(&Some(("1".into(), "one".into())), 100);
        assert!(!screen.number_notice.is_open());
        screen.reading = false;
        editor(&mut screen, "private value");
        assert!(!screen.accepts_channel_number());
        screen.set_number_notice(&Some(("1".into(), "one".into())), 100);
        assert!(!screen.number_notice.is_open());
    }

    #[test]
    fn favorites_is_always_reachable_even_without_a_provider() {
        let screen = OttScreen::new(EntryId(1), InstanceId(1));
        assert!(screen.form.index_of_key(RowKey(screen.keys["nav:favorites"])).is_some());
        assert!(screen.form.index_of_key(RowKey(screen.keys["nav:back"])).is_some());
    }
    #[test]
    fn navigation_and_editor_rows_fit_every_shipped_language() {
        let mut failures = Vec::new();
        let width = RouteLayout::screen().sectioned_table().w;
        for language in plx_platform::i18n::SHIPPED {
            let _locale = plx_platform::i18n::language_on_this_thread_for_test(language);
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            failures.extend(screen.form.table.app_fit_failures(width, language.tag()));
            screen.install(snapshot(1, &["channel"]));
            effects(|fx| screen.open_browse(fx));
            failures.extend(screen.form.table.app_fit_failures(width, language.tag()));
            editor(&mut screen, "");
            failures.extend(screen.form.table.app_fit_failures(width, language.tag()));
            editor(&mut screen, "user supplied value");
            failures.extend(screen.form.table.app_fit_failures(width, language.tag()));
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
    #[test]
    fn new_catalogue_rows_keep_existing_focus_identities() {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        screen.install(snapshot(1, &["b", "c"]));
        let key = screen.keys["item:b"];
        screen.install(snapshot(2, &["a", "b", "c"]));
        assert_eq!(screen.keys["item:b"], key);
        screen.install(snapshot(1, &["obsolete"]));
        assert_eq!(screen.snapshot.revision, 2);
        assert!(!screen.keys.contains_key("item:obsolete"));
    }
    #[test]
    fn edit_is_private_and_only_save_emits_a_setting_change() {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        editor(&mut screen, "first-secret");
        let mut before = Canon::new(); screen.write(&mut before);
        effects(|fx| screen.edit(&TextEdit::Clear, fx));
        effects(|fx| screen.edit(&TextEdit::Commit("second-secret".into()), fx));
        let mut after = Canon::new(); screen.write(&mut after);
        assert_eq!(before.finish(), after.finish(), "private text leaked into diagnostic state");
        let save = screen.keys["edit:save"];
        let changes = effects(|fx| screen.activate(save, false, fx));
        assert!(changes.iter().any(|change| matches!(&change.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::SubmitSetting { session: 7, key, value }, .. }))
            if key == "profile.source" && value == "second-secret")));
        assert!(screen.handles_private_text(), "Save waits for the durable result");
        deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditorResult { session: 7, error: None }));
        assert!(!screen.handles_private_text());
        let action = OttAction::SubmitSetting { session: 7, key: "source".into(), value: "secret-must-not-log".into() };
        assert!(!format!("{action:?}").contains("secret-must-not-log"));
    }
    #[test]
    fn cancel_discards_draft_and_long_press_toggles_only_playable_rows() {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        screen.install(snapshot(1, &["channel"]));
        let key = screen.keys["item:channel"];
        assert!(effects(|fx| screen.activate(key, true, fx)).iter().any(|x| matches!(&x.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::ToggleFavorite(id), .. })) if id == "channel")));
        editor(&mut screen, "draft");
        let cancel = screen.keys["edit:cancel"];
        let changes = effects(|fx| screen.activate(cancel, false, fx));
        assert_eq!(changes.iter().filter(|x| matches!(&x.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::CancelSetting { session: 7 }, .. })))).count(), 1);
        assert!(!changes.iter().any(|x| matches!(&x.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::SubmitSetting { .. }, .. })))));
        assert_eq!(screen.snapshot.rows.len(), 1);
        assert!(!screen.handles_private_text());
    }
    #[test]
    fn m3u_media_link_editor_edits_and_submits_existing_value_above_eight_kib() {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        let original = "x".repeat(9000);
        deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor {
            session: 7, kind: OttEditorKind::Setting, text_policy: OttTextPolicy::Literal,
            key: "profile.media".into(), title: "Media library link".into(),
            value: original.clone(), secret: true, limit: 16384,
        }))));
        assert_eq!(screen.editor.as_ref().unwrap().draft.text(), original);
        effects(|fx| screen.edit(&TextEdit::Backspace, fx));
        assert_eq!(screen.editor.as_ref().unwrap().draft.text().len(), 8999);
        effects(|fx| screen.edit(&TextEdit::Commit("z".into()), fx));
        let expected = format!("{}z", "x".repeat(8999));
        assert_eq!(screen.editor.as_ref().unwrap().draft.text(), expected);
        let save = screen.keys["edit:save"];
        let sent = effects(|fx| screen.activate(save, false, fx));
        assert_eq!(sent.iter().filter(|change| matches!(&change.fx,
            Fx::App(AppFx::Ott(OttOut::Action {
                action: OttAction::SubmitSetting { session: 7, key, value }, ..
            })) if key == "profile.media" && value == &expected)).count(), 1);
        assert!(screen.editor.as_ref().unwrap().pending);
    }

    #[test]
    fn m3u_media_link_editor_larger_cap_is_exactly_scoped_and_honors_declared_limit() {
        for (kind, key, limit, cap) in [
            (OttEditorKind::Setting, "profile.media", 16384, 16384),
            (OttEditorKind::Setting, "profile.media", 64, 64),
            (OttEditorKind::Setting, "profile.source", 16384, 8192),
            (OttEditorKind::Setting, "profile.media.other", 16384, 8192),
            (OttEditorKind::Search, "profile.media", 16384, 8192),
        ] {
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor {
                session: 7, kind, text_policy: OttTextPolicy::Literal,
                key: key.into(), title: "Fixture".into(), value: String::new(), secret: true, limit,
            }))));
            let allowed = "x".repeat(cap);
            effects(|fx| screen.edit(&TextEdit::Commit(allowed.clone().into()), fx));
            assert_eq!(screen.editor.as_ref().unwrap().draft.text(), allowed, "{kind:?}/{key}");
            effects(|fx| screen.edit(&TextEdit::Commit("x".into()), fx));
            assert_eq!(screen.editor.as_ref().unwrap().draft.text(), allowed, "{kind:?}/{key}");
        }
    }

    #[test]
    fn editor_pending_result_retains_draft_and_allows_retry_after_error() {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        editor(&mut screen, "private-source");
        let save = screen.keys["edit:save"];
        let first = effects(|fx| screen.activate(save, false, fx));
        assert_eq!(first.iter().filter(|change| matches!(&change.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::SubmitSetting { session: 7, value, .. }, .. }))
            if value == "private-source")).count(), 1);
        assert!(screen.editor.as_ref().unwrap().pending);
        assert!(effects(|fx| screen.activate(save, false, fx)).iter().all(|change| !matches!(change.fx, Fx::App(_))));
        effects(|fx| screen.edit(&TextEdit::Clear, fx));
        assert_eq!(screen.editor.as_ref().unwrap().draft.text(), "private-source");
        let edit = screen.keys["edit:value"];
        effects(|fx| screen.activate(edit, false, fx));
        assert!(!screen.editing);
        deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditorResult { session: 99, error: None }));
        assert!(screen.editor.as_ref().unwrap().pending);
        deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditorResult {
            session: 7, error: Some("Invalid source".into()),
        }));
        let current = screen.editor.as_ref().unwrap();
        assert!(!current.pending);
        assert_eq!(current.error, "Invalid source");
        assert_eq!(current.draft.text(), "private-source");
        let retry = effects(|fx| screen.activate(save, false, fx));
        assert_eq!(retry.iter().filter(|change| matches!(&change.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::SubmitSetting { session: 7, value, .. }, .. }))
            if value == "private-source")).count(), 1);
        deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditorResult { session: 7, error: None }));
        assert!(screen.editor.is_none());
    }

    #[test]
    fn editor_pending_cancel_and_back_ignore_results_for_a_replaced_session() {
        for use_back in [false, true] {
            let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
            editor_session(&mut screen, "old-private-source", 7);
            let save = screen.keys["edit:save"];
            effects(|fx| screen.activate(save, false, fx));
            if use_back { deliver(&mut screen, key_event(Key::Back)); }
            else {
                let cancel = screen.keys["edit:cancel"];
                effects(|fx| screen.activate(cancel, false, fx));
            }
            assert!(screen.editor.is_none());
            editor_session(&mut screen, "new-private-source", 8);
            let save = screen.keys["edit:save"];
            effects(|fx| screen.activate(save, false, fx));
            for error in [None, Some("Old failure".into())] {
                deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditorResult { session: 7, error }));
                let current = screen.editor.as_ref().unwrap();
                assert_eq!(current.spec.session, 8);
                assert_eq!(current.draft.text(), "new-private-source");
                assert!(current.pending);
                assert!(current.error.is_empty());
            }
            deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditorResult { session: 8, error: None }));
            assert!(screen.editor.is_none());
        }
    }

    #[test]
    fn source_retirement_closes_only_the_exact_unsubmitted_setting_session() {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        editor_session(&mut screen, "old draft", 7);
        editor_session(&mut screen, "replacement draft", 8);
        deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditorRetired { session: 7 }));
        assert_eq!(screen.editor.as_ref().unwrap().draft.text(), "replacement draft");
        assert!(!screen.editor.as_ref().unwrap().pending);
        deliver(&mut screen, ScreenEvent::App(AppMsg::OttSnapshot(snapshot(2, &["new-row"]))));
        assert!(screen.editor.is_some(), "metadata alone is not retirement");
        let effects = deliver_recorded(&mut screen,
            ScreenEvent::App(AppMsg::OttEditorRetired { session: 8 }));
        assert!(screen.editor.is_none());
        assert!(screen.keys.contains_key("item:new-row"));
        assert!(!effects.iter().any(|change| matches!(&change.fx,
            Fx::App(AppFx::Ott(OttOut::Action {
                action: OttAction::CancelSetting { .. } | OttAction::SubmitSetting { .. }, ..
            })))), "retirement must not enqueue cancellation or another save");
    }

    #[test]
    fn source_retirement_cannot_close_search_or_legacy_session_zero() {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        editor_session(&mut screen, "legacy", 0);
        deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditorRetired { session: 0 }));
        assert!(screen.editor.is_some());
        deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor {
            session: 9, kind: OttEditorKind::Search, text_policy: OttTextPolicy::Predictive,
            key: String::new(), title: "Search".into(), value: "draft query".into(),
            secret: false, limit: 256,
        }))));
        deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditorRetired { session: 9 }));
        assert_eq!(screen.editor.as_ref().unwrap().draft.text(), "draft query");
    }

    #[test]
    fn companion_legacy_editor_session_zero_keeps_immediate_submit_behavior() {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        editor_session(&mut screen, "legacy-value", 0);
        let save = screen.keys["edit:save"];
        let changes = effects(|fx| screen.activate(save, false, fx));
        assert!(changes.iter().any(|change| matches!(&change.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::SubmitSetting { session: 0, .. }, .. })))));
        assert!(screen.editor.is_none());
    }

    #[test]
    fn editor_replacement_and_unmount_cancel_but_cover_and_success_do_not() {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        editor_session(&mut screen, "draft", 7);
        for event in [ScreenEvent::Cover, ScreenEvent::Suspend] {
            let changes = deliver_recorded(&mut screen, event);
            assert!(!changes.iter().any(|change| matches!(&change.fx,
                Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::CancelSetting { .. }, .. })))));
            assert!(screen.editor.is_some());
        }
        let changes = deliver_recorded(&mut screen, ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor {
            session: 0, kind: OttEditorKind::Setting, text_policy: OttTextPolicy::Predictive, key: "legacy".into(), title: "Legacy".into(), value: String::new(), secret: false, limit: 256,
        }))));
        assert_eq!(changes.iter().filter(|change| matches!(&change.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::CancelSetting { session: 7 }, .. })))).count(), 1);
        editor_session(&mut screen, "next", 8);
        let changes = deliver_recorded(&mut screen, ScreenEvent::Unmount);
        assert_eq!(changes.iter().filter(|change| matches!(&change.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::CancelSetting { session: 8 }, .. })))).count(), 1);
        assert!(screen.editor.is_none());
        editor_session(&mut screen, "saved", 9);
        let save = screen.keys["edit:save"];
        effects(|fx| screen.activate(save, false, fx));
        let changes = deliver_recorded(&mut screen, ScreenEvent::App(AppMsg::OttEditorResult { session: 9, error: None }));
        assert!(screen.editor.is_none());
        assert!(!changes.iter().any(|change| matches!(&change.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::CancelSetting { .. }, .. })))));
    }

    #[test]
    fn editor_success_uses_the_new_page_instead_of_its_old_return_focus() {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        let mut profiles = (*snapshot(1, &["old-profile"])).clone();
        profiles.page_id = "profiles:m3u".into();
        profiles.selected_row = Some("old-profile".into());
        screen.install(Arc::new(profiles));
        editor(&mut screen, "private-source");
        let save = screen.keys["edit:save"];
        effects(|fx| screen.activate(save, false, fx));
        let mut channels = (*snapshot(2, &["new-channel"])).clone();
        channels.selected_row = Some("new-channel".into());
        deliver(&mut screen, ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(channels))));
        deliver(&mut screen, ScreenEvent::App(AppMsg::OttEditorResult { session: 7, error: None }));
        assert!(screen.editor.is_none());
        assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:new-channel"));
    }

    #[test]
    fn selected_metadata_opens_the_full_scrolling_description_and_returns() {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        let mut view = (*snapshot(1, &["first", "second"])).clone();
        view.rows[1].description = "First sentence. Second sentence.\n\n".repeat(200);
        screen.install(Arc::new(view));
        let key = screen.keys["item:second"];
        deliver(&mut screen, ScreenEvent::FocusMoved { from: None, to: FocusKey { entry: EntryId(1), elem: key }, by: plx_ui::screen::By::Dir });
        let open = screen.keys["nav:description"];
        effects(|fx| screen.activate(open, false, fx));
        assert!(screen.reading); assert_eq!(screen.media, Some(1));
        deliver(&mut screen, key_event(Key::Down));
        assert!(screen.read_target > 0);
        deliver(&mut screen, key_event(Key::Back));
        assert!(!screen.reading); assert_eq!(screen.media, Some(1));
    }
    #[test]
    fn catalogue_status_remains_visible_with_selected_media() {
        for status in ["Catalogue limit reached", "Source unavailable", "128 videos"] {
            for reading in [false, true] {
                let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
                let mut view = (*snapshot(1, &["Selected video"])).clone();
                view.status = status.into();
                view.selected_row = Some("Selected video".into());
                screen.install(Arc::new(view));
                screen.reading = reading;
                let measure = plx_ui::fixture::FixtureMeasure;
                let cx = Cx::<TestHost> { views: (), tick: Default::default(), measure: &measure,
                    focus: Default::default(), press: Default::default(),
                    owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
                let mut frame = DrawFrame::new(&cx, plx_ui::Painter::recording());
                let runs = plx_ui::draw_census::capture_runs(|| {
                    plx_gfx::gfx::without_frame_clear(|| screen.draw(&mut frame))
                });
                assert!(runs.iter().any(|run| run.text == "Selected video" && run.ink[3] > 0.0),
                    "the selected video remains visible");
                let painted = runs.iter().find(|run| run.text == status && run.ink[3] > 0.0)
                    .unwrap_or_else(|| panic!("{status:?} disappeared behind selected media (reading={reading})"));
                let area = RouteLayout::screen().narrative;
                assert!(painted.at.x >= area.x && painted.at.x + painted.at.w <= area.x + area.w + 1.0,
                    "the status stays in the metadata column: {:?}", painted.at);
            }
        }
    }
    #[test]
    fn initial_loading_is_counted_only_when_drawn() {
        use plx_ui::placeholder::{self, Reason};
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        let measure = plx_ui::fixture::FixtureMeasure;
        let cx = Cx::<TestHost> { views: (), tick: Default::default(), measure: &measure,
            focus: Default::default(), press: Default::default(),
            owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
        let draw = |screen: &mut OttScreen, painter| {
            placeholder::capture_declared(|| {
                let mut frame = DrawFrame::new(&cx, painter);
                plx_gfx::gfx::without_frame_clear(|| screen.draw(&mut frame));
            }).1
        };
        assert_eq!(draw(&mut screen, plx_ui::Painter::recording()).count, 0);
        let loading = draw(&mut screen, plx_ui::Painter::root());
        assert_eq!(loading.of(Reason::WorkingReadout), 1);
        assert_eq!(loading.entries[0].key, msg::browse_library_loading());
        screen.install(snapshot(1, &["Ready video"]));
        assert_eq!(draw(&mut screen, plx_ui::Painter::root()).of(Reason::WorkingReadout), 0);
    }
    #[test]
    fn left_edge_never_discards_an_uncommitted_editor() {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        editor(&mut screen, "draft");
        deliver(&mut screen, key_event(Key::Left));
        assert!(screen.handles_private_text());
        deliver(&mut screen, key_event(Key::Back));
        assert!(!screen.handles_private_text());
    }

    #[test]
    fn metadata_preview_waits_for_settled_focus_and_never_plays() {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        screen.install(snapshot(1, &["first", "second"]));
        screen.preview_focus(screen.keys["item:first"]);
        let early = effects(|fx| screen.preview_tick(0.10, fx));
        assert!(!early.iter().any(|x| matches!(x.fx, Fx::App(_))));
        screen.preview_focus(screen.keys["item:second"]);
        let ready = effects(|fx| screen.preview_tick(0.21, fx));
        assert_eq!(ready.iter().filter(|x| matches!(&x.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::PreviewAt { id, .. }, .. })) if id == "second")).count(), 1);
        assert!(!ready.iter().any(|x| matches!(&x.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::Play(_), .. })))));
        screen.preview_focus(screen.keys["item:second"]);
        assert!(!effects(|fx| screen.preview_tick(1.0, fx)).iter().any(|x| matches!(x.fx, Fx::App(_))));
        screen.preview_focus(screen.keys["item:first"]);
        screen.preview_focus(screen.keys["nav:settings"]);
        assert!(!effects(|fx| screen.preview_tick(1.0, fx)).iter().any(|x| matches!(x.fx, Fx::App(_))));
    }


    fn preview_snapshot(revision: u64, context: Option<&str>) -> Arc<OttView> {
        let mut view = (*snapshot(revision, &["same"])).clone();
        view.preview_context = context.map(str::to_owned);
        Arc::new(view)
    }

    #[test]
    fn preview_without_context_never_falls_back_to_unscoped_action() {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        screen.install(preview_snapshot(1, None));
        screen.preview_focus(screen.keys["item:same"]);
        assert!(screen.preview.is_none() && screen.preview_token.is_none());
        assert!(!effects(|fx| screen.preview_tick(1.0, fx)).iter()
            .any(|effect| matches!(effect.fx, Fx::App(_))));
    }

    #[test]
    fn preview_same_page_new_source_discards_old_dwell_and_rearms_same_id() {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        screen.install(preview_snapshot(1, Some("source-a")));
        screen.preview_focus(screen.keys["item:same"]);
        assert!(!effects(|fx| screen.preview_tick(0.19, fx)).iter()
            .any(|effect| matches!(effect.fx, Fx::App(_))));
        screen.install(preview_snapshot(2, Some("source-b")));
        assert!(screen.preview.is_none() && screen.preview_token.is_none());
        assert_eq!(screen.preview_dwell, 0.0);
        assert!(!effects(|fx| screen.preview_tick(1.0, fx)).iter()
            .any(|effect| matches!(effect.fx, Fx::App(_))));
        screen.preview_focus(screen.keys["item:same"]);
        let sent = effects(|fx| screen.preview_tick(0.21, fx));
        assert_eq!(sent.iter().filter(|effect| matches!(&effect.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::PreviewAt { context, id }, .. }))
                if context == "source-b" && id == "same")).count(), 1);
        assert!(!sent.iter().any(|effect| matches!(&effect.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::Preview(_) | OttAction::Play(_), .. })))));
    }

    #[test]
    fn preview_same_source_snapshot_keeps_dwell_and_does_not_repeat_completed_request() {
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        screen.install(preview_snapshot(1, Some("source-a")));
        screen.preview_focus(screen.keys["item:same"]);
        effects(|fx| screen.preview_tick(0.10, fx));
        screen.install(preview_snapshot(2, Some("source-a")));
        let sent = effects(|fx| screen.preview_tick(0.11, fx));
        assert_eq!(sent.iter().filter(|effect| matches!(&effect.fx,
            Fx::App(AppFx::Ott(OttOut::Action { action: OttAction::PreviewAt { context, id }, .. }))
                if context == "source-a" && id == "same")).count(), 1);
        screen.install(preview_snapshot(3, Some("source-a")));
        screen.preview_focus(screen.keys["item:same"]);
        assert!(!effects(|fx| screen.preview_tick(1.0, fx)).iter()
            .any(|effect| matches!(effect.fx, Fx::App(_))));
    }

    #[test]
    fn standby_is_a_quiet_label_without_controls() {
        let runs = plx_ui::draw_census::capture_runs(|| draw_standby(plx_ui::Painter::recording()));
        assert!(runs.iter().any(|run| run.text == msg::ott_standby()));
        assert!(runs.iter().all(|run| run.text == msg::ott_standby()));
    }
}

#[cfg(test)]
mod menu_visibility_tests {
    use super::*;
    use plx_appkit::menu_visibility::{self, Item, Visibility};
    struct Reset(Visibility);
    impl Drop for Reset { fn drop(&mut self) { menu_visibility::project(self.0); } }
    #[test]
    fn actual_browse_menu_hides_rows_but_always_keeps_settings_and_back() {
        let _reset = Reset(menu_visibility::projected());
        let mut hidden = Visibility::default();
        for item in Item::ALL { hidden.hide(item); }
        menu_visibility::project(hidden);
        let mut screen = OttScreen::new(EntryId(91), InstanceId(91));
        for id in ["nav:settings", "nav:back"] {
            assert!(screen.form.index_of_key(RowKey(screen.keys[id])).is_some());
        }
        for id in ["nav:providers", "nav:profiles", "nav:catalog", "nav:categories", "nav:favorites", "nav:refresh"] {
            assert!(screen.keys.get(id).is_none_or(|key| screen.form.index_of_key(RowKey(*key)).is_none()));
        }
        menu_visibility::project(Default::default());
        screen.install(Arc::new(OttView { revision: 2, ..OttView::default() }));
        for id in ["nav:providers", "nav:profiles", "nav:catalog", "nav:categories", "nav:favorites", "nav:refresh"] {
            assert!(screen.form.index_of_key(RowKey(screen.keys[id])).is_some());
        }
    }
}

#[cfg(test)]
mod menu_header_tests {
    use super::*;
    use plx_appkit::menu_visibility::{self, Item, Visibility};
    struct Reset(Visibility);
    impl Drop for Reset { fn drop(&mut self) { menu_visibility::project(self.0); } }
    #[test]
    fn live_hide_and_reset_removes_cached_header_search_favorite_and_description_stops() {
        let _reset = Reset(menu_visibility::projected());
        menu_visibility::project(Default::default());
        let measure = plx_ui::fixture::FixtureMeasure;
        let mut screen = OttScreen::new(EntryId(92), InstanceId(92));
        let mut view = OttView { revision: 1, page_id: "channels".into(),
            search: Some(OttSearch { query: String::new(), scope: "Home".into() }),
            rows: vec![OttRow { current_programme: None, content_title: None, guide: None, guide_available: false, records_available: false, media: None, id: "film".into(), title: "Film".into(), detail: String::new(),
                description: "Details".into(), poster: None, action: OttAction::Play("film".into()),
                enabled: true, favorite: false }], ..Default::default() };
        screen.install(Arc::new(view.clone()));
        let search = screen.keys["nav:search"];
        let description = screen.keys["nav:description"];
        let favorite = screen.keys["nav:favorite"];
        let before = screen.header(&measure).unwrap();
        assert!(before.stops.iter().any(|(key,_)| *key == search));
        assert!(before.stops.iter().any(|(key,_)| *key == description));
        assert!(before.stops.iter().any(|(key,_)| *key == favorite));
        let mut hidden = Visibility::default(); hidden.hide(Item::Search); hidden.hide(Item::Description); hidden.hide(Item::Favorite);
        menu_visibility::project(hidden); view.revision += 1;
        screen.install(Arc::new(view.clone()));
        let after = screen.header(&measure).unwrap();
        assert!(!after.stops.iter().any(|(key,_)| *key == search || *key == favorite || *key == description));
        menu_visibility::project(Default::default()); view.revision += 1;
        screen.install(Arc::new(view));
        let reset = screen.header(&measure).unwrap();
        assert!(reset.stops.iter().any(|(key,_)| *key == search));
        assert!(reset.stops.iter().any(|(key,_)| *key == description));
        assert!(reset.stops.iter().any(|(key,_)| *key == favorite));
    }
}

#[cfg(test)]
#[path = "random_tests.rs"]
mod random_tests;

#[cfg(test)]
#[path = "order_tests.rs"]
mod order_tests;

#[cfg(test)]
#[path = "usb_folder_tests.rs"]
mod usb_folder_tests;

#[cfg(test)]
#[path = "records_tests.rs"]
mod records_tests;

#[cfg(test)]
mod guide_timer_marker_tests {
    use super::*;
    use plx_ui::Painter;
    #[test]
    fn native_guide_snapshot_projects_timer_to_date_in_classic_and_modern() {
        let _serial = plx_base::testlock::serial();
        let measure = plx_ui::fixture::FixtureMeasure;
        let frame = Rect::new(0.0, 0.0, 900.0, 2400.0);
        for classic in [false, true] {
            let mut screen = OttScreen::new(EntryId(195), InstanceId(195));
            screen.classic_mode = classic;
            screen.form.table.set_classic(classic);
            let mut snapshot = OttView { page_id: "guide".into(), rows: vec![OttRow {
                current_programme: None, content_title: None, guide: None,
                guide_available: false, records_available: false, media: None,
                id: "programme".into(), title: "Programme".into(), detail: "12:34".into(),
                description: String::new(), poster: None,
                action: OttAction::GuideProgramme { generation: 1, programme_id: "programme".into() },
                enabled: true, favorite: false,
            }], ..Default::default() };
            let dots = |screen: &OttScreen| plx_ui::draw_census::capture_rect_inks(||
                screen.form.table.draw(Painter::recording(), frame, &measure))
                .iter().filter(|(r,_,_)| r.w == 8.0 && r.h == 8.0).count();
            screen.install(Arc::new(snapshot.clone())); let plain = dots(&screen);
            snapshot.rows[0].favorite = true; snapshot.revision += 1;
            screen.install(Arc::new(snapshot.clone())); assert_eq!(dots(&screen), plain + 1);
            let row = screen.form.table.sections.iter().flat_map(|s| &s.rows)
                .find(|r| r.label == "Programme").unwrap();
            assert!(!row.checked, "timer is not a title checkbox");
            assert_eq!(row.detail, "12:34", "marker never rewrites programme data");
            snapshot.rows[0].favorite = false; snapshot.revision += 1;
            screen.install(Arc::new(snapshot)); assert_eq!(dots(&screen), plain);
        }
    }
}
