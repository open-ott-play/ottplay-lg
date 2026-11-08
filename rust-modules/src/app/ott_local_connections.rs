//! Off-frame validation for the single live Local inventory. Never writes state.
use crate::app::ott_local_root::{LocalSource, RootError};
use plx_base::task::OffFrame;
use crate::app::ott_local_session::{AuthorityRef, BoundSource, LocalError, SourceKind};
use crate::app::ott_local_sources::{FolderDescriptor, LocalSourceKind};
use plx_data::ott;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub(in crate::app) struct Request {
    pub known: Vec<LocalSource>,
    pub remembered: Vec<FolderDescriptor>,
    pub lookup: Option<String>,
    pub allow_new: bool,
    pub allow_reconnect: bool,
    /// Only startup or an explicit connect/play/shuffle may reacquire saved intent.
    /// Sources, including a path lookup, must keep this false.
    pub restore_remembered: bool,
    pub new_id: String,
    pub generation: u64,
}
pub(in crate::app) struct Validated {
    pub sources: Vec<LocalSource>,
    pub lookup: Option<String>,
    pub matched: Vec<BoundSource>,
    /// Observed source losses are retained independently of command success.
    /// On failure the runtime may apply revocations only, never new grants.
    pub operation_error: Option<LocalError>,
}

fn source_kind(kind: LocalSourceKind) -> SourceKind {
    match kind {
        LocalSourceKind::Folder => SourceKind::Folder,
        LocalSourceKind::File => SourceKind::File,
    }
}
fn saved_kind(kind: SourceKind) -> LocalSourceKind {
    match kind {
        SourceKind::Folder => LocalSourceKind::Folder,
        SourceKind::File => LocalSourceKind::File,
    }
}

/// Classification is an observation, never a live grant. The acquisition below
/// independently proves the canonical object before, during and after opening.
fn canonical(path: &str) -> Result<(String, SourceKind), LocalError> {
    if !Path::new(path).is_absolute() || path.len() > 4096 || path.chars().any(char::is_control) {
        return Err(LocalError::Unsupported);
    }
    let before = std::fs::symlink_metadata(path).map_err(|_| LocalError::SourceUnavailable)?;
    let kind = if before.is_dir() {
        SourceKind::Folder
    } else if before.is_file() {
        let name = basename(path)?;
        if !ott::is_local_video_path(&name) || !ott::valid_local_relative_path(&name) {
            return Err(LocalError::Unsupported);
        }
        SourceKind::File
    } else {
        // No symlink, FIFO, socket or device is ever promoted to a source.
        return Err(LocalError::Unsupported);
    };
    let resolved = std::fs::canonicalize(path).map_err(|_| LocalError::SourceUnavailable)?;
    let after = std::fs::symlink_metadata(&resolved).map_err(|_| LocalError::SourceUnavailable)?;
    if before.dev() != after.dev() || before.ino() != after.ino()
        || before.is_file() != after.is_file() || before.is_dir() != after.is_dir()
    {
        return Err(LocalError::SourceUnavailable);
    }
    let resolved = resolved.to_str().ok_or(LocalError::SourceUnavailable)?.to_owned();
    Ok((resolved, kind))
}
fn basename(path: &str) -> Result<String, LocalError> {
    let name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Local videos");
    if name.trim().is_empty()
        || name.len() > 512
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        return Err(LocalError::SourceUnavailable);
    }
    Ok(name.into())
}
fn cancelled(cancel: &AtomicBool) -> Result<(), LocalError> {
    if cancel.load(Ordering::Acquire) {
        Err(LocalError::RuntimeChanged)
    } else {
        Ok(())
    }
}
fn append(
    off: &OffFrame,
    cancel: &AtomicBool,
    sources: &mut Vec<LocalSource>,
    saved: FolderDescriptor,
    generation: u64,
    restore: bool,
) -> Result<(), LocalError> {
    saved.validate().map_err(|_| LocalError::Unsupported)?;
    let kind = source_kind(saved.kind);
    if let Some(existing) = sources.iter().map(LocalSource::descriptor).find(|s| s.id == saved.id) {
        return if existing.canonical_path.as_deref() == Some(saved.path.as_str()) && existing.kind == kind {
            Ok(())
        } else {
            Err(LocalError::SourceAmbiguous)
        };
    }
    if let Some(existing) = sources.iter().map(LocalSource::descriptor)
        .find(|s| s.canonical_path.as_deref() == Some(saved.path.as_str()))
    {
        return if existing.kind == kind { Ok(()) } else { Err(LocalError::SourceAmbiguous) };
    }
    if sources.len() >= 32 {
        return Err(LocalError::TooManyItems);
    }
    // A folder on removable storage requires its separately verified volume
    // witness. A File is only this exact path's intent, never its parent grant.
    let requires_usb = kind == SourceKind::Folder
        && (saved.usb.is_some() || crate::app::ott_usb::is_usb_path(&saved.path));
    let available = restore
        && !requires_usb
        && canonical(&saved.path).is_ok_and(|(path, actual)| path == saved.path && actual == kind);
    cancelled(cancel)?;
    let descriptor = BoundSource {
        id: saved.id,
        name: saved.name,
        kind,
        available,
        authority_ref: AuthorityRef(sources.len() as u64 + 1),
        generation,
        canonical_path: Some(saved.path),
    };
    sources.push(if available {
        acquire(off, descriptor, cancel)?
    } else {
        let source = LocalSource::unavailable(descriptor);
        if requires_usb { source.requiring_usb_verification() } else { source }
    });
    Ok(())
}

