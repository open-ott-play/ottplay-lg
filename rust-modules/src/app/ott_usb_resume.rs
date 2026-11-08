//! Receipt-constrained Saved projection for a relocated USB filesystem.
//! This module never grants access, starts playback, or publishes Saved state.
use super::{category_preferences, profile_fingerprint, selection, transient, Saved, SavedResume};
use crate::app::ott_local_root::LocalSource;
use crate::app::ott_local_session::{BoundSource, SourceKind};
use crate::app::ott_local_sources::{FolderDescriptor, RememberedFolders, UsbBinding};
use crate::app::ott_usb::VerifiedRelocation;
use plx_data::ott::favorites::{Favorite, Locator};
use plx_data::ott::{ProviderKind, ProviderProfile};
use std::collections::BTreeMap;

/// A projection retained by the existing USB install until its addressed Saved
/// acknowledgement. It cannot authorize a source or overwrite a newer item.
#[derive(Clone, Default)]
pub(super) struct CheckpointRebase {
    favorite: Option<FavoriteRebase>,
    kiosk: Option<FavoriteRebase>,
}
#[derive(Clone)]
struct FavoriteRebase {
    entry_id: String,
    before: String,
    after: String,
}
impl CheckpointRebase {
    /// Call only after the same USB install's durable acknowledgement and final
    /// receipt/context validation. Keep the latest position of this exact item.
    pub(super) fn apply(&self, checkpoints: &mut BTreeMap<String, SavedResume>) -> bool {
        let favorite = self.favorite.as_ref().is_some_and(|plan| {
            checkpoints.get_mut("favorites").is_some_and(|resume| plan.apply(resume))
        });
        let kiosk = self.kiosk.as_ref().is_some_and(|plan| {
            checkpoints.get_mut(super::kiosk_resume::KEY).is_some_and(|resume| plan.apply(resume))
        });
        favorite || kiosk
    }
}
impl FavoriteRebase {
    fn apply(&self, resume: &mut SavedResume) -> bool {
        if resume.entry_id != self.entry_id || resume.source_fingerprint != self.before {
            return false;
        }
        let changed = resume.source_fingerprint != self.after;
        resume.source_fingerprint.clone_from(&self.after);
        changed
    }
}

/// The caller runs this after private Local Favorite proofs have been rebased,
/// including after the writer merges its latest checkpoints. Repeating it
/// before/after the normal category projection is safe. Only the candidate is
/// changed; the returned checkpoint plan remains inert until durable success.
pub(super) fn rebase(
    current: &Saved,
    candidate: &mut Saved,
    source: &LocalSource,
    relocation: &VerifiedRelocation,
) -> Result<CheckpointRebase, &'static str> {
    if !relocation.is_current(source) {
        return Err("USB relocation is no longer current");
    }
    project(
        current,
        candidate,
        relocation.old_root(),
        relocation.new_root(),
        relocation.stable_binding(),
    )
}

fn descriptor_matches(folder: &FolderDescriptor, root: &BoundSource, binding: &UsbBinding) -> bool {
    folder.id == root.id
        && folder.name == root.name
        && Some(folder.path.as_str()) == root.canonical_path.as_deref()
        && folder.usb.as_ref() == Some(binding)
}

