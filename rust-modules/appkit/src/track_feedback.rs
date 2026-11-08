//! Addressed commit feedback for the existing track form. No IO or global polling.
use super::*;

#[derive(Clone, Debug)]
pub enum TrackCommitSource {
    External(plx_media::route::ExternalTracks),
    Hls(plx_media::route::HlsTrackCatalog),
    Plex { server: plx_plex::plex::ServerId, rating_key: String, session: String },
    /// A menu can still be drawn in a fixture or while playback is disappearing.
    /// This identity can never authorize a write.
    Inactive,
}
impl PartialEq for TrackCommitSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::External(a), Self::External(b)) => a.same_session(b),
            (Self::Hls(a), Self::Hls(b)) => a.same_session(b),
            (Self::Plex { server: a, rating_key: ar, session: as_ },
             Self::Plex { server: b, rating_key: br, session: bs }) => a == b && ar == br && as_ == bs,
            (Self::Inactive, Self::Inactive) => true,
            _ => false,
        }
    }
}
impl Eq for TrackCommitSource {}
impl TrackCommitSource {
    pub fn capture(ps: &plx_media::route::PlaybackSession) -> Self {
        if let Some(tracks) = plx_media::route::hls_track_catalog(ps) {
            return Self::Hls(tracks);
        }
        if let Some(tracks) = plx_media::route::external_tracks(ps) {
            return Self::External(tracks.clone());
        }
        let server = plx_media::route::cur_sid(ps);
        let rating_key = plx_media::route::cur_rk(ps);
        let session = plx_media::route::sess(ps);
        if !plx_media::route::has_url(ps) || !server.is_set() || rating_key.is_empty() || session.is_empty() {
            Self::Inactive
        } else {
            Self::Plex { server, rating_key, session }
        }
    }
    pub fn matches(&self, ps: &plx_media::route::PlaybackSession) -> bool {
        !matches!(self, Self::Inactive) && *self == Self::capture(ps)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackCommitFailure { TemporarySelection, Busy, StalePlayback, Unavailable, SaveFailed, SaveUncertain }
impl TrackCommitFailure {
    pub fn retryable(self) -> bool { matches!(self, Self::Busy | Self::SaveFailed) }
    fn label(self) -> &'static str {
        use plx_platform::i18n::msg;
        match self {
            Self::TemporarySelection => "Choose the temporary tracks manually or start a new playback before changing the server route.",
            Self::Busy => msg::widgets_tracks_commit_busy(),
            Self::StalePlayback => msg::widgets_tracks_commit_stale(),
            Self::Unavailable => msg::widgets_tracks_commit_unavailable(),
            Self::SaveFailed => msg::widgets_tracks_commit_save_failed(),
            Self::SaveUncertain => msg::widgets_tracks_commit_save_uncertain(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackCommitOutcome {
    /// Accepted by the route owner; does not claim physical decoder success.
    Admitted,
    Unchanged,
    /// Returned only after verified persistence and publication of accepted settings.
    Saved,
    Rejected(TrackCommitFailure),
}
/// A UI-only rollback point for legacy Plex controls that optimistically mutate on OK.
/// Native controls already follow published snapshots and must never be rolled back here.
#[derive(Clone)]
struct PlexCheckpoint {
    active_audio: c_int,
    active_sub: c_int,
    forced_only: bool,
    forced_lang: String,
    tone: SubtitleTone,
    size: SubtitleSize,
    position: SubtitlePosition,
    enhancement: Option<plx_plex::plex::AudioEnhancements>,
}
impl PlexCheckpoint {
    fn write(&self, c: &mut Canon) {
        c.u32(self.active_audio as u32).u32(self.active_sub as u32).bool(self.forced_only)
            .str(&self.forced_lang).u32(self.tone as u32).u32(self.size as u32).u32(self.position as u32)
            .bool(self.enhancement.is_some());
        if let Some(v) = self.enhancement { c.bool(v.boost_dialog).bool(v.normalize_loudness); }
    }
}
struct PlexEdit { before: PlexCheckpoint, after: PlexCheckpoint }
#[derive(Default)]
pub(super) struct CommitFeedback {
    failure: Option<TrackCommitFailure>,
    retry: Option<(TrackCommit, bool)>,
    uncertain: bool,
    rollback: Option<PlexEdit>,
}
impl TrackMenuState {
    pub fn commit_feedback_blocks_writes(&self) -> bool { self.commit_feedback.uncertain }

    pub fn show_commit_failure(&mut self, why: TrackCommitFailure, commit: TrackCommit, keep_open: bool) {
        self.reject_addressed_checkpoint(why.retryable());
        self.commit_feedback.uncertain |= why == TrackCommitFailure::SaveUncertain;
        self.commit_feedback.failure = Some(why);
        self.commit_feedback.retry = (why.retryable() && !self.commit_feedback.uncertain)
            .then_some((commit, keep_open));
        if self.pages.top() == Some(TrackPage::CommitFailure) {
            self.form.refresh(self.commit_failure_form());
        } else {
            self.push(TrackPage::CommitFailure);
        }
    }
    pub fn clear_commit_failure(&mut self, ps: &plx_media::route::PlaybackSession, meta: metadata::MetadataView<'_>) {
        if let Some(edit) = self.commit_feedback.rollback.take() { self.restore_plex_ui(edit.after); }
        if self.pages.top() == Some(TrackPage::CommitFailure) { self.pop(ps, meta); }
        self.commit_feedback.failure = None;
        self.commit_feedback.retry = None;
        // An uncertain write is only cleared by the app's reconciliation/restart,
        // never by navigating away from this error page or another successful edit.
    }
    pub(super) fn retry_commit(&self) -> TrackOk {
        if self.commit_feedback.uncertain { return TrackOk::Inert; }
        self.commit_feedback.retry.as_ref().map_or(TrackOk::Inert, |(commit, keep_open)|
            TrackOk::Commit { commit: commit.clone(), keep_open: *keep_open })
    }
    pub(super) fn commit_failure_form(&self) -> TrackForm {
        let mut section = FormSection::new("");
        if let Some(why) = self.commit_feedback.failure { section = section.note(why.label()); }
        if self.commit_feedback.retry.is_some() && !self.commit_feedback.uncertain {
            section = section.item(TrackRow::CommitRetry, RowKind::Button, (),
                Row::new(plx_platform::i18n::msg::widgets_tracks_commit_retry()));
        }
        Form::new().section(section)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inactive_source_never_authorizes_a_commit() {
        assert!(!TrackCommitSource::Inactive.matches(&plx_media::route::PlaybackSession::IDLE));
    }
    #[test]
    fn external_source_identity_is_catalog_lifetime_not_selection_or_url() {
        let make = || plx_media::route::ExternalTracks::new(Default::default(), vec![], vec![], None).unwrap();
        let catalog = make();
        assert_eq!(TrackCommitSource::External(catalog.clone()), TrackCommitSource::External(catalog));
        assert_ne!(TrackCommitSource::External(make()), TrackCommitSource::External(make()));
    }
    #[test]
    fn failure_form_is_visible_retry_is_scoped_and_back_restores_the_picker() {
        let _serial = plx_base::testlock::serial();
        let ps = plx_media::route::PlaybackSession::IDLE;
        let meta = plx_data::stores::metadata::MetadataStore::default();
        let mut menu = TrackMenuState::new(&ps, meta.view(), 1, Vec::new());
        assert!(menu.focus_id(TrackRow::Style));
        menu.on_ok(meta.view());
        assert!(menu.focus_id(TrackRow::OpenField(StyleField::Color)));
        menu.on_ok(meta.view());
        let row = TrackRow::Choice(StyleField::Color, 1);
        assert!(menu.focus_id(row));
        let scroll = menu.form.table.scroll_pos();
        let before = menu.tone;
        let commit = TrackCommit::SubtitleTone(SubtitleTone::Silver);
        menu.show_commit_failure(TrackCommitFailure::SaveFailed, commit.clone(), true);
        assert_eq!(menu.pages.top(), Some(TrackPage::CommitFailure));
        assert_eq!(menu.form.selected_id(), Some(&TrackRow::CommitRetry));
        assert_eq!(menu.tone, before, "failure cannot claim the unsaved setting");
        assert_eq!(menu.on_ok(meta.view()), TrackOk::Commit { commit, keep_open: true });
        assert!(menu.pop(&ps, meta.view()));
        assert_eq!(menu.form.selected_id(), Some(&row));
        assert_eq!(menu.form.table.scroll_pos(), scroll);
    }
    fn checked_row(menu: &TrackMenuState, row: TrackRow) -> bool {
        let i = menu.form.index_of(&row).expect("row exists");
        menu.form.table.sections.iter().flat_map(|section| &section.rows).nth(i).expect("row exists").checked
    }
    #[test]
    fn actual_plex_size_pick_rejection_restores_checked_rung_and_allows_same_pick() {
        let _serial = plx_base::testlock::serial();
        let ps = plx_media::route::PlaybackSession::IDLE;
        let meta = plx_data::stores::metadata::MetadataStore::default();
        let mut menu = TrackMenuState::new(&ps, meta.view(), 1, Vec::new());
        menu.size = SubtitleSize::Medium;
        assert!(menu.focus_id(TrackRow::Style)); menu.on_ok_addressed(meta.view());
        assert!(menu.focus_id(TrackRow::OpenField(StyleField::Size))); menu.on_ok_addressed(meta.view());
        let medium = TrackRow::Choice(StyleField::Size, SubtitleSize::Medium.index() as usize);
        let large = TrackRow::Choice(StyleField::Size, SubtitleSize::Large.index() as usize);
        assert!(checked_row(&menu, medium));
        assert!(menu.focus_id(large));
        let TrackOk::Commit { commit, keep_open } = menu.on_ok_addressed(meta.view()) else { panic!("size commit") };
        assert_eq!(menu.size, SubtitleSize::Large, "real legacy optimistic pick occurred");
        menu.show_commit_failure(TrackCommitFailure::SaveFailed, commit.clone(), keep_open);
        assert!(menu.pop(&ps, meta.view()));
        assert_eq!(menu.size, SubtitleSize::Medium);
        assert!(checked_row(&menu, medium));
        assert!(!checked_row(&menu, large));
        assert_eq!(menu.on_ok_addressed(meta.view()), TrackOk::Commit { commit, keep_open });
        menu.clear_commit_failure(&ps, meta.view()); // accepted retry; discard its checkpoint
        assert_eq!(menu.size, SubtitleSize::Large);
        let huge = TrackRow::Choice(StyleField::Size, SubtitleSize::Huge.index() as usize);
        assert!(menu.focus_id(huge));
        let TrackOk::Commit { commit, keep_open } = menu.on_ok_addressed(meta.view()) else { panic!("next commit") };
        menu.show_commit_failure(TrackCommitFailure::Busy, commit, keep_open);
        assert!(menu.pop(&ps, meta.view()));
        assert_eq!(menu.size, SubtitleSize::Large, "next attempt captured its own newer UI baseline");
    }
    #[test]
    fn error_page_retry_waits_for_saved_then_applies_the_accepted_plex_ui_state() {
        let _serial = plx_base::testlock::serial();
        let ps = plx_media::route::PlaybackSession::IDLE;
        let meta = plx_data::stores::metadata::MetadataStore::default();
        let mut menu = TrackMenuState::new(&ps, meta.view(), 1, Vec::new());
        menu.size = SubtitleSize::Medium;
        assert!(menu.focus_id(TrackRow::Style)); menu.on_ok_addressed(meta.view());
        assert!(menu.focus_id(TrackRow::OpenField(StyleField::Size))); menu.on_ok_addressed(meta.view());
        let large = TrackRow::Choice(StyleField::Size, SubtitleSize::Large.index() as usize);
        assert!(menu.focus_id(large));
        let TrackOk::Commit { commit, keep_open } = menu.on_ok_addressed(meta.view()) else { panic!("size commit") };
        menu.show_commit_failure(TrackCommitFailure::SaveFailed, commit.clone(), keep_open);
        assert_eq!(menu.size, SubtitleSize::Medium);
        assert!(menu.focus_id(TrackRow::CommitRetry));
        assert_eq!(menu.on_ok_addressed(meta.view()), TrackOk::Commit { commit, keep_open });
        assert_eq!(menu.size, SubtitleSize::Medium, "Retry does not claim a saved value before its reply");
        menu.clear_commit_failure(&ps, meta.view()); // Saved outcome
        assert_eq!(menu.size, SubtitleSize::Large);
        assert!(checked_row(&menu, large));
        assert!(menu.focus_id(large));
        assert_eq!(menu.on_ok_addressed(meta.view()), TrackOk::Inert);
    }
    #[test]
    fn actual_plex_audio_pick_rejection_restores_active_track_and_allows_same_pick() {
        let _serial = plx_base::testlock::serial();
        let ps = plx_media::route::PlaybackSession::IDLE;
        let meta = super::super::tests::store_with_audio(vec![
            metadata::Stream { id: 10, index: 0, codec: "aac".into(), channels: 2, default: true, ..Default::default() },
            metadata::Stream { id: 20, index: 1, codec: "aac".into(), channels: 2, ..Default::default() },
        ]);
        let mut menu = TrackMenuState::new(&ps, meta.view(), 0, Vec::new());
        assert_eq!(menu.active_audio, 0);
        assert!(menu.focus_id(TrackRow::Audio(1)));
        let TrackOk::Commit { commit, keep_open } = menu.on_ok_addressed(meta.view()) else { panic!("audio commit") };
        assert_eq!(menu.active_audio, 1, "real optimistic pick occurred");
        menu.show_commit_failure(TrackCommitFailure::Busy, commit.clone(), keep_open);
        assert!(menu.pop(&ps, meta.view()));
        assert_eq!(menu.active_audio, 0);
        assert!(checked_row(&menu, TrackRow::Audio(0)));
        assert!(!checked_row(&menu, TrackRow::Audio(1)));
        assert_eq!(menu.on_ok_addressed(meta.view()), TrackOk::Commit { commit, keep_open });
    }
    #[test]
    fn uncertain_latch_changes_canon_after_the_error_page_is_gone() {
        let _serial = plx_base::testlock::serial();
        let ps = plx_media::route::PlaybackSession::IDLE;
        let meta = plx_data::stores::metadata::MetadataStore::default();
        let mut menu = TrackMenuState::new(&ps, meta.view(), 1, Vec::new());
        assert!(menu.focus_id(TrackRow::Style));
        let mut before = Canon::new(); menu.canon(&mut before);
        menu.show_commit_failure(TrackCommitFailure::SaveUncertain,
            TrackCommit::TextAppearance(TextAppearance::default()), true);
        menu.clear_commit_failure(&ps, meta.view());
        assert!(menu.commit_feedback_blocks_writes());
        let mut after = Canon::new(); menu.canon(&mut after);
        assert_ne!(before.finish(), after.finish());
    }
    #[test]
    fn uncertain_save_never_offers_retry_even_after_leaving_error_page() {
        let _serial = plx_base::testlock::serial();
        let ps = plx_media::route::PlaybackSession::IDLE;
        let meta = plx_data::stores::metadata::MetadataStore::default();
        let mut menu = TrackMenuState::new(&ps, meta.view(), 1, Vec::new());
        assert!(menu.focus_id(TrackRow::Style));
        menu.show_commit_failure(TrackCommitFailure::SaveUncertain,
            TrackCommit::TextAppearance(TextAppearance::default()), true);
        assert_eq!(menu.pages.top(), Some(TrackPage::CommitFailure));
        assert!(!menu.ids().contains(&TrackRow::CommitRetry));
        assert_eq!(menu.retry_commit(), TrackOk::Inert);
        assert!(menu.pop(&ps, meta.view()));
        assert!(menu.commit_feedback_blocks_writes());
        menu.show_commit_failure(TrackCommitFailure::SaveFailed,
            TrackCommit::TextAppearance(TextAppearance::default()), true);
        assert!(!menu.ids().contains(&TrackRow::CommitRetry));
    }
}

impl TrackCommit {
    /// Stable logical payload. Catalog identity is checked separately; no Arc address is encoded.
    pub fn write_feedback_canon(&self, c: &mut Canon) {
        match self {
            Self::Audio { carried: a, memo } => {
                c.u32(0).u64(a.sid as u64).u32(a.ordinal as u32).str(&a.codec)
                    .u64(a.channels as u64).bool(a.can_normalize_loudness).bool(a.immersive)
                    .u64(a.sample_rate as u64).u64(a.bit_depth as u64).str(&format!("{memo:?}"));
            }
            Self::AudioEnhancement(v) => { c.u32(1).bool(v.boost_dialog).bool(v.normalize_loudness); }
            Self::Subtitle { render_ordinal, stream_id, sidecar_key, sidecar_codec, memo } => {
                c.u32(2).u32(*render_ordinal as u32).u64(*stream_id as u64)
                    .bool(sidecar_key.is_some()).str(sidecar_key.as_deref().unwrap_or(""))
                    .str(sidecar_codec).str(&format!("{memo:?}"));
            }
            Self::SubtitleForcedOnly { render_ordinal, stream_id, lang, changed } => {
                c.u32(3).u32(*render_ordinal as u32).u64(*stream_id as u64).str(lang).bool(*changed);
            }
            Self::SubtitleTone(v) => { c.u32(4).u32(*v as u32); }
            Self::SubtitleSize(v) => { c.u32(5).u32(*v as u32); }
            Self::SubtitlePosition(v) => { c.u32(6).u32(*v as u32); }
            Self::TextAppearance(v) => { c.u32(7).u32(v.generation()); }
            Self::External(v) => {
                c.u32(8).u32(v.kind() as u32).bool(v.ordinal().is_some()).u32(v.ordinal().unwrap_or(-1) as u32);
            }
            Self::SubtitleOffset(v) => { c.u32(9).u64(*v as u64); }
            Self::Hls(v) => match v.target() {
                plx_media::route::HlsTrackTarget::Audio(id) => { c.u32(10).u32(id.opaque_key()); }
                plx_media::route::HlsTrackTarget::Subtitle(id) => {
                    c.u32(11).bool(id.is_some()).u32(id.map_or(0, |id| id.opaque_key()));
                }
            },
        }
    }
}

impl TrackMenuState {
    fn plex_checkpoint(&self) -> Option<PlexCheckpoint> {
        (self.hls.is_none() && self.external.is_none() && self.native_preferences.is_none()).then(|| PlexCheckpoint {
            active_audio: self.active_audio, active_sub: self.active_sub,
            forced_only: self.forced_only, forced_lang: self.forced_lang.clone(),
            tone: self.tone, size: self.size, position: self.position,
            enhancement: self.enhance_shown,
        })
    }
    /// Interactive submission retains before/after UI snapshots until the original owner answers.
    /// Trusted headless setup keeps using `on_ok` and its existing immediate UI behavior.
    pub fn on_ok_addressed(&mut self, meta: metadata::MetadataView<'_>) -> TrackOk {
        if self.commit_feedback_blocks_writes() { return TrackOk::Inert; }
        let before = self.plex_checkpoint();
        let retry_after = if self.pages.top() == Some(TrackPage::CommitFailure) {
            self.commit_feedback.rollback.as_ref().map(|edit| edit.after.clone())
        } else { None };
        let out = self.on_ok(meta);
        if matches!(out, TrackOk::Commit { .. }) {
            let after = retry_after.or_else(|| self.plex_checkpoint());
            self.commit_feedback.rollback = before.zip(after).map(|(before, after)| PlexEdit { before, after });
        }
        out
    }
    pub fn take_search_commit_addressed(&mut self) -> Option<(metadata::Stream, TrackCommit)> {
        if self.commit_feedback_blocks_writes() { return None; }
        let before = self.plex_checkpoint();
        let out = self.take_search_commit();
        if out.is_some() {
            self.commit_feedback.rollback = before.zip(self.plex_checkpoint()).map(|(before, after)| PlexEdit { before, after });
        }
        out
    }
    fn restore_plex_ui(&mut self, state: PlexCheckpoint) {
        self.active_audio = state.active_audio;
        self.active_sub = state.active_sub;
        self.forced_only = state.forced_only;
        self.forced_lang = state.forced_lang;
        self.tone = state.tone;
        self.size = state.size;
        self.position = state.position;
        self.enhance_shown = state.enhancement;
    }
    fn reject_addressed_checkpoint(&mut self, retry: bool) {
        if let Some(edit) = self.commit_feedback.rollback.take() {
            self.restore_plex_ui(edit.before.clone());
            if retry { self.commit_feedback.rollback = Some(edit); }
        }
    }
    pub fn discard_addressed_checkpoint(&mut self) { self.commit_feedback.rollback = None; }
    pub(super) fn write_addressed_checkpoint(&self, c: &mut Canon) {
        c.bool(self.commit_feedback.rollback.is_some());
        if let Some(edit) = &self.commit_feedback.rollback { edit.before.write(c); edit.after.write(c); }
    }
}

impl TrackMenuState {
    pub(super) fn commit_retry_row(&self) -> Option<TrackRow> {
        (self.commit_feedback.retry.is_some() && !self.commit_feedback.uncertain).then_some(TrackRow::CommitRetry)
    }
    pub(super) fn write_commit_feedback(&self, c: &mut Canon) {
        self.write_addressed_checkpoint(c);
        c.bool(self.commit_feedback.uncertain);
        c.u32(self.commit_feedback.failure.map_or(0, |failure| failure as u32 + 1));
        c.bool(self.commit_feedback.retry.is_some());
        if let Some((commit, keep_open)) = &self.commit_feedback.retry {
            c.bool(*keep_open);
            commit.write_feedback_canon(c);
        }
    }
}