pub(in crate::app) fn validate(
    off: &OffFrame,
    request: Request,
    cancel: &AtomicBool,
) -> Result<Validated, LocalError> {
    validate_with_mount_probe(off, request, cancel, &mut crate::app::ott_usb::mounts)
}

// The injected operation is the existing off-frame mount observation, not a
// replacement validator: FD, pathname, generation and cancellation checks below
// run identically for the live runner and deterministic removable-media fixtures.
pub(super) fn validate_with_mount_probe(
    off: &OffFrame,
    request: Request,
    cancel: &AtomicBool,
    observe: &mut dyn FnMut(&OffFrame, &AtomicBool) -> Result<Vec<plx_platform::tv::removable::MountCandidate>, crate::app::ott_usb::Error>,
) -> Result<Validated, LocalError> {
    if request.generation == 0 || request.known.len() > 32 {
        return Err(LocalError::NativeError);
    }
    cancelled(cancel)?;
    // Only an explicit reconnect may restore a previously observed unavailable
    // source; a successful pathname lookup is not a new grant by itself.
    let reconnectable: Vec<String> = request.known.iter().map(LocalSource::descriptor)
        .filter(|source| !source.available).map(|source| source.id.clone()).collect();
    // Retain known USB intent even if observation below revokes its live
    // witness. Neither startup nor explicit path reconnect may downgrade it.
    let usb_paths: Vec<String> = request.remembered.iter()
        .filter(|saved| saved.kind == LocalSourceKind::Folder && saved.usb.is_some())
        .map(|saved| saved.path.clone())
        .chain(request.known.iter().filter(|source| source.requires_usb_verification())
            .filter_map(|source| source.descriptor().canonical_path.clone()))
        .collect();
    let usb_folder = |path: &str| crate::app::ott_usb::is_usb_path(path)
        || usb_paths.iter().any(|saved| saved == path);
    let mut sources = request.known;
    let mut matched = Vec::new();
    let operation_error = (|| -> Result<(), LocalError> {
        // A failed/incomplete snapshot cannot be interpreted as absent volumes.
        // Observe once for all connected USB roots; publication remains Runtime-owned.
        let needs_usb = sources.iter().any(|source| source.usb_witness().is_some());
        let usb = if needs_usb { Some(observe(off, cancel)
            .map_err(|error| if error == crate::app::ott_usb::Error::Cancelled {
                LocalError::RuntimeChanged
            } else { LocalError::SourceUnavailable })?) } else { None };
        // Revalidation may revoke availability but cannot repoint an existing authority.
        for source in &mut sources {
            cancelled(cancel)?;
            if source.descriptor().kind == SourceKind::Folder
                && source.descriptor().canonical_path.as_deref().is_some_and(&usb_folder)
                && !source.usb_witness().is_some_and(|witness| usb.as_ref()
                    .is_some_and(|mounts| witness.is_observed(mounts, source.descriptor()))) {
                *source = source.unavailable_observation();
                continue;
            }
            *source = source.observe(off, &mut || !cancel.load(Ordering::Acquire))
                .map_err(|_| LocalError::RuntimeChanged)?;
        }
        for saved in request.remembered {
            cancelled(cancel)?;
            append(
                off, cancel, &mut sources,
                saved,
                request.generation,
                request.restore_remembered,
            )?;
        }
        if let Some(query) = &request.lookup {
            cancelled(cancel)?;
            let usb_query = usb_folder(query);
            let read_only = !request.allow_new && !request.allow_reconnect;
            if read_only {
                // A known unavailable intent remains visible, without acquisition.
                matched.extend(sources.iter().map(LocalSource::descriptor)
                    .filter(|source| source.canonical_path.as_deref() == Some(query.as_str()))
                    .cloned());
                if !matched.is_empty() { return Ok(()); }
                // A read-only query does not inspect an unknown removable path.
                if usb_query { return Err(LocalError::SourceUnavailable); }
            }
            let (path, kind) = match canonical(query) {
                Ok(value) => value,
                Err(error) => {
                    matched.extend(
                        sources.iter().map(LocalSource::descriptor)
                            .filter(|source| source.canonical_path.as_deref() == Some(query.as_str()))
                            .cloned(),
                    );
                    return if usb_query && matched.is_empty() { Err(LocalError::SourceUnavailable) }
                        else if error == LocalError::Unsupported && matched.is_empty() { Err(error) }
                        else { Ok(()) };
                }
            };
            if kind == SourceKind::Folder && (usb_query || usb_folder(&path)) {
                matched.extend(sources.iter().map(LocalSource::descriptor)
                    .filter(|source| source.kind == kind && source.available
                        && source.canonical_path.as_deref() == Some(path.as_str()))
                    .cloned());
                return if matched.is_empty() { Err(LocalError::SourceUnavailable) } else { Ok(()) };
            }
            if request.allow_reconnect {
                for source in &mut sources {
                    let old = source.descriptor();
                    if !old.available && old.kind == kind && old.canonical_path.as_ref() == Some(&path)
                        && reconnectable.contains(&old.id)
                    {
                        if old.generation == request.generation {
                            return Err(LocalError::RuntimeChanged);
                        }
                        let mut descriptor = old.clone();
                        descriptor.generation = request.generation;
                        *source = acquire(off, descriptor, cancel)?;
                    }
                }
            }
            matched.extend(
                sources
                    .iter().map(LocalSource::descriptor)
                    .filter(|s| (read_only || (s.available && s.kind == kind))
                        && s.canonical_path.as_ref() == Some(&path))
                    .cloned(),
            );
            if matched.is_empty() && request.allow_new {
                let name = basename(&path)?;
                append(
                    off, cancel, &mut sources,
                    FolderDescriptor { id: request.new_id, name, path: path.clone(), kind: saved_kind(kind), usb: None },
                    request.generation,
                    true,
                )?;
                matched.extend(
                    sources
                        .iter().map(LocalSource::descriptor)
                        .filter(|s| s.available && s.kind == kind && s.canonical_path.as_ref() == Some(&path))
                        .cloned(),
                );
            }
        }
        cancelled(cancel)?;
        Ok(())
    })().err();
    cancelled(cancel)?;
    Ok(Validated {
        sources,
        lookup: request.lookup,
        matched,
        operation_error,
    })
}

