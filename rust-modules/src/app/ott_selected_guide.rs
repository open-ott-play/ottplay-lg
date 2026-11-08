//! One admitted selected-entry guide projection in the existing metadata owner.
//! No persistence, provider IO, speculative playback, or second catalogue.
use super::*;
use plx_data::ott::guide::{ChannelMetadata, Query};
use plx_screens::ott::OttNowNext;

pub(super) struct Candidate {
    metadata_generation: u64,
    target: (String, String),
    source_profile: ProviderProfile,
    entry_id: String,
    media: MediaLocator,
    live: Option<ChannelMetadata>,
    query: Query,
    facts: Option<OttNowNext>,
    failed: bool,
}

pub(super) struct Retained {
    source_generation: u64,
    candidate: Candidate,
}

impl Candidate {
    pub(super) fn from_reply(reply: &super::super::ott_metadata::Guide, now_s: i64) -> Self {
        Self {
            metadata_generation: reply.generation,
            target: (reply.profile_id.clone(), reply.entry_id.clone()),
            source_profile: reply.source_profile.clone(),
            entry_id: reply.entry.id.clone(),
            media: reply.entry.media.clone(),
            live: reply.entry.live.clone(),
            query: reply.request.query,
            facts: reply.result.as_ref().ok()
                .and_then(|result| guide_projection::current_next(result, now_s)),
            failed: reply.result.is_err(),
        }
    }

    fn matches_entry(&self, entry: &CatalogEntry) -> bool {
        self.entry_id == entry.id && self.media == entry.media && self.live == entry.live
    }

    fn source_current(&self, runtime: &Runtime) -> bool {
        runtime.prepare_play_request(PlaybackRequest::ordinary(&self.target.1, 0))
            .is_ok_and(|(_, profile, entry, _)| profile == self.source_profile
                && entry.as_ref().is_none_or(|entry| self.matches_entry(entry)))
    }

    /// A current error is Some(None); None refuses stale or unsupported facts.
    /// The caller must also check its dispatched LiveWitness and the actual
    /// native route. Browsing another source does not redefine this Playing.
    pub(super) fn playing_facts(
        &self,
        playing: &Playing,
        generation: u64,
        target: Option<&(String, String)>,
        query: Option<Query>,
    ) -> Option<Option<OttNowNext>> {
        if self.metadata_generation != generation
            || target != Some(&self.target)
            || query != Some(self.query)
            || !matches!(self.query.validate().ok()?, Query::CurrentNext { .. })
            || playing.identity.is_none()
            || playing.archive.is_some()
            || self.source_profile != playing.profile
            || self.target.0 != playing.profile.id
            || (self.target.1 != playing.entry.id && self.target.1 != playing.selection_id)
            || !self.matches_entry(&playing.entry)
            || !guide_ui::is_live(&playing.profile, &playing.entry)
            || (playing.profile.kind == ProviderKind::M3u
                && plx_data::ott::m3u_source_kind(&playing.profile.source)
                    != Ok(plx_data::ott::M3uSourceKind::Playlist))
        {
            return None;
        }
        match &self.facts {
            Some(facts) => Some(Some(facts.clone())),
            None if self.failed => Some(None),
            None => None,
        }
    }
}

impl Runtime {
    /// Called only after the existing metadata receiver's complete admission.
    /// A stale reply must neither enrich the catalogue nor clear newer facts.
    pub(super) fn retain_selected_guide(
        &mut self,
        candidate: Option<Candidate>,
        playing: Option<&Playing>,
    ) -> bool {
        if candidate.as_ref().is_some_and(|candidate| !candidate.source_current(self)
            && !playing.is_some_and(|playing| candidate.playing_facts(playing,
                self.metadata_generation, self.metadata_target.as_ref(), self.metadata_query).is_some())) {
            return false;
        }
        self.metadata_guide = candidate.filter(|candidate| candidate.facts.is_some() || candidate.failed)
            .map(|candidate| Retained { source_generation: self.generation, candidate });
        self.changed();
        true
    }

    /// Reuses the one admitted metadata result if it arrived before the first
    /// frame. A current failure remains an explicit clear until publication.
    /// The caller supplies a native-route-validated Playing and checks its
    /// proof-only LiveWitness before consuming these facts.
    pub(super) fn playing_row_guide(&self, playing: &Playing) -> Option<Option<OttNowNext>> {
        let retained = self.metadata_guide.as_ref()?;
        if !self.loaded || self.persistence_uncertain
            || self.metadata_cancel.load(Ordering::Acquire)
            || !self.metadata_attachment_current()
        {
            return None;
        }
        retained.candidate.playing_facts(playing, self.metadata_generation,
            self.metadata_target.as_ref(), self.metadata_query)
    }

    pub(super) fn selected_row_guide(&self, id: &str, entry: Option<&CatalogEntry>) -> Option<OttNowNext> {
        let retained = self.metadata_guide.as_ref()?;
        let candidate = &retained.candidate;
        if !self.loaded || self.persistence_uncertain
            || !matches!(self.page, "channels" | "favorites")
            || retained.source_generation != self.generation
            || candidate.metadata_generation != self.metadata_generation
            || self.metadata_target.as_ref() != Some(&candidate.target)
            || self.metadata_query != Some(candidate.query)
            || candidate.target.1 != id
            || self.metadata_cancel.load(Ordering::Acquire)
            || !self.metadata_attachment_current()
            || !entry.is_some_and(|entry| candidate.matches_entry(entry)
                && guide_ui::is_live(&candidate.source_profile, entry))
            || !candidate.source_current(self)
        { return None; }
        let now_s = i64::try_from(self.wall_clock_ms / 1000).ok()?;
        guide_projection::advance(candidate.facts.as_ref()?, now_s)
    }

    /// A non-active Favorite profile may change without a catalogue generation.
    /// Retire its actual in-flight metadata before publishing the new document.
    pub(super) fn retire_changed_metadata_source(&mut self, next: &Saved) {
        let Some((owner, selection)) = self.metadata_target.as_ref() else { return; };
        let old_profile = self.saved.profiles.profiles.iter().find(|profile| &profile.id == owner);
        let next_profile = next.profiles.profiles.iter().find(|profile| &profile.id == owner);
        let favorite_changed = selection.strip_prefix("favorite:").is_some_and(|identity| {
            self.saved.favorites.entries().iter().find(|favorite| favorite.identity() == identity)
                != next.favorites.entries().iter().find(|favorite| favorite.identity() == identity)
        });
        if old_profile == next_profile && !favorite_changed { return; }
        self.metadata_cancel.store(true, Ordering::Release);
        self.metadata_generation = self.metadata_generation.wrapping_add(1);
        self.metadata_target = None;
        self.metadata_query = None;
        self.metadata_next = None;
        self.metadata_refresh_ms = 0;
        self.metadata_playback_identity = None;
        self.metadata_local_selection = None;
        self.metadata_attachment = None;
        self.metadata_guide = None;
        self.favorite_preview = None;
        self.changed();
    }
}
