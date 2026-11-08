//! Fullscreen picture intent belongs to one logical playback, not one decoder epoch.
//! Only the live More owner can change it. Native capability and placement remain
//! in the media layer; this runner neither starts playback nor changes transport.
use super::{bridge::AppHost, App};
use plx_machine::machine::{Delivery, Fx, InputOwner, InstanceId, MachineId, RequestId};
use plx_media::{
    player::{
        video_geometry::Transform,
        viewport::{self, Source},
    },
    route::{self, PlaybackIdentity},
};
use plx_screens::{
    player::overlay::{OverlayKind, PlayerOverlayScreen},
    registry::{
        AppArg, AppMsg, PlayerGeometryFailure as Failure, PlayerGeometryOffer as Offer,
        PlayerGeometryOutcome as Outcome, PlayerGeometryRequest as Request, PlayerReq,
    },
};
use plx_ui::{containers::modal::Phase, dispatch::Dispatcher, screen::ScreenEvent};

const UNRECORDED: &str = "Native fullscreen geometry is outside the supported recording domain";

struct Accepted {
    identity: PlaybackIdentity,
    transform: Transform,
}

#[derive(Default)]
pub(crate) struct Geometry {
    accepted: Option<Accepted>,
    /// Do not repeat a refused automatic placement every frame. NotReady is not
    /// recorded here: an engine can be ready before its first decoded picture.
    attempted: Option<(Source, Transform)>,
    /// A queued synchronous response has not yet retired the screen's pending
    /// request. Remember its monotonic ID until that screen instance disappears.
    requests: Vec<(InstanceId, RequestId)>,
}

impl Geometry {
    pub(crate) fn clear_exit(&mut self, identity: Option<&PlaybackIdentity>) {
        if self
            .accepted
            .as_ref()
            .is_some_and(|accepted| Some(&accepted.identity) == identity)
        {
            self.accepted = None;
            self.attempted = None;
        }
    }

    fn reconcile(&mut self, identity: Option<&PlaybackIdentity>) {
        if identity.is_some_and(|identity| {
            self.accepted
                .as_ref()
                .is_some_and(|accepted| &accepted.identity != identity)
        }) {
            self.accepted = None;
            self.attempted = None;
        }
    }
}

fn screen(d: &Dispatcher<AppHost>, owner: InstanceId) -> Option<&PlayerOverlayScreen> {
    let body = d.nav.instance(owner)?;
    if body.staged {
        return None;
    }
    body.screen.as_any()?.downcast_ref()
}

fn accepts(d: &Dispatcher<AppHost>, request: &Request) -> bool {
    let MachineId::Instance(owner) = request.reply.to else {
        return false;
    };
    screen(d, owner).is_some_and(|screen| screen.accepts_geometry_request(request))
}

fn begin(d: &mut Dispatcher<AppHost>, request: &Request) -> bool {
    if !accepts(d, request) || d.nav.is_deliverable(&request.reply) {
        return false;
    }
    let MachineId::Instance(owner) = request.reply.to else {
        return false;
    };
    d.track_inflight(owner, request.reply.req);
    true
}

fn finish(d: &mut Dispatcher<AppHost>, request: &Request, outcome: Outcome) {
    if !d.nav.is_deliverable(&request.reply) {
        return;
    }
    let live = accepts(d, request);
    let MachineId::Instance(owner) = request.reply.to else {
        return;
    };
    if let Some(body) = d.nav.instance_mut(owner) {
        body.inflight.retain(|id| *id != request.reply.req);
    }
    if live {
        d.emit(
            MachineId::Player,
            Fx::Deliver(
                request.reply.to,
                Delivery::Screen(ScreenEvent::Async(
                    request.reply.req,
                    AppMsg::PlayerGeometryReply { outcome },
                )),
            ),
        );
    }
}

pub(crate) fn reject_unhandled(d: &mut Dispatcher<AppHost>, request: &Request) {
    if begin(d, request) {
        finish(d, request, Outcome::Rejected(Failure::Inactive));
    }
}

fn replaying(app: &App) -> bool {
    matches!(&app.rec, super::recorder::Recplay::Replaying(_)) || app.bridge.is_controlled_replay()
}

fn ready(app: &App) -> bool {
    matches!(app.route(), AppArg::Player)
        && super::bridge::player(&app.pages).is_some()
        && super::ott_runtime::playback_allowed(app)
        && !super::ott_runtime::kiosk_user_keys_blocked(app)
        && !app.player.lifecycle.awaiting_load()
        && app.player.pending_exit.is_none()
        && !app.decoder_parked
        && !app.pages.nav.suspended
        && !app.pages.has_pending_navigation()
        && !app.pages.nav.tabs.stack.is_pending()
        && !app.pages.nav.tabs.stack.transition.in_flight()
        && !route::play_pending()
        && !app.player_viewport.has_lease()
}

