//! Native OTT integration. The app owns IO and publishes immutable screen snapshots;
//! provider parsing and portable identities remain independent of rendering and Plex.
use super::*;
#[path = "ott_auth_return.rs"]
pub(super) mod auth_return;
#[path = "ott_catalog_return.rs"]
mod catalog_return;
#[path = "ott_classic_bookmark.rs"]
mod classic_bookmark;
#[path = "ott_classic_clock.rs"]
mod classic_clock;
#[path = "ott_classic_navigation.rs"]
mod classic_navigation;
#[path = "ott_classic_play_handoff.rs"]
mod classic_play_handoff;
#[path = "ott_classic_source.rs"]
mod classic_source;
#[path = "ott_exit_confirmation.rs"]
mod exit_confirmation;
#[path = "ott_menu_settings.rs"]
mod menu_settings;
#[path = "ott_player_browse.rs"]
mod player_browse;
#[path = "ott_player_startup.rs"]
mod player_startup;
#[path = "ott_row_guide.rs"]
pub(super) mod row_guide;
#[path = "ott_vportal_quality.rs"]
mod vportal_quality;

#[path = "ott_archive_play.rs"]
mod archive_play;
#[path = "ott_archive_programme.rs"]
mod archive_programme;
#[path = "ott_commands.rs"]
mod commands;
#[path = "ott_guide_projection.rs"]
mod guide_projection;
#[path = "ott_hosted_epg_settings.rs"]
mod hosted_epg_settings;
#[path = "ott_live_hud.rs"]
mod live_hud;
#[path = "ott_live_label.rs"]
mod live_label;
#[path = "ott_live_witness.rs"]
mod live_witness;
#[path = "ott_managed.rs"]
mod managed;
#[path = "ott_favorite_profile_rename.rs"]
mod profile_rename;
#[path = "ott_selected_guide.rs"]
mod selected_guide;
pub(super) use managed::{
    foreground as managed_foreground, input_fence as managed_input_fence, start as start_managed,
};
#[path = "ott_local_assets.rs"]
mod asset_selection;
#[path = "ott_browse_focus.rs"]
mod browse_focus;
#[path = "ott_catalog_diagnostics.rs"]
mod catalog_diagnostics;
#[path = "ott_catalog_organization.rs"]
mod catalog_organization;
#[path = "ott_catalog_publication.rs"]
mod catalog_publication;
#[path = "ott_category_preferences.rs"]
mod category_preferences;
#[path = "ott_category_state.rs"]
mod category_state;
#[path = "ott_companion_adapter.rs"]
mod companion_adapter;
#[path = "ott_continuity.rs"]
mod continuity;
#[path = "ott_continuity_diagnostics.rs"]
mod continuity_diagnostics;
#[path = "ott_desktop_input.rs"]
mod desktop_input;
#[path = "ott_failed_playback.rs"]
mod failed_playback;
#[path = "ott_favorites_item_ids.rs"]
mod favorite_item_ids;
#[path = "ott_favorites.rs"]
mod favorites;
#[path = "ott_favorites_items.rs"]
mod favorites_items;
#[path = "ott_favorites_recovery.rs"]
mod favorites_recovery;
#[path = "ott_guide_runtime.rs"]
mod guide_ui;
#[path = "ott_hls_tracks.rs"]
mod hls_tracks;
#[path = "ott_input_policy.rs"]
mod input_policy;
#[path = "ott_interface_mode.rs"]
mod interface_mode;
#[path = "ott_local_asset_runtime.rs"]
pub(super) mod local_assets;
#[path = "ott_local_profile.rs"]
mod local_profile;
#[path = "ott_m3u_attachment_runtime.rs"]
mod m3u_attachment_runtime;
#[path = "ott_m3u_media_editor.rs"]
mod m3u_media_editor;
#[path = "ott_m3u_slots.rs"]
mod m3u_slots;
#[path = "ott_menu_visibility.rs"]
mod menu_visibility;
#[path = "ott_modern_shelves.rs"]
mod modern_shelves;
#[path = "ott_native_controls.rs"]
mod native_controls;
#[path = "ott_native_playback_diagnostics.rs"]
mod native_playback_diagnostics;
#[path = "ott_notification_settings.rs"]
mod notification_settings;
#[path = "ott_numeric_input.rs"]
mod numeric_input;
#[path = "ott_catalog_organization_runtime.rs"]
mod organization_runtime;
#[path = "ott_catalog_organization_ui.rs"]
mod organization_ui;
#[path = "ott_osd_ui.rs"]
mod osd_ui;
#[path = "ott_parental.rs"]
mod parental;
#[path = "ott_parental_runtime.rs"]
mod parental_runtime;
#[path = "ott_playback_failure.rs"]
mod playback_failure;
#[path = "ott_playback_menu.rs"]
mod playback_menu;
#[path = "ott_playback_step.rs"]
mod playback_step;
#[path = "ott_player_remote.rs"]
mod player_remote;
#[path = "ott_playing_favorite.rs"]
mod playing_favorite;
#[path = "ott_preplay_diagnostics.rs"]
mod preplay;
#[path = "ott_preview.rs"]
mod preview;
#[path = "ott_profile_edit.rs"]
mod profile_edit;
#[path = "ott_profile_setup.rs"]
mod profile_setup;
#[path = "ott_public_preferences.rs"]
mod public_preferences;
#[path = "ott_random.rs"]
mod random;
#[path = "ott_reminders_runtime.rs"]
mod reminders;
#[path = "ott_remote_settings.rs"]
mod remote_settings;
#[path = "ott_save_error.rs"]
mod save_error;
#[path = "ott_search.rs"]
mod search;
#[path = "ott_selection.rs"]
mod selection;
#[path = "ott_settings_groups.rs"]
mod settings_groups;
#[path = "ott_sleep_timer.rs"]
mod sleep_timer;
#[path = "ott_subtitle_settings.rs"]
mod subtitle_settings;
#[path = "ott_transient.rs"]
mod transient;
#[path = "ott_ui_projection.rs"]
mod ui_projection;
#[path = "ott_ui_text.rs"]
mod ui_text;
#[path = "ott_usb_resume.rs"]
mod usb_resume;
#[path = "ott_usb_runtime.rs"]
mod usb_runtime;
pub(super) use transient::LocalKioskPreparationReceipt as LocalKioskPreparation;
#[path = "ott_kiosk_capture.rs"]
mod kiosk_capture;
#[path = "ott_kiosk_resume.rs"]
mod kiosk_resume;
#[path = "ott_kiosk_source.rs"]
mod kiosk_source;
#[path = "ott_kiosk_usb.rs"]
mod kiosk_usb;
use super::ott_catalog::{self, Event as CatalogEvent};
use super::ott_local_session::OwnerToken;
use super::ott_remote::{Invocation, Remote};
use plx_data::ott::favorites::{Document, Favorite};
use plx_data::ott::{
    Catalog, CatalogEntry, MediaLocator, ProviderKind, ProviderProfile, ProviderProfiles,
};
use plx_machine::machine::{Delivery, Fx, InstanceId, MachineId};
use plx_platform::storage::{private, CommitReceipt};
use plx_platform::tv::secure::Namespace;
use plx_screens::ott::{OttAction, OttEditor, OttOut, OttPoster, OttRow, OttTextPolicy, OttView};
use plx_screens::registry::AppMsg;
pub(super) use selection::{
    attachment_profile, legacy_id_collision, matches_entry, LocalFavoritePermit,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender, SyncSender},
    Arc,
};
pub(super) use transient::plex_remote::PlexKioskPreparation;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Saved {
    version: u32,
    profiles: ProviderProfiles,
    favorites: Document,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "favorite_item_ids::present"
    )]
    favorite_items: Option<favorite_item_ids::RowIds>,
    reminders: plx_data::ott::reminders::Document,
    local_favorite_proofs: selection::LocalFavoriteProofs,
    category: String,
    settings: BTreeMap<String, Value>,
    favorites_selected: bool,
    receipts: Vec<plx_data::ott::favorites_wire::DurableStatus>,
    resume: BTreeMap<String, SavedResume>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedResume {
    entry_id: String,
    position_ms: u64,
    #[serde(default)]
    source_fingerprint: String,
}
impl Default for Saved {
    fn default() -> Self {
        Self {
            version: 1,
            profiles: ProviderProfiles::default(),
            favorites: Document::default(),
            favorite_items: None,
            reminders: plx_data::ott::reminders::Document::default(),
            local_favorite_proofs: selection::LocalFavoriteProofs::default(),
            category: String::new(),
            settings: BTreeMap::new(),
            favorites_selected: false,
            receipts: Vec::new(),
            resume: BTreeMap::new(),
        }
    }
}
impl Saved {
    fn plex_sources(&self) -> Result<Vec<plx_plex::plex::session::SourceRef>, &'static str> {
        let sources: Vec<plx_plex::plex::session::SourceRef> =
            match self.settings.get("nativePlexSources") {
                Some(value) => serde_json::from_value(value.clone())
                    .map_err(|_| "Invalid stored Plex sources")?,
                None => Vec::new(),
            };
        if sources.len() > 15 {
            return Err("Too many stored Plex sources");
        }
        let mut ids = std::collections::BTreeSet::new();
        for source in &sources {
            let origin = source.origin().ok_or("Invalid stored Plex origin")?;
            if source.machine_id.is_empty()
                || source.token.is_empty()
                || !ids.insert(source.machine_id.clone())
            {
                return Err("Invalid stored Plex source identity");
            }
            super::ott_profile::Profile::parse(json!({"address":origin.base(),"token":source.token,"serverId":source.machine_id,"name":source.name,"allowInsecureHttp":!origin.is_tls()}).to_string().as_bytes())?;
        }
        Ok(sources)
    }
    fn validate(&self) -> Result<(), &'static str> {
        parental::Policy::read(&self.settings).map_err(|_| "Invalid parental policy")?;
        if self.version != 1 || self.category.len() > 4096 || self.receipts.len() > 32 {
            return Err("Invalid native profile state");
        }
        if serde_json::to_vec(&self.receipts)
            .map_err(|_| "Invalid receipt ledger")?
            .len()
            > 65536
        {
            return Err("Receipt ledger exceeds limit");
        }
        self.profiles
            .validate()
            .map_err(|_| "Invalid provider profiles")?;
        self.favorites
            .validate()
            .map_err(|_| "Invalid Favorites document")?;
        if let Some(ids) = &self.favorite_items {
            ids.validate()?;
        }
        self.reminders
            .encode()
            .map_err(|_| "Invalid reminder document")?;
        self.local_favorite_proofs
            .validate()
            .map_err(|_| "Invalid private Local Favorite proofs")?;
        self.plex_sources()?;
        crate::app::ott_m3u_attachment::Registry::read(&self.settings, &self.profiles)
            .map_err(|error| error.message())?;
        super::ott_local_sources::RememberedFolders::read(&self.settings)?;
        subtitle_settings::Stored::read(self)?;
        menu_visibility::Stored::read(self)?;
        remote_settings::saved_address(self)?;
        category_preferences::Preferences::read(&self.settings)
            .map_err(|_| "Invalid category preferences")?;
        catalog_organization::Document::read(&self.settings).map_err(|error| error.message())?;
        super::ott_kiosk_stored::read(&self.settings).map_err(|_| "Invalid stored kiosk policy")?;
        if serde_json::to_vec(&self.settings)
            .map_err(|_| "Invalid settings")?
            .len()
            > 1024 * 1024
        {
            return Err("Settings exceed storage limit");
        }
        if self.resume.len() > 91
            || self.resume.iter().any(|(key, value)| {
                key.len() > 128
                    || value.entry_id.len() > 4096
                    || value.source_fingerprint.len() > 128
                    || value.position_ms > u64::MAX / 1_000_000
            })
        {
            return Err("Invalid playback resume state");
        }
        Ok(())
    }
}
enum ResolvedWork {
    Ready(CatalogEntry, ReadySource),
    Choices(CatalogEntry, ott_catalog::VPortalChoices),
}
enum Job {
    Preplay(preplay::Event),
    VPortalPrepared(
        u64,
        ProviderProfile,
        CatalogEntry,
        ott_catalog::VPortalChoices,
    ),
    HlsTrack(hls_tracks::Completion),
    Parental(parental::Completion),
    PublicLoaded(Result<Option<public_preferences::Snapshot>, &'static str>),
    PublicSaved(
        Result<public_preferences::Snapshot, public_preferences::WriteFailure>,
        Option<Invocation>,
        Value,
    ),
    Loaded(Result<(Saved, [u8; 16]), &'static str>),
    LoadedItemsUncertain(Saved, [u8; 16]),
    PlexRestored(
        Arc<AtomicBool>,
        Vec<Result<super::ott_import::VerifiedSource, &'static str>>,
    ),
    PlexVerified(
        Invocation,
        Result<super::ott_import::VerifiedSource, &'static str>,
    ),
    Saved(Result<Saved, &'static str>, Option<Invocation>, bool, Value),
    SaveRejected(private::Error, Option<Invocation>, bool, Value),
    Uncertain(Option<Saved>, Option<Invocation>),
    Play(
        u64,
        ProviderProfile,
        Result<(CatalogEntry, ReadySource), &'static str>,
    ),
    PlexPrepared(
        OwnerToken,
        transient::plex_remote::PreparationId,
        Result<super::ott_plex_prepare::Proposal, super::ott_plex_queue::WireError>,
    ),
    FavoriteCommit(favorites::CommitResult),
    FavoriteRecovery(favorites_recovery::Completion),
}
#[derive(Clone, PartialEq)]
struct PlaybackRequest {
    kiosk_selection: Option<kiosk_source::PinnedSelection>,
    archive: Option<archive_play::Intent>,
    transient_owner: Option<OwnerToken>,
    local_selection: Option<transient::LocalSelection>,
    // Present only for an explicitly owned continuation, never inferred from the URL.
    continuation: Option<plx_media::route::PlaybackIdentity>,
    id: String,
    position_ms: u64,
    advance_on_failure: bool,
}
impl std::fmt::Debug for PlaybackRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaybackRequest")
            .field("id", &self.id)
            .field("position_ms", &self.position_ms)
            .field("advance_on_failure", &self.advance_on_failure)
            .field(
                "transient_owner",
                &self.transient_owner.map(|owner| owner.0),
            )
            .finish_non_exhaustive()
    }
}
impl PlaybackRequest {
    fn ordinary(id: &str, position_ms: u64) -> Self {
        Self {
            kiosk_selection: None,
            archive: None,
            continuation: None,
            transient_owner: None,
            local_selection: None,
            id: id.into(),
            position_ms,
            advance_on_failure: true,
        }
    }
}
/// Evidence local to one Job::Play dispatch. Only the actual synchronous
/// start_resolved call may produce Some(receipt); it is never retained.
pub(super) struct NativeStartReceipt {
    pub generation: u64,
    pub before: plx_media::route::PlayRequestTicket,
    pub after: plx_media::route::PlayRequestTicket,
}
struct ResolveRequest {
    local_read: Option<transient::LocalSourceRead>,
    archive: Option<archive_play::Intent>,
    plex: Option<transient::plex_remote::Preparation>,
    plex_choice: Option<transient::plex_remote::NativeChoice>,
    generation: u64,
    profile: ProviderProfile,
    entry: Option<CatalogEntry>,
    favorite: Option<Favorite>,
    local_proof: Option<LocalFavoritePermit>,
    cancel: Arc<AtomicBool>,
}
enum MetadataSelection {
    Entry(CatalogEntry),
    Favorite(Favorite, Option<LocalFavoritePermit>),
    Local(local_assets::Work),
    LocalPublish(local_assets::Completion),
}
enum ReadySource {
    PreparedPlex(transient::plex_remote::NativeChoice),
    External(
        plx_media::ff::ExternalSource,
        plx_media::ff::inspect::MediaInfo,
        Option<super::ott_vportal_lease::Lease>,
    ),
    Plex {
        server_id: String,
        rating_key: String,
    },
}
#[derive(Clone)]
struct Playing {
    channel_capture: Option<selection::ChannelCapture>,
    kiosk_selection: Option<kiosk_source::PinnedSelection>,
    archive: Option<archive_play::Intent>,
    transient_owner: Option<OwnerToken>,
    local_selection: Option<transient::LocalSelection>,
    identity: Option<plx_media::route::PlaybackIdentity>,
    profile: ProviderProfile,
    entry: CatalogEntry,
    selection_id: String,
    source: Option<plx_media::ff::ExternalSource>,
    // Source-owned policy survives the outer RPC's successful dispatch reply.
    advance_on_failure: bool,
    // Immutable fact of this source, even after its lease moves to a worker.
    requires_server_lease: bool,
}
// One cleanup slot with an explicit owner: a rejected candidate must not be
// mistaken for the currently playing source when an exact request fails.
enum HeldLease {
    Playback(super::ott_vportal_lease::Lease),
    Cleanup(super::ott_vportal_lease::Lease),
}
impl HeldLease {
    fn is_playback(&self) -> bool {
        matches!(self, Self::Playback(_))
    }
    fn begin_release(&self) {
        match self {
            Self::Playback(lease) | Self::Cleanup(lease) => lease.begin_release(),
        }
    }
    fn into_lease(self) -> super::ott_vportal_lease::Lease {
        match self {
            Self::Playback(lease) | Self::Cleanup(lease) => lease,
        }
    }
}
#[cfg(test)]
impl From<super::ott_vportal_lease::Lease> for HeldLease {
    fn from(lease: super::ott_vportal_lease::Lease) -> Self {
        Self::Playback(lease)
    }
}
#[derive(Clone, PartialEq, Eq)]
struct EditorSession {
    id: u64,
    owner: InstanceId,
    key: String,
    purpose: search::EditorPurpose,
}

/// Protected-state availability is independent of transient catalogue/UI status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StateLoad {
    Idle,
    Loading,
    Failed(&'static str),
}

/// One writer slot, with browse authority distinct from playback ownership.
/// Ordinary persistence must never reselect the active provider in a browsed page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveNavigation {
    Preserve,
    Activate(u64),
}

/// Only the source workflow present when a remote activation was admitted.
/// Session IDs and the navigation epoch prevent late completion from owning a
/// replacement draft, even if its profile/key/visible page happen to match.
struct SourceEditorCapture {
    owner: InstanceId,
    navigation: u64,
    setup: Option<u64>,
    editor: Option<EditorSession>,
}

struct SetupSession {
    id: u64,
    owner: Option<InstanceId>,
    draft: profile_setup::Draft,
    original: Option<ProviderProfile>,
    selected_key: String,
}

pub(super) struct Runtime {
    settings_groups: settings_groups::State,
    classic_navigation: classic_navigation::State,
    auth_return: auth_return::State,
    native_catalog: Option<player_browse::NativeCatalog>,
    menu_settings: menu_settings::State,
    desktop_input: desktop_input::State,
    numeric_input: numeric_input::State,
    sleep_timer: sleep_timer::State,
    // These long-lived opaque owners hold large optional receipt/source payloads.
    // Heap ownership keeps moving Runtime through input dispatch bounded; the
    // state, lifetime and authority rules remain owned by their existing modules.
    organization: Box<organization_runtime::State>,
    parental: parental_runtime::State,
    guide: Option<guide_ui::State>,
    search: search::Search,
    transient: transient::State,
    local_inventory: transient::Inventory,
    saved: Saved,
    reminders: reminders::State,
    reminder_save: Option<plx_data::ott::reminders::CommitToken>,
    m3u_attachments: crate::app::ott_m3u_attachment::Authorities,
    play_attachment: Option<crate::app::ott_m3u_attachment::Receipt>,
    playing_attachment: Option<crate::app::ott_m3u_attachment::Receipt>,
    metadata_attachment: Option<crate::app::ott_m3u_attachment::Receipt>,
    write_permit: private::WritePermit,
    clock_sample: Option<plx_base::task::time::ClockSample>,
    classic_clock: classic_clock::State,
    /// Presentation accepted only after verified load or durable save/apply.
    /// An uncertain visible Saved image must not update the player HUD.
    osd_display: (
        plx_appkit::player_hud::model::OsdOpacity,
        plx_appkit::player_hud::model::PermanentClock,
    ),
    /// Only accepted settings may change list geometry or input routing.
    classic_preferences: plx_appkit::classic_settings::Preferences,
    /// Per-provider display flags from the same accepted settings publication.
    classic_display: BTreeMap<&'static str, [u8; 5]>,
    clock_anchor: Option<plx_base::task::time::ClockSample>,
    wall_clock_ms: u64,
    catalog: Catalog,
    catalog_diagnostics: catalog_diagnostics::State,
    preplay: preplay::State,
    playback_failure: playback_failure::State,
    explicit_step: playback_step::State,
    native_playback_capture: native_playback_diagnostics::Capture,
    page: &'static str,
    provider: ProviderKind,
    revision: u64,
    generation: u64,
    catalog_revision: u64,
    selected: Option<String>,
    browse_focus: browse_focus::State,
    classic_bookmark: classic_bookmark::State,
    local_profile: local_profile::State,
    profile_focus: Option<String>,
    profile_edit_epoch: u64,
    settings_focus: Option<&'static str>,
    plex_omitted_servers: usize,
    reply: Option<InstanceId>,
    status: String,
    saving: bool,
    pending_profile_rename: Option<profile_rename::Plan>,
    playback_epoch: u64,
    initial_native_ticket: plx_media::route::PlayRequestTicket,
    save_playback_epoch: u64,
    navigation_epoch: u64,
    save_navigation: SaveNavigation,
    save_ui_owner: Option<InstanceId>,
    save_source_editor: Option<SourceEditorCapture>,
    subtitle_save: Option<subtitle_settings::PendingSave>,
    hls_tracks: hls_tracks::State,
    public_preferences: public_preferences::State,
    loaded: bool,
    state_load: StateLoad,
    cancel: Arc<AtomicBool>,
    catalog_tx: SyncSender<CatalogEvent>,
    catalog_rx: Receiver<CatalogEvent>,
    tx: Sender<Job>,
    rx: Receiver<Job>,
    remote: Remote,
    edit_profile: Option<ProviderProfile>,
    edit_profile_original: Option<ProviderProfile>,
    profile_edit_save_only: Option<EditorSession>,
    m3u_slot_draft: Option<m3u_slots::Draft>,
    profile_edit_pending: Option<profile_edit::SaveOnly>,
    edit_serial: u64,
    edit_session: Option<EditorSession>,
    exit_confirmation: exit_confirmation::State,
    pending_editor_save: Option<EditorSession>,
    native_controls: Option<native_controls::Pending>,
    pending_category_choice: Option<category_state::Pending>,
    pending_playing_favorite: Option<playing_favorite::Pending>,
    setup_serial: u64,
    setup: Option<SetupSession>,
    pending_setup_save: Option<u64>,
    archive_hours_editor: Option<archive_settings::Editor>,
    m3u_media_editor: Option<m3u_media_editor::Editor>,
    play_generation: u64,
    resolver_busy: bool,
    resolver_next: Option<ResolveRequest>,
    play_cancel: Arc<AtomicBool>,
    play_ticket: Option<plx_media::route::PlayRequestTicket>,
    playing: Option<Playing>,
    // A rare receipt must not add another full Playing to every Runtime stack copy.
    failed_playback: Option<Box<failed_playback::FailedPlayback>>,
    pending_play: Option<Invocation>,
    favorites: plx_data::ott::favorites_wire::Protocol,
    favorites_recovery_done: Arc<AtomicBool>,
    remote_snapshot: Value,
    catalog_loading: bool,
    catalog_complete: bool,
    // An ordinary Back retains a source-bound terminal outcome or the intent
    // to resume cancelled catalogue work. It never retains playback authority.
    catalog_return: Option<catalog_return::Pending>,
    catalog_dirty: bool,
    catalog_warning: Option<(u64, &'static str)>,
    catalog_done: Arc<AtomicBool>,
    catalog_next: Option<ProviderProfile>,
    entropy: Option<[u8; 16]>,
    uuid_counter: u64,
    command_receipts: Box<commands::Receipts>,
    kiosk_capture: kiosk_capture::Capture,
    verified_imports: Vec<super::ott_import::VerifiedSource>,
    managed: managed::State,
    classic_source: classic_source::State,
    continuity: continuity::Continuity,
    continuity_retry_log: Option<continuity_diagnostics::Pending>,
    play_target: Option<PlaybackRequest>,
    classic_play_handoff: classic_play_handoff::State,
    // Exact source captured at resolver admission. Catalogue generation and
    // selection IDs alone do not identify a pending mixed-Favorites request.
    play_profile: Option<(u64, ProviderProfile)>,
    /// Only an automatic saved-file restore may be superseded by transport on
    /// the fallback that is still playing. An explicit successor keeps priority.
    restore_play_generation: Option<u64>,
    /// Only the exact automatic live retry may be cancelled by a later Pause.
    live_retry_generation: Option<u64>,
    /// Preroll protection belongs to the admitted logical playback, not its
    /// resolver generation or a later decoder reload of the same item.
    resume_floor: Option<(plx_media::route::PlaybackIdentity, continuity::ResumeFloor)>,
    resume_pending: Option<PlaybackRequest>,
    checkpoints: BTreeMap<String, SavedResume>,
    last_checkpoint_ms: u64,
    checkpoint_write_selection: Option<(String, String, String)>,
    /// Owns only the existing single-flight checkpoint writer, never a settings
    /// or verifier write. Common admission/completion retires this marker.
    checkpoint_save_epoch: Option<u64>,
    persistence_uncertain: bool,
    active_lease: Option<HeldLease>,
    quality: Box<vportal_quality::State>,
    plex_refresh_pending: bool,
    plex_refresh_requested: bool,
    companion: super::ott_companion::Supervisor,
    companion_adapter: Option<companion_adapter::Adapter>,
    metadata_tx: Sender<super::ott_metadata::Event>,
    metadata_rx: Receiver<super::ott_metadata::Event>,
    metadata_cancel: Arc<AtomicBool>,
    metadata_generation: u64,
    hosted_epg_config: Option<plx_data::ott::hosted_epg::Config>,
    metadata_target: Option<(String, String)>,
    metadata_query: Option<plx_data::ott::guide::Query>,
    metadata_guide: Option<selected_guide::Retained>,
    row_guide: Option<row_guide::Retained>,
    row_guide_attempt: Option<row_guide::Attempt>,
    row_guide_coverage: Option<row_guide::Coverage>,
    metadata_playback_identity: Option<plx_media::route::PlaybackIdentity>,
    programme_metadata_generation: Option<u32>,
    programme_leaf: Option<ProgrammeLeaf>,
    live_hud: live_hud::State,
    metadata_local_selection: Option<transient::LocalSelection>,
    metadata_done: Arc<AtomicBool>,
    metadata_next: Option<(ProviderProfile, MetadataSelection)>,
    local_assets: local_assets::State,
    favorite_preview: Option<(String, CatalogEntry)>,
    metadata_refresh_ms: u64,
    source_lifecycle: Box<work_lifecycle::State>,
}

#[path = "ott_archive_settings.rs"]
mod archive_settings;

#[path = "ott_lifecycle.rs"]
mod work_lifecycle;

#[path = "ott_erasure.rs"]
mod erasure;
pub(super) use erasure::{retire_for_erasure, Erasure};

impl Runtime {
    pub(super) fn track_selection_kiosk(&self) -> Result<super::ott_kiosk_policy::State, ()> {
        if !self.loaded || self.persistence_uncertain {
            return Err(());
        }
        Ok(
            super::ott_kiosk_stored::borrow_validated(&self.saved.settings)?
                .map_or(super::ott_kiosk_policy::State::Off, |pin| pin.state()),
        )
    }

    /// Reminder identity belongs to the configured source, never to a list offset.
    /// Transient/favorite rebinding has no durable active-catalog authority here.
    pub(super) fn reminder_source_id(&self) -> Option<String> {
        if !self.loaded || self.persistence_uncertain || self.transient.accepted.is_some() {
            return None;
        }
        self.active()
            .filter(|profile| profile.enabled)
            .map(profile_fingerprint)
    }

    pub(super) fn reminder_contains(&self, key: &plx_data::ott::reminders::Key) -> bool {
        self.reminders.contains(key)
    }

    pub(super) fn toggle_reminder(
        &mut self,
        record: plx_data::ott::reminders::Record,
        source_generation: u64,
    ) -> Result<(), &'static str> {
        if let Some(error) = self.settings_write_refusal() {
            return Err(error);
        }
        let profile = self
            .active()
            .ok_or("The programme source is no longer current")?;
        if source_generation != self.generation
            || self.reminder_source_id().as_deref() != Some(record.key.source_id.as_str())
            || !self
                .catalog
                .entries
                .iter()
                .any(|entry| entry.id == record.key.channel_id && guide_ui::is_live(profile, entry))
        {
            return Err("The programme source is no longer current");
        }
        let (candidate, token) = self
            .reminders
            .toggle(&self.saved, record, self.wall_clock_ms)?;
        match self.save(candidate, None, false) {
            Ok(()) => {
                self.reminder_save = Some(token);
                Ok(())
            }
            Err(error) => {
                // A known launch refusal cannot have applied the candidate.
                let _ = self
                    .reminders
                    .finish_save(token, Err(reminders::SaveFailure::Rejected));
                Err(error)
            }
        }
    }

    fn withdraw_reminder_ui(app: &mut App) {
        if let Some(player) = bridge::player_mut(&mut app.pages) {
            player.clear_reminders();
        }
    }

    fn reminder_effects(&mut self, app: &mut App, effects: Vec<plx_data::ott::reminders::Fx>) {
        use plx_data::ott::reminders::Fx as ReminderFx;
        let now = app.player.now_ms;
        for effect in effects {
            let token = match &effect {
                ReminderFx::PromptSwitch { token, .. } => Some(*token),
                _ => None,
            };
            let shown = bridge::player_mut(&mut app.pages)
                .is_some_and(|player| player.reminder_effect(effect, now));
            if !shown {
                if let Some(token) = token {
                    self.reminders.dismiss(token);
                }
            }
        }
    }

