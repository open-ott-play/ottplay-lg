//! Passive current-source track receipts. The native route remains the only
//! selection owner; this adapter neither probes media nor persists preferences.
use super::*;
use super::super::input_policy;
use crate::app::ott_remote::media_tracks as wire;
use crate::app::{ott_kiosk_policy as kiosk, ott_work_policy as work};
use plx_media::ff::inspect::{InspectedTrack, TrackKind};
use plx_media::route::{self, ExternalTrackChoice, ExternalTracks, TrackAvailability};

#[path = "ott_media_tracks_plex.rs"]
mod plex;
#[path = "ott_media_tracks_hls.rs"]
pub(in crate::app::ott_runtime) mod hls;

#[derive(Clone, Debug)]
enum NativeInventory {
    External(ExternalTracks),
    Plex(route::PlexTracks),
    Hls(route::HlsTrackCatalog),
}
impl NativeInventory {
    fn same_catalog(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::External(a), Self::External(b)) => a.same_session(b),
            (Self::Plex(a), Self::Plex(b)) => a == b,
            (Self::Hls(a), Self::Hls(b)) => a.same_session(b),
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
enum NativeChoice {
    External(ExternalTrackChoice),
    Plex(route::PlexTrackChoice),
    Hls(route::HlsTrackChoice),
}

enum NativeRequest {
    External(route::ExternalTrackRequest),
    Plex(route::PlexTrackRequest),
}
impl NativeRequest {
    fn status(&self, ps: &route::PlaybackSession) -> route::ExternalTrackRequestStatus {
        match self {
            Self::External(request) => route::external_track_request_status(ps, request),
            Self::Plex(request) => route::plex_track_request_status(ps, request),
        }
    }
    fn cancel(&self, ps: &mut route::PlaybackSession) {
        match self {
            Self::External(request) => { route::cancel_external_track_request(ps, request); }
            Self::Plex(request) => { route::cancel_plex_track_request(ps, request); }
        }
    }
}

pub(super) type PolicyReader = fn(&Runtime) -> Result<kiosk::State, ()>;

#[derive(Clone, Debug)]
struct Owner {
    runtime: String,
    playback: route::PlaybackIdentity,
    worker: route::WorkerTicket,
    inventory: NativeInventory,
    intent: route::TrackIntentTicket,
    kiosk: kiosk::State,
    mutable: bool,
}
impl PartialEq for Owner {
    fn eq(&self, other: &Self) -> bool {
        self.runtime == other.runtime
            && self.playback == other.playback
            && self.worker == other.worker
            && self.inventory.same_catalog(&other.inventory)
            && self.intent == other.intent
            && self.kiosk == other.kiosk
            && self.mutable == other.mutable
    }
}
impl Eq for Owner {}
struct Binding {
    owner: Owner,
    playback_token: String,
    context: Option<wire::Context<Owner>>,
}
#[derive(Default)]
pub(super) struct State {
    catalog: wire::Catalog<Owner, NativeChoice>,
    binding: Option<Binding>,
    pending: Option<Pending>,
}
struct Pending {
    invocation: Invocation,
    request: wire::Request,
    selection: wire::Selection<Owner, NativeChoice>,
    owner: Owner,
    route: NativeRequest,
}
impl State {
    fn invalidate(&mut self) {
        self.catalog.invalidate();
        if let Some(binding) = &mut self.binding {
            binding.context = None;
        }
    }
}

/// Current passive list availability, not a promise that a mutation can run.
/// This captures existing facts only: no IDs, leases, probes or effects.
pub(super) fn supports_current(runtime: &Runtime, app: &App, policy: PolicyReader) -> bool {
    capture(runtime, app, policy).is_ok()
}

fn complete_inventory(native: &ExternalTracks) -> bool {
    let embedded = native.inventory();
    let subtitles = embedded.subtitles.len().saturating_add(native.local_subtitles().len());
    native.is_complete()
        && embedded.audio.len() <= wire::MAX_TRACKS_PER_KIND
        && subtitles <= wire::MAX_TRACKS_PER_KIND
        && embedded.audio.len().saturating_add(subtitles) <= wire::MAX_TRACKS
}

fn capture(runtime: &Runtime, app: &App, policy: PolicyReader) -> Result<Owner, wire::Error> {
    if !runtime.loaded || runtime.persistence_uncertain {
        return Err(wire::Error::Unavailable);
    }
    let ps = &app.player.session;
    if !route::has_url(ps) {
        return Err(wire::Error::NoPlayback);
    }
    match plx_media::player::state(ps) {
        plx_media::player::PlaybackState::Idle | plx_media::player::PlaybackState::Error => {
            return Err(wire::Error::NoPlayback)
        }
        plx_media::player::PlaybackState::Resolving
        | plx_media::player::PlaybackState::Connecting => return Err(wire::Error::Busy),
        _ => {}
    }
    let playback = route::playback_identity(ps).ok_or(wire::Error::NoPlayback)?;
    let inventory = match route::external_source(ps) {
        Some(plx_media::ff::ExternalSource::Hls(_)) => {
            if !super::super::hls_tracks::source_available(runtime) { return Err(wire::Error::Unavailable); }
            let tracks = route::hls_track_catalog(ps).ok_or(wire::Error::Unsupported)?;
            if !hls::complete(&tracks) { return Err(wire::Error::CatalogIncomplete); }
            NativeInventory::Hls(tracks)
        },
        Some(_) => {
            let tracks = route::external_tracks(ps).ok_or(wire::Error::CatalogIncomplete)?;
            if !complete_inventory(tracks) {
                return Err(wire::Error::CatalogIncomplete);
            }
            NativeInventory::External(tracks.clone())
        }
        None => NativeInventory::Plex(plex::capture(app)?),
    };
    let kiosk = policy(runtime).map_err(|_| wire::Error::Restricted)?;
    Ok(Owner {
        runtime: runtime.remote.runtime().into(),
        playback,
        worker: route::worker_ticket(),
        inventory,
        intent: route::track_intent_ticket(),
        kiosk,
        // Do not consult this adapter's pending receipt: command temporarily
        // takes it out of Runtime. The separate reservation is checked at apply.
        mutable: mutation_allowed(runtime, app, kiosk).is_ok(),
    })
}

fn label(track: &InspectedTrack) -> String {
    if !track.title.trim().is_empty() {
        return track.title.clone();
    }
    if let Some(key) = plx_data::metadata::lang_key(&track.language) {
        return plx_plex::plex::languages::label(&key).to_string();
    }
    format!("Track {}", track.ordinal + 1)
}
fn inventory(
    native: &ExternalTracks,
    mutable: bool,
    token: &str,
) -> wire::Inventory<ExternalTrackChoice> {
    if !complete_inventory(native) {
        return wire::Inventory::Incomplete;
    }
    let mut tracks = Vec::new();
    for (kind, rows, prefix) in [
        (wire::Kind::Audio, &native.inventory().audio, "a"),
        (wire::Kind::Subtitle, &native.inventory().subtitles, "s"),
    ] {
        for track in rows {
            let choice = (mutable
                && native.availability(track.kind, track.ordinal) == TrackAvailability::Supported)
                .then(|| native.choice(track.kind, Some(track.ordinal)).ok())
                .flatten();
            tracks.push(wire::Track {
                id: format!("{token}_{prefix}{}", track.ordinal),
                kind,
                label: label(track),
                language: Some(track.language.clone()),
                codec: Some(track.codec.clone()),
                choice,
            });
        }
    }
    for (index, track) in native.local_subtitles().iter().enumerate() {
        tracks.push(wire::Track {
            // This is a short-lived catalog row ID, not the local target's
            // identity or an index callers may reinterpret as a native ordinal.
            id: format!("{token}_l{index}"),
            kind: wire::Kind::Subtitle,
            label: track.title.clone(),
            language: Some(track.language.clone()),
            codec: Some(track.format.codec().into()),
            choice: mutable
                .then(|| native.local_choice(&track.target).ok())
                .flatten(),
        });
    }
    wire::Inventory::Complete {
        tracks,
        subtitle_off: mutable
            .then(|| native.choice(TrackKind::Subtitle, None).ok())
            .flatten(),
    }
}

fn native_inventory(
    native: &NativeInventory,
    mutable: bool,
    token: &str,
) -> wire::Inventory<NativeChoice> {
    match native {
        NativeInventory::Plex(tracks) => plex::inventory(tracks, mutable, token),
        NativeInventory::Hls(tracks) => hls::inventory(tracks, mutable, token),
        NativeInventory::External(tracks) => match inventory(tracks, mutable, token) {
            wire::Inventory::Incomplete => wire::Inventory::Incomplete,
            wire::Inventory::Complete { tracks, subtitle_off } => wire::Inventory::Complete {
                tracks: tracks.into_iter().map(|track| wire::Track {
                    id: track.id, kind: track.kind, label: track.label,
                    language: track.language, codec: track.codec,
                    choice: track.choice.map(NativeChoice::External),
                }).collect(),
                subtitle_off: subtitle_off.map(NativeChoice::External),
            },
        },
    }
}
fn synchronize(state: &mut State, runtime: &mut Runtime, owner: Owner) -> Result<(), wire::Error> {
    if state
        .binding
        .as_ref()
        .is_some_and(|binding| binding.context.is_some() && binding.owner == owner)
    {
        return Ok(());
    }
    let same_playback = state
        .binding
        .as_ref()
        .filter(|b| b.owner.playback == owner.playback)
        .map(|binding| binding.playback_token.clone());
    state.invalidate();
    let playback_token = match same_playback {
        Some(token) => token,
        None => super::super::favorites::nonce(runtime).map_err(|_| wire::Error::Unavailable)?,
    };
    let source_token =
        super::super::favorites::nonce(runtime).map_err(|_| wire::Error::Unavailable)?;
    let catalog_token =
        super::super::favorites::nonce(runtime).map_err(|_| wire::Error::Unavailable)?;
    let context = wire::Context::new(
        owner.clone(),
        runtime.remote.runtime().into(),
        playback_token.clone(),
        source_token,
    )?;
    state.catalog.install(
        &context,
        context.clone(),
        catalog_token.clone(),
        native_inventory(&owner.inventory, owner.mutable, &catalog_token),
    )?;
    state.binding = Some(Binding {
        owner,
        playback_token,
        context: Some(context),
    });
    Ok(())
}

fn invocation_current(runtime: &Runtime, invocation: &Invocation) -> Result<(), wire::Error> {
    if !runtime.remote.invocation_is_current(invocation) {
        return Err(wire::Error::RuntimeMismatch);
    }
    if runtime.now_ms() >= invocation.deadline_ms {
        return Err(wire::Error::Unavailable);
    }
    Ok(())
}
fn mutation_allowed(runtime: &Runtime, app: &App, policy: kiosk::State) -> Result<(), wire::Error> {
    if input_policy::pending(runtime)
        || !app.running
        || !work::playback_allowed(app.window_activity.allow_present(true), app.ott_standby)
        || !kiosk::remote_track_selection_allowed(policy)
    {
        return Err(wire::Error::Restricted);
    }
    if !controls::transport_ready(app) {
        return Err(wire::Error::Busy);
    }
    Ok(())
}
fn apply(
    state: &mut State,
    runtime: &mut Runtime,
    app: &mut App,
    invocation: &Invocation,
    request: &wire::Request,
    selection: wire::Selection<Owner, NativeChoice>,
    policy: PolicyReader,
) -> Result<Option<wire::Response>, wire::Error> {
    invocation_current(runtime, invocation)?;
    let owner = capture(runtime, app, policy)?;
    mutation_allowed(runtime, app, owner.kiosk)?;
    let binding = state.binding.as_ref().ok_or(wire::Error::CatalogChanged)?;
    if owner != binding.owner {
        return Err(wire::Error::CatalogChanged);
    }
    state.catalog.recheck(&selection, binding.context.as_ref(), runtime.now_ms())?;
    if state.pending.is_some() {
        return Err(wire::Error::Busy);
    }
    // Subtitles/no-ops commit synchronously. Ordinary audio waits for its native
    // attempt; HLS waits for prepared selection publication by the shared owner.
    let pending = match selection.choice() {
        NativeChoice::Hls(choice) => {
            let route = route::begin_hls_track_request(&mut app.player.session, choice)
                .map_err(hls::apply_error)?;
            let recipient = hls::Remote::new(invocation.clone(), request.clone(), selection, owner, policy);
            super::super::hls_tracks::enqueue_remote(runtime, app, route, recipient);
            return Ok(None);
        }
        NativeChoice::External(choice) => route::apply_external_track_request(&mut app.player.session, choice)
            .map(|request| request.map(NativeRequest::External))
            .map_err(|error| match error {
                route::ExternalTrackApplyError::Busy => wire::Error::Busy,
                route::ExternalTrackApplyError::Inactive => wire::Error::NoPlayback,
                route::ExternalTrackApplyError::InvalidChoice(_) => wire::Error::NotSelectable,
            })?,
        NativeChoice::Plex(choice) => {
            // Convert the original absolute expiry, never a newly granted budget.
            let anchor = runtime.clock_anchor.or(runtime.clock_sample).ok_or(wire::Error::Unavailable)?;
            let deadline = anchor.deadline_at_unix_ms(invocation.deadline_ms.min(selection.deadline_ms()));
            route::apply_plex_track_request(&mut app.player.session, choice, deadline, app.adapters.player.mt())
                .map(|request| request.map(NativeRequest::Plex))
                .map_err(plex::apply_error)?
        }
    };
    if let Some(route) = pending {
        state.pending = Some(Pending {
            invocation: invocation.clone(), request: request.clone(), selection, owner, route,
        });
        Ok(None)
    } else {
        Ok(Some(selection.requested_response()))
    }
}

pub(super) fn command(
    runtime: &mut Runtime,
    app: &mut App,
    invocation: &Invocation,
    policy: PolicyReader,
) -> Option<Value> {
    let request = invocation
        .media_tracks_projection()
        .and_then(|raw| wire::Request::decode(raw).ok());
    let Some(request) = request else {
        return Some(rejected("Invalid raw media_tracks request"));
    };
    let mut state = std::mem::take(&mut runtime.command_receipts.media_tracks);
    let result = (|| {
        invocation_current(runtime, invocation)?;
        if request.runtime() != runtime.remote.runtime() {
            return Err(wire::Error::RuntimeMismatch);
        }
        let owner = capture(runtime, app, policy).inspect_err(|_| state.invalidate())?;
        if request.operation() == "select" {
            // Preserve the current Restricted/Busy reason before a changed
            // admission snapshot remints context. Retire any old mapping even
            // if the caller returns to an eligible state before the next tick.
            mutation_allowed(runtime, app, owner.kiosk).inspect_err(|_| state.invalidate())?;
        }
        synchronize(&mut state, runtime, owner)?;
        let live = state
            .binding
            .as_ref()
            .and_then(|binding| binding.context.as_ref());
        if request.operation() == "list" {
            return state.catalog.list(&request, live, runtime.now_ms()).map(Some);
        }
        let selection =
            state
                .catalog
                .select(&request, live, runtime.now_ms(), invocation.deadline_ms)?;
        apply(&mut state, runtime, app, invocation, &request, selection, policy)
    })();
    runtime.command_receipts.media_tracks = state;
    result
        .unwrap_or_else(|error| Some(request.error_response(error)))
        .map(wire::Response::into_envelope)
}

/// Retire stale passive receipts at the existing command tick. This neither
/// discovers media nor mints tokens; new mappings are minted only by list/select.
pub(super) fn retire_stale(runtime: &mut Runtime, app: &App, policy: PolicyReader) {
    let Some(binding) = runtime.command_receipts.media_tracks.binding.as_ref() else {
        return;
    };
    if !capture(runtime, app, policy).is_ok_and(|owner| owner == binding.owner) {
        runtime.command_receipts.media_tracks.invalidate();
    }
}

pub(super) fn has_pending(runtime: &Runtime) -> bool {
    runtime.command_receipts.media_tracks.pending.is_some()
        || super::super::hls_tracks::has_remote_pending(runtime)
}

/// Only Plex requests carry the fixed-deadline native worker guard. Preserve
/// the existing External request / kiosk cancellation ordering.
pub(super) fn has_pending_plex(runtime: &Runtime) -> bool {
    runtime.command_receipts.media_tracks.pending.as_ref()
        .is_some_and(|pending| matches!(&pending.route, NativeRequest::Plex(_)))
}

/// Run before the pump as well as at the ordinary command tick. In particular,
/// a stalled frame must not execute an expired request before checking its time.
/// A clock failure cancels the addressed request instead of extending it.
pub(super) fn poll_pending(
    runtime: &mut Runtime,
    app: &mut App,
    policy: PolicyReader,
    now_ms: Option<u64>,
) {
    super::super::hls_tracks::poll_remote(runtime, app, now_ms);
    // The core retires only its exact terminal serial. This also drains a late
    // terminal event after this adapter has already returned an error.
    route::retire_terminal_plex_track_request(&mut app.player.session);
    let Some(pending) = runtime.command_receipts.media_tracks.pending.take() else { return; };
    let result = (|| {
        let now = now_ms.ok_or(wire::Error::Unavailable)?;
        if !runtime.remote.invocation_is_current(&pending.invocation)
            || runtime.remote.runtime() != pending.owner.runtime
        {
            return Err(wire::Error::RuntimeMismatch);
        }
        if now >= pending.invocation.deadline_ms || now >= pending.selection.deadline_ms() {
            return Err(wire::Error::Unavailable);
        }
        if !runtime.loaded || runtime.persistence_uncertain {
            return Err(wire::Error::Unavailable);
        }
        let current_policy = policy(runtime).map_err(|_| wire::Error::Restricted)?;
        // Saved policy still describes the old state while the shared kiosk
        // writer/capture owns a transition. Honor its input fence before the
        // native pump can claim this request.
        if input_policy::pending(runtime)
            || !app.running || !work::playback_allowed(app.window_activity.allow_present(true), app.ott_standby)
            || current_policy != pending.owner.kiosk
            || !kiosk::remote_track_selection_allowed(current_policy)
        {
            return Err(wire::Error::Restricted);
        }
        if route::playback_identity(&app.player.session).as_ref() != Some(&pending.owner.playback) {
            return Err(wire::Error::PlaybackChanged);
        }
        match pending.route.status(&app.player.session) {
            route::ExternalTrackRequestStatus::Queued | route::ExternalTrackRequestStatus::Pending => Ok(None),
            route::ExternalTrackRequestStatus::Started => Ok(Some(pending.selection.requested_response())),
            route::ExternalTrackRequestStatus::Failed => Err(wire::Error::Unavailable),
            route::ExternalTrackRequestStatus::Superseded => Err(wire::Error::CatalogChanged),
        }
    })();
    match result {
        Ok(None) => runtime.command_receipts.media_tracks.pending = Some(pending),
        result => {
            if result.is_err() {
                // Cancel only this addressed request. Plex also revokes its
                // claimed/deferred effect guard; it cannot undo a Load already
                // performed or revoke a later accepted intent.
                pending.route.cancel(&mut app.player.session);
            }
            if runtime.command_receipts.media_tracks.binding.as_ref().is_some_and(|binding|
                binding.owner == pending.owner)
            {
                runtime.command_receipts.media_tracks.invalidate();
            }
            route::retire_terminal_plex_track_request(&mut app.player.session);
            let response = result.unwrap_or_else(|error| Some(pending.request.error_response(error)))
                .expect("terminal selection response");
            runtime.remote.complete(pending.invocation, response.into_envelope());
        }
    }
}

#[cfg(test)]
mod local_projection_tests {
    use super::*;
    use plx_media::ff::inspect::TrackInventory;
    use route::{ExternalTrackError, LocalSubtitleTrack, SidecarFormat};