fn current_more_owner(app: &App) -> Option<InstanceId> {
    let InputOwner::Entry(entry) = app.pages.nav.input_owner()? else {
        return None;
    };
    let surface = app
        .pages
        .nav
        .modals
        .surfaces
        .iter()
        .find(|surface| surface.entry.id == entry)?;
    if !matches!(surface.phase, Phase::Opening | Phase::Open)
        || surface.motion.held()
        || !matches!(&surface.entry.arg, AppArg::PlayerOverlay(arg) if matches!(arg.kind, OverlayKind::More { .. }))
    {
        return None;
    }
    let body = surface.entry.inst.as_ref()?;
    (!body.staged).then_some(body.id)
}

fn can_offer(app: &App) -> bool {
    let mut surfaces = app
        .pages
        .nav
        .modals
        .surfaces
        .iter()
        .filter(|surface| surface.phase != Phase::Hidden);
    if let Some(surface) = surfaces.next() {
        // Opening mounts with a presentation hold. Keep its passive source offer
        // stable through that hold; withdrawing it permanently retires the screen's
        // captured identity. current_more_owner still forbids applying while held.
        if surfaces.next().is_some()
            || !matches!(surface.phase, Phase::Opening | Phase::Open)
            || !matches!(&surface.entry.arg, AppArg::PlayerOverlay(arg) if matches!(arg.kind, OverlayKind::More { .. }))
        {
            return false;
        }
    }
    true
}

fn actual_offer(app: &mut App) -> Option<Offer> {
    let source = viewport::capture_source(&mut app.adapters.player, &app.player.session)?;
    let state =
        viewport::content_state(&mut app.adapters.player, &app.player.session, &source).ok()?;
    Some(Offer { source, state })
}

/// Runs after placement reconciliation. A temporary engine gap keeps the logical
/// intent; a new logical source or matching explicit exit retires it. Publication
/// always describes the current engine, including after an automatic refusal.
pub(crate) fn sync(app: &mut App) {
    let identity = route::playback_identity(&app.player.session);
    let mut geometry = std::mem::take(&mut app.player_geometry);
    geometry.reconcile(identity.as_ref());
    if let Some(exit) = &app.player.pending_exit {
        geometry.clear_exit(exit.playback_identity.as_ref());
    }
    geometry
        .requests
        .retain(|(owner, _)| screen(&app.pages, *owner).is_some());
    let mut offer = None;
    if ready(app) && !replaying(app) {
        offer = actual_offer(app);
        if let (Some(current), Some(accepted)) = (offer.as_ref(), geometry.accepted.as_ref()) {
            if Some(&accepted.identity) == identity.as_ref()
                && current.state.transform != accepted.transform
                && geometry.attempted.as_ref()
                    != Some(&(current.source.clone(), accepted.transform))
            {
                app.rec.refuse(UNRECORDED);
                let result = viewport::apply_content(
                    &mut app.adapters.player,
                    &app.player.session,
                    &current.source,
                    accepted.transform,
                );
                if result != Err(viewport::Failure::NotReady) {
                    geometry.attempted = Some((current.source.clone(), accepted.transform));
                }
                offer = actual_offer(app);
            }
        }
        if !can_offer(app) {
            offer = None;
        }
    }
    if offer.is_some() {
        app.rec.refuse(UNRECORDED);
    }
    app.bridge.publish_geometry_offer(offer);
    app.player_geometry = geometry;
}

pub(crate) fn request(app: &mut App, request: PlayerReq) -> Option<PlayerReq> {
    let PlayerReq::GeometryAddressed(request) = request else {
        return Some(request);
    };
    let MachineId::Instance(owner) = request.reply.to else {
        return None;
    };
    if app
        .player_geometry
        .requests
        .iter()
        .any(|(seen_owner, seen)| *seen_owner == owner && seen.0 >= request.reply.req.0)
        || !begin(&mut app.pages, &request)
    {
        return None;
    }
    if let Some((_, seen)) = app
        .player_geometry
        .requests
        .iter_mut()
        .find(|(seen_owner, _)| *seen_owner == owner)
    {
        *seen = request.reply.req;
    } else {
        app.player_geometry
            .requests
            .push((owner, request.reply.req));
    }
    let outcome = if replaying(app) {
        app.rec.refuse(UNRECORDED);
        Outcome::Rejected(Failure::Inactive)
    } else if !ready(app) || current_more_owner(app) != Some(owner) {
        Outcome::Rejected(Failure::Inactive)
    } else if app.bridge.geometry_offer() != Some(&request.offer)
        || actual_offer(app).as_ref() != Some(&request.offer)
    {
        Outcome::Rejected(Failure::StaleOffer)
    } else if let Some(identity) = route::playback_identity(&app.player.session) {
        app.rec.refuse(UNRECORDED);
        match viewport::apply_content(
            &mut app.adapters.player,
            &app.player.session,
            &request.offer.source,
            request.transform,
        ) {
            Ok(_) => {
                app.player_geometry.accepted = Some(Accepted {
                    identity,
                    transform: request.transform,
                });
                app.player_geometry.attempted =
                    Some((request.offer.source.clone(), request.transform));
                Outcome::Submitted(viewport::ContentState {
                    transform: request.transform,
                    ..request.offer.state
                })
            }
            Err(error) => Outcome::Rejected(Failure::Media(error)),
        }
    } else {
        Outcome::Rejected(Failure::StaleOffer)
    };
    finish(&mut app.pages, &request, outcome);
    None
}
