//! The visible screen's addressed lease for the existing video plane. Source
//! capabilities are process-local; this state never chooses or starts media.
use super::*;
use crate::registry::{PlayerReq, PlayerViewportAction, PlayerViewportOutcome, PlayerViewportRequest};
use plx_machine::machine::{Addr, RequestId};
use plx_media::player::viewport::{LogicalRect, Source, Target};

pub(super) struct ViewportState {
    source: Option<Source>,
    modern_rect: Option<Rect>,
    pending: Option<PlayerViewportRequest>,
    submitted: Option<PlayerViewportRequest>,
    blocked: Option<Target>,
    next: Option<u32>,
}
impl Default for ViewportState {
    fn default() -> Self { Self { source: None, modern_rect: None, pending: None, submitted: None, blocked: None, next: Some(1) } }
}
impl ViewportState {
    pub(super) fn write(&self, out: &mut Canon) {
        out.bool(self.source.is_some());
        out.option(self.next, |out, next| { out.u32(next); });
        for request in [&self.pending, &self.submitted] {
            out.option(request.as_ref(), |out, request| { out.u32(request.reply.req.0); write_action(out, request.action); });
        }
        out.option(self.blocked, |out, target| write_target(out, target));
    }
}
fn write_target(out: &mut Canon, target: Target) {
    match target {
        Target::Fullscreen => { out.u32(0); }
        Target::Inline { rect, canvas } => {
            out.u32(1).u32(rect.x as u32).u32(rect.y as u32).u32(rect.width as u32).u32(rect.height as u32)
                .u32(canvas.0 as u32).u32(canvas.1 as u32);
        }
    }
}
fn write_action(out: &mut Canon, action: PlayerViewportAction) {
    match action {
        PlayerViewportAction::Set(target) => { out.u32(0); write_target(out, target); }
        PlayerViewportAction::Release { claim } => { out.u32(1).u32(claim.0); }
    }
}