// Failure to open is an unavailable observation, not a new grant. Cancellation
// remains addressed operation failure and never retires a shared handle here.
fn acquire(off: &OffFrame, descriptor: BoundSource, cancel: &AtomicBool) -> Result<LocalSource, LocalError> {
    match LocalSource::acquire_authorized(off, descriptor.clone(), &mut || !cancel.load(Ordering::Acquire)) {
        Ok(source) => Ok(source),
        Err(RootError::Cancelled) => Err(LocalError::RuntimeChanged),
        Err(RootError::Unavailable) => Ok(LocalSource::unavailable(descriptor)),
    }
}

#[cfg(test)]
mod usb_tests {
    use super::*;
    fn request() -> Request {
        Request { known: vec![], remembered: vec![], lookup: None, allow_new: true,
            allow_reconnect: true, restore_remembered: true, new_id: "new-root".into(), generation: 8 }
    }
    #[test]
    fn remembered_usb_path_cannot_reopen_a_cold_boot_grant() {
        let mut request = request();
        request.remembered.push(FolderDescriptor {
            kind: LocalSourceKind::Folder, usb: None, id: "remembered".into(), name: "Videos".into(), path: "/tmp/usb/disk/part/Movies".into() });
        let result = validate(&OffFrame::for_test(), request, &AtomicBool::new(false)).unwrap();
        assert!(result.operation_error.is_none());
        assert_eq!(result.sources.len(), 1);
        assert!(!result.sources[0].descriptor().available);
        assert!(result.sources[0].root_handle().is_none());
    }
    #[test]
    fn unobserved_usb_string_lookup_cannot_create_or_reconnect_a_source() {
        let mut request = request();
        request.lookup = Some("/tmp/usb/disk/part".into());
        let result = validate(&OffFrame::for_test(), request, &AtomicBool::new(false)).unwrap();
        assert_eq!(result.operation_error, Some(LocalError::SourceUnavailable));
        assert!(result.sources.is_empty());
        assert!(result.matched.is_empty());
    }
}

