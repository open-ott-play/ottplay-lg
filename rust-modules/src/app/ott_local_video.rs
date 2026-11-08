//! Existing resolver-worker Local input. Retain the authorized root before work,
//! open once beneath it, then carry that same file through probe and playback.
use super::*;
use plx_base::task::OffFrame;

pub(in crate::app::ott_runtime) fn capture_video_root(
    runtime: &Runtime,
    request: &PlaybackRequest,
    profile: &ProviderProfile,
) -> Result<Option<LocalSourceRead>, &'static str> {
    if profile.kind != ProviderKind::Local || plx_data::ott::http_url(&profile.source).is_some() {
        // Server library shares the Local provider kind but has HTTP authority,
        // not a device-folder root. Keep its existing resolver path unchanged.
        return Ok(None);
    }
    let read = if let Some(candidate) = runtime.transient.local_candidate()
        .filter(|candidate| request.transient_owner == Some(candidate.owner))
    {
        let CandidatePhase::Scanning { source, profile: staged, proposed: Some(_), .. } = &candidate.phase
            else { return Err("Local source is not ready"); };
        if staged != profile { return Err("Local source profile changed"); }
        capture_source_read(&candidate.inventory, &runtime.saved.profiles, runtime.generation, source)
            .filter(|read| request.local_selection.as_ref() == Some(&read.selection))
    } else {
        let selected = request.local_selection.clone()
            .or_else(|| runtime.local_profile_selection(profile))
            .ok_or("Local source is unavailable or ambiguous")?;
        let source = runtime.current_local_source(&selected).ok_or("Local source was withdrawn")?;
        if source.canonical_path.as_deref() != Some(profile.source.as_str()) {
            return Err("Local source profile changed");
        }
        runtime.capture_local_source(&selected)
    };
    read.map(Some).ok_or("Local source changed before playback")
}

pub(in crate::app::ott_runtime) fn open_video_source(
    worker: &OffFrame,
    read: Option<&LocalSourceRead>,
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    cancel: &AtomicBool,
) -> Result<plx_media::ff::ExternalSource, &'static str> {
    profile.validate().map_err(|_| "Invalid Local profile")?;
    if profile.kind != ProviderKind::Local { return Err("Invalid Local profile"); }
    let MediaLocator::Local { relative_path } = &entry.media else { return Err("Not a Local file"); };
    let read = read.ok_or("Local source authority is unavailable")?;
    if read.source.descriptor().canonical_path.as_deref() != Some(profile.source.as_str()) {
        return Err("Local source profile changed");
    }
    let mut current = || !cancel.load(Ordering::Acquire) && read.is_live();
    if !current() { return Err("Local playback was cancelled"); }
    if read.source.descriptor().kind == local::SourceKind::File {
        if read.source.descriptor().single_file_name() != Some(relative_path.as_str()) {
            return Err("Local file selection changed");
        }
        let held = read.source.single_file(relative_path).ok_or("Local file was withdrawn")?;
        if !current() { return Err("Local playback was cancelled"); }
        return Ok(plx_media::ff::ExternalSource::HeldLocal(held));
    }
    let folder = read.folder_read().ok_or("Local folder authority is unavailable")?;
    let root = folder.root.file_if_live(worker).ok_or("Local source was withdrawn")?;
    let file = plx_platform::storage::readonly::open_regular(root, relative_path, &mut current)
        .map_err(|_| "Could not open authorized Local file")?;
    if !current() { return Err("Local playback was cancelled"); }
    let held = plx_media::ff::HeldLocalFile::from_authorized(worker, file,
        std::path::Path::new(&profile.source).join(relative_path), folder.root.withdrawal_fence())
        .map_err(|_| "Could not retain Local file")?;
    if !current() { return Err("Local playback was cancelled"); }
    Ok(plx_media::ff::ExternalSource::HeldLocal(held))
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_local_video_tests.rs"]
mod tests;
