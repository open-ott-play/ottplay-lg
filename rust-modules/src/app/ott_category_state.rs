//! Runtime projection of the existing per-profile category preferences.
//! The one Saved writer owns publication; no catalog or Favorites copy lives here.
use super::*;
use category_preferences::{Preferences, Source};

fn profile(saved: &Saved) -> Option<(&str, String)> {
    saved.profiles.profiles.iter()
        .find(|p| Some(&p.id) == saved.profiles.active_profile.as_ref())
        .map(|p| (p.id.as_str(), profile_fingerprint(p)))
}
fn source<'a>(profile: &'a Option<(&'a str, String)>) -> Option<Source<'a>> {
    profile.as_ref().map(|(profile_id, fingerprint)| Source { profile_id, fingerprint })
}
fn invalid<T>(result: Result<T, category_preferences::Error>) -> Result<T, &'static str> {
    result.map_err(|_| "Invalid category preferences")
}

/// Read the same per-profile selection used by ordinary profile switches.
/// Only the exact active Saved source may seed legacy single-category state;
/// existing history (including All or a different fingerprint) wins. The local
/// preferences value is discarded, so binding a view never writes Saved.
pub(super) fn remembered_category(saved: &Saved, profile: &ProviderProfile) -> Result<String, &'static str> {
    let fingerprint = profile_fingerprint(profile);
    let source = Source { profile_id: &profile.id, fingerprint: &fingerprint };
    let active = saved.profiles.active_profile.as_ref() == Some(&profile.id)
        && saved.profiles.profiles.iter().filter(|p| p.id == profile.id).eq(std::iter::once(profile));
    let mut preferences = invalid(Preferences::read(&saved.settings))?;
    Ok(invalid(preferences.switch_source(active.then_some(source), Some(source),
        &saved.category, saved.favorites_selected))?.category)
}

/// Rebind only presentation metadata when a saved profile is renamed. The
/// shared full fingerprint remains unchanged for source/authority consumers.
/// Category history follows the same source through a display-name-only edit.
fn preserve_renamed_categories(before: &Saved, candidate: &Saved, preferences: &mut Preferences)
    -> Result<bool, &'static str>
{
    let mut changed = false;
    for old in &before.profiles.profiles {
        let Some(new) = candidate.profiles.profiles.iter().find(|new| new.id == old.id) else { continue; };
        if old.name == new.name { continue; }
        let mut renamed = old.clone();
        renamed.name = new.name.clone();
        if &renamed != new { continue; }
        let fingerprint = profile_fingerprint(new);
        let new_source = Source { profile_id: &new.id, fingerprint: &fingerprint };
        // A caller may already have prepared this candidate or deliberately
        // selected its category; never replace that exact after-image.
        if invalid(preferences.remembered(new_source))?.is_some() { continue; }
        let old_fingerprint = profile_fingerprint(old);
        let old_source = Source { profile_id: &old.id, fingerprint: &old_fingerprint };
        let category = if before.profiles.active_profile.as_ref() == Some(&old.id) {
            // Reuse legacy seeding and stale-fingerprint refusal; a label edit
            // cannot resurrect history already belonging to another source.
            invalid(preferences.switch_source(Some(old_source), Some(old_source),
                &before.category, before.favorites_selected))?.category
        } else {
            let Some(category) = invalid(preferences.remembered(old_source))? else { continue; };
            category.to_owned()
        };
        invalid(preferences.choose_category(new_source, &category))?;
        changed = true;
    }
    Ok(changed)
}

/// This runs on every candidate before the existing writer is admitted, so UI,
/// remote profile selection, profile edits, import and offline binding agree.
pub(super) fn prepare_profile_change(before: &Saved, candidate: &mut Saved) -> Result<(), &'static str> {
    candidate.profiles.validate().map_err(|_| "Invalid provider profiles")?;
    let old = profile(before);
    let new = profile(candidate);
    let mut preferences = invalid(Preferences::read(&candidate.settings))?;
    let renamed = preserve_renamed_categories(before, candidate, &mut preferences)?;
    let changed = old != new;
    if changed {
        let selection = invalid(preferences.switch_source(source(&old), source(&new),
            &before.category, before.favorites_selected))?;
        candidate.category = selection.category;
        candidate.favorites_selected = selection.favorites_selected;
    }
    let pruned = invalid(preferences.prune_deleted(&candidate.profiles))?;
    if changed || renamed || pruned != 0 { invalid(preferences.write_into(&mut candidate.settings))?; }
    let mut organization = catalog_organization::Document::read(&candidate.settings).map_err(|error| error.message())?;
    if organization.prune_deleted(&candidate.profiles).map_err(|error| error.message())? != 0 {
        organization.write_into(&mut candidate.settings).map_err(|error| error.message())?;
    }
    Ok(())
}