    fn load_reminders(&mut self, app: &mut App) -> Result<(), &'static str> {
        Self::withdraw_reminder_ui(app);
        self.reminder_save = None;
        if self.clock_sample.is_none() {
            self.reminders.invalidate();
            return Err("Reminder calendar time is unavailable");
        }
        self.uuid_counter = self
            .uuid_counter
            .checked_add(1)
            .ok_or("Operation identity limit reached")?;
        self.reminders
            .load_committed(&self.saved, self.wall_clock_ms, self.uuid_counter)
    }

    fn finish_reminders(
        &mut self,
        app: &mut App,
        result: Result<&Saved, reminders::SaveFailure>,
    ) -> Option<&'static str> {
        let token = self.reminder_save.take()?;
        match self.reminders.finish_save(token, result) {
            Ok(finished) => {
                self.reminder_effects(app, finished.effects);
                (finished.outcome == plx_data::ott::reminders::CommitOutcome::Unknown)
                    .then_some("Reminder storage must be reloaded before editing")
            }
            Err(error) => {
                self.reminders.invalidate();
                Self::withdraw_reminder_ui(app);
                Some(error)
            }
        }
    }

    fn reminders_active(&self, app: &App) -> bool {
        playback_allowed(app)
            && self.play_work_allowed()
            && self.loaded
            && !self.persistence_uncertain
            && self.clock_sample.is_some()
            && matches!(app.route(), AppArg::Player)
            && !app.pages.has_pending_navigation()
            && !app.pages.surface_up()
            && input_policy::navigation(self).is_ok()
    }

    fn poll_reminders(&mut self, app: &mut App, active: bool) {
        let source = self.reminder_source_id().unwrap_or_default();
        let context = plx_data::ott::reminders::Context {
            source_id: &source,
            revision: self.generation,
            active: active && !source.is_empty(),
        };
        let entries = &self.catalog.entries;
        let profile = self.active().cloned();
        let minutes = self
            .saved
            .settings
            .get("sEpgRemindMinutes")
            .and_then(Value::as_u64)
            .map(|minutes| minutes.min(u64::from(u32::MAX)) as u32)
            .unwrap_or(1);
        match self
            .reminders
            .poll(self.wall_clock_ms, context, minutes, |key| {
                key.source_id == source
                    && entries.iter().any(|entry| {
                        entry.id == key.channel_id
                            && profile
                                .as_ref()
                                .is_some_and(|profile| guide_ui::is_live(profile, entry))
                    })
            }) {
            Ok(effects) => self.reminder_effects(app, effects),
            Err(error) => {
                self.reminders.invalidate();
                Self::withdraw_reminder_ui(app);
                self.status = error.into();
                self.changed();
            }
        }
        if !active {
            Self::withdraw_reminder_ui(app);
        }
    }

    fn answer_reminder(
        &mut self,
        app: &mut App,
        token: plx_data::ott::reminders::PromptToken,
        accept: bool,
    ) -> Result<(), &'static str> {
        if !accept {
            self.reminders.dismiss(token);
            return Ok(());
        }
        if !self.play_work_allowed() {
            return Err("Media work is waiting for foreground or worker retirement");
        }
        input_policy::ui(self, &OttAction::ReminderAnswer { token, accept })?;
        let source = self
            .reminder_source_id()
            .ok_or("The programme source is no longer current")?;
        let profile = self
            .active()
            .ok_or("The programme source is no longer current")?;
        let resolves =
            self.reminders.prompt_key(token).is_some_and(|key| {
                key.source_id == source
                    && self.catalog.entries.iter().any(|entry| {
                        entry.id == key.channel_id && guide_ui::is_live(profile, entry)
                    })
            });
        let context = plx_data::ott::reminders::Context {
            source_id: &source,
            revision: self.generation,
            active: self.reminders_active(app),
        };
        let record = self
            .reminders
            .accept(token, self.wall_clock_ms, context, resolves)
            .ok_or("This programme reminder is no longer current")?;
        self.play_request_with_policy(app, PlaybackRequest::ordinary(&record.key.channel_id, 0))?;
        // Only the successfully admitted explicit answer may arm kiosk capture.
        kiosk_capture::explicit_admitted(self);
        guide_ui::close(self);
        Ok(())
    }

    fn release_active_lease(&mut self) {
        if let Some(held) = self.active_lease.take() {
            let lease = held.into_lease();
            lease.begin_release();
            self.active_lease = Some(HeldLease::Cleanup(lease));
        }
    }

    pub(super) fn new() -> Self {
        let mut runtime = Self::unloaded(companion_adapter::init());
        runtime.start_state_load();
        runtime
    }

    fn start_state_load(&mut self) -> bool {
        self.start_state_load_with(|sender, write_permit| {
            plx_base::task::spawn_off_frame("ott-state", move |worker| {
                let preferences = public_preferences::load();
                let _ = sender.send(Job::PublicLoaded(preferences));
                plx_machine::idle::invalidate();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    private::load(Namespace::OttState)
                        .map_err(|_| "Could not read protected OTT state")
                        .and_then(|value| match value {
                            Some(value) => {
                                serde_json::from_value(value).map_err(|_| "Invalid OTT state")
                            }
                            None => Ok(Saved::default()),
                        })
                        .and_then(|state: Saved| {
                            state.validate()?;
                            let state = favorites::recover(state, &write_permit)?;
                            let entropy = plx_platform::storage::state::Generation::random()
                                .map_err(|_| "Could not generate operation identities")?
                                .0;
                            let (state, uncertain) = favorite_item_ids::migrate_with(
                                state,
                                |state| favorite_item_ids::prepare(state, worker),
                                |state| favorites::write_verified(state, &write_permit),
                            );
                            Ok((state, entropy, uncertain))
                        })
                }))
                .unwrap_or(Err("Protected settings worker failed"));
                let job = match result {
                    Ok((state, entropy, true)) => Job::LoadedItemsUncertain(state, entropy),
                    other => Job::Loaded(other.map(|(state, entropy, _)| (state, entropy))),
                };
                let _ = sender.send(job);
                plx_machine::idle::invalidate();
            })
        })
    }

    /// The caller owns one load at a time. The spawn boundary is injectable so
    /// admission can be tested without reading or rewriting protected records.
    fn start_state_load_with(
        &mut self,
        spawn: impl FnOnce(Sender<Job>, private::WritePermit) -> bool,
    ) -> bool {
        if self.loaded
            || self.saving
            || self.persistence_uncertain
            || self.state_load == StateLoad::Loading
        {
            return false;
        }
        self.state_load = StateLoad::Loading;
        self.status = "Loading…".into();
        let started = spawn(self.tx.clone(), self.write_permit.clone());
        if !started {
            self.state_load = StateLoad::Failed("Could not start state loader");
            self.status = "Could not start state loader".into();
        }
        self.changed();
        started
    }

    // Construct state without starting the protected-state loader.
    fn unloaded(companion: super::ott_companion::Supervisor) -> Self {
        let (tx, rx) = mpsc::channel();
        let (metadata_tx, metadata_rx) = mpsc::channel();
        let (catalog_tx, catalog_rx) = mpsc::sync_channel(4);
        Self {
            settings_groups: settings_groups::State::default(),
            classic_navigation: classic_navigation::State::default(),
            auth_return: auth_return::State::default(),
            native_catalog: None,
            menu_settings: menu_settings::State::default(),
            desktop_input: desktop_input::State::default(),
            numeric_input: numeric_input::State::default(),
            sleep_timer: sleep_timer::State::default(),
            guide: None,
            organization: Box::default(),
            search: search::Search::default(),
            parental: parental_runtime::State::default(),
            saved: Saved::default(),
            reminders: reminders::State::default(),
            reminder_save: None,
            m3u_attachments: crate::app::ott_m3u_attachment::Authorities::default(),
            play_attachment: None,
            playing_attachment: None,
            metadata_attachment: None,
            transient: transient::State::default(),
            local_inventory: transient::Inventory::default(),
            write_permit: private::WritePermit::new(),
            clock_sample: None,
            classic_clock: Default::default(),
            osd_display: Default::default(),
            classic_preferences: Default::default(),
            classic_display: BTreeMap::new(),
            clock_anchor: None,
            wall_clock_ms: 0,
            catalog: Catalog::default(),
            catalog_diagnostics: Default::default(),
            preplay: Default::default(),
            playback_failure: Default::default(),
            explicit_step: Default::default(),
            native_playback_capture: Default::default(),
            page: "providers",
            provider: ProviderKind::M3u,
            revision: 1,
            generation: 0,
            catalog_revision: 0,
            selected: None,
            browse_focus: browse_focus::State::default(),
            classic_bookmark: classic_bookmark::State::default(),
            local_profile: local_profile::State::default(),
            profile_focus: None,
            profile_edit_epoch: 1,
            settings_focus: None,
            plex_omitted_servers: 0,
            reply: None,
            status: "Loading…".into(),
            saving: false,
            pending_profile_rename: None,
            playback_epoch: 0,
            initial_native_ticket: plx_media::route::play_request_ticket(),
            save_playback_epoch: 0,
            navigation_epoch: 0,
            save_navigation: SaveNavigation::Preserve,
            save_ui_owner: None,
            save_source_editor: None,
            subtitle_save: None,
            hls_tracks: Default::default(),
            public_preferences: public_preferences::State::default(),
            loaded: false,
            state_load: StateLoad::Idle,
            cancel: Arc::new(AtomicBool::new(false)),
            catalog_tx,
            catalog_rx,
            tx,
            rx,
            edit_profile: None,
            edit_profile_original: None,
            profile_edit_save_only: None,
            m3u_slot_draft: None,
            profile_edit_pending: None,
            edit_serial: 0,
            edit_session: None,
            exit_confirmation: Default::default(),
            pending_editor_save: None,
            native_controls: None,
            pending_category_choice: None,
            pending_playing_favorite: None,
            setup_serial: 0,
            setup: None,
            pending_setup_save: None,
            archive_hours_editor: None,
            m3u_media_editor: None,
            remote: Remote::from_install("", ""),
            play_generation: 0,
            resolver_busy: false,
            resolver_next: None,
            play_cancel: Arc::new(AtomicBool::new(false)),
            play_ticket: None,
            playing: None,
            failed_playback: None,
            pending_play: None,
            favorites: plx_data::ott::favorites_wire::Protocol::default(),
            favorites_recovery_done: Arc::new(AtomicBool::new(true)),
            remote_snapshot: Value::Null,
            catalog_loading: false,
            catalog_complete: false,
            catalog_return: None,
            catalog_dirty: false,
            catalog_warning: None,
            catalog_done: Arc::new(AtomicBool::new(true)),
            catalog_next: None,
            entropy: None,
            uuid_counter: 0,
            command_receipts: Box::default(),
            kiosk_capture: kiosk_capture::Capture::default(),
            verified_imports: Vec::new(),
            managed: managed::State::default(),
            classic_source: classic_source::State::default(),
            continuity: continuity::Continuity::new(0, None),
            continuity_retry_log: None,
            play_target: None,
            classic_play_handoff: Default::default(),
            play_profile: None,
            restore_play_generation: None,
            live_retry_generation: None,
            resume_floor: None,
            resume_pending: None,
            checkpoints: BTreeMap::new(),
            last_checkpoint_ms: 0,
            checkpoint_write_selection: None,
            checkpoint_save_epoch: None,
            persistence_uncertain: false,
            active_lease: None,
            quality: Default::default(),
            plex_refresh_pending: false,
            plex_refresh_requested: false,
            companion,
            companion_adapter: Some(companion_adapter::Adapter::default()),
            metadata_tx,
            metadata_rx,
            metadata_cancel: Arc::new(AtomicBool::new(false)),
            metadata_generation: 0,
            hosted_epg_config: None,
            metadata_target: None,
            metadata_query: None,
            metadata_guide: None,
            row_guide: None,
            row_guide_attempt: None,
            row_guide_coverage: None,
            metadata_playback_identity: None,
            programme_metadata_generation: None,
            programme_leaf: None,
            live_hud: live_hud::State::default(),
            metadata_local_selection: None,
            metadata_done: Arc::new(AtomicBool::new(true)),
            metadata_next: None,
            local_assets: local_assets::State::default(),
            favorite_preview: None,
            metadata_refresh_ms: 0,
            source_lifecycle: Box::default(),
        }
    }
    /// Calendar time supplied by the frame runner, separate from deadline time.
    pub(super) fn wall_now_s(&self) -> Option<i64> {
        self.clock_sample?;
        i64::try_from(self.wall_clock_ms / 1000).ok()
    }

    fn now_ms(&self) -> u64 {
        self.clock_sample
            .expect("OTT runtime receives a clock sample before dispatch")
            .unix_ms()
    }
    fn active(&self) -> Option<&ProviderProfile> {
        self.saved
            .profiles
            .profiles
            .iter()
            .find(|p| Some(&p.id) == self.saved.profiles.active_profile.as_ref())
    }
    // Browsing a provider must not edit another provider's currently playing profile.
    fn provider_profile(&self) -> Option<&ProviderProfile> {
        self.active()
            .filter(|profile| profile.enabled && profile.kind == self.provider)
            .or_else(|| {
                self.saved
                    .profiles
                    .profiles
                    .iter()
                    .find(|profile| profile.enabled && profile.kind == self.provider)
            })
    }
    /// Shared admission reason for mutations that depend on protected state.
    fn settings_write_refusal(&self) -> Option<&'static str> {
        if self.persistence_uncertain {
            return Some(
                "Settings write needs reconciliation; restart the player before another change",
            );
        }
        if let StateLoad::Failed(error) = self.state_load {
            return Some(error);
        }
        if !self.loaded {
            return Some(if self.state_load == StateLoad::Loading {
                "Protected OTT settings are still loading"
            } else {
                "Protected OTT settings are not loaded"
            });
        }
        self.saving
            .then_some("Another settings operation is in progress")
    }

    fn import_plex(
        &mut self,
        profile: super::ott_profile::Profile,
        invocation: Invocation,
    ) -> Result<(), &'static str> {
        if let Some(error) = self.settings_write_refusal() {
            return Err(error);
        }
        let tx = self.tx.clone();
        let started = plx_base::task::spawn_off_frame("ott-plex-verify", move |worker| {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                super::ott_import::verify_profile(worker, &profile)
            }))
            .unwrap_or(Err("Plex verification worker failed"));
            let _ = tx.send(Job::PlexVerified(invocation, result));
            plx_machine::idle::invalidate();
        });
        if !started {
            return Err("Could not start Plex verifier");
        }
        self.checkpoint_save_epoch = None;
        self.saving = true;
        self.save_navigation = SaveNavigation::Activate(self.navigation_epoch);
        self.save_playback_epoch = self.playback_epoch;
        self.save_ui_owner = None;
        self.save_source_editor = self.capture_source_editor();
        self.status = "Verifying Plex source…".into();
        self.changed();
        Ok(())
    }
    /// An accepted UI action or a visible partial navigation advances this
    /// stamp. Only a save begun by the accepted action acquires its new stamp.
    fn accepted_navigation(&mut self, started_save: bool) {
        auth_return::navigation(self);
        managed::navigation(&mut self.managed);
        browse_focus::request_flush(self);
        self.usb_navigation_exit();
        self.navigation_epoch = self.navigation_epoch.wrapping_add(1);
        self.rebase_organization_navigation();
        if started_save {
            self.save_ui_owner = self.reply;
            self.rebase_category_choice();
        }
        if started_save && matches!(self.save_navigation, SaveNavigation::Activate(_)) {
            self.save_navigation = SaveNavigation::Activate(self.navigation_epoch);
        }
    }
    // Unlike display revision, this stamp does not change with playback progress.
    // Retiring every published profile-image change prevents inactive A -> B -> A reuse.
    fn retire_profile_edits(&mut self) {
        auth_return::navigation(self);
        if self.profile_edit_epoch != 0 {
            self.profile_edit_epoch = self.profile_edit_epoch.checked_add(1).unwrap_or(0);
        }
    }
    fn observe_profile_edits(&mut self, next: &Saved) {
        if self.saved.profiles != next.profiles || m3u_slots::settings_changed(&self.saved, next) {
            self.retire_profile_edits();
        }
    }
    fn profile_edit_context(&self, profile: &ProviderProfile) -> String {
        plx_data::ott::favorites_wire::hash(
            json!([
                self.remote.runtime(),
                self.profile_edit_epoch,
                self.navigation_epoch,
                self.page,
                self.provider,
                profile
            ])
            .to_string()
            .as_bytes(),
        )
    }
    fn profile_edit_surface(&self, app: &App) -> Result<(), &'static str> {
        const CHANGED: &str = "This profile changed; reopen its setup before saving";
        if self.profile_edit_epoch == 0
            || self.page != "profiles"
            || !profile_edit::supported(self.provider)
            || self.reply.is_none()
            || self.reply != current_ui_owner(app)
            || app.pages.has_pending_navigation()
        {
            return Err(CHANGED);
        }
        if let Some(error) = self.settings_write_refusal() {
            return Err(error);
        }
        Ok(())
    }
    fn profile_edit_target(
        &self,
        app: &App,
        context: &str,
        target: &str,
    ) -> Result<ProviderProfile, &'static str> {
        const CHANGED: &str = "This profile changed; reopen its setup before saving";
        self.profile_edit_surface(app)?;
        let mut matches = self
            .saved
            .profiles
            .profiles
            .iter()
            .filter(|p| p.id == target);
        let profile = matches.next().ok_or(CHANGED)?;
        if matches.next().is_some()
            || !profile.enabled
            || profile.kind != self.provider
            || managed::shadows_profile(self, profile)
            || context != self.profile_edit_context(profile)
        {
            return Err(CHANGED);
        }
        if let Some(error) = self.settings_write_refusal() {
            return Err(error);
        }
        Ok(profile.clone())
    }
    fn sync_saved_context(&mut self) {
        self.observe_m3u_attachments();
        self.parental_confirmed();
        self.provider = classic_source::profile(self)
            .or_else(|| self.active())
            .map(|p| p.kind)
            .unwrap_or_else(|| {
                match self
                    .saved
                    .settings
                    .get("selectedProvider")
                    .and_then(Value::as_str)
                {
                    Some("vportal") => ProviderKind::VPortal,
                    Some("local" | "offline") => ProviderKind::Local,
                    Some("plex") => ProviderKind::Plex,
                    Some("xtream") => ProviderKind::Xtream,
                    Some("edem" | "ilook" | "ilooktv") => ProviderKind::Edem,
                    Some("stalker") => ProviderKind::Stalker,
                    Some(id) => ProviderKind::from_id(id).unwrap_or(ProviderKind::M3u),
                    _ => ProviderKind::M3u,
                }
            });
        self.observe_organization_context();
        browse_focus::reconcile(self);
    }
    fn changed(&mut self) {
        self.reconcile_search();
        self.revision = self.revision.wrapping_add(1);
        plx_machine::idle::invalidate();
    }
    fn cancel_catalog(&mut self) {
        self.catalog_diagnostics.clear();
        self.catalog_return = None;
        self.usb_retire_browser();
        guide_ui::close(self);
        self.catalog_next = None;
        self.catalog_dirty = false;
        self.catalog_warning = None;
        self.metadata_next = None;
        self.favorite_preview = None;
        self.metadata_refresh_ms = 0;
        self.resume_pending = None;
        self.metadata_cancel.store(true, Ordering::Relaxed);
        self.metadata_generation = self.metadata_generation.wrapping_add(1);
        self.metadata_target = None;
        self.metadata_query = None;
        self.metadata_guide = None;
        self.row_guide = None;
        self.row_guide_attempt = None;
        self.row_guide_coverage = None;
        self.metadata_local_selection = None;
        self.metadata_attachment = None;
        self.cancel.store(true, Ordering::Relaxed);
        if matches!(self.local_inventory.phase,
            transient::InventoryPhase::Loading(generation) if generation == self.generation)
        {
            self.local_inventory.phase = transient::InventoryPhase::Unloaded;
        }
        self.continuity.stop(self.generation);
        self.generation = self.generation.wrapping_add(1);
        self.catalog_diagnostics.reset_performance(self.generation);
        self.catalog_loading = false;
        self.catalog_complete = false;
    }
    fn selection_complete(&self) -> bool {
        self.catalog_complete
            || (!kiosk_source::strict_folder_traversal(self) && self.view_selection().1)
    }
    fn resume_key(&self) -> Option<String> {
        if managed::profile(self).is_some() || classic_source::binding_profile(self).is_some() {
            return None;
        }
        if self.saved.favorites_selected {
            Some("favorites".into())
        } else {
            classic_source::profile(self)
                .or_else(|| self.active())
                .map(|p| p.id.clone())
        }
    }
    fn ordered_ids(&self) -> Vec<String> {
        if kiosk_source::strict_folder_traversal(self) {
            return self
                .organization_rows("")
                .into_iter()
                .map(|(_, entry)| entry.id.clone())
                .collect();
        }
        let (_, favorites) = self.view_selection();
        let category = self.effective_category();
        if favorites {
            self.saved
                .favorites
                .entries()
                .iter()
                .map(|f| format!("favorite:{}", f.identity()))
                .collect()
        } else {
            self.organization_rows(category)
                .into_iter()
                .map(|(_, entry)| entry.id.clone())
                .collect()
        }
    }
    /// A Favorite may already be playing when strict folder policy is enabled.
    /// Locate it by the held Local owner without depending on document membership
    /// or changing the installed ID. Successors still use ordinary folder IDs.
    fn strict_folder_anchor(&self, playing: &Playing) -> Option<String> {
        if !playing.selection_id.starts_with("favorite:")
            || !kiosk_source::strict_folder_traversal(self)
        {
            return None;
        }
        self.folder_row_for(playing)
    }
    /// Root files use the current held folder's display name, without changing
    /// catalogue categories or deriving a label from a path or profile alias.
    fn local_playing_collection_label(&self, playing: &Playing) -> Option<&str> {
        if playing.profile.kind != ProviderKind::Local {
            return None;
        }
        let MediaLocator::Local { relative_path } = &playing.entry.media else {
            return None;
        };
        if !plx_data::ott::valid_local_relative_path(relative_path) || relative_path.contains('/') {
            return None;
        }
        let selection = playing
            .local_selection
            .clone()
            .or_else(|| self.local_profile_selection(&playing.profile))?;
        let root = self.current_local_root(&selection)?;
        if root.canonical_path.as_deref() != Some(playing.profile.source.as_str())
            || !plx_data::ott::valid_local_source_label(&root.name)
        {
            return None;
        }
        Some(root.name.as_str())
    }
    fn load_catalog(&mut self) {
        self.load_catalog_profile(self.active().cloned(), false);
    }
    /// Start the existing loader for the published temporary owner, without
    /// selecting or saving a numbered profile or acquiring playback intent.
    /// The caller must first restore the managed State into this Runtime.
    pub(super) fn managed_catalog(&mut self) -> bool {
        let Some(profile) = managed::profile(self).cloned() else {
            return false;
        };
        self.load_catalog_profile(Some(profile), true);
        true
    }
    fn load_catalog_profile(&mut self, profile: Option<ProviderProfile>, temporary: bool) {
        let previous_generation = self.generation;
        if !temporary {
            classic_source::release(self);
        }
        transient::discard(self);
        self.cancel_catalog();
        if temporary {
            managed::rebase_catalog(self, previous_generation);
            classic_source::rebase(self, previous_generation);
        }
        self.cancel = Arc::new(AtomicBool::new(false));
        self.catalog = Catalog::default();
        self.catalog_revision = self.catalog_revision.wrapping_add(1);
        let resume = (!temporary)
            .then(|| self.resume_key())
            .flatten()
            .and_then(|key| {
                self.checkpoints
                    .get(&key)
                    .or_else(|| self.saved.resume.get(&key))
            })
            .filter(|value| self.resume_is_current(value) || self.resume_waits_for_bindings(value))
            .map(|value| continuity::ResumePoint {
                entry_id: value.entry_id.clone(),
                position_ms: value.position_ms,
            })
            .or_else(|| {
                (!temporary && self.resume_key().is_some())
                    .then(|| commands::offline_selected_resume(&self.saved))
                    .flatten()
            });
        self.continuity = continuity::Continuity::new(self.generation, resume);
        if temporary {
            self.continuity.suppress_unrequested_start();
        }
        self.catalog_loading = false;
        if let Some(profile) = profile {
            self.provider = profile.kind;
            // Keep retirement above this guard: a cleared active slot must not
            // publish a late batch from its previous source or start a new one.
            if profile.source.is_empty() {
                self.page = if self.saved.favorites_selected {
                    "favorites"
                } else {
                    "profiles"
                };
                self.continuity.suppress_unrequested_start();
                self.catalog_complete = true;
                self.status.clear();
                self.changed();
                return;
            }
            self.page = if self.saved.favorites_selected {
                "favorites"
            } else {
                "channels"
            };
            self.catalog_dirty = true;
            if profile.kind == ProviderKind::M3u
                && plx_data::ott::m3u_source_kind(&profile.source)
                    == Ok(plx_data::ott::M3uSourceKind::PortalOnly)
            {
                // Media-only M3U parents have no television catalogue. The
                // existing Favorite attachment resolver fetches media lazily.
                self.catalog_complete = true;
                self.status.clear();
            } else {
                self.status = "Loading…".into();
                self.catalog_loading = true;
                self.catalog_next = Some(profile);
                self.launch_catalog();
            }
        }
        self.changed();
    }
    /// Refresh listings without converting an asynchronous completion into a
    /// playback command. In-flight resolvers keep their independent generation.
    fn refresh_catalog_preserving_playback(&mut self) {
        self.refresh_catalog_preserving_playback_impl(true);
    }
    fn refresh_catalog_preserving_playback_impl(&mut self, preserve_managed: bool) {
        if self.transient.accepted.is_some() || self.transient.candidate.is_some() {
            return;
        }
        let page = self.page;
        let provider = self.provider;
        let mut continuity = std::mem::replace(
            &mut self.continuity,
            continuity::Continuity::new(self.generation, None),
        );
        let resume_pending = self.resume_pending.take();
        let retry = self.continuity_retry_log.take();
        if !preserve_managed || (!self.managed_catalog() && !classic_source::refresh(self)) {
            self.load_catalog();
        }
        self.page = page;
        self.provider = provider;
        continuity.rebase(self.generation);
        self.continuity = continuity;
        self.resume_pending = resume_pending;
        if let Some(retry) = retry {
            retry.emit(
                continuity_diagnostics::Event::Cancelled,
                continuity_diagnostics::Reason::Changed,
                self.now_ms(),
                self.generation,
            );
        }
    }
    fn take_catalog_task(&mut self) -> Option<ott_catalog::Task> {
        self.take_catalog_task_checked(None)
    }
    fn take_catalog_task_checked(&mut self, app: Option<&App>) -> Option<ott_catalog::Task> {
        if !self.catalog_work_allowed()
            || self.usb_remote_waiting_result()
            || !self.catalog_done.load(Ordering::Acquire)
        {
            return None;
        }
        // Independent USB RPC and selected-root restoration run before any
        // managed-provider capture, without borrowing its source authority.
        if let Some(task) = self.take_usb_remote_task() {
            return Some(task);
        }
        if let Some(task) = self.take_initial_usb_catalog_task() {
            return task;
        }
        self.take_usb_task()
            .or_else(|| transient::take_binding_task(self))
            .or_else(|| {
                if !self.usb_catalog_allowed()
                    || !self.catalog_next.as_ref().is_some_and(|profile| {
                        transient::lifecycle_target_ready(self, profile, None)
                    })
                {
                    return None;
                }
                let mut classic_read = None;
                if let Some(profile) = self
                    .catalog_next
                    .as_ref()
                    .filter(|profile| classic_source::owns_id(self, &profile.id))
                    .cloned()
                {
                    let app = app?;
                    match classic_source::capture_read(self, app, &profile) {
                        Ok(read) => classic_read = read,
                        Err(_) => return None,
                    }
                }
                let mut managed_read = None;
                if let Some(profile) = self
                    .catalog_next
                    .as_ref()
                    .filter(|profile| managed::owns_retained_profile(self, profile))
                    .cloned()
                {
                    let app = app?;
                    match managed::capture_read(self, app, &profile) {
                        Ok(Some(read)) => {
                            managed::record_catalog(self, &profile, &read);
                            managed_read = Some(read);
                        }
                        _ => {
                            self.cancel.store(true, Ordering::Release);
                            self.catalog_next = None;
                            self.catalog_loading = false;
                            self.status = "Plex source is no longer available".into();
                            self.catalog_diagnostics.failed(
                                self.generation,
                                catalog_diagnostics::Failure::SourceUnavailable,
                            );
                            self.changed();
                            return None;
                        }
                    }
                }
                self.catalog_next.take().and_then(|profile| {
                    match self.local_catalog_task(profile) {
                        Ok(task) => Some(match (task, managed_read, classic_read) {
                            (ott_catalog::Task::Provider(profile), Some(read), _) => {
                                ott_catalog::Task::ManagedPlex { profile, read }
                            }
                            (ott_catalog::Task::Provider(profile), _, Some(read)) => {
                                ott_catalog::Task::ClassicPlex { profile, read }
                            }
                            (task, _, _) => task,
                        }),
                        Err(error) => {
                            transient::launch_failed(self);
                            self.status = error.into();
                            self.catalog_loading = false;
                            self.catalog_diagnostics.failed(
                                self.generation,
                                catalog_diagnostics::Failure::CatalogFailed,
                            );
                            self.catalog_dirty = true;
                            self.changed();
                            None
                        }
                    }
                })
            })
    }
    fn launch_catalog(&mut self) {
        self.launch_catalog_checked(None);
    }
    fn launch_catalog_from_app(&mut self, app: &App) {
        self.launch_catalog_checked(Some(app));
    }
    fn launch_catalog_checked(&mut self, app: Option<&App>) {
        let task = match app {
            Some(app) => self.take_catalog_task_checked(Some(app)),
            None => self.take_catalog_task(),
        };
        let Some(task) = task else {
            return;
        };
        let generation = self.generation;
        if !ott_catalog::start(
            task,
            generation,
            self.cancel.clone(),
            self.catalog_tx.clone(),
            self.catalog_done.clone(),
        ) {
            if self.usb_remote_launch_failed() {
                return;
            }
            self.usb_launch_failed();
            transient::launch_failed(self);
            self.status = "Could not start catalogue loader".into();
            self.catalog_loading = false;
            // USB failure cleanup may restore a different catalogue generation.
            // This refusal belongs only to the task whose start was attempted.
            self.catalog_diagnostics
                .failed(generation, catalog_diagnostics::Failure::LoaderUnavailable);
            self.catalog_dirty = true;
            self.changed();
        }
    }
    fn save(
        &mut self,
        candidate: Saved,
        invocation: Option<Invocation>,
        reload: bool,
    ) -> Result<(), &'static str> {
        self.save_with_response(candidate, invocation, reload, ok(json!({"persisted":true})))
    }
    fn save_reply(
        &mut self,
        candidate: Saved,
        invocation: Invocation,
        reload: bool,
        response: Value,
    ) -> Result<(), &'static str> {
        self.save_with_response(candidate, Some(invocation), reload, response)
    }
    fn save_with_response(
        &mut self,
        candidate: Saved,
        invocation: Option<Invocation>,
        reload: bool,
        response: Value,
    ) -> Result<(), &'static str> {
        self.start_save(candidate, invocation, reload, response, None)
    }
    fn save_public_preference(
        &mut self,
        candidate: Saved,
        patch: public_preferences::Patch,
        invocation: Option<Invocation>,
        response: Value,
    ) -> Result<(), &'static str> {
        public_preferences::validate_candidate(&self.saved, &candidate, &patch)?;
        self.start_save(candidate, invocation, false, response, Some(patch))
    }
    fn start_save(
        &mut self,
        mut candidate: Saved,
        invocation: Option<Invocation>,
        reload: bool,
        response: Value,
        public_patch: Option<public_preferences::Patch>,
    ) -> Result<(), &'static str> {
        let mut profile_rename = None;
        if public_patch.is_some() {
            if let Some(error) = public_preferences::write_refusal(self) {
                return Err(error);
            }
        } else {
            if let Some(error) = self.settings_write_refusal() {
                return Err(error);
            }
            if self
                .saved
                .receipts
                .iter()
                .any(|receipt| receipt.outcome == plx_data::ott::favorites_wire::Outcome::Unknown)
            {
                return Err("A Favorites operation needs reconciliation");
            }
            profile_rename = profile_rename::prepare(self, &mut candidate)?;
            category_state::prepare_profile_change(&self.saved, &mut candidate)?;
            if profile_rename.is_none() {
                browse_focus::prepare(self, &mut candidate)?;
            }
            // Forget proof with an explicitly removed or replaced descriptor. Live
            // mount identity refusal is supplied by the existing Local source owner.
            let previous_roots =
                super::ott_local_sources::RememberedFolders::read(&self.saved.settings)?;
            let candidate_roots =
                super::ott_local_sources::RememberedFolders::read(&candidate.settings)?;
            for previous in previous_roots.folders() {
                if !candidate_roots
                    .folders()
                    .iter()
                    .any(|root| root == previous)
                {
                    candidate
                        .local_favorite_proofs
                        .invalidate_root(&previous.id);
                }
            }
            transient::revoke_unavailable_proofs(self, &mut candidate);
            let kiosk = super::ott_kiosk_stored::read(&candidate.settings)
                .map_err(|_| "Invalid stored kiosk policy")?;
            candidate.local_favorite_proofs.prune_with_pins(
                &candidate.favorites,
                kiosk.as_ref().map_or(&[], |pin| pin.pins()),
            );
            candidate
                .local_favorite_proofs
                .reconcile_profiles(&candidate.profiles.profiles);
            if profile_rename.is_none() {
                kiosk_resume::merge(&mut candidate, &self.checkpoints);
            }
            self.usb_preserve_checkpoint(&mut candidate);
            self.usb_rebase_saved(&mut candidate)?;
            kiosk_resume::sanitize(&mut candidate);
            self.prune_m3u_attachments(&mut candidate)?;
            candidate.validate()?;
        }
        // Explicit reselection is an intent even when the durable image is
        // unchanged (notably a provider with no configured profile).
        let activates = public_patch.is_none()
            && (reload
                || invocation.as_ref().is_some_and(|request| {
                    matches!(request.action.as_str(), "provider" | "profile")
                })
                || candidate.profiles.active_profile != self.saved.profiles.active_profile
                || candidate.settings.get("selectedProvider")
                    != self.saved.settings.get("selectedProvider"));
        let navigation = if activates {
            SaveNavigation::Activate(self.navigation_epoch)
        } else {
            SaveNavigation::Preserve
        };
        let source_editor = if activates && invocation.is_some() {
            self.capture_source_editor()
        } else {
            None
        };
        let sender = self.tx.clone();
        let before = self.saved.clone();
        let write_permit = self.write_permit.clone();
        let deadline = invocation.as_ref().map(|request| {
            self.clock_sample
                .expect("clock sampled before save")
                .deadline_at_unix_ms(request.deadline_ms)
        });
        let started = plx_base::task::spawn_off_frame("ott-save", move |off_frame| {
            if let Some(patch) = public_patch {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if deadline.is_some_and(|deadline| deadline.expired(off_frame)) {
                        return Err(public_preferences::WriteFailure::Expired);
                    }
                    public_preferences::write(patch, &write_permit)
                }))
                .unwrap_or(Err(public_preferences::WriteFailure::Uncertain));
                let _ = sender.send(Job::PublicSaved(result, invocation, response));
                plx_machine::idle::invalidate();
                return;
            }
            let panic_invocation = invocation.clone();
            let job = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let result = favorite_item_ids::write_prepared_with(
                    &mut candidate,
                    || {
                        write_permit.is_valid()
                            && !deadline.is_some_and(|deadline| deadline.expired(off_frame))
                    },
                    |state| favorite_item_ids::prepare(state, off_frame),
                    |state| favorites::write_verified(state, &write_permit),
                );
                let expired = deadline.is_some_and(|deadline| deadline.expired(off_frame));
                match result {
                    Ok(()) => Job::Saved(Ok(candidate), invocation, reload, response),
                    Err(favorites::WriteFailure::NotApplied) => Job::Saved(
                        Err(if expired {
                            "Settings request expired"
                        } else {
                            "Could not prepare protected OTT settings for saving"
                        }),
                        invocation,
                        reload,
                        response,
                    ),
                    Err(favorites::WriteFailure::NotAppliedStorage(error)) => {
                        if expired {
                            Job::Saved(
                                Err("Settings request expired"),
                                invocation,
                                reload,
                                response,
                            )
                        } else {
                            Job::SaveRejected(error, invocation, reload, response)
                        }
                    }
                    Err(favorites::WriteFailure::Unknown) => {
                        let visible = private::load(Namespace::OttState).ok().flatten();
                        let after = matches!(
                            favorites::classify_readback(&before, &candidate, visible.as_ref()),
                            Ok(favorites::Readback::After)
                        );
                        Job::Uncertain(after.then_some(candidate), invocation)
                    }
                }
            }))
            .unwrap_or(Job::Uncertain(None, panic_invocation));
            let _ = sender.send(job);
            plx_machine::idle::invalidate();
        });
        if !started {
            return Err("Could not start settings writer");
        }
        self.checkpoint_save_epoch = None;
        self.saving = true;
        self.pending_profile_rename = profile_rename;
        self.save_playback_epoch = self.playback_epoch;
        self.save_navigation = navigation;
        self.save_ui_owner = None;
        self.save_source_editor = source_editor;
        self.status = "Saving…".into();
        self.changed();
        Ok(())
    }
    fn refresh_plex_sources(&mut self) {
        if !self.source_work_allowed()
            || !self.plex_refresh_requested
            || self.plex_refresh_pending
            || !self.loaded
        {
            return;
        }
        self.plex_refresh_requested = false;
        let Ok(sources) = self.saved.plex_sources() else {
            return;
        };
        if sources.is_empty() {
            return;
        }
        let tx = self.tx.clone();
        let cancel = self.begin_plex_restore();
        self.plex_refresh_pending =
            plx_base::task::spawn_off_frame("ott-plex-restore", move |worker| {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    super::ott_import::restore_verified_sources(worker, sources, &cancel)
                }))
                .unwrap_or_else(|_| vec![Err("Plex restoration worker failed")]);
                let _ = tx.send(Job::PlexRestored(cancel, result));
                plx_machine::idle::invalidate();
            });
        if !self.plex_refresh_pending {
            self.status = "Could not restore Plex sources".into();
            self.changed();
        }
    }
    fn suspend(&mut self, app: &mut App) {
        self.suspend_with_retry_reason(app, continuity_diagnostics::Reason::Suspend);
    }
    fn suspend_with_retry_reason(&mut self, app: &mut App, reason: continuity_diagnostics::Reason) {
        self.remote.suspend_notifications();
        self.failed_playback = None;
        // Reset at the actual lifecycle edge, even if no frame is rendered
        // between background/Standby and the next foreground input.
        self.sleep_timer = sleep_timer::State::default();
        self.retire_parental_source();
        let preparation_preserved = self.preserve_kiosk_preparation(app);
        self.preserve_admitted_resume(app);
        guide_ui::close(self);
        self.poll_reminders(app, false);
        companion_adapter::suspend(self, app);
        self.checkpoint(app, true);
        if !preparation_preserved && self.resume_pending.is_none() {
            self.resume_pending = self.play_target.clone();
        }
        self.cancel_play_request();
        self.suspend_source_work(app);
        self.continuity.suspend(self.generation);
        self.log_retry_cancelled(reason);
        // Managed server streams expire when their lease ends. Resolve a fresh one
        // after wake instead of asking the native lifecycle to reuse a stopped URL.
        if self.active_lease.is_some() {
            self.release_active_lease();
            if let Some(playing) = self.playing.take() {
                let position_ms =
                    transient::bookmark_position(self, &playing).unwrap_or_else(|| {
                        playback::intended_pos(&app.player.session).max(0) as u64 / 1_000_000
                    });
                self.resume_pending = Some(PlaybackRequest {
                    kiosk_selection: playing.kiosk_selection,
                    archive: playing.archive,
                    continuation: playing.identity,
                    transient_owner: playing.transient_owner,
                    local_selection: playing.local_selection,
                    id: playing.selection_id,
                    position_ms,
                    advance_on_failure: playing.advance_on_failure,
                });
            }
            plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
            plx_media::route::cancel_play(&mut app.player.session);
        }
        self.changed();
    }
    /// Retire catalogue ownership without touching a replacement native route.
    fn retire_catalog_playback(&mut self, app: &mut App) {
        self.failed_playback = None;
        transient::cancel_play_candidate(self, app);
        transient::note_stop(self);
        self.playback_epoch = self.playback_epoch.wrapping_add(1);
        commands::cancel_pending_play(self);
        self.cancel_play_request();
        self.resume_pending = None;
        self.continuity.stop(self.generation);
        self.release_active_lease();
        self.playing = None;
        self.resume_floor = None;
        self.changed();
    }
    fn stop_catalog(&mut self, app: &mut App) {
        self.checkpoint(app, true);
        self.retire_catalog_playback(app);
        self.log_retry_cancelled(continuity_diagnostics::Reason::Changed);
    }
    fn consume_exit(&mut self, app: &mut App) {
        if let Some(exit) = app.player.pending_exit.take() {
            browse_focus::request_flush(self);
            app.player_geometry
                .clear_exit(exit.playback_identity.as_ref());
            self.checkpoint_at(app, true, Some(&exit));
            self.retire_catalog_playback(app);
            let reason = match exit.cause {
                plx_media::player::machine::ExitCause::RequestedExit => {
                    continuity_diagnostics::Reason::RequestedExit
                }
                plx_media::player::machine::ExitCause::Other => {
                    continuity_diagnostics::Reason::Changed
                }
            };
            self.log_retry_cancelled(reason);
            companion_adapter::exit(self);
        }
    }
    /// Accepted viewer transport commits the currently playing fallback. This
    /// is called by remote and native performers, never by internal clock holds.
    fn viewer_transport(&mut self, app: &App) {
        self.viewer_transport_intent(app, false);
    }
    fn viewer_seek(&mut self, app: &App) {
        self.viewer_transport_intent(app, true);
    }
    fn viewer_transport_intent(&mut self, app: &App, seek: bool) {
        if !playback_allowed(app) || app.player.pending_exit.is_some() {
            return;
        }
        let restoring = self.restore_play_generation == Some(self.play_generation);
        if self.play_target.is_some() && !restoring {
            return;
        }
        let Some(playing) = self
            .playing
            .as_ref()
            .filter(|playing| playing_matches(playing, app))
        else {
            return;
        };
        // A native Plex selection can be resolving while the outgoing source
        // remains installed. Transport on that old source must not reclaim it.
        if playing_superseded(playing, app) {
            return;
        }
        // The performer supplies accepted viewer intent, not an inference from
        // the native seek mailbox: internal speed/reload work uses that too.
        if seek {
            self.resume_floor = None;
        }
        let id = playing.selection_id.clone();
        self.playback_epoch = self.playback_epoch.wrapping_add(1);
        if restoring {
            // The scan may already have admitted B before input was dispatched.
            // Fence both its queued resolver and an eventual worker completion.
            self.cancel_play_request();
            self.selected = Some(id.clone());
            self.status.clear();
        }
        // Propagate accepted viewer intent; OS suspend/resume never calls this.
        transient::note_pause(self, lifecycle::viewer_paused());
        self.continuity.user_selected(self.generation, &id);
        if lifecycle::viewer_paused() {
            self.continuity.pause(self.generation);
        }
        self.changed();
    }
    fn reconcile_owner(&mut self, app: &mut App) {
        if self.failed_playback.is_some() && failed_playback::current(self, app).is_none() {
            self.failed_playback = None;
            self.playback_failure.clear();
        }
        let identity = plx_media::route::playback_identity(&app.player.session);
        self.playback_failure.reconcile_owner(identity.as_ref());
        // An explicit OTT selection made after the retained native request is
        // now the pending owner. Its admission fence lasts only for the resolve;
        // an automatic restore never acquires this priority.
        if self.play_target.is_some()
            && self.restore_play_generation.is_none()
            && self
                .play_ticket
                .is_some_and(plx_media::route::play_request_is_current)
        {
            return;
        }
        let Some(playing) = self.playing.as_ref() else {
            return;
        };
        if playing_superseded(playing, app) {
            self.stop_catalog(app);
        }
    }
    fn reconcile_failure(&mut self, app: &mut App) {
        if self.current_kiosk_pin_owned(app) && !self.archive_playback_owned(app) {
            return;
        }
        if self.transient.owns_playback() {
            return;
        }
        if !playback_allowed(app)
            || self.play_target.is_some()
            || plx_media::player::state(&app.player.session)
                != plx_media::player::PlaybackState::Error
        {
            return;
        }
        let Some((current, advance_on_failure, live)) = self
            .playing
            .as_ref()
            .filter(|playing| playing_matches(playing, app))
            .map(|playing| (playing.selection_id.clone(), playing.advance_on_failure,
                playing.archive.is_none() && guide_ui::is_live(&playing.profile, &playing.entry)))
        else {
            return;
        };
        let kind = plx_media::player::error_now(&app.player.session).kind;
        let details = plx_media::player::report::failure_details_now(
            &app.player.session,
            &mut app.adapters.player,
        );
        self.playback_failure.capture(
            self.playing
                .as_ref()
                .and_then(|playing| playing.identity.as_ref()),
            kind,
            details,
        );
        let live_retry = live && !lifecycle::viewer_paused()
            && recoverable_live_transport(details);
        self.failed_playback = failed_playback::capture(self, app);
        let plex_failure = transient::plex_remote::failure_event(self, app);
        self.checkpoint(app, false);
        self.playing = None;
        self.resume_floor = None;
        self.release_active_lease();
        plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
        if let Some(page) = bridge::player_mut(&mut app.pages) {
            page.transport_reset(false);
        }
        plx_media::route::cancel_play(&mut app.player.session);
        self.status = "Native playback failed".into();
        if live_retry && !plex_failure {
            // Only attributed transient transport evidence may reopen a live
            // channel. This bounded path never applies next-item failure policy.
            let decision = self.continuity.live_disconnected(self.generation, &current, self.now_ms());
            if self.continuity.pending_retry().is_some() {
                self.status = "Live stream interrupted; retrying the same channel".into();
            }
            self.apply_continuity(decision);
            self.changed();
            return;
        }
        if plex_failure || !advance_on_failure {
            self.continuity.stop(self.generation);
            self.changed();
            return;
        }
        let ids = self.ordered_ids();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let decision = self.continuity_failed(&current, &refs, self.selection_complete(), None);
        self.apply_continuity(decision);
        self.changed();
    }
    fn log_retry_cancelled(&mut self, reason: continuity_diagnostics::Reason) {
        if let Some(pending) = self.continuity_retry_log.take() {
            pending.emit(
                continuity_diagnostics::Event::Cancelled,
                reason,
                self.now_ms(),
                self.generation,
            );
        }
    }
    // Covers source/selection cancellation paths which do not use the explicit lifecycle wrappers.
    // No string allocation or catalogue scan while the same cooldown remains pending.
    fn reconcile_retry_log(&mut self) {
        if self.continuity_retry_log.is_some_and(|old| {
            !self.continuity.pending_retry().is_some_and(|now| {
                old.catalog_generation == now.generation
                    && old.failures == now.failures
                    && old.not_before_ms == now.not_before_ms
            })
        }) {
            self.log_retry_cancelled(continuity_diagnostics::Reason::Changed);
        }
    }
    fn continuity_failed(
        &mut self,
        id: &str,
        ids: &[&str],
        complete: bool,
        failed_play_generation: Option<u64>,
    ) -> continuity::Decision {
        self.reconcile_retry_log();
        let decision =
            self.continuity
                .start_failed(self.generation, id, ids, complete, self.now_ms());
        if let Some(pending) = self.continuity.pending_retry() {
            let record = continuity_diagnostics::Pending {
                catalog_generation: pending.generation,
                failed_play_generation,
                row_ordinal: ids
                    .iter()
                    .position(|id| *id == pending.after)
                    .map_or(0, |index| index + 1),
                failures: pending.failures,
                not_before_ms: pending.not_before_ms,
            };
            // A duplicate failure may arrive after the catalogue order changed. The
            // unchanged cooldown retains its original row and failed-attempt tuple.
            if !self.continuity_retry_log.is_some_and(|old| {
                old.catalog_generation == record.catalog_generation
                    && old.failures == record.failures
                    && old.not_before_ms == record.not_before_ms
            }) {
                self.log_retry_cancelled(continuity_diagnostics::Reason::Changed);
                record.emit(
                    continuity_diagnostics::Event::Pending,
                    continuity_diagnostics::Reason::None,
                    self.now_ms(),
                    self.generation,
                );
                self.continuity_retry_log = Some(record);
            }
        } else {
            self.reconcile_retry_log();
        }
        decision
    }
    fn retry_continuity(
        &mut self,
        now_ms: u64,
        ids: &[&str],
        complete: bool,
    ) -> continuity::Decision {
        let retry = self.continuity_retry_log.take();
        let decision = self.continuity.tick(self.generation, now_ms, ids, complete);
        if matches!(decision, continuity::Decision::Wait)
            && self.continuity.pending_retry().is_some()
        {
            self.continuity_retry_log = retry;
            return decision;
        }
        if let Some(retry) = retry {
            let (event, reason) = if matches!(decision, continuity::Decision::Start { .. }) {
                (
                    continuity_diagnostics::Event::RetryStart,
                    continuity_diagnostics::Reason::None,
                )
            } else if matches!(decision, continuity::Decision::Wait) {
                // The deadline was consumed, but an incomplete catalogue still
                // owns the continuation. A later batch may supply its successor.
                (
                    continuity_diagnostics::Event::DiscoveryWait,
                    continuity_diagnostics::Reason::None,
                )
            } else {
                (
                    continuity_diagnostics::Event::Cancelled,
                    continuity_diagnostics::Reason::Exhausted,
                )
            };
            retry.emit(event, reason, now_ms, self.generation);
        }
        decision
    }
    fn apply_continuity(&mut self, mut decision: continuity::Decision) {
        // Admission failures skip an item once; no tight retry of a bad source.
        let ids = self.ordered_ids();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        for _ in 0..=ids.len() {
            let continuity::Decision::Start {
                entry_id,
                position_ms,
                reason,
            } = decision
            else {
                break;
            };
            let restoring_live = reason == continuity::Reason::Restore
                && self.effective_profile().is_some_and(|profile|
                    self.catalog.entries.iter().any(|entry|
                        entry.id == entry_id && guide_ui::is_live(profile, entry)));
            let result = match transient::lifecycle_resume_ready(
                self,
                &PlaybackRequest::ordinary(&entry_id, position_ms),
            ) {
                Ok(false) => {
                    self.continuity
                        .defer_start(self.generation, &entry_id, position_ms, reason);
                    break;
                }
                Ok(true) => self.play_at(&entry_id, position_ms),
                Err(error) => Err(error),
            };
            match result {
                Ok(()) => {
                    self.restore_play_generation =
                        (reason == continuity::Reason::Restore).then_some(self.play_generation);
                    self.live_retry_generation =
                        (reason == continuity::Reason::LiveRetry).then_some(self.play_generation);
                    break;
                }
                Err(error) => {
                    // A synchronous refusal may leave a queued target behind.
                    // Retire it before waiting, so standby cannot save the failed
                    // attempt as a new playback request to retry on wake.
                    let failed_generation = self
                        .play_target
                        .as_ref()
                        .filter(|target| target.id == entry_id)
                        .map(|_| self.play_generation);
                    self.cancel_play_request();
                    self.status = error.into();
                    self.changed();
                    if reason == continuity::Reason::LiveRetry || restoring_live {
                        // A stale/missing/rejected source is not permission to
                        // select another channel, even on synchronous refusal.
                        self.continuity.stop(self.generation);
                        self.log_retry_cancelled(continuity_diagnostics::Reason::Changed);
                        break;
                    }
                    decision = self.continuity_failed(
                        &entry_id,
                        &refs,
                        self.selection_complete(),
                        failed_generation,
                    );
                }
            }
        }
    }
    fn invalidate_source(&mut self, app: &mut App) {
        transient::cancel_play_candidate(self, app);
        transient::discard(self);
        self.cancel_play_request();
        self.cancel_catalog();
        self.catalog = Catalog::default();
        self.catalog_revision = self.catalog_revision.wrapping_add(1);
        self.release_active_lease();
        if self.playing.take().is_some() {
            plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
            plx_media::route::cancel_play(&mut app.player.session);
        }
    }
    fn resume_is_current(&self, value: &SavedResume) -> bool {
        if let Some(id) = value.entry_id.strip_prefix("favorite:") {
            let Some(favorite) = self
                .saved
                .favorites
                .entries()
                .iter()
                .find(|f| f.identity() == id)
            else {
                return false;
            };
            let Ok(bindings) = self.favorite_bindings(favorite) else {
                return false;
            };
            return match selection::bind_bound(&bindings, favorite) {
                Ok(selection::SelectionOwner::Profile(profile)) => {
                    profile_fingerprint(profile) == value.source_fingerprint
                        && self.favorite_permit(profile, favorite).is_ok()
                }
                Ok(selection::SelectionOwner::LocalRoot(root)) => transient::source_profile(root)
                    .is_ok_and(|profile| {
                        profile_fingerprint(&profile) == value.source_fingerprint
                            && self.favorite_permit(&profile, favorite).is_ok()
                    }),
                Ok(selection::SelectionOwner::M3uAttachment(receipt)) => receipt
                    .effective_profile()
                    .is_ok_and(|profile| profile_fingerprint(&profile) == value.source_fingerprint),
                Err(_) => false,
            };
        }
        classic_source::profile(self)
            .or_else(|| self.active())
            .is_some_and(|profile| profile_fingerprint(profile) == value.source_fingerprint)
    }
    fn numeric_write_ready(&self) -> bool {
        !self.saving || self.checkpoint_save_epoch == Some(self.save_playback_epoch)
    }
    fn checkpoint(&mut self, app: &App, force: bool) {
        self.checkpoint_at(app, force, None);
    }
    /// An explicit global Favorites selection outlives the temporary catalogue
    /// underneath it. Keep that catalogue's private bookmark too, but only
    /// persist a Favorite which still binds to the admitted source and owner.
    fn durable_global_favorite(&self, playing: &Playing) -> bool {
        // A managed publication remains nondurable even when its row is a Favorite.
        if managed::owns_profile(self, &playing.profile)
            || classic_source::temporary_id(self, &playing.profile.id)
        {
            return false;
        }
        if !self.saved.favorites_selected
            || playing.transient_owner
                != self
                    .transient
                    .accepted
                    .as_ref()
                    .map(transient::TransientMedia::owner)
        {
            return false;
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Identity {
            origin: plx_data::ott::favorites::Origin,
            locator: plx_data::ott::favorites::Locator,
        }
        let Some(encoded) = playing.selection_id.strip_prefix("favorite:") else {
            return false;
        };
        // Parse only the admitted identity. Comparing typed fields avoids
        // serializing every entry in a large Favorites document on each tick.
        let Ok(identity) = serde_json::from_str::<Identity>(encoded) else {
            return false;
        };
        let Some(favorite) = self.saved.favorites.entries().iter().find(|favorite| {
            favorite.origin == identity.origin && favorite.locator == identity.locator
        }) else {
            return false;
        };
        if !selection::matches_entry(favorite, &playing.entry) || favorite.identity() != encoded {
            return false;
        }
        let Ok(bindings) = self.favorite_bindings(favorite) else {
            return false;
        };
        let same_source = match selection::bind_bound(&bindings, favorite) {
            Ok(selection::SelectionOwner::Profile(profile)) => *profile == playing.profile,
            Ok(selection::SelectionOwner::LocalRoot(root)) => {
                transient::source_profile(root).is_ok_and(|profile| profile == playing.profile)
            }
            Ok(selection::SelectionOwner::M3uAttachment(receipt)) => receipt
                .effective_profile()
                .is_ok_and(|profile| profile == playing.profile),
            Err(_) => false,
        };
        same_source
            && (favorite.origin.provider != "local"
                || self
                    .saved
                    .local_favorite_proofs
                    .authorize(&bindings, &self.saved.favorites, favorite, &playing.profile)
                    .is_ok())
    }
    fn checkpoint_at(
        &mut self,
        app: &App,
        force: bool,
        exit: Option<&plx_media::player::machine::ExitReceipt>,
    ) {
        if self.continuity.pending_restore().is_some()
            || self.restore_play_generation == Some(self.play_generation)
        {
            return;
        }
        let Some(playing) = self
            .playing
            .as_ref()
            .filter(|playing| {
                exit.map_or_else(
                    || playing_matches(playing, app),
                    |exit| {
                        // Compare the owner captured before teardown, not the current
                        // route: a newer admission may already be installed now.
                        playing.identity == exit.playback_identity
                            && match &playing.entry.media {
                                MediaLocator::Plex {
                                    server_id,
                                    rating_key,
                                } => {
                                    *rating_key == exit.rating_key
                                        && plx_plex::plex::id_of_machine(server_id)
                                            == Some(exit.sid)
                                }
                                _ => playing.source.is_some() && playing.source == exit.source,
                            }
                    },
                )
            })
            .cloned()
        else {
            return;
        };
        // Archive offsets belong to an expiring interval, never the live channel resume.
        if playing.archive.is_some() {
            return;
        }
        let mut key = if playing.selection_id.starts_with("favorite:") {
            "favorites".into()
        } else {
            playing.profile.id.clone()
        };
        let mut position_ms = exit
            .map_or_else(
                || playback::intended_pos(&app.player.session),
                |exit| exit.position_ns,
            )
            .max(0) as u64
            / 1_000_000;
        if let Some((owner, floor)) = self.resume_floor.as_mut() {
            if playing.identity.as_ref() == Some(&*owner) {
                // ExitReceipt captures intended position before teardown. It
                // may be a pending target, so it can never discharge this floor.
                // Neither can intended_pos while the initial seek is pending.
                let observed = exit.is_none()
                    && plx_media::player::seen_frame()
                    && !plx_media::route::play_pending();
                let duration_ns = plx_media::player::duration_ns();
                let raw_ns = plx_media::player::playpos_ns();
                let observed_ms =
                    (observed && raw_ns >= 0 && (duration_ns <= 0 || raw_ns <= duration_ns))
                        .then_some(raw_ns.max(0) as u64 / 1_000_000);
                let duration_ms =
                    (observed && duration_ns > 0).then_some(duration_ns.max(0) as u64 / 1_000_000);
                position_ms = floor.checkpoint(position_ms, observed_ms, duration_ms);
            } else {
                self.resume_floor = None;
            }
        }
        // A live bookmark identifies a channel, not a seekable broadcast offset.
        if guide_ui::is_live(&playing.profile, &playing.entry) {
            position_ms = 0;
        }
        let durable_local = kiosk_resume::checkpoint(self, &playing, position_ms);
        let temporary = self.transient_checkpoint(&playing, position_ms);
        if let Some(point) = durable_local {
            key = kiosk_resume::KEY.into();
            self.checkpoints.insert(key.clone(), point);
        } else if temporary && !self.durable_global_favorite(&playing) {
            return;
        } else {
            self.checkpoints.insert(
                key.clone(),
                SavedResume {
                    entry_id: playing.selection_id.clone(),
                    position_ms,
                    source_fingerprint: profile_fingerprint(&playing.profile),
                },
            );
        }
        let now = self.now_ms();
        let changed = self
            .checkpoints
            .iter()
            .any(|(key, value)| self.saved.resume.get(key) != Some(value));
        // Channel/source selection must survive a webOS close or an in-place
        // upgrade even inside the position checkpoint interval. Only advancing
        // the playhead of the same admitted selection remains throttled.
        let point = &self.checkpoints[&key];
        let selection_changed = self.saved.resume.get(&key).is_none_or(|saved|
            saved.entry_id != point.entry_id || saved.source_fingerprint != point.source_fingerprint);
        let selection_attempted = self.checkpoint_write_selection.as_ref().is_some_and(|attempt|
            attempt.0 == key && attempt.1 == point.entry_id && attempt.2 == point.source_fingerprint);
        if changed
            && (force || (selection_changed && !selection_attempted)
                || now.saturating_sub(self.last_checkpoint_ms) >= 10000)
            && !self.saving
            && !self.persistence_uncertain
        {
            let selection = (key, point.entry_id.clone(), point.source_fingerprint.clone());
            if self.save(self.saved.clone(), None, false).is_ok() {
                self.checkpoint_save_epoch = Some(self.save_playback_epoch);
            }
            // A refused admission or failed writer keeps the normal interval
            // for this selection. A busy writer never spends a newer choice's
            // bypass because it cannot enter this attempt block.
            self.checkpoint_write_selection = Some(selection);
            self.last_checkpoint_ms = now;
        }
    }
    fn request_metadata(&mut self, profile: ProviderProfile, entry: CatalogEntry) {
        if self.guide.is_some() {
            return;
        }
        if self.defer_preview_for_local_assets(&profile, &entry.id) {
            return;
        }
        if profile.kind == ProviderKind::Local {
            if self.metadata_target.as_ref() == Some(&(profile.id.clone(), entry.id.clone())) {
                return;
            }
            let binding = self.local_profile_selection(&profile);
            if let Some(binding) = binding {
                let id = entry.id.clone();
                self.request_local_asset_preview(profile, entry, id, binding);
            }
            return;
        }
        if !matches!(
            profile.kind,
            ProviderKind::M3u | ProviderKind::Xtream | ProviderKind::Stalker | ProviderKind::Edem | ProviderKind::Operator(_)
        ) {
            return;
        }
        self.metadata_local_selection = None;
        self.metadata_attachment = None;
        let target = (profile.id.clone(), entry.id.clone());
        let identity = self
            .playing
            .as_ref()
            .filter(|playing| {
                playing.profile == profile
                    && playing.entry.id == entry.id
                    && playing.entry.media == entry.media
                    && playing.entry.live == entry.live
            })
            .and_then(|playing| playing.identity.clone());
        if self.metadata_target.as_ref() == Some(&target)
            && self.metadata_playback_identity == identity
        {
            return;
        }
        self.metadata_playback_identity = identity;
        self.metadata_cancel.store(true, Ordering::Relaxed);
        self.metadata_cancel = Arc::new(AtomicBool::new(false));
        self.metadata_generation = self.metadata_generation.wrapping_add(1);
        self.metadata_target = Some(target);
        self.metadata_query = Some(plx_data::ott::guide::Query::CurrentNext {
            now: (self.wall_clock_ms / 1000) as i64,
        });
        self.metadata_refresh_ms = 0;
        self.metadata_next = Some((profile, MetadataSelection::Entry(entry)));
        live_hud::arm(self);
        self.launch_metadata();
    }
    fn request_favorite_metadata(&mut self, id: &str) -> Result<(), &'static str> {
        if self.guide.is_some() {
            return Ok(());
        }
        let playback_identity = self
            .playing
            .as_ref()
            .filter(|playing| playing.selection_id == id)
            .and_then(|playing| playing.identity.clone());
        if self
            .metadata_target
            .as_ref()
            .is_some_and(|(_, target)| target == id)
            && self.metadata_playback_identity == playback_identity
            && self.metadata_attachment_current()
        {
            return Ok(());
        }
        let identity = id
            .strip_prefix("favorite:")
            .ok_or("Invalid Favorite selection")?;
        let favorite = self
            .saved
            .favorites
            .entries()
            .iter()
            .find(|f| f.identity() == identity)
            .cloned()
            .ok_or("Favorite no longer exists")?;
        let bindings = self
            .favorite_bindings(&favorite)
            .map_err(|_| "Source bindings are unavailable")?;
        let (profile, binding) = match selection::bind_bound(&bindings, &favorite)
            .map_err(|_| "Favorite source is unavailable or ambiguous")?
        {
            selection::SelectionOwner::Profile(profile) => (profile.clone(), None),
            selection::SelectionOwner::LocalRoot(root) => (
                transient::source_profile(root)?,
                Some(self.bind_local_selection(root)?),
            ),
            selection::SelectionOwner::M3uAttachment(receipt) => (
                receipt
                    .effective_profile()
                    .map_err(|error| error.message())?,
                None,
            ),
        };
        let local_proof = self.favorite_permit(&profile, &favorite)?;
        if self.defer_preview_for_local_assets(&profile, id) {
            return Ok(());
        }
        self.metadata_attachment = self.attachment_for(&favorite)?;
        self.metadata_local_selection = binding;
        self.metadata_playback_identity = playback_identity;
        self.metadata_cancel.store(true, Ordering::Release);
        self.metadata_cancel = Arc::new(AtomicBool::new(false));
        self.metadata_generation = self.metadata_generation.wrapping_add(1);
        let owner_id = self
            .metadata_attachment
            .as_ref()
            .map_or_else(|| profile.id.clone(), |receipt| receipt.parent().id.clone());
        self.metadata_target = Some((owner_id, id.into()));
        self.metadata_query = Some(plx_data::ott::guide::Query::CurrentNext {
            now: (self.wall_clock_ms / 1000) as i64,
        });
        self.metadata_refresh_ms = 0;
        self.metadata_next = Some((profile, MetadataSelection::Favorite(favorite, local_proof)));
        live_hud::arm(self);
        self.launch_metadata();
        Ok(())
    }
    fn launch_metadata(&mut self) {
        self.launch_metadata_checked(None);
    }
    fn launch_metadata_from_app(&mut self, app: &App) {
        self.launch_metadata_checked(Some(app));
    }
    fn launch_metadata_checked(&mut self, app: Option<&App>) {
        if !self.metadata_attachment_current() {
            return;
        }
        if !self.source_work_allowed() {
            return;
        }
        self.rearm_metadata_work();
        let mut managed_read = None;
        let mut classic_read = None;
        if let Some((profile, _)) = &self.metadata_next {
            if classic_source::owns_id(self, &profile.id) {
                let Some(app) = app else {
                    return;
                };
                match classic_source::capture_read(self, app, profile) {
                    Ok(read) => classic_read = read,
                    Err(_) => return,
                }
            }
            if managed::owns_retained_profile(self, profile) {
                let Some(app) = app else {
                    return;
                };
                match managed::capture_read(self, app, profile) {
                    Ok(read) => managed_read = read,
                    Err(error) => {
                        self.metadata_cancel.store(true, Ordering::Release);
                        self.metadata_generation = self.metadata_generation.wrapping_add(1);
                        self.metadata_next = None;
                        self.metadata_target = None;
                        self.metadata_query = None;
                        self.favorite_preview = None;
                        self.status = error.into();
                        self.changed();
                        return;
                    }
                }
            }
            let profile = profile.clone();
            managed::record_metadata(self, &profile, managed_read.as_ref());
        }
        if !self.metadata_work_allowed() {
            return;
        }
        if matches!(
            self.metadata_next.as_ref(),
            Some((_, MetadataSelection::LocalPublish(_)))
        ) {
            return;
        }
        if !self.metadata_done.load(Ordering::Acquire) {
            return;
        }
        let Some((profile, selection)) = self.metadata_next.take() else {
            return;
        };
        let query = self.metadata_query;
        let started = match selection {
            MetadataSelection::LocalPublish(completion) => {
                self.metadata_next = Some((profile, MetadataSelection::LocalPublish(completion)));
                return;
            }
            MetadataSelection::Local(work) => super::ott_metadata::start_local(
                work,
                self.metadata_generation,
                self.metadata_cancel.clone(),
                self.metadata_tx.clone(),
                self.metadata_done.clone(),
            ),
            MetadataSelection::Entry(entry) => query.is_some_and(|query| {
                let row_request = self.request_row_guide(&profile, &entry, query);
                if let Some(request) = row_request.as_ref() {
                    self.note_row_guide_attempt(request);
                }
                super::ott_metadata::start(
                    profile,
                    entry,
                    self.metadata_generation,
                    query,
                    row_request,
                    self.hosted_epg_config.clone(),
                    self.metadata_cancel.clone(),
                    self.metadata_tx.clone(),
                    self.metadata_done.clone(),
                )
            }),
            MetadataSelection::Favorite(favorite, local_proof) => query.is_some_and(|query| {
                if let Some(read) = managed_read {
                    super::ott_metadata::start_managed_favorite(
                        profile,
                        favorite,
                        read,
                        self.metadata_generation,
                        query,
                        self.metadata_cancel.clone(),
                        self.metadata_tx.clone(),
                        self.metadata_done.clone(),
                    )
                } else if let Some(read) = classic_read {
                    super::ott_metadata::start_classic_favorite(
                        profile,
                        favorite,
                        read,
                        self.metadata_generation,
                        query,
                        self.metadata_cancel.clone(),
                        self.metadata_tx.clone(),
                        self.metadata_done.clone(),
                    )
                } else if let Some(receipt) = self.metadata_attachment.clone() {
                    super::ott_metadata::start_attachment_favorite(
                        receipt,
                        favorite,
                        self.metadata_generation,
                        query,
                        self.metadata_cancel.clone(),
                        self.metadata_tx.clone(),
                        self.metadata_done.clone(),
                    )
                } else {
                    super::ott_metadata::start_favorite(
                        profile,
                        favorite,
                        local_proof,
                        self.metadata_generation,
                        query,
                        self.hosted_epg_config.clone(),
                        self.metadata_cancel.clone(),
                        self.metadata_tx.clone(),
                        self.metadata_done.clone(),
                    )
                }
            }),
        };
        if !started {
            guide_ui::start_failed(self);
            self.local_asset_start_failed();
            self.metadata_target = None;
            self.metadata_query = None;
        }
    }
    fn refresh_metadata(&mut self) {
        self.tick_row_guide();
        if !transient::lifecycle_metadata_ready(self) {
            return;
        }
        if self.guide.is_some() {
            return;
        }
        if self.metadata_refresh_ms == 0 || self.now_ms() < self.metadata_refresh_ms {
            return;
        }
        self.metadata_refresh_ms = 0;
        let Some((profile_id, entry_id)) = self.metadata_target.clone() else {
            return;
        };
        if entry_id.starts_with("favorite:") {
            self.metadata_target = None;
            let _ = self.request_favorite_metadata(&entry_id);
            return;
        }
        let selection = self
            .effective_profile()
            .filter(|p| p.id == profile_id)
            .and_then(|p| {
                self.catalog
                    .entries
                    .iter()
                    .find(|e| e.id == entry_id)
                    .map(|e| (p.clone(), e.clone()))
            })
            .or_else(|| {
                self.playing
                    .as_ref()
                    .filter(|p| p.profile.id == profile_id && p.entry.id == entry_id)
                    .map(|p| (p.profile.clone(), p.entry.clone()))
            });
        if let Some((profile, entry)) = selection {
            self.metadata_target = None;
            self.request_metadata(profile, entry);
        }
    }
    fn protected_state_status(&self) -> Option<&'static str> {
        if self.persistence_uncertain {
            return Some("Settings write needs reconciliation; restart the player");
        }
        match self.state_load {
            StateLoad::Failed(error) => Some(error),
            StateLoad::Loading => Some("Loading…"),
            StateLoad::Idle => None,
        }
    }

    fn profile_add_refusal(&self) -> Option<&'static str> {
        self.saved
            .profiles
            .check_addition(self.provider)
            .err()
            .map(|error| error.message())
    }

    fn category_label(&self) -> Option<String> {
        if !matches!(self.page, "channels" | "favorites") {
            return None;
        }
        if self.view_selection().1 {
            return Some(plx_platform::i18n::msg::ott_favorites().into());
        }
        let selected = self.effective_category();
        if selected.is_empty() {
            return Some(plx_platform::i18n::msg::browse_library_all().into());
        }
        let id = self.organization_category_id(selected);
        self.organization_categories()
            .into_iter()
            .find(|category| category.id == id)
            .map(|category| category.title)
    }

    fn view(&self) -> OttView {
        plx_base::diag::spans::span("ott_view", || {
            let mut view = menu_settings::view(self)
                .unwrap_or_else(|| settings_groups::view(self, self.raw_view()));
            view.pending_play = classic_play_handoff::selected(self);
            view.pending_step = playback_step::pending_source(self);
            view.pending_step_sdl = !cfg!(feature = "hostsim");
            if view.pending_play.is_some() {
                view.status = "Opening video…".into();
            }
            view.classic_preferences = self.classic_preferences();
            view.volume_available = self.playing.is_some();
            ui_text::view(view)
        })
    }

    fn remote_find_status(&self) -> Option<String> {
        let message = self.remote_snapshot["find_message"]
            .as_str()
            .filter(|s| !s.is_empty())?;
        let diagnostic = self.remote_snapshot["find_diagnostic"]
            .as_str()
            .unwrap_or("");
        Some(if diagnostic.is_empty() {
            message.to_owned()
        } else {
            // The runner already reduces errors to safe transport/HTTP diagnostics.
            // Localize the message before appending so the exact-string UI map still applies.
            format!("{} · {diagnostic}", ui_text::text(message))
        })
    }

    fn raw_view(&self) -> OttView {
        if let Some(view) = vportal_quality::view(self) {
            return view;
        }
        if let Some(view) = self.usb_browser_view() {
            return view;
        }
        if let Some(view) = guide_ui::view(self) {
            return view;
        }
        if self.page == "organization" {
            return organization_ui::view(self);
        }
        if self.page == "actions" {
            return self.local_order_view();
        }
        if let Some(view) = menu_visibility::view(self) {
            return view;
        }
        if let Some(view) = interface_mode::view(self) {
            return view;
        }
        if let Some(view) = sleep_timer::view(self) {
            return view;
        }
        if let Some(view) = osd_ui::view(self) {
            return view;
        }
        if let Some(mut view) = companion_adapter::view(self) {
            if let Some(status) = self.protected_state_status() {
                view.status = status.into();
            }
            return view;
        }
        // Resolve once per model rebuild, not once for every row.
        let effective_category = self.effective_category();
        let category_context = if self.page == "categories" {
            self.category_action_context()
        } else {
            String::new()
        };
        use ui_projection::row;
        let search = self.search_filter();
        let media_resume = self.ui_media_resume();
        let mut rows: Vec<OttRow> = match self.page {
            "profile-setup" => self
                .setup
                .as_ref()
                .map(|setup| {
                    let editable = self.pending_setup_save != Some(setup.id);
                    let mut fields: Vec<_> = setup
                        .draft
                        .fields()
                        .iter()
                        .map(|field| {
                            let mut item = row(
                                field.key.into(),
                                field.title.into(),
                                setup.draft.hint(field.key).unwrap_or("Not set").into(),
                                OttAction::EditSetting(field.key.into()),
                                false,
                            );
                            item.enabled = editable;
                            item
                        })
                        .collect();
                    let mut save = row(
                        "profile.setup.save".into(),
                        "Save profile".into(),
                        String::new(),
                        OttAction::EditSetting("profile.setup.save".into()),
                        false,
                    );
                    save.enabled = editable && self.loaded && !self.saving;
                    fields.push(save);
                    fields.push(row(
                        "profile.setup.cancel".into(),
                        "Cancel".into(),
                        String::new(),
                        OttAction::EditSetting("profile.setup.cancel".into()),
                        false,
                    ));
                    fields
                })
                .unwrap_or_default(),
            "providers" => ProviderKind::all()
            .into_iter()
            .map(|kind| {
                let id = kind.canonical_id().to_owned();
                row(
                    id.clone(),
                    match kind {
                        ProviderKind::M3u => "M3U television",
                        ProviderKind::VPortal => "VPortal",
                        ProviderKind::Local => "Local videos",
                        ProviderKind::Plex => "Plex",
                        ProviderKind::Xtream => "Xtream",
                        ProviderKind::Edem => "Edem / iLook TV",
                        ProviderKind::Operator(id) => id.name(),
                        ProviderKind::Stalker => "Stalker",
                    }
                    .into(),
                    String::new(),
                    OttAction::SelectProvider(id),
                    false,
                )
            })
            .collect(),
            "profiles" => {
                let mut profiles: Vec<_> = if self.classic_m3u_slots() {
                    self.m3u_slot_rows()
                } else {
                    self.saved
                        .profiles
                        .profiles
                        .iter()
                        .filter(|p| {
                            p.enabled
                                && p.kind == self.provider
                                && !managed::shadows_profile(self, p)
                        })
                        .flat_map(|p| profile_edit::rows(self, p))
                        .collect()
                };
                if self.provider == ProviderKind::Plex {
                    if let Some(row) = managed::row(self) {
                        profiles.insert(0, row);
                    }
                }
                if self.provider != ProviderKind::Plex && !self.classic_m3u_slots() {
                    let refusal = self.profile_add_refusal();
                    let mut add = row(
                        "action/profile.add".into(),
                        "Add profile".into(),
                        refusal.unwrap_or("Enter a source address or folder").into(),
                        OttAction::EditSetting("profile.add".into()),
                        false,
                    );
                    add.enabled = self.loaded && !self.saving && refusal.is_none();
                    profiles.push(add);
                } else if self.provider == ProviderKind::Plex {
                    let mut connect = row(
                        "action/plex.connect".into(),
                        "Connect Plex".into(),
                        String::new(),
                        OttAction::EditSetting("plex.connect".into()),
                        false,
                    );
                    connect.enabled = self.loaded && !self.saving;
                    profiles.push(connect);
                }
                if self.provider == ProviderKind::M3u {
                    let mut links = row(
                        "action/m3u-media.links".into(),
                        "Media library links…".into(),
                        String::new(),
                        OttAction::ShowM3uMediaLinks,
                        false,
                    );
                    links.enabled = self.m3u_media_ready();
                    profiles.push(links);
                }
                profiles
            }
            "m3u-media-links" => self.m3u_media_rows(),
            "categories" => std::iter::once(row(
                "all".into(),
                "All".into(),
                String::new(),
                OttAction::SelectCategoryAt {
                    context: category_context.clone(),
                    category: String::new(),
                },
                false,
            ))
            .chain(std::iter::once(row(
                "favorites".into(),
                "Favorites".into(),
                String::new(),
                OttAction::ShowFavorites,
                false,
            )))
            .chain(self.organization_categories().into_iter().map(|category| {
                row(
                    category.id.clone(),
                    category.title,
                    String::new(),
                    OttAction::SelectCategoryAt {
                        context: category_context.clone(),
                        category: category.id,
                    },
                    false,
                )
            }))
            .collect(),
            "favorites" => self
                .saved
                .favorites
                .entries()
                .iter()
                .enumerate()
                .filter(|(_, f)| search(&f.title))
                .map(|(index, f)| {
                    let id = format!("favorite:{}", f.identity());
                    let mut item = row(
                        id.clone(),
                        format!("{}  {}", index + 1, f.title),
                        format!("{} · {}", f.origin.provider, f.origin.profile),
                        OttAction::Play(id.clone()),
                        true,
                    );
                    let preview = self
                        .favorite_preview
                        .as_ref()
                        .filter(|(target, _)| *target == id)
                        .map(|(_, entry)| entry);
                    item.content_title = Some(f.title.clone());
                    item.media = Some(ui_projection::favorite(f, preview, &id, media_resume));
                    item.guide = self.selected_row_guide(&id, preview);
                    if let Some(entry) = preview {
                        item.guide_available = self
                            .prepare_play_request(PlaybackRequest::ordinary(&id, 0))
                            .is_ok_and(|(_, profile, _, _)| guide_ui::is_live(&profile, entry));
                        item.records_available = self
                            .prepare_play_request(PlaybackRequest::ordinary(&id, 0))
                            .is_ok_and(|(_, profile, _, _)| {
                                guide_ui::records_available(self, &profile, entry)
                            });
                        item.description = entry.description.clone();
                        item.poster = if matches!(entry.media, MediaLocator::Local { .. }) {
                            self.local_entry_art(entry)
                        } else {
                            entry_poster(entry)
                        };
                    }
                    if let Some(guide) = &item.guide {
                        item.description = guide
                            .current
                            .as_ref()
                            .map(|programme| programme.description.clone())
                            .unwrap_or_default();
                    }
                    item
                })
                .collect(),
            "settings" => [
                ("profile.add", "Add profile"),
                ("profile.name", "Profile name"),
                ("profile.source", "Source address or folder"),
                ("profile.history_hours", "Archive hours"),
                ("remote.device_id", "Device ID"),
                ("remote.address", "Remote control server"),
                ("remote.find", "Find command server"),
                ("uiFontScale", "Interface text scale (%)"),
                ("volumeStep", "Volume step (%)"),
                ("infoTimeout", "Information display time (seconds)"),
                ("interfaceMode", "Interface mode"),
                ("sleepTimeout", "Sleep timer"),
                ("osdOpacity", "OSD background level"),
                ("permanentTime", "Permanent clock"),
                ("pageSize", "Classic list rows"),
                ("listPosition", "Classic list side"),
                ("arrowFun", "Classic list arrows"),
                ("seek13Duration", "Seek step for 1/3 (seconds)"),
                ("native.audio", "Audio tracks"),
                ("native.subtitles", "Subtitles and appearance"),
                ("native.settings", "Playback and interface settings"),
                ("menu.items", "Menu items"),
                ("parental.unlock", "Unlock parental access"),
                ("parental.create", "Set parental PIN"),
                ("parental.disable", "Disable parental PIN"),
                ("parental.channels", "Toggle channel protection"),
                ("parental.settings", "Toggle settings protection"),
                ("parental.providers", "Toggle provider selection protection"),
            ]
            .into_iter()
            .filter(|(id, _)| {
                *id != "profile.history_hours" || self.archive_hours_profile().is_some()
            })
            .filter(|(id, _)| {
                *id != "remote.find" || self.remote_snapshot["find_configured"] == true
            })
            .map(|(id, title)| {
                let mut item = row(
                    id.into(),
                    title.into(),
                    if id == "uiFontScale" {
                        format!("{}%", self.font_scale_percent())
                    } else if id == "volumeStep" {
                        format!("{}%", self.volume_step())
                    } else if id == "infoTimeout" {
                        format!("{} s", self.info_timeout().seconds())
                    } else if id == "remote.device_id" {
                        self.remote.persisted_device_id().unwrap_or("Identity loading or unavailable").into()
                    } else if id == "remote.find" {
                        self.remote_find_status().unwrap_or_default()
                    } else if id == "remote.address"
                        && self.remote_snapshot["find_configured"] != true
                    {
                        self.remote_snapshot["find_unavailable"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .unwrap_or("Discovery is not configured. Enter a server address.")
                            .into()
                    } else if id == "interfaceMode" {
                        interface_mode::label(self.interface_mode()).into()
                    } else if id == "sleepTimeout" {
                        self.sleep_timer_choice().label().into()
                    } else if let Some(detail) = osd_ui::detail(self, id) {
                        detail
                    } else if id == "profile.history_hours" {
                        self.archive_hours_label()
                    } else {
                        String::new()
                    },
                    OttAction::EditSetting(id.into()),
                    false,
                );
                if id == "remote.device_id" {
                    item.enabled = false;
                }
                if id == "interfaceMode" {
                    item.enabled = public_preferences::write_refusal(self).is_none();
                }
                if id == "sleepTimeout" {
                    item.enabled = self.settings_write_refusal().is_none();
                }
                if matches!(
                    id,
                    "osdOpacity"
                        | "permanentTime"
                        | "pageSize"
                        | "listPosition"
                        | "arrowFun"
                        | "seek13Duration"
                ) {
                    item.enabled = self.osd_setting_token(id).is_some();
                }
                if id == "remote.find" {
                    item.enabled = self.remote_snapshot["persisted"] != false
                        && self.remote_snapshot["saving"] != true;
                }
                if id == "profile.add" {
                    let refusal = self.profile_add_refusal();
                    item.enabled = self.loaded && !self.saving && refusal.is_none();
                    item.detail = refusal.unwrap_or_default().into();
                }
                item
            })
            .collect(),
            _ => {
                let context = ui_projection::CatalogContext::new(self.effective_profile());
                let favorite_index = self.selection_bindings().ok().and_then(|bindings| {
                    self.row_owner().and_then(|owner| {
                        selection::FavoriteIndex::new_proven(
                            &bindings,
                            &self.saved.favorites,
                            owner,
                            &self.saved.local_favorite_proofs,
                        )
                        .ok()
                        .map(|index| {
                            index.with_channel_catalog(
                                owner,
                                &self.catalog.entries,
                                self.catalog_complete,
                            )
                        })
                    })
                });
                self.organization_rows(effective_category)
                    .into_iter()
                    .filter(|(_, entry)| search(&entry.title))
                    .map(|(i, e)| {
                        let mut item = context.row(
                            i,
                            e,
                            favorite_index
                                .as_ref()
                                .is_some_and(|index| index.contains(e)),
                            |profile, entry| guide_ui::records_available(self, profile, entry),
                        );
                        item.content_title = Some(e.title.clone());
                        item.media = ui_projection::entry(e, &e.id, media_resume);
                        item.guide = self.selected_row_guide(&e.id, Some(e));
                        item.current_programme = self.current_row_programme(e);
                        item.description = e.description.clone();
                        item.poster = if matches!(e.media, MediaLocator::Local { .. }) {
                            self.local_entry_art(e)
                        } else {
                            entry_poster(e)
                        };
                        if let Some(guide) = &item.guide {
                            item.description = guide
                                .current
                                .as_ref()
                                .map(|programme| programme.description.clone())
                                .unwrap_or_default();
                        }
                        item
                    })
                    .collect()
            }
        };
        // Count this exact filtered list before unrelated action rows are appended.
        let catalog_progress = ui_projection::catalog(
            self.page,
            rows.len(),
            self.catalog_loading,
            self.catalog_complete,
        );
        if self.page == "profiles" && self.provider == ProviderKind::Local {
            rows.extend(self.usb_rows());
            rows.extend(
                commands::offline_rows(self)
                    .into_iter()
                    .map(|(number, name)| {
                        row(
                            format!("offline.bind:{number}"),
                            name,
                            "Choose this device's folder".into(),
                            OttAction::EditSetting(format!("offline.bind:{number}")),
                            false,
                        )
                    }),
            );
        }
        if self.page == "settings" {
            if self.remote.can_retry_load() {
                rows.push(row(
                    "remote.retry".into(),
                    "Retry remote control setup".into(),
                    String::new(),
                    OttAction::EditSetting("remote.retry".into()),
                    false,
                ));
            }
            rows.push(row(
                "remote.cancel".into(),
                "Cancel pairing".into(),
                String::new(),
                OttAction::EditSetting("remote.cancel".into()),
                false,
            ));
            if self.remote_snapshot["enabled"] == true {
                rows.push(row(
                    "remote.disable".into(),
                    "Disable remote control".into(),
                    String::new(),
                    OttAction::EditSetting("remote.disable".into()),
                    false,
                ));
            }
            if let Some(servers) = self.remote_snapshot["servers"].as_array() {
                // Keep asynchronous choices next to the action that reveals them.
                let after = rows
                    .iter()
                    .position(|row| row.id == "remote.find")
                    .or_else(|| rows.iter().position(|row| row.id == "remote.address"))
                    .map_or(rows.len(), |index| index + 1);
                rows.splice(
                    after..after,
                    servers.iter().enumerate().map(|(index, server)| {
                        let address = server["address"].as_str().unwrap_or("");
                        let id = server["id"].as_str().unwrap_or("");
                        let domain = server["domain"].as_str().unwrap_or("");
                        row(
                            // A position reused by a later result must not reuse its input key.
                            format!(
                                "remote.server:{index}:{}",
                                server["choice"].as_str().unwrap_or("")
                            ),
                            address.into(),
                            if domain.is_empty() {
                                id.into()
                            } else {
                                format!("{domain} · {id}")
                            },
                            // The engine binds this opaque token to the validated
                            // result generation and canonical endpoint. No index fallback.
                            OttAction::EditSetting(format!(
                                "remote.choice:{}",
                                server["choice"].as_str().unwrap_or("")
                            )),
                            false,
                        )
                    }),
                );
            }
        }
        if self.page == "categories" {
            rows.extend(organization_ui::entry(self));
        }
        rows.extend(companion_adapter::rows(self));
        // Keep first-run setup discoverable without requiring the Browse menu.
        if matches!(self.page, "providers" | "profiles")
            && !self
                .saved
                .profiles
                .profiles
                .iter()
                .any(|profile| profile.enabled)
        {
            rows.push(row(
                "action/settings".into(),
                "Settings".into(),
                String::new(),
                OttAction::ShowSettings,
                false,
            ));
        }
        if self.page == "settings" {
            let visible = menu_visibility::visibility(self);
            rows.retain(|row| match row.id.as_str() {
                "native.audio" => visible.visible(plx_appkit::menu_visibility::Item::Audio),
                "native.subtitles" => visible.visible(plx_appkit::menu_visibility::Item::Subtitles),
                _ => true,
            });
        }
        let selected_row = match self.page {
            "profile-setup" => self.setup.as_ref().map(|setup| setup.selected_key.clone()),
            "providers" => Some(self.provider.canonical_id().into()),
            "categories" => Some(if self.view_selection().1 {
                "favorites".into()
            } else if effective_category.is_empty() {
                "all".into()
            } else {
                self.organization_category_id(effective_category)
            }),
            "settings" => self
                .settings_focus
                .filter(|id| rows.iter().any(|row| row.enabled && row.id == *id))
                .map(str::to_owned),
            "m3u-media-links" => self
                .profile_focus
                .as_ref()
                .and_then(|id| {
                    rows.iter().find(|row|
                matches!(&row.action, OttAction::EditM3uMediaLink { profile, .. } if profile == id))
                })
                .filter(|row| row.enabled)
                .or_else(|| rows.iter().find(|row| row.enabled))
                .map(|row| row.id.clone()),
            "profiles"
                if auth_return::focus(self)
                    && rows
                        .iter()
                        .any(|row| row.enabled && row.id == "action/plex.connect") =>
            {
                Some("action/plex.connect".into())
            }
            "profiles" => self
                .profile_focus
                .as_ref()
                .filter(|id| {
                    rows.iter()
                        .any(|row| row.enabled && row.id.as_str() == id.as_str())
                })
                .cloned()
                .or_else(|| self.provider_profile().map(|profile| profile.id.clone()))
                .or_else(|| {
                    rows.iter()
                        .find(|row| row.enabled)
                        .map(|row| row.id.clone())
                }),
            _ => browse_focus::selected(self, &rows).or_else(|| {
                self.selected
                    .as_ref()
                    .filter(|id| rows.iter().any(|row| row.id == **id))
                    .cloned()
            }),
        };
        let page_id = if self.page == "profile-setup" {
            format!(
                "profile-setup:{}",
                self.setup
                    .as_ref()
                    .map(|setup| setup.draft.kind())
                    .unwrap_or(self.provider)
                    .canonical_id()
            )
        } else if self.page == "profiles" {
            format!("profiles:{}", self.provider.canonical_id())
        } else {
            self.search_page_id().unwrap_or_else(|| self.page.into())
        };
        let shelves = modern_shelves::project(self, &rows);
        let shelf_context = (!shelves.is_empty()).then(|| modern_shelves::context(self));
        OttView {
            shelf_context,
            shelves,
            status_working: false,
            pending_play: None,
            pending_step: playback_step::pending_source(self),
            pending_step_sdl: !cfg!(feature = "hostsim"),
            classic_preferences: self.classic_preferences(),
            volume_available: self.playing.is_some(),
            category_label: self.category_label(),
            preview_context: preview::context(self),
            order_context: self.local_order_key(),
            random_context: random::context(self, &rows),
            catalog_progress,
            search: self.search_view(),
            page_id,
            revision: self.revision,
            title: match self.page {
                "providers" => "Providers",
                "profiles" => "Profiles",
                "profile-setup" => "Profile setup",
                "m3u-media-links" => "Media library links",
                "categories" => "Categories",
                "favorites" => "Favorites",
                "settings" => "Settings",
                _ => "Channel list",
            }
            .into(),
            description: if self.page == "m3u-media-links" {
                String::new()
            } else if self.page == "profile-setup" {
                "Enter the connection details supplied by your provider".into()
            } else if self.page == "settings"
                && self.remote_snapshot["pairing_server"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty())
            {
                self.remote_snapshot["pairing_server"]
                    .as_str()
                    .unwrap_or("")
                    .into()
            } else {
                self.effective_profile()
                    .map_or_else(String::new, |p| p.name.clone())
            },
            status: if let Some(status) = self.protected_state_status() {
                status.into()
            } else if self.page == "settings" {
                self.remote_find_status()
                    .or_else(|| {
                        self.remote_snapshot["pairing_code"]
                            .as_str()
                            .filter(|code| !code.is_empty())
                            .map(|code| format!("Pairing code: {code}"))
                    })
                    .unwrap_or_else(|| {
                        let message = self.remote_snapshot["message"].as_str().unwrap_or("");
                        let active_remote_status = matches!(
                            self.remote_snapshot["state"].as_str(),
                            Some(
                                "loading"
                                    | "discovering"
                                    | "choose"
                                    | "pairing"
                                    | "waiting"
                                    | "saving"
                                    | "error"
                            )
                        );
                        if self.status.is_empty() || (active_remote_status && !message.is_empty()) {
                            message.to_owned()
                        } else {
                            self.status.clone()
                        }
                    })
            } else if self.page == "profiles"
                && self.provider == ProviderKind::Plex
                && self.plex_omitted_servers > 0
            {
                let notice = format!(
                    "{} additional Plex servers exceed the profile limit",
                    self.plex_omitted_servers
                );
                if self.status.is_empty() {
                    notice
                } else {
                    format!("{} · {notice}", self.status)
                }
            } else if catalog_progress.is_some()
                && (self.status == format!("{} videos", self.catalog.entries.len())
                    || (self.page == "channels"
                        && self.catalog_loading
                        && self.status == "Loading…"))
            {
                // The typed progress owns this generated count/loading caption.
                // Provider errors and protected-state messages remain visible.
                String::new()
            } else {
                self.status.clone()
            },
            rows,
            selected_row,
        }
    }
    fn open_setup(&mut self, profile: ProviderProfile, is_new: bool) -> Result<(), &'static str> {
        if is_new {
            self.saved
                .profiles
                .check_addition(profile.kind)
                .map_err(|error| error.message())?;
        }
        let draft = profile_setup::Draft::new(&profile)?;
        let id = self
            .setup_serial
            .checked_add(1)
            .ok_or("Setup session limit reached; reopen the player")?;
        let selected_key = draft.fields()[0].key.into();
        self.cancel_setup();
        self.setup_serial = id;
        self.setup = Some(SetupSession {
            id,
            owner: self.reply,
            draft,
            original: (!is_new).then_some(profile),
            selected_key,
        });
        self.page = "profile-setup";
        self.status.clear();
        Ok(())
    }

    fn capture_source_editor(&self) -> Option<SourceEditorCapture> {
        let owner = self.reply?;
        let editor = self.edit_session.clone();
        let setup = if let Some(setup) = &self.setup {
            if setup.owner != Some(owner)
                || editor.as_ref().is_some_and(|editor| {
                    editor.owner != owner
                        || editor.purpose != search::EditorPurpose::Setting
                        || !editor.key.starts_with("profile.setup.")
                })
            {
                return None;
            }
            Some(setup.id)
        } else {
            let editor = editor.as_ref()?;
            if editor.owner != owner
                || editor.purpose != search::EditorPurpose::Setting
                || !((editor.key == "profile.source" && self.edit_profile.is_some())
                    || (editor.key == m3u_media_editor::KEY && self.m3u_media_editor.is_some()))
            {
                return None;
            }
            None
        };
        Some(SourceEditorCapture {
            owner,
            navigation: self.navigation_epoch,
            setup,
            editor,
        })
    }

    /// Called only after the remote write was durably accepted and applied,
    /// with its original activation still allowed to replace the source.
    fn retire_source_editor(
        &mut self,
        app: &mut App,
        capture: SourceEditorCapture,
        navigation: SaveNavigation,
    ) {
        if navigation != SaveNavigation::Activate(capture.navigation)
            || self.navigation_epoch != capture.navigation
            || self.reply != Some(capture.owner)
            || current_ui_owner(app) != Some(capture.owner)
            || self.edit_session != capture.editor
            || self.setup.as_ref().map(|setup| setup.id) != capture.setup
            || self
                .setup
                .as_ref()
                .is_some_and(|setup| setup.owner != Some(capture.owner))
        {
            return;
        }
        self.cancel_setup();
        if let Some(editor) = capture.editor {
            app.pages.emit(
                MachineId::Nav,
                Fx::Deliver(
                    MachineId::Instance(capture.owner),
                    Delivery::Screen(plx_ui::screen::ScreenEvent::App(AppMsg::OttEditorRetired {
                        session: editor.id,
                    })),
                ),
            );
        }
    }

    fn cancel_setup(&mut self) {
        self.setup = None;
        self.cancel_editor();
        if self.page == "profile-setup" {
            self.page = "profiles";
        }
        // A previously admitted save retains its own ID until its result arrives.
    }

    fn setup_candidate(&mut self) -> Result<(u64, Saved), &'static str> {
        let setup = self
            .setup
            .as_ref()
            .ok_or("Profile setup is no longer active")?;
        let profile = setup.draft.prepare()?;
        let original = setup.original.clone();
        let id = setup.id;
        let candidate = self.profile_candidate(profile, original.as_ref())?;
        Ok((id, candidate))
    }

    /// Both scalar and structured editors retain the original target snapshot.
    /// Validate only that target so unrelated profile updates remain saveable.
    fn profile_candidate(
        &mut self,
        profile: ProviderProfile,
        original: Option<&ProviderProfile>,
    ) -> Result<Saved, &'static str> {
        let current = self
            .saved
            .profiles
            .profiles
            .iter()
            .find(|p| p.id == profile.id);
        if current != original {
            return Err("This profile changed; reopen its setup before saving");
        }
        let mut candidate = self.saved.clone();
        if profile.kind == ProviderKind::Stalker
            && original.is_some_and(|original| original.name != profile.name)
        {
            // A UI rename supersedes the raw name remembered by the wire setter.
            // Source-only/no-op edits retain its intentional empty-name semantics.
            candidate
                .settings
                .remove(&format!("wireName:{}", profile.id));
        }
        if let Some(existing) = candidate
            .profiles
            .profiles
            .iter_mut()
            .find(|p| p.id == profile.id)
        {
            *existing = profile.clone();
        } else {
            candidate
                .profiles
                .check_addition(profile.kind)
                .map_err(|error| error.message())?;
            commands::assign_profile_slot(&mut candidate, &profile)?;
            candidate.profiles.profiles.push(profile.clone());
        }
        candidate.profiles.active_profile = Some(profile.id);
        self.prune_m3u_attachments(&mut candidate)?;
        candidate.validate()?;
        Ok(candidate)
    }

    /// A retained editor may retry its own durable save after application failed.
    /// An old receipt cannot rebase a newer editor, even when it edits the same ID.
    fn complete_profile_editor(&mut self, session: &EditorSession, persisted: bool) {
        if !persisted || self.edit_session.as_ref() != Some(session) {
            return;
        }
        self.complete_archive_hours(session);
        self.complete_m3u_media_editor(session);
        self.complete_m3u_slot(session);
        if let Some(profile) = self
            .edit_profile
            .as_ref()
            .and_then(|draft| {
                self.saved
                    .profiles
                    .profiles
                    .iter()
                    .find(|current| current.id == draft.id)
            })
            .cloned()
        {
            self.edit_profile = Some(profile.clone());
            self.edit_profile_original = Some(profile);
        }
    }

    fn complete_setup(&mut self, succeeded: bool, persisted: bool) {
        let Some(id) = self.pending_setup_save.take() else {
            return;
        };
        let Some(setup) = self.setup.as_mut().filter(|setup| setup.id == id) else {
            return;
        };
        if persisted {
            // A verified write can still fail when applied to the running player.
            // Retained input then edits our newly durable profile, not an external change.
            setup.original = self
                .saved
                .profiles
                .profiles
                .iter()
                .find(|profile| profile.id == setup.draft.profile_id())
                .cloned();
        }
        if succeeded {
            self.cancel_setup();
        }
    }

    /// Read the same credential-eligible registry snapshot for UI and remote setup.
    /// This returns profile metadata only; no token, origin or authorization is changed.
    fn native_plex_setup_servers(&self) -> Vec<super::ott_plex_setup::PlexSetupServer> {
        use super::ott_plex_setup::PlexSetupServer;
        let servers: Vec<_> = plx_plex::plex::server_ids()
            .into_iter()
            .filter_map(|sid| {
                let client = plx_plex::plex::client_for(sid)?;
                let machine_id = client.machine_id();
                if self.managed.temporary_machine() == Some(machine_id) {
                    return None;
                }
                if machine_id.is_empty()
                    || !plx_plex::plex::grant::credential_allowed_for(machine_id, client.origin())
                {
                    return None;
                }
                Some(PlexSetupServer {
                    machine_id: machine_id.into(),
                    name: plx_plex::plex::server_facts(sid)
                        .map(|facts| facts.name.clone())
                        .unwrap_or_default(),
                    id_hint: sid.raw(),
                })
            })
            .collect();
        servers
    }

    fn recover_plex_profiles(&mut self, app: &mut App, explicit: bool) -> Result<(), &'static str> {
        if !explicit && managed::browse_active(self, app) {
            return Ok(());
        }
        use super::ott_plex_setup::{self, PlexSetupPlan};
        self.profile_focus = None;
        self.plex_omitted_servers = 0;
        // The registry is a source snapshot, not evidence of current connectivity.
        // Apply its existing credential policy without probing or authorizing URLs.
        let servers = self.native_plex_setup_servers();
        if !explicit
            && self.saved.profiles.profiles.iter().any(|profile| {
                profile.enabled
                    && profile.kind == ProviderKind::Plex
                    && servers
                        .iter()
                        .any(|server| server.machine_id == profile.source)
            })
        {
            return Ok(());
        }
        if let Some(error) = self.settings_write_refusal() {
            return Err(error);
        }
        match ott_plex_setup::plan(&self.saved.profiles, &servers)? {
            PlexSetupPlan::Login if !explicit && self.managed.has_candidate() => {}
            PlexSetupPlan::Login => auth_return::request(self, app)?,
            PlexSetupPlan::Profiles {
                updated,
                first_profile_id,
                changed,
                omitted_servers,
            } => {
                self.profile_focus = Some(first_profile_id);
                self.plex_omitted_servers = omitted_servers;
                if changed {
                    let mut candidate = self.saved.clone();
                    candidate.profiles = updated;
                    // Active source and selectedProvider stay unchanged: this only
                    // prepares the browsed list, with no catalogue load or autoplay.
                    self.save(candidate, None, false)?;
                }
            }
        }
        Ok(())
    }

    fn cancel_editor(&mut self) {
        self.archive_hours_editor = None;
        self.m3u_media_editor = None;
        self.organization_cancel_editor();
        self.edit_session = None;
        self.edit_profile = None;
        self.edit_profile_original = None;
        self.profile_edit_save_only = None;
        self.m3u_slot_draft = None;
        // An admitted writer owns its separate completion address. Canceling
        // a draft must neither cancel that write nor retarget its result.
    }

    fn setting_submission_pending(&self, owner: InstanceId, session: u64) -> bool {
        self.pending_editor_save
            .as_ref()
            .is_some_and(|pending| pending.owner == owner && pending.id == session)
            || parental_runtime::submission_pending(self, owner, session)
            || local_profile::pending(self, owner, session)
    }

    /// A submitted draft already has disabled Save. Settle admission errors on
    /// that exact editor instead of replacing it with a PIN prompt or leaving
    /// it waiting for a writer that was never started.
    fn admit_setting_submission(
        &mut self,
        app: &mut App,
        action: &OttAction,
    ) -> Option<Result<bool, &'static str>> {
        let OttAction::SubmitSetting { session, key, .. } = action else {
            return None;
        };
        // Unaddressed companion editors retain their existing adapter contract.
        if *session == 0 {
            return None;
        }
        let current = self
            .edit_session
            .as_ref()
            .filter(|editor| {
                editor.id == *session
                    && editor.key == *key
                    && editor.purpose == search::EditorPurpose::Setting
                    && self.reply == Some(editor.owner)
                    && current_ui_owner(app) == Some(editor.owner)
            })
            .cloned();
        if current
            .as_ref()
            .is_some_and(|editor| self.setting_submission_pending(editor.owner, editor.id))
        {
            // The original writer/verifier alone owns its eventual result.
            return Some(Ok(false));
        }
        let result = input_policy::ui(self, action).and_then(|_| {
            // PIN editors have their own Control/verification checks in action().
            if key.starts_with("parental.") {
                Ok(())
            } else {
                self.require_parental(parental::Target::Settings)
            }
        });
        if let (Err(error), Some(editor)) = (&result, current) {
            self.editor_result(app, &editor, Some((*error).into()));
        }
        Some(result.map(|_| true))
    }

    fn editor_result(&mut self, app: &mut App, session: &EditorSession, error: Option<String>) {
        if error.is_none() && self.edit_session.as_ref() == Some(session) {
            self.cancel_editor();
        }
        app.pages.emit(
            MachineId::Nav,
            Fx::Deliver(
                MachineId::Instance(session.owner),
                Delivery::Screen(plx_ui::screen::ScreenEvent::App(AppMsg::OttEditorResult {
                    session: session.id,
                    error,
                })),
            ),
        );
    }

    fn publish(&self, app: &mut App) {
        if let Some(instance) = self.reply {
            app.pages.emit(
                MachineId::Nav,
                Fx::Deliver(
                    MachineId::Instance(instance),
                    Delivery::Screen(plx_ui::screen::ScreenEvent::App(AppMsg::OttSnapshot(
                        Arc::new(self.view()),
                    ))),
                ),
            );
            self.catalog_diagnostics.view_published(self.generation);
        }
    }
    pub(super) fn pending_play_profile(&self) -> Option<&ProviderProfile> {
        self.play_target.as_ref()?;
        self.play_profile
            .as_ref()
            .filter(|(generation, _)| *generation == self.play_generation)
            .map(|(_, profile)| profile)
    }
    fn cancel_play_request(&mut self) {
        self.classic_play_handoff = Default::default();
        self.live_retry_generation = None;
        self.explicit_step.clear();
        preplay::cancel(self, preplay::CancelReason::Cancelled);
        vportal_quality::cancel(self);
        self.play_attachment = None;
        self.play_cancel.store(true, Ordering::Relaxed);
        self.play_generation = self.play_generation.wrapping_add(1);
        self.play_target = None;
        self.play_profile = None;
        self.restore_play_generation = None;
        self.play_ticket = None;
        self.resolver_next = None;
        if let Some(invocation) = self.pending_play.take() {
            let response = favorites_items::finish(
                &invocation,
                rejected("Playback request superseded"),
                Some("cancelled"),
            );
            self.remote.complete(invocation, response);
        }
    }
    fn rebind_native_admission(
        &mut self,
        app: &App,
        profile: &ProviderProfile,
        entry: &CatalogEntry,
    ) -> Result<(), &'static str> {
        profile_rename::guard(self, profile)?;
        managed::guard_profile(self, app, profile)?;
        classic_source::guard_profile(self, app, profile)?;
        if favorites_items::pending_error(self).is_some() {
            return Err("Favorite selection changed before playback");
        }
        let request = self
            .play_target
            .as_ref()
            .ok_or("Playback request is no longer current")?
            .clone();
        let (bound, bound_profile, bound_entry, favorite) =
            match transient::staged_native_binding(self, &request) {
                Some(result) => {
                    let (profile, entry) = result?;
                    (request.clone(), profile, Some(entry), None)
                }
                None => self.prepare_play_request(request.clone())?,
            };
        if bound != request
            || &bound_profile != profile
            || bound_entry
                .as_ref()
                .is_some_and(|current| current.id != entry.id || current.media != entry.media)
        {
            return Err("Playback source changed before native admission");
        }
        let policy_profile = self.attachment_policy_profile(&bound_profile, favorite.as_ref())?;
        self.require_parental_playback(&policy_profile)?;
        kiosk_source::admit_bound(
            self,
            app,
            kiosk_source::BoundCandidate {
                request: &bound,
                profile: &bound_profile,
                entry: Some(entry),
                favorite: favorite.as_ref(),
            },
        )
        .map_err(kiosk_source::SourceError::message)
    }
    fn play_reply(
        &mut self,
        app: &mut App,
        id: &str,
        invocation: &Invocation,
    ) -> Result<(), &'static str> {
        self.play_with_policy(app, id, invocation.action != "play_catalog")?;
        self.pending_play = Some(invocation.clone());
        kiosk_capture::explicit_admitted(self);
        Ok(())
    }
    #[cfg(all(test, feature = "hostsim"))]
    fn play(&mut self, app: &mut App, id: &str) -> Result<(), &'static str> {
        self.play_with_policy(app, id, true)
    }
    fn play_with_policy(
        &mut self,
        app: &mut App,
        id: &str,
        advance_on_failure: bool,
    ) -> Result<(), &'static str> {
        if !self.play_work_allowed() {
            return Err("Media work is waiting for foreground or worker retirement");
        }
        let advance_on_failure =
            advance_on_failure && transient::plex_remote::choice(self, id).is_none();
        let request = PlaybackRequest {
            kiosk_selection: None,
            archive: None,
            continuation: None,
            transient_owner: None,
            local_selection: None,
            id: id.into(),
            position_ms: 0,
            advance_on_failure,
        };
        self.play_request_with_policy(app, request)
    }
    fn play_request_with_policy(
        &mut self,
        app: &mut App,
        request: PlaybackRequest,
    ) -> Result<(), &'static str> {
        self.play_request_with_quality_policy(app, request, false)
    }
    fn play_request_with_quality_policy(
        &mut self,
        app: &mut App,
        request: PlaybackRequest,
        interactive: bool,
    ) -> Result<(), &'static str> {
        if !self.play_work_allowed() {
            return Err("Media work is waiting for foreground or worker retirement");
        }
        let prepared = self.prepare_play_request(request)?;
        self.play_prepared_request_with_quality_policy(app, prepared, interactive)
    }
    fn play_prepared_request_with_quality_policy(
        &mut self,
        app: &mut App,
        (request, profile, entry, favorite): (
            PlaybackRequest,
            ProviderProfile,
            Option<CatalogEntry>,
            Option<Favorite>,
        ),
        interactive: bool,
    ) -> Result<(), &'static str> {
        if !self.play_work_allowed() {
            return Err("Media work is waiting for foreground or worker retirement");
        }
        let id = request.id.clone();
        profile_rename::guard(self, &profile)?;
        if !transient::lifecycle_target_ready(self, &profile, request.local_selection.as_ref()) {
            return Err("The local source is still being validated");
        }
        managed::guard_profile(self, app, &profile)?;
        classic_source::guard_profile(self, app, &profile)?;
        let policy_profile = self.attachment_policy_profile(&profile, favorite.as_ref())?;
        self.require_parental_playback(&policy_profile)?;
        kiosk_source::admit_bound(
            self,
            app,
            kiosk_source::BoundCandidate {
                request: &request,
                profile: &profile,
                entry: entry.as_ref(),
                favorite: favorite.as_ref(),
            },
        )
        .map_err(kiosk_source::SourceError::message)?;
        transient::cancel_play_candidate(self, app);
        self.resume_pending = None;
        self.playback_epoch = self.playback_epoch.wrapping_add(1);
        self.continuity.user_selected(self.generation, &id);
        let result =
            self.queue_play_request_quality(request, profile, entry, favorite, true, interactive);
        if result.is_err() {
            self.play_target = None;
            self.play_profile = None;
            if let Some(playing) = self.playing.as_ref() {
                self.continuity
                    .retain_selection(self.generation, &playing.selection_id);
            }
        }
        result
    }
    fn play_at(&mut self, id: &str, position_ms: u64) -> Result<(), &'static str> {
        self.play_request(PlaybackRequest::ordinary(id, position_ms))
    }
    fn favorite_permit(
        &self,
        profile: &ProviderProfile,
        favorite: &Favorite,
    ) -> Result<Option<LocalFavoritePermit>, &'static str> {
        self.saved
            .local_favorite_proofs
            .authorize(
                &self
                    .favorite_bindings(favorite)
                    .map_err(selection::BindingError::message)?,
                &self.saved.favorites,
                favorite,
                profile,
            )
            .map_err(selection::BindingError::message)
    }
    fn prepare_play_request(
        &self,
        mut request: PlaybackRequest,
    ) -> Result<
        (
            PlaybackRequest,
            ProviderProfile,
            Option<CatalogEntry>,
            Option<Favorite>,
        ),
        &'static str,
    > {
        if let Some(bound) = kiosk_source::bind_pinned_request(self, &request)
            .map_err(kiosk_source::SourceError::message)?
        {
            profile_rename::guard(self, &bound.1)?;
            self.validate_archive_request(&bound.0, &bound.1, bound.2.as_ref())?;
            return Ok(bound);
        }
        let id = &request.id;
        let (profile, entry, favorite, binding) =
            if let Some(identity) = id.strip_prefix("favorite:") {
                let favorite = self
                    .saved
                    .favorites
                    .entries()
                    .iter()
                    .find(|f| f.identity() == identity)
                    .cloned()
                    .ok_or("Favorite no longer exists")?;
                let bindings = self
                    .favorite_bindings(&favorite)
                    .map_err(|_| "Source bindings are unavailable")?;
                let owner = selection::bind_bound(&bindings, &favorite)
                    .map_err(|_| "Favorite source is unavailable or ambiguous")?;
                match owner {
                    selection::SelectionOwner::Profile(profile) => {
                        (profile.clone(), None, Some(favorite), None)
                    }
                    selection::SelectionOwner::LocalRoot(root) => (
                        transient::source_profile(root)?,
                        None,
                        Some(favorite),
                        Some(self.bind_local_selection(root)?),
                    ),
                    selection::SelectionOwner::M3uAttachment(receipt) => (
                        receipt
                            .effective_profile()
                            .map_err(|error| error.message())?,
                        None,
                        Some(favorite),
                        None,
                    ),
                }
            } else {
                let entry = self
                    .catalog
                    .entries
                    .iter()
                    .find(|entry| entry.id == *id)
                    .cloned()
                    .ok_or("Video is no longer in the catalogue")?;
                let profile = self
                    .effective_profile()
                    .cloned()
                    .ok_or("Select a provider profile")?;
                let binding = match self.row_owner() {
                    Some(selection::SelectionOwner::LocalRoot(root)) => {
                        Some(self.bind_local_selection(root)?)
                    }
                    _ => None,
                };
                (profile, Some(entry), None, binding)
            };
        if let Some(favorite) = &favorite {
            self.favorite_permit(&profile, favorite)?;
            request.advance_on_failure = false;
        }
        if request.continuation.is_some() && request.local_selection != binding {
            return Err("Local source changed before continuation");
        }
        request.local_selection = binding;
        if entry.as_ref().is_some_and(|entry| guide_ui::is_live(&profile, entry)) {
            request.advance_on_failure = false;
        }
        if request.transient_owner.is_none() {
            request.transient_owner = self
                .transient
                .accepted
                .as_ref()
                .map(transient::TransientMedia::owner);
        }
        if request.transient_owner.is_some()
            && request.transient_owner
                != self
                    .transient
                    .accepted
                    .as_ref()
                    .map(transient::TransientMedia::owner)
        {
            return Err("Temporary playback session ended");
        }
        self.validate_archive_request(&request, &profile, entry.as_ref())?;
        profile_rename::guard(self, &profile)?;
        Ok((request, profile, entry, favorite))
    }
    fn validate_archive_request(
        &self,
        request: &PlaybackRequest,
        profile: &ProviderProfile,
        entry: Option<&CatalogEntry>,
    ) -> Result<(), &'static str> {
        if let Some(archive) = &request.archive {
            if request.advance_on_failure
                || request.position_ms != 0
                || request.continuation.is_some()
            {
                return Err("Archive playback must be an exact request");
            }
            if let Some(entry) = entry {
                archive.validate(
                    profile,
                    entry,
                    archive_play::epoch_seconds(self.wall_clock_ms)?,
                )?;
            }
        }
        Ok(())
    }
    // An archive retains its original channel as the source proof, but it is
    // finite playback: ordinary kiosk recovery must never replace it with live.
    fn archive_playback_owned(&self, app: &App) -> bool {
        match &self.play_target {
            Some(request) => request.archive.is_some(),
            None => self
                .playing
                .as_ref()
                .is_some_and(|playing| playing.archive.is_some() && playing_matches(playing, app)),
        }
    }
    fn play_request(&mut self, request: PlaybackRequest) -> Result<(), &'static str> {
        let (request, profile, entry, favorite) = self.prepare_play_request(request)?;
        self.queue_play_request(request, profile, entry, favorite, true)
    }
    fn queue_play_request(
        &mut self,
        request: PlaybackRequest,
        profile: ProviderProfile,
        entry: Option<CatalogEntry>,
        favorite: Option<Favorite>,
        select: bool,
    ) -> Result<(), &'static str> {
        self.queue_play_request_quality(request, profile, entry, favorite, select, false)
    }
    fn queue_play_request_quality(
        &mut self,
        request: PlaybackRequest,
        profile: ProviderProfile,
        entry: Option<CatalogEntry>,
        favorite: Option<Favorite>,
        select: bool,
        interactive: bool,
    ) -> Result<(), &'static str> {
        profile_rename::guard(self, &profile)?;
        if !self.play_work_allowed() {
            return Err("Media work is waiting for foreground or worker retirement");
        }
        if !transient::lifecycle_target_ready(self, &profile, request.local_selection.as_ref()) {
            return Err("The local source is still being validated");
        }
        let local_proof = favorite
            .as_ref()
            .map(|favorite| match request.kiosk_selection.as_ref() {
                Some(selection) => kiosk_source::pinned_permit(self, selection, &profile)
                    .map_err(kiosk_source::SourceError::message),
                None => self.favorite_permit(&profile, favorite),
            })
            .transpose()?
            .flatten();
        let attachment = favorite
            .as_ref()
            .map(|favorite| self.attachment_for(favorite))
            .transpose()?
            .flatten();
        let plex_choice = transient::plex_remote::choice(self, &request.id);
        let local_read = transient::capture_video_root(self, &request, &profile)?;
        preplay::cancel(self, preplay::CancelReason::Superseded);
        self.cancel_play_request();
        self.play_cancel = Arc::new(AtomicBool::new(false));
        self.play_ticket = Some(plx_media::route::play_request_ticket());
        if select {
            self.selected = Some(request.id.clone());
        }
        let selected_id = request.id.clone();
        let archive = request.archive.clone();
        self.play_target = Some(request);
        self.play_profile = Some((self.play_generation, profile.clone()));
        self.play_attachment = attachment;
        self.playback_failure.clear();
        self.failed_playback = None;
        preplay::begin(self, &profile);
        if interactive && profile.kind == ProviderKind::VPortal {
            let result = vportal_quality::request(self, &profile);
            if result.is_err() {
                let generation = self.play_generation;
                preplay::failed(self, generation, preplay::Failure::AdmissionFailed);
            }
            result?;
        }
        transient::plex_remote::selected(self, &selected_id);
        self.status = "Opening video…".into();
        self.resolver_next = Some(ResolveRequest {
            archive,
            generation: self.play_generation,
            profile,
            entry,
            favorite,
            local_proof,
            cancel: self.play_cancel.clone(),
            plex: None,
            plex_choice,
            local_read,
        });
        playback_step::bind_pending(self);
        self.changed();
        if let Some(attempt) = kiosk_source::admitted_pinned_request(self) {
            self.note_kiosk_attempt(attempt);
        }
        let generation = self.play_generation;
        let result = self.launch_resolver();
        if result.is_err() {
            preplay::failed(self, generation, preplay::Failure::ResolveFailed);
        }
        result
    }
    fn validate_archive_admission(
        &self,
        request: &PlaybackRequest,
        profile: &ProviderProfile,
        entry: &CatalogEntry,
        source: &ReadySource,
    ) -> Result<(), &'static str> {
        let Some(archive) = &request.archive else {
            return Ok(());
        };
        // Rebind after asynchronous resolution; neither an edited profile nor a
        // replacement catalogue row may inherit the original archive authority.
        let (_, current_profile, current_entry, _) = self.prepare_play_request(request.clone())?;
        let now = archive_play::epoch_seconds(self.wall_clock_ms)?;
        archive.validate(
            &current_profile,
            current_entry.as_ref().unwrap_or(entry),
            now,
        )?;
        archive.validate(profile, entry, now)?;
        if !matches!(
            source,
            ReadySource::External(plx_media::ff::ExternalSource::Http(_), _, None)
        ) {
            return Err("Archive requires a direct HTTP source");
        }
        Ok(())
    }
    fn retire_result(&mut self, result: Result<(CatalogEntry, ReadySource), &'static str>) {
        if let Ok((_, ReadySource::External(_, _, Some(lease)))) = result {
            lease.begin_release();
            self.active_lease = Some(HeldLease::Cleanup(lease));
        }
    }
    fn launch_resolver(&mut self) -> Result<(), &'static str> {
        self.launch_resolver_checked(false)
    }
    fn launch_resolver_from_app(&mut self, app: &App) -> Result<(), &'static str> {
        if let Some(next) = &self.resolver_next {
            managed::guard_profile(self, app, &next.profile)?;
            classic_source::guard_profile(self, app, &next.profile)?;
        }
        self.launch_resolver_checked(true)
    }
    fn launch_resolver_checked(&mut self, managed_checked: bool) -> Result<(), &'static str> {
        // No-App continuation paths may stage a value, but must not start an
        // account-bearing lookup before the main-thread owner check.
        if !managed_checked
            && self.resolver_next.as_ref().is_some_and(|next| {
                managed::owns_retained_profile(self, &next.profile)
                    || classic_source::owns_id(self, &next.profile.id)
            })
        {
            return Ok(());
        }
        if self.resolver_next.is_some() && !self.pending_attachment_current() {
            return Err("M3U media attachment changed before lookup");
        }
        if !self.play_work_allowed() {
            return Ok(());
        }
        if self.resolver_busy {
            return Ok(());
        }
        if self.resolver_next.as_ref().is_some_and(|next| {
            !transient::lifecycle_target_ready(
                self,
                &next.profile,
                self.play_target
                    .as_ref()
                    .and_then(|request| request.local_selection.as_ref()),
            )
        }) {
            return Ok(());
        }
        let Some(request) = self.resolver_next.take() else {
            return Ok(());
        };
        let attachment = self.play_attachment.clone();
        let ResolveRequest {
            archive,
            generation,
            profile,
            entry,
            favorite,
            local_proof,
            cancel,
            plex,
            plex_choice,
            local_read,
        } = request;
        let mut reporter = preplay::worker(self, generation, &profile);
        let sender = self.tx.clone();
        let interactive = vportal_quality::interactive(self, generation);
        let preferred = vportal_quality::preference(&self.saved, &profile);
        let retired_quality = Arc::new(std::sync::Mutex::new(if plex.is_none() {
            std::mem::take(&mut self.quality.retired)
        } else {
            Vec::new()
        }));
        let worker_quality = retired_quality.clone();
        // Preparation only reads Plex metadata. An actual prepared choice is
        // a replacement: transfer the previous lease to this same worker so
        // its stop completion is drained before native Plex admission.
        let previous_lease = Arc::new(std::sync::Mutex::new(if plex.is_none() {
            self.active_lease.take()
        } else {
            None
        }));
        let worker_lease = previous_lease.clone();
        let started = plx_base::task::spawn_off_frame("ott-media", move |worker| {
            reporter.phase(preplay::Phase::Resolving);
            if let Some(work) = plex {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    transient::plex_remote::run(worker, &work)
                }))
                .unwrap_or(Err(super::ott_plex_queue::WireError::Queue(
                    super::ott_plex_queue::QueueError::Unavailable,
                )));
                let _ = sender.send(Job::PlexPrepared(work.owner, work.purpose.key(), result));
                plx_machine::idle::invalidate();
                return;
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let previous = worker_lease
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .take();
                let mut retired = std::mem::take(
                    &mut *worker_quality
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner()),
                );
                if let Some(lease) = previous {
                    retired.push(lease.into_lease());
                }
                super::ott_vportal_lease::finish_many(
                    worker,
                    retired,
                    std::time::Duration::from_secs(11),
                )?;
                if cancel.load(Ordering::Relaxed) {
                    return Err("Playback request cancelled");
                }
                let entry = match entry {
                    Some(entry) => entry,
                    None => {
                        let favorite = favorite.ok_or("Favorite is missing")?;
                        if let Some(receipt) = attachment.as_ref() {
                            ott_catalog::find_attachment_favorite(
                                worker, receipt, &favorite, &cancel,
                            )?
                        } else {
                            ott_catalog::find_favorite(
                                worker,
                                &profile,
                                &favorite,
                                local_proof.as_ref(),
                                &cancel,
                            )?
                        }
                    }
                };
                if let Some(archive) = &archive {
                    archive.validate_binding(&profile, &entry)?;
                }
                let ready = if let Some(choice) = plex_choice {
                    ReadySource::PreparedPlex(choice)
                } else if let MediaLocator::Plex {
                    server_id,
                    rating_key,
                } = &entry.media
                {
                    ReadySource::Plex {
                        server_id: server_id.clone(),
                        rating_key: rating_key.clone(),
                    }
                } else {
                    let (source, lease) = if matches!(entry.media, MediaLocator::Local { .. }) {
                        (
                            transient::open_video_source(
                                worker,
                                local_read.as_ref(),
                                &profile,
                                &entry,
                                &cancel,
                            )?,
                            None,
                        )
                    } else {
                        let resolved = if let Some(intent) = archive.as_ref().filter(|intent| intent.provider_route()) {
                            archive_play::resolve_provider(intent, &profile, &entry, worker, &cancel)?
                        } else if matches!(&entry.media, MediaLocator::VPortal { .. })
                            && archive.is_none()
                        {
                            let choices = ott_catalog::vportal_choices(
                                worker,
                                &profile,
                                &entry,
                                &cancel,
                                preferred.as_deref(),
                                interactive,
                            )?;
                            if interactive && choices.options.len() > 1 {
                                return Ok(ResolvedWork::Choices(entry, choices));
                            }
                            choices.choose(worker, None, &cancel)?
                        } else {
                            ott_catalog::resolve_typed(worker, &profile, &entry, &cancel)?
                        };
                        let resolved = match &archive {
                            Some(archive) if !archive.provider_route() => {
                                archive_play::resolved(archive, &profile, &entry, resolved, worker, &cancel)?
                            }
                            _ => resolved,
                        };
                        (
                            plx_media::ff::ExternalSource::Http(resolved.url),
                            resolved.lease,
                        )
                    };
                    if profile.kind == ProviderKind::M3u
                        && archive.is_none()
                        && preplay::is_hls_source(&source)
                    {
                        reporter.live_hls();
                    }
                    reporter.inspect(|| vportal_quality::inspect(source, lease, &cancel))?
                };
                if cancel.load(Ordering::Relaxed) {
                    if let ReadySource::External(_, _, Some(lease)) = ready {
                        lease.finish_off_frame()?;
                    }
                    return Err("Playback request cancelled");
                }
                Ok(ResolvedWork::Ready(entry, ready))
            }))
            .unwrap_or(Err("Media worker failed"));
            if result.is_err() {
                reporter.failed();
            }
            let job = match result {
                Ok(ResolvedWork::Choices(entry, choices)) => {
                    Job::VPortalPrepared(generation, profile, entry, choices)
                }
                result => Job::Play(
                    generation,
                    profile,
                    result.map(|ready| match ready {
                        ResolvedWork::Ready(entry, source) => (entry, source),
                        ResolvedWork::Choices(..) => unreachable!("choices handled above"),
                    }),
                ),
            };
            let _ = sender.send(job);
            plx_machine::idle::invalidate();
        });
        self.resolver_busy = started;
        if started {
            Ok(())
        } else {
            preplay::failed(self, generation, preplay::Failure::WorkerUnavailable);
            self.quality.retired.extend(std::mem::take(
                &mut *retired_quality
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()),
            ));
            if let Some(lease) = previous_lease
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .take()
            {
                self.active_lease = Some(lease);
            }
            Err("Could not open media worker")
        }
    }
}

