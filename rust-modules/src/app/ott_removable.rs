//! Pure removable-source identity and authority policy. All observations are
//! supplied by the existing Local worker; this module discovers nothing.
//! A plan is not a file-access grant and makes no playback/autoplay decision.

use super::ott_local_session::AuthorityRef;
use plx_data::ott::valid_local_relative_path;

pub(super) const OBSERVATION_LIMIT: usize = 256;
const IDENTITY_BYTES: usize = 256;
const PATH_BYTES: usize = 4096;

/// Receiver-private stable volume evidence, supplied by the trusted worker.
/// Namespace and value are exact, never inferred from a label or mount path.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct VolumeKey {
    pub namespace: String,
    pub value: String,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct RememberedSource {
    /// Existing public source ID, independent of the current mount location.
    pub source_id: String,
    pub volume: Option<VolumeKey>,
    /// Empty selects the volume root; otherwise exact POSIX components.
    pub relative_folder: String,
}

/// This is evidence from a completed filesystem check, not a request to probe.
#[derive(Clone, PartialEq, Eq)]
pub(super) enum FolderObservation {
    Readable { canonical_path: String },
    Missing,
    PermissionDenied,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct MountObservation {
    pub volume: Option<VolumeKey>,
    /// Opaque existing-worker incarnation, changed after loss or replacement.
    /// A kernel mount ID alone is insufficient: the kernel can reuse it.
    pub incarnation: String,
    pub canonical_mount: String,
    /// Identifies which exact folder the worker actually checked.
    pub relative_folder: String,
    pub folder: FolderObservation,
}

/// A borrowed view of the existing inventory, never a second live registry.
pub(super) struct InventorySnapshot<'a> {
    pub generation: u64,
    pub complete: bool,
    pub mounts: &'a [MountObservation],
}

#[derive(Clone, Copy)]
pub(super) enum Selection<'a> {
    Automatic,
    /// Explicit selection of the exact observation, not a mount-path lookup.
    Explicit {
        incarnation: &'a str,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct GrantStamp {
    pub authority_ref: AuthorityRef,
    pub generation: u64,
}

/// Private evidence attached to an existing BoundSource by the integrator.
/// It is deliberately neither Debug nor Serialize.
#[derive(Clone)]
pub(super) struct Binding {
    source: RememberedSource,
    observation: MountObservation,
    grant: GrantStamp,
}

impl Binding {
    pub fn source_id(&self) -> &str {
        &self.source.source_id
    }
    pub fn canonical_path(&self) -> &str {
        match &self.observation.folder {
            FolderObservation::Readable { canonical_path } => canonical_path,
            _ => unreachable!("only readable observations are admitted"),
        }
    }
    pub fn grant(&self) -> GrantStamp {
        self.grant
    }
}

/// The common owner retains revoked evidence but must not revive its grant.
#[derive(Clone, Copy)]
pub(super) struct PreviousBinding<'a> {
    pub binding: &'a Binding,
    pub retired: bool,
}

pub(super) struct BindingPlan {
    source: RememberedSource,
    observation: MountObservation,
    generation: u64,
    previous: Option<GrantStamp>,
}