    fn local(title: &str) -> LocalSubtitleTrack {
        LocalSubtitleTrack {
            target: Default::default(),
            format: SidecarFormat::Srt,
            title: title.into(),
            language: "eng".into(),
        }
    }
    fn native(rows: Vec<LocalSubtitleTrack>, complete: bool) -> ExternalTracks {
        let track = InspectedTrack {
            kind: TrackKind::Subtitle,
            stream_index: 1,
            ordinal: 0,
            codec: "subrip".into(),
            language: "spa".into(),
            title: "Embedded".into(),
            channels: 0,
            sample_rate: 0,
            bit_depth: 0,
            decoder_available: true,
            disposition_known: false,
            default: false,
            forced: false,
        };
        let mut native = ExternalTracks::new(
            TrackInventory::complete_for_test(vec![track]).unwrap(),
            Vec::new(),
            vec![TrackAvailability::Supported],
            None,
        )
        .unwrap();
        native
            .replace_local_subtitles(rows, complete, wire::MAX_TRACKS, wire::MAX_TRACKS_PER_KIND)
            .unwrap();
        native
    }
    fn complete(
        native: &ExternalTracks,
        policy: kiosk::State,
    ) -> (
        Vec<wire::Track<ExternalTrackChoice>>,
        Option<ExternalTrackChoice>,
    ) {
        let wire::Inventory::Complete {
            tracks,
            subtitle_off,
        } = inventory(native, kiosk::remote_track_selection_allowed(policy), "c-fixture")
        else {
            panic!("complete fixture");
        };
        (tracks, subtitle_off)
    }

