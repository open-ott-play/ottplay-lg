//! Pure state model for a multi-window playback scene.
//!
//! This module owns no provider profiles, credentials, IO, media backend, or hardware capability
//! claims. Callers resolve an opaque source reference through their existing source authority,
//! then report resolver and backend outcomes here. A visible layout's slot count is presentation
//! geometry only; it says nothing about how many native pipelines the backend can admit.

use std::num::NonZeroU64;

/// Stable scene slots. Layouts expose a prefix of this fixed set.
pub const SLOT_COUNT: usize = 9;

/// A stable slot index in `0..=8`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SlotId(u8);

impl SlotId {
    pub const SLOT_0: Self = Self(0);
    pub const SLOT_1: Self = Self(1);
    pub const SLOT_2: Self = Self(2);
    pub const SLOT_3: Self = Self(3);
    pub const SLOT_4: Self = Self(4);
    pub const SLOT_5: Self = Self(5);
    pub const SLOT_6: Self = Self(6);
    pub const SLOT_7: Self = Self(7);
    pub const SLOT_8: Self = Self(8);

    pub const fn new(index: u8) -> Option<Self> {
        if index < SLOT_COUNT as u8 {
            Some(Self(index))
        } else {
            None
        }
    }

    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

impl std::fmt::Debug for SlotId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SlotId({})", self.0)
    }
}

/// Opaque handle into a caller-owned, currently authorized source registry.
///
/// It is intentionally not serializable and its token is never printed. It is not a credential
/// or a portable identity; callers must re-resolve it from their source registry after restart.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceRef(NonZeroU64);

impl SourceRef {
    pub const fn from_opaque(token: NonZeroU64) -> Self {
        Self(token)
    }
}

impl std::fmt::Debug for SourceRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SourceRef(<opaque>)")
    }
}

/// Opaque item identity resolved under one `SourceRef`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MediaRef(NonZeroU64);

impl MediaRef {
    pub const fn from_opaque(token: NonZeroU64) -> Self {
        Self(token)
    }
}

impl std::fmt::Debug for MediaRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MediaRef(<opaque>)")
    }
}

/// Opaque incarnation identifier for one scene lifetime.
///
/// The scene owner must mint a value not reused while stale async results may still arrive. A new
/// `Scene` requires this value; scene state cannot be cloned into a second owner.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SceneId(NonZeroU64);

impl SceneId {
    pub const fn from_opaque(token: NonZeroU64) -> Self {
        Self(token)
    }
}

impl std::fmt::Debug for SceneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SceneId(<opaque>)")
    }
}

/// Fixed display geometry. `visible_capacity` is never a backend decoder limit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    #[default]
    Single,
    PictureInPicture,
    Grid4,
    Grid9,
    /// Center slot plus `satellites` perimeter slots. Valid values are `0..=8`.
    Perimeter {
        satellites: u8,
    },
}

impl Layout {
    pub const fn perimeter(satellites: u8) -> Option<Self> {
        if satellites <= 8 {
            Some(Self::Perimeter { satellites })
        } else {
            None
        }
    }

    pub const fn is_valid(self) -> bool {
        !matches!(self, Self::Perimeter { satellites } if satellites > 8)
    }

    pub const fn visible_capacity(self) -> usize {
        match self {
            Self::Single => 1,
            Self::PictureInPicture => 2,
            Self::Grid4 => 4,
            Self::Grid9 => 9,
            Self::Perimeter { satellites } if satellites <= 8 => satellites as usize + 1,
            Self::Perimeter { .. } => 0,
        }
    }

