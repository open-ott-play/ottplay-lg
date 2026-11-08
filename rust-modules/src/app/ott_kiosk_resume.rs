//! One bounded resume slot for explicitly durable Local-folder kiosk playback.
//! Stored descriptors correlate data only; every playback/checkpoint still needs
//! the existing current LocalSelection and held source grant. No IO or new owner.
use super::*;
use crate::app::ott_kiosk_stored::{self as stored, TargetRef};
use crate::app::ott_local_session as local;
use crate::app::ott_local_sources::{FolderDescriptor, LocalSourceKind, RememberedFolders};

pub(super) const KEY: &str = "@kiosk.local";

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Scope {
    source_id: String,
    fingerprint: String,
}

fn descriptor(saved: &Saved, id: &str) -> Option<FolderDescriptor> {
    let folders = RememberedFolders::read(&saved.settings).ok()?;
    let folder = folders.folders().iter().find(|folder| folder.id == id)?;
    if folder.kind != LocalSourceKind::Folder {
        return None;
    }
    folder.usb.as_ref()?.validate().ok()?;
    Some(folder.clone())
}

/// Exact serialized binding, including UUID spelling and filesystem-relative
/// folder. Strictness does not change the source; rename or replacement does.
fn stored_scope(saved: &Saved) -> Option<Scope> {
    let policy = stored::read(&saved.settings).ok()??;
    let TargetRef::LocalFolder {
        source_id,
        source_fingerprint,
    } = policy.target()
    else {
        return None;
    };
    let folder = descriptor(saved, source_id)?;
    let bytes = serde_json::to_vec(&("local-kiosk-resume-v1", source_fingerprint, &folder)).ok()?;
    Some(Scope {
        source_id: source_id.into(),
        fingerprint: plx_data::ott::favorites_wire::hash(&bytes),
    })
}

pub(super) fn capture(runtime: &Runtime, selection: &transient::LocalSelection) -> Option<Scope> {
    if !runtime.loaded || runtime.persistence_uncertain {
        return None;
    }
    let scope = stored_scope(&runtime.saved)?;
    preflight(runtime, &scope.source_id).ok()?;
    if scope.source_id != selection.source_id {
        return None;
    }
    let read = runtime.capture_local_root(selection)?;
    if !read.root.is_live() {
        return None;
    }
    let root = runtime.current_local_source(selection)?;
    let folder = descriptor(&runtime.saved, &scope.source_id)?;
    let policy = stored::read(&runtime.saved.settings).ok()??;
    let TargetRef::LocalFolder {
        source_fingerprint, ..
    } = policy.target()
    else {
        return None;
    };
    if root.kind != local::SourceKind::Folder
        || root.name != folder.name
        || root.canonical_path.as_deref() != Some(folder.path.as_str())
        || selection::local_root_fingerprint(root).ok()?.as_str() != source_fingerprint
    {
        return None;
    }
    Some(scope)
}

pub(super) fn preflight(
    runtime: &Runtime,
    source_id: &str,
) -> Result<(), kiosk_source::SourceError> {
    if descriptor(&runtime.saved, source_id).is_none() {
        return Ok(());
    }
    // Count the bounded union without cloning Saved, Favorites or credentials.
    let count = runtime.saved.resume.len()
        + runtime
            .checkpoints
            .keys()
            .filter(|key| !runtime.saved.resume.contains_key(*key))
            .count();
    let reserved = runtime.saved.resume.contains_key(KEY) || runtime.checkpoints.contains_key(KEY);
    if count > 91 || (!reserved && count >= 91) {
        return Err(kiosk_source::SourceError::ResumeCapacity);
    }
    Ok(())
}

/// Reserve inside the existing policy transaction, never in live state. The
/// first row at zero is only startup intent; no native success is asserted.
pub(super) fn reserve(runtime: &Runtime, candidate: &mut Saved, id: &str) {
    let Some(scope) = stored_scope(candidate) else {
        return;
    };
    let existing = runtime
        .checkpoints
        .get(KEY)
        .or_else(|| candidate.resume.get(KEY))
        .filter(|point| point.source_fingerprint == scope.fingerprint)
        .cloned();
    candidate.resume.insert(
        KEY.into(),
        existing.unwrap_or(SavedResume {
            entry_id: id.into(),
            position_ms: 0,
            source_fingerprint: scope.fingerprint,
        }),
    );
}

pub(super) fn restore(runtime: &Runtime, scope: &Scope) -> Option<continuity::ResumePoint> {
    runtime
        .checkpoints
        .get(KEY)
        .or_else(|| runtime.saved.resume.get(KEY))
        .filter(|point| point.source_fingerprint == scope.fingerprint)
        .map(|point| continuity::ResumePoint {
            entry_id: point.entry_id.clone(),
            position_ms: point.position_ms,
        })
}

