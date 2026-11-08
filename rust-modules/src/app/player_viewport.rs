//! One app-owned placement lease for the same native player. The screen asks;
//! this runner verifies its live owner and performs only the media placement API.
use super::{bridge::AppHost, App};
use plx_machine::machine::{Delivery, Fx, InstanceId, MachineId, RequestId};
use plx_media::player::viewport::{self, Source, Target};
use plx_screens::registry::{AppMsg, PlayerReq, PlayerViewportAction as Action,
    PlayerViewportFailure as Failure, PlayerViewportOutcome as Outcome, PlayerViewportRequest};
use plx_ui::{dispatch::Dispatcher, screen::ScreenEvent};

const UNRECORDED: &str = "Native player placement is outside the supported recording domain";

struct Lease {
    owner: InstanceId,
    claim: RequestId,
    source: Source,
    target: Target,
    retiring: bool,
}

#[derive(Default)]
pub(crate) struct Placement {
    lease: Option<Lease>,
    published: Option<(InstanceId, Source)>,
    /// A synchronous reply is queued before the screen consumes it. Keep its
    /// monotonic watermark so a duplicate in that same drain cannot apply twice.
    /// Entries live no longer than the actual mounted screen instances.
    requests: Vec<(InstanceId, RequestId)>,
}

impl Placement {
    /// A retiring aperture still owns placement until its fullscreen restore settles.
    pub(crate) fn has_lease(&self) -> bool { self.lease.is_some() }
}

fn screen(d: &Dispatcher<AppHost>, owner: InstanceId) -> Option<&plx_screens::ott::OttScreen> {
    let body = d.nav.instance(owner)?;
    if body.staged { return None; }
    body.screen.as_any()?.downcast_ref()
}

fn accepts(d: &Dispatcher<AppHost>, request: &PlayerViewportRequest) -> bool {
    let MachineId::Instance(owner) = request.reply.to else { return false; };
    screen(d, owner).is_some_and(|screen| screen.accepts_viewport_request(request))
}

fn begin(d: &mut Dispatcher<AppHost>, request: &PlayerViewportRequest) -> bool {
    if !accepts(d, request) || d.nav.is_deliverable(&request.reply) { return false; }
    let MachineId::Instance(owner) = request.reply.to else { return false; };
    d.track_inflight(owner, request.reply.req);
    true
}

fn finish(d: &mut Dispatcher<AppHost>, request: &PlayerViewportRequest, outcome: Outcome) {
    if !d.nav.is_deliverable(&request.reply) { return; }
    let live = accepts(d, request);
    let MachineId::Instance(owner) = request.reply.to else { return; };
    if let Some(body) = d.nav.instance_mut(owner) {
        body.inflight.retain(|id| *id != request.reply.req);
    }
    if live {
        d.emit(MachineId::Player, Fx::Deliver(request.reply.to,
            Delivery::Screen(ScreenEvent::Async(request.reply.req,
                AppMsg::PlayerViewportReply { outcome }))));
    }
}

/// A bypass of the app runner cannot place the sink through the legacy player
/// request performer. It still completes an exact live request with a refusal.
pub(crate) fn reject_unhandled(d: &mut Dispatcher<AppHost>, request: &PlayerViewportRequest) {
    if begin(d, request) { finish(d, request, Outcome::Rejected(Failure::Inactive)); }
}

fn current_target(app: &App) -> Option<(InstanceId, Target)> {
    let owner = super::ott_runtime::current_ui_owner(app)?;
    if !super::ott_runtime::playback_allowed(app)
        || super::ott_runtime::kiosk_user_keys_blocked(app)
        || app.player.lifecycle.awaiting_load() || app.decoder_parked
        || app.pages.nav.suspended
        || app.pages.has_pending_navigation() || app.pages.nav.tabs.stack.is_pending()
        || app.pages.nav.tabs.stack.transition.in_flight()
    { return None; }
    let entry = app.pages.nav.entry_of_instance(owner)?;
    // A held modal is only prewarmed into a recording; its aperture is not
    // visible yet. Keep the native sink fullscreen until the live draw begins.
    if app.pages.nav.modals.surfaces.iter().any(|surface|
        surface.entry.id == entry && surface.motion.held()) { return None; }
    let target = screen(&app.pages, owner)?.viewport_target()?;
    matches!(target, Target::Inline { .. }).then_some((owner, target))
}

fn publish(d: &mut Dispatcher<AppHost>, owner: InstanceId, source: Option<Source>) {
    d.emit(MachineId::Player, Fx::Deliver(MachineId::Instance(owner),
        Delivery::Screen(ScreenEvent::App(AppMsg::PlayerViewportSource(source)))));
}

