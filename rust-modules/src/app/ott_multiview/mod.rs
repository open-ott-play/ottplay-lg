//! Native OTT scene coordinator. This is a reducer over opaque source handles and the pure data
//! scene. It does not choose a provider, duplicate parental rules, or claim a backend decoder limit.
//! The owner translates returned typed effects into worker/native commands and reports actual
//! completions back here.

use super::multiview_sources::{
    AuthorizedInput, Error as SourceError, PreparedMetadata, Snapshot, SourceHandle, SourceRow,
    SourceWork, Support,
};
use super::{App, Runtime};
use plx_data::ott::multiview::{
    AdmissionOutcome, AudioPolicy, Error as SceneError, Layout, ResolveOutcome, ResolveTicket,
    Scene, SceneId, SlotFailure, SlotId, SlotStatus,
};
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static NEXT_SCENE_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_UI_SERIAL: AtomicU64 = AtomicU64::new(1);

/// Process-scoped scene incarnation authority. IDs are never reused in this process; stale worker
/// events cannot cross process lifetimes because workers and their channels are process-owned.
fn mint(counter: &AtomicU64) -> Result<NonZeroU64, Error> {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        let next = current.checked_add(1).ok_or(Error::IdentityExhausted)?;
        let id = NonZeroU64::new(current).ok_or(Error::IdentityExhausted)?;
        match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Ok(id),
            Err(observed) => current = observed,
        }
    }
}