    pub fn visible_slots(self) -> impl Iterator<Item = SlotId> {
        (0..self.visible_capacity()).map(|index| SlotId(index as u8))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Revisions {
    /// Changes to layout or any visible slot state.
    pub scene: u64,
    /// Changes only when display geometry changes.
    pub layout: u64,
    /// Changes to a slot's source binding. This is independent of focus and scene revisions.
    pub source: u64,
    /// Changes to the active slot.
    pub focus: u64,
    /// Changes to the audio policy or selected audio owner.
    pub audio: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioPolicy {
    /// Exactly the focused slot owns audio. An empty or failed focused slot means silence.
    FollowsFocus,
    /// Audio belongs to the selected slot, or to no slot when `None`.
    Explicit(Option<SlotId>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotFailure {
    SourceUnavailable,
    ItemUnavailable,
    ParentalDenied,
    ResolverFailed,
    BackendRejected,
    BackendUnsupported,
    ProviderUnsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotStatus {
    Empty,
    Resolving {
        media: MediaRef,
    },
    Resolved {
        media: MediaRef,
    },
    Playing {
        media: MediaRef,
    },
    Paused {
        media: MediaRef,
    },
    Failed {
        media: Option<MediaRef>,
        reason: SlotFailure,
    },
    Unsupported {
        media: MediaRef,
    },
}

impl SlotStatus {
    const fn media(self) -> Option<MediaRef> {
        match self {
            Self::Empty => None,
            Self::Resolving { media }
            | Self::Resolved { media }
            | Self::Playing { media }
            | Self::Paused { media }
            | Self::Unsupported { media } => Some(media),
            Self::Failed { media, .. } => media,
        }
    }

    const fn is_pipeline_admitted(self) -> bool {
        matches!(self, Self::Playing { .. } | Self::Paused { .. })
    }

    const fn can_replace_request(self) -> bool {
        !self.is_pipeline_admitted()
    }

    const fn is_idle(self) -> bool {
        matches!(self, Self::Empty)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slot {
    source: Option<SourceRef>,
    source_generation: u64,
    request_generation: u64,
    status: SlotStatus,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            source: None,
            source_generation: 0,
            request_generation: 0,
            status: SlotStatus::Empty,
        }
    }
}

/// A resolver operation fenced to one exact slot, source binding, and attempt.
///
/// A slot switching A → B → A receives a new source generation on each switch, so an old A
/// completion cannot attach to the later A binding. Focus and layout changes do not invalidate an
/// otherwise-current source request.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ResolveTicket {
    scene: SceneId,
    slot: SlotId,
    source: SourceRef,
    media: MediaRef,
    source_generation: u64,
    request_generation: u64,
}

impl ResolveTicket {
    pub const fn slot(self) -> SlotId {
        self.slot
    }

    pub const fn scene(self) -> SceneId {
        self.scene
    }

    pub const fn source(self) -> SourceRef {
        self.source
    }

    pub const fn media(self) -> MediaRef {
        self.media
    }
}

impl std::fmt::Debug for ResolveTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolveTicket")
            .field("slot", &self.slot)
            .field("source", &self.source)
            .field("source_generation", &self.source_generation)
            .field("request_generation", &self.request_generation)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveOutcome {
    Resolved,
    Failed(SlotFailure),
}

/// The backend's answer for one already-resolved slot. This is an observation of an actual
/// backend result, not a capacity estimate or promise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionOutcome {
    Admitted,
    Unsupported,
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidSlot,
    SlotNotVisible,
    SlotBusy,
    NoSource,
    SourceMismatch,
    KioskRestricted,
    KioskConflict,
    WouldHideActiveSlot,
    WouldHideAudioOwner,
    WouldHideKioskSlot,
    InvalidLayout,
    StaleCompletion,
    InvalidTransition,
    RevisionExhausted,
}

/// A single-item strict kiosk pin. The app layer must verify policy and mint these opaque refs;
/// this type only enforces that the scene cannot silently substitute a different slot/source/item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrictKioskPin {
    slot: SlotId,
    source: SourceRef,
    media: MediaRef,
}

impl StrictKioskPin {
    pub const fn new(slot: SlotId, source: SourceRef, media: MediaRef) -> Self {
        Self {
            slot,
            source,
            media,
        }
    }
}

/// Pure multiview scene state. No member is a media backend or source authority.
#[derive(Debug, PartialEq, Eq)]
pub struct Scene {
    id: SceneId,
    slots: [Slot; SLOT_COUNT],
    layout: Layout,
    active_slot: SlotId,
    audio: AudioPolicy,
    kiosk: Option<StrictKioskPin>,
    revisions: Revisions,
}

impl Scene {
    pub fn new(id: SceneId) -> Self {
        Self {
            id,
            slots: [Slot::default(); SLOT_COUNT],
            layout: Layout::Single,
            active_slot: SlotId::SLOT_0,
            audio: AudioPolicy::FollowsFocus,
            kiosk: None,
            revisions: Revisions::default(),
        }
    }

    pub const fn layout(&self) -> Layout {
        self.layout
    }

    pub const fn revisions(&self) -> Revisions {
        self.revisions
    }

    pub const fn active_slot(&self) -> SlotId {
        self.active_slot
    }

    pub const fn audio_policy(&self) -> AudioPolicy {
        self.audio
    }

    pub const fn audio_owner(&self) -> Option<SlotId> {
        self.audio_owner_for(self.active_slot)
    }

    const fn audio_owner_for(&self, focused: SlotId) -> Option<SlotId> {
        match self.audio {
            AudioPolicy::FollowsFocus => Some(focused),
            AudioPolicy::Explicit(owner) => owner,
        }
    }

    pub const fn strict_kiosk(&self) -> Option<StrictKioskPin> {
        self.kiosk
    }

    pub fn visible_slots(&self) -> impl Iterator<Item = SlotId> {
        self.layout.visible_slots()
    }

    pub fn slot_status(&self, slot: SlotId) -> Result<SlotStatus, Error> {
        self.slots
            .get(slot.index())
            .map(|slot| slot.status)
            .ok_or(Error::InvalidSlot)
    }

    pub fn slot_source(&self, slot: SlotId) -> Result<Option<SourceRef>, Error> {
        self.slots
            .get(slot.index())
            .map(|slot| slot.source)
            .ok_or(Error::InvalidSlot)
    }

    /// Per-slot incarnation revisions for status reporting. These are not media identifiers.
    pub fn slot_revisions(&self, slot: SlotId) -> Result<(u64, u64), Error> {
        self.slots
            .get(slot.index())
            .map(|slot| (slot.source_generation, slot.request_generation))
            .ok_or(Error::InvalidSlot)
    }

    /// Counts slots whose backend has explicitly acknowledged a running or paused pipeline.
    /// This is an observed scene count, not the backend's maximum concurrency capability.
    pub fn admitted_pipeline_slots(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.status.is_pipeline_admitted())
            .count()
    }

    pub fn set_layout(&mut self, layout: Layout) -> Result<(), Error> {
        if !layout.is_valid() {
            return Err(Error::InvalidLayout);
        }
        if self.layout == layout {
            return Ok(());
        }
        let capacity = layout.visible_capacity();
        if self.kiosk.is_some_and(|pin| pin.slot.index() >= capacity) {
            return Err(Error::WouldHideKioskSlot);
        }
        if self.slots[capacity..]
            .iter()
            .any(|slot| !slot.status.is_idle())
        {
            return Err(Error::WouldHideActiveSlot);
        }
        let active_slot = if self.active_slot.index() >= capacity {
            SlotId::SLOT_0
        } else {
            self.active_slot
        };
        if let AudioPolicy::Explicit(Some(owner)) = self.audio {
            if owner.index() >= capacity {
                return Err(Error::WouldHideAudioOwner);
            }
        }
        let scene_revision = self.next_scene_revision()?;
        let layout_revision = self.next_layout_revision()?;
        let focus_changed = active_slot != self.active_slot;
        let focus_revision = focus_changed
            .then(|| self.next_focus_revision())
            .transpose()?;
        let audio_changed = self.audio_owner_for(active_slot) != self.audio_owner();
        let audio_revision = audio_changed
            .then(|| self.next_audio_revision())
            .transpose()?;
        self.layout = layout;
        self.active_slot = active_slot;
        self.revisions.scene = scene_revision;
        self.revisions.layout = layout_revision;
        if let Some(revision) = focus_revision {
            self.revisions.focus = revision;
        }
        if let Some(revision) = audio_revision {
            self.revisions.audio = revision;
        }
        Ok(())
    }

    pub fn focus(&mut self, slot: SlotId) -> Result<(), Error> {
        self.ensure_visible(slot)?;
        if self.active_slot == slot {
            return Ok(());
        }
        let scene_revision = self.next_scene_revision()?;
        let focus_revision = self.next_focus_revision()?;
        let audio_changed = self.audio_owner_for(slot) != self.audio_owner();
        let audio_revision = audio_changed
            .then(|| self.next_audio_revision())
            .transpose()?;
        self.active_slot = slot;
        self.revisions.scene = scene_revision;
        self.revisions.focus = focus_revision;
        if let Some(revision) = audio_revision {
            self.revisions.audio = revision;
        }
        Ok(())
    }

    pub fn set_audio_policy(&mut self, policy: AudioPolicy) -> Result<(), Error> {
        if self.audio == policy {
            return Ok(());
        }
        if let AudioPolicy::Explicit(Some(owner)) = policy {
            self.ensure_visible(owner)?;
        }
        let scene_revision = self.next_scene_revision()?;
        let audio_revision = self.next_audio_revision()?;
        self.audio = policy;
        self.revisions.scene = scene_revision;
        self.revisions.audio = audio_revision;
        Ok(())
    }

    /// Bind one opaque source to a slot. A prior media operation must be retired first. Rebinding
    /// always advances the slot generation, including A → B → A, and invalidates stale results.
    pub fn bind_source(&mut self, slot_id: SlotId, source: SourceRef) -> Result<(), Error> {
        self.ensure_visible(slot_id)?;
        if let Some(pin) = self.kiosk {
            if pin.slot != slot_id || pin.source != source {
                return Err(Error::KioskRestricted);
            }
        }
        let slot = self.slots.get(slot_id.index()).ok_or(Error::InvalidSlot)?;
        if !slot.status.is_idle() {
            return Err(Error::SlotBusy);
        }
        let source_generation = slot
            .source_generation
            .checked_add(1)
            .ok_or(Error::RevisionExhausted)?;
        let source_revision = self.next_source_revision()?;
        let scene_revision = self.next_scene_revision()?;
        let slot = &mut self.slots[slot_id.index()];
        slot.source = Some(source);
        slot.source_generation = source_generation;
        slot.request_generation = 0;
        self.revisions.source = source_revision;
        self.revisions.scene = scene_revision;
        Ok(())
    }

    /// Clear an idle source binding. Active or pending slots must first be retired explicitly.
    pub fn clear_source(&mut self, slot_id: SlotId) -> Result<(), Error> {
        self.ensure_visible(slot_id)?;
        if self.kiosk.is_some_and(|pin| pin.slot == slot_id) {
            return Err(Error::KioskRestricted);
        }
        let slot = self.slots.get(slot_id.index()).ok_or(Error::InvalidSlot)?;
        if !slot.status.is_idle() {
            return Err(Error::SlotBusy);
        }
        if slot.source.is_none() {
            return Ok(());
        }
        let source_generation = slot
            .source_generation
            .checked_add(1)
            .ok_or(Error::RevisionExhausted)?;
        let source_revision = self.next_source_revision()?;
        let scene_revision = self.next_scene_revision()?;
        let slot = &mut self.slots[slot_id.index()];
        slot.source = None;
        slot.source_generation = source_generation;
        slot.request_generation = 0;
        self.revisions.source = source_revision;
        self.revisions.scene = scene_revision;
        Ok(())
    }

    /// Begin resolving an opaque item under the slot's current source. A new attempt supersedes
    /// only the older non-admitted attempt in this slot; it never cancels a sibling slot.
    pub fn begin_resolution(
        &mut self,
        slot_id: SlotId,
        source: SourceRef,
        media: MediaRef,
    ) -> Result<ResolveTicket, Error> {
        self.ensure_visible(slot_id)?;
        if self
            .kiosk
            .is_some_and(|pin| pin.slot != slot_id || pin.source != source || pin.media != media)
        {
            return Err(Error::KioskRestricted);
        }
        let slot = self.slots.get(slot_id.index()).ok_or(Error::InvalidSlot)?;
        if slot.source != Some(source) {
            return Err(if slot.source.is_none() {
                Error::NoSource
            } else {
                Error::SourceMismatch
            });
        }
        if !slot.status.can_replace_request() {
            return Err(Error::SlotBusy);
        }
        let request_generation = slot
            .request_generation
            .checked_add(1)
            .ok_or(Error::RevisionExhausted)?;
        let scene_revision = self.next_scene_revision()?;
        let ticket = ResolveTicket {
            scene: self.id,
            slot: slot_id,
            source,
            media,
            source_generation: slot.source_generation,
            request_generation,
        };
        let slot = &mut self.slots[slot_id.index()];
        slot.request_generation = request_generation;
        slot.status = SlotStatus::Resolving { media };
        self.revisions.scene = scene_revision;
        Ok(ticket)
    }

    pub fn complete_resolution(
        &mut self,
        ticket: ResolveTicket,
        outcome: ResolveOutcome,
    ) -> Result<(), Error> {
        let slot = self.current_slot_for(ticket)?;
        if slot.status
            != (SlotStatus::Resolving {
                media: slot.status.media().ok_or(Error::StaleCompletion)?,
            })
        {
            return Err(Error::StaleCompletion);
        }
        let media = slot.status.media().ok_or(Error::StaleCompletion)?;
        let next_status = match outcome {
            ResolveOutcome::Resolved => SlotStatus::Resolved { media },
            ResolveOutcome::Failed(reason) => SlotStatus::Failed {
                media: Some(media),
                reason,
            },
        };
        self.set_current_status(ticket, next_status)
    }

    /// Record an actual backend result for a successfully resolved request.
    pub fn complete_admission(
        &mut self,
        ticket: ResolveTicket,
        outcome: AdmissionOutcome,
    ) -> Result<(), Error> {
        let slot = self.current_slot_for(ticket)?;
        let SlotStatus::Resolved { media } = slot.status else {
            return Err(Error::InvalidTransition);
        };
        let next_status = match outcome {
            AdmissionOutcome::Admitted => SlotStatus::Playing { media },
            AdmissionOutcome::Unsupported => SlotStatus::Unsupported { media },
            AdmissionOutcome::Rejected => SlotStatus::Failed {
                media: Some(media),
                reason: SlotFailure::BackendRejected,
            },
        };
        self.set_current_status(ticket, next_status)
    }

    pub fn pause(&mut self, ticket: ResolveTicket) -> Result<(), Error> {
        let slot = self.current_slot_for(ticket)?;
        let SlotStatus::Playing { media } = slot.status else {
            return Err(Error::InvalidTransition);
        };
        self.set_current_status(ticket, SlotStatus::Paused { media })
    }

    pub fn resume(&mut self, ticket: ResolveTicket) -> Result<(), Error> {
        let slot = self.current_slot_for(ticket)?;
        let SlotStatus::Paused { media } = slot.status else {
            return Err(Error::InvalidTransition);
        };
        self.set_current_status(ticket, SlotStatus::Playing { media })
    }

    /// User close, which strict kiosk deliberately refuses for its pinned slot.
    pub fn close_media(&mut self, slot_id: SlotId) -> Result<(), Error> {
        self.ensure_visible(slot_id)?;
        if self.kiosk.is_some_and(|pin| pin.slot == slot_id) {
            return Err(Error::KioskRestricted);
        }
        self.retire_slot(slot_id)
    }

    /// Backend/lifecycle acknowledgement that this exact request has released its pipeline.
    /// Unlike user close, this is allowed in kiosk mode so failures and standby can retire safely.
    pub fn report_pipeline_stopped(&mut self, ticket: ResolveTicket) -> Result<(), Error> {
        let _ = self.current_slot_for(ticket)?;
        self.retire_slot(ticket.slot)
    }

    pub fn enable_strict_kiosk(&mut self, pin: StrictKioskPin) -> Result<(), Error> {
        self.ensure_visible(pin.slot)?;
        for (index, slot) in self.slots.iter().enumerate() {
            if index != pin.slot.index() && !slot.status.is_idle() {
                return Err(Error::KioskConflict);
            }
        }
        let slot = &self.slots[pin.slot.index()];
        if slot.source.is_some_and(|source| source != pin.source)
            || slot.status.media().is_some_and(|media| media != pin.media)
        {
            return Err(Error::KioskConflict);
        }
        if self.kiosk == Some(pin) {
            return Ok(());
        }
        let scene_revision = self.next_scene_revision()?;
        self.kiosk = Some(pin);
        self.revisions.scene = scene_revision;
        Ok(())
    }

    /// The caller must perform the existing settings authorization before clearing this policy.
    pub fn disable_strict_kiosk(&mut self) -> Result<(), Error> {
        if self.kiosk.is_none() {
            return Ok(());
        }
        let scene_revision = self.next_scene_revision()?;
        self.kiosk = None;
        self.revisions.scene = scene_revision;
        Ok(())
    }

    fn ensure_visible(&self, slot: SlotId) -> Result<(), Error> {
        if slot.index() >= SLOT_COUNT {
            return Err(Error::InvalidSlot);
        }
        if slot.index() >= self.layout.visible_capacity() {
            return Err(Error::SlotNotVisible);
        }
        Ok(())
    }

    fn current_slot_for(&self, ticket: ResolveTicket) -> Result<&Slot, Error> {
        let slot = self
            .slots
            .get(ticket.slot.index())
            .ok_or(Error::StaleCompletion)?;
        if slot.source != Some(ticket.source)
            || self.id != ticket.scene
            || slot.status.media() != Some(ticket.media)
            || slot.source_generation != ticket.source_generation
            || slot.request_generation != ticket.request_generation
        {
            return Err(Error::StaleCompletion);
        }
        Ok(slot)
    }

    fn set_current_status(
        &mut self,
        ticket: ResolveTicket,
        status: SlotStatus,
    ) -> Result<(), Error> {
        let _ = self.current_slot_for(ticket)?;
        let scene_revision = self.next_scene_revision()?;
        self.slots[ticket.slot.index()].status = status;
        self.revisions.scene = scene_revision;
        Ok(())
    }

    fn retire_slot(&mut self, slot_id: SlotId) -> Result<(), Error> {
        let slot = self.slots.get(slot_id.index()).ok_or(Error::InvalidSlot)?;
        if slot.status.is_idle() {
            return Ok(());
        }
        let request_generation = slot
            .request_generation
            .checked_add(1)
            .ok_or(Error::RevisionExhausted)?;
        let scene_revision = self.next_scene_revision()?;
        let slot = &mut self.slots[slot_id.index()];
        slot.request_generation = request_generation;
        slot.status = SlotStatus::Empty;
        self.revisions.scene = scene_revision;
        Ok(())
    }

    fn next_scene_revision(&self) -> Result<u64, Error> {
        self.revisions
            .scene
            .checked_add(1)
            .ok_or(Error::RevisionExhausted)
    }

    fn next_source_revision(&self) -> Result<u64, Error> {
        self.revisions
            .source
            .checked_add(1)
            .ok_or(Error::RevisionExhausted)
    }

    fn next_focus_revision(&self) -> Result<u64, Error> {
        self.revisions
            .focus
            .checked_add(1)
            .ok_or(Error::RevisionExhausted)
    }

    fn next_audio_revision(&self) -> Result<u64, Error> {
        self.revisions
            .audio
            .checked_add(1)
            .ok_or(Error::RevisionExhausted)
    }

    fn next_layout_revision(&self) -> Result<u64, Error> {
        self.revisions
            .layout
            .checked_add(1)
            .ok_or(Error::RevisionExhausted)
    }
}

#[cfg(test)]
#[path = "multiview_tests.rs"]
mod tests;