/// Host composition uses the same admitted lease as the native plane, and only
/// after the original screen consumed its matching Submitted response. A queued
/// acknowledgment never makes an invisible or covered aperture eligible.
#[cfg(feature = "hostsim")]
pub(crate) fn composite(app: &mut App) -> bool {
    let Some(lease) = app.player_viewport.lease.as_ref() else { return false; };
    if lease.retiring || current_target(app) != Some((lease.owner, lease.target))
        || !screen(&app.pages, lease.owner).is_some_and(|screen|
            screen.has_submitted_viewport(&lease.source, lease.claim, lease.target))
    { return false; }
    viewport::capture_source(&mut app.adapters.player, &app.player.session).as_ref() == Some(&lease.source)
}

/// Runs before dispatch and after navigation/update, including non-presenting
/// frames. Restoration is app-owned even after the emitting screen is gone.
pub(crate) fn sync(app: &mut App) {
    let target = current_target(app);
    let mut source = target.and_then(|_| viewport::capture_source(
        &mut app.adapters.player, &app.player.session));
    if source.is_some() && (matches!(&app.rec, super::recorder::Recplay::Replaying(_))
        || app.bridge.is_controlled_replay())
    {
        app.rec.refuse(UNRECORDED);
        source = None;
    }
    let mut placement = std::mem::take(&mut app.player_viewport);
    placement.requests.retain(|(owner, _)| screen(&app.pages, *owner).is_some());
    if let Some(lease) = placement.lease.as_mut() {
        lease.retiring |= target != Some((lease.owner, lease.target))
            || source.as_ref() != Some(&lease.source);
        if lease.retiring {
            app.rec.refuse(UNRECORDED);
            let result = viewport::apply(&mut app.adapters.player, &app.player.session,
                &lease.source, Target::Fullscreen);
            if result.is_ok() || !viewport::source_is_current(
                &mut app.adapters.player, &app.player.session, &lease.source)
            { placement.lease = None; }
        }
    }
    let publication = target.and_then(|(owner, _)| source.map(|source| (owner, source)));
    if placement.published != publication {
        if let Some((old_owner, _)) = &placement.published {
            if publication.as_ref().is_none_or(|(owner, _)| owner != old_owner) {
                publish(&mut app.pages, *old_owner, None);
            }
        }
        if let Some((owner, source)) = &publication {
            // A tape cannot silently replay native capability publication as a
            // generic catalogue event: playback is outside its accepted domain.
            app.rec.refuse(UNRECORDED);
            publish(&mut app.pages, *owner, Some(source.clone()));
        }
        placement.published = publication;
    }
    app.player_viewport = placement;
}

pub(crate) fn request(app: &mut App, request: PlayerReq) -> Option<PlayerReq> {
    let PlayerReq::ViewportAddressed(request) = request else { return Some(request); };
    let MachineId::Instance(owner) = request.reply.to else { return None; };
    if app.player_viewport.requests.iter().any(|(seen_owner, seen)|
        *seen_owner == owner && seen.0 >= request.reply.req.0)
        || !begin(&mut app.pages, &request)
    { return None; }
    if let Some((_, seen)) = app.player_viewport.requests.iter_mut().find(|(seen_owner, _)| *seen_owner == owner) {
        *seen = request.reply.req;
    } else {
        app.player_viewport.requests.push((owner, request.reply.req));
    }
    if matches!(&app.rec, super::recorder::Recplay::Replaying(_)) || app.bridge.is_controlled_replay() {
        app.rec.refuse(UNRECORDED);
        finish(&mut app.pages, &request, Outcome::Rejected(Failure::Inactive));
        return None;
    }
    let outcome = match request.action {
        Action::Set(target) => {
            if current_target(app) != Some((owner, target)) {
                Outcome::Rejected(Failure::Inactive)
            } else {
                app.rec.refuse(UNRECORDED);
                match viewport::apply(&mut app.adapters.player, &app.player.session, &request.source, target) {
                    Ok(_) => {
                        app.player_viewport.lease = Some(Lease { owner, claim: request.reply.req,
                            source: request.source.clone(), target, retiring: false });
                        Outcome::Submitted
                    }
                    Err(error) => Outcome::Rejected(Failure::Media(error)),
                }
            }
        }
        Action::Release { claim } => {
            let exact = app.player_viewport.lease.as_ref().is_some_and(|lease|
                lease.owner == owner && lease.claim == claim && lease.source == request.source);
            if !exact {
                Outcome::Rejected(Failure::StaleClaim)
            } else {
                if let Some(lease) = app.player_viewport.lease.as_mut() { lease.retiring = true; }
                app.rec.refuse(UNRECORDED);
                match viewport::apply(&mut app.adapters.player, &app.player.session,
                    &request.source, Target::Fullscreen)
                {
                    Ok(_) => { app.player_viewport.lease = None; Outcome::Submitted }
                    Err(error) => Outcome::Rejected(Failure::Media(error)),
                }
            }
        }
    };
    finish(&mut app.pages, &request, outcome);
    None
}