#[cfg(test)]
mod observed_loss_tests {
    use super::*;
    struct Folder(std::path::PathBuf);
    impl Folder {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("ott-observed-loss-{}", std::process::id()));
            std::fs::create_dir(&path).expect("unique owned test directory");
            Self(std::fs::canonicalize(path).unwrap())
        }
        fn path(&self) -> String { self.0.to_str().unwrap().into() }
    }
    impl Drop for Folder {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }
    fn source(folder: &Folder) -> BoundSource {
        BoundSource { id:"root".into(),name:"Movies".into(),kind:SourceKind::Folder,
            available:true,authority_ref:AuthorityRef(1),generation:7,
            canonical_path:Some(folder.path()) }
    }
    fn request(known: LocalSource, generation: u64, reconnect: bool) -> Request {
        Request { lookup: reconnect.then(||known.descriptor().canonical_path.clone().unwrap()),
            known:vec![known], remembered:Vec::new(),allow_new:reconnect,
            allow_reconnect:reconnect,restore_remembered:true,new_id:"unused-new-root".into(),generation }
    }
    #[test]
    fn observed_absence_then_same_path_return_requires_explicit_fresh_reconnect() {
        let _serial = plx_base::testlock::serial();
        let folder = Folder::new(); let original = acquire(&OffFrame::for_test(), source(&folder), &AtomicBool::new(false)).unwrap();
        std::fs::remove_dir(&folder.0).unwrap();
        let lost = validate(&OffFrame::for_test(), request(original.clone(),8,false),&AtomicBool::new(false)).unwrap().sources.remove(0);
        assert!(!lost.descriptor().available);
        std::fs::create_dir(&folder.0).unwrap();
        let automatic = validate(&OffFrame::for_test(), request(lost.clone(),9,false),&AtomicBool::new(false)).unwrap().sources.remove(0);
        assert!(!automatic.descriptor().available, "returning pathname cannot reactivate an observed lost authority");
        let reconnected = validate(&OffFrame::for_test(), request(lost,10,true),&AtomicBool::new(false)).unwrap().sources.remove(0);
        assert!(reconnected.descriptor().available);
        assert_eq!(reconnected.descriptor().id, original.descriptor().id);
        assert_ne!(reconnected.descriptor().generation, original.descriptor().generation);
        assert_eq!(reconnected.descriptor().generation, 10);
    }
    #[test]
    fn failed_requested_path_does_not_discard_an_observed_known_root_loss() {
        let _serial = plx_base::testlock::serial();
        let folder = Folder::new(); let original = acquire(&OffFrame::for_test(), source(&folder), &AtomicBool::new(false)).unwrap();
        std::fs::remove_dir(&folder.0).unwrap();
        let result = validate(&OffFrame::for_test(), request(original,8,true),&AtomicBool::new(false)).unwrap();
        assert!(!result.sources[0].descriptor().available);
        assert!(!result.matched[0].available);
    }
    #[test]
    fn both_append_capacity_failures_retain_observed_loss_before_path_returns() {
        let _serial = plx_base::testlock::serial();
        let parent = Folder::new();
        let mut known = Vec::new();
        for index in 0..33 {
            let path = parent.0.join(format!("folder-{index}"));
            std::fs::create_dir(&path).unwrap();
            if index < 32 {
                let mut root = source(&parent);
                root.id = format!("root-{index}"); root.name = format!("Folder {index}");
                root.authority_ref = AuthorityRef(index + 1);
                root.canonical_path = Some(path.to_str().unwrap().into());
                known.push(acquire(&OffFrame::for_test(), root, &AtomicBool::new(false)).unwrap());
            }
        }
        let target = parent.0.join("folder-32").to_str().unwrap().to_owned();
        std::fs::remove_dir(parent.0.join("folder-0")).unwrap();
        for remembered in [false, true] {
            let request = Request { known: known.clone(), generation:8, allow_new:true,
                allow_reconnect:true, restore_remembered:true, new_id:"root-32".into(),
                remembered: if remembered { vec![FolderDescriptor {
                    kind:LocalSourceKind::Folder, usb:None, id:"root-32".into(), name:"Folder 32".into(), path:target.clone() }] } else { Vec::new() },
                lookup: (!remembered).then(||target.clone()) };
            let report = validate(&OffFrame::for_test(), request, &AtomicBool::new(false)).unwrap();
            assert_eq!(report.operation_error, Some(LocalError::TooManyItems));
            assert_eq!(report.sources.len(),32);
            assert!(!report.sources[0].descriptor().available, "append error must not erase a completed loss observation");
            std::fs::create_dir(parent.0.join("folder-0")).unwrap();
            let returned = validate(&OffFrame::for_test(), Request { known:report.sources, generation:9,
                allow_new:false, allow_reconnect:false,restore_remembered:false,new_id:String::new(),
                remembered:Vec::new(),lookup:None }, &AtomicBool::new(false)).unwrap();
            assert!(returned.operation_error.is_none());
            assert!(!returned.sources[0].descriptor().available, "same pathname does not restore the old authority");
            std::fs::remove_dir(parent.0.join("folder-0")).unwrap();
        }
    }

    #[test]
    fn portable_binding_blocks_cold_plain_path_and_explicit_reconnect() {
        let _serial = plx_base::testlock::serial();
        let folder = Folder::new();
        let descriptor = FolderDescriptor { id: "root".into(), name: "Movies / 夏".into(),
            path: folder.path(), kind: LocalSourceKind::Folder,
            usb: Some(crate::app::ott_local_sources::UsbBinding::from_observed(
                "ext4", "saved-volume", "/", "Movies/夏").unwrap()) };
        assert!(!crate::app::ott_usb::is_usb_path(&descriptor.path));
        let off = OffFrame::for_test();
        let cancel = AtomicBool::new(false);
        let cold = validate(&off, Request { known: vec![], remembered: vec![descriptor.clone()],
            lookup: None, allow_new: false, allow_reconnect: false, restore_remembered: true,
            new_id: "unused".into(), generation: 8 }, &cancel).unwrap();
        assert!(cold.operation_error.is_none());
        assert_eq!(cold.sources.len(), 1);
        assert!(!cold.sources[0].descriptor().available && cold.sources[0].root_handle().is_none());
        let result = validate(&off, Request { known: cold.sources,
            remembered: vec![descriptor.clone()], lookup: Some(folder.path()),
            allow_new: true, allow_reconnect: true, restore_remembered: true,
            new_id: "must-not-replace".into(), generation: 9 }, &cancel).unwrap();
        assert_eq!(result.operation_error, Some(LocalError::SourceUnavailable));
        assert!(result.matched.is_empty());
        assert_eq!(result.sources.len(), 1);
        assert!(!result.sources[0].descriptor().available && result.sources[0].root_handle().is_none());
        assert_eq!(result.sources[0].descriptor().id, descriptor.id);
    }

    #[test]
    fn ordinary_private_folder_without_binding_still_restores_from_path() {
        let _serial = plx_base::testlock::serial();
        let folder = Folder::new();
        let result = validate(&OffFrame::for_test(), Request {
            known: vec![], remembered: vec![FolderDescriptor {
                id: "root".into(), name: "Movies".into(), path: folder.path(),
                kind: LocalSourceKind::Folder, usb: None }], lookup: Some(folder.path()),
            allow_new: true, allow_reconnect: true, restore_remembered: true,
            new_id: "unused".into(), generation: 8 }, &AtomicBool::new(false)).unwrap();
        assert!(result.operation_error.is_none());
        assert_eq!(result.matched.len(), 1);
        assert_eq!(result.sources.len(), 1);
        assert!(result.sources[0].is_live() && result.sources[0].root_handle().is_some());
        assert!(result.sources[0].usb_witness().is_none());
    }

}

