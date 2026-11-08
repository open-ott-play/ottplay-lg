//! Exact, receiver-attested name-only migration in the existing Saved writer.
//! The portable resolver stays strict. No alias history or second writer exists.
use super::*;
use plx_data::ott::favorites::{Document, Locator};

const REFUSED: &str = "Profile rename cannot preserve its current Favorites bindings";
const BUSY: &str = "Stop using this profile before renaming it";

pub(super) struct Plan {
    pub(super) before: ProviderProfile,
    pub(super) after: ProviderProfile,
    pub(super) entries: Vec<(Favorite, Favorite)>,
    document: Document,
    bound_selections: Vec<String>,
}

#[derive(PartialEq, Eq)]
enum Binding { Profile(String), Local(String), Attachment(String) }

fn binding(runtime: &Runtime, profiles: &ProviderProfiles, favorite: &Favorite)
    -> Result<Binding, selection::BindingError> {
    // Mirror the ordinary favorite binder's complete source snapshot; in
    // particular, a dormant attachment or LocalRoot must not acquire a profile
    // merely because its display label changed.
    let bindings = if favorite.origin.provider == "local" {
        selection::SelectionBindings::new(profiles,
            runtime.local_inventory.sources.iter().map(crate::app::ott_local_root::LocalSource::descriptor),
            runtime.local_inventory.generation())?
    } else { selection::SelectionBindings::profiles_only(profiles) };
    let bindings = bindings.with_temporary_profile(managed::binding_profile(runtime)
        .or_else(|| classic_source::binding_profile(runtime))).with_attachments(&runtime.m3u_attachments);
    Ok(match selection::bind_bound(&bindings, favorite)? {
        selection::SelectionOwner::Profile(profile) => Binding::Profile(profile.id.clone()),
        selection::SelectionOwner::LocalRoot(root) => Binding::Local(root.id.clone()),
        selection::SelectionOwner::M3uAttachment(receipt) => Binding::Attachment(receipt.parent().id.clone()),
    })
}

impl Plan {
    fn new(runtime: &Runtime, candidate: &Saved) -> Result<Option<Self>, &'static str> {
        let before = &runtime.saved;
        before.profiles.validate().map_err(|_| REFUSED)?;
        candidate.profiles.validate().map_err(|_| REFUSED)?;
        let renamed: Vec<_> = before.profiles.profiles.iter().filter_map(|old| {
            let new = candidate.profiles.profiles.iter().find(|new| new.id == old.id)?;
            let mut same = old.clone(); same.name = new.name.clone();
            (old.name != new.name && same == *new).then_some((old, new))
        }).collect();
        if renamed.is_empty() { return Ok(None); }
        // A combined source/document import is not an attested rename command.
        // Fail before publication instead of partly migrating the transaction.
        if renamed.len() != 1 || before.favorites != candidate.favorites
            || before.favorite_items != candidate.favorite_items
            || before.local_favorite_proofs != candidate.local_favorite_proofs {
            return Err(REFUSED);
        }
        let (old, new) = renamed[0];
        let mut profiles = before.profiles.clone();
        *profiles.profiles.iter_mut().find(|profile| profile.id == old.id).ok_or(REFUSED)? = new.clone();
        if profiles != candidate.profiles || !old.enabled { return Err(REFUSED); }
        let registry = crate::app::ott_m3u_attachment::Registry::read(&before.settings, &before.profiles)
            .map_err(|_| REFUSED)?;
        let next_registry = crate::app::ott_m3u_attachment::Registry::read(&candidate.settings, &candidate.profiles)
            .map_err(|_| REFUSED)?;
        if registry.for_parent(&old.id).is_some() || next_registry.for_parent(&old.id).is_some() {
            return Err(REFUSED);
        }
        // Kiosk can hold a source origin without a document member or decoder.
        // This first migration supports idle non-kiosk name edits only.
        for saved in [before, candidate] {
            if let Some(kiosk) = crate::app::ott_kiosk_stored::read(&saved.settings).map_err(|_| REFUSED)? {
                if kiosk.origin().provider == old.kind.canonical_id()
                    && [old.name.as_str(), new.name.as_str()].contains(&kiosk.origin().profile.as_str())
                    || kiosk.pins().iter().any(|favorite| binding(runtime, &before.profiles, favorite) == Ok(Binding::Profile(old.id.clone()))) {
                    return Err(REFUSED);
                }
            }
        }
        let mut entries = Vec::new();
        let mut bound_selections = Vec::new();
        let mut rows = Vec::with_capacity(before.favorites.entries().len());
        for favorite in before.favorites.entries() {
            let prior = binding(runtime, &before.profiles, favorite);
            if prior == Ok(Binding::Profile(old.id.clone())) {
                bound_selections.push(format!("favorite:{}", favorite.identity()));
            }
            let mut next = favorite.clone();
            let canonical = matches!(&favorite.locator, Locator::Plex { server_id, .. }
                if favorite.origin.profile == "Plex" && favorite.origin.source_alias == *server_id)
                || matches!(prior, Ok(Binding::Local(_)));
            if prior == Ok(Binding::Profile(old.id.clone())) && !canonical {
                next.origin.profile = new.name.clone();
                // Local's rootAlias/sourceAlias are source identity, not display name.
                // Other kinds may retain an explicit alias; only replace an old
                // name-derived alias when the exact new owner otherwise fails.
                if binding(runtime, &candidate.profiles, &next) != Ok(Binding::Profile(old.id.clone()))
                    && !matches!(next.locator, Locator::Local { .. })
                    && next.origin.source_alias == old.name {
                    next.origin.source_alias = new.name.clone();
                }
                if binding(runtime, &candidate.profiles, &next) != Ok(Binding::Profile(old.id.clone())) { return Err(REFUSED); }
            } else if binding(runtime, &candidate.profiles, &next) != prior {
                // Also reject accidentally acquiring another missing Favorite,
                // redirecting an existing one, or introducing ambiguity.
                return Err(REFUSED);
            }
            if next != *favorite { entries.push((favorite.clone(), next.clone())); }
            rows.push(next);
        }
        let document = Document::parse(&serde_json::to_vec(&json!({"version":1,"entries":rows}))
            .map_err(|_| REFUSED)?).map_err(|_| REFUSED)?;
        // With no affected Favorite and no changed collateral binding, retain
        // the existing ordinary profile-edit path (including active profiles).
        if bound_selections.is_empty() { return Ok(None); }
        Ok(Some(Self { before: old.clone(), after: new.clone(), entries, document, bound_selections }))
    }

    pub(super) fn selection(&self, value: &str) -> Option<String> {
        self.entries.iter().find_map(|(old, new)|
            (value.strip_prefix("favorite:") == Some(old.identity().as_str()))
                .then(|| format!("favorite:{}", new.identity())))
    }

    fn checkpoints(&self, values: &mut BTreeMap<String, SavedResume>) {
        let old = profile_fingerprint(&self.before);
        let new = profile_fingerprint(&self.after);
        for (key, point) in values {
            if point.source_fingerprint != old { continue; }
            if key == &self.before.id {
                point.source_fingerprint = new.clone();
            } else if key == "favorites" && self.bound_selections.contains(&point.entry_id) {
                if let Some(id) = self.selection(&point.entry_id) { point.entry_id = id; }
                point.source_fingerprint = new.clone();
            }
        }
    }

    fn apply(&self, saved: &mut Saved) -> Result<(), &'static str> {
        let ids = favorite_item_ids::ordered(saved)?;
        favorite_item_ids::rename(saved, &self.entries)?;
        saved.favorites = self.document.clone();
        if favorite_item_ids::ordered(saved)? != ids { return Err(REFUSED); }
        saved.local_favorite_proofs.rename_profile(&self.before, &self.after, &self.entries)
            .map_err(|_| REFUSED)?;
        self.checkpoints(&mut saved.resume);
        browse_focus::rename_saved(saved, self)?;
        Ok(())
    }
}

