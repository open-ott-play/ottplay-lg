//! Pure presentation of already admitted catalogue facts. No provider fetch,
//! playback inspection or second catalogue is owned here.
use super::{CatalogEntry, Favorite, MediaLocator, ProviderKind, ProviderProfile, Runtime};
use plx_data::ott::favorites::Locator;
use plx_screens::ott::{OttCatalogProgress, OttMedia, OttMediaKind, OttMediaProgress};

/// Common baseline for app-owned catalogue, Guide and USB rows.
pub(super) fn row(id: String, title: String, detail: String,
    action: plx_screens::ott::OttAction, favorite: bool) -> plx_screens::ott::OttRow
{
    plx_screens::ott::OttRow { current_programme: None, content_title: None, guide: None,
        guide_available: false, records_available: false, media: None, id, title, detail, action,
        description: String::new(), poster: None, enabled: true, favorite,
    }
}

/// One catalogue publication retains one effective source. Records supports
/// M3U only; other providers must not repeatedly validate their source per row.
/// The callback keeps the full M3U validator at the app-owned query boundary.
pub(super) struct CatalogContext<'a> {
    profile: Option<&'a ProviderProfile>,
    records_profile: Option<&'a ProviderProfile>,
}

impl<'a> CatalogContext<'a> {
    pub(super) fn new(profile: Option<&'a ProviderProfile>) -> Self {
        Self { profile, records_profile: profile.filter(|profile| profile.kind == ProviderKind::M3u) }
    }

    pub(super) fn row(&self, index: usize, entry: &CatalogEntry, favorite: bool,
        records_available: impl FnOnce(&ProviderProfile, &CatalogEntry) -> bool) -> plx_screens::ott::OttRow
    {
        let mut item = row(entry.id.clone(), format!("{}  {}", index + 1, entry.title),
            entry.category.clone(), plx_screens::ott::OttAction::Play(entry.id.clone()), favorite);
        item.guide_available = self.profile.is_some_and(|profile| super::guide_ui::is_live(profile, entry));
        item.records_available = self.records_profile.is_some_and(|profile| records_available(profile, entry));
        item
    }
}

fn kind(locator: &MediaLocator) -> Option<OttMediaKind> {
    Some(match locator {
        MediaLocator::StalkerApi { .. } => OttMediaKind::LiveChannel,
        MediaLocator::Local { .. } | MediaLocator::ServerLibrary { .. } => OttMediaKind::LocalVideo,
        // The source parser admitted a playable leaf, but these locators retain
        // no reliable movie/episode/live discriminator. A filename is not one.
        MediaLocator::Http { .. } | MediaLocator::Plex { .. } | MediaLocator::VPortal { .. } => OttMediaKind::Video,
        MediaLocator::Xtream { stream_kind, series_id, .. } => match stream_kind.as_str() {
            "live" => OttMediaKind::LiveChannel,
            "movie" => OttMediaKind::Movie,
            "series" if series_id.as_ref().is_some_and(|id| !id.is_empty()) => OttMediaKind::Episode,
            "series" => OttMediaKind::Video,
            _ => return None,
        },
        MediaLocator::Stalker { stream_kind, series, .. } => match stream_kind.as_str() {
            "itv" => OttMediaKind::LiveChannel,
            "vod" if series.parse::<u64>().is_ok_and(|number| number > 0) => OttMediaKind::Episode,
            // VOD includes flattened series leaves whose hierarchy may be lost.
            "vod" => OttMediaKind::Video,
            _ => return None,
        },
    })
}

fn facts(kind: OttMediaKind, duration: Option<u64>, id: &str, resume: Option<(&str, u64)>) -> OttMedia {
    // A live catalogue duration may describe a programme rather than the stream.
    let duration_ms = duration.filter(|duration| *duration > 0 && kind != OttMediaKind::LiveChannel);
    let progress = duration_ms.and_then(|duration_ms| {
        let (target, position_ms) = resume?;
        (target == id && position_ms > 0 && position_ms < duration_ms)
            .then_some(OttMediaProgress { position_ms, duration_ms })
    });
    OttMedia { kind, duration_ms, progress, synopsis: None }
}

pub(super) fn entry(entry: &CatalogEntry, id: &str, resume: Option<(&str, u64)>) -> Option<OttMedia> {
    let mut media = facts(kind(&entry.media)?, entry.duration_ms, id, resume);
    media.synopsis = entry.synopsis.as_ref().and_then(|span| span.validated_for(&entry.description));
    Some(media)
}

pub(super) fn favorite(favorite: &Favorite, preview: Option<&CatalogEntry>, id: &str,
    resume: Option<(&str, u64)>) -> OttMedia
{
    // A portable favorite is a playable identity, not a cached provider row.
    // In particular `channel` can refer to a finite M3U video too.
    let fallback = match &favorite.locator {
        Locator::Local { .. } => OttMediaKind::LocalVideo,
        _ => OttMediaKind::Video,
    };
    let mut media = facts(preview.and_then(|entry| kind(&entry.media)).unwrap_or(fallback),
        preview.and_then(|entry| entry.duration_ms), id, resume);
    media.synopsis = preview.and_then(|entry| entry.synopsis.as_ref()
        .and_then(|span| span.validated_for(&entry.description)));
    media
}

pub(super) fn catalog(page: &str, loaded: usize, loading: bool, complete: bool) -> Option<OttCatalogProgress> {
    let (loading, complete) = match page {
        // Favorites is the client's complete list, independent of provider IO.
        "favorites" => (false, true),
        "channels" => (loading, complete),
        _ => return None,
    };
    Some(OttCatalogProgress { loaded, total: complete.then_some(loaded),
        loading: loading && !complete, complete })
}

impl Runtime {
    /// Resolve at most one existing checkpoint per snapshot, before the row map.
    /// A transient context never borrows the configured provider's durable one.
    pub(super) fn ui_media_resume(&self) -> Option<(&str, u64)> {
        if !matches!(self.page, "channels" | "favorites") { return None; }
        if self.transient.accepted.is_some() {
            let playing = self.playing.as_ref()?;
            return super::transient::bookmark_position(self, playing)
                .map(|position| (playing.selection_id.as_str(), position));
        }
        let key = if self.page == "favorites" { "favorites" } else { self.active()?.id.as_str() };
        let value = self.checkpoints.get(key).or_else(|| self.saved.resume.get(key))?;
        self.resume_is_current(value).then_some((value.entry_id.as_str(), value.position_ms))
    }
}

#[cfg(test)]
mod guide_compat_tests {
    use super::*;

    #[test]
    fn api_live_channel_has_no_finite_duration_or_resume_bar() {
        let media = MediaLocator::StalkerApi { channel_id: "7".into() };
        assert!(matches!(kind(&media), Some(OttMediaKind::LiveChannel)));
        let projected = facts(OttMediaKind::LiveChannel, Some(60_000), "api:7", Some(("api:7", 5_000)));
        assert!(projected.duration_ms.is_none());
        assert!(projected.progress.is_none());
    }
}