#[cfg(test)]
mod single_file_tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            for n in 0..128 {
                let path = std::env::temp_dir().join(format!("ott-file-bindings-{}-{n}", std::process::id()));
                match fs::create_dir(&path) {
                    Ok(()) => {
                        let path = fs::canonicalize(path).unwrap();
                        fs::write(path.join("movie.mp4"), b"test fixture").unwrap();
                        fs::write(path.join("sibling.mp4"), b"not selected").unwrap();
                        return Self(path);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => panic!("test directory: {e}"),
                }
            }
            panic!("test directory slots exhausted")
        }
        fn path(&self) -> String { self.0.join("movie.mp4").to_str().unwrap().into() }
        fn saved(&self) -> FolderDescriptor {
            FolderDescriptor { id: "selected-file".into(), name: "Selected movie".into(), path: self.path(), kind: LocalSourceKind::File, usb: None }
        }
        fn request(&self) -> Request {
            Request { known: vec![], remembered: vec![], lookup: Some(self.path()), allow_new: true,
                allow_reconnect: true, restore_remembered: true, new_id: "selected-file".into(), generation: 7 }
        }
        fn run(&self, request: Request) -> Validated {
            validate(&OffFrame::for_test(), request, &AtomicBool::new(false)).unwrap()
        }
        fn connected(&self) -> LocalSource {
            let mut result = self.run(self.request());
            assert!(result.operation_error.is_none());
            assert_eq!(result.matched.len(), 1);
            result.sources.remove(0)
        }
    }
    impl Drop for Fixture { fn drop(&mut self) { plx_base::testscratch::remove_tree(&self.0); } }

    #[test]
    fn explicit_file_has_one_exact_grant_and_basename_without_parent() {
        let f = Fixture::new();
        let source = f.connected();
        assert!(source.descriptor().kind == SourceKind::File);
        assert_eq!(source.descriptor().name, "movie.mp4");
        assert!(source.is_live() && source.root_handle().is_none());
        assert!(source.single_file("movie.mp4").is_some());
        assert!(source.single_file("sibling.mp4").is_none());
    }

    #[test]
    fn sources_reports_remembered_file_but_cannot_acquire_it_or_unknown_lookup() {
        let f = Fixture::new();
        let mut request = f.request();
        request.allow_new = false;
        request.allow_reconnect = false;
        request.restore_remembered = false;
        let unknown = f.run(request);
        assert!(unknown.sources.is_empty() && unknown.matched.is_empty());
        let mut request = f.request();
        request.allow_new = false;
        request.allow_reconnect = false;
        request.restore_remembered = false;
        request.remembered.push(f.saved());
        let result = f.run(request);
        assert!(result.operation_error.is_none());
        assert_eq!(result.sources.len(), 1);
        assert!(result.sources[0].descriptor().kind == SourceKind::File);
        assert!(!result.sources[0].is_live());
        assert_eq!(result.matched.len(), 1);
        assert!(!result.matched[0].available);
    }

    #[test]
    fn sources_reuses_live_file_grant_without_reacquiring() {
        let f = Fixture::new();
        let source = f.connected();
        let mut request = f.request();
        request.known.push(source.clone());
        request.remembered.push(f.saved());
        request.allow_new = false;
        request.allow_reconnect = false;
        request.restore_remembered = false;
        let result = f.run(request);
        assert!(result.operation_error.is_none());
        assert_eq!(result.sources.len(), 1);
        assert_eq!(result.matched.len(), 1);
        assert!(source.same_grant(&result.sources[0]));
    }

    #[test]
    fn startup_restores_file_kind_only_from_explicit_remembered_intent() {
        let f = Fixture::new();
        let mut request = f.request();
        request.lookup = None;
        request.allow_new = false;
        request.allow_reconnect = false;
        request.remembered.push(f.saved());
        let result = f.run(request);
        assert!(result.operation_error.is_none());
        assert_eq!(result.sources.len(), 1);
        assert!(result.sources[0].is_live());
        assert_eq!(result.sources[0].descriptor().name, "Selected movie");
        assert!(result.sources[0].descriptor().kind == SourceKind::File);
    }

    #[test]
    fn remembered_folder_cannot_turn_into_file_and_live_file_cannot_turn_into_folder() {
        let f = Fixture::new();
        let mut request = f.request();
        request.lookup = None;
        let mut saved = f.saved(); saved.kind = LocalSourceKind::Folder;
        request.remembered.push(saved);
        let result = f.run(request);
        assert!(!result.sources[0].is_live());
        assert!(result.sources[0].descriptor().kind == SourceKind::Folder);
        let original = f.connected();
        fs::remove_file(f.path()).unwrap(); fs::create_dir(f.path()).unwrap();
        let mut request = f.request(); request.known.push(original.clone()); request.generation = 8;
        let result = f.run(request);
        assert_eq!(result.operation_error, Some(LocalError::SourceAmbiguous));
        assert_eq!(result.sources.len(), 1);
        assert!(!result.sources[0].is_live());
        assert!(result.sources[0].descriptor().kind == SourceKind::File);
        assert!(original.is_live(), "observation cannot withdraw the owner's grant");
    }

    #[test]
    fn observed_file_loss_requires_explicit_fresh_reconnect() {
        let f = Fixture::new(); let original = f.connected();
        fs::rename(f.path(), f.0.join("old.mp4")).unwrap();
        fs::write(f.path(), b"replacement").unwrap();
        let mut request = f.request(); request.known.push(original.clone()); request.generation = 8;
        request.allow_new = false; request.allow_reconnect = false; request.restore_remembered = false;
        let mut result = f.run(request);
        assert!(!result.sources[0].is_live());
        let mut request = f.request(); request.known.push(result.sources.remove(0)); request.generation = 9;
        let result = f.run(request);
        assert!(result.operation_error.is_none());
        assert_eq!(result.sources.len(), 1);
        assert!(result.sources[0].is_live());
        assert!(!result.sources[0].same_grant(&original));
        assert_eq!(result.sources[0].descriptor().generation, 9);
    }

    #[test]
    fn unsupported_file_symlink_and_fifo_never_become_grants() {
        let f = Fixture::new(); fs::write(f.0.join("notes.txt"), b"notes").unwrap();
        std::os::unix::fs::symlink(f.path(), f.0.join("alias.mp4")).unwrap();
        let fifo = f.0.join("pipe.mp4");
        let raw = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);
        for name in ["notes.txt", "alias.mp4", "pipe.mp4"] {
            let mut request = f.request(); request.lookup = Some(f.0.join(name).to_str().unwrap().into());
            let result = f.run(request);
            assert_eq!(result.operation_error, Some(LocalError::Unsupported));
            assert!(result.sources.is_empty() && result.matched.is_empty());
        }
    }

    #[test]
    fn cancellation_prevents_file_grant_publication() {
        let f = Fixture::new();
        assert!(matches!(validate(&OffFrame::for_test(), f.request(), &AtomicBool::new(true)), Err(LocalError::RuntimeChanged)));
    }
}