fn ok(data: Value) -> Value {
    json!({"status":"ok", "data":data})
}
fn rejected(message: &str) -> Value {
    json!({"status":"rejected", "data":{"error":message}})
}

/// Current interactive catalog, including the one retained above Player.
pub(crate) fn current_ui_owner(app: &App) -> Option<InstanceId> {
    bridge::ott_ui_target(&app.pages)
}

pub(super) fn open_player_catalog(
    app: &mut App,
    request: plx_screens::registry::PlayerCatalogRequest,
) -> Result<(), &'static str> {
    let Some(mut runtime) = app.ott.take() else {
        return Err("Playback changed before continuation");
    };
    let result = player_browse::open(&mut runtime, app, &request);
    app.ott = Some(runtime);
    result
}

pub(super) fn open_player_settings(
    app: &mut App,
    request: plx_screens::registry::PlayerCatalogRequest,
) -> Result<(), &'static str> {
    let Some(mut runtime) = app.ott.take() else {
        return Err("Playback changed before opening Settings");
    };
    let result = menu_settings::open(&mut runtime, app, &request);
    if let Err(error) = result {
        runtime.status = error.into();
        runtime.changed();
    }
    app.ott = Some(runtime);
    result
}

/// Presentation only: the admitted OTT resolver runs before the media route owns a flight.
pub(super) fn player_startup_pending(app: &App) -> bool {
    player_startup::pending(app)
}