impl OttScreen {
    pub(super) fn refresh_viewport(&mut self, measure:&dyn plx_machine::machine::Measure) {
        self.viewport.modern_rect=if self.classic_mode || self.browse || self.editor.is_some() || self.reading { None }
            else { self.media.and_then(|i| self.snapshot.rows.get(i))
                .and_then(|row| metadata::preview_rect(row,&self.status,measure,self.route(),self.guide_clock.now_for(row.guide.as_ref()))) };
    }
    pub fn viewport_target(&self) -> Option<Target> {
        if !self.classic_visible || self.editor.is_some() || self.reading
            || self.snapshot.page_id == "loading" { return None; }
        let rect = if self.classic_mode { self.classic_layout.get()?.preview }
            else { if self.browse || self.media.is_none() { return None; } self.viewport.modern_rect? };
        if rect.w <= 0.0 || rect.h <= 0.0 { return None; }
        Some(Target::Inline { rect: LogicalRect {
            x: rect.x.round() as i32, y: rect.y.round() as i32,
            width: rect.w.round() as i32, height: rect.h.round() as i32,
        }, canvas: (Rect::FULL.w as i32, Rect::FULL.h as i32) })
    }
    pub fn accepts_viewport_request(&self, request: &PlayerViewportRequest) -> bool {
        self.viewport.pending.as_ref() == Some(request)
            && self.viewport.source.as_ref() == Some(&request.source)
            && match request.action {
                PlayerViewportAction::Set(target) => self.viewport_target() == Some(target),
                PlayerViewportAction::Release { claim } => self.viewport.submitted.as_ref()
                    .is_some_and(|submitted| submitted.reply.req == claim && submitted.source == request.source),
            }
    }
    /// App compositing is permitted only after this exact source/claim/target
    /// was acknowledged and remains eligible for the current visible screen.
    pub fn has_submitted_viewport(&self, source: &Source, claim: RequestId, target: Target) -> bool {
        self.viewport.source.as_ref() == Some(source)
            && self.viewport_target() == Some(target)
            && self.viewport.submitted.as_ref().is_some_and(|request|
                &request.source == source && request.reply.req == claim
                    && request.action == PlayerViewportAction::Set(target))
    }
    pub(super) fn viewport_aperture(&self) -> Option<Rect> {
        let submitted = self.viewport.submitted.as_ref()?;
        if self.viewport.source.as_ref() != Some(&submitted.source) { return None; }
        let PlayerViewportAction::Set(target) = submitted.action else { return None; };
        if self.viewport_target() != Some(target) { return None; }
        let Target::Inline { rect, .. } = target else { return None; };
        Some(Rect::new(rect.x as f32, rect.y as f32, rect.width as f32, rect.height as f32))
    }
    fn request_viewport<H: AppLike>(&mut self, action: PlayerViewportAction, fx: &mut Effects<'_, H>) {
        let Some(source) = &self.viewport.source else { return; };
        let from = fx.from();
        if from != MachineId::Instance(self.instance) { return; }
        let Some(id) = self.viewport.next else { return; };
        self.viewport.next = id.checked_add(1);
        let request = PlayerViewportRequest { reply: Addr { to: from, req: RequestId(id) }, source: source.clone(), action };
        self.viewport.pending = Some(request.clone());
        fx.push(Fx::App(AppFx::Player(PlayerReq::ViewportAddressed(request))));
    }
    pub(super) fn sync_viewport<H: AppLike>(&mut self, fx: &mut Effects<'_, H>) {
        let desired = self.viewport_target();
        // A Set that lost eligibility cannot later reopen its hole. The app
        // independently reconciles fullscreen even when this page is destroyed.
        if self.viewport.pending.as_ref().is_some_and(|request| matches!(request.action,
            PlayerViewportAction::Set(target) if desired != Some(target))) { self.viewport.pending = None; }
        if self.viewport.pending.is_some() { return; }
        let submitted = self.viewport.submitted.as_ref().and_then(|request| match request.action {
            PlayerViewportAction::Set(target) => Some((request.reply.req, target)), _ => None,
        });
        if let Some((claim, target)) = submitted {
            if desired == Some(target) { return; }
            self.request_viewport(PlayerViewportAction::Release { claim }, fx);
            return;
        }
        if let Some(target) = desired.filter(|target| self.viewport.blocked != Some(*target)) {
            self.request_viewport(PlayerViewportAction::Set(target), fx);
        }
    }
    pub(super) fn viewport_event<H: AppLike>(&mut self, event: &ScreenEvent<H>, fx: &mut Effects<'_, H>) -> bool {
        match event {
            ScreenEvent::App(AppMsg::PlayerViewportSource(source)) => {
                if self.viewport.source != *source {
                    self.viewport.source = source.clone(); self.viewport.pending = None;
                    self.viewport.submitted = None; self.viewport.blocked = None;
                    fx.invalidate(plx_machine::present::Provenance::Lifecycle);
                }
                true
            }
            ScreenEvent::Async(id, AppMsg::PlayerViewportReply { outcome }) => {
                let Some(pending) = self.viewport.pending.as_ref().filter(|request| request.reply.req == *id) else { return true; };
                if !self.accepts_viewport_request(pending) { self.viewport.pending = None; return true; }
                let pending = self.viewport.pending.take().expect("matched pending viewport");
                match (pending.action, outcome) {
                    (PlayerViewportAction::Set(_), PlayerViewportOutcome::Submitted) => {
                        self.viewport.submitted = Some(pending); self.viewport.blocked = None;
                    }
                    (PlayerViewportAction::Set(target), PlayerViewportOutcome::Rejected(_)) => {
                        self.viewport.submitted = None; self.viewport.blocked = Some(target);
                    }
                    (PlayerViewportAction::Release { .. }, _) => { self.viewport.submitted = None; }
                }
                fx.invalidate(plx_machine::present::Provenance::Lifecycle);
                true
            }
            ScreenEvent::Resume | ScreenEvent::Uncover => { self.viewport.blocked = None; false }
            _ => false,
        }
    }
}