pub(super) enum Assessment {
    /// Only the same still-observed incarnation retains an existing grant.
    Current,
    Revalidate(BindingPlan),
    Unavailable(Unavailable),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Unavailable {
    IncompleteInventory,
    Absent,
    UnknownIdentity,
    Ambiguous,
    DifferentVolume,
    FolderMissing,
    PermissionDenied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PolicyError {
    InvalidEvidence,
    StaleEvidence,
    ChangedEvidence,
    InvalidGrant,
}

fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn valid_volume(key: &VolumeKey) -> bool {
    text(&key.namespace, 64) && text(&key.value, IDENTITY_BYTES)
}

fn valid_relative(path: &str) -> bool {
    path.len() <= PATH_BYTES && (path.is_empty() || valid_local_relative_path(path))
}

fn valid_absolute(path: &str) -> bool {
    text(path, PATH_BYTES)
        && path.starts_with('/')
        && !path.contains('\\')
        && (path == "/"
            || path[1..]
                .split('/')
                .all(|part| !matches!(part, "" | "." | "..")))
}

fn expected_folder(mount: &str, relative: &str) -> String {
    if relative.is_empty() {
        mount.to_owned()
    } else if mount == "/" {
        format!("/{relative}")
    } else {
        format!("{mount}/{relative}")
    }
}

fn validate(
    source: &RememberedSource,
    snapshot: &InventorySnapshot<'_>,
) -> Result<(), PolicyError> {
    if !text(&source.source_id, IDENTITY_BYTES)
        || !source
            .source_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
        || !valid_relative(&source.relative_folder)
        || source.volume.as_ref().is_some_and(|key| !valid_volume(key))
        || snapshot.generation == 0
        || snapshot.mounts.len() > OBSERVATION_LIMIT
    {
        return Err(PolicyError::InvalidEvidence);
    }
    for (i, mount) in snapshot.mounts.iter().enumerate() {
        if !text(&mount.incarnation, IDENTITY_BYTES)
            || !valid_absolute(&mount.canonical_mount)
            || !valid_relative(&mount.relative_folder)
            || mount.volume.as_ref().is_some_and(|key| !valid_volume(key))
            || snapshot.mounts[..i]
                .iter()
                .any(|old| old.incarnation == mount.incarnation)
        {
            return Err(PolicyError::InvalidEvidence);
        }
        if let FolderObservation::Readable { canonical_path } = &mount.folder {
            if !valid_absolute(canonical_path)
                || *canonical_path
                    != expected_folder(&mount.canonical_mount, &mount.relative_folder)
            {
                return Err(PolicyError::InvalidEvidence);
            }
        }
    }
    Ok(())
}

fn choose<'a>(
    source: &RememberedSource,
    snapshot: &'a InventorySnapshot<'_>,
    selection: Selection<'_>,
) -> Result<&'a MountObservation, Unavailable> {
    if !snapshot.complete {
        return Err(Unavailable::IncompleteInventory);
    }
    // Duplicate trusted keys cannot be disambiguated by choosing a mount path.
    if let Some(key) = &source.volume {
        if snapshot
            .mounts
            .iter()
            .filter(|m| m.volume.as_ref() == Some(key))
            .count()
            > 1
        {
            return Err(Unavailable::Ambiguous);
        }
    }
    let mount = match selection {
        Selection::Automatic => {
            let key = source.volume.as_ref().ok_or(Unavailable::UnknownIdentity)?;
            snapshot
                .mounts
                .iter()
                .find(|m| m.volume.as_ref() == Some(key))
                .ok_or(Unavailable::Absent)?
        }
        Selection::Explicit { incarnation } => {
            let mount = snapshot
                .mounts
                .iter()
                .find(|m| m.incarnation == incarnation)
                .ok_or(Unavailable::Absent)?;
            if source.volume.is_some() && source.volume != mount.volume {
                return Err(Unavailable::DifferentVolume);
            }
            if let Some(key) = &mount.volume {
                if snapshot
                    .mounts
                    .iter()
                    .filter(|m| m.volume.as_ref() == Some(key))
                    .count()
                    > 1
                {
                    return Err(Unavailable::Ambiguous);
                }
            }
            mount
        }
    };
    if mount.relative_folder != source.relative_folder {
        return Err(Unavailable::FolderMissing);
    }
    match mount.folder {
        FolderObservation::Readable { .. } => Ok(mount),
        FolderObservation::Missing => Err(Unavailable::FolderMissing),
        FolderObservation::PermissionDenied => Err(Unavailable::PermissionDenied),
    }
}

/// All non-Current outcomes retire any old live authority immediately. Keep the
/// previous private binding as a retired receipt until replacement is admitted.
pub(super) fn assess(
    source: &RememberedSource,
    snapshot: &InventorySnapshot<'_>,
    selection: Selection<'_>,
    previous: Option<PreviousBinding<'_>>,
) -> Result<Assessment, PolicyError> {
    validate(source, snapshot)?;
    if previous.is_some_and(|old| {
        old.binding.source != *source || snapshot.generation < old.binding.grant.generation
    }) {
        return Err(PolicyError::StaleEvidence);
    }
    // An explicitly admitted unknown volume can remain selected in this process
    // while its exact incarnation is still observed. This is not reattachment.
    let selection = match (selection, previous) {
        (Selection::Automatic, Some(old))
            if !old.retired
                && source.volume.is_none()
                && snapshot
                    .mounts
                    .iter()
                    .any(|m| m == &old.binding.observation) =>
        {
            Selection::Explicit {
                incarnation: &old.binding.observation.incarnation,
            }
        }
        (selection, _) => selection,
    };
    let observation = match choose(source, snapshot, selection) {
        Ok(observation) => observation,
        Err(reason) => return Ok(Assessment::Unavailable(reason)),
    };
    if previous.is_some_and(|old| !old.retired && old.binding.observation == *observation) {
        return Ok(Assessment::Current);
    }
    if previous.is_some_and(|old| snapshot.generation == old.binding.grant.generation) {
        return Err(PolicyError::StaleEvidence);
    }
    Ok(Assessment::Revalidate(BindingPlan {
        source: source.clone(),
        observation: observation.clone(),
        generation: snapshot.generation,
        previous: previous.map(|old| old.binding.grant),
    }))
}

/// Consume a plan only after a fresh completed worker observation for the SAME
/// request/source generation. Pure data cannot prove that IO was repeated: the
/// runner must supply the real completion, not recycle the planning snapshot.
/// The existing inventory owner supplies authority; this function mints none.
pub(super) fn admit(
    plan: BindingPlan,
    fresh: &InventorySnapshot<'_>,
    grant: GrantStamp,
) -> Result<Binding, PolicyError> {
    validate(&plan.source, fresh)?;
    if fresh.generation != plan.generation {
        return Err(PolicyError::StaleEvidence);
    }
    let observation = choose(
        &plan.source,
        fresh,
        Selection::Explicit {
            incarnation: &plan.observation.incarnation,
        },
    )
    .map_err(|_| PolicyError::ChangedEvidence)?;
    if observation != &plan.observation {
        return Err(PolicyError::ChangedEvidence);
    }
    if grant.authority_ref.0 == 0
        || grant.generation != fresh.generation
        || plan.previous.is_some_and(|old| {
            grant.authority_ref != old.authority_ref || grant.generation <= old.generation
        })
    {
        return Err(PolicyError::InvalidGrant);
    }
    Ok(Binding {
        source: plan.source,
        observation: plan.observation,
        grant,
    })
}

#[cfg(test)]
#[path = "ott_removable_tests.rs"]
mod tests;
