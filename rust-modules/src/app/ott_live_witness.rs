//! Proof for one actual-playing live metadata request, without retaining guide data.
//! Native route/current receiver admission remains the caller's responsibility.
use super::*;
use plx_data::ott::guide::{ChannelMetadata, Query};
use plx_media::route::PlaybackIdentity;

#[derive(Clone)]
pub(super) struct LiveWitness {
    identity: PlaybackIdentity,
    profile: ProviderProfile,
    entry_id: String,
    media: MediaLocator,
    live: Option<ChannelMetadata>,
    metadata_generation: u64,
    target_id: String,
    query: Query,
}

pub(super) fn live_source(profile: &ProviderProfile, entry: &CatalogEntry) -> bool {
    guide_ui::is_live(profile, entry)
        && (profile.kind != ProviderKind::M3u
            || plx_data::ott::m3u_source_kind(&profile.source)
                == Ok(plx_data::ott::M3uSourceKind::Playlist))
}

impl LiveWitness {
    /// Call only for the exact source submitted to the existing metadata worker.
    /// An ordinary Favorite can request its resolved entry id; neither that nor
    /// the outer Favorite id is sufficient without the exact profile and entry.
    pub(super) fn capture(
        playing: &Playing,
        request_profile: &ProviderProfile,
        request_entry: &CatalogEntry,
        target_id: &str,
        generation: u64,
        query: Query,
    ) -> Option<Self> {
        if playing.archive.is_some()
            || !live_source(request_profile, request_entry)
            || !matches!(query.validate().ok()?, Query::CurrentNext { .. })
            || request_profile != &playing.profile
            || request_entry.id != playing.entry.id
            || request_entry.media != playing.entry.media
            || request_entry.live != playing.entry.live
            || !live_source(&playing.profile, &playing.entry)
            || (target_id != playing.entry.id && target_id != playing.selection_id)
        {
            return None;
        }
        Some(Self {
            identity: playing.identity.clone()?,
            profile: request_profile.clone(),
            entry_id: request_entry.id.clone(),
            media: request_entry.media.clone(),
            live: request_entry.live.clone(),
            metadata_generation: generation,
            target_id: target_id.into(),
            query,
        })
    }

    pub(super) fn matches_source(&self, profile: &ProviderProfile, entry: &CatalogEntry) -> bool {
        profile == &self.profile
            && entry.id == self.entry_id
            && entry.media == self.media
            && entry.live == self.live
            && guide_ui::is_live(profile, entry)
    }

    /// This intentionally ignores the currently browsed catalogue/metadata
    /// generation. A different highlighted channel does not replace playback.
    pub(super) fn matches_playing(&self, playing: &Playing) -> bool {
        playing.archive.is_none()
            && playing.identity.as_ref() == Some(&self.identity)
            && self.matches_source(&playing.profile, &playing.entry)
            && (self.target_id == playing.entry.id || self.target_id == playing.selection_id)
    }

    /// All completion kinds, including an error without a resolved entry, need
    /// this exact dispatched request. Typed successes additionally compare their
    /// captured source via matches_source before publishing any programme facts.
    pub(super) fn accepts_completion(
        &self,
        playing: &Playing,
        generation: u64,
        profile_id: &str,
        target_id: &str,
        query: Query,
        dispatched_identity: Option<&PlaybackIdentity>,
    ) -> bool {
        self.matches_playing(playing)
            && dispatched_identity == Some(&self.identity)
            && generation == self.metadata_generation
            && profile_id == self.profile.id
            && target_id == self.target_id
            && query == self.query
    }
}