/// Only called behind the live worker receipt. Tests exercise projection here;
/// they do not manufacture filesystem authority or claim device verification.
fn project(
    current: &Saved,
    candidate: &mut Saved,
    old: &BoundSource,
    new: &BoundSource,
    binding: &UsbBinding,
) -> Result<CheckpointRebase, &'static str> {
    binding.validate()?;
    if old.id != new.id
        || old.name != new.name
        || old.kind != SourceKind::Folder
        || new.kind != SourceKind::Folder
        || old.authority_ref != new.authority_ref
        || old.generation == 0
        || old.generation != new.generation
        || !new.available
    {
        return Err("USB relocation changed source identity");
    }
    let before_folders = RememberedFolders::read(&current.settings)?;
    let after_folders = RememberedFolders::read(&candidate.settings)?;
    if !before_folders
        .folders()
        .iter()
        .any(|folder| descriptor_matches(folder, old, binding))
        || !after_folders
            .folders()
            .iter()
            .any(|folder| descriptor_matches(folder, new, binding))
    {
        return Err("USB relocation does not match remembered folders");
    }
    current
        .profiles
        .validate()
        .map_err(|_| "Invalid previous profiles")?;
    candidate
        .profiles
        .validate()
        .map_err(|_| "Invalid candidate profiles")?;
    current
        .favorites
        .validate()
        .map_err(|_| "Invalid Favorites document")?;
    // This operation moves a source; it never imports, removes or reinterprets
    // a Favorite, nor changes the global Favorites selection.
    if candidate.favorites != current.favorites
        || candidate.favorites_selected != current.favorites_selected
    {
        return Err("Favorites changed during USB relocation");
    }
    let pairs = relocated_profiles(current, candidate, old, new);
    let plan = favorite_plan(
        current,
        candidate,
        old,
        new,
        &before_folders,
        &after_folders,
        &pairs,
    );
    let mut next = candidate.clone();
    rebase_categories(current, &mut next, &pairs)?;
    if let (Some(plan), Some(resume)) = (&plan, next.resume.get_mut("favorites")) {
        plan.apply(resume);
    }
    let kiosk = super::kiosk_resume::relocation(current, &mut next, &old.id)
        .map(|(entry_id, before, after)| FavoriteRebase { entry_id, before, after });
    *candidate = next;
    Ok(CheckpointRebase { favorite: plan, kiosk })
}

fn relocated_profiles<'a>(
    current: &'a Saved,
    candidate: &'a Saved,
    old: &BoundSource,
    new: &BoundSource,
) -> Vec<(&'a ProviderProfile, &'a ProviderProfile)> {
    current
        .profiles
        .profiles
        .iter()
        .filter_map(|before| {
            if before.kind != ProviderKind::Local
                || Some(before.source.as_str()) != old.canonical_path.as_deref()
            {
                return None;
            }
            let after = candidate
                .profiles
                .profiles
                .iter()
                .find(|profile| profile.id == before.id)?;
            let mut expected = before.clone();
            expected.source = new.canonical_path.clone()?;
            expected.enabled = true;
            (after == &expected).then_some((before, after))
        })
        .collect()
}

fn fingerprint_pair(
    current: &Saved,
    candidate: &Saved,
    favorite: &Favorite,
    old: &BoundSource,
    new: &BoundSource,
    folders: (&RememberedFolders, &RememberedFolders),
    pairs: &[(&ProviderProfile, &ProviderProfile)],
) -> Option<(String, String)> {
    if favorite.origin.provider != "local" {
        return None;
    }
    let Locator::Local { root_alias, .. } = &favorite.locator else {
        return None;
    };
    if let Ok(before) = selection::resolve_profile(&current.profiles, favorite) {
        let (before, after) = pairs
            .iter()
            .copied()
            .find(|(profile, _)| *profile == before)?;
        if selection::resolve_profile(&candidate.profiles, favorite).ok()? != after {
            return None;
        }
        // Existing proof lookup is required for ambiguous legacy basenames;
        // multi-segment paths intentionally return None from item_for.
        let old_item = current
            .local_favorite_proofs
            .item_for(selection::SelectionOwner::Profile(before), favorite)
            .ok()?;
        let new_item = candidate
            .local_favorite_proofs
            .item_for(selection::SelectionOwner::Profile(after), favorite)
            .ok()?;
        if old_item != new_item {
            return None;
        }
        return Some((profile_fingerprint(before), profile_fingerprint(after)));
    }
    if !matches!(
        selection::resolve_profile(&current.profiles, favorite),
        Err(selection::BindingError::MissingProfile)
    ) || !matches!(
        selection::resolve_profile(&candidate.profiles, favorite),
        Err(selection::BindingError::MissingProfile)
    ) || favorite.origin.profile != "Local videos"
        || favorite.origin.subprofile.is_some()
        || root_alias != &old.name
        || favorite.origin.source_alias != old.name
        || folders
            .0
            .folders()
            .iter()
            .filter(|folder| folder.name == old.name)
            .count()
            != 1
        || folders
            .1
            .folders()
            .iter()
            .filter(|folder| folder.name == new.name)
            .count()
            != 1
    {
        return None;
    }
    let old_item = current
        .local_favorite_proofs
        .item_for(selection::SelectionOwner::LocalRoot(old), favorite)
        .ok()?;
    let new_item = candidate
        .local_favorite_proofs
        .item_for(selection::SelectionOwner::LocalRoot(new), favorite)
        .ok()?;
    if old_item != new_item {
        return None;
    }
    let after = transient::source_profile(new).ok()?;
    // Reconstruct only the old serialized profile fingerprint. No unavailable
    // root is made live or passed to a resolver as an authorized source.
    let mut before = after.clone();
    before.source = old.canonical_path.clone()?;
    Some((profile_fingerprint(&before), profile_fingerprint(&after)))
}