pub(super) fn pending_channel_display_stamp(app: &App) -> Option<(u64, u64, u64, bool)> {
    player_startup::display_stamp(app)
}
pub(super) fn pending_channel_display(app: &App) -> Option<plx_data::metadata::NowPlaying> {
    player_startup::display(app)
}

/// Publish source authority before UI dispatch; AppHost admits each event namespace separately.
pub(super) fn publish_remote_player_offer(app: &mut App) {
    if let Some(runtime) = app.ott.as_mut() { runtime.explicit_step.input_publication(); }
    let offer = app
        .ott
        .as_ref()
        .and_then(|runtime| player_remote::offer(runtime, app));
    app.bridge.publish_ott_player_offer(offer);
    let menu = app
        .ott
        .as_ref()
        .and_then(|runtime| playback_menu::offer(runtime, app));
    app.bridge.publish_playback_menu_offer(menu);
}

pub(super) fn player_menu_action(
    app: &mut App,
    request: plx_screens::registry::PlayerMenuRequest,
) -> Result<Option<plx_screens::registry::PlayerReq>, &'static str> {
    let result = if let Some(mut runtime) = app.ott.take() {
        let result = playback_menu::perform(&mut runtime, app, &request);
        app.ott = Some(runtime);
        result
    } else {
        Err("Playback changed; reopen the menu")
    };
    if let Err(reason) = &result {
        playback_menu::refused(app, &request, *reason);
    }
    result
}

