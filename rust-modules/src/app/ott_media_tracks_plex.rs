//! Passive projection of the installed Plex leaf. The shared route owns every
//! choice and temporary effect; the wire never receives a Plex ID or media path.
use super::*;

pub(super) fn capture(app: &App) -> Result<route::PlexTracks, wire::Error> {
    let view = app.bridge.metadata_view();
    let item = view.playing().ok_or(wire::Error::CatalogIncomplete)?;
    let tracks = route::PlexTracks::capture(&app.player.session, item)
        .ok_or(wire::Error::CatalogIncomplete)?;
    let audio = tracks.rows().iter().filter(|row| row.kind == TrackKind::Audio).count();
    let subtitles = tracks.rows().iter().filter(|row| row.kind == TrackKind::Subtitle).count();
    if tracks.rows().len() > wire::MAX_TRACKS
        || audio > wire::MAX_TRACKS_PER_KIND
        || subtitles > wire::MAX_TRACKS_PER_KIND
    {
        return Err(wire::Error::CatalogIncomplete);
    }
    Ok(tracks)
}

pub(super) fn inventory(
    tracks: &route::PlexTracks,
    mutable: bool,
    token: &str,
) -> wire::Inventory<NativeChoice> {
    wire::Inventory::Complete {
        tracks: tracks.rows().iter().enumerate().map(|(index, row)| wire::Track {
            // Catalog-scoped row ordinal only; it is never interpreted at apply.
            id: format!("{token}_p{index}"),
            kind: match row.kind {
                TrackKind::Audio => wire::Kind::Audio,
                TrackKind::Subtitle => wire::Kind::Subtitle,
            },
            label: row.label.clone(),
            language: Some(row.language.clone()),
            codec: Some(row.codec.clone()),
            choice: mutable.then(|| row.choice.clone()).flatten().map(NativeChoice::Plex),
        }).collect(),
        subtitle_off: mutable.then(|| tracks.subtitle_off()).flatten().map(NativeChoice::Plex),
    }
}

pub(super) fn apply_error(error: route::PlexTrackApplyError) -> wire::Error {
    match error {
        route::PlexTrackApplyError::Inactive => wire::Error::NoPlayback,
        route::PlexTrackApplyError::Busy => wire::Error::Busy,
        route::PlexTrackApplyError::Stale => wire::Error::CatalogChanged,
        route::PlexTrackApplyError::NotSelectable => wire::Error::NotSelectable,
        route::PlexTrackApplyError::Expired => wire::Error::Unavailable,
    }
}