/// Merge only a correlated latest slot over a staged point. A new designation
/// keeps its newly reserved point rather than an old session's checkpoint. An
/// existing USB point is retained here for the verified relocation projection.
pub(super) fn merge(saved: &mut Saved, checkpoints: &BTreeMap<String, SavedResume>) {
    for (key, value) in checkpoints {
        if key != KEY
            || saved
                .resume
                .get(KEY)
                .is_some_and(|old| old.source_fingerprint == value.source_fingerprint)
            || stored_scope(saved)
                .is_some_and(|scope| scope.fingerprint == value.source_fingerprint)
        {
            saved.resume.insert(key.clone(), value.clone());
        }
    }
}

/// Run after USB receipt-based projection, and on the final shutdown image.
pub(super) fn sanitize(saved: &mut Saved) {
    let valid = stored_scope(saved).is_some_and(|scope| {
        saved
            .resume
            .get(KEY)
            .is_some_and(|point| point.source_fingerprint == scope.fingerprint)
    });
    if !valid {
        saved.resume.remove(KEY);
    }
}

pub(super) fn reconciled(runtime: &mut Runtime) {
    let scope = stored_scope(&runtime.saved);
    if !runtime.checkpoints.get(KEY).is_some_and(|point| {
        scope
            .as_ref()
            .is_some_and(|scope| point.source_fingerprint == scope.fingerprint)
    }) {
        runtime.checkpoints.remove(KEY);
    }
    let mut retired = false;
    if let Some(transient::TransientMedia::Local(session)) = runtime.transient.accepted.as_mut() {
        if session.kiosk_resume.is_some() && session.kiosk_resume.as_ref() != scope.as_ref() {
            session.kiosk_resume = None;
            retired = true;
        }
    }
    if retired {
        let retained = if runtime.restore_play_generation == Some(runtime.play_generation) {
            let id = runtime
                .playing
                .as_ref()
                .map(|playing| playing.selection_id.clone());
            runtime.cancel_play_request();
            runtime.selected.clone_from(&id);
            id
        } else {
            None
        };
        runtime
            .continuity
            .cancel_restore(runtime.generation, retained.as_deref());
    }
}

/// Called only after successful activation of an already durable designation.
/// A new ordinary Local command constructs a fresh session with None instead.
pub(super) fn activated(runtime: &mut Runtime, source_id: &str) {
    let scope = match runtime.transient.accepted.as_ref() {
        Some(transient::TransientMedia::Local(session))
            if session.context.source().id == source_id =>
        {
            capture(runtime, &session.read.selection)
        }
        _ => None,
    };
    if let Some(transient::TransientMedia::Local(session)) = runtime.transient.accepted.as_mut() {
        session.kiosk_resume = scope;
    }
}

pub(super) fn checkpoint(
    runtime: &Runtime,
    playing: &Playing,
    position_ms: u64,
) -> Option<SavedResume> {
    let transient::TransientMedia::Local(session) = runtime.transient.accepted.as_ref()? else {
        return None;
    };
    let scope = session.kiosk_resume.as_ref()?;
    if playing.transient_owner != Some(session.context.owner())
        || playing.profile != session.profile
        || capture(runtime, &session.read.selection).as_ref() != Some(scope)
        || playing.local_selection.as_ref() != Some(&session.read.selection)
    {
        return None;
    }
    let id = runtime.folder_row_for(playing)?;
    Some(SavedResume {
        entry_id: id,
        position_ms,
        source_fingerprint: scope.fingerprint.clone(),
    })
}

/// Only the existing VerifiedRelocation caller can use this projection. Both
/// surrounding full descriptor/UUID checks remain in ott_usb_resume::project.
pub(super) fn relocation(
    current: &Saved,
    candidate: &mut Saved,
    relocated_source_id: &str,
) -> Option<(String, String, String)> {
    let before = stored_scope(current)?;
    let after = stored_scope(candidate)?;
    if before.source_id != relocated_source_id || before.source_id != after.source_id {
        return None;
    }
    let point = candidate.resume.get_mut(KEY)?;
    if point.source_fingerprint != before.fingerprint
        && point.source_fingerprint != after.fingerprint
    {
        return None;
    }
    point.source_fingerprint.clone_from(&after.fingerprint);
    Some((
        point.entry_id.clone(),
        before.fingerprint,
        after.fingerprint,
    ))
}

#[cfg(test)]
#[path = "ott_kiosk_resume_tests.rs"]
mod tests;
