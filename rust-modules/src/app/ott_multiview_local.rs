//! Independent on-device slots. No fullscreen route ticket, compositor client,
//! global player transport or active-provider mutation is spent here.
use super::multiview::{BackendIntent, SceneEffect, StopResult, Transition};
use super::multiview_sources::{LocalResolved, PreparedMetadata};
use super::*;
use plx_data::ott::multiview::{AdmissionOutcome, Layout, ResolveTicket, SceneId, SlotId};
use plx_media::local_multiview::{Event, FrameYuv, Limits, PcmChunk, Session};
use std::sync::mpsc::{self, Receiver, Sender};

fn picture_limits(runtime: &Runtime, slot: SlotId) -> Limits {
    if runtime.multiview.layout() == Some(Layout::Grid4) {
        Limits::grid4(runtime.multiview.active_slot() == Some(slot))
    } else {
        Limits::default()
    }
}

#[path = "ott_multiview_local_commands.rs"]
mod commands;
#[path = "ott_multiview_local_handoff.rs"]
mod handoff;
#[path = "ott_multiview_local_recovery.rs"]
mod recovery;
#[path = "ott_multiview_local_transport.rs"]
mod transport;
pub(super) use transport::player_request;

enum Job {
    Metadata(
        ResolveTicket,
        Result<PreparedMetadata, multiview_sources::Error>,
    ),
    Media(
        ResolveTicket,
        Result<LocalResolved, multiview_sources::Error>,
    ),
}
struct Work {
    ticket: ResolveTicket,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    terminal_consumed: bool,
}
struct Picture {
    ticket: ResolveTicket,
    receipt: LocalResolved,
    decoder: Session,
    admitted: bool,
    frame: Option<FrameYuv>,
    terminal: bool,
    paused: bool,
    position_us: i64,
}
struct LeaseWork {
    lease: Arc<std::sync::Mutex<Option<crate::app::ott_vportal_lease::Lease>>>,
    done: Arc<AtomicBool>,
    success: Arc<AtomicBool>,
    started: bool,
}
pub(super) struct Driver {
    pictures: [Option<Picture>; 9],
    handoff: handoff::Handoff,
    recovery: [recovery::Budget; 9],
    last_stop: [Option<&'static str>; 9],
    workers: [Option<Work>; 9],
    stopping: [Option<ResolveTicket>; 9],
    leases: [std::collections::VecDeque<LeaseWork>; 9],
    tx: Sender<Job>,
    rx: Receiver<Job>,
    layout: Option<Layout>,
    closing: bool,
    audio_request: Option<(SceneId, u64, Option<SlotId>)>,
    audio_applied: Option<SlotId>,
    audio: std::collections::VecDeque<(SlotId, ResolveTicket, PcmChunk)>,
    mixer: plx_media::local_pcm_mix::Mixer,
    background_audio_percent: u8,
    mixing: bool,
    audio_epoch: u64,
    serial: u64,
    sources: Vec<commands::SourceReceipt>,
    replacements: [Option<multiview_sources::SourceHandle>; 9],
    restore_previous: bool,
    restore_ready: bool,
    geometry_revision: u64,
    cleanup_failed: bool,
}
impl Default for Driver {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            pictures: std::array::from_fn(|_| None),
            handoff: Default::default(),
            recovery: Default::default(),
            last_stop: [None; 9],
            workers: std::array::from_fn(|_| None),
            stopping: [None; 9],
            leases: std::array::from_fn(|_| Default::default()),
            tx,
            rx,
            layout: None,
            closing: false,
            audio_request: None,
            audio_applied: None,
            audio: Default::default(),
            mixer: Default::default(),
            background_audio_percent: 20,
            mixing: false,
            audio_epoch: 0,
            serial: 0,
            sources: Vec::new(),
            replacements: [None; 9],
            restore_previous: false,
            restore_ready: false,
            geometry_revision: 0,
            cleanup_failed: false,
        }
    }
}
impl Driver {
    #[cfg(all(test, feature = "hostsim"))]
    pub(super) fn active_layout_for_test(layout: Layout) -> Self {
        let mut driver = Self::default();
        driver.layout = Some(layout);
        driver
    }