pub(super) fn remote_player_action(
    app: &mut App,
    request: plx_screens::registry::OttPlayerRequest,
) -> Result<Option<plx_screens::registry::PlayerReq>, &'static str> {
    let Some(mut runtime) = app.ott.take() else {
        return Err("Playback changed before continuation");
    };
    let result = player_remote::perform(&mut runtime, app, &request);
    app.ott = Some(runtime);
    result
}

/// Refresh the existing presentation after a durable locale publication.
/// No provider, catalogue, draft or playback authority changes here.
pub(super) fn locale_changed(app: &mut App) {
    let Some(mut runtime) = app.ott.take() else {
        return;
    };
    runtime.changed();
    runtime.publish(app);
    app.ott = Some(runtime);
}

/// Apply Classic presentation only after the existing auth and preference owners settle.
pub(super) fn reconcile_classic_navigation(app: &mut App) -> bool {
    let Some(mut runtime) = app.ott.take() else {
        return false;
    };
    let changed = classic_navigation::reconcile(&mut runtime, app);
    app.ott = Some(runtime);
    changed
}

/// Presentation uses the receiver's monotonic clock; it never changes input ownership.
pub(super) fn notifications_need_frame(app: &App) -> bool {
    app.running
        && playback_allowed(app)
        && app.ott.as_ref().is_some_and(|runtime| {
            runtime
                .clock_sample
                .is_some_and(|sample| runtime.remote.notifications_need_frame(sample.unix_ms()))
        })
}
pub(super) fn draw_notifications(app: &mut App) {
    if !app.running || !playback_allowed(app) {
        return;
    }
    if let Some(runtime) = app.ott.as_mut() {
        let Some(now) = runtime.clock_sample.map(|sample| sample.unix_ms()) else {
            return;
        };
        let opacity = runtime.osd_preferences().0.background_alpha(0.70);
        runtime.remote.draw_notifications(now, opacity);
    }
}

/// Closed, current-native evidence only. Unknown/malformed/refused sources and
/// decoder failures are terminal; they cannot gain an automatic retry policy.
fn recoverable_live_transport(details: Option<plx_media::player::report::FailureDetails>) -> bool {
    use plx_media::player::report::IoFailureDetails;
    use plx_telemetry::telemetry::classes::{DemuxExitClass, TraceOutcome};
    matches!(details.and_then(|details| details.io_failure), Some(IoFailureDetails::Hls {
        exit: DemuxExitClass::HlsTransportOpenFailed | DemuxExitClass::HlsTransportFailed,
        outcome: Some(TraceOutcome::Transport | TraceOutcome::Deadline | TraceOutcome::ServerState),
    }))
}

pub(super) fn playback_allowed(app: &App) -> bool {
    app.window_activity.allow_present(true) && !app.ott_standby
}

pub(super) fn suspend(app: &mut App) {
    let Some(mut runtime) = app.ott.take() else {
        return;
    };
    runtime.suspend(app);
    app.ott = Some(runtime);
}

pub(super) fn resume(app: &mut App) {
    if !playback_allowed(app) {
        return;
    }
    if let Some(runtime) = app.ott.as_mut() {
        runtime.resume_source_work();
    }
}

/// Teardown has left the frame loop. Cancel producers, retain the last playhead,
/// and bound waiting for already admitted source cleanup with one allowance.
pub(super) fn shutdown(app: &mut App) {
    plx_appkit::menu_visibility::project(Default::default());
    shutdown_with_budget(app, std::time::Duration::from_secs(40));
}

fn shutdown_with_budget(app: &mut App, allowance: std::time::Duration) {
    use plx_base::task::time::{drain_for, ClockSample};
    use std::time::Duration;

    fn collect(runtime: &mut Runtime, leases: &mut Vec<super::ott_vportal_lease::Lease>, job: Job) {
        match job {
            Job::Parental(completion) => {
                runtime.retire_parental_completion(completion);
            }
            Job::Saved(result, ..) => {
                profile_rename::settle(runtime, result.as_ref().ok());
                runtime.saving = false;
                runtime.checkpoint_save_epoch = None;
                runtime.subtitle_save = None;
                if let Ok(saved) = result {
                    runtime.saved = saved;
                    public_preferences::overlay(&runtime.public_preferences, &mut runtime.saved);
                }
            }
            Job::PublicSaved(result, ..) => {
                runtime.saving = false;
                runtime.checkpoint_save_epoch = None;
                let sent = classic_bookmark::take_write(runtime);
                let success = result.is_ok();
                public_preferences::accept_write(runtime, result);
                if let Some(sent) = sent {
                    classic_bookmark::acknowledged(runtime, &sent.value, success);
                }
                public_preferences::overlay(&runtime.public_preferences, &mut runtime.saved);
            }
            Job::SaveRejected(..) => {
                profile_rename::settle(runtime, None);
                runtime.saving = false;
                runtime.checkpoint_save_epoch = None;
                runtime.subtitle_save = None;
            }
            Job::LoadedItemsUncertain(_, _) => {
                runtime.persistence_uncertain = true;
            }
            Job::Uncertain(..) => {
                profile_rename::settle(runtime, None);
                runtime.saving = false;
                runtime.checkpoint_save_epoch = None;
                runtime.subtitle_save = None;
                runtime.persistence_uncertain = true;
            }
            Job::FavoriteCommit(result) => favorites::finish(runtime, result),
            Job::FavoriteRecovery(result) => favorites_recovery::finish(runtime, result),
            Job::PlexPrepared(..) => {
                runtime.resolver_busy = false;
            }
            Job::VPortalPrepared(_, _, _, choices) => {
                runtime.resolver_busy = false;
                leases.extend(choices.retire());
            }
            Job::Play(_, _, result) => {
                runtime.resolver_busy = false;
                if let Ok((_, ReadySource::External(_, _, Some(lease)))) = result {
                    lease.begin_release();
                    leases.push(lease);
                }
            }
            _ => {}
        }
    }

    let Some(mut runtime) = app.ott.take() else {
        return;
    };
    // Release overlay textures on the owning thread while the GL context is
    // still alive, and cancel ephemeral feed/media before the bounded drain.
    runtime.remote.suspend_notifications();
    // This sample belongs to post-loop IO, not to a machine step. Projection
    // through steady_from keeps wall-clock adjustments from extending cleanup.
    let started = ClockSample::capture_main(app.adapters.player.mt()).ok();
    Runtime::withdraw_reminder_ui(app);
    runtime.reminders.invalidate();
    hls_tracks::shutdown(&mut runtime, app);
    runtime.reminder_save = None;
    runtime.consume_exit(app);
    runtime.checkpoint(app, false);
    let classic_bookmark = classic_bookmark::shutdown_candidate(&runtime, app);
    // Stop native audio, reads and pending loads before waiting for remote
    // cleanup. Final firmware/SDL teardown remains in run::shutdown.
    playback::replace_player(
        &mut app.player,
        &mut app.adapters.player,
        &mut app.repause_at,
        &mut app.refresh_hubs,
        &mut app.pages,
    );
    transient::plex_remote::cancel_shutdown(&runtime);
    runtime.cancel_play_request();
    runtime.cancel_catalog();
    let mut leases = std::mem::take(&mut runtime.quality.retired);
    if let Some(lease) = runtime.active_lease.take() {
        lease.begin_release();
        leases.push(lease.into_lease());
    }
    let remaining = || {
        started
            .and_then(|start| {
                ClockSample::capture_main(app.adapters.player.mt())
                    .ok()
                    .map(|now| {
                        allowance.saturating_sub(Duration::from_millis(
                            now.steady_from(start)
                                .unix_ms()
                                .saturating_sub(start.unix_ms()),
                        ))
                    })
            })
            .unwrap_or(Duration::ZERO)
    };
    let rx = std::mem::replace(&mut runtime.rx, mpsc::channel().1);
    // A completed resolver may already be queued even when no state write is
    // outstanding. Collect it before dropping the receiver and its lease.
    while let Ok(job) = rx.try_recv() {
        collect(&mut runtime, &mut leases, job);
    }
    if runtime.saving {
        drain_for(&rx, remaining().min(Duration::from_secs(3)), |job| {
            collect(&mut runtime, &mut leases, job);
            runtime.saving
        });
    }
    if runtime.resolver_busy {
        drain_for(&rx, remaining(), |job| {
            collect(&mut runtime, &mut leases, job);
            runtime.resolver_busy
        });
    }
    while let Ok(job) = rx.try_recv() {
        collect(&mut runtime, &mut leases, job);
    }
    if runtime.resolver_busy {
        plx_base::eventlog::log("ott: shutdown source cleanup deadline expired");
    }
    if runtime.loaded
        && !runtime.saving
        && !runtime.persistence_uncertain
        && !runtime
            .saved
            .receipts
            .iter()
            .any(|receipt| receipt.outcome == plx_data::ott::favorites_wire::Outcome::Unknown)
    {
        let mut saved = runtime.saved.clone();
        transient::revoke_unavailable_proofs(&runtime, &mut saved);
        kiosk_resume::merge(&mut saved, &runtime.checkpoints);
        kiosk_resume::sanitize(&mut saved);
        let _ = browse_focus::prepare(&runtime, &mut saved);
        let _ = favorites::write_verified(&saved, &runtime.write_permit);
    }
    // This UI-only patch shares the existing writer and CAS door. Never race an
    // outstanding writer or retry an uncertain public transaction at teardown.
    if let Some(value) = classic_bookmark.filter(|_| !remaining().is_zero()) {
        if public_preferences::write_refusal(&runtime).is_none()
            && public_preferences::classic_bookmark(&runtime.public_preferences, &value.source)
                != Some(Some(value.clone()))
        {
            let result = public_preferences::write(
                public_preferences::Patch::ClassicBookmark(value),
                &runtime.write_permit,
            );
            public_preferences::accept_write(&mut runtime, result);
        }
    }
    for lease in leases {
        if lease.finish_after_loop(remaining()).is_err() {
            plx_base::eventlog::log(
                "ott: shutdown session stop did not finish within its allowance",
            );
        }
    }
}

/// Synchronous native-input handoff, after the media performer accepted it.
pub(super) fn viewer_transport(app: &mut App) {
    viewer_action(app, playback::ViewerTransport::Transport);
}

/// Preserve the performer's distinction between accepted seek and pause/play.
pub(super) fn viewer_action(app: &mut App, action: playback::ViewerTransport) {
    if let Some(mut runtime) = app.ott.take() {
        match action {
            playback::ViewerTransport::None => {}
            playback::ViewerTransport::Transport => runtime.viewer_transport(app),
            playback::ViewerTransport::Seek => runtime.viewer_seek(app),
        }
        app.ott = Some(runtime);
    }
}

pub(super) fn track_request(
    app: &mut App,
    request: plx_screens::registry::PlayerReq,
) -> Option<plx_screens::registry::PlayerReq> {
    hls_tracks::player_request(app, request)
        .and_then(|request| subtitle_settings::player_request(app, request))
}

/// Trusted developer commands use the same persistent-setting owner as the UI.
pub(super) fn commit_track(app: &mut App, commit: plx_appkit::track_menu::TrackCommit) {
    if let Err(commit) = hls_tracks::trusted_commit(app, commit) {
        subtitle_settings::trusted_commit(app, commit);
    }
}

/// Validate queued remote track work before the native pump consumes it. The
/// runner supplies a fresh clock only while a request is pending; no new timer.
pub(super) fn before_playback_tick(app: &mut App) {
    if !app
        .ott
        .as_ref()
        .is_some_and(commands::track_request_pending)
    {
        return;
    }
    let Some(mut runtime) = app.ott.take() else {
        return;
    };
    let now = plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt())
        .ok()
        .zip(runtime.clock_anchor)
        .map(|(sample, anchor)| sample.steady_from(anchor).unix_ms());
    commands::before_playback_tick(&mut runtime, app, now);
    app.ott = Some(runtime);
}

pub(super) fn tick(app: &mut App, clock_sample: plx_base::task::time::ClockSample) {
    tick_with_start(app, clock_sample, |runtime, app, profile, entry, source| {
        runtime.start_resolved_with_kiosk(app, profile, entry, source)
    });
}

