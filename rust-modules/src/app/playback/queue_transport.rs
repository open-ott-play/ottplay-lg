//! Explicit queue navigation and Up Next share one stop → resolve → start ritual.
use plx_screens::player::input::HudStart;
use super::*;
use plx_media::route::{previous_restarts, queue_neighbor, QueueStep};
use plx_plex::plex::QueueRow;

#[derive(Clone, Copy)]
pub(super) enum Candidate { Neighbor(QueueStep), UpNext }

pub(super) enum PlayOutcome { Refused, Started, Failed }

/// Clone the exact proposed leaf before source authorization can rebind a
/// catalogue or presentation. The candidate is never inferred from its title.
fn target(ps: &plx_media::route::PlaybackSession, candidate: Candidate) -> Option<QueueRow> {
    match candidate {
        Candidate::Neighbor(direction) => queue_neighbor(ps, direction),
        Candidate::UpNext => {
            let u = plx_media::route::up_next(ps)?.clone();
            if let Some(row) = queue_neighbor(ps, QueueStep::Next).filter(|row| row.rk == u.rk) {
                return Some(row);
            }
            Some(QueueRow {
                sid: plx_media::route::item_sid(plx_media::route::cur_sid(ps)),
                rk: u.rk, kind: "episode".into(), title: u.ep_title, show_title: u.show_title,
                season: u.season, index: u.index, part: u.part, vcodec: u.vcodec, acodec: u.acodec,
                resume_ms: u.resume_ms, dur_ms: u.dur_ms, ..Default::default()
            })
        }
    }
}

fn row_key(row: &QueueRow) -> (plx_plex::plex::ServerId, i64, &str) {
    (row.sid, row.item_id, &row.rk)
}

pub(super) fn step(
    app: &mut super::super::App,
    direction: QueueStep,
) -> ViewerTransport {
    if direction == QueueStep::Previous
        && previous_restarts(plx_media::player::intended_pos_ns(&app.player.session))
    {
        request_seek(0);
        resume_if_paused(&mut app.adapters.player);
        return ViewerTransport::Seek;
    }
    play(app, Candidate::Neighbor(direction), HudStart::Ordinary, false, true);
    ViewerTransport::None
}

pub(super) fn play(
    app: &mut super::super::App,
    candidate: Candidate,
    hud_start: HudStart,
    finish_current: bool,
    interactive: bool,
) -> PlayOutcome {
    play_with(app, candidate, hud_start, finish_current, interactive, start)
}

/// The same final gate accepts an injected actuator in host tests. Production
/// always supplies `start`; tests can observe admitted targets without spawning
/// a resolver or constructing a second PIN/grant implementation.
pub(super) fn play_with<F>(app: &mut super::super::App, candidate: Candidate,
    hud_start: HudStart, finish_current: bool, interactive: bool, start: F) -> PlayOutcome
where F: FnOnce(&mut super::super::App, QueueRow, Candidate, HudStart, bool) -> PlayOutcome
{
    let Some(row) = target(&app.player.session, candidate) else { return PlayOutcome::Refused; };
    let source = (plx_media::route::playback_identity(&app.player.session),
        plx_media::route::cur_sid(&app.player.session), plx_media::route::cur_rk(&app.player.session));
    let allowed = if interactive {
        super::super::ott_runtime::foreign_ui_play_allowed(app, row.sid, &row.rk)
    } else {
        super::super::ott_runtime::foreign_play_allowed(app, row.sid, &row.rk)
    };
    if !allowed { return PlayOutcome::Refused; }
    if source != (plx_media::route::playback_identity(&app.player.session),
        plx_media::route::cur_sid(&app.player.session), plx_media::route::cur_rk(&app.player.session))
        || !target(&app.player.session, candidate).as_ref().is_some_and(|current| row_key(current) == row_key(&row))
    { return PlayOutcome::Refused; }
    start(app, row, candidate, hud_start, finish_current)
}

fn start(app: &mut super::super::App, row: QueueRow, candidate: Candidate,
    hud_start: HudStart, finish_current: bool) -> PlayOutcome
{
    // No suspension exists between this source/PIN admission and the outgoing
    // stop/resolve request. The eventual native Load separately rechecks its
    // retained target at the App boundary. A refused target leaves this player intact.
    let ps = &mut app.player.session;
    let pa = &mut app.adapters.player;
    let pages = &mut app.pages;
    let bridge = &mut app.bridge;
    if finish_current { ps.finished = true; }
    match candidate {
        Candidate::Neighbor(direction) => log(&format!("queue transport: {direction:?} rk={}", row.rk)),
        Candidate::UpNext => log(&format!("up next: S{}E{} rk={}", row.season, row.index, row.rk)),
    }
    // The row is owned before the outgoing queue or engine can be retired. Its server identity
    // follows it across the handoff, independent of the server being browsed behind the player.
    let resume = plx_data::metadata::resume_ns(row.resume_ms, row.dur_ms);
    let context = if row.kind == "episode" {
        plx_ui::fmt::episode_kicker(row.season, row.index, &row.title)
    } else {
        String::new()
    };
    close_player_overlays(pages);
    plx_media::player::stop_bufferfeed(ps, pa);
    let requested = plx_media::route::request_play_queue_item(ps, bridge.metadata_mut(), start_caps(), &row, &context);
    bridge.start_store_jobs();
    if !requested { return PlayOutcome::Failed; }
    bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::RetirePlaying);
    bridge.metadata_run(plx_data::stores::metadata::MetadataCmd::RequestDetail {
            sid: row.sid,
            rk: row.rk,
        });
    start_playback(
        ps,
        pa,
        resume,
        Origin::Unchanged,
        hud_start,
        None,
        pages,
        bridge,
    );
    PlayOutcome::Started
}