    pub fn owns_ui(&self, runtime: &Runtime, app: &App) -> bool {
        self.active()
            && !self.closing
            && !self.cleanup_failed
            && commands::eligible(runtime, app)
            && runtime
                .multiview
                .validate_active_owner(runtime, app)
                .is_ok()
    }
    pub fn set_layout(
        &mut self,
        runtime: &mut Runtime,
        app: &App,
        layout: Layout,
    ) -> Result<(), multiview::Error> {
        if !commands::eligible(runtime, app) || !plx_media::local_multiview::available() {
            return Err(multiview::Error::NotReady);
        }
        if self.closing || (self.cleanup_failed && self.active()) {
            return Err(multiview::Error::BackendPending);
        }
        if !self.active()
            && (self.has_work() || super::multiview_local_present::has_owned_work(runtime))
        {
            return Err(multiview::Error::BackendPending);
        }
        let mut coordinator = std::mem::take(&mut runtime.multiview);
        let result = if self.active() {
            coordinator.set_layout(layout)
        } else {
            coordinator.activate_local(runtime, app, layout)
        };
        runtime.multiview = coordinator;
        runtime.multiview_effects.extend(result?.effects);
        if self.layout.is_none() {
            self.last_stop = [None; 9];
        }
        self.layout = Some(layout);
        self.cleanup_failed = false;
        self.restore_ready = false;
        Ok(())
    }
    pub fn close_restore_previous(
        &mut self,
        runtime: &mut Runtime,
        app: &App,
    ) -> Result<(), multiview::Error> {
        if !commands::eligible(runtime, app) {
            return Err(multiview::Error::NotReady);
        }
        self.revoke(runtime);
        self.restore_previous = !self.cleanup_failed;
        Ok(())
    }
    pub fn refresh_sources(
        &mut self,
        runtime: &mut Runtime,
        app: &App,
    ) -> Result<(), multiview::Error> {
        if !self.owns_ui(runtime, app) {
            return Err(multiview::Error::NotReady);
        }
        let mut coordinator = std::mem::take(&mut runtime.multiview);
        let result = coordinator.refresh_local_sources(runtime);
        runtime.multiview = coordinator;
        result
    }
    pub fn active_timeline(&self, runtime: &Runtime) -> Option<(i64, Option<i64>, bool)> {
        let slot = runtime.multiview.active_slot()?;
        let p = self.pictures[slot.index()]
            .as_ref()
            .filter(|p| p.admitted && !p.terminal && !p.receipt.input.is_cancelled())?;
        Some((
            p.position_us,
            p.receipt
                .input
                .entry
                .duration_ms
                .and_then(|ms| i64::try_from(ms).ok())
                .and_then(|ms| ms.checked_mul(1000)),
            p.paused,
        ))
    }
    pub fn active_seekable(&self, runtime: &mut Runtime, app: &App) -> bool {
        self.owns_ui(runtime, app)
            && runtime.multiview.active_slot().is_some_and(|slot| {
                self.stopping[slot.index()].is_none()
                    && self.pictures[slot.index()].as_ref().is_some_and(|picture| {
                        picture.admitted && !picture.terminal
                            && runtime.multiview.active_playback() == Some((slot, picture.ticket))
                            && runtime.multiview.slot_status(slot).is_ok_and(|status| matches!(status,
                                plx_data::ott::multiview::SlotStatus::Playing { .. }
                                    | plx_data::ott::multiview::SlotStatus::Paused { .. }))
                            && picture.receipt.input.reauthorize(runtime, app).is_ok()
                            && picture.decoder.is_seekable()
                    })
            })
    }
    pub fn step_active(
        &mut self,
        runtime: &mut Runtime,
        app: &App,
        offset: i64,
    ) -> Result<(), multiview::Error> {
        if offset == 0 {
            return Err(multiview::Error::InvalidTransition);
        }
        let slot = runtime
            .multiview
            .active_slot()
            .ok_or(multiview::Error::SceneInactive)?;
        let handle = runtime
            .multiview
            .local_step_source(slot, self.replacements[slot.index()], offset)
            .ok_or(multiview::Error::SourceMissing)?;
        self.play_source(runtime, app, slot, handle)
    }
    pub fn active(&self) -> bool {
        self.layout.is_some()
    }
    pub fn has_work(&self) -> bool {
        self.handoff.has_waiters()
            || self.recovery.iter().any(recovery::Budget::pending)
            || self.workers.iter().any(Option::is_some)
            || self.pictures.iter().any(Option::is_some)
            || self.stopping.iter().any(Option::is_some)
            || self.replacements.iter().any(Option::is_some)
            || self.leases.iter().any(|leases| !leases.is_empty())
    }
    pub fn take_restore_previous(&mut self) -> bool {
        std::mem::take(&mut self.restore_ready)
    }
    pub fn geometry(
        &self,
        width: u32,
        height: u32,
    ) -> Option<plx_appkit::multiview_layout::RasterPlan> {
        use plx_appkit::multiview_layout::{Layout as Geometry, Mode};
        let mode = match self.layout? {
            Layout::Single => Mode::Perimeter { satellites: 0 },
            Layout::PictureInPicture => Mode::Pip,
            Layout::Grid4 => Mode::Grid4,
            Layout::Grid9 => Mode::Grid9,
            Layout::Perimeter { satellites } => Mode::Perimeter { satellites },
        };
        Geometry::new(
            plx_ui::Rect::new(0.0, 0.0, width as f32, height as f32),
            mode,
            1.0,
        )
        .ok()?
        .rasterize_pixels(self.geometry_revision)
        .ok()
    }
    pub fn play_source(
        &mut self,
        runtime: &mut Runtime,
        app: &App,
        slot: SlotId,
        handle: multiview_sources::SourceHandle,
    ) -> Result<(), multiview::Error> {
        if self.closing || self.cleanup_failed {
            return Err(multiview::Error::SceneClosing);
        }
        runtime.multiview.validate_active_owner(runtime, app)?;
        let index = slot.index();
        self.recovery[index] = Default::default();
        if self.stopping[index].is_some() || self.replacements[index].is_some() {
            self.replacements[index] = Some(handle);
            return Ok(());
        }
        if self.workers[index].is_some()
            || self.pictures[index].is_some()
            || !self.leases[index].is_empty()
            // The coordinator owns a queued attempt before this frame has
            // created its worker. Retire it through the same replacement path.
            || matches!(runtime.multiview.slot_status(slot),
                Ok(plx_data::ott::multiview::SlotStatus::Resolving { .. }
                    | plx_data::ott::multiview::SlotStatus::Resolved { .. }
                    | plx_data::ott::multiview::SlotStatus::Playing { .. }
                    | plx_data::ott::multiview::SlotStatus::Paused { .. }
                    | plx_data::ott::multiview::SlotStatus::Failed { .. }
                    | plx_data::ott::multiview::SlotStatus::Unsupported { .. }))
        {
            let transition = runtime.multiview.close_slot(slot)?;
            self.replacements[index] = Some(handle);
            runtime.multiview_effects.extend(transition.effects);
            return Ok(());
        }
        self.start_source(runtime, app, slot, handle)
    }