/// Existing one-writer acknowledgement, scoped to the actual UI/source owner.
/// The durable Favorites flag survives a retired UI; its late browse action does not.
pub(super) struct Pending {
    navigation: u64,
    screen: Option<InstanceId>,
    owner: Option<OwnerToken>,
    category: String,
    favorites: bool,
    source_context: String,
}

impl Pending {
    pub(super) fn capture(runtime: &Runtime, category: Option<&str>) -> Self {
        Self {
            navigation: runtime.navigation_epoch,
            screen: runtime.reply,
            owner: runtime.transient.accepted.as_ref().map(transient::TransientMedia::owner),
            category: category.unwrap_or_else(|| runtime.view_selection().0).to_owned(),
            favorites: category.is_none(),
            source_context: runtime.category_source_context(),
        }
    }
}

impl Runtime {
    /// Stable across catalog batches, but not across source replacement or
    /// leaving and returning to this list. Only the opaque digest reaches UI.
    pub(super) fn category_source_context(&self) -> String {
        let value = json!([
            self.remote.runtime(), self.generation,
            self.active(), self.effective_profile(), self.provider,
            self.transient.accepted.as_ref().map(transient::TransientMedia::owner).map(|owner| owner.0),
            self.saved.settings.get(catalog_organization::KEY),
        ]);
        plx_data::ott::favorites_wire::hash(value.to_string().as_bytes())
    }
    pub(super) fn category_action_context(&self) -> String {
        plx_data::ott::favorites_wire::hash(json!([
            self.category_source_context(), self.navigation_epoch
        ]).to_string().as_bytes())
    }
    pub(super) fn effective_category(&self) -> &str {
        let (remembered, favorites) = self.view_selection();
        if favorites { return ""; }
        if let Some(projection) = self.organization_projection() {
            return match projection.selection_id(remembered) { Some("") | None => "", Some(_) => remembered };
        }
        // Resolve structural keys and unique legacy labels through the same
        // catalogue projection used by the list and remote category receipts.
        let available = self.catalog.resolve_category(remembered).map(|_| remembered);
        category_preferences::catalog_selection(remembered, available, self.catalog_complete).effective()
    }