// Keep the complete Job admission path shared. Tests substitute only the final
// decoder resource preparation, never request validation or Favorite evidence.
fn tick_with_start(
    app: &mut App,
    clock_sample: plx_base::task::time::ClockSample,
    mut start: impl FnMut(
        &mut Runtime,
        &mut App,
        ProviderProfile,
        CatalogEntry,
        ReadySource,
    ) -> Result<(), &'static str>,
) {
    let programme_changed = observe_programme(app, clock_sample.unix_ms());
    let Some(mut runtime) = app.ott.take() else {
        app.bridge.publish_ott_catalog_offer(None);
        plx_appkit::menu_visibility::project(Default::default());
        return;
    };
    let anchor = *runtime.clock_anchor.get_or_insert(clock_sample);
    runtime.wall_clock_ms = clock_sample.unix_ms();
    runtime.clock_sample = Some(clock_sample.steady_from(anchor));
    parental_runtime::tick(&mut runtime, app);
    let before = runtime.revision;
    runtime.sync_source_lifecycle(app);
    classic_source::reconcile(&mut runtime, app);
    runtime.consume_exit(app);
    runtime.reconcile_owner(app);
    runtime.reconcile_m3u_attachment(app);
    if app.bridge.ui_font_scale_percent() != 100 {
        app.rec
            .refuse("Nondefault UI typography is not bound in this recording format");
    }
    hls_tracks::tick(&mut runtime, app);
    // The cooldown may already have issued a resolver request. Accepted Pause
    // still wins over its late completion, without cancelling a newer explicit
    // Play (queue admission clears this exact-generation receipt).
    if runtime.live_retry_generation == Some(runtime.play_generation)
        && lifecycle::viewer_paused()
    {
        runtime.cancel_play_request();
        runtime.continuity.pause(runtime.generation);
        runtime.status.clear();
        runtime.changed();
    }
    while let Ok(job) = runtime.rx.try_recv() {
        if matches!(
            &job,
            Job::Play(..) | Job::PlexPrepared(..) | Job::VPortalPrepared(..)
        ) {
            runtime.resolver_busy = false;
        }
        let job = match job {
            Job::LoadedItemsUncertain(saved, entropy) => {
                runtime.retire_profile_edits();
                runtime.persistence_uncertain = true;
                Job::Loaded(Ok((saved, entropy)))
            }
            other => other,
        };
        match job {
            Job::Preplay(event) => preplay::receive(&mut runtime, event),
            Job::LoadedItemsUncertain(_, _) => unreachable!("load uncertainty normalized above"),
            Job::PublicLoaded(result) => public_preferences::loaded(&mut runtime, app, result),
            Job::PublicSaved(result, invocation, response) => {
                public_preferences::saved(&mut runtime, app, result, invocation, response)
            }
            Job::HlsTrack(completion) => hls_tracks::completed(&mut runtime, app, completion),
            Job::Parental(completion) => parental_runtime::completed(&mut runtime, app, completion),
            Job::Loaded(result) => {
                runtime.kiosk_capture.cancel_unwritten();
                runtime.pending_category_choice = None;
                runtime.pending_playing_favorite = None;
                runtime.state_load = StateLoad::Idle;
                let browse = (runtime.page, runtime.provider);
                match result {
                    Ok((saved, entropy)) => {
                        // Entropy is filled once and never cleared: this marks the
                        // first decoded image, not permission to open any source.
                        let first_loaded_image = runtime.entropy.is_none();
                        runtime.entropy.get_or_insert(entropy);
                        runtime.reconcile_kiosk_policy(&saved);
                        runtime.observe_profile_edits(&saved);
                        runtime.saved = saved;
                        public_preferences::overlay(
                            &runtime.public_preferences,
                            &mut runtime.saved,
                        );
                        transient::local_save_finished(&mut runtime, true);
                        runtime.sync_saved_context();
                        runtime.loaded = true;
                        runtime.parental_authoritative_load();
                        runtime.status.clear();
                        if let Err(error) = runtime.load_reminders(app) {
                            runtime.status = error.into();
                        }
                        if let Err(error) = commands::settings_after_saved(&mut runtime, app) {
                            runtime.status = error.into();
                        }
                        if let Err(error) = runtime
                            .saved
                            .plex_sources()
                            .and_then(|sources| super::ott_import::set_authorized_sources(&sources))
                        {
                            runtime.loaded = false;
                            runtime.state_load = StateLoad::Failed(error);
                            runtime.status = error.into();
                            let _ = super::ott_import::set_authorized_sources(&[]);
                        } else {
                            runtime.plex_refresh_requested = true;
                            let kiosk_startup = runtime.stage_kiosk_startup(app);
                            if first_loaded_image {
                                runtime.arm_initial_usb_restore(app);
                            }
                            if runtime.active().is_some() {
                                if runtime.navigation_epoch == 0 {
                                    runtime.load_catalog();
                                } else {
                                    runtime.continuity.suppress_unrequested_start();
                                    runtime.refresh_catalog_preserving_playback();
                                }
                            } else if runtime.navigation_epoch == 0
                                && runtime.saved.favorites_selected
                            {
                                // Global Favorites is a browse preference even when no
                                // persistent profile is active. Targets still require
                                // their normal source binding when explicitly played.
                                runtime.page = "favorites";
                            }
                            // A denied startup must not regain playback authority
                            // through ordinary first-row catalogue continuity.
                            if matches!(
                                runtime.track_selection_kiosk(),
                                Ok(super::ott_kiosk_policy::State::Locked { .. })
                            ) {
                                runtime.continuity.suppress_unrequested_start();
                            }
                            if !kiosk_startup {
                                if let Err(error) =
                                    kiosk_source::stage_startup_item(&mut runtime, app)
                                {
                                    runtime.status = error.message().into();
                                }
                            }
                        }
                    }
                    Err(error) => {
                        runtime.reminders.invalidate();
                        Runtime::withdraw_reminder_ui(app);
                        runtime.loaded = false;
                        runtime.state_load = StateLoad::Failed(error);
                        runtime.status = error.into();
                    }
                }
                // Initial restoration has browse authority only before the
                // first accepted navigation. A retry cannot reclaim it.
                if runtime.navigation_epoch != 0 {
                    (runtime.page, runtime.provider) = browse;
                }
                runtime.changed();
            }
            job @ (Job::Saved(..) | Job::SaveRejected(..)) => {
                use plx_appkit::track_menu::{TrackCommitFailure, TrackCommitOutcome};
                use std::borrow::Cow;
                // Both outcomes settle the same admitted write and addressed editor.
                // Only the new diagnostic carries owned text; legacy static errors stay borrowed.
                let (result, invocation, reload, response) = match job {
                    Job::Saved(result, invocation, reload, response) => {
                        (result.map_err(Cow::Borrowed), invocation, reload, response)
                    }
                    Job::SaveRejected(error, invocation, reload, response) => (
                        Err(Cow::Owned(save_error::message(error))),
                        invocation,
                        reload,
                        response,
                    ),
                    _ => unreachable!("matched settings write completion"),
                };
                let provider_settings_activation = commands::provider_settings_before_saved(&mut runtime, invocation.as_ref());
                profile_rename::settle(&mut runtime, result.as_ref().ok());
                let profile_saved_only =
                    runtime.finish_profile_edit(result.as_ref().ok(), invocation.as_ref(), reload);
                let mut local_profile_save = local_profile::take_saved(
                    &mut runtime,
                    app,
                    result.as_ref().ok(),
                    invocation.as_ref(),
                );
                let browse = (runtime.page, runtime.provider);
                let navigation =
                    std::mem::replace(&mut runtime.save_navigation, SaveNavigation::Preserve);
                let ui_owner = runtime.save_ui_owner.take();
                let source_editor = runtime.save_source_editor.take();
                // A UI-owned write remains durable after its screen leaves, but
                // cannot activate playback from a retired or departing instance.
                // Remote writes have no UI owner and retain their own authority.
                let owner_current = ui_owner.is_none_or(|owner| {
                    current_ui_owner(app) == Some(owner) && runtime.reply == Some(owner)
                });
                let may_navigate = owner_current
                    && matches!(navigation,
                    SaveNavigation::Activate(epoch) if epoch == runtime.navigation_epoch);
                runtime.saving = false;
                runtime.checkpoint_save_epoch = None;
                let subtitle_save = runtime.subtitle_save.take();
                let publish_subtitles = subtitle_save
                    .as_ref()
                    .is_some_and(|pending| pending.can_publish(app));
                let mut subtitle_outcome = TrackCommitOutcome::Saved;
                let restore_allowed =
                    may_navigate && runtime.save_playback_epoch == runtime.playback_epoch && provider_settings_activation;
                let usb_install_pending = runtime.usb_install_pending();
                let usb_activation_ready = result.as_ref().is_ok_and(|candidate| {
                    owner_current
                        && runtime.save_playback_epoch == runtime.playback_epoch
                        && runtime.usb_save_ready(candidate)
                });
                let restore_allowed = restore_allowed
                    && (!usb_install_pending || usb_activation_ready)
                    && local_profile_save
                        .as_ref()
                        .is_none_or(|staged| staged.ready);
                let mut editor_error = None;
                let setup_persisted = result.is_ok();
                let capture_write = invocation.is_none() && runtime.kiosk_capture.pending();
                let kiosk_target_changed = result.as_ref().ok().is_some_and(|saved| {
                    kiosk_source::policy_target_changed(&runtime.saved.settings, &saved.settings)
                });
                if kiosk_target_changed {
                    runtime.kiosk_capture.cancel_unwritten();
                }
                let mut kiosk_settings_applied = false;
                let usb_preserve_browse = usb_install_pending && runtime.usb_automatic_install();
                let mut usb_startup = None;
                let reminder_error = runtime.finish_reminders(
                    app,
                    result
                        .as_ref()
                        .map_err(|_| reminders::SaveFailure::Rejected),
                );
                let response = match result {
                    Ok(saved) => {
                        let next = saved
                            .profiles
                            .profiles
                            .iter()
                            .find(|p| Some(&p.id) == saved.profiles.active_profile.as_ref());
                        let source_changed = runtime.active() != next;
                        let usb_ready = usb_activation_ready;
                        if usb_ready {
                            usb_startup = runtime.usb_startup_snapshot(app);
                        }
                        if (source_changed && restore_allowed) || usb_ready {
                            runtime.invalidate_source(app);
                        }
                        let folder_scope_changed =
                            runtime.saved.settings.get(super::ott_kiosk_stored::KEY)
                                != saved.settings.get(super::ott_kiosk_stored::KEY)
                                && !kiosk_source::strict_folder_traversal(&runtime);
                        runtime.reconcile_kiosk_policy(&saved);
                        runtime.retire_changed_metadata_source(&saved);
                        runtime.retire_changed_archive_fallback(&saved);
                        runtime.observe_profile_edits(&saved);
                        runtime.saved = saved;
                        public_preferences::overlay(
                            &runtime.public_preferences,
                            &mut runtime.saved,
                        );
                        if folder_scope_changed && kiosk_source::strict_folder_traversal(&runtime) {
                            // Favorites may have completed the previous selection while this
                            // folder was still scanning. Retain every playback/retry intent.
                            runtime.continuity.selection_domain_changed(
                                runtime.generation,
                                runtime.catalog_complete,
                            );
                        }
                        runtime.usb_save_finished(usb_ready);
                        kiosk_resume::reconciled(&mut runtime);
                        runtime.parental_confirmed();
                        runtime.sync_saved_context();
                        player_browse::reconcile_strict_presentation(&mut runtime, app);
                        runtime.status.clear();
                        let applied = if subtitle_save.is_some() {
                            // This transaction changed only native subtitle preferences.
                            // Preserve other settings and a replacement source's live values.
                            subtitle_settings::Stored::read(&runtime.saved).map(|stored| {
                                if publish_subtitles {
                                    stored.apply(app, false);
                                }
                            })
                        } else {
                            runtime
                                .saved
                                .plex_sources()
                                .and_then(|sources| {
                                    super::ott_import::set_authorized_sources(&sources)
                                })
                                .and_then(|_| {
                                    runtime.verified_imports.drain(..).try_for_each(|source| {
                                        super::ott_import::register_verified(&source).map(|_| ())
                                    })
                                })
                                .and_then(|_| commands::settings_after_saved(&mut runtime, app))
                        };
                        let applied = applied.and_then(|()| {
                            local_profile::publish(&mut runtime, &mut local_profile_save)
                        });
                        kiosk_settings_applied = applied.is_ok();
                        if let Err(error) = applied {
                            subtitle_outcome =
                                TrackCommitOutcome::Rejected(TrackCommitFailure::Unavailable);
                            runtime.verified_imports.clear();
                            runtime.status = error.into();
                            editor_error =
                                Some(format!("Saved, but could not apply settings: {error}"));
                            json!({"status":"rejected","data":{"saved":true,"reloaded":false,"error":error}})
                        } else {
                            if restore_allowed {
                                if let Some(capture) = source_editor {
                                    runtime.retire_source_editor(app, capture, navigation);
                                }
                            }
                            if !profile_saved_only
                                && provider_settings_activation
                                && (reload || source_changed)
                                && (!usb_install_pending || usb_activation_ready)
                            {
                                if restore_allowed {
                                    transient::cancel_play_candidate(&mut runtime, app);
                                    runtime.cancel_play_request();
                                    runtime.load_catalog();
                                } else {
                                    if source_changed {
                                        // IDs in the old catalogue cannot be
                                        // resumed or advanced in a new profile.
                                        // Keep the decoder and already-bound
                                        // resolver; retire only unbound policy.
                                        runtime.continuity.stop(runtime.generation);
                                        runtime.resume_pending = None;
                                    } else {
                                        runtime.continuity.suppress_unrequested_start();
                                    }
                                    // A newly saved profile is authoritative. A data-only
                                    // refresh may retain the temporary managed catalogue.
                                    runtime
                                        .refresh_catalog_preserving_playback_impl(!source_changed);
                                }
                            }
                            response
                        }
                    }
                    Err(error) => {
                        runtime.usb_save_failed();
                        subtitle_outcome =
                            TrackCommitOutcome::Rejected(TrackCommitFailure::SaveFailed);
                        runtime.verified_imports.clear();
                        runtime.status = error.to_string();
                        let response = rejected(error.as_ref());
                        editor_error = Some(error.into_owned());
                        response
                    }
                };
                local_profile::finish(&mut runtime, local_profile_save);
                if let Some(startup) = usb_startup {
                    runtime.finish_usb_startup(app, startup);
                }
                if !may_navigate || usb_preserve_browse {
                    (runtime.page, runtime.provider) = browse;
                }
                if capture_write {
                    kiosk_capture::settled(
                        &mut runtime,
                        app,
                        if setup_persisted {
                            kiosk_capture::Completion::Durable
                        } else {
                            kiosk_capture::Completion::NotApplied
                        },
                    );
                } else if setup_persisted
                    && kiosk_target_changed
                    && !usb_install_pending
                    && !invocation.as_ref().is_some_and(|inv| inv.action == "kiosk")
                {
                    kiosk_source::apply_durable_policy(&mut runtime, app);
                }
                transient::local_save_finished(&mut runtime, setup_persisted);
                runtime.complete_setup(editor_error.is_none(), setup_persisted);
                settings_groups::settled(
                    &mut runtime,
                    app,
                    setup_persisted,
                    editor_error.is_none(),
                );
                let category_saved = setup_persisted && editor_error.is_none();
                let category_allowed = invocation
                    .as_ref()
                    .is_none_or(|inv| commands::category_select_completion_allowed(&runtime, inv));
                let category_applied =
                    runtime.finish_category_choice(app, category_saved && category_allowed);
                playing_favorite::settled(&mut runtime, app, setup_persisted);
                runtime.finish_organization_save(app, setup_persisted);
                if let Some(session) = runtime.pending_editor_save.take() {
                    runtime.complete_profile_editor(&session, setup_persisted);
                    let editor_error =
                        remote_settings::after_saved(&mut runtime, app, &session, editor_error);
                    runtime.editor_result(app, &session, editor_error);
                }
                if let Some(invocation) = invocation {
                    // Kiosk settlement captures activation freshness before its
                    // own enforcement can retire the outgoing playback epoch.
                    let kiosk_outcome = if setup_persisted {
                        commands::KioskSettlement::Saved {
                            settings_applied: kiosk_settings_applied,
                            target_changed: kiosk_target_changed,
                        }
                    } else {
                        commands::KioskSettlement::NotApplied
                    };
                    let response =
                        commands::kiosk_settled(&mut runtime, app, &invocation, kiosk_outcome)
                            .unwrap_or(response);
                    let response = transient::connection_saved(
                        &mut runtime,
                        app,
                        &invocation,
                        response["status"] == "ok",
                    )
                    .unwrap_or(response);
                    let response = commands::category_select_settled(
                        &runtime,
                        &invocation,
                        category_saved,
                        category_applied,
                        response,
                    );
                    let response = if response["status"] != "ok" {
                        commands::settings_failure(
                            &invocation,
                            if setup_persisted {
                                "effect_failed"
                            } else {
                                "storage_failed"
                            },
                        )
                        .unwrap_or(response)
                    } else {
                        response
                    };
                    let response = commands::provider_settings_settled(&mut runtime, &invocation,
                        setup_persisted, kiosk_settings_applied).unwrap_or(response);
                    runtime.remote.complete(invocation, response);
                }
                if let Some(pending) = subtitle_save {
                    pending.finish(app, subtitle_outcome);
                }
                vportal_quality::settled(
                    &mut runtime,
                    app,
                    setup_persisted && kiosk_settings_applied,
                );
                if let Some(error) = reminder_error {
                    runtime.status = error.into();
                }
                runtime.changed();
            }
            Job::Uncertain(visible, invocation) => {
                profile_rename::settle(&mut runtime, None);
                runtime.finish_profile_edit(None, invocation.as_ref(), false);
                settings_groups::uncertain(&mut runtime);
                runtime.retire_profile_edits();
                local_profile::uncertain(&mut runtime, invocation.as_ref());
                runtime.kiosk_capture.cancel_unwritten();
                let capture_write = invocation.is_none() && runtime.kiosk_capture.pending();
                runtime.parental_uncertain();
                let _ = runtime.finish_reminders(app, Err(reminders::SaveFailure::Unknown));
                runtime.reminders.invalidate();
                Runtime::withdraw_reminder_ui(app);
                runtime.m3u_attachments.invalidate_all();
                runtime.save_navigation = SaveNavigation::Preserve;
                runtime.save_ui_owner = None;
                runtime.save_source_editor = None;
                runtime.saving = false;
                runtime.checkpoint_save_epoch = None;
                let subtitle_save = runtime.subtitle_save.take();
                runtime.persistence_uncertain = true;
                runtime.usb_save_finished(false);
                runtime.invalidate_source(app);
                runtime.loaded = false;
                let _ = super::ott_import::set_authorized_sources(&[]);
                runtime.verified_imports.clear();
                if let Some(saved) = visible {
                    // A visible but unverified image has no browse authority.
                    runtime.saved = saved;
                    public_preferences::overlay(&runtime.public_preferences, &mut runtime.saved);
                }
                if capture_write {
                    kiosk_capture::settled(&mut runtime, app, kiosk_capture::Completion::Unknown);
                }
                runtime.status = "Settings write needs reconciliation; restart the player".into();
                runtime.complete_setup(false, false);
                runtime.finish_category_choice(app, false);
                playing_favorite::settled(&mut runtime, app, false);
                runtime.finish_organization_save(app, false);
                if let Some(session) = runtime.pending_editor_save.take() {
                    let error = runtime.status.clone();
                    runtime.editor_result(app, &session, Some(error));
                }
                if let Some(invocation) = invocation {
                    let response =
                        commands::category_select_rejection(&runtime, &invocation, "unavailable")
                            .or_else(|| {
                                commands::kiosk_settled(
                                    &mut runtime,
                                    app,
                                    &invocation,
                                    commands::KioskSettlement::Unknown,
                                )
                            })
                            .or_else(|| commands::provider_settings_uncertain(&mut runtime, &invocation))
                            .or_else(|| commands::settings_failure(&invocation, "storage_failed"))
                            .unwrap_or_else(|| rejected(&runtime.status));
                    runtime.remote.complete(invocation, response);
                }
                if let Some(pending) = subtitle_save {
                    pending.finish(
                        app,
                        plx_appkit::track_menu::TrackCommitOutcome::Rejected(
                            plx_appkit::track_menu::TrackCommitFailure::SaveUncertain,
                        ),
                    );
                }
                runtime.changed();
            }
            Job::PlexRestored(cancel, sources) => {
                if !runtime.accept_plex_restore(&cancel) {
                    continue;
                }
                let mut restored = false;
                let mut failure = None;
                for source in sources {
                    match source.and_then(|source| super::ott_import::register_verified(&source)) {
                        Ok(_) => restored = true,
                        Err(error) => failure = Some(error),
                    }
                }
                if restored
                    && runtime
                        .active()
                        .is_some_and(|p| p.kind == ProviderKind::Plex)
                {
                    // A startup scan may have raced source restoration. Replace
                    // it through the bounded catalogue queue, even while loading.
                    runtime.refresh_catalog_preserving_playback();
                }
                if let Some(error) = failure {
                    if runtime
                        .active()
                        .is_some_and(|p| p.kind == ProviderKind::Plex)
                    {
                        runtime.status = error.into();
                        runtime.changed();
                    }
                }
            }
            Job::PlexVerified(invocation, result) => {
                let navigation =
                    std::mem::replace(&mut runtime.save_navigation, SaveNavigation::Preserve);
                let playback_epoch = runtime.save_playback_epoch;
                let ui_owner = runtime.save_ui_owner.take();
                let source_editor = runtime.save_source_editor.take();
                runtime.saving = false;
                runtime.checkpoint_save_epoch = None;
                let prepared = (|| {
                    let verified = result?;
                    if invocation.deadline_ms <= runtime.now_ms()
                        || !runtime.remote.invocation_is_current(&invocation)
                    {
                        return Err("Plex configuration request expired");
                    }
                    let mut saved = runtime.saved.clone();
                    let mut sources: Vec<plx_plex::plex::session::SourceRef> = saved
                        .settings
                        .get("nativePlexSources")
                        .cloned()
                        .and_then(|value| serde_json::from_value(value).ok())
                        .unwrap_or_default();
                    sources.retain(|source| source.machine_id != verified.source.machine_id);
                    sources.push(verified.source.clone());
                    if sources.len() > 15 {
                        return Err("Too many stored Plex sources");
                    }
                    saved.settings.insert(
                        "nativePlexSources".into(),
                        serde_json::to_value(sources).map_err(|_| "Could not save Plex source")?,
                    );
                    let id = format!(
                        "plex-import-{}",
                        &plx_data::ott::favorites_wire::hash(verified.source.machine_id.as_bytes())
                            [..16]
                    );
                    if let Some(profile) = saved.profiles.profiles.iter_mut().find(|p| {
                        p.kind == ProviderKind::Plex && p.source == verified.source.machine_id
                    }) {
                        profile.name = verified.source.name.clone();
                    } else {
                        saved.profiles.profiles.push(ProviderProfile {
                            id: id.clone(),
                            name: verified.source.name.clone(),
                            kind: ProviderKind::Plex,
                            source: verified.source.machine_id.clone(),
                            source_alias: String::new(),
                            enabled: true,
                        });
                    }
                    saved.profiles.active_profile = saved
                        .profiles
                        .profiles
                        .iter()
                        .find(|p| {
                            p.kind == ProviderKind::Plex && p.source == verified.source.machine_id
                        })
                        .map(|p| p.id.clone());

                    let fields = invocation.params["settings"]
                        .as_object()
                        .ok_or("Invalid Plex settings request")?
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>();
                    runtime.save_reply(
                        saved,
                        invocation.clone(),
                        true,
                        ok(json!({"provider":"plex","saved":true,"fields":fields,"reloaded":true})),
                    )?;
                    // Verification and persistence are one intent. Do not let
                    // an old verifier borrow newer browse or playback epochs.
                    runtime.save_navigation = navigation;
                    runtime.save_playback_epoch = playback_epoch;
                    runtime.save_ui_owner = ui_owner;
                    runtime.save_source_editor = source_editor;
                    runtime.verified_imports.push(verified);
                    Ok(())
                })();
                if let Err(error) = prepared {
                    runtime.remote.complete(invocation, rejected(error));
                    runtime.status = error.into();
                    runtime.changed();
                }
            }
            Job::VPortalPrepared(generation, profile, entry, choices) => {
                vportal_quality::received(&mut runtime, app, generation, profile, entry, choices);
                runtime.changed();
            }
            Job::Play(generation, profile, result) if generation == runtime.play_generation => {
                let resolved = result.is_ok();
                vportal_quality::completed_resolution(&mut runtime, generation);
                let automatic_resume = runtime.restore_play_generation == Some(generation);
                runtime.restore_play_generation = None;
                let superseded = runtime
                    .play_ticket
                    .take()
                    .is_some_and(|ticket| !plx_media::route::play_request_is_current(ticket));
                let expired = runtime.pending_play.as_ref().is_some_and(|request| {
                    request.deadline_ms <= runtime.now_ms()
                        || !runtime.remote.invocation_is_current(request)
                });
                let exact_catalog_request = runtime
                    .play_target
                    .as_ref()
                    .is_some_and(|request| !request.advance_on_failure);
                let items_error = favorites_items::pending_error(&runtime)
                    .or_else(|| (!playback_allowed(app)).then_some("policy_denied"));
                let mut native_start = None;
                let response = if superseded {
                    preplay::cancel(&mut runtime, preplay::CancelReason::Superseded);
                    // A native retry also advances admission. Cancel the stale
                    // request without abandoning an unchanged owned source.
                    let retained = runtime
                        .playing
                        .as_ref()
                        .filter(|playing| {
                            playing_matches(playing, app)
                                && (!exact_catalog_request
                                    || !playing.requires_server_lease
                                    || runtime
                                        .active_lease
                                        .as_ref()
                                        .is_some_and(HeldLease::is_playback))
                        })
                        .map(|playing| playing.selection_id.clone());
                    if let Some(id) = retained {
                        commands::cancel_pending_play(&mut runtime);
                        runtime.cancel_play_request();
                        runtime.resume_pending = None;
                        runtime.continuity.retain_selection(runtime.generation, &id);
                    } else {
                        runtime.stop_catalog(app);
                    }
                    runtime.retire_result(result);
                    Err("Playback request superseded")
                } else if !runtime.pending_attachment_current() {
                    runtime.retire_result(result);
                    Err("M3U media attachment changed before playback")
                } else if expired {
                    preplay::cancel(&mut runtime, preplay::CancelReason::Expired);
                    runtime.retire_result(result);
                    Err("Playback request expired")
                } else if items_error.is_some()
                    && runtime
                        .pending_play
                        .as_ref()
                        .is_some_and(|inv| inv.action == favorites_items::ACTION)
                {
                    runtime.retire_result(result);
                    Err("Favorite selection changed before playback")
                } else if !commands::pending_catalog_current(&runtime) {
                    runtime.retire_result(result);
                    Err("Catalog changed before playback admission")
                } else if !transient::target_current(&runtime)
                    || transient::before_native(&mut runtime, app).is_err()
                {
                    runtime.retire_result(result);
                    Err("Local playback admission expired or changed")
                } else if !playback_allowed(app) {
                    runtime.retire_result(result);
                    Err("Playback is suspended")
                } else {
                    result.and_then(|(entry, source)| {
                        preplay::phase(&mut runtime, generation, preplay::Phase::Admitting);
                        if let Err(error) = runtime.rebind_native_admission(app, &profile, &entry) {
                            runtime.retire_result(Ok((entry, source)));
                            return Err(error);
                        }
                        // The successful Job and fresh admission binding attest this
                        // exact Favorite. A selection ID alone never creates proof.
                        let resolved_capture = runtime
                            .play_target
                            .clone()
                            .and_then(|request| runtime.prepare_play_request(request).ok())
                            .and_then(|(_, _, direct, favorite)| {
                                direct.is_none().then_some(favorite).flatten()
                            })
                            .and_then(|favorite| {
                                selection::ChannelCapture::resolved(favorite, &entry)
                            });
                        let before = plx_media::route::play_request_ticket();
                        // Carry the resolver's reason only through synchronous
                        // admission. Pre-admission refusal keeps its old policy.
                        runtime.restore_play_generation = automatic_resume.then_some(generation);
                        let result = start(&mut runtime, app, profile, entry, source);
                        runtime.restore_play_generation = None;
                        if result.is_ok() {
                            if let Some(playing) = runtime.playing.as_mut() {
                                if playing.channel_capture.is_none() {
                                    playing.channel_capture = resolved_capture;
                                }
                            }
                        }
                        native_start = Some(NativeStartReceipt {
                            generation,
                            before,
                            after: plx_media::route::play_request_ticket(),
                        });
                        result
                    })
                };
                let response_failed = response.is_err();
                classic_play_handoff::completed(&mut runtime, generation, !response_failed);
                let refused_step = response_failed && !resolved && !expired && !superseded
                    && playback_allowed(app) && commands::pending_catalog_current(&runtime)
                    && runtime.pending_attachment_current() && transient::target_current(&runtime);
                playback_step::resolved(&mut runtime, app, generation, refused_step);
                // Observe this attempt before failure policy can retire it or queue a successor.
                if response_failed {
                    preplay::failed(
                        &mut runtime,
                        generation,
                        if resolved {
                            preplay::Failure::AdmissionFailed
                        } else {
                            preplay::Failure::ResolveFailed
                        },
                    );
                } else {
                    preplay::phase(&mut runtime, generation, preplay::Phase::Admitted);
                }
                // An exact kiosk target retains its source-owned retry policy.
                // Observe it before generic exact-catalog failure retires the target.
                let pinned_retry = response_failed
                    && !expired
                    && !superseded
                    && runtime.kiosk_resolve_failed(app, generation);
                let catalog_failure = response_failed
                    && !pinned_retry
                    && commands::catalog_failure(&mut runtime, app);
                let reply = match response {
                    Ok(()) => {
                        runtime.status.clear();
                        runtime.pending_play.as_ref().map_or_else(
                            || ok(json!({"dispatched":true})),
                            |inv| commands::play_response(&runtime, inv),
                        )
                    }
                    Err(error) => {
                        let keep_previous_lease = (catalog_failure
                            || transient::plex_remote::target_current(&runtime).is_some())
                            && runtime
                                .active_lease
                                .as_ref()
                                .is_some_and(HeldLease::is_playback)
                            && runtime.playing.as_ref().is_some_and(|playing| {
                                playing.requires_server_lease && playing_matches(playing, app)
                            });
                        if !keep_previous_lease {
                            runtime.release_active_lease();
                        }
                        runtime.status = error.into();
                        rejected(error)
                    }
                };
                let reply =
                    transient::finish_native(&mut runtime, app, response, native_start.as_ref())
                        .unwrap_or(reply);
                // Local errors use a rejected envelope with state:error.
                // A catalogue warning must not replace their domain failure.
                if response.is_ok()
                    && reply["status"] == "ok"
                    && reply["data"]["state"] != "error"
                    && reply["data"].get("error").is_none()
                    && runtime.playing.as_ref().is_some_and(|playing| {
                        runtime.effective_profile() == Some(&playing.profile)
                    })
                {
                    if let Some((generation, warning)) = runtime.catalog_warning {
                        if generation == runtime.generation {
                            runtime.status = warning.into();
                        }
                    }
                }
                if let Some(invocation) = runtime.pending_play.take() {
                    let reply = favorites_items::finish(&invocation, reply, items_error);
                    runtime.remote.complete(invocation, reply);
                }
                let target = runtime.take_completed_play_target();
                if expired {
                    if let Some(playing) =
                        runtime.playing.as_ref().filter(|p| playing_matches(p, app))
                    {
                        runtime
                            .continuity
                            .retain_selection(runtime.generation, &playing.selection_id);
                    }
                }
                if response_failed
                    && !pinned_retry
                    && !catalog_failure
                    && !expired
                    && !superseded
                    && playback_allowed(app)
                {
                    if let Some(request) = target.filter(|request| request.advance_on_failure) {
                        let id = request.id;
                        let ids = runtime.ordered_ids();
                        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                        let decision = runtime.continuity_failed(
                            &id,
                            &refs,
                            runtime.selection_complete(),
                            Some(generation),
                        );
                        runtime.apply_continuity(decision);
                    }
                }
                runtime.changed();
            }
            Job::PlexPrepared(owner, id, result) => {
                transient::plex_remote::prepared(&mut runtime, app, owner, id, result)
            }
            Job::FavoriteRecovery(result) => favorites_recovery::finish(&mut runtime, result),
            Job::FavoriteCommit(result) => {
                favorites::finish(&mut runtime, result);
                runtime.observe_organization_context();
                transient::local_save_finished(&mut runtime, true);
            }
            Job::Play(_, _, result) => runtime.retire_result(result),
        }
    }
    runtime.reconcile_m3u_attachment(app);
    let resolver_generation = runtime.play_generation;
    if let Err(error) = runtime.launch_resolver_from_app(app) {
        preplay::failed(
            &mut runtime,
            resolver_generation,
            preplay::Failure::ResolveFailed,
        );
        transient::plex_remote::resolver_failed(&mut runtime);
        // Synchronous resolver refusal has the same kiosk-before-catalog order.
        let pinned_retry = runtime.kiosk_resolve_failed(app, runtime.play_generation);
        let catalog_failure = !pinned_retry && commands::catalog_failure(&mut runtime, app);
        runtime.status = error.into();
        let response = transient::finish_native(&mut runtime, app, Err(error), None)
            .unwrap_or_else(|| rejected(error));
        let items_error = favorites_items::pending_error(&runtime);
        if let Some(invocation) = runtime.pending_play.take() {
            let response = favorites_items::finish(&invocation, response, items_error);
            runtime.remote.complete(invocation, response);
        }
        if let Some(request) = runtime
            .play_target
            .take()
            .filter(|request| !pinned_retry && !catalog_failure && request.advance_on_failure)
        {
            let id = request.id;
            let ids = runtime.ordered_ids();
            let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            let decision = runtime.continuity_failed(
                &id,
                &refs,
                runtime.selection_complete(),
                Some(resolver_generation),
            );
            if playback_allowed(app) {
                runtime.apply_continuity(decision);
            }
        }
        runtime.changed();
    }
    // Coalesce a worker's ready batches before building catalogue indexes and the
    // immutable screen model. Large local folders must not rebuild the full list
    // once for every 64 files that arrived during the same frame.
    runtime.rearm_source_work(app);
    runtime.usb_delayed_mount_tick_at(app, runtime.now_ms());
    runtime.launch_catalog_from_app(app);
    let mut incoming = Catalog::default();
    let mut finished = None;
    for _ in 0..4 {
        let Ok(event) = runtime.catalog_rx.try_recv() else {
            break;
        };
        // USB serials are request identities. Their handler checks the original
        // runtime, source owner, deadline and cancellation independently.
        let event = match event {
            CatalogEvent::UsbRemote(serial, result) => {
                runtime.usb_remote_result(app, serial, result);
                continue;
            }
            event => event,
        };
        let generation = match &event {
            CatalogEvent::UsbRemote(..) => continue,
            CatalogEvent::Batch(generation, _)
            | CatalogEvent::Finished(generation, _)
            | CatalogEvent::LocalFinishedWithWarning(generation, _)
            | CatalogEvent::Usb(generation, _)
            | CatalogEvent::LocalBindings(generation, _) => *generation,
        };
        if managed::guard_catalog_event(&runtime, app, generation).is_err()
            || classic_source::guard_catalog_event(&runtime, app, generation).is_err()
        {
            continue;
        }
        match event {
            CatalogEvent::Usb(generation, result) => runtime.usb_result(app, generation, result),
            CatalogEvent::LocalBindings(generation, result) => {
                transient::binding_result(&mut runtime, app, generation, result)
            }
            CatalogEvent::Batch(generation, mut batch) if generation == runtime.generation => {
                runtime.catalog_diagnostics.accepted_batch(
                    runtime.generation,
                    generation,
                    batch.entries.len(),
                );
                incoming.entries.append(&mut batch.entries);
                if let Some(mut graph) = batch.hierarchy {
                    let accumulated = incoming.hierarchy.get_or_insert_with(Default::default);
                    accumulated.nodes.append(&mut graph.nodes);
                    accumulated.memberships.append(&mut graph.memberships);
                }
            }
            CatalogEvent::Finished(generation, result) if generation == runtime.generation => {
                runtime.catalog_warning = None;
                finished = Some(result)
            }
            CatalogEvent::LocalFinishedWithWarning(generation, warning)
                if generation == runtime.generation =>
            {
                runtime.catalog_warning = Some((generation, warning));
                runtime.status = warning.into();
                finished = Some(Ok(()));
            }
            _ => {}
        }
    }
    transient::scan_result(&mut runtime, app, &mut incoming, &mut finished);
    let mut unchanged_all = None;
    if !incoming.entries.is_empty() || incoming.hierarchy.is_some() {
        let unchanged =
            catalog_publication::UnchangedAll::capture(&runtime, app, &incoming, before);
        runtime.catalog_dirty = true;
        runtime
            .catalog_diagnostics
            .merge_attempted(runtime.generation);
        if let Err(error) =
            plx_base::diag::spans::span("ott_merge", || runtime.catalog.merge(incoming))
        {
            runtime.status = "Catalogue limit reached".into();
            let generation = runtime.generation;
            let managed_generation = managed::profile(&runtime)
                .is_some()
                .then_some(runtime.generation);
            runtime.cancel_catalog();
            // cancel_catalog retires the producer. Attribute this terminal
            // refusal to the new current generation, not its cancelled worker.
            runtime.catalog_diagnostics.failed(
                runtime.generation,
                catalog_diagnostics::Failure::from_merge(error),
            );
            if let Some(generation) = managed_generation {
                managed::rebase_catalog(&mut runtime, generation);
            }
            classic_source::rebase(&mut runtime, generation);
            finished = None;
        } else {
            runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
            runtime.status = format!("{} videos", runtime.catalog.entries.len());
            unchanged_all = unchanged;
        }
        // Keep the presentation revision exact for rendered numeric input.
        // The independent catalogue revision above refreshes new read snapshots.
        if unchanged_all.is_none() {
            runtime.changed();
        }
    }
    if let Some(result) = finished {
        runtime.catalog_dirty = true;
        runtime.catalog_loading = false;
        runtime.catalog_complete = result.is_ok();
        if result.is_ok() {
            runtime.catalog_diagnostics.clear();
        } else {
            runtime.catalog_diagnostics.failed(
                runtime.generation,
                catalog_diagnostics::Failure::CatalogFailed,
            );
        }
        runtime.status = result.err().map(str::to_owned).unwrap_or_else(|| {
            runtime
                .catalog_warning
                .filter(|(generation, _)| *generation == runtime.generation)
                .map(|(_, warning)| warning.to_owned())
                .unwrap_or_else(|| format!("{} videos", runtime.catalog.entries.len()))
        });
        runtime.changed();
        playing_favorite::catalog_finished(&mut runtime);
    }
    if runtime.catalog_dirty {
        transient::accepted_catalog_update(&mut runtime);
    }
    while let Ok(event) = runtime.metadata_rx.try_recv() {
        let mut selected_guide = None;
        let mut row_guide = None;
        let event = match event {
            super::ott_metadata::Event::Preview(event) => {
                if managed::guard_metadata_event(&runtime, app, event.generation, &event.profile_id)
                    .is_err()
                    || classic_source::guard_metadata_event(&runtime, app, &event.profile_id)
                        .is_err()
                {
                    continue;
                }
                if guide_ui::accept_failure(&mut runtime, &event) {
                    continue;
                }
                event
            }
            super::ott_metadata::Event::Guide(mut guide) => {
                if guide_ui::accept(&mut runtime, &guide) {
                    continue;
                }
                selected_guide = Some(selected_guide::Candidate::from_reply(
                    &guide,
                    i64::try_from(runtime.wall_clock_ms / 1000).unwrap_or(i64::MAX),
                ));
                row_guide = guide.row_guide.take();
                let Some(preview) = guide.into_preview(
                    runtime.metadata_generation,
                    runtime.metadata_target.as_ref(),
                    runtime.metadata_query.as_ref(),
                    runtime.metadata_cancel.load(Ordering::Acquire),
                ) else {
                    continue;
                };
                preview
            }
            super::ott_metadata::Event::Local {
                generation,
                completion,
            } => {
                runtime.accept_local_assets(app, generation, completion);
                continue;
            }
        };
        if managed::guard_metadata_event(&runtime, app, event.generation, &event.profile_id)
            .is_err()
            || classic_source::guard_metadata_event(&runtime, app, &event.profile_id).is_err()
        {
            continue;
        }
        if !runtime.metadata_attachment_current() {
            continue;
        }
        if let Some(binding) = runtime.metadata_local_selection.as_ref() {
            let current = runtime
                .saved
                .favorites
                .entries()
                .iter()
                .find(|f| event.entry_id == format!("favorite:{}", f.identity()));
            if !current.is_some_and(|favorite| {
                runtime
                    .selection_bindings()
                    .is_ok_and(|bindings| bindings.receipt_is_current(&binding.receipt, favorite))
            }) {
                continue;
            }
        }
        if event.generation != runtime.metadata_generation
            || runtime.metadata_target.as_ref()
                != Some(&(event.profile_id.clone(), event.entry_id.clone()))
        {
            continue;
        }
        if runtime
            .metadata_playback_identity
            .as_ref()
            .is_some_and(|identity| {
                !runtime.playing.as_ref().is_some_and(|playing| {
                    playing.identity.as_ref() == Some(identity)
                        && runtime.metadata_profile_matches(playing, &event.profile_id)
                        && (playing.entry.id == event.entry_id
                            || playing.selection_id == event.entry_id)
                        && playing_matches(playing, app)
                })
            })
        {
            continue;
        }
        live_hud::complete(&mut runtime, app, &event, selected_guide.as_ref());
        let actual_playing = runtime
            .playing
            .as_ref()
            .filter(|playing| playing_matches(playing, app))
            .cloned();
        if !runtime.retain_selected_guide(selected_guide, actual_playing.as_ref()) {
            continue;
        }
        runtime.accept_row_guide(row_guide);
        runtime.metadata_refresh_ms = runtime.now_ms().saturating_add(30000);
        let metadata_succeeded = event.result.is_ok();
        let mut source_entry = event.entry;
        if let Ok(data) = event.result {
            let now = (runtime.wall_clock_ms / 1000) as i64;
            let programme = data["programmes"].as_array().and_then(|rows| {
                rows.iter().find(|row| {
                    row["start"].as_i64().is_some_and(|v| v <= now)
                        && row["end"].as_i64().is_some_and(|v| v > now)
                })
            });
            let details = programme.unwrap_or(&data);
            if let Some(entry) = source_entry.as_mut() {
                enrich_entry(entry, details);
            }

            runtime.metadata_refresh_ms = details["end"]
                .as_i64()
                .filter(|end| *end > now)
                .map(|end| {
                    runtime.now_ms().saturating_add(
                        (end as u64)
                            .saturating_mul(1000)
                            .saturating_sub(runtime.wall_clock_ms),
                    )
                })
                .unwrap_or_else(|| runtime.now_ms().saturating_add(60000));
            if runtime
                .effective_profile()
                .is_some_and(|profile| profile.id == event.profile_id)
            {
                if let Some(entry) = runtime
                    .catalog
                    .entries
                    .iter()
                    .find(|entry| entry.id == event.entry_id)
                    .cloned()
                {
                    let mut updated = entry.clone();
                    enrich_entry(&mut updated, details);
                    if updated != entry
                        && runtime
                            .catalog
                            .merge(Catalog {
                                hierarchy: None,
                                entries: vec![updated],
                            })
                            .is_ok()
                    {
                        let previous_revision = runtime.catalog_revision;
                        runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
                        runtime.rebase_row_guide_enrichment(previous_revision);
                        runtime.changed();
                    }
                }
            }
        }
        runtime.clamp_row_guide_refresh();
        let enrich_playing = metadata_succeeded
            && runtime
                .metadata_playback_identity
                .as_ref()
                .is_some_and(|identity| {
                    runtime.playing.as_ref().is_some_and(|playing| {
                        playing.identity.as_ref() == Some(identity)
                            && runtime.metadata_profile_matches(playing, &event.profile_id)
                            && (playing.entry.id == event.entry_id
                                || playing.selection_id == event.entry_id)
                            && playing_matches(playing, app)
                    })
                });
        if enrich_playing {
            if let (Some(playing), Some(entry)) = (runtime.playing.as_mut(), source_entry.as_ref())
            {
                if playing.entry.description != entry.description
                    || playing.entry.poster != entry.poster
                    || playing.entry.duration_ms != entry.duration_ms
                {
                    playing.entry.description = entry.description.clone();
                    playing.entry.poster = entry.poster.clone();
                    playing.entry.duration_ms = entry.duration_ms;
                    runtime.changed();
                }
            }
        }
        if event.entry_id.starts_with("favorite:") {
            if let Some(entry) = source_entry {
                let local = runtime
                    .metadata_local_selection
                    .clone()
                    .filter(|_| matches!(entry.media, MediaLocator::Local { .. }));
                runtime.favorite_preview = Some((event.entry_id.clone(), entry.clone()));
                if let Some(binding) = local {
                    if let Some(root) = runtime.current_local_root(&binding) {
                        if let Ok(profile) = transient::source_profile(root) {
                            runtime.request_local_asset_preview(
                                profile,
                                entry,
                                event.entry_id,
                                binding,
                            );
                        }
                    }
                }
                runtime.changed();
            }
        }
    }
    transient::cleanup_local_proofs(&mut runtime);
    transient::pending_tick(&mut runtime, app);
    runtime.poll_local_assets(app);
    runtime.launch_metadata_from_app(app);
    if playback_allowed(app) {
        live_hud::rearm_player(&mut runtime, app);
        runtime.refresh_metadata();
    }
    runtime.refresh_plex_sources();
    companion_adapter::tick(&mut runtime, app);
    runtime.reconcile_failure(app);
    if let Err(error) = kiosk_capture::tick(&mut runtime, app) {
        runtime.status = error.message().into();
    }
    if playback_allowed(app) && runtime.play_work_allowed() {
        let resume = match runtime
            .resume_pending
            .as_ref()
            .map(|request| transient::lifecycle_resume_ready(&runtime, request))
        {
            Some(Ok(true)) => runtime.resume_pending.take(),
            Some(Err(error)) => {
                runtime.resume_pending = None;
                runtime.status = error.into();
                runtime.changed();
                None
            }
            Some(Ok(false)) | None => None,
        };
        if let Some(request) = resume {
            let exact = !request.advance_on_failure;
            let automatic_resume = runtime.continuity.issued_restore(
                runtime.generation,
                &request.id,
                request.position_ms,
            );
            let result = if request.continuation.as_ref().is_some_and(|identity| {
                !plx_media::route::playback_identity_matches(&app.player.session, identity)
            }) {
                // A new native play may have the same URL. Never resolve or stop it
                // on behalf of the older parked application request.
                runtime.retire_catalog_playback(app);
                Err("Playback changed before continuation")
            } else {
                runtime.play_request(request)
            };
            if result.is_ok() {
                runtime.restore_play_generation =
                    automatic_resume.then_some(runtime.play_generation);
                if let Some(attempt) = kiosk_source::admitted_pinned_request(&runtime) {
                    runtime.note_kiosk_attempt(attempt);
                }
            }
            if let Err(error) = result {
                if exact {
                    runtime.continuity.stop(runtime.generation);
                }
                runtime.play_target = None;
                runtime.play_profile = None;
                runtime.status = error.into();
                runtime.changed();
            }
        }
        if runtime.catalog_dirty && !runtime.transient.owns_playback() {
            runtime.catalog_dirty = false;
            let ids = runtime.ordered_ids();
            let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            let decision = runtime.continuity.catalog_updated(
                runtime.generation,
                &refs,
                runtime.selection_complete(),
            );
            runtime.apply_continuity(decision);
        }
        runtime.checkpoint(app, false);
    }
    menu_settings::reconcile(&mut runtime, app);
    let current_ott = current_ui_owner(app);
    // Keep only the owned draft while its Language surface has input. Commands
    // below still require current_ott, not this retained presentation owner.
    let retain_language = settings_groups::retained_language_owner(&runtime, app)
        .is_some_and(|owner| runtime.reply == Some(owner));
    if runtime.reply != current_ott && !retain_language {
        runtime.reply = None;
        runtime.classic_play_handoff = Default::default();
        runtime.exit_confirmation.retire();
        runtime.cancel_setup();
        settings_groups::cancel(&mut runtime);
    }
    for request in app.bridge.take_ott_reqs() {
        let source = match &request {
            OttOut::Refresh { reply } | OttOut::Action { reply, .. } => *reply,
        };
        // Fullscreen reminder answers carry the actual PlayerScreen instance.
        // They cannot turn its page into an OTT catalogue reply target.
        if let OttOut::Action {
            action: OttAction::ReminderAnswer { token, accept },
            ..
        } = &request
        {
            if matches!(app.route(), AppArg::Player)
                && app.pages.top_page() == Some(source)
                && !app.pages.has_pending_navigation()
                && !app.pages.surface_up()
            {
                if let Err(error) = runtime.answer_reminder(app, *token, *accept) {
                    runtime.status = error.into();
                    runtime.changed();
                }
            }
            continue;
        }
        if Some(source) != current_ott || app.pages.has_pending_navigation() {
            continue;
        }
        match request {
            OttOut::Refresh { reply } => {
                player_browse::bind_native_panel(&mut runtime, app, reply);
                runtime.reply = Some(reply);
                menu_settings::bind(&mut runtime, app, reply);
                runtime.changed();
            }
            OttOut::Action { reply, action } => {
                runtime.reply = Some(reply);
                let navigates = match &action {
                    action
                        if settings_groups::presentation_action(&runtime, action)
                            || interface_mode::presentation_action(&runtime, action)
                            || sleep_timer::presentation_action(&runtime, action)
                            || osd_ui::presentation_action(&runtime, action) =>
                    {
                        false
                    }
                    OttAction::SelectManagedPlex { .. }
                    | OttAction::Preview(_)
                    | OttAction::PreviewAt { .. }
                    | OttAction::FocusAt { .. }
                    | OttAction::ToggleReminder { .. }
                    | OttAction::VolumeStep(_)
                    | OttAction::ShuffleOrder(_)
                    | OttAction::ResetOrder(_)
                    | OttAction::CloseActions
                    | OttAction::GuideOrder { .. }
                    | OttAction::SelectVPortalQuality { .. } => false,
                    OttAction::EditSetting(key)
                        if key == "local.usb"
                            || key.starts_with("local.usb.select:")
                            || key.starts_with("local.usb.browse:") =>
                    {
                        false
                    }
                    OttAction::Back if runtime.usb_browsing() => false,
                    OttAction::Refresh if runtime.setup.is_some() => false,
                    OttAction::EditSetting(key)
                        if key == "profile.setup.save"
                            && runtime.setup.as_ref().is_some_and(|setup| {
                                runtime.pending_setup_save == Some(setup.id)
                            }) =>
                    {
                        false
                    }
                    OttAction::CancelSearch { session } => {
                        runtime.edit_session.as_ref().is_some_and(|editor| {
                            editor.id == *session
                                && editor.owner == reply
                                && matches!(editor.purpose, search::EditorPurpose::Search { .. })
                        })
                    }
                    OttAction::CancelSetting { session } => runtime
                        .edit_session
                        .as_ref()
                        .is_some_and(|editor| editor.id == *session && editor.owner == reply),
                    OttAction::SubmitSetting { session, .. } => {
                        !runtime.setting_submission_pending(reply, *session)
                    }
                    _ => true,
                };
                let explicit_play = matches!(
                    &action,
                    OttAction::Play(_)
                        | OttAction::Random(_)
                        | OttAction::PlayArchive { .. }
                        | OttAction::ReturnLive { .. }
                );
                let search_submission = matches!(&action, OttAction::SubmitSearch { .. });
                apply_ui_action(
                    &mut runtime,
                    app,
                    action,
                    navigates,
                    explicit_play,
                    search_submission,
                );
                if runtime.reply.is_none() {
                    break;
                }
            }
        }
    }
    let now = runtime.now_ms();
    runtime.favorites.expire(now);
    let notification_policy =
        public_preferences::notifications_enabled(&runtime.public_preferences);
    runtime.remote.tick_notifications(
        now,
        notification_policy.unwrap_or(false),
        app.running && runtime.loaded && playback_allowed(app),
    );
    let state = state(app, &runtime);
    for invocation in runtime.remote.tick(now, state) {
        let response = if let Err(error) = input_policy::remote(&mut runtime, &invocation) {
            Some(if invocation.action == "local_browser" {
                runtime.usb_remote_failure(&invocation, "restricted")
            } else if invocation.action == "local_media"
                && invocation.params.get("source_handle").is_some()
            {
                super::ott_local_session::handle_failure(
                    &invocation.params,
                    runtime.remote.runtime(),
                    super::ott_local_session::LocalError::Restricted,
                )
            } else {
                commands::category_select_rejection(&runtime, &invocation, "restricted")
                    .unwrap_or_else(|| commands::policy_rejection(&invocation, error))
            })
        } else if matches!(
            invocation.action.as_str(),
            "native_companion" | "torrent_indexer"
        ) {
            companion_adapter::command(&mut runtime, app, &invocation)
        } else if plx_data::ott::favorites_wire::Protocol::accepts_action(&invocation.action) {
            favorites::command(&mut runtime, &invocation, now)
        } else {
            commands::command(&mut runtime, app, &invocation)
        };
        if response
            .as_ref()
            .is_none_or(|response| response.get("status").and_then(Value::as_str) == Some("ok"))
        {
            managed::remote_intent(&mut runtime, &invocation);
        }
        if let Some(response) = response {
            runtime.remote.complete(invocation, response);
        }
    }
    if runtime.remote.take_configuration_receipt().is_some() {
        runtime.changed();
    }
    let snapshot = runtime.remote.snapshot();
    if snapshot != runtime.remote_snapshot {
        runtime.remote_snapshot = snapshot;
        runtime.changed();
    }
    managed::tick(&mut runtime, app);
    commands::after_tick(&mut runtime, app);
    let active_playback = native_playback_diagnostics::playback_owner(&runtime, app);
    native_playback_diagnostics::tick(
        &mut runtime.native_playback_capture,
        active_playback,
        app,
        crate::app::clock::now(),
    );
    vportal_quality::observe(&mut runtime, app);
    // Input and committed settings win over an inactivity deadline on this frame.
    sleep_timer::tick(&mut runtime, app);
    runtime.kiosk_recovery_tick(app);
    native_controls::after_tick(&mut runtime, app);
    numeric_input::tick(&mut runtime, app, now);
    // Consume failed-start deadlines only after viewer and remote commands, so
    // Stop/standby/source changes win even on the frame a retry becomes due.
    // Failed playback has already retired `playing`, so accepted Pause may
    // have no installed source for viewer_transport_intent to capture. Retire
    // the deadline itself rather than letting recovery override viewer intent.
    if lifecycle::viewer_paused() && runtime.continuity.pending_retry().is_some() {
        runtime.continuity.pause(runtime.generation);
        runtime.log_retry_cancelled(continuity_diagnostics::Reason::Changed);
    }
    if playback_allowed(app)
        && runtime.play_work_allowed()
        && runtime.play_target.is_none()
        && runtime
            .continuity
            .retry_due(runtime.generation, runtime.now_ms())
    {
        let ids = runtime.ordered_ids();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let decision =
            runtime.retry_continuity(runtime.now_ms(), &refs, runtime.selection_complete());
        runtime.apply_continuity(decision);
    }
    runtime.reconcile_retry_log();
    let reminders_active = runtime.reminders_active(app);
    runtime.poll_reminders(app, reminders_active);
    // An uncertain document is never projected. Recovery reveals menus, while
    // the independent settings and kiosk gates continue to deny unsafe actions.
    if !runtime.loaded || runtime.persistence_uncertain {
        plx_appkit::menu_visibility::project(Default::default());
    }
    if !plx_appkit::menu_visibility::projected().is_default() {
        app.rec
            .refuse("Nondefault menu visibility is not bound in this recording format");
    }
    local_profile::drain(&mut runtime, app);
    menu_settings::reconcile(&mut runtime, app);
    settings_groups::reconcile_owned(&mut runtime, app);
    classic_bookmark::tick(&mut runtime, app);
    browse_focus::flush(&mut runtime);
    // A later source/owner change cannot inherit the earlier nonvisual proof.
    // Other ordinary changes already force their own immediate publication.
    if runtime.revision == before
        && unchanged_all.is_some_and(|proof| !proof.current(&runtime, app))
    {
        runtime.changed();
    }
    let programme_dirty = programme_changed || runtime.revision != before;
    publish_programme(&mut runtime, app, programme_dirty);
    if runtime.revision != before {
        runtime.publish(app);
    }
    menu_settings::restore(&mut runtime, app);
    exit_confirmation::reconcile(&mut runtime, app);
    classic_clock::publish(&mut runtime, app);
    app.bridge.publish_ott_catalog_offer(
        menu_settings::display_offer(&runtime, app)
            .or_else(|| player_browse::display_offer(&runtime, app)),
    );
    app.ott = Some(runtime);
}

