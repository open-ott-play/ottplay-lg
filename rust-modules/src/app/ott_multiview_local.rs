//! Independent on-device slots. No fullscreen route ticket, compositor client,
//! global player transport or active-provider mutation is spent here.
use super::multiview::{BackendIntent, Effect, StopResult, Transition};
use super::multiview_sources::{LocalResolved, PreparedMetadata};
use super::*;
use plx_data::ott::multiview::{AdmissionOutcome, Layout, ResolveTicket, SceneId, SlotId};
use plx_media::local_multiview::{Event, FrameYuv, Limits, PcmChunk, Session};
use std::sync::mpsc::{self, Receiver, Sender};

#[path = "ott_multiview_local_commands.rs"]
mod commands;
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
    workers: [Option<Work>; 9],
    stopping: [Option<ResolveTicket>; 9],
    leases: [Option<LeaseWork>; 9],
    tx: Sender<Job>,
    rx: Receiver<Job>,
    layout: Option<Layout>,
    closing: bool,
    audio_request: Option<(SceneId, u64, Option<SlotId>)>,
    audio_applied: Option<SlotId>,
    audio: std::collections::VecDeque<(SlotId, ResolveTicket, PcmChunk)>,
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
            workers: std::array::from_fn(|_| None),
            stopping: [None; 9],
            leases: std::array::from_fn(|_| None),
            tx,
            rx,
            layout: None,
            closing: false,
            audio_request: None,
            audio_applied: None,
            audio: Default::default(),
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
        self.workers.iter().any(Option::is_some)
            || self.pictures.iter().any(Option::is_some)
            || self.stopping.iter().any(Option::is_some)
            || self.replacements.iter().any(Option::is_some)
            || self.leases.iter().any(Option::is_some)
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
        if self.layout == Some(Layout::Single) {
            return plx_appkit::multiview_layout::RasterPlan::checked(
                self.geometry_revision,
                width,
                height,
                Mode::Perimeter { satellites: 0 },
                vec![plx_appkit::multiview_layout::RasterSlot {
                    id: 0,
                    layer: 0,
                    x: 0,
                    y: 0,
                    width,
                    height,
                }],
            )
            .ok();
        }
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
        .rasterize_even(self.geometry_revision)
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
        if self.stopping[index].is_some() || self.replacements[index].is_some() {
            self.replacements[index] = Some(handle);
            return Ok(());
        }
        if self.workers[index].is_some()
            || self.pictures[index].is_some()
            || self.leases[index].is_some()
        {
            let transition = runtime.multiview.close_slot(slot)?;
            self.replacements[index] = Some(handle);
            runtime.multiview_effects.extend(transition.effects);
            return Ok(());
        }
        let t = runtime.multiview.bind_source(slot, handle)?;
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
            .filter(|p| p.admitted)
            .ok_or("No active picture")?;
        p.receipt
            .input
            .reauthorize(runtime, app)
            .map_err(|_| "Source changed")?;
        p.decoder.set_paused(paused);
        p.paused = paused;
        self.audio.clear();
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
        self.audio.clear();
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
            .filter(|p| p.admitted && !p.receipt.input.is_cancelled())
            .map(|p| p.ticket)
    }
    pub fn audio_owner(&self) -> Option<SlotId> {
        self.audio_applied
    }
    pub fn audio_output_failed(&mut self, runtime: &mut Runtime) {
        self.audio_applied = None;
        self.audio_request = None;
        self.audio.clear();
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
            self.audio.clear();
        }
    }
    pub fn take_audio(&mut self) -> Option<(SlotId, ResolveTicket, PcmChunk)> {
        self.audio.pop_front()
    }
    fn queue(&mut self, runtime: &mut Runtime, transition: Result<Transition, multiview::Error>) {
        if let Ok(transition) = transition {
            runtime.multiview_effects.extend(transition.effects);
        }
    }
    pub fn revoke(&mut self, runtime: &mut Runtime) {
        self.restore_previous = false;
        self.replacements = [None; 9];
        if !self.closing {
            self.closing = true;
            let transition = runtime.multiview.close_scene();
            self.queue(runtime, transition);
        }
        self.audio_applied = None;
        self.audio_request = None;
        self.audio.clear();
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
            if !slots_ready
                && !self.closing
                && matches!(
                    &effect,
                    Effect::PrepareMetadata { .. } | Effect::Backend(BackendIntent::Load { .. })
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
            let ticket = match &job {
                Job::Metadata(t, _) | Job::Media(t, _) => *t,
            };
            let index = ticket.slot().index();
            if !self.workers[index]
                .as_ref()
                .is_some_and(|w| w.ticket == ticket && !w.cancel.load(Ordering::Acquire))
            {
                continue;
            }
            self.workers[index] = None;
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
                    let receipt = result
                        .ok()
                        .filter(|r| r.input.reauthorize(runtime, app).is_ok());
                    if let Some(receipt) = receipt {
                        match Session::open(receipt.source.clone(), Limits::default()) {
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
                                let transition = runtime
                                    .multiview
                                    .backend_start_completed(ticket, AdmissionOutcome::Unsupported);
                                self.queue(runtime, transition);
                            }
                        }
                    } else {
                        let transition = runtime
                            .multiview
                            .backend_start_completed(ticket, AdmissionOutcome::Rejected);
                        self.queue(runtime, transition);
                    }
                }
            }
        }
        for index in 0..9 {
            let Some(picture) = self.pictures[index].as_mut() else {
                continue;
            };
            if picture.receipt.input.reauthorize(runtime, app).is_err()
                && self.stopping[index].is_none()
            {
                let transition = runtime.multiview.close_slot(picture.ticket.slot());
                if let Ok(transition) = transition {
                    runtime.multiview_effects.extend(transition.effects);
                }
                picture.decoder.cancel();
                picture.frame = None;
                picture
                    .receipt
                    .input
                    .cancellation()
                    .store(true, Ordering::Release);
            }
            picture.decoder.set_audio(
                self.audio_applied == Some(picture.ticket.slot()) && self.stopping[index].is_none(),
            );
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
                        if self.audio_applied == Some(picture.ticket.slot())
                            && picture.admitted
                            && self.stopping[index].is_none()
                            && !picture.receipt.input.is_cancelled() =>
                    {
                        if self.audio.len() >= 8 {
                            self.audio.pop_front();
                        }
                        self.audio
                            .push_back((picture.ticket.slot(), picture.ticket, chunk));
                    }
                    Event::Failure(_) | Event::Ended => {
                        picture.terminal = true;
                        picture.frame = None;
                        picture.decoder.cancel();
                        if self.audio_applied == Some(picture.ticket.slot()) {
                            self.audio_applied = None;
                            self.audio.clear();
                        }
                        let transition = if picture.admitted {
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
            if self.workers[index]
                .as_ref()
                .is_some_and(|w| w.cancel.load(Ordering::Acquire) && w.done.load(Ordering::Acquire))
            {
                self.workers[index] = None;
            }
            let retired = self.pictures[index].as_ref().is_none_or(|p| {
                p.decoder.is_retired()
                    && (p.terminal
                        || self.stopping[index].is_some()
                        || p.receipt.input.is_cancelled())
            });
            if retired && self.workers[index].is_none() {
                if let Some(mut picture) = self.pictures[index].take() {
                    if let Some(lease) = picture.receipt.lease.take() {
                        self.leases[index] = Some(LeaseWork {
                            lease: Arc::new(std::sync::Mutex::new(Some(lease))),
                            done: Arc::new(AtomicBool::new(false)),
                            success: Arc::new(AtomicBool::new(false)),
                            started: false,
                        });
                    }
                }
            }
            if let Some(cleanup) = self.leases[index].as_mut() {
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
                if cleanup.done.load(Ordering::Acquire) {
                    if !cleanup.success.load(Ordering::Acquire) {
                        self.cleanup_failed = true;
                        self.restore_previous = false;
                        self.replacements[index] = None;
                        runtime.status =
                            "Previous window playback session did not finish closing".into();
                        runtime.changed();
                    }
                    self.leases[index] = None;
                }
            }
            if self.pictures[index].is_none()
                && self.workers[index].is_none()
                && self.leases[index].is_none()
            {
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
                    }
                }
            }
        }
        if self.closing && !self.has_work() {
            self.layout = None;
            self.closing = false;
            self.restore_ready = std::mem::take(&mut self.restore_previous) && !self.cleanup_failed;
        }
    }
    fn apply(&mut self, runtime: &mut Runtime, app: &App, effect: Effect) {
        if self.closing {
            match &effect {
                Effect::PrepareMetadata { work, .. } => {
                    work.cancellation().store(true, Ordering::Release);
                    return;
                }
                Effect::Backend(BackendIntent::Load { input, .. }) => {
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
            Effect::PrepareMetadata { ticket, work } => {
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
                    });
                } else {
                    let t = runtime
                        .multiview
                        .metadata_failed(ticket, multiview_sources::Error::Exhausted);
                    self.queue(runtime, t);
                }
            }
            Effect::CancelResolve {
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
            Effect::Backend(BackendIntent::Load { ticket, input }) => {
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
                    });
                } else {
                    let t = runtime
                        .multiview
                        .backend_start_completed(ticket, AdmissionOutcome::Rejected);
                    self.queue(runtime, t);
                }
            }
            Effect::Backend(BackendIntent::Stop { ticket }) => {
                let index = ticket.slot().index();
                self.stopping[index] = Some(ticket);
                if let Some(w) = self.workers[index].as_ref().filter(|w| w.ticket == ticket) {
                    w.cancel.store(true, Ordering::Release);
                }
                if let Some(p) = self.pictures[index].as_mut().filter(|p| p.ticket == ticket) {
                    p.decoder.cancel();
                    p.frame = None;
                }
                if self.audio_applied == Some(ticket.slot()) {
                    self.audio_applied = None;
                    self.audio.clear();
                }
            }
            Effect::Backend(BackendIntent::Layout { layout, .. }) => {
                self.layout = Some(layout);
                self.geometry_revision = self.geometry_revision.saturating_add(1);
            }
            Effect::Backend(BackendIntent::AudioOwner {
                scene,
                audio_revision,
                owner,
            }) => {
                self.audio.clear();
                self.audio_applied = None;
                self.audio_request = Some((scene, audio_revision, owner));
            }
            Effect::Backend(BackendIntent::Focus { .. }) => {}
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