    fn start_source(
        &mut self,
        runtime: &mut Runtime,
        app: &App,
        slot: SlotId,
        handle: multiview_sources::SourceHandle,
    ) -> Result<(), multiview::Error> {
        let index = slot.index();
        let t = runtime.multiview.bind_source(slot, handle)?;
        self.last_stop[index] = None;
        runtime.multiview_effects.extend(t.effects);
        let mut coordinator = std::mem::take(&mut runtime.multiview);
        let result = coordinator.begin_play_local(runtime, app, slot);
        runtime.multiview = coordinator;
        runtime.multiview_effects.extend(result?.effects);
        Ok(())
    }
    pub fn set_active_paused(
        &mut self,
        runtime: &mut Runtime,
        app: &App,
        paused: bool,
    ) -> Result<(), &'static str> {
        runtime
            .multiview
            .validate_active_owner(runtime, app)
            .map_err(|_| "Multiview authority changed")?;
        let slot = runtime.multiview.active_slot().ok_or("No active slot")?;
        let p = self.pictures[slot.index()]
            .as_mut()
            .filter(|p| p.admitted && !p.terminal)
            .ok_or("No active picture")?;
        p.receipt
            .input
            .reauthorize(runtime, app)
            .map_err(|_| "Source changed")?;
        let transition = runtime.multiview.set_paused(p.ticket, paused)
            .map_err(|_| "Window playback changed before pause")?;
        p.decoder.set_paused(paused);
        p.paused = paused;
        self.clear_slot_audio(slot);
        runtime.multiview_effects.extend(transition.effects);
        runtime.changed();
        Ok(())
    }
    pub fn seek_active(
        &mut self,
        runtime: &mut Runtime,
        app: &App,
        position_us: i64,
    ) -> Result<(), &'static str> {
        runtime
            .multiview
            .validate_active_owner(runtime, app)
            .map_err(|_| "Multiview authority changed")?;
        let slot = runtime.multiview.active_slot().ok_or("No active slot")?;
        let p = self.pictures[slot.index()]
            .as_mut()
            .filter(|p| p.admitted)
            .ok_or("No active picture")?;
        p.receipt
            .input
            .reauthorize(runtime, app)
            .map_err(|_| "Source changed")?;
        p.decoder
            .seek(position_us)
            .map_err(|_| "This slot cannot seek")?;
        p.position_us = position_us;
        p.frame = None;
        self.clear_slot_audio(slot);
        Ok(())
    }
    pub fn take_frame(&mut self, slot: SlotId) -> Option<FrameYuv> {
        self.pictures[slot.index()]
            .as_mut()
            .filter(|p| p.admitted && !p.receipt.input.is_cancelled())?
            .frame
            .take()
    }
    pub fn slot_ticket(&self, slot: SlotId) -> Option<ResolveTicket> {
        self.pictures[slot.index()]
            .as_ref()
            .filter(|p| p.admitted && !p.terminal && self.stopping[slot.index()].is_none() && !p.receipt.input.is_cancelled())
            .map(|p| p.ticket)
    }
    pub fn hls_lookahead_counts(&self, ticket: ResolveTicket) -> Option<(u32, u32)> {
        self.pictures[ticket.slot().index()].as_ref().filter(|picture| picture.ticket == ticket)
            .map(|picture| picture.decoder.hls_lookahead_counts())
    }
    pub fn last_stop(&self, slot: SlotId) -> Option<&'static str> {
        self.last_stop[slot.index()]
    }
    pub fn texture_failed(&mut self, slot: SlotId) {
        self.last_stop[slot.index()] = Some("texture_upload");
    }
    pub fn set_background_audio_percent(&mut self, percent: u8) {
        let percent = percent.min(100);
        if self.background_audio_percent != percent {
            self.background_audio_percent = percent;
            self.reset_audio();
        }
    }

    pub fn mixing_audio(&self) -> bool {
        self.mixing
    }

    pub fn audio_epoch(&self) -> u64 {
        self.audio_epoch
    }

    fn reset_audio(&mut self) {
        self.audio.clear();
        self.mixer.clear();
        self.audio_epoch = self.audio_epoch.wrapping_add(1);
    }

    fn clear_slot_audio(&mut self, slot: SlotId) {
        self.mixer.clear_slot(slot.index());
        self.audio.retain(|(s, _, _)| *s != slot);
        if self.mixing || self.audio_applied == Some(slot) {
            self.audio_epoch = self.audio_epoch.wrapping_add(1);
        }
    }

    /// A silent/empty focused slot has no receipt. Its admitted neighbours
    /// can still contribute at background level through a current output ticket.
    pub fn output_ticket(&self, owner: Option<SlotId>) -> Option<(SlotId, ResolveTicket)> {
        let slot = owner?;
        if let Some(ticket) = self.slot_ticket(slot) {
            return Some((slot, ticket));
        }
        if self.mixing {
            for picture in self.pictures.iter().flatten() {
                let slot = picture.ticket.slot();
                if let Some(ticket) = self.slot_ticket(slot) {
                    return Some((slot, ticket));
                }
            }
        }
        None
    }

    pub fn audio_owner(&self) -> Option<SlotId> {
        self.audio_applied
    }
    pub fn audio_output_failed(&mut self, runtime: &mut Runtime) {
        self.audio_applied = None;
        self.audio_request = None;
        self.reset_audio();
        let transition = runtime
            .multiview
            .set_audio_policy(plx_data::ott::multiview::AudioPolicy::Explicit(None));
        self.queue(runtime, transition);
    }
    pub fn audio_request(&self) -> Option<(SceneId, u64, Option<SlotId>)> {
        self.audio_request
    }
    /// The real output owner acknowledges only after it switched/flushed its sink.
    pub fn confirm_audio(&mut self, runtime: &mut Runtime, accepted: bool) {
        let Some((scene, revision, owner)) = self.audio_request.take() else {
            return;
        };
        if runtime
            .multiview
            .audio_route_completed(scene, revision, owner, accepted)
            .is_ok()
        {
            self.audio_applied = if accepted { owner } else { None };
            if !accepted || owner.is_none() {
                self.reset_audio();
            }
        }
    }
    pub fn take_audio(&mut self, queued_frames: usize) -> Option<(SlotId, ResolveTicket, PcmChunk)> {
        if !self.mixing {
            return self.audio.pop_front();
        }
        let focus = self.audio_applied?;
        let (slot, ticket) = self.output_ticket(Some(focus))?;
        let chunk = self.mixer.pull(focus.index(), self.background_audio_percent, queued_frames)?;
        Some((slot, ticket, chunk))
    }
    fn queue(&mut self, runtime: &mut Runtime, transition: Result<Transition, multiview::Error>) {
        if let Ok(transition) = transition {
            for issue in transition.issues {
                self.last_stop[issue.slot.index()].get_or_insert(issue.issue.token());
            }
            runtime.multiview_effects.extend(transition.effects);
        }
    }
    fn retain_lease(&mut self, index: usize, lease: crate::app::ott_vportal_lease::Lease) {
        lease.begin_release();
        self.leases[index].push_back(LeaseWork {
            lease: Arc::new(std::sync::Mutex::new(Some(lease))),
            done: Arc::new(AtomicBool::new(false)),
            success: Arc::new(AtomicBool::new(false)),
            started: false,
        });
    }
    fn retire_receipt(&mut self, index: usize, mut receipt: LocalResolved) {
        receipt.input.cancellation().store(true, Ordering::Release);
        if let Some(lease) = receipt.lease.take() {
            self.retain_lease(index, lease);
        }
    }
    fn reject_job(&mut self, job: Job) {
        if let Job::Media(ticket, Ok(receipt)) = job {
            self.retire_receipt(ticket.slot().index(), receipt);
        }
    }
    fn receive_job(&mut self, runtime: &mut Runtime, app: &App, job: Job) {
        let ticket = match &job {
            Job::Metadata(t, _) | Job::Media(t, _) => *t,
        };
        let index = ticket.slot().index();
        let admitted = self.workers[index]
            .as_mut()
            .filter(|w| w.ticket == ticket)
            .is_some_and(|w| {
                w.terminal_consumed = true;
                !w.cancel.load(Ordering::Acquire)
            });
        if !admitted {
            self.reject_job(job);
            return;
        }
        match job {
            Job::Metadata(_, result) => {
                let mut coordinator = std::mem::take(&mut runtime.multiview);
                let transition = match result {
                    Ok(metadata) => coordinator.metadata_succeeded(metadata, runtime, app),
                    Err(error) => coordinator.metadata_failed(ticket, error),
                };
                runtime.multiview = coordinator;
                self.queue(runtime, transition);
            }
            Job::Media(_, result) => {
                let limits = picture_limits(runtime, ticket.slot());
                self.media_ready_with(runtime, app, ticket, result, |source| {
                    Session::open(source, limits)
                })
            }
        }
    }
    fn retire_worker(&mut self, index: usize) {
        // Worker retirement alone is insufficient: a successful receipt may
        // still be queued after this frame's RX drain and own a provider lease.
        if self.workers[index]
            .as_ref()
            .is_some_and(|w| w.terminal_consumed && w.done.load(Ordering::Acquire))
        {
            self.workers[index] = None;
        }
    }
    fn slot_drained(&self, index: usize) -> bool {
        self.pictures[index].is_none()
            && self.workers[index].is_none()
            && !self.handoff.waiting(index)
            && self.leases[index].is_empty()
    }
    fn finish_lease_if_ready(&mut self, runtime: &mut Runtime, index: usize) {
        let Some(cleanup) = self.leases[index].front() else {
            return;
        };
        if !cleanup.done.load(Ordering::Acquire) {
            return;
        }
        if !cleanup.success.load(Ordering::Acquire) {
            self.cleanup_failed = true;
            self.restore_previous = false;
            self.replacements[index] = None;
            runtime.status = "Previous window playback session did not finish closing".into();
            runtime.changed();
        }
        self.leases[index].pop_front();
    }
    fn media_ready_with(
        &mut self,
        runtime: &mut Runtime,
        app: &App,
        ticket: ResolveTicket,
        result: Result<LocalResolved, multiview_sources::Error>,
        open: impl FnOnce(
            plx_media::ff::ExternalSource,
        ) -> Result<Session, plx_media::local_multiview::Error>,
    ) {
        let index = ticket.slot().index();
        match result {
            Ok(receipt) => {
                if self.closing
                    || self.cleanup_failed
                    || receipt.input.reauthorize(runtime, app).is_err()
                {
                    self.retire_receipt(index, receipt);
                    let t = runtime
                        .multiview
                        .backend_start_completed(ticket, AdmissionOutcome::Rejected);
                    self.queue(runtime, t);
                    return;
                }
                if let Some(origin) = self.wait_for_handoff(&receipt) {
                    self.handoff.hold(ticket, receipt, origin, runtime.frame_time());
                    return;
                }
                match open(receipt.source.clone()) {
                    Ok(mut decoder) => {
                        decoder.set_audio(false);
                        self.pictures[index] = Some(Picture {
                            ticket,
                            receipt,
                            decoder,
                            admitted: false,
                            frame: None,
                            terminal: false,
                            paused: false,
                            position_us: 0,
                        });
                    }
                    Err(_) => {
                        self.retire_receipt(index, receipt);
                        let t = runtime
                            .multiview
                            .backend_start_completed(ticket, AdmissionOutcome::Unsupported);
                        self.queue(runtime, t);
                    }
                }
            }
            Err(_) => {
                let t = runtime
                    .multiview
                    .backend_start_completed(ticket, AdmissionOutcome::Rejected);
                self.queue(runtime, t);
            }
        }
    }
    pub fn revoke(&mut self, runtime: &mut Runtime) {
        self.restore_previous = false;
        self.replacements = [None; 9];
        self.recovery = Default::default();
        if !self.closing {
            self.closing = true;
            let transition = runtime.multiview.close_scene();
            self.queue(runtime, transition);
        }
        self.audio_applied = None;
        self.audio_request = None;
        self.reset_audio();
        self.sources.clear();
        for work in self.workers.iter().flatten() {
            work.cancel.store(true, Ordering::Release);
        }
        for picture in self.pictures.iter_mut().flatten() {
            picture
                .receipt
                .input
                .cancellation()
                .store(true, Ordering::Release);
            picture.decoder.cancel();
            picture.frame = None;
        }
    }
    pub fn tick(&mut self, runtime: &mut Runtime, app: &App) {
        let mixing = self.background_audio_percent > 0
            && runtime.multiview.audio_policy() == Some(plx_data::ott::multiview::AudioPolicy::FollowsFocus);
        if self.mixing != mixing {
            self.mixing = mixing;
            self.reset_audio();
        }
        for index in 0..9 {
            self.finish_lease_if_ready(runtime, index);
        }
        if self.closing && !commands::eligible(runtime, app) {
            self.restore_previous = false;
        }
        if self.active()
            && !self.closing
            && runtime
                .multiview
                .validate_active_owner(runtime, app)
                .is_err()
        {
            self.restore_previous = false;
            self.revoke(runtime);
        }
        // Keep start effects ordered behind actual ordinary-player retirement;
        // cancellation and Stop remain drainable while takeover is waiting.
        let slots_ready = super::multiview_local_present::slots_ready(runtime, app);
        let mut deferred = Vec::new();
        let count = runtime.multiview_effects.len().min(64);
        for _ in 0..count {
            let Some(effect) = runtime.multiview_effects.pop_front() else {
                break;
            };
            let start = match &effect {
                SceneEffect::PrepareMetadata { ticket, .. }
                | SceneEffect::Backend(BackendIntent::Load { ticket, .. }) => {
                    Some(ticket.slot().index())
                }
                _ => None,
            };
            if (!slots_ready
                || start.is_some_and(|i| self.workers[i].is_some() || !self.leases[i].is_empty()))
                && !self.closing
                && matches!(
                    &effect,
                    SceneEffect::PrepareMetadata { .. } | SceneEffect::Backend(BackendIntent::Load { .. })
                )
            {
                if runtime.multiview.effect_current(runtime, &effect) {
                    deferred.push(effect);
                }
                continue;
            }
            self.apply(runtime, app, effect);
        }
        runtime.multiview_effects.extend(deferred);
        for _ in 0..18 {
            let Ok(job) = self.rx.try_recv() else {
                break;
            };
            self.receive_job(runtime, app, job);
        }
        let output_tile = if self.layout == Some(Layout::Grid4) {
            let (_, _, width, height) = plx_base::surface::viewport();
            ((width.max(0) as u32).div_ceil(2), (height.max(0) as u32).div_ceil(2))
        } else {
            (0, 0)
        };
        for index in 0..9 {
            let Some(picture) = self.pictures[index].as_mut() else {
                continue;
            };
            picture.decoder.set_output_tile(output_tile.0, output_tile.1);
            if picture.receipt.input.reauthorize(runtime, app).is_err()
                && self.stopping[index].is_none()
            {
                self.last_stop[index] = Some("authorization_changed");
                let transition = runtime.multiview.close_slot(picture.ticket.slot());
                if let Ok(transition) = transition {
                    runtime.multiview_effects.extend(transition.effects);
                }
                self.mixer.clear_slot(index);
                self.audio_epoch = self.audio_epoch.wrapping_add(1);
                picture.decoder.cancel();
                picture.frame = None;
                picture
                    .receipt
                    .input
                    .cancellation()
                    .store(true, Ordering::Release);
            }
            let routing_mix = self.mixing && (self.audio_applied.is_some()
                || self.audio_request.is_some_and(|(_, _, owner)| owner.is_some()));
            let audible = (routing_mix || self.audio_applied == Some(picture.ticket.slot()))
                && self.stopping[index].is_none() && !picture.terminal && !picture.paused
                && !picture.receipt.input.is_cancelled();
            picture.decoder.set_max_fps(picture_limits(runtime, picture.ticket.slot()).max_fps);
            picture.decoder.set_audio(audible);
            for _ in 0..8 {
                let Some(event) = picture.decoder.poll() else {
                    break;
                };
                match event {
                    Event::Video(frame)
                        if self.stopping[index].is_none()
                            && !picture.receipt.input.is_cancelled() =>
                    {
                        if !picture.admitted {
                            let transition = runtime.multiview.backend_start_completed(
                                picture.ticket,
                                AdmissionOutcome::Admitted,
                            );
                            picture.admitted = transition.is_ok();
                            if picture.admitted { self.recovery[index].resumed(); }
                            if let Ok(t) = transition {
                                runtime.multiview_effects.extend(t.effects);
                            }
                        }
                        if picture.admitted {
                            picture.position_us = frame.pts_us.max(0);
                            picture.frame = Some(frame);
                            plx_machine::idle::invalidate();
                        }
                    }
                    Event::Audio(chunk)
                        if audible && picture.admitted
                            && self.stopping[index].is_none()
                            && !picture.receipt.input.is_cancelled() =>
                    {
                        if self.mixing {
                            self.mixer.push(index, chunk);
                        } else {
                            if self.audio.len() >= 8 {
                                self.audio.pop_front();
                            }
                            self.audio.push_back((picture.ticket.slot(), picture.ticket, chunk));
                        }
                    }
                    Event::Failure(_) | Event::Ended => {
                        let retry_requested = !self.closing
                            && !self.cleanup_failed
                            && self.stopping[index].is_none()
                            && self.replacements[index].is_none()
                            && !picture.paused
                            && !picture.receipt.input.is_cancelled()
                            && self.recovery[index].on_failure(&event, picture, runtime);
                        if self.stopping[index].is_none() {
                            use plx_media::local_multiview::Error;
                            self.last_stop[index] = Some(match event {
                                Event::Failure(Error::Unavailable) => "unavailable",
                                Event::Failure(Error::InvalidLimits) => "invalid_limits",
                                Event::Failure(Error::Unsupported) => "unsupported",
                                Event::Failure(Error::Source) => "source_read",
                                Event::Failure(Error::SourceDenied) => "provider_session_denied",
                                Event::Failure(Error::SourceInterrupted) => "provider_interrupted",
                                Event::Failure(Error::LostProviderLease) => "provider_lease_expired",
                                Event::Failure(Error::SourceTemporarilyUnavailable) => "source_temporarily_unavailable",
                                Event::Failure(Error::Playlist) => "playlist",
                                Event::Failure(Error::Decode) => "decode",
                                Event::Failure(Error::Budget) => "resource_limit",
                                Event::Failure(Error::Cancelled) => "cancelled",
                                Event::Failure(Error::Thread) => "worker_unavailable",
                                _ => "ended",
                            });
                        }
                        picture.terminal = true;
                        picture.frame = None;
                        picture.decoder.cancel();
                        self.mixer.clear_slot(index);
                        if self.mixing {
                            self.audio_epoch = self.audio_epoch.wrapping_add(1);
                        } else if self.audio_applied == Some(picture.ticket.slot()) {
                            self.audio_applied = None;
                            self.audio.clear();
                        }
                        let transition = if picture.admitted && !retry_requested && matches!(event, Event::Failure(_)) {
                            runtime.multiview.playback_failed(picture.ticket)
                        } else if picture.admitted || retry_requested {
                            runtime.multiview.close_slot(picture.ticket.slot())
                        } else {
                            runtime
                                .multiview
                                .backend_start_completed(picture.ticket, AdmissionOutcome::Rejected)
                        };
                        if let Ok(t) = transition {
                            runtime.multiview_effects.extend(t.effects);
                        }
                    }
                    _ => {}
                }
            }
        }
        for index in 0..9 {
            self.retire_worker(index);
            let retired = self.pictures[index].as_ref().is_none_or(|p| {
                p.decoder.is_retired()
                    && (p.terminal
                        || self.stopping[index].is_some()
                        || p.receipt.input.is_cancelled())
            });
            if retired && self.workers[index].is_none() {
                if let Some(mut picture) = self.pictures[index].take() {
                    self.retired_handoff(&picture);
                    if let Some(lease) = picture.receipt.lease.take() {
                        self.retain_lease(index, lease);
                    }
                }
            }
            if let Some(cleanup) = self.leases[index].front_mut() {
                if !cleanup.started {
                    let lease = cleanup.lease.clone();
                    let done = cleanup.done.clone();
                    let success = cleanup.success.clone();
                    cleanup.started =
                        plx_base::task::spawn_off_frame("local-mv-lease-close", move |_| {
                            let result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    lease
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner())
                                        .take()
                                        .is_none_or(|lease| lease.finish_off_frame().is_ok())
                                }))
                                .unwrap_or(false);
                            success.store(result, Ordering::Release);
                            done.store(true, Ordering::Release);
                        });
                }
            }
            self.finish_lease_if_ready(runtime, index);
            if self.slot_drained(index) {
                if let Some(ticket) = self.stopping[index].take() {
                    let transition = runtime
                        .multiview
                        .stop_completed(ticket, StopResult::Stopped);
                    self.queue(runtime, transition);
                }
                if !self.closing {
                    if let Some(handle) = self.replacements[index].take() {
                        if let Some(slot) = SlotId::new(index as u8) {
                            let _ = self.play_source(runtime, app, slot, handle);
                        }
                    } else if self.recovery[index].pending() {
                        self.recover_drained_slot(runtime, app, index, runtime.frame_time());
                    }
                }
            }
        }
        self.poll_handoffs(runtime, app);
        if self.closing && !self.has_work() {
            self.handoff = Default::default();
            self.layout = None;
            self.closing = false;
            self.restore_ready = std::mem::take(&mut self.restore_previous) && !self.cleanup_failed;
        }
    }
    fn apply(&mut self, runtime: &mut Runtime, app: &App, effect: SceneEffect) {
        if self.closing || self.cleanup_failed {
            match &effect {
                SceneEffect::PrepareMetadata { work, .. } => {
                    work.cancellation().store(true, Ordering::Release);
                    return;
                }
                SceneEffect::Backend(BackendIntent::Load { input, .. }) => {
                    input.cancellation().store(true, Ordering::Release);
                    return;
                }
                _ => {}
            }
        }
        if !runtime.multiview.effect_current(runtime, &effect) {
            return;
        }
        match effect {
            SceneEffect::PrepareMetadata { ticket, work } => {
                let index = ticket.slot().index();
                if self.workers[index].is_some() || self.pictures[index].is_some() {
                    return;
                }
                let cancel = work.cancellation();
                let done = Arc::new(AtomicBool::new(false));
                let finish = done.clone();
                let tx = self.tx.clone();
                if plx_base::task::spawn_off_frame("local-mv-metadata", move |worker| {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        work.prepare_local_metadata(worker)
                    }))
                    .unwrap_or(Err(multiview_sources::Error::Resolve));
                    let _ = tx.send(Job::Metadata(ticket, result));
                    finish.store(true, Ordering::Release);
                }) {
                    self.workers[index] = Some(Work {
                        ticket,
                        cancel,
                        done,
                        terminal_consumed: false,
                    });
                } else {
                    let t = runtime
                        .multiview
                        .metadata_failed(ticket, multiview_sources::Error::Exhausted);
                    self.queue(runtime, t);
                }
            }
            SceneEffect::CancelResolve {
                ticket,
                cancellation,
            } => {
                cancellation.store(true, Ordering::Release);
                if let Some(work) = self.workers[ticket.slot().index()]
                    .as_ref()
                    .filter(|w| w.ticket == ticket)
                {
                    work.cancel.store(true, Ordering::Release);
                }
            }
            SceneEffect::Backend(BackendIntent::Load { ticket, input }) => {
                if input.reauthorize(runtime, app).is_err() {
                    let t = runtime
                        .multiview
                        .backend_start_completed(ticket, AdmissionOutcome::Rejected);
                    self.queue(runtime, t);
                    return;
                }
                let index = ticket.slot().index();
                let cancel = input.cancellation();
                let done = Arc::new(AtomicBool::new(false));
                let finish = done.clone();
                let tx = self.tx.clone();
                if plx_base::task::spawn_off_frame("local-mv-source", move |worker| {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        input.resolve_local(worker)
                    }))
                    .unwrap_or(Err(multiview_sources::Error::Resolve));
                    let _ = tx.send(Job::Media(ticket, result));
                    finish.store(true, Ordering::Release);
                }) {
                    self.workers[index] = Some(Work {
                        ticket,
                        cancel,
                        done,
                        terminal_consumed: false,
                    });
                } else {
                    let t = runtime
                        .multiview
                        .backend_start_completed(ticket, AdmissionOutcome::Rejected);
                    self.queue(runtime, t);
                }
            }
            SceneEffect::Backend(BackendIntent::Stop { ticket }) => {
                let index = ticket.slot().index();
                self.stopping[index] = Some(ticket);
                if let Some(w) = self.workers[index].as_ref().filter(|w| w.ticket == ticket) {
                    w.cancel.store(true, Ordering::Release);
                }
                if let Some(p) = self.pictures[index].as_mut().filter(|p| p.ticket == ticket) {
                    p.decoder.cancel();
                    p.frame = None;
                }
                self.clear_slot_audio(ticket.slot());
                if !self.mixing && self.audio_applied == Some(ticket.slot()) {
                    self.audio_applied = None;
                }
            }
            SceneEffect::Backend(BackendIntent::Layout { layout, .. }) => {
                self.layout = Some(layout);
                self.geometry_revision = self.geometry_revision.saturating_add(1);
            }
            SceneEffect::Backend(BackendIntent::AudioOwner {
                scene,
                audio_revision,
                owner,
            }) => {
                if !self.mixing || owner.is_none() {
                    self.reset_audio();
                }
                self.audio_applied = None;
                self.audio_request = Some((scene, audio_revision, owner));
            }
            SceneEffect::Backend(BackendIntent::Focus { .. }) => {}
        }
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        for w in self.workers.iter().flatten() {
            w.cancel.store(true, Ordering::Release);
        }
        for p in self.pictures.iter_mut().flatten() {
            p.receipt
                .input
                .cancellation()
                .store(true, Ordering::Release);
            p.decoder.cancel();
        }
    }
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_multiview_local_lease_tests.rs"]
pub(super) mod lease_tests;
