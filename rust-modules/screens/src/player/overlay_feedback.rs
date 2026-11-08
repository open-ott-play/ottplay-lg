//! One pending, addressed track edit owned by the exact open overlay.
use super::*;
use crate::registry::TrackCommitRequest;
use plx_appkit::track_menu::{TrackCommit, TrackCommitOutcome, TrackCommitSource};
use plx_machine::machine::{Addr, MachineId, RequestId};

pub(super) struct PendingTrackCommit { request: TrackCommitRequest, keep_open: bool, focus: Option<plx_appkit::track_menu::TrackRow> }
impl PlayerOverlayScreen {
    pub fn accepts_track_commit(&self, request: &TrackCommitRequest) -> bool {
        !self.track_closed && self.pending_track_commit.as_ref()
            .is_some_and(|pending| pending.request == *request)
    }
    pub(super) fn request_track_commit<H: AppLike>(
        &mut self, commit: TrackCommit, keep_open: bool, fx: &mut Effects<'_, H>,
    ) {
        if self.track_closed || self.pending_track_commit.is_some() { return; }
        if matches!(&self.panel, Panel::Tracks(p) if p.commit_feedback_blocks_writes()) { return; }
        let from = fx.from();
        if !matches!(from, MachineId::Instance(_)) { return; }
        let Some(id) = self.next_track_request else { return };
        self.next_track_request = id.checked_add(1);
        let request = TrackCommitRequest {
            reply: Addr { to: from, req: RequestId(id) },
            source: self.track_source.clone(), commit,
        };
        let focus = match &self.panel { Panel::Tracks(p) => p.selected_id(), _ => None };
        self.pending_track_commit = Some(PendingTrackCommit { request: request.clone(), keep_open, focus });
        fx.push(Fx::App(AppFx::Player(PlayerReq::CommitTrackAddressed(request))));
        if self.kind.extends_hud() { self.moved(fx); }
    }
    pub(super) fn retire_track_commit(&mut self) {
        self.track_closed = true;
        self.pending_track_commit = None;
        if let Panel::Tracks(p) = &mut self.panel { p.discard_addressed_checkpoint(); }
    }
    pub(super) fn track_commit_reply<H: AppLike + crate::registry::MetadataLike>(
        &mut self, id: RequestId, outcome: TrackCommitOutcome,
        ps: &plx_media::route::PlaybackSession, cx: &Cx<'_, H>, fx: &mut Effects<'_, H>,
    ) {
        if self.track_closed || !self.pending_track_commit.as_ref()
            .is_some_and(|p| p.request.reply.req == id) { return; }
        if self.track_source != TrackCommitSource::capture(ps) {
            self.retire_track_commit();
            self.dismiss(fx);
            self.closing(fx);
            return;
        }
        let pending = self.pending_track_commit.take().expect("checked pending request");
        match outcome {
            TrackCommitOutcome::Rejected(why) => {
                if let Panel::Tracks(p) = &mut self.panel {
                    if let Some(focus) = pending.focus { p.focus_id(focus); }
                    p.show_commit_failure(why, pending.request.commit, pending.keep_open);
                    self.moved(fx);
                } else {
                    // Timing has no status form. Its offset was optimistic; retire it
                    // on rejection so it cannot keep showing an unapplied value.
                    self.retire_track_commit();
                    self.dismiss(fx);
                    self.closing(fx);
                }
            }
            TrackCommitOutcome::Admitted | TrackCommitOutcome::Unchanged | TrackCommitOutcome::Saved => {
                if let Panel::Tracks(p) = &mut self.panel { p.clear_commit_failure(ps, H::metadata(cx)); }
                if pending.keep_open { if self.kind.extends_hud() { self.moved(fx); } }
                else { self.retire_track_commit(); self.dismiss(fx); self.closing(fx); }
            }
        }
    }
}

impl PlayerOverlayScreen {
    pub(super) fn write_track_feedback(&self, c: &mut Canon) {
        c.bool(self.track_closed).bool(self.next_track_request.is_some()).u32(self.next_track_request.unwrap_or(0));
        c.bool(self.pending_track_commit.is_some());
        if let Some(p) = &self.pending_track_commit {
            c.u32(p.request.reply.req.0).bool(p.keep_open).bool(p.request.source == self.track_source);
            c.u32(p.focus.map_or(u32::MAX, |id| plx_ui::form::FormId::key(&id).0));
            p.request.commit.write_feedback_canon(c);
        }
    }
}
#[cfg(test)]
#[path = "overlay_feedback_tests.rs"]
mod tests;