fn favorite_plan(
    current: &Saved,
    candidate: &Saved,
    old: &BoundSource,
    new: &BoundSource,
    before_folders: &RememberedFolders,
    after_folders: &RememberedFolders,
    pairs: &[(&ProviderProfile, &ProviderProfile)],
) -> Option<FavoriteRebase> {
    let resume = candidate.resume.get("favorites")?;
    let identity = resume.entry_id.strip_prefix("favorite:")?;
    let favorite = current
        .favorites
        .entries()
        .iter()
        .find(|favorite| favorite.identity() == identity)?;
    let (before, after) = fingerprint_pair(
        current,
        candidate,
        favorite,
        old,
        new,
        (before_folders, after_folders),
        pairs,
    )?;
    if resume.source_fingerprint != before && resume.source_fingerprint != after {
        return None;
    }
    Some(FavoriteRebase {
        entry_id: resume.entry_id.clone(),
        before,
        after,
    })
}

fn rebase_categories(
    current: &Saved,
    candidate: &mut Saved,
    pairs: &[(&ProviderProfile, &ProviderProfile)],
) -> Result<(), &'static str> {
    use category_preferences::{Preferences, Source};
    let invalid = |_| "Invalid category preferences";
    let original = Preferences::read(&current.settings).map_err(invalid)?;
    let mut preferences = Preferences::read(&candidate.settings).map_err(invalid)?;
    let mut changed = false;
    for (before, after) in pairs {
        let old_fingerprint = profile_fingerprint(before);
        let new_fingerprint = profile_fingerprint(after);
        if old_fingerprint == new_fingerprint {
            continue;
        }
        let from = Source {
            profile_id: &before.id,
            fingerprint: &old_fingerprint,
        };
        let to = Source {
            profile_id: &after.id,
            fingerprint: &new_fingerprint,
        };
        let active = current.profiles.active_profile.as_ref() == Some(&before.id)
            && candidate.profiles.active_profile.as_ref() == Some(&after.id);
        let remembered = if active {
            let mut seed = original.clone();
            Some(
                seed.switch_source(
                    Some(from),
                    Some(from),
                    &current.category,
                    current.favorites_selected,
                )
                .map_err(invalid)?
                .category,
            )
        } else {
            original
                .remembered(from)
                .map_err(invalid)?
                .map(str::to_owned)
        };
        if let Some(category) = remembered {
            preferences
                .choose_category(to, &category)
                .map_err(invalid)?;
            if active {
                candidate.category = category;
            }
            changed = true;
        }
    }
    if changed {
        preferences
            .write_into(&mut candidate.settings)
            .map_err(invalid)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "ott_usb_resume_tests.rs"]
mod tests;