/// Prepare on a clone: every rejection leaves both live and candidate images intact.
/// Capture RAM values against their old owner BEFORE changing their fingerprints.
pub(super) fn prepare(runtime: &Runtime, candidate: &mut Saved) -> Result<Option<Plan>, &'static str> {
    let Some(plan) = Plan::new(runtime, candidate)? else { return Ok(None); };
    let affected = |profile: &ProviderProfile| profile.id == plan.before.id;
    if runtime.active().is_some_and(affected)
        || runtime.effective_profile().is_some_and(affected)
        || runtime.playing.as_ref().is_some_and(|playing| affected(&playing.profile))
        || runtime.pending_play_profile().is_some_and(affected)
        || runtime.resolver_next.as_ref().is_some_and(|request| affected(&request.profile))
        || runtime.catalog_next.as_ref().is_some_and(affected)
        || runtime.resume_pending.as_ref().is_some_and(|request| plan.bound_selections.contains(&request.id)) {
        return Err(BUSY);
    }
    let mut staged = candidate.clone();
    staged.profiles = runtime.saved.profiles.clone();
    browse_focus::prepare(runtime, &mut staged)?;
    kiosk_resume::merge(&mut staged, &runtime.checkpoints);
    staged.profiles = candidate.profiles.clone();
    plan.apply(&mut staged)?;
    *candidate = staged;
    Ok(Some(plan))
}

/// A reservation shares the existing single writer's lifetime. Check before
/// modifying request ownership, and again at the common queue/native gates.
pub(super) fn guard(runtime: &Runtime, profile: &ProviderProfile) -> Result<(), &'static str> {
    if runtime.pending_profile_rename.as_ref().is_some_and(|plan| plan.before.id == profile.id) {
        Err(BUSY)
    } else { Ok(()) }
}

/// The same terminal settlement is used by normal acknowledgement and shutdown.
/// New unrelated navigation/focus/checkpoints survive; no request is replayed.
pub(super) fn settle(runtime: &mut Runtime, saved: Option<&Saved>) {
    let Some(plan) = runtime.pending_profile_rename.take() else { return; };
    let Some(saved) = saved else { return; };
    if saved.profiles.profiles.iter().find(|profile| profile.id == plan.after.id) != Some(&plan.after)
        || saved.favorites != plan.document { return; }
    plan.checkpoints(&mut runtime.checkpoints);
    browse_focus::rename_pending(runtime, &plan);
    if let Some(id) = runtime.selected.as_deref().and_then(|id| plan.selection(id)) {
        runtime.selected = Some(id);
    }
}