fn mint_scene_id() -> Result<(SceneId, String), Error> {
    let token = mint(&NEXT_SCENE_ID)?;
    Ok((
        SceneId::from_opaque(token),
        format!("scene_{}", token.get()),
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    NotReady,
    KioskRestricted,
    SceneActive,
    SceneInactive,
    SceneClosing,
    SourceMissing,
    SourceStale,
    StaleCompletion,
    BackendPending,
    InvalidTransition,
    IdentityExhausted,
    Scene(SceneError),
    Source(SourceError),
}

impl From<SceneError> for Error {
    fn from(value: SceneError) -> Self {
        Self::Scene(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Issue {
    SourceUnavailable,
    UnsupportedSource,
    ParentalDenied,
    ResolveFailed,
    BackendRejected,
    BackendUnsupported,
}

/// A per-slot failure that the UI may show alongside successful neighboring slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SlotIssue {
    pub slot: SlotId,
    pub issue: Issue,
}

/// The backend receives typed intents. These values are commands to attempt an operation, not
/// proof that a native pipeline/window exists; completion must be reported separately.
pub(super) enum BackendIntent {
    Layout {
        scene: SceneId,
        layout: Layout,
    },
    Focus {
        scene: SceneId,
        slot: SlotId,
    },
    AudioOwner {
        scene: SceneId,
        audio_revision: u64,
        owner: Option<SlotId>,
    },
    Load {
        ticket: ResolveTicket,
        input: AuthorizedInput,
    },
    Stop {
        ticket: ResolveTicket,
    },
}

pub(super) enum Effect {
    PrepareMetadata {
        ticket: ResolveTicket,
        work: SourceWork,
    },
    CancelResolve {
        ticket: ResolveTicket,
        cancellation: Arc<std::sync::atomic::AtomicBool>,
    },
    Backend(BackendIntent),
}

pub(super) struct Transition {
    pub revision: plx_data::ott::multiview::Revisions,
    pub effects: Vec<Effect>,
    pub issues: Vec<SlotIssue>,
}

impl Transition {
    fn new(scene: &Scene) -> Self {
        Self {
            revision: scene.revisions(),
            effects: Vec::new(),
            issues: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AudioRouteState {
    /// Intended by focus/follow-focus or explicit policy.
    pub desired: Option<SlotId>,
    /// Last owner the backend explicitly acknowledged.
    pub applied: Option<SlotId>,
    pub pending_revision: Option<u64>,
}

/// Stable context stamped on one published UI offer. The renderer sends only `ui_serial` in its
/// event; the coordinator keeps the captured scene/source incarnation so an old offer cannot be
/// replayed against a newly opened scene with reset model revisions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UiOfferContext {
    pub ui_serial: u64,
    pub scene: String,
    pub snapshot_epoch: u64,
    pub scene_revision: u64,
    pub layout_revision: u64,
    pub focus_revision: u64,
    pub audio_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SlotView {
    pub slot: SlotId,
    pub slot_generation: String,
    pub source_revision: String,
    pub source: Option<SourceView>,
    pub status: &'static str,
    pub error: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SourceView {
    pub source_id: String,
    pub label: String,
    pub provider: &'static str,
    pub profile: String,
    pub kind: &'static str,
    pub identity_scheme: u8,
    pub favorite_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SceneStatus {
    pub runtime: String,
    pub scene: String,
    pub layout_revision: String,
    pub focus_revision: String,
    pub audio_revision: String,
    pub slots: Vec<SlotView>,
    pub admitted_pipeline_slots: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SourceOption {
    pub source_id: String,
    pub label: String,
    pub support: Support,
    pub provider: &'static str,
    pub profile: String,
    pub kind: &'static str,
    pub identity_scheme: u8,
    pub favorite_sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lifecycle {
    Inactive,
    Active,
    Closing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StopResult {
    Stopped,
    Failed,
}

/// One scene owner and its exact, snapshot-scoped source registry. It is deliberately not Clone.
pub(super) struct Coordinator {
    lifecycle: Lifecycle,
    scene: Option<Scene>,
    scene_id: Option<SceneId>,
    scene_token: Option<String>,
    scene_runtime: Option<String>,
    ui_focused: SlotId,
    published_ui: Option<UiOfferContext>,
    snapshot: Option<Snapshot>,
    sources: [Option<SourceHandle>; 9],
    tickets: [Option<ResolveTicket>; 9],
    cancellations: [Option<Arc<std::sync::atomic::AtomicBool>>; 9],
    stopping: [Option<ResolveTicket>; 9],
    closing_slots: [bool; 9],
    audio_applied: Option<SlotId>,
    audio_pending: Option<(SceneId, u64, Option<SlotId>)>,
}

impl Default for Coordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl Coordinator {
    pub fn new() -> Self {
        Self {
            lifecycle: Lifecycle::Inactive,
            scene: None,
            scene_id: None,
            scene_token: None,
            scene_runtime: None,
            ui_focused: SlotId::SLOT_0,
            published_ui: None,
            snapshot: None,
            sources: [None; 9],
            tickets: [None; 9],
            cancellations: std::array::from_fn(|_| None),
            stopping: [None; 9],
            closing_slots: [false; 9],
            audio_applied: None,
            audio_pending: None,
        }
    }

    /// Activate a scene only through the existing verified settings/lifecycle state. Strict
    /// kiosk is single-source today; until it has an explicit scene pin contract, scene activation
    /// fails closed while kiosk policy is enabled.
    pub fn activate(
        &mut self,
        runtime: &Runtime,
        app: &App,
        layout: Layout,
    ) -> Result<Transition, Error> {
        self.ensure_inactive()?;
        self.ensure_policy(runtime, app)?;
        let snapshot = Snapshot::capture(runtime).map_err(Error::Source)?;
        let (scene_id, scene_token) = mint_scene_id()?;
        let mut scene = Scene::new(scene_id);
        scene.set_layout(layout)?;

        self.scene = Some(scene);
        self.scene_id = Some(scene_id);
        self.scene_token = Some(scene_token);
        self.scene_runtime = Some(runtime.remote.runtime().to_owned());
        self.ui_focused = SlotId::SLOT_0;
        self.published_ui = None;
        self.snapshot = Some(snapshot);
        self.lifecycle = Lifecycle::Active;
        let scene = self.scene.as_ref().ok_or(Error::SceneInactive)?;
        let mut result = Transition::new(scene);
        result.effects.push(Effect::Backend(BackendIntent::Layout {
            scene: scene_id,
            layout,
        }));
        self.request_audio_route(&mut result)?;
        Ok(result)
    }

    pub fn source_rows(&self) -> Vec<&SourceRow> {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.rows().collect())
            .unwrap_or_default()
    }

    /// Stable opaque IDs are scoped to the captured snapshot epoch. They are registry references,
    /// not provider IDs, URLs, local paths, credentials, or portable identities.
    pub fn source_options(&self) -> Vec<SourceOption> {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return Vec::new();
        };
        snapshot
            .rows()
            .enumerate()
            .filter_map(|(index, row)| {
                let descriptor = row.descriptor()?;
                Some(SourceOption {
                    source_id: format!("src_{}_{}", snapshot.epoch(), index + 1),
                    label: row.title.clone(),
                    support: row.support,
                    provider: descriptor.provider,
                    profile: descriptor.profile.clone(),
                    kind: descriptor.kind,
                    identity_scheme: 1,
                    favorite_sha256: row.identity_digest().to_owned(),
                })
            })
            .collect()
    }

    pub fn source_id(&self, handle: SourceHandle) -> Option<String> {
        let snapshot = self.snapshot.as_ref()?;
        snapshot.rows().enumerate().find_map(|(index, row)| {
            (row.handle == handle).then(|| format!("src_{}_{}", snapshot.epoch(), index + 1))
        })
    }

    fn source_view(&self, handle: SourceHandle) -> Option<SourceView> {
        let snapshot = self.snapshot.as_ref()?;
        let (index, row) = snapshot
            .rows()
            .enumerate()
            .find(|(_, row)| row.handle == handle)?;
        let descriptor = row.descriptor()?;
        Some(SourceView {
            source_id: format!("src_{}_{}", snapshot.epoch(), index + 1),
            label: row.title.clone(),
            provider: descriptor.provider,
            profile: descriptor.profile.clone(),
            kind: descriptor.kind,
            identity_scheme: 1,
            favorite_sha256: row.identity_digest().to_owned(),
        })
    }

    pub fn layout(&self) -> Option<Layout> {
        self.scene.as_ref().map(Scene::layout)
    }

    pub fn active_slot(&self) -> Option<SlotId> {
        self.scene.as_ref().map(Scene::active_slot)
    }

    /// The active slot is the destination for commands from the active window/remote focus.
    #[cfg(all(test, feature = "hostsim"))]
    pub fn active_playback(&self) -> Option<(SlotId, ResolveTicket)> {
        if self.lifecycle != Lifecycle::Active {
            return None;
        }
        let scene = self.scene.as_ref()?;
        let slot = scene.active_slot();
        self.tickets[slot.index()].map(|ticket| (slot, ticket))
    }

    pub fn slot_status(&self, slot: SlotId) -> Result<SlotStatus, Error> {
        Ok(self
            .scene
            .as_ref()
            .ok_or(Error::SceneInactive)?
            .slot_status(slot)?)
    }

    pub fn focused_slot(&self) -> Option<SlotId> {
        self.scene.as_ref().and_then(|scene| {
            scene
                .visible_slots()
                .any(|slot| slot == self.ui_focused)
                .then_some(self.ui_focused)
        })
    }

    /// Immutable cleanup correlation only. This never authorizes source work or
    /// admission and deliberately survives retirement of its source snapshot.
    pub fn matches_scene_owner(&self, runtime: &str, scene: &str) -> bool {
        self.scene.is_some()
            && self.lifecycle != Lifecycle::Inactive
            && self.scene_runtime.as_deref() == Some(runtime)
            && self.scene_token.as_deref() == Some(scene)
    }

    pub fn scene_status(&self, runtime: &Runtime) -> Option<SceneStatus> {
        let scene = self.scene.as_ref()?;
        if !self.snapshot.as_ref()?.current_for(runtime) {
            return None;
        }
        let scene_token = self.scene_token.as_ref()?.clone();
        let revisions = scene.revisions();
        let slots = scene
            .visible_slots()
            .filter_map(|slot| {
                let (source_generation, request_generation) = scene.slot_revisions(slot).ok()?;
                let source = self.sources[slot.index()];
                let (status, error) = match scene.slot_status(slot).ok()? {
                    SlotStatus::Empty => ("empty", None),
                    SlotStatus::Resolving { .. } | SlotStatus::Resolved { .. } => {
                        ("starting", None)
                    }
                    SlotStatus::Playing { .. } => ("playing", None),
                    SlotStatus::Paused { .. } => ("paused", None),
                    SlotStatus::Failed { reason, .. } => {
                        ("failed", Some(slot_failure_token(reason)))
                    }
                    SlotStatus::Unsupported { .. } => ("failed", Some("unsupported")),
                };
                Some(SlotView {
                    slot,
                    slot_generation: slot_generation(slot, source_generation, request_generation),
                    source_revision: source_revision(slot, source_generation),
                    source: source.and_then(|handle| self.source_view(handle)),
                    status,
                    error,
                })
            })
            .collect();
        Some(SceneStatus {
            runtime: runtime.remote.runtime().to_owned(),
            scene: scene_token,
            layout_revision: format!("layout-{}", revisions.layout),
            focus_revision: format!("focus-{}", revisions.focus),
            audio_revision: format!("audio-{}", revisions.audio),
            slots,
            admitted_pipeline_slots: scene.admitted_pipeline_slots(),
        })
    }

    /// Stamp a new renderer offer. Call only when publishing/replacing the offer, not for every
    /// draw; serials are process-wide and never reused across scene teardown/recreation.
    pub fn publish_ui_offer(&mut self) -> Result<UiOfferContext, Error> {
        self.ensure_active()?;
        let serial = mint(&NEXT_UI_SERIAL)?.get();
        let scene = self.scene.as_ref().ok_or(Error::SceneInactive)?;
        let revisions = scene.revisions();
        let context = UiOfferContext {
            ui_serial: serial,
            scene: self
                .scene_token
                .as_ref()
                .ok_or(Error::SceneInactive)?
                .clone(),
            snapshot_epoch: self.snapshot.as_ref().ok_or(Error::SourceStale)?.epoch(),
            scene_revision: revisions.scene,
            layout_revision: revisions.layout,
            focus_revision: revisions.focus,
            audio_revision: revisions.audio,
        };
        self.published_ui = Some(context.clone());
        Ok(context)
    }

    /// Accept one event only from the latest published UI offer. Focus is highlight-only; an
    /// explicit activation changes the active window and follows the scene's audio policy.
    pub fn accept_ui_event(
        &mut self,
        runtime: &Runtime,
        app: &App,
        event: plx_appkit::multiview_chrome::Event,
    ) -> Result<Transition, Error> {
        self.ensure_active()?;
        self.validate_active_owner(runtime, app)?;
        let context = self.published_ui.as_ref().ok_or(Error::StaleCompletion)?;
        let scene = self.scene.as_ref().ok_or(Error::SceneInactive)?;
        let (event_revision, slot) = match event {
            plx_appkit::multiview_chrome::Event::FocusSlot { revision, slot }
            | plx_appkit::multiview_chrome::Event::ActivateSlot { revision, slot } => {
                (revision, slot)
            }
        };
        let slot = SlotId::new(slot).ok_or(Error::Scene(SceneError::InvalidSlot))?;
        if !offer_matches(
            context,
            event_revision,
            self.scene_token.as_deref(),
            self.snapshot.as_ref().map(Snapshot::epoch),
            scene.revisions().scene,
        ) {
            return Err(Error::StaleCompletion);
        }
        if !scene.visible_slots().any(|visible| visible == slot) {
            return Err(Error::Scene(SceneError::SlotNotVisible));
        }
        self.published_ui = None; // one-shot offer serial: duplicate events cannot race focus/audio
        match event {
            plx_appkit::multiview_chrome::Event::FocusSlot { .. } => {
                self.ui_focused = slot;
                Ok(Transition::new(
                    self.scene.as_ref().ok_or(Error::SceneInactive)?,
                ))
            }
            plx_appkit::multiview_chrome::Event::ActivateSlot { .. } => {
                self.ui_focused = slot;
                self.focus(slot)
            }
        }
    }

    /// Guard for every owner-side request while a scene is active. This prevents commands from
    /// using a stale favorite/profile snapshot after settings publication, and reuses the normal
    /// lifecycle/kiosk/parental gates rather than recreating them here.
    pub fn validate_active_owner(&self, runtime: &Runtime, app: &App) -> Result<(), Error> {
        self.ensure_active()?;
        self.ensure_policy(runtime, app)?;
        if !self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.current_for(runtime))
        {
            return Err(Error::SourceStale);
        }
        Ok(())
    }

    /// Verify that a queued effect still belongs to the exact live scene/slot operation.
    /// Cancellation is the exception: its own already-raised token may retire an old worker
    /// after the coordinator has cleared that ticket, but it can never authorize another worker.
    pub fn effect_current(&self, runtime: &Runtime, effect: &Effect) -> bool {
        match effect {
            Effect::PrepareMetadata { ticket, work } => {
                self.require_current_ticket(*ticket).is_ok()
                    && self
                        .snapshot
                        .as_ref()
                        .is_some_and(|snapshot| snapshot.current_for(runtime))
                    && !work
                        .cancellation()
                        .load(std::sync::atomic::Ordering::Acquire)
            }
            Effect::CancelResolve {
                ticket,
                cancellation,
            } => {
                cancellation.load(std::sync::atomic::Ordering::Acquire)
                    && self.tickets[ticket.slot().index()] != Some(*ticket)
            }
            Effect::Backend(intent) => match intent {
                BackendIntent::Layout { scene, layout } => {
                    self.lifecycle == Lifecycle::Active
                        && self.scene_id == Some(*scene)
                        && self
                            .scene
                            .as_ref()
                            .is_some_and(|current| current.layout() == *layout)
                        && self
                            .snapshot
                            .as_ref()
                            .is_some_and(|snapshot| snapshot.current_for(runtime))
                }
                BackendIntent::Focus { scene, slot } => {
                    self.lifecycle == Lifecycle::Active
                        && self.scene_id == Some(*scene)
                        && self
                            .scene
                            .as_ref()
                            .is_some_and(|current| current.active_slot() == *slot)
                        && self
                            .snapshot
                            .as_ref()
                            .is_some_and(|snapshot| snapshot.current_for(runtime))
                }
                BackendIntent::AudioOwner {
                    scene,
                    audio_revision,
                    owner,
                } => {
                    self.lifecycle == Lifecycle::Active
                        && self.scene_id == Some(*scene)
                        && self.audio_pending == Some((*scene, *audio_revision, *owner))
                        && self
                            .snapshot
                            .as_ref()
                            .is_some_and(|snapshot| snapshot.current_for(runtime))
                }
                BackendIntent::Load { ticket, input } => {
                    input.ticket == *ticket
                        && self.require_current_ticket(*ticket).is_ok()
                        && self.scene.as_ref().is_some_and(|scene| {
                            scene.slot_status(ticket.slot()).ok()
                                == Some(SlotStatus::Resolved {
                                    media: ticket.media(),
                                })
                        })
                        && input.current_for(runtime)
                }
                BackendIntent::Stop { ticket } => {
                    self.scene_id == Some(ticket.scene())
                        && self.stopping[ticket.slot().index()] == Some(*ticket)
                        && matches!(self.lifecycle, Lifecycle::Active | Lifecycle::Closing)
                        && self.scene.as_ref().is_some_and(|scene| {
                            scene
                                .slot_status(ticket.slot())
                                .ok()
                                .is_some_and(|status| status != SlotStatus::Empty)
                        })
                }
            },
        }
    }

    /// Only backend acknowledgements count as admitted pipelines. No maximum is inferred from
    /// `Layout`; an unsupported/over-capacity attempt remains a per-slot result.
    #[cfg(all(test, feature = "hostsim"))]
    pub fn admitted_pipeline_slots(&self) -> usize {
        self.scene
            .as_ref()
            .map_or(0, Scene::admitted_pipeline_slots)
    }

    pub fn audio_policy(&self) -> Option<AudioPolicy> {
        self.scene.as_ref().map(Scene::audio_policy)
    }

    pub fn audio_route_state(&self) -> AudioRouteState {
        let desired = self.scene.as_ref().and_then(Scene::audio_owner);
        AudioRouteState {
            desired,
            applied: self.audio_applied,
            pending_revision: self.audio_pending.map(|(_, revision, _)| revision),
        }
    }

    pub fn bind_source(&mut self, slot: SlotId, handle: SourceHandle) -> Result<Transition, Error> {
        self.ensure_active()?;
        if self.stopping[slot.index()].is_some() {
            return Err(Error::BackendPending);
        }
        let snapshot = self.snapshot.as_ref().ok_or(Error::SourceStale)?;
        if !snapshot.contains(&handle) {
            return Err(Error::SourceStale);
        }
        self.scene
            .as_mut()
            .ok_or(Error::SceneInactive)?
            .bind_source(slot, handle.source_ref())?;
        self.sources[slot.index()] = Some(handle);
        self.tickets[slot.index()] = None;
        self.cancellations[slot.index()] = None;
        Ok(Transition::new(
            self.scene.as_ref().ok_or(Error::SceneInactive)?,
        ))
    }

    /// Begin an independently cancellable resolver operation for one slot. Source selection and
    /// parental admission are delegated to the existing snapshot broker and Runtime gates.
    pub fn begin_play(
        &mut self,
        runtime: &mut Runtime,
        app: &App,
        slot: SlotId,
    ) -> Result<Transition, Error> {
        self.ensure_active()?;
        self.ensure_policy(runtime, app)?;
        if self.stopping[slot.index()].is_some() {
            return Err(Error::BackendPending);
        }
        let handle = self.sources[slot.index()].ok_or(Error::SourceMissing)?;
        let snapshot = self.snapshot.as_ref().ok_or(Error::SourceStale)?;
        if !snapshot.contains(&handle) {
            return Err(Error::SourceStale);
        }
        let ticket = self
            .scene
            .as_mut()
            .ok_or(Error::SceneInactive)?
            .begin_resolution(slot, handle.source_ref(), handle.media_ref())?;
        self.tickets[slot.index()] = Some(ticket);
        let result = match self
            .snapshot
            .as_ref()
            .ok_or(Error::SourceStale)?
            .capture_work(runtime, app, &handle, ticket)
        {
            Ok(work) => {
                self.cancellations[slot.index()] = Some(work.cancellation());
                let mut transition =
                    Transition::new(self.scene.as_ref().ok_or(Error::SceneInactive)?);
                transition
                    .effects
                    .push(Effect::PrepareMetadata { ticket, work });
                Ok(transition)
            }
            Err(error) => {
                let issue = issue_for_source_error(error);
                self.scene
                    .as_mut()
                    .ok_or(Error::SceneInactive)?
                    .complete_resolution(ticket, ResolveOutcome::Failed(slot_failure(issue)))?;
                self.tickets[slot.index()] = None;
                let mut transition =
                    Transition::new(self.scene.as_ref().ok_or(Error::SceneInactive)?);
                transition.issues.push(SlotIssue { slot, issue });
                Ok(transition)
            }
        };
        result
    }

    /// Resolver errors are isolated to the addressed slot. Sibling requests remain live.
    pub fn resolve_failed(
        &mut self,
        ticket: ResolveTicket,
        error: SourceError,
    ) -> Result<Transition, Error> {
        self.require_current_ticket(ticket)?;
        let issue = issue_for_source_error(error);
        self.scene
            .as_mut()
            .ok_or(Error::SceneInactive)?
            .complete_resolution(ticket, ResolveOutcome::Failed(slot_failure(issue)))?;
        self.cancellations[ticket.slot().index()] = None;
        self.tickets[ticket.slot().index()] = None;
        let mut transition = Transition::new(self.scene.as_ref().ok_or(Error::SceneInactive)?);
        transition.issues.push(SlotIssue {
            slot: ticket.slot(),
            issue,
        });
        Ok(transition)
    }

    /// The broker repeats source and parental admission before returning metadata. The
    /// coordinator fences it against the exact scene, slot, source and item ticket.
    pub fn metadata_succeeded(
        &mut self,
        input: PreparedMetadata,
        runtime: &mut Runtime,
        app: &App,
    ) -> Result<Transition, Error> {
        self.ensure_active()?;
        self.ensure_policy(runtime, app)?;
        let ticket = input.ticket();
        self.require_current_ticket(ticket)?;
        let authorized = match input.authorize_completion(runtime, app) {
            Ok(authorized) => authorized,
            Err(error) => return self.resolve_failed(ticket, error),
        };
        let ticket = authorized.ticket;
        self.require_current_ticket(ticket)?;
        self.scene
            .as_mut()
            .ok_or(Error::SceneInactive)?
            .complete_resolution(ticket, ResolveOutcome::Resolved)?;
        self.cancellations[ticket.slot().index()] = None;
        let mut transition = Transition::new(self.scene.as_ref().ok_or(Error::SceneInactive)?);
        transition
            .effects
            .push(Effect::Backend(BackendIntent::Load {
                ticket,
                input: authorized,
            }));
        Ok(transition)
    }

    /// Report only the actual backend result. The scene's admitted count never predicts capacity.
    pub fn backend_start_completed(
        &mut self,
        ticket: ResolveTicket,
        outcome: AdmissionOutcome,
    ) -> Result<Transition, Error> {
        self.require_current_ticket(ticket)?;
        let slot = ticket.slot();
        self.scene
            .as_mut()
            .ok_or(Error::SceneInactive)?
            .complete_admission(ticket, outcome)?;
        let mut transition = Transition::new(self.scene.as_ref().ok_or(Error::SceneInactive)?);
        if self.closing_slots[slot.index()] {
            match outcome {
                AdmissionOutcome::Admitted => {
                    self.stopping[slot.index()] = Some(ticket);
                    transition
                        .effects
                        .push(Effect::Backend(BackendIntent::Stop { ticket }));
                }
                AdmissionOutcome::Unsupported | AdmissionOutcome::Rejected => {
                    self.scene
                        .as_mut()
                        .ok_or(Error::SceneInactive)?
                        .report_pipeline_stopped(ticket)?;
                    self.clear_slot_attempt(slot);
                    self.finalize_if_closed();
                }
            }
        } else if outcome == AdmissionOutcome::Admitted {
            self.request_audio_route(&mut transition)?;
        } else {
            transition.issues.push(SlotIssue {
                slot,
                issue: if outcome == AdmissionOutcome::Unsupported {
                    Issue::BackendUnsupported
                } else {
                    Issue::BackendRejected
                },
            });
        }
        transition.revision = self
            .scene
            .as_ref()
            .map_or(transition.revision, Scene::revisions);
        Ok(transition)
    }

    pub fn set_layout(&mut self, layout: Layout) -> Result<Transition, Error> {
        self.ensure_active()?;
        let scene_id = self.scene_id.ok_or(Error::SceneInactive)?;
        let scene = self.scene.as_mut().ok_or(Error::SceneInactive)?;
        let prior_focus = scene.active_slot();
        let prior_audio = scene.audio_owner();
        scene.set_layout(layout)?;
        let mut transition = Transition::new(scene);
        transition
            .effects
            .push(Effect::Backend(BackendIntent::Layout {
                scene: scene_id,
                layout,
            }));
        if scene.active_slot() != prior_focus {
            transition
                .effects
                .push(Effect::Backend(BackendIntent::Focus {
                    scene: scene_id,
                    slot: scene.active_slot(),
                }));
        }
        if scene.audio_owner() != prior_audio {
            self.request_audio_route(&mut transition)?;
        }
        Ok(transition)
    }

    pub fn focus(&mut self, slot: SlotId) -> Result<Transition, Error> {
        self.ensure_active()?;
        let scene_id = self.scene_id.ok_or(Error::SceneInactive)?;
        let scene = self.scene.as_mut().ok_or(Error::SceneInactive)?;
        let prior_audio = scene.audio_owner();
        scene.focus(slot)?;
        let mut transition = Transition::new(scene);
        transition
            .effects
            .push(Effect::Backend(BackendIntent::Focus {
                scene: scene_id,
                slot,
            }));
        if scene.audio_owner() != prior_audio {
            self.request_audio_route(&mut transition)?;
        }
        Ok(transition)
    }

    pub fn set_audio_policy(&mut self, policy: AudioPolicy) -> Result<Transition, Error> {
        self.ensure_active()?;
        self.scene
            .as_mut()
            .ok_or(Error::SceneInactive)?
            .set_audio_policy(policy)?;
        let mut transition = Transition::new(self.scene.as_ref().ok_or(Error::SceneInactive)?);
        self.request_audio_route(&mut transition)?;
        Ok(transition)
    }

    pub fn audio_route_completed(
        &mut self,
        scene: SceneId,
        scene_revision: u64,
        owner: Option<SlotId>,
        accepted: bool,
    ) -> Result<(), Error> {
        let expected = self.audio_pending.ok_or(Error::StaleCompletion)?;
        if expected != (scene, scene_revision, owner) || self.scene_id != Some(scene) {
            return Err(Error::StaleCompletion);
        }
        if accepted {
            self.audio_applied = owner;
        }
        self.audio_pending = None;
        Ok(())
    }

    pub fn close_slot(&mut self, slot: SlotId) -> Result<Transition, Error> {
        self.ensure_active()?;
        if self.stopping[slot.index()].is_some() {
            return Err(Error::BackendPending);
        }
        let status = self
            .scene
            .as_ref()
            .ok_or(Error::SceneInactive)?
            .slot_status(slot)?;
        let ticket = self.tickets[slot.index()];
        let mut transition = Transition::new(self.scene.as_ref().ok_or(Error::SceneInactive)?);
        match (status, ticket) {
            (SlotStatus::Empty, _) => return Ok(transition),
            (SlotStatus::Resolving { .. }, Some(ticket)) => {
                if let Some(cancellation) = self.cancellations[slot.index()].take() {
                    cancellation.store(true, Ordering::Release);
                    transition.effects.push(Effect::CancelResolve {
                        ticket,
                        cancellation,
                    });
                }
                self.scene
                    .as_mut()
                    .ok_or(Error::SceneInactive)?
                    .report_pipeline_stopped(ticket)?;
                self.clear_slot_attempt(slot);
            }
            (
                SlotStatus::Resolved { .. }
                | SlotStatus::Playing { .. }
                | SlotStatus::Paused { .. },
                Some(ticket),
            ) => {
                self.closing_slots[slot.index()] = true;
                self.stopping[slot.index()] = Some(ticket);
                transition
                    .effects
                    .push(Effect::Backend(BackendIntent::Stop { ticket }));
            }
            (SlotStatus::Failed { .. } | SlotStatus::Unsupported { .. }, Some(ticket)) => {
                self.scene
                    .as_mut()
                    .ok_or(Error::SceneInactive)?
                    .report_pipeline_stopped(ticket)?;
                self.clear_slot_attempt(slot);
            }
            (_, None) => return Err(Error::InvalidTransition),
        }
        transition.revision = self
            .scene
            .as_ref()
            .map_or(transition.revision, Scene::revisions);
        Ok(transition)
    }

    /// Close the whole scene for a user exit or policy/lifecycle teardown. The source snapshot is
    /// retired immediately; the scene remains until every admitted/possibly-started slot is stopped.
    pub fn close_scene(&mut self) -> Result<Transition, Error> {
        self.ensure_active()?;
        if let Some(snapshot) = self.snapshot.as_ref() {
            snapshot.retire();
        }
        self.lifecycle = Lifecycle::Closing;
        let mut transition = Transition::new(self.scene.as_ref().ok_or(Error::SceneInactive)?);
        for index in 0..9 {
            let slot = SlotId::new(index as u8).ok_or(Error::InvalidTransition)?;
            let status = self.scene.as_ref().unwrap().slot_status(slot)?;
            let Some(ticket) = self.tickets[index] else {
                continue;
            };
            match status {
                SlotStatus::Resolving { .. } => {
                    if let Some(cancellation) = self.cancellations[index].take() {
                        cancellation.store(true, Ordering::Release);
                        transition.effects.push(Effect::CancelResolve {
                            ticket,
                            cancellation,
                        });
                    }
                    self.scene
                        .as_mut()
                        .unwrap()
                        .report_pipeline_stopped(ticket)?;
                    self.clear_slot_attempt(slot);
                }
                SlotStatus::Resolved { .. }
                | SlotStatus::Playing { .. }
                | SlotStatus::Paused { .. } => {
                    if self.stopping[index].is_none() {
                        self.stopping[index] = Some(ticket);
                        self.closing_slots[index] = true;
                        transition
                            .effects
                            .push(Effect::Backend(BackendIntent::Stop { ticket }));
                    }
                }
                SlotStatus::Failed { .. } | SlotStatus::Unsupported { .. } => {
                    self.scene
                        .as_mut()
                        .unwrap()
                        .report_pipeline_stopped(ticket)?;
                    self.clear_slot_attempt(slot);
                }
                SlotStatus::Empty => self.clear_slot_attempt(slot),
            }
        }
        self.finalize_if_closed();
        transition.revision = self
            .scene
            .as_ref()
            .map_or(transition.revision, Scene::revisions);
        Ok(transition)
    }

    pub fn stop_completed(
        &mut self,
        ticket: ResolveTicket,
        result: StopResult,
    ) -> Result<Transition, Error> {
        let index = ticket.slot().index();
        if self.stopping[index] != Some(ticket) {
            return Err(Error::StaleCompletion);
        }
        let mut transition = Transition::new(self.scene.as_ref().ok_or(Error::SceneInactive)?);
        if result == StopResult::Failed {
            let mut transition = Transition::new(self.scene.as_ref().ok_or(Error::SceneInactive)?);
            transition.issues.push(SlotIssue {
                slot: ticket.slot(),
                issue: Issue::BackendRejected,
            });
            return Ok(transition);
        }
        if self.scene.as_ref().is_some_and(|scene| {
            scene
                .slot_status(ticket.slot())
                .ok()
                .is_some_and(|status| status != SlotStatus::Empty)
        }) {
            self.scene
                .as_mut()
                .ok_or(Error::SceneInactive)?
                .report_pipeline_stopped(ticket)?;
        }
        self.stopping[index] = None;
        self.closing_slots[index] = false;
        self.clear_slot_attempt(ticket.slot());
        transition.revision = self
            .scene
            .as_ref()
            .map_or(transition.revision, Scene::revisions);
        self.finalize_if_closed();
        Ok(transition)
    }

    fn request_audio_route(&mut self, transition: &mut Transition) -> Result<(), Error> {
        let scene_id = self.scene_id.ok_or(Error::SceneInactive)?;
        let scene = self.scene.as_ref().ok_or(Error::SceneInactive)?;
        let revision = scene.revisions().audio;
        let owner = scene.audio_owner();
        self.audio_pending = Some((scene_id, revision, owner));
        transition
            .effects
            .push(Effect::Backend(BackendIntent::AudioOwner {
                scene: scene_id,
                audio_revision: revision,
                owner,
            }));
        Ok(())
    }

    fn require_current_ticket(&self, ticket: ResolveTicket) -> Result<(), Error> {
        let index = ticket.slot().index();
        if self.lifecycle != Lifecycle::Active
            || self.tickets[index] != Some(ticket)
            || self.scene_id != Some(ticket.scene())
            || self.sources[index].is_none_or(|handle| {
                handle.source_ref() != ticket.source()
                    || handle.media_ref() != ticket.media()
                    || self
                        .snapshot
                        .as_ref()
                        .is_none_or(|snapshot| !snapshot.contains(&handle))
            })
        {
            return Err(Error::StaleCompletion);
        }
        Ok(())
    }

    fn ensure_policy(&self, runtime: &Runtime, app: &App) -> Result<(), Error> {
        if !app.running
            || !runtime.loaded
            || runtime.persistence_uncertain
            || !runtime.play_work_allowed()
            || !super::playback_allowed(app)
        {
            return Err(Error::NotReady);
        }
        if !matches!(
            runtime.track_selection_kiosk(),
            Ok(crate::app::ott_kiosk_policy::State::Off)
        ) {
            return Err(Error::KioskRestricted);
        }
        Ok(())
    }

    fn ensure_inactive(&self) -> Result<(), Error> {
        if self.lifecycle != Lifecycle::Inactive || self.scene.is_some() {
            Err(Error::SceneActive)
        } else {
            Ok(())
        }
    }

    fn ensure_active(&self) -> Result<(), Error> {
        match self.lifecycle {
            Lifecycle::Active => Ok(()),
            Lifecycle::Inactive => Err(Error::SceneInactive),
            Lifecycle::Closing => Err(Error::SceneClosing),
        }
    }

    fn clear_slot_attempt(&mut self, slot: SlotId) {
        let index = slot.index();
        self.tickets[index] = None;
        self.cancellations[index] = None;
        self.closing_slots[index] = false;
    }

    fn finalize_if_closed(&mut self) {
        if self.lifecycle == Lifecycle::Closing
            && self.stopping.iter().all(Option::is_none)
            && self.scene.as_ref().is_some_and(|scene| {
                scene
                    .visible_slots()
                    .all(|slot| scene.slot_status(slot).ok() == Some(SlotStatus::Empty))
            })
        {
            if let Some(snapshot) = self.snapshot.take() {
                snapshot.retire();
            }
            self.scene = None;
            self.scene_id = None;
            self.scene_token = None;
            self.scene_runtime = None;
            self.sources = [None; 9];
            self.tickets = [None; 9];
            self.cancellations = std::array::from_fn(|_| None);
            self.stopping = [None; 9];
            self.closing_slots = [false; 9];
            self.audio_pending = None;
            self.audio_applied = None;
            self.lifecycle = Lifecycle::Inactive;
        }
    }
}

fn offer_matches(
    context: &UiOfferContext,
    event_serial: u64,
    current_scene: Option<&str>,
    current_snapshot_epoch: Option<u64>,
    current_scene_revision: u64,
) -> bool {
    event_serial == context.ui_serial
        && current_scene == Some(context.scene.as_str())
        && current_snapshot_epoch == Some(context.snapshot_epoch)
        && current_scene_revision == context.scene_revision
}

fn slot_generation(slot: SlotId, source_generation: u64, request_generation: u64) -> String {
    format!(
        "slot-{}-generation-{}-{}",
        slot.index(),
        source_generation,
        request_generation
    )
}

fn source_revision(slot: SlotId, source_generation: u64) -> String {
    format!("slot-{}-source-{}", slot.index(), source_generation)
}

fn issue_for_source_error(error: SourceError) -> Issue {
    match error {
        SourceError::Unsupported => Issue::UnsupportedSource,
        SourceError::Policy => Issue::ParentalDenied,
        SourceError::Resolve => Issue::ResolveFailed,
        SourceError::Cancelled
        | SourceError::Stale
        | SourceError::UnknownSource
        | SourceError::Exhausted => Issue::SourceUnavailable,
    }
}

fn slot_failure(issue: Issue) -> SlotFailure {
    match issue {
        Issue::SourceUnavailable => SlotFailure::SourceUnavailable,
        Issue::UnsupportedSource => SlotFailure::ProviderUnsupported,
        Issue::ParentalDenied => SlotFailure::ParentalDenied,
        Issue::ResolveFailed => SlotFailure::ResolverFailed,
        Issue::BackendRejected => SlotFailure::BackendRejected,
        Issue::BackendUnsupported => SlotFailure::BackendUnsupported,
    }
}

fn slot_failure_token(failure: SlotFailure) -> &'static str {
    match failure {
        SlotFailure::SourceUnavailable => "source_unavailable",
        SlotFailure::ItemUnavailable
        | SlotFailure::ResolverFailed
        | SlotFailure::BackendRejected => "unavailable",
        SlotFailure::ParentalDenied => "policy_restricted",
        SlotFailure::BackendUnsupported | SlotFailure::ProviderUnsupported => "unsupported",
    }
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "tests.rs"]
mod tests;
