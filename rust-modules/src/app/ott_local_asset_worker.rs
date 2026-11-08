//! IO body for the existing metadata worker. No worker or cache is owned here.
use super::*;
use plx_base::task::OffFrame;
use plx_platform::storage::readonly::{self, EntryKind, ReadError};

// Optional immediate-directory work is bounded independently of library size.
const MAX_LOCAL_ASSET_DIRECTORY_ENTRIES: usize = 4096;
const MAX_LOCAL_ASSET_NAME_BYTES: usize = 256 * 1024;
const LOCAL_METADATA_PATH: &str = ".ottplay/metadata.json";

fn format(value: asset_selection::SubtitleFormat) -> sidecar::SidecarFormat {
    match value {
        asset_selection::SubtitleFormat::Srt => sidecar::SidecarFormat::Srt,
        asset_selection::SubtitleFormat::Vtt => sidecar::SidecarFormat::Vtt,
        asset_selection::SubtitleFormat::Ass => sidecar::SidecarFormat::Ass,
        asset_selection::SubtitleFormat::Ssa => sidecar::SidecarFormat::Ssa,
    }
}

fn display_text(value: &str, limit: usize) -> String {
    let value = value.trim();
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) { end -= 1; }
    value[..end].to_owned()
}

fn metadata(
    root: &std::fs::File,
    entry: &CatalogEntry,
    check: &mut dyn FnMut() -> bool,
) -> Result<(CatalogEntry, Option<String>), ReadError> {
    let bytes = match readonly::read_bounded(root, LOCAL_METADATA_PATH,
        plx_data::ott::MAX_CATALOG_BYTES, check)
    {
        Ok(bytes) => bytes,
        Err(ReadError::Aborted) => return Err(ReadError::Aborted),
        // Optional metadata never makes the underlying video unavailable.
        Err(_) => return Ok((entry.clone(), None)),
    };
    let Ok(map) = plx_data::ott::local_metadata::LocalMetadataMap::parse(&bytes) else {
        return Ok((entry.clone(), None));
    };
    if !check() { return Err(ReadError::Aborted); }
    let mut entry = entry.clone();
    let poster = map.apply_to(&mut entry).and_then(|row| row.poster_relative_path.clone());
    Ok((entry, poster))
}

fn discover(
    context: &Context,
    worker: &OffFrame,
    cancel: &AtomicBool,
    capacity: usize,
    subtitles: bool,
) -> Result<Discovered, ReadError> {
    let root = context.read.root.file_if_live(worker).ok_or(ReadError::Aborted)?;
    let mut check = || !cancel.load(Ordering::Acquire) && context.read.root.is_live();
    let (entry, poster) = metadata(root, &context.entry, &mut check)?;
    if !subtitles {
        return Ok(Discovered { entry, poster, subtitles: Vec::new(), complete: true });
    }
    let path = context.path();
    let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
    let observation = match readonly::list_bounded(root, parent,
        MAX_LOCAL_ASSET_DIRECTORY_ENTRIES, MAX_LOCAL_ASSET_NAME_BYTES, &mut check)
    {
        Ok(list) => list,
        Err(ReadError::Aborted) => return Err(ReadError::Aborted),
        Err(_) => return Ok(Discovered { entry, poster, subtitles: Vec::new(), complete: false }),
    };
    let mut path_bytes = 0usize;
    let mut complete = observation.complete;
    let paths: Vec<_> = observation.entries.iter().map_while(|row| {
        let len = parent.len().saturating_add(usize::from(!parent.is_empty())).saturating_add(row.name.len());
        if len > MAX_LOCAL_ASSET_NAME_BYTES.saturating_sub(path_bytes) {
            complete = false;
            return None;
        }
        path_bytes += len;
        Some(if parent.is_empty() { row.name.clone() } else { format!("{parent}/{}", row.name) })
    }).collect();
    let rows: Vec<_> = observation.entries.iter().zip(&paths).map(|(row, path)| {
        asset_selection::ObservedEntry {
            relative_path: path,
            kind: match row.kind {
                EntryKind::RegularFile => asset_selection::ObservedKind::RegularFile,
                EntryKind::Directory => asset_selection::ObservedKind::Directory,
                EntryKind::Symlink => asset_selection::ObservedKind::Symlink,
                EntryKind::Other => asset_selection::ObservedKind::Other,
                EntryKind::Unreadable => asset_selection::ObservedKind::Unreadable,
            },
        }
    }).collect();
    let selected = asset_selection::select(asset_selection::SelectInput {
        media_relative_path: path,
        directory: asset_selection::DirectoryObservation {
            parent_relative_path: parent, entries: &rows, complete,
        },
        remaining_subtitle_capacity: capacity,
    }).map_err(|_| ReadError::InvalidPath)?;
    if !check() { return Err(ReadError::Aborted); }
    let subtitles = selected.subtitles.iter().map(|row| DiscoveredSubtitle {
        path: row.relative_path.to_owned(),
        format: format(row.format),
        title: display_text(row.relative_path.rsplit('/').next().unwrap_or(row.relative_path),
            wire::MAX_LABEL_BYTES),
        // Display truncation never alters the private path identity.
        language: row.language_suffix.filter(|s| s.len() <= wire::MAX_METADATA_BYTES)
            .unwrap_or("").to_owned(),
    }).collect();
    Ok(Discovered { entry, poster, subtitles, complete: selected.complete })
}

impl Work {
    pub(in crate::app) fn run(self, worker: &OffFrame, cancel: &AtomicBool) -> Completion {
        let result = match self.kind {
            WorkKind::Discover { capacity } => Outcome::Discover(discover(&self.context, worker, cancel, capacity, true)),
            WorkKind::Preview => Outcome::Preview(discover(&self.context, worker, cancel, 0, false)),
            WorkKind::Read { request, path } => {
                let mut check = || !cancel.load(Ordering::Acquire) && self.context.read.root.is_live()
                    && sidecar::local_request_is_current(&request);
                let result = self.context.read.root.file_if_live(worker)
                    .ok_or(ReadError::Aborted)
                    .and_then(|root| readonly::read_bounded(root, &path, sidecar::owned_byte_limit(), &mut check));
                let result = match result {
                    Ok(bytes) if check() => request.prepare_owned(worker, bytes),
                    _ => Err(request.read_failed(worker)),
                };
                Outcome::Read { request, result }
            }
        };
        Completion { context: self.context, result }
    }
}
