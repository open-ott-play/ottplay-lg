//! Per-channel Records is a projection of complete, bounded XMLTV replies.
//! It creates no recording service, request worker, clock or playback owner.
use super::{archive, epg::Programme, guide::{Coverage, Query, ResultSet},
    m3u_source_kind, CatalogEntry, CatalogError, M3uSourceKind, MediaLocator,
    ProviderKind, ProviderProfile, MAX_CATALOG_ENTRIES};
use std::collections::BTreeMap;

// Same admissible retention as the M3U parser and native profile fallback.
pub const MAX_RETENTION_SECONDS: u64 = 365 * 86400;

pub(super) fn valid_window(start: i64, end: i64) -> bool {
    start >= 0 && end > start && end.checked_sub(start)
        .is_some_and(|width| width as u64 <= MAX_RETENTION_SECONDS)
}

/// Only a supported, explicit M3U catch-up rule grants Records. A fallback
/// supplies omitted depth, never an omitted/invalid rule or another provider.
pub fn query(profile: &ProviderProfile, entry: &CatalogEntry, fallback_hours: u64, now: i64)
    -> Result<Query, CatalogError>
{
    profile.validate()?;
    if profile.kind != ProviderKind::M3u || !profile.enabled
        || m3u_source_kind(&profile.source) != Ok(M3uSourceKind::Playlist)
        || entry.duration_ms.is_some() || fallback_hours > 8760
    { return Err(CatalogError::InvalidProfile); }
    let MediaLocator::Http { url } = &entry.media else { return Err(CatalogError::InvalidFormat); };
    let mut source = entry.live.as_ref().and_then(|live| live.catchup.clone())
        .ok_or(CatalogError::InvalidFormat)?;
    if source.retention_seconds == 0 { source.retention_seconds = fallback_hours * 3600; }
    if source.retention_seconds == 0 || source.retention_seconds > MAX_RETENTION_SECONDS {
        return Err(CatalogError::InvalidFormat);
    }
    let start = now.checked_sub(source.retention_seconds as i64).ok_or(CatalogError::InvalidFormat)?;
    let query = Query::Records { start, end: now }.validate()?;
    // Validate the existing transport rule through the actual archive planner.
    // This is a local admissibility check, not a promise that a server stores EPG.
    archive::plan(&source, url, archive::ArchiveInterval {
        start: now.checked_sub(1).ok_or(CatalogError::InvalidFormat)?, end: now,
    }, now)?;
    Ok(query)
}

fn available(programme: &Programme, start: i64, end: i64) -> bool {
    programme.start > start && programme.end <= end && programme.end > programme.start
}

/// Original rows remain in ResultSet for source/generation/programme checks.
/// Equal starts choose the later source row, matching Classic's stable sort
/// followed by reverse traversal. Titles are exact, case-sensitive keys.
pub fn indices(result: &ResultSet) -> Result<Vec<usize>, CatalogError> {
    let Query::Records { start, end } = result.query.validate()? else {
        return Err(CatalogError::InvalidFormat);
    };
    if result.coverage != Coverage::RequestedWindow { return Err(CatalogError::InvalidFormat); }
    if result.programmes.len() > MAX_CATALOG_ENTRIES {
        return Err(CatalogError::LimitExceeded);
    }
    let mut latest: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, programme) in result.programmes.iter().enumerate() {
        if !available(programme, start, end) { continue; }
        let replace = latest.get(programme.title.as_str())
            .is_none_or(|&old| programme.start >= result.programmes[old].start);
        if replace { latest.insert(&programme.title, index); }
    }
    let mut indices: Vec<_> = latest.into_values().collect();
    indices.sort_by(|&a, &b| result.programmes[a].title.encode_utf16()
        .cmp(result.programmes[b].title.encode_utf16()));
    Ok(indices)
}

#[cfg(test)]
#[path = "records_tests.rs"]
mod tests;
