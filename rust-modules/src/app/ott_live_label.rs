//! Pure presentation facts for an already matched, presented OTT playback.
//! The caller owns native identity/presentation and source capability checks.
use super::{CatalogEntry, OwnerToken, Playing, ProviderProfile};

pub(super) struct Label<'a> {
    pub label: &'a str,
    pub ordinal: Option<u32>,
}

/// The current underlying source catalog, never a filtered screen or Favorites order.
/// `source_current` is the caller's existing source-capability check: an unavailable
/// or revoked Local/attachment owner must not lend its catalog number to playback.
pub(super) struct CatalogSource<'a> {
    pub profile: &'a ProviderProfile,
    pub entries: &'a [CatalogEntry],
    pub transient_owner: Option<OwnerToken>,
    pub source_current: bool,
}

/// Does not decide whether a native session is actually presented. Call only after
/// the existing playing_matches and presentation gates. The label is always the
/// admitted entry's unformatted title; a mismatched catalog only removes its number.
pub(super) fn project<'a>(playing: &'a Playing, catalog: Option<CatalogSource<'_>>) -> Label<'a> {
    Label {
        label: &playing.entry.title,
        ordinal: catalog.and_then(|catalog| ordinal(playing, catalog)),
    }
}

fn ordinal(playing: &Playing, catalog: CatalogSource<'_>) -> Option<u32> {
    if !catalog.source_current
        || !catalog.profile.enabled
        || catalog.profile != &playing.profile
        || catalog.transient_owner != playing.transient_owner
    {
        return None;
    }
    let mut matching = catalog.entries.iter().enumerate()
        .filter(|(_, entry)| entry.id == playing.entry.id);
    let (index, entry) = matching.next()?;
    // Reused or ambiguous IDs cannot become a channel identity merely because one
    // of their rows happens to have the same title or source URL.
    if matching.next().is_some()
        || entry.media != playing.entry.media
        || entry.live != playing.entry.live
    {
        return None;
    }
    one_based(index)
}

fn one_based(index: usize) -> Option<u32> {
    u32::try_from(index).ok()?.checked_add(1)
}

#[cfg(test)]
#[path = "ott_live_label_tests.rs"]
mod tests;