    pub(super) fn category_choice_candidate(&self, category: Option<&str>) -> Result<Saved, &'static str> {
        let mut saved = self.saved.clone();
        saved.favorites_selected = category.is_none();
        if let Some(category) = category {
            if category.len() > 4096 { return Err("Category is too long"); }
            if self.transient.accepted.is_none() && managed::profile(self).is_none() {
                if let Some(current) = classic_source::category_profile(self)? {
                    let fingerprint = profile_fingerprint(current);
                    let source = Source { profile_id: &current.id, fingerprint: &fingerprint };
                    let mut preferences = invalid(Preferences::read(&saved.settings))?;
                    let selection = invalid(preferences.choose_category(source, category))?;
                    if saved.profiles.active_profile.as_ref() == Some(&current.id) {
                        saved.category = selection.category;
                    }
                    invalid(preferences.write_into(&mut saved.settings))?;
                } else if classic_source::profile(self).is_none() {
                    if category.is_empty() { saved.category.clear(); }
                    else { return Err("Select a provider profile first"); }
                }
            }
        }
        Ok(saved)
    }

    pub(super) fn save_category_choice(&mut self, category: Option<String>) -> Result<(), &'static str> {
        self.save_category_choice_with_reply(category, None)
    }

    pub(super) fn save_remote_category_choice(&mut self, category: Option<String>,
        invocation: Invocation, response: Value) -> Result<(), &'static str> {
        self.save_category_choice_with_reply(category, Some((invocation, response)))
    }

    fn save_category_choice_with_reply(&mut self, category: Option<String>,
        reply: Option<(Invocation, Value)>) -> Result<(), &'static str> {
        let saved = self.category_choice_candidate(category.as_deref())?;
        let pending = Pending::capture(self, category.as_deref());
        let public = category.as_deref().map(|category| classic_bookmark::category_patch(self, category))
            .transpose()?.flatten();
        if let Some(value) = public {
            let (invocation, response) = reply.map(|(invocation, response)| (Some(invocation), response))
                .unwrap_or((None, Value::Null));
            self.save_public_preference(self.saved.clone(), public_preferences::Patch::ClassicBookmark(value.clone()),
                invocation, response)?;
            classic_bookmark::category_started(self, value);
        } else {
            match reply {
                Some((invocation, response)) => self.save_reply(saved, invocation, false, response)?,
                None => self.save(saved, None, false)?,
            }
        }
        self.pending_category_choice = Some(pending);
        Ok(())
    }

    pub(super) fn rebase_category_choice(&mut self) {
        if let Some(pending) = self.pending_category_choice.as_mut() {
            pending.navigation = self.navigation_epoch;
        }
    }

    pub(super) fn finish_category_choice(&mut self, app: &App, success: bool) -> bool {
        let Some(pending) = self.pending_category_choice.take() else { return false; };
        if !success || pending.navigation != self.navigation_epoch
            || pending.source_context != self.category_source_context()
            || (!pending.favorites && !pending.category.is_empty() && {
                let current = self.organization_category_id(&pending.category);
                !self.organization_categories().iter().any(|row| row.id == current)
            })
            || pending.owner != self.transient.accepted.as_ref().map(transient::TransientMedia::owner)
            || pending.screen.is_some_and(|screen| current_ui_owner(app) != Some(screen)
                || self.reply != Some(screen))
        { return false; }
        if pending.owner.is_some() {
            transient::set_view(self, &pending.category, pending.favorites);
        } else if managed::profile(self).is_some() {
            managed::set_view(self, &pending.category, pending.favorites);
        } else if classic_source::profile(self).is_some() {
            classic_source::set_view(self, &pending.category);
            if !pending.favorites { classic_bookmark::record_category(self, &pending.category); }
        }
        // An explicit list choice opens its whole list, including a reselected
        // All/Favorites whose source context is unchanged. Clear only after the
        // durable, current choice is accepted; a failed/stale choice keeps Search.
        self.search = search::Search::default();
        self.page = if pending.favorites { "favorites" } else { "channels" };
        self.changed();
        true
    }
}

/// Organization and affected selection are staged together, without invoking
/// the explicit-choice API that intentionally turns global Favorites off.
pub(super) fn organization_remap(saved: &mut Saved, profile: &ProviderProfile,
    before: &catalog_organization::Projection, after: &catalog_organization::Projection) -> Result<(), &'static str> {
    let fingerprint = profile_fingerprint(profile);
    let source = Source { profile_id: &profile.id, fingerprint: &fingerprint };
    let remap = |current: &str| -> Option<String> {
        let id = before.selection_id(current)?;
        Some(if after.category(id).is_some() { id.into() } else { String::new() })
    };
    let mut preferences = invalid(Preferences::read(&saved.settings))?;
    if let Some(remembered) = invalid(preferences.remembered(source))?.map(str::to_owned) {
        if let Some(next) = remap(&remembered) {
            if invalid(preferences.remap_category(source, &remembered, &next))? {
                invalid(preferences.write_into(&mut saved.settings))?;
            }
        }
    }
    if saved.profiles.active_profile.as_ref() == Some(&profile.id) {
        if let Some(next) = remap(&saved.category) { saved.category = next; }
    }
    Ok(())
}

/// Resolve only source-bound UI rows. The existing unscoped internal/wire
/// action and the global Favorites intent keep their original semantics.
/// Refuse before any writer, editor cancellation or navigation can start.
pub(super) fn resolve_action(runtime: &Runtime, action: OttAction) -> Result<OttAction, &'static str> {
    let OttAction::SelectCategoryAt { context, category } = action else { return Ok(action); };
    if !runtime.loaded || runtime.persistence_uncertain || runtime.page != "categories"
        || context != runtime.category_action_context()
        || (!category.is_empty() && !runtime.organization_categories().iter().any(|row| row.id == category))
    {
        return Err("The category list changed; select the category again");
    }
    Ok(OttAction::SelectCategory(category))
}