    #[test]
    fn local_projection_is_distinct_from_embedded_zero_and_off() {
        let row = local("Adjacent English");
        let native = native(vec![row.clone()], true);
        let (tracks, off) = complete(&native, kiosk::State::Off);
        assert_eq!(tracks.len(), 2);
        assert_ne!(tracks[0].id, tracks[1].id);
        assert_eq!(tracks[0].choice.as_ref().unwrap().ordinal(), Some(0));
        assert!(tracks[0].choice.as_ref().unwrap().local_target().is_none());
        let chosen = tracks[1].choice.as_ref().unwrap();
        assert_eq!(chosen.local_target(), Some(&row.target));
        assert_eq!(chosen.ordinal(), None);
        assert!(off.unwrap().local_target().is_none());
        assert_eq!(tracks[1].codec.as_deref(), Some("srt"));
        assert_eq!(tracks[1].label, row.title);
    }

    #[test]
    fn incomplete_adjacent_discovery_does_not_issue_remote_receipts() {
        let native = native(vec![local("Known row")], false);
        assert!(!native.local_subtitles().is_empty());
        assert!(matches!(
            inventory(&native, true, "c-fixture"),
            wire::Inventory::Incomplete
        ));
    }

    #[test]
    fn kiosk_keeps_local_rows_visible_but_not_selectable() {
        let native = native(vec![local("Known row")], true);
        let (tracks, off) = complete(&native, kiosk::State::Locked { strict: true });
        assert_eq!(tracks.len(), 2);
        assert!(tracks.iter().all(|row| row.choice.is_none()));
        assert!(off.is_none());
    }