fn apply_ui_action(
    runtime: &mut Runtime,
    app: &mut App,
    action: OttAction,
    navigates: bool,
    explicit_play: bool,
    search_submission: bool,
) {
    // Prompt opening/cancellation are presentation, not accepted navigation.
    // The addressed affirmative answer alone applies the navigation effects.
    let navigates = navigates && !exit_confirmation::handles(runtime, app, &action);
    let quality_play = matches!(&action, OttAction::Play(_));
    let profile_submit =
        matches!(&action, OttAction::SubmitSetting { key, .. } if key == "profile.source");
    let duplicate_submit = matches!(&action, OttAction::SubmitSetting { session, key, .. }
        if key == "profile.source" && runtime.reply.is_some_and(|owner| local_profile::pending(runtime, owner, *session)));
    let navigates = navigates && !duplicate_submit;
    let was_saving = runtime.saving;
    let browse_before = (runtime.page, runtime.provider);
    match handle_action(runtime, app, action) {
        Ok(()) if navigates => {
            let started_save = !was_saving && runtime.saving;
            runtime.accepted_navigation(started_save);
            if quality_play {
                vportal_quality::seal(runtime);
            }
            if profile_submit {
                local_profile::seal(runtime);
            }
            if explicit_play {
                playback_step::bind_pending(runtime);
                kiosk_capture::explicit_admitted(runtime);
                classic_play_handoff::begin(runtime, app);
            }
        }
        Err(error) => {
            // Some compound actions navigate before persistence
            // admission can fail. Keep that visible navigation, but
            // never promote a failed action's save authority.
            if navigates && browse_before != (runtime.page, runtime.provider) {
                runtime.accepted_navigation(false);
            }
            if !search_submission {
                runtime.status = error.into();
            }
        }
        _ => {}
    }
    auth_return::finalize(runtime, app);
    settings_groups::reconcile_owned(runtime, app);
    runtime.changed();
}

fn handle_action(
    runtime: &mut Runtime,
    app: &mut App,
    selected: OttAction,
) -> Result<(), &'static str> {
    if let Some(result) = exit_confirmation::action(runtime, app, &selected) {
        return result;
    }
    classic_source::reconcile(runtime, app);
    menu_settings::guard(runtime, app, &selected)?;
    // Validate the captured row before any gate can replace/cancel an existing editor.
    let edit_profile = match &selected {
        OttAction::EditProfile { context, target } => {
            Some(m3u_slots::target(runtime, app, context, target)?)
        }
        _ => None,
    };
    // An old private-editor offer must fail before gates can replace an existing draft.
    let media_parent = match &selected {
        OttAction::EditM3uMediaLink { profile, context } => {
            Some(runtime.m3u_media_target(app, profile, context)?)
        }
        OttAction::ShowM3uMediaLinks
            if !runtime.m3u_media_ready() || runtime.page != "profiles" =>
        {
            return Err("M3U media settings changed; reopen the editor")
        }
        _ => None,
    };
    let random_play = matches!(&selected, OttAction::Random(_));
    let selected = settings_groups::back(runtime, selected);
    let selected = random::resolve(runtime, selected)?;
    let selected = category_state::resolve_action(runtime, selected)?;
    let selected = organization_runtime::list_order::resolve_action(runtime, selected)?;
    let selected = preview::resolve(runtime, selected)?;
    osd_ui::validate(runtime, &selected)?;
    // These inner pages already expose ShowSettings as their visible Back row.
    // Resolve the key identically before the outer Player/catalog Back handlers,
    // retaining the same policy gates without cancelling the current source.
    let selected = match selected {
        OttAction::Back
            if matches!(
                runtime.page,
                "menu-items"
                    | "interface-mode"
                    | "sleep-timer"
                    | "osd-opacity"
                    | "permanent-clock"
                    | "classic-page-size"
                    | "classic-list-position"
                    | "classic-list-arrows"
                    | "numeric-seek-step"
            ) =>
        {
            OttAction::ShowSettings
        }
        OttAction::Back if runtime.page == "m3u-media-links" => OttAction::ShowProfiles,
        action => action,
    };
    if let Some(result) = classic_play_handoff::back(runtime, app, &selected) {
        return result;
    }
    // This inner picker owns Back even when the catalogue is a Player modal.
    // It still passes the ordinary policy gates before retiring its resources.
    if !(runtime.page == "vportal-quality" && matches!(&selected, OttAction::Back)) {
        if let Some(result) = menu_settings::back(runtime, app, &selected) {
            return result;
        }
        if let Some(result) = player_browse::back(runtime, app, &selected) {
            return result;
        }
    }
    if let Some(result) = input_policy::browse_back(runtime, app, &selected) {
        return result;
    }
    let submitted = match runtime.admit_setting_submission(app, &selected) {
        Some(result) => {
            if !result? {
                return Ok(());
            }
            true
        }
        None => false,
    };
    if !submitted {
        input_policy::ui(
            runtime,
            if settings_groups::navigation_only(runtime, &selected) {
                &OttAction::ShowSettings
            } else {
                &selected
            },
        )?;
    }
    if let Some(result) = organization_runtime::cleanup_action(runtime, app, &selected) {
        return result;
    }
    if let Some(result) = parental_runtime::action(runtime, app, &selected) {
        return result;
    }
    if !submitted {
        parental_runtime::ui_gate(runtime, app, &selected)?;
    }
    if let OttAction::VolumeStep(direction) = &selected {
        return desktop_input::volume_step(runtime, app, *direction);
    }
    if let OttAction::FocusAt { id, .. } = &selected {
        browse_focus::record(runtime, id);
        return Ok(());
    }
    if !matches!(&selected, OttAction::Preview(_)) {
        runtime.native_controls = None;
    }
    if let Some(result) = guide_ui::action(runtime, app, &selected) {
        return result;
    }
    if let Some(result) = organization_runtime::list_order::action(runtime, app, &selected) {
        return result;
    }
    if let Some(result) = organization_runtime::action(runtime, app, &selected) {
        return result;
    }
    if let Some(result) = settings_groups::color_action(runtime, app, &selected) {
        return result;
    }
    if let Some(result) = settings_groups::language_action(runtime, app, &selected) {
        return result;
    }
    if let Some(result) = settings_groups::action(runtime, &selected) {
        return result;
    }
    settings_groups::external_action(runtime, &selected);
    if let Some(result) = menu_visibility::action(runtime, &selected) {
        return result;
    }
    if let Some(result) = interface_mode::action(runtime, &selected) {
        return result;
    }
    if let Some(result) = sleep_timer::action(runtime, &selected) {
        return result;
    }
    if let Some(result) = osd_ui::action(runtime, &selected) {
        return result;
    }
    if let Some(result) = search::action(runtime, app, &selected) {
        return result;
    }
    if let Some(result) = companion_adapter::action(runtime, app, &selected) {
        return result;
    }
    if let Some(result) = vportal_quality::action(runtime, app, &selected) {
        return result;
    }
    match selected {
        OttAction::VolumeStep(_) => {
            unreachable!("volume was handled before native control cleanup")
        }
        OttAction::OpenGuide(_)
        | OttAction::OpenRecords(_)
        | OttAction::GuideDay { .. }
        | OttAction::GuideOrder { .. }
        | OttAction::GuideProgramme { .. }
        | OttAction::PlayArchive { .. }
        | OttAction::ReturnLive { .. }
        | OttAction::ToggleReminder { .. } => unreachable!("guide action was handled above"),
        OttAction::ReminderAnswer { .. } => return Err("Reminder reply must come from the player"),
        OttAction::ExitAnswer { .. } => return Err("Exit confirmation is no longer current"),
        OttAction::Random(_) => unreachable!("random was resolved to an ordinary Play action"),
        OttAction::SelectCategoryAt { .. } | OttAction::PreviewAt { .. } => {
            unreachable!("source context was resolved before admission")
        }
        OttAction::ShowActions(_)
        | OttAction::ShowActionsAt { .. }
        | OttAction::ShuffleOrder(_)
        | OttAction::ResetOrder(_)
        | OttAction::CloseActions => unreachable!("order action was handled above"),
        OttAction::MenuVisibility { .. } => unreachable!("menu action was handled above"),
        OttAction::ShowCatalog
        | OttAction::BeginSearch
        | OttAction::ClearSearch
        | OttAction::SubmitSearch { .. }
        | OttAction::CancelSearch { .. } => unreachable!("search action was handled above"),
        OttAction::ShowProviders => {
            runtime.cancel_setup();
            runtime.page = "providers";
        }
        OttAction::ShowProfiles => {
            runtime.cancel_setup();
            runtime.page = "profiles";
        }
        OttAction::ShowM3uMediaLinks => {
            runtime.cancel_setup();
            runtime.page = "m3u-media-links";
        }
        OttAction::EditM3uMediaLink { .. } => {
            runtime.begin_m3u_media(
                app,
                media_parent.ok_or("M3U media settings changed; reopen the editor")?,
            )?;
        }
        OttAction::ShowCategories => {
            runtime.cancel_setup();
            runtime.page = "categories";
            runtime.resume_returned_catalog();
        }
        OttAction::ShowFavorites => {
            runtime.save_category_choice(None)?;
            runtime.cancel_setup();
        }
        OttAction::ShowSettings => {
            runtime.cancel_setup();
            runtime.page = "settings";
        }
        OttAction::SelectProvider(id) => {
            runtime.provider = match id.as_str() {
                "m3u" => ProviderKind::M3u,
                "vportal" => ProviderKind::VPortal,
                "local" | "offline" => ProviderKind::Local,
                "plex" => ProviderKind::Plex,
                "xtream" => ProviderKind::Xtream,
                "edem" | "ilook" | "ilooktv" => ProviderKind::Edem,
                "stalker" => ProviderKind::Stalker,
                id => ProviderKind::from_id(id).ok_or("Unknown provider")?,
            };
            runtime.cancel_setup();
            runtime.page = "profiles";
            runtime.profile_focus = None;
            runtime.plex_omitted_servers = 0;
            if runtime.provider == ProviderKind::Plex {
                runtime.recover_plex_profiles(app, false)?;
            }
        }
        OttAction::ResumeManagedPlex { operation } => {
            return managed::resume_ui(runtime, app, operation);
        }
        OttAction::SelectManagedPlex { server_id, context } => {
            return managed::select_ui(runtime, app, &server_id, &context);
        }
        OttAction::EditProfile { .. } => {
            let profile = edit_profile.ok_or("Profile editor is no longer active")?;
            m3u_slots::open(runtime, app, profile)?;
        }
        OttAction::SelectProfile(id) => {
            if runtime
                .saved
                .profiles
                .profiles
                .iter()
                .any(|profile| profile.id == id && profile.source.is_empty())
            {
                return Err("Configure this profile before selecting it");
            }
            let mut saved = runtime.saved.clone();
            saved.profiles.active_profile = Some(id);

            runtime.save(saved, None, true)?;
            runtime.cancel_setup();
        }
        OttAction::SelectCategory(category) => {
            runtime.save_category_choice(Some(category))?;
            runtime.cancel_setup();
        }
        OttAction::ToggleFavorite(id) => {
            let mut saved = runtime.saved.clone();
            if let Some(identity) = id.strip_prefix("favorite:") {
                saved.favorites.remove(identity);
            } else {
                let row = runtime
                    .catalog
                    .entries
                    .iter()
                    .find(|e| e.id == id)
                    .ok_or("Video not found")?;
                let bindings = runtime
                    .selection_bindings()
                    .map_err(|_| "Source inventory is invalid")?;
                let owner = runtime.row_owner().ok_or("Select a provider profile")?;
                if matches!(owner, selection::SelectionOwner::Profile(profile) if profile.kind == ProviderKind::M3u)
                    && !runtime.catalog_complete
                {
                    return Err("The channel catalogue is not complete");
                }
                let kiosk = super::ott_kiosk_stored::read(&saved.settings)
                    .map_err(|_| "Invalid stored kiosk policy")?;
                saved
                    .local_favorite_proofs
                    .toggle_capture_with_pins(
                        &mut saved.favorites,
                        kiosk.as_ref().map_or(&[], |pin| pin.pins()),
                        &bindings,
                        owner,
                        &runtime.catalog.entries,
                        row,
                    )
                    .map_err(selection::BindingError::message)?;
            }
            runtime.save(saved, None, false)?;
        }
        OttAction::FocusAt { .. } => unreachable!("browse focus was handled above"),
        OttAction::Preview(id) => {
            if id.starts_with("favorite:") {
                return runtime.request_favorite_metadata(&id);
            }
            if let Some(entry) = runtime
                .catalog
                .entries
                .iter()
                .find(|entry| entry.id == id)
                .cloned()
            {
                if let Some(profile) = runtime.effective_profile().cloned() {
                    runtime.request_metadata(profile, entry);
                }
            }
        }
        OttAction::SelectVPortalQuality { .. } => unreachable!("quality action handled above"),
        OttAction::Play(id) => {
            if !playback_allowed(app) {
                return Err("Playback is suspended");
            }
            if !random_play && player_browse::play_current(runtime, app, &id)? {
                return Ok(());
            }
            let browse_choice = !random_play
                && preview::context(runtime).is_some_and(|context| {
                    preview::resolve(
                        runtime,
                        OttAction::FocusAt {
                            context,
                            id: id.clone(),
                        },
                    )
                    .is_ok()
                });
            runtime.checkpoint(app, true);
            if random_play {
                runtime.play_with_policy(app, &id, false)?;
            } else {
                let interactive = vportal_quality::may_offer(runtime, app);
                if !runtime.play_work_allowed() {
                    return Err("Media work is waiting for foreground or worker retirement");
                }
                let (mut request, profile, entry, favorite) =
                    runtime.prepare_play_request(PlaybackRequest::ordinary(&id, 0))?;
                // Explicit selection owns this exact row. A failed admission is
                // not EOF and must not silently start another channel or film.
                request.advance_on_failure = false;
                runtime.play_prepared_request_with_quality_policy(
                    app,
                    (request, profile, entry, favorite),
                    interactive,
                )?;
            }
            // An admitted row activation is also explicit browse intent. This
            // branch excludes remote/autoplay and the random shortcut.
            if browse_choice {
                browse_focus::record(runtime, &id);
                browse_focus::request_flush(runtime);
            }
            runtime.cancel_setup();
        }
        OttAction::Refresh => {
            if runtime.setup.is_none() {
                runtime.cancel_editor();
            }
            if matches!(runtime.state_load, StateLoad::Failed(_)) {
                runtime.start_state_load();
            } else if runtime.setup.is_none() && runtime.loaded {
                // Only this admitted viewer action may attribute cancellation to Refresh.
                // Hold its diagnostic record away from generic source invalidation below.
                let retry = runtime.continuity_retry_log.take();
                if runtime.transient.accepted.is_none()
                    && runtime.transient.candidate.is_none()
                    && (managed::profile(runtime).is_some()
                        || classic_source::profile(runtime).is_some())
                {
                    runtime.refresh_catalog_preserving_playback();
                } else {
                    transient::refresh_effective(runtime, app);
                }
                if let Some(retry) = retry {
                    if runtime.continuity.pending_retry().is_some_and(|pending| {
                        retry.catalog_generation == pending.generation
                            && retry.failures == pending.failures
                            && retry.not_before_ms == pending.not_before_ms
                    }) {
                        // A transient Plex refresh can leave the cooldown untouched.
                        runtime.continuity_retry_log = Some(retry);
                    } else {
                        retry.emit(
                            continuity_diagnostics::Event::Cancelled,
                            continuity_diagnostics::Reason::Refresh,
                            runtime.now_ms(),
                            runtime.generation,
                        );
                    }
                }
            }
        }
        OttAction::Back => {
            if runtime.usb_browsing() {
                return runtime.usb_browser_back(app);
            }
            if classic_play_handoff::close_catalog(runtime, app) {
                return Ok(());
            }
            // Categories is another view of the same catalogue. Closing it must
            // retain progressive loading, the selected source and its playback.
            if runtime.page == "categories" && runtime.setup.is_none() {
                runtime.cancel_setup();
                runtime.page = if runtime.view_selection().1 {
                    "favorites"
                } else {
                    "channels"
                };
                return Ok(());
            }
            let return_catalog = runtime.catalog_return_after_back();
            let leaving_setup = runtime.setup.is_some();
            runtime.cancel_setup();
            transient::cancel_play_candidate(runtime, app);
            transient::discard(runtime);
            // A save admitted before Back may refresh data, but cannot restore
            // its older playback intent when the durable reply arrives.
            runtime.playback_epoch = runtime.playback_epoch.wrapping_add(1);
            runtime.cancel_play_request();
            // Cancelling a pending Local play may already have restored and
            // rebased the retained catalogue. Capture this cancellation's owner.
            let classic_generation = runtime.generation;
            let managed_generation = managed::profile(runtime)
                .is_some()
                .then_some(runtime.generation);
            runtime.cancel_catalog();
            if let Some(generation) = managed_generation {
                managed::rebase_catalog(runtime, generation);
            }
            classic_source::rebase(runtime, classic_generation);
            runtime.catalog_return =
                return_catalog.map(|pending| pending.rebase(runtime.generation));
            runtime.status.clear();
            if leaving_setup || matches!(runtime.page, "channels" | "favorites") {
                runtime.page = "profiles";
            } else if runtime.page != "providers" {
                runtime.page = "providers";
            } else if app.pages.nav.tabs.stack.depth() > 1 {
                bridge::nav_pop(&mut app.pages);
            } else {
                // A valid root Back was consumed by the owned confirmation above.
                return Err("Exit confirmation is no longer current");
            }
        }
        OttAction::EditSetting(key) => {
            if let Some(result) = runtime.usb_action(app, &key) {
                return result;
            }
            if key == "profile.add" {
                if let Some(error) = runtime.profile_add_refusal() {
                    return Err(error);
                }
            }
            if key == "profile.setup.cancel" {
                if runtime.setup.is_none() {
                    return Err("Profile setup is no longer active");
                }
                return handle_action(runtime, app, OttAction::Back);
            }
            if key == "profile.setup.save" {
                let setup = runtime
                    .setup
                    .as_ref()
                    .ok_or("Profile setup is no longer active")?;
                if runtime.pending_setup_save == Some(setup.id) {
                    return Ok(());
                }
                let (id, candidate) = runtime.setup_candidate()?;
                runtime.save(candidate, None, true)?;
                runtime.pending_setup_save = Some(id);
                runtime.cancel_editor();
                return Ok(());
            }
            if !key.starts_with("profile.setup.") {
                runtime.cancel_setup();
            }
            if key == "plex.connect" {
                runtime.cancel_setup();
                runtime.provider = ProviderKind::Plex;
                runtime.page = "profiles";
                return runtime.recover_plex_profiles(app, true);
            }
            if key == "remote.retry" {
                runtime.remote.retry_load()?;
                runtime.status.clear();
                return Ok(());
            }
            if key == "remote.find" {
                if runtime.page != "settings" {
                    return Err("Server choices changed. Find servers again.");
                }
                runtime.remote.find_servers(runtime.now_ms())?;
                runtime.status.clear();
                return Ok(());
            }
            if let Some(choice) = key.strip_prefix("remote.choice:") {
                if runtime.page != "settings" {
                    return Err("Server choices changed. Find servers again.");
                }
                runtime
                    .remote
                    .select_discovered_choice(choice, runtime.now_ms())?;
                runtime.status.clear();
                return Ok(());
            }
            if key == "remote.cancel" {
                runtime.remote.cancel_pairing();
                return Ok(());
            }
            if key == "remote.disable" {
                runtime.remote.disable()?;
                return Ok(());
            }
            if key.starts_with("remote.server:") {
                return Err("Server choices changed. Find servers again.");
            }
            if key == "native.audio" || key == "native.subtitles" {
                return native_controls::open(
                    runtime,
                    app,
                    if key == "native.audio" { 0 } else { 1 },
                );
            }
            if key == "native.settings" {
                bridge::open_settings(&mut app.pages);
                return Ok(());
            }
            let mut actual_key = key.clone();
            let (title, value, secret, limit, text_policy) = match key.as_str() {
                "profile.add" => {
                    runtime.edit_profile = Some(runtime.new_profile_draft()?);
                    if matches!(
                        runtime.provider,
                        ProviderKind::Xtream | ProviderKind::Stalker | ProviderKind::Edem | ProviderKind::Operator(_)
                    ) {
                        let profile = runtime.edit_profile.take().expect("new profile draft");
                        return runtime.open_setup(profile, true);
                    }
                    actual_key = "profile.source".into();
                    (
                        "New profile source",
                        String::new(),
                        true,
                        16384,
                        OttTextPolicy::Literal,
                    )
                }
                "profile.name" => {
                    runtime.edit_profile = runtime.provider_profile().cloned();
                    runtime.edit_profile_original = runtime.edit_profile.clone();
                    (
                        "Profile name",
                        runtime
                            .edit_profile
                            .as_ref()
                            .ok_or("Select a profile first")?
                            .name
                            .clone(),
                        false,
                        256,
                        OttTextPolicy::Predictive,
                    )
                }
                "profile.source" => {
                    runtime.edit_profile = runtime.provider_profile().cloned();
                    runtime.edit_profile_original = runtime.edit_profile.clone();
                    if runtime.edit_profile.as_ref().is_some_and(|profile| {
                        matches!(profile.kind, ProviderKind::Xtream | ProviderKind::Stalker | ProviderKind::Edem | ProviderKind::Operator(_))
                    }) {
                        let profile = runtime.edit_profile.take().expect("selected profile draft");
                        return runtime.open_setup(profile, false);
                    }
                    (
                        "Source address or folder",
                        runtime
                            .edit_profile
                            .as_ref()
                            .ok_or("Select a profile first")?
                            .source
                            .clone(),
                        true,
                        16384,
                        OttTextPolicy::Literal,
                    )
                }
                key if key.starts_with("profile.setup.") => {
                    let setup = runtime
                        .setup
                        .as_mut()
                        .ok_or("Profile setup is no longer active")?;
                    if runtime.pending_setup_save == Some(setup.id) {
                        return Err("This profile is being saved");
                    }
                    let field = *setup
                        .draft
                        .fields()
                        .iter()
                        .find(|field| field.key == key)
                        .ok_or("This field is not available for this provider")?;
                    let value = setup.draft.value(key)?.to_owned();
                    setup.selected_key = field.key.into();
                    (
                        field.title,
                        value,
                        field.secret,
                        field.limit,
                        field.text_policy,
                    )
                }
                "profile.history_hours" => (
                    "Archive hours (0 uses playlist values)",
                    runtime.begin_archive_hours()?,
                    false,
                    4,
                    OttTextPolicy::Literal,
                ),
                "volumeStep" => (
                    "Volume step (3–10%)",
                    runtime.volume_step().to_string(),
                    false,
                    2,
                    OttTextPolicy::Literal,
                ),
                "infoTimeout" => (
                    "Information display time (3–20 seconds)",
                    runtime.info_timeout().seconds().to_string(),
                    false,
                    2,
                    OttTextPolicy::Literal,
                ),
                "uiFontScale" => (
                    "Interface text scale (20–200%)",
                    runtime.font_scale_percent().to_string(),
                    false,
                    3,
                    OttTextPolicy::Literal,
                ),
                "remote.address" => (
                    "Remote control server",
                    remote_settings::address_input(runtime),
                    false,
                    2048,
                    OttTextPolicy::Literal,
                ),
                key if key.starts_with("offline.bind:") => (
                    "Folder on this device",
                    String::new(),
                    false,
                    4096,
                    OttTextPolicy::Literal,
                ),
                _ => return Err("Unknown setting"),
            };
            runtime.open_setting_editor(
                app,
                OttEditor {
                    kind: plx_screens::ott::OttEditorKind::Setting,
                    session: 0,
                    title: title.into(),
                    key: actual_key,
                    value,
                    secret,
                    limit,
                    text_policy,
                },
            )?;
        }
        OttAction::CancelSetting { session } => {
            if runtime.edit_session.as_ref().is_some_and(|current| {
                current.id == session
                    && Some(current.owner) == runtime.reply
                    && current.purpose == search::EditorPurpose::Setting
            }) {
                runtime.cancel_editor();
            }
        }
        OttAction::SubmitSetting {
            session,
            key,
            value,
        } => {
            let request = EditorSession {
                id: session,
                owner: runtime.reply.ok_or("Editor screen is no longer active")?,
                key: key.clone(),
                purpose: search::EditorPurpose::Setting,
            };
            // A wrong-key envelope cannot acknowledge the active pending editor.
            // Retired sessions still receive the existing failure acknowledgement.
            if runtime.edit_session.as_ref().is_some_and(|current| {
                current.id == request.id
                    && current.owner == request.owner
                    && current.key != request.key
            }) {
                return Err("Editor session is no longer active");
            }
            let result = (|| {
                if runtime.edit_session.as_ref() != Some(&request) {
                    return Err("Editor session is no longer active");
                }
                // A repeated delivery must not release the pending screen or
                // start a second writer for the same submitted draft.
                if runtime.pending_editor_save.as_ref() == Some(&request) {
                    return Ok(false);
                }
                if runtime.pending_editor_save.is_some() {
                    return Err("Another editor save is in progress");
                }
                if key.starts_with("organization.") {
                    return runtime.submit_organization(&request, &value);
                }
                if key.starts_with("profile.setup.") {
                    let setup = runtime
                        .setup
                        .as_mut()
                        .ok_or("Profile setup is no longer active")?;
                    if runtime.pending_setup_save == Some(setup.id) {
                        return Err("This profile is being saved");
                    }
                    setup.draft.stage(&key, &value)?;
                    runtime.status.clear();
                    return Ok(true);
                }
                if key == m3u_media_editor::KEY {
                    let candidate = runtime.m3u_media_candidate(&value)?;
                    runtime.save(candidate, None, false)?;
                } else if key == "profile.history_hours" {
                    let saved = runtime.archive_hours_candidate(&value)?;
                    runtime.save(saved, None, false)?;
                } else if key == "volumeStep" {
                    let saved = runtime.volume_step_candidate(&value, session)?;
                    runtime.save(saved, None, false)?;
                } else if key == "infoTimeout" {
                    let saved = runtime.info_timeout_candidate(&value, session)?;
                    runtime.save(saved, None, false)?;
                } else if key == "uiFontScale" {
                    let saved = runtime.font_scale_candidate(&value, session)?;
                    runtime.save(saved, None, false)?;
                } else if let Some(number) = key.strip_prefix("offline.bind:") {
                    let saved = commands::bind_local(
                        runtime,
                        number.parse().map_err(|_| "Invalid local profile slot")?,
                        value.trim(),
                    )?;
                    runtime.save(saved, None, true)?;
                } else if key == "remote.address" {
                    let candidate = remote_settings::candidate(runtime, &value)?;
                    // Only the typed, credential-free address is written. Pairing
                    // still waits for this editor's durable acknowledgement.
                    let patch = public_preferences::Patch::RemoteAddress(
                        remote_settings::saved_address(&candidate)?
                            .ok_or("Missing server address")?
                            .to_owned(),
                    );
                    runtime.save_public_preference(
                        candidate,
                        patch,
                        None,
                        ok(json!({"persisted":true})),
                    )?;
                } else {
                    let mut profile = runtime
                        .edit_profile
                        .clone()
                        .ok_or("Profile editor is no longer active")?;
                    match key.as_str() {
                        "profile.name" => profile.name = value.trim().into(),
                        "profile.source" => profile.source = value.trim().into(),
                        _ => return Err("Unknown setting"),
                    }
                    profile
                        .validate()
                        .map_err(|_| "Invalid profile source or name")?;
                    let original = runtime.edit_profile_original.clone();
                    if key == "profile.source"
                        && profile.kind == ProviderKind::Local
                        && plx_data::ott::http_url(&profile.source).is_none()
                        && !runtime.usb_folder_path(&profile.source)?
                    {
                        local_profile::begin(runtime, app, request.clone(), profile, original)?;
                        return Ok(false);
                    }
                    runtime.save_profile_editor(&request, profile, original.as_ref())?;
                }
                runtime.pending_editor_save = Some(request.clone());
                Ok(false)
            })();
            match result {
                Ok(true) => runtime.editor_result(app, &request, None),
                Ok(false) => {}
                Err(error) => {
                    runtime.editor_result(app, &request, Some(error.to_owned()));
                    return Err(error);
                }
            }
        }
    }
    Ok(())
}