    #[test]
    fn replacement_invalidates_projected_choice_even_at_identical_row_index() {
        let mut native = native(vec![local("Same title")], true);
        let (tracks, _) = complete(&native, kiosk::State::Off);
        let old = tracks[1].choice.as_ref().unwrap().clone();
        native
            .replace_local_subtitles(
                vec![local("Same title")],
                true,
                wire::MAX_TRACKS,
                wire::MAX_TRACKS_PER_KIND,
            )
            .unwrap();
        assert_eq!(native.plan(&old), Err(ExternalTrackError::StaleSession));
        let (new, _) = complete(&native, kiosk::State::Off);
        assert_ne!(old, new[1].choice.as_ref().unwrap().clone());
    }

    #[test]
    fn remote_projection_uses_existing_wire_metadata_and_label_limits() {
        let mut row = local(&"x".repeat(wire::MAX_LABEL_BYTES + 1));
        row.language = "x".repeat(wire::MAX_METADATA_BYTES + 1);
        let native = native(vec![row], true);
        let mut catalog = wire::Catalog::default();
        let context = wire::Context::new(
            7u64,
            "native-example".into(),
            "p-example".into(),
            "s-example".into(),
        )
        .unwrap();
        catalog
            .install(
                &context,
                context.clone(),
                "c-example".into(),
                inventory(&native, true, "c-example"),
            )
            .unwrap();
        let request = wire::Request::decode(
            br#"{"action":"media_tracks","params":{"op":"list","runtime":"native-example"}}"#,
        )
        .unwrap();
        let response = catalog.list(&request, Some(&context), 100).unwrap();
        let rows = response.data()["tracks"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[1]["label"].as_str().unwrap().len() <= wire::MAX_LABEL_BYTES);
        assert!(rows[1]["language"].is_null());
        assert_eq!(rows[1]["codec"], "srt");
    }

    #[test]
    fn projection_refuses_total_subtitle_overflow_instead_of_truncating() {
        let mut native = native(Vec::new(), true);
        // Deliberately inject a producer using a looser cap. Projection must
        // enforce the existing remote contract independently and not truncate.
        let rows = (0..wire::MAX_TRACKS_PER_KIND)
            .map(|_| local("Known row"))
            .collect();
        native
            .replace_local_subtitles(
                rows,
                true,
                wire::MAX_TRACKS + 1,
                wire::MAX_TRACKS_PER_KIND + 1,
            )
            .unwrap();
        assert!(matches!(
            inventory(&native, true, "c-fixture"),
            wire::Inventory::Incomplete
        ));
    }
}