/// The linear OTT catalogue owns its successor; the legacy Plex shelf retains
/// its own Up Next behaviour when playback did not originate in this catalogue.
pub(super) fn finish_catalog_playback(app: &mut App) -> bool {
    if app
        .ott
        .as_ref()
        .is_some_and(|runtime| runtime.transient.owns_playback())
    {
        return true;
    }
    // The outgoing decoder's EOF can remain set across frames while a newly
    // selected native Plex item resolves. Neither OTT nor the legacy handler
    // may consume it, including after reconciliation retired OTT ownership.
    if retained_request_replaces_installed(app) {
        return true;
    }
    let Some(mut runtime) = app.ott.take() else {
        return false;
    };
    let current = runtime
        .playing
        .as_ref()
        .filter(|p| playing_matches(p, app))
        .map(|p| p.selection_id.clone());
    let Some(current) = current else {
        app.ott = Some(runtime);
        return false;
    };
    // EOF is consumed before the ordinary failure tick. Keep an owned error
    // for that tick (including kiosk recovery), never commit it as completion.
    if plx_media::player::state(&app.player.session) == plx_media::player::PlaybackState::Error {
        app.ott = Some(runtime);
        return true;
    }
    if runtime.playing.as_ref().is_some_and(|playing|
        playing.archive.is_none() && guide_ui::is_live(&playing.profile, &playing.entry))
    {
        // A live source has no successful final episode. Retain its exact row
        // and retry through the existing bounded, generation-fenced runner.
        let accepted = runtime.continuity.accepts_end(runtime.generation, &current);
        let decision = runtime.continuity.live_disconnected(runtime.generation, &current, runtime.now_ms());
        if accepted {
            // A terminal live decoder has the same source-bound catalogue
            // recovery as an attributed error, without lending transport
            // authority or advancing to a different channel.
            runtime.checkpoint(app, false);
            runtime.failed_playback = failed_playback::capture(&runtime, app);
            runtime.playing = None;
            runtime.resume_floor = None;
            runtime.release_active_lease();
            plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
            if let Some(page) = bridge::player_mut(&mut app.pages) {
                page.transport_reset(false);
            }
            plx_media::route::cancel_play(&mut app.player.session);
            runtime.status = if runtime.continuity.pending_retry().is_some() {
                "Live stream ended; retrying the same channel"
            } else { "Live stream ended" }.into();
            runtime.apply_continuity(decision);
            runtime.changed();
        }
        app.ott = Some(runtime);
        return true;
    }
    if runtime
        .playing
        .as_ref()
        .is_some_and(|playing| playing.archive.is_some())
    {
        // An interval is one explicit selection. EOF cannot silently switch to
        // another channel or restart the live source. Preserve a newer candidate.
        if runtime.play_target.is_none() {
            runtime.retire_catalog_playback(app);
            plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
            plx_media::route::cancel_play(&mut app.player.session);
            runtime.status = "Archive ended".into();
        }
        app.ott = Some(runtime);
        return true;
    }
    if transient::plex_remote::ended(&mut runtime, app) {
        app.ott = Some(runtime);
        return true;
    }
    if !runtime.continuity.accepts_end(runtime.generation, &current) {
        // This source still belongs to OTT, so the legacy EOF handler must not
        // consume it either. Keep its decoder and checkpoint until a valid EOF.
        app.ott = Some(runtime);
        return true;
    }
    runtime.resume_floor = None;
    let durable = runtime
        .playing
        .as_ref()
        .and_then(|playing| kiosk_resume::checkpoint(&runtime, playing, 0))
        .filter(|_| {
            runtime.continuity.pending_restore().is_none()
                && runtime.restore_play_generation != Some(runtime.play_generation)
        });
    let mut checkpoint_changed = durable.is_some();
    if let Some(point) = durable {
        runtime.checkpoints.insert(kiosk_resume::KEY.into(), point);
    }
    if let Some(playing) = runtime.playing.clone().filter(|playing| {
        !runtime.transient_checkpoint(playing, 0) || runtime.durable_global_favorite(playing)
    }) {
        let key = if current.starts_with("favorite:") {
            "favorites".into()
        } else {
            playing.profile.id.clone()
        };
        runtime.checkpoints.insert(
            key,
            SavedResume {
                entry_id: current.clone(),
                position_ms: 0,
                source_fingerprint: profile_fingerprint(&playing.profile),
            },
        );
        checkpoint_changed = true;
    }
    if checkpoint_changed && !runtime.saving {
        let _ = runtime.save(runtime.saved.clone(), None, false);
    }
    let ids = runtime.ordered_ids();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let anchor = runtime
        .playing
        .as_ref()
        .and_then(|playing| runtime.strict_folder_anchor(playing));
    let decision = if let Some(anchor) = anchor {
        runtime.continuity.ended_at(
            runtime.generation,
            &current,
            &anchor,
            &refs,
            runtime.selection_complete(),
        )
    } else {
        runtime.continuity.ended(
            runtime.generation,
            &current,
            &refs,
            runtime.selection_complete(),
        )
    };
    if let Some(playing) = runtime.playing.as_mut() {
        playing.advance_on_failure = true;
    }
    runtime.release_active_lease();
    // Retire the old decoder and server lease once, including while waiting for
    // a not-yet-discovered successor. Only catalogue completion permits wrapping.
    plx_media::player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
    runtime.apply_continuity(decision);
    app.ott = Some(runtime);
    true
}

/// A retained Plex request takes ownership before its source is installed.
/// Keep this separate from installed-source matching: the outgoing decoder may
/// still need checkpointing when ownership passes to the replacement.
fn playing_superseded(playing: &Playing, app: &App) -> bool {
    if !same_logical_playback(playing, app) {
        return true;
    }
    if let Some((sid, rk)) = plx_media::route::playback_request_identity(&app.player.session) {
        !matches!(&playing.entry.media, MediaLocator::Plex { server_id, rating_key }
            if rating_key == rk && plx_plex::plex::id_of_machine(server_id) == Some(sid))
    } else {
        plx_media::route::external_source(&app.player.session)
            .is_some_and(|source| playing.source.as_ref() != Some(&source))
    }
}

fn retained_request_replaces_installed(app: &App) -> bool {
    plx_media::route::playback_request_identity(&app.player.session).is_some_and(|(sid, rk)| {
        plx_media::route::external_source(&app.player.session).is_some()
            || sid != plx_media::route::cur_sid(&app.player.session)
            || rk != plx_media::route::cur_rk(&app.player.session)
    })
}

fn same_logical_playback(playing: &Playing, app: &App) -> bool {
    match (
        &playing.identity,
        plx_media::route::playback_identity(&app.player.session),
    ) {
        (Some(expected), Some(actual)) => *expected == actual,
        // Legacy untagged development sources keep their existing comparison.
        // An untagged source can never adopt a newly tagged native playback.
        (None, None) => true,
        _ => false,
    }
}

fn playing_matches(playing: &Playing, app: &App) -> bool {
    if !same_logical_playback(playing, app) {
        return false;
    }
    match &playing.entry.media {
        MediaLocator::Plex {
            rating_key,
            server_id,
        } => {
            plx_media::route::cur_rk(&app.player.session) == *rating_key
                && plx_plex::plex::id_of_machine(server_id)
                    == Some(plx_media::route::cur_sid(&app.player.session))
        }
        _ => {
            playing.source.is_some()
                && plx_media::route::external_source(&app.player.session) == playing.source
        }
    }
}

fn start_resolved(
    runtime: &mut Runtime,
    app: &mut App,
    profile: ProviderProfile,
    entry: CatalogEntry,
    source: ReadySource,
) -> Result<(), &'static str> {
    start_resolved_with(
        runtime,
        app,
        profile,
        entry,
        source,
        &mut playback::LivePlaybackResources,
    )
}

fn start_resolved_with<R: playback::PlaybackResources>(
    runtime: &mut Runtime,
    app: &mut App,
    profile: ProviderProfile,
    entry: CatalogEntry,
    source: ReadySource,
    resources: &mut R,
) -> Result<(), &'static str> {
    if let Err(error) = managed::guard_profile(runtime, app, &profile)
        .and_then(|()| classic_source::guard_profile(runtime, app, &profile))
    {
        runtime.retire_result(Ok((entry, source)));
        return Err(error);
    }
    let mut request = runtime
        .play_target
        .clone()
        .unwrap_or_else(|| PlaybackRequest::ordinary(&entry.id, 0));
    let restore_position_ms = request.position_ms;
    if guide_ui::is_live(&profile, &entry) {
        request.advance_on_failure = false;
        // Historical bookmarks can contain elapsed live time. Resume the
        // channel at its live edge even if the probe reports a finite duration;
        // an explicitly admitted archive keeps its own interval offset.
        if request.archive.is_none() { request.position_ms = 0; }
        if let Some(target) = runtime.play_target.as_mut() {
            target.advance_on_failure = false;
            if target.archive.is_none() { target.position_ms = 0; }
        }
    }
    let automatic_resume = runtime.restore_play_generation == Some(runtime.play_generation);
    if let Err(error) = runtime.validate_archive_admission(&request, &profile, &entry, &source) {
        runtime.retire_result(Ok((entry, source)));
        return Err(error);
    }
    if request.continuation.as_ref().is_some_and(|identity| {
        !plx_media::route::playback_identity_matches(&app.player.session, identity)
            || !matches!(&source, ReadySource::External(_, _, _))
    }) {
        // Retire a rejected candidate lease through the existing cleanup owner.
        // This guard must precede replacement of the current native route.
        runtime.retire_result(Ok((entry, source)));
        return Err("Playback changed before continuation");
    }
    if let ReadySource::PreparedPlex(choice) = &source {
        if !choice.current() || request.transient_owner != Some(choice.owner)
            || choice.item().is_none_or(|item| !matches!(&entry.media,
                MediaLocator::Plex { server_id, rating_key } if server_id == &choice.authority.machine_id && rating_key == &item.rating_key))
        { return Err("Plex source changed before native admission"); }
    }
    let requires_server_lease = matches!(&source, ReadySource::External(_, _, Some(_)));
    if plx_media::route::has_url(&app.player.session) || matches!(app.route(), AppArg::Player) {
        playback::replace_player_in_place(
            &mut app.player,
            &mut app.adapters.player,
            &mut app.repause_at,
            &mut app.pages,
        );
    } else {
        plx_media::route::cancel_play(&mut app.player.session);
    }
    let retained_resume_floor = runtime
        .resume_floor
        .take()
        .filter(|(owner, _)| request.continuation.as_ref() == Some(owner));
    runtime.playing = None;
    runtime.active_lease = None;
    let selection_id = request.id;
    let position_ms = request.position_ms;
    let mut resume_floor_ms = position_ms;
    let advance_on_failure = request.advance_on_failure && !guide_ui::is_live(&profile, &entry);
    let mut owned_source = None;
    match source {
        ReadySource::PreparedPlex(choice) => {
            let item = choice.item().ok_or("Plex metadata is unavailable")?;
            let media = item.media.first().ok_or("Plex media is unavailable")?;
            let part = media.part.first().ok_or("Plex part is unavailable")?;
            if !plx_media::route::request_play(
                &mut app.player.session,
                app.bridge.metadata_mut(),
                playback::start_caps(),
                choice.authority.server,
                &item.rating_key,
                &part.key,
                &media.video_codec,
                &media.audio_codec,
                &entry.title,
                &entry.category,
                (media.id.0 != 0).then_some(media.id),
            ) {
                return Err("Plex playback request was refused");
            }
            app.bridge.start_store_jobs();
            if position_ms > 0 {
                plx_media::route::arm_play_resume(
                    &mut app.player.session,
                    position_ms.saturating_mul(1_000_000).min(i64::MAX as u64) as i64,
                );
            }
        }
        ReadySource::External(source, info, lease) => {
            runtime.active_lease = lease.map(HeldLease::Playback);
            app.bridge
                .metadata_run(plx_data::stores::metadata::MetadataCmd::InstallPlaying(
                    None,
                ));
            app.bridge
                .metadata_run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(
                    Some(plx_data::metadata::NowPlaying {
                        ott: None,
                        is_episode: false,
                        is_real_episode: false,
                        title: entry.title.clone(),
                        ep_title: String::new(),
                        season: 0,
                        index: 0,
                        summary: entry.description.clone(),
                        year: 0,
                        dur_ms: info.duration_ns.max(0) / 1_000_000,
                        rating: String::new(),
                        thumb: entry.poster.clone(),
                        detail_rk: String::new(),
                    }),
                ));
            let admission = if let Some(identity) = request.continuation.as_ref() {
                plx_media::route::continue_external_source(
                    &mut app.player.session,
                    &info,
                    source,
                    &entry.title,
                    &entry.category,
                    playback::start_caps(),
                    identity,
                )
            } else {
                plx_media::route::request_external_source(
                    &mut app.player.session,
                    &info,
                    source,
                    &entry.title,
                    &entry.category,
                    playback::start_caps(),
                )
            };
            admission.map_err(|_| "The native decoder cannot open this source")?;
            subtitle_settings::Stored::read(&runtime.saved)?.apply(app, true);
            owned_source = plx_media::route::external_source(&app.player.session);
            let resume_ns = if info.duration_ns > 0 {
                (position_ms
                    .min((info.duration_ns / 1_000_000).max(0) as u64)
                    .saturating_mul(1_000_000))
                .min(i64::MAX as u64) as i64
            } else {
                0
            };
            resume_floor_ms = resume_ns.max(0) as u64 / 1_000_000;
            if !playback::start_playback_with(
                &mut app.player.session,
                &mut app.adapters.player,
                resume_ns,
                playback::Origin::Here,
                plx_screens::player::input::HudStart::Ordinary,
                None,
                &mut app.pages,
                &mut app.bridge,
                resources,
            ) {
                return Err("Native playback could not start");
            }
        }
        ReadySource::Plex {
            server_id,
            rating_key,
        } => {
            let sid = plx_plex::plex::id_of_machine(&server_id)
                .ok_or("Favorite Plex server is not connected")?;
            let accepted = plx_media::route::request_play(
                &mut app.player.session,
                app.bridge.metadata_mut(),
                playback::start_caps(),
                sid,
                &rating_key,
                "",
                "",
                "",
                &entry.title,
                &entry.category,
                None,
            );
            app.bridge.start_store_jobs();
            if !accepted {
                return Err("Plex playback request was refused");
            }
            if position_ms > 0 {
                plx_media::route::arm_play_resume(
                    &mut app.player.session,
                    position_ms.saturating_mul(1_000_000).min(i64::MAX as u64) as i64,
                );
            }
        }
    }
    let channel_capture =
        if runtime.catalog_complete && runtime.effective_profile() == Some(&profile) {
            let bindings = selection::SelectionBindings::profiles_only(&runtime.saved.profiles)
                .with_attachments(&runtime.m3u_attachments);
            selection::capture_channel(
                &bindings,
                selection::SelectionOwner::Profile(&profile),
                &runtime.catalog.entries,
                &entry,
            )
            .ok()
            .flatten()
        } else {
            None
        };
    runtime.playing_attachment = runtime.play_attachment.clone();
    runtime.playing = Some(Playing {
        channel_capture,
        kiosk_selection: request.kiosk_selection,
        archive: request.archive,
        identity: plx_media::route::playback_identity(&app.player.session),
        transient_owner: request.transient_owner,
        local_selection: request.local_selection,
        profile,
        entry,
        selection_id,
        source: owned_source,
        advance_on_failure,
        requires_server_lease,
    });
    let identity = runtime
        .playing
        .as_ref()
        .and_then(|playing| playing.identity.clone());
    runtime
        .playback_failure
        .admitted(runtime.play_generation, identity.clone());
    runtime.resume_floor = if automatic_resume {
        identity.zip(continuity::ResumeFloor::new(resume_floor_ms))
    } else {
        retained_resume_floor.filter(|(owner, _)| identity.as_ref() == Some(owner))
    };
    transient::plex_remote::admitted(runtime);
    bridge::dismiss_ott_catalogs(&mut app.pages);
    runtime.reply = None;
    if let Some(playing) = runtime.playing.clone() {
        if automatic_resume {
            runtime.continuity.restore_admitted(
                runtime.generation,
                &playing.selection_id,
                restore_position_ms,
            );
        }
        if playing.archive.is_none()
            && transient::plex_remote::choice(runtime, &playing.entry.id).is_none()
        {
            if runtime.playing_attachment.is_some() && playing.selection_id.starts_with("favorite:")
            {
                let _ = runtime.request_favorite_metadata(&playing.selection_id);
            } else {
                runtime.request_metadata(playing.profile.clone(), playing.entry.clone());
            }
        }
        if request.continuation.is_none() {
            runtime.transient_selection_admitted(&playing);
        }
        runtime.transient_checkpoint(&playing, position_ms);
    }
    Ok(())
}

/// Only a committed, presented source may start the calendar label. The callback
/// latch is reset by native stop/reload; pending admission must not reuse it.
fn observe_programme(app: &mut App, wall_clock_ms: u64) -> bool {
    use plx_media::player::PlaybackState;
    let ps = &app.player.session;
    let identity = plx_media::route::playback_identity(ps);
    let presented = app.running
        && playback_allowed(app)
        && plx_media::route::has_url(ps)
        && !plx_media::route::play_pending()
        && matches!(
            plx_media::player::state(ps),
            PlaybackState::Playing
                | PlaybackState::Paused
                | PlaybackState::Buffering
                | PlaybackState::Seeking
        )
        && plx_media::player::seen_frame();
    app.player
        .programme
        .observe(identity, presented, i64::try_from(wall_clock_ms).ok())
}

fn same_programme(
    a: &plx_data::metadata::OttNowPlaying,
    b: &plx_data::metadata::OttNowPlaying,
) -> bool {
    a.live == b.live
        && a.archive == b.archive
        && a.collection_label == b.collection_label
        && a.confirmed_start_unix_ms == b.confirmed_start_unix_ms
        && a.art == b.art
        && match (&a.next, &b.next) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.title == b.title && a.estimated_start_unix_ms == b.estimated_start_unix_ms
            }
            _ => false,
        }
}

/// The witness was established from the exact installed Plex leaf. Absolute
/// HTTP artwork keeps a credential-free fetch owner; only server paths use Plex.
fn programme_leaf_art(
    leaf: &plx_data::metadata::NowPlaying,
    sid: plx_plex::plex::ServerId,
) -> Option<OttPoster> {
    if let Some(url) = plx_data::ott::http_url(&leaf.thumb) {
        return Some(OttPoster::Http { url });
    }
    let key = &leaf.thumb;
    (key.starts_with('/')
        && !key.starts_with("//")
        && !key.contains('\\')
        && !key.chars().any(char::is_control))
    .then(|| OttPoster::Plex {
        server: sid.raw(),
        key: key.clone(),
    })
}

/// One presentation projection witnessed by an exact accepted Plex lifetime.
/// It is never populated from the store's retained, potentially unrelated NowPlaying.
struct ProgrammeLeaf {
    identity: plx_media::route::PlaybackIdentity,
    server_id: String,
    rating_key: String,
    value: Option<plx_data::metadata::NowPlaying>,
}

fn publish_programme(runtime: &mut Runtime, app: &mut App, changed: bool) {
    if archive_programme::publish(runtime, app) {
        return;
    }
    // Cache the existing store revision, including our own publication below. A
    // late matching Detail must refresh the footer without an OTT model change.
    let metadata_generation = app.bridge.metadata_mut().gen();
    let metadata_changed = runtime.programme_metadata_generation != Some(metadata_generation);
    runtime.programme_metadata_generation = Some(metadata_generation);
    if live_hud::publish(runtime, app, changed || metadata_changed) {
        return;
    }
    use plx_data::metadata::{NowPlaying, OttNextPlaying, OttNowPlaying};
    let Some(playing) = runtime.playing.as_ref().filter(|playing| {
        playing_matches(playing, app)
            && matches!(
                playing.profile.kind,
                ProviderKind::Local | ProviderKind::Plex | ProviderKind::VPortal
            )
    }) else {
        runtime.programme_leaf = None;
        if let Some(mut now) = app
            .bridge
            .metadata_view()
            .now_playing()
            .filter(|n| n.ott.is_some())
            .cloned()
        {
            now.ott = None;
            app.bridge
                .metadata_run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(
                    Some(now),
                ));
        }
        return;
    };
    let leaf_owner = match (&playing.identity, &playing.entry.media) {
        (
            Some(identity),
            MediaLocator::Plex {
                server_id,
                rating_key,
            },
        ) if playing.profile.kind == ProviderKind::Plex => Some((identity, server_id, rating_key)),
        _ => None,
    };
    let leaf_changed = match (&runtime.programme_leaf, leaf_owner) {
        (None, None) => false,
        (Some(retained), Some((identity, server_id, rating_key))) => {
            &retained.identity != identity
                || &retained.server_id != server_id
                || &retained.rating_key != rating_key
        }
        _ => true,
    };
    if leaf_changed {
        runtime.programme_leaf =
            leaf_owner.map(|(identity, server_id, rating_key)| ProgrammeLeaf {
                identity: identity.clone(),
                server_id: server_id.clone(),
                rating_key: rating_key.clone(),
                value: None,
            });
    }
    let current = app.bridge.metadata_view().now_playing();
    let native_duration = plx_media::player::duration_ns().max(0) / 1_000_000;
    let duration = if native_duration > 0 {
        Some(native_duration)
    } else {
        playing
            .entry
            .duration_ms
            .and_then(|v| i64::try_from(v).ok())
            .filter(|v| *v > 0)
    };
    if !changed
        && !metadata_changed
        && !leaf_changed
        && current.is_some_and(|now| now.ott.is_some() && now.dur_ms == duration.unwrap_or(0))
    {
        return;
    }
    // Catalogue enrichment may arrive after the item started, but only this
    // same source fingerprint can contribute the playing item's presentation.
    let entry = runtime
        .effective_profile()
        .filter(|profile| profile_fingerprint(profile) == profile_fingerprint(&playing.profile))
        .and_then(|_| {
            runtime
                .catalog
                .entries
                .iter()
                .find(|entry| entry.id == playing.entry.id)
        })
        .unwrap_or(&playing.entry);
    let start = playing
        .identity
        .as_ref()
        .and_then(|identity| app.player.programme.started_at(identity));
    let next = if runtime.play_target.is_some() || runtime.transient.owns_playback() {
        None
    } else {
        let ids = runtime.ordered_ids();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let anchor = runtime.strict_folder_anchor(playing);
        let next = if let Some(anchor) = anchor {
            runtime.continuity.peek_next_at(
                runtime.generation,
                &playing.selection_id,
                &anchor,
                &refs,
                runtime.selection_complete(),
            )
        } else {
            runtime.continuity.peek_next(
                runtime.generation,
                &playing.selection_id,
                &refs,
                runtime.selection_complete(),
            )
        };
        next.and_then(|id| {
            let title = if let Some(identity) = id.strip_prefix("favorite:") {
                runtime
                    .saved
                    .favorites
                    .entries()
                    .iter()
                    .find(|favorite| favorite.identity() == identity)
                    .map(|favorite| favorite.title.clone())
            } else {
                runtime
                    .catalog
                    .entries
                    .iter()
                    .find(|entry| entry.id == id)
                    .map(|entry| entry.title.clone())
            }?;
            Some(OttNextPlaying {
                title,
                estimated_start_unix_ms: start
                    .zip(duration)
                    .and_then(|(start, duration)| start.checked_add(duration)),
            })
        })
    };
    let fresh_leaf = if playing.profile.kind == ProviderKind::Plex {
        app.bridge.metadata_view().current_leaf_now_playing(
            plx_media::route::cur_sid(&app.player.session),
            &plx_media::route::cur_rk(&app.player.session),
        )
    } else {
        None
    };
    if let (Some(retained), Some(fresh)) = (&mut runtime.programme_leaf, &fresh_leaf) {
        let mut value = fresh.clone();
        // Clock, successor and artwork projection remain live Runtime values.
        value.ott = None;
        retained.value = Some(value);
    }
    let leaf = fresh_leaf.or_else(|| {
        runtime
            .programme_leaf
            .as_ref()
            .and_then(|retained| retained.value.clone())
    });
    let art = if playing.profile.kind == ProviderKind::Local {
        runtime.local_playing_art(playing)
    } else {
        entry_poster(entry).or_else(|| {
            leaf_owner.and_then(|_| leaf.as_ref()).and_then(|leaf| {
                programme_leaf_art(leaf, plx_media::route::cur_sid(&app.player.session))
            })
        })
    };
    let collection_label = runtime
        .local_playing_collection_label(playing)
        .unwrap_or(&entry.category)
        .to_owned();
    let projection = OttNowPlaying {
        live: None,
        archive: None,
        collection_label,
        confirmed_start_unix_ms: start,
        next,
        art,
    };
    let mut now = leaf.unwrap_or_else(|| NowPlaying {
        ott: None,
        is_episode: false,
        is_real_episode: false,
        title: entry.title.clone(),
        ep_title: String::new(),
        season: 0,
        index: 0,
        summary: entry.description.clone(),
        year: 0,
        dur_ms: duration.unwrap_or(0),
        rating: String::new(),
        thumb: String::new(),
        detail_rk: String::new(),
    });
    if !entry.description.is_empty() {
        now.summary = entry.description.clone();
    }
    now.dur_ms = duration.unwrap_or(0);
    let unchanged = current.is_some_and(|old| {
        old.title == now.title
            && old.ep_title == now.ep_title
            && old.is_episode == now.is_episode
            && old.is_real_episode == now.is_real_episode
            && old.season == now.season
            && old.index == now.index
            && old.year == now.year
            && old.rating == now.rating
            && old.thumb == now.thumb
            && old.detail_rk == now.detail_rk
            && old.summary == now.summary
            && old.dur_ms == now.dur_ms
            && old
                .ott
                .as_ref()
                .is_some_and(|old| same_programme(old, &projection))
    });
    if unchanged {
        return;
    }
    now.ott = Some(projection);
    app.bridge
        .metadata_run(plx_data::stores::metadata::MetadataCmd::SetNowPlaying(
            Some(now),
        ));
    runtime.programme_metadata_generation = Some(app.bridge.metadata_mut().gen());
}

/// Read-only completed-work projection. Frame times belong to the instrument's
/// runtime epoch, not the catalogue generation. This never drains or resets either.
pub(super) fn performance_snapshot(app: &App, runtime: &Runtime) -> Value {
    json!({
        "version": 1,
        "frame": performance_frame(app.instr.timing_snapshot()),
        "catalog": runtime.catalog_diagnostics.performance_counts(runtime.generation),
    })
}

fn performance_frame(snapshot: plx_base::diag::heartbeat::TimingSnapshot) -> Value {
    use plx_base::diag::{heartbeat::Phase, spans::SpanLabel};
    let legs: serde_json::Map<String, Value> = Phase::TIMED
        .into_iter()
        .zip(snapshot.legs)
        .map(|(phase, timing)| {
            (
                phase.as_str().to_owned(),
                json!({
                    "last_us": timing.last_us, "max_us": timing.max_us,
                    "samples": timing.samples, "invalid_samples": timing.invalid_samples,
                }),
            )
        })
        .collect();
    let spans: serde_json::Map<String, Value> = SpanLabel::ALL
        .into_iter()
        .zip(snapshot.spans)
        .map(|(label, timing)| {
            (
                label.as_str().to_owned(),
                json!({
                    "last_sum_us": timing.last_sum_us, "max_frame_sum_us": timing.max_frame_sum_us,
                    "last_call_max_us": timing.last_call_max_us, "max_call_us": timing.max_call_us,
                    "last_calls": timing.last_calls, "samples": timing.samples,
                    "total_calls": timing.total_calls, "invalid_samples": timing.invalid_samples,
                }),
            )
        })
        .collect();
    json!({
        "scope": snapshot.scope, "epoch": snapshot.epoch, "sequence": snapshot.sequence,
        "armed": snapshot.armed, "presented": snapshot.presented,
        "measured_iterations": snapshot.measured_iterations,
        "longest_phase": snapshot.longest_phase.map(Phase::as_str),
        "legs": legs, "spans": spans,
    })
}

#[cfg(test)]
mod performance_snapshot_tests {
    use super::performance_frame;
    use plx_base::diag::{
        heartbeat::{LegTiming, Phase, SpanTiming, TimingSnapshot},
        spans::SpanLabel,
    };
    use serde_json::json;

    fn empty() -> TimingSnapshot {
        TimingSnapshot {
            scope: "instrument_runtime_epoch",
            epoch: 1,
            sequence: 0,
            armed: false,
            presented: None,
            measured_iterations: 0,
            longest_phase: None,
            legs: [LegTiming::default(); Phase::TIMED.len()],
            spans: [SpanTiming::default(); SpanLabel::ALL.len()],
        }
    }

    #[test]
    fn unmeasured_performance_exports_null_and_only_closed_labels() {
        let frame = performance_frame(empty());
        assert_eq!(frame["scope"], "instrument_runtime_epoch");
        assert_eq!(frame["armed"], false);
        assert!(frame["presented"].is_null());
        assert!(frame["longest_phase"].is_null());
        assert_eq!(frame["legs"].as_object().unwrap().len(), Phase::TIMED.len());
        for phase in Phase::TIMED {
            let leg = &frame["legs"][phase.as_str()];
            assert!(leg["last_us"].is_null());
            assert!(leg["max_us"].is_null());
            assert_eq!(leg["samples"], 0);
        }
        assert_eq!(
            frame["spans"].as_object().unwrap().len(),
            SpanLabel::ALL.len()
        );
        for label in SpanLabel::ALL {
            let span = &frame["spans"][label.as_str()];
            for key in [
                "last_sum_us",
                "max_frame_sum_us",
                "last_call_max_us",
                "max_call_us",
            ] {
                assert!(span[key].is_null());
            }
            assert_eq!(span["total_calls"], 0);
        }
    }

    #[test]
    fn performance_projection_preserves_zero_and_distinguishes_sum_from_call_max() {
        let mut snapshot = empty();
        snapshot.armed = true;
        snapshot.sequence = 9;
        snapshot.presented = Some(false);
        snapshot.measured_iterations = 9;
        snapshot.longest_phase = Some(Phase::Results);
        snapshot.legs[1] = LegTiming {
            last_us: Some(0),
            max_us: Some(120),
            samples: 9,
            invalid_samples: 0,
        };
        snapshot.spans[SpanLabel::OttMerge as usize] = SpanTiming {
            last_sum_us: Some(90),
            max_frame_sum_us: Some(120),
            last_call_max_us: Some(50),
            max_call_us: Some(80),
            last_calls: 2,
            samples: 3,
            total_calls: 5,
            invalid_samples: 1,
        };
        let before = snapshot;
        let frame = performance_frame(snapshot);
        assert_eq!(
            frame,
            performance_frame(snapshot),
            "projection must be non-destructive"
        );
        assert_eq!(snapshot, before);
        assert_eq!(frame["sequence"], 9);
        assert_eq!(frame["presented"], false);
        assert_eq!(frame["longest_phase"], "results");
        assert_eq!(frame["legs"]["results"]["last_us"], 0);
        assert!(frame["legs"]["draw"]["last_us"].is_null());
        assert_eq!(
            frame["spans"]["ott_merge"],
            json!({
                "last_sum_us":90, "max_frame_sum_us":120, "last_call_max_us":50,
                "max_call_us":80, "last_calls":2, "samples":3, "total_calls":5, "invalid_samples":1,
            })
        );
    }
}

fn state(app: &App, runtime: &Runtime) -> Value {
    use plx_media::player::PlaybackState;
    let current = plx_media::player::state(&app.player.session);
    let failed = failed_playback::current(runtime, app);
    let active = current != PlaybackState::Idle || failed.is_some();
    let state = if app.ott_standby {
        "standby"
    } else if runtime.play_target.is_some() {
        "loading"
    } else if failed.is_some() {
        "error"
    } else {
        match current {
            PlaybackState::Idle => "idle",
            PlaybackState::Resolving => "loading",
            PlaybackState::Connecting => "loading",
            PlaybackState::Buffering => "buffering",
            PlaybackState::Seeking => "buffering",
            PlaybackState::Error => "error",
            PlaybackState::Paused => "paused",
            PlaybackState::Playing => {
                if plx_media::player::transport::paused() {
                    "paused"
                } else {
                    "playing"
                }
            }
        }
    };
    let playing = runtime
        .playing
        .as_ref()
        .filter(|p| active && playing_matches(p, app))
        .or(failed);
    let mut state = json!({"player":{"version":env!("PLX_VERSION"),"build_sha":env!("PLX_BUILD_SHA"),"platform":"webos-native","runtime":runtime.remote.runtime(),"standby":app.ott_standby},
        "playback":{"state":state,"kind":if !active {"none"} else if playing.is_some_and(|p|p.archive.is_some()) {"archive"} else if playing.is_some_and(|p|p.profile.kind==ProviderKind::M3u) {"live"} else {"vod"},
        "title": playing.map(|p|&p.entry.title),"provider":playing.map(|p|if p.profile.kind==ProviderKind::Local {"offline"} else {p.profile.kind.canonical_id()}),"media_id":playing.map(|p|&p.entry.id),"channel_number":null,
        "position":active.then(||playback::playpos().max(0) as f64/1e9),"duration":(active&&playback::dur()>0).then(||playback::dur() as f64/1e9),"volume":null}});
    companion_adapter::decorate_state(runtime, &mut state);
    state["playback"] = super::ott_remote::project_playback(&state["playback"]);
    state
}

fn profile_fingerprint(profile: &ProviderProfile) -> String {
    plx_data::ott::favorites_wire::hash(&serde_json::to_vec(profile).unwrap_or_default())
}

fn entry_poster(entry: &CatalogEntry) -> Option<OttPoster> {
    if let MediaLocator::Plex { server_id, .. } = &entry.media {
        plx_plex::plex::id_of_machine(server_id)
            .filter(|_| !entry.poster.is_empty())
            .map(|sid| OttPoster::Plex {
                server: sid.raw(),
                key: entry.poster.clone(),
            })
    } else {
        plx_data::ott::http_url(&entry.poster).map(|url| OttPoster::Http { url })
    }
}
fn enrich_entry(entry: &mut CatalogEntry, details: &Value) {
    if let Some(text) = details["description"]
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= plx_data::ott::MAX_METADATA_BYTES)
    {
        entry.description = text.into();
    }
    if let Some(url) = details["poster"].as_str().and_then(plx_data::ott::http_url) {
        entry.poster = url;
    }
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_runtime_tests.rs"]
mod tests;

/// Shared UI/backend admission; status and lifecycle do not pass through this gate.
pub(super) fn kiosk_user_keys_blocked(app: &App) -> bool {
    app.ott
        .as_ref()
        .is_some_and(|r| input_policy::pending(r) || input_policy::strict(r))
}
pub(super) fn kiosk_player_request(app: &App, request: &plx_screens::registry::PlayerReq) -> bool {
    app.ott
        .as_ref()
        .is_none_or(|r| input_policy::player(r, request))
}
pub(super) fn foreign_content_allowed(app: &mut App) -> bool {
    app.ott
        .as_mut()
        .is_none_or(|r| input_policy::foreign_content(r).is_ok())
}
#[cfg(test)]
#[path = "ott_reminder_wiring_tests.rs"]
mod reminder_wiring_tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DesktopQuit {
    Pass,
    Consume,
    Exit,
}

/// Physical desktop Q preserves the existing exit lifecycle and requires no OTT runtime.
pub(super) fn desktop_quit(app: &mut App) -> DesktopQuit {
    desktop_input::quit(app)
}

/// Physical Mac keyboard controls for an unobstructed fullscreen video.
pub(super) fn desktop_key(app: &mut App) -> bool {
    desktop_input::ingest(app)
}

/// Action-free physical release retirement precedes standby and kiosk input gates.
pub(super) fn desktop_key_boundary(app: &mut App) -> bool {
    desktop_input::boundary(app)
}
