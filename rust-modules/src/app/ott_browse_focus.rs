//! Private browse selection, separate from playback ownership and resume points.
//! Arrow/hover changes stay in RAM; existing save/exit boundaries persist them.
use super::*;

const KEY: &str = "nativeBrowseFocusV1";
const MAX_PROFILES: usize = 90;
const MAX_TEXT: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    fingerprint: String,
    category: String,
    entry_id: String,
}
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Stored {
    profiles: BTreeMap<String, Record>,
    favorite: Option<String>,
}
impl Stored {
    fn read(saved: &Saved) -> Result<Self, &'static str> {
        let Some(value) = saved.settings.get(KEY) else { return Ok(Self::default()); };
        if serde_json::to_vec(value).map_err(|_| "Invalid browse focus")?.len() > 512 * 1024 {
            return Err("Browse focus exceeds storage limit");
        }
        let stored: Self = serde_json::from_value(value.clone()).map_err(|_| "Invalid browse focus")?;
        if stored.profiles.len() > MAX_PROFILES || stored.profiles.iter().any(|(key, record)| {
            key.is_empty() || key.len() > 128 || record.entry_id.is_empty()
                || record.entry_id.len() > MAX_TEXT || record.category.len() > MAX_TEXT
                || record.fingerprint.len() != 64
                || !record.fingerprint.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        }) || stored.favorite.as_ref().is_some_and(|id|
            id.len() != 64 || !id.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)))
        { return Err("Invalid browse focus"); }
        Ok(stored)
    }
    fn write(self, saved: &mut Saved) -> Result<(), &'static str> {
        if self.profiles.is_empty() && self.favorite.is_none() { saved.settings.remove(KEY); }
        else {
            let value = serde_json::to_value(self).map_err(|_| "Invalid browse focus")?;
            if serde_json::to_vec(&value).map_err(|_| "Invalid browse focus")?.len() > 512 * 1024 {
                return Ok(());
            }
            let mut settings = saved.settings.clone(); settings.insert(KEY.into(), value);
            // Optional browse metadata must not make an otherwise valid settings
            // write exceed its existing budget. Uncommitted focus remains in RAM.
            if serde_json::to_vec(&settings).map_err(|_| "Invalid browse focus")?.len() <= 1024 * 1024 {
                saved.settings = settings;
            }
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq)]
struct BoundRecord { profile: ProviderProfile, record: Record }
#[derive(Clone, PartialEq)]
struct Temporary {
    profile: ProviderProfile,
    owner: OwnerToken,
    category: String,
    entry_id: String,
}
#[derive(Clone, PartialEq, Eq)]
struct FavoriteFocus { id: String, digest: String }
#[derive(Default)]
pub(super) struct State {
    committed: BTreeMap<String, BoundRecord>,
    favorite: Option<FavoriteFocus>,
    pending: BTreeMap<String, BoundRecord>,
    pending_favorite: Option<FavoriteFocus>,
    temporary: Option<Temporary>,
    classic: Option<BoundRecord>,
    flush_requested: bool,
}
fn current_profile<'a>(saved: &'a Saved, id: &str) -> Option<&'a ProviderProfile> {
    saved.profiles.profiles.iter().find(|profile| profile.id == id && profile.enabled)
}
fn favorite_focus(saved: &Saved, digest: &str) -> Option<FavoriteFocus> {
    saved.favorites.entries().iter().find_map(|favorite| {
        let id = format!("favorite:{}", favorite.identity());
        (plx_data::ott::favorites_wire::hash(id.as_bytes()) == digest)
            .then(|| FavoriteFocus { id, digest: digest.into() })
    })
}


impl State {
    /// A late successful save can acknowledge only its exact snapshot. Newer
    /// uncommitted focus survives; source replacement cannot inherit old focus.
    fn reconcile(&mut self, saved: &Saved) {
        let stored = Stored::read(saved).unwrap_or_default();
        self.committed = stored.profiles.into_iter().filter_map(|(id, record)| {
            let profile = current_profile(saved, &id)?;
            (record.fingerprint == profile_fingerprint(profile))
                .then(|| (id, BoundRecord { profile: profile.clone(), record }))
        }).collect();
        self.favorite = stored.favorite.and_then(|digest| favorite_focus(saved, &digest));
        self.pending.retain(|id, pending| {
            current_profile(saved, id) == Some(&pending.profile)
                && self.committed.get(id) != Some(pending)
        });
        if self.pending_favorite.as_ref().is_some_and(|value|
            self.favorite.as_ref() == Some(value) || favorite_focus(saved, &value.digest).is_none()) {
            self.pending_favorite = None;
        }
        if !self.dirty() { self.flush_requested = false; }
    }
    fn dirty(&self) -> bool { !self.pending.is_empty() || self.pending_favorite.is_some() }
    fn profile(&mut self, profile: &ProviderProfile, category: &str, id: &str) -> bool {
        if id.is_empty() || id.len() > MAX_TEXT || category.len() > MAX_TEXT { return false; }
        let bound = BoundRecord { profile: profile.clone(), record: Record {
            fingerprint: profile_fingerprint(profile), category: category.into(), entry_id: id.into(),
        }};
        if self.pending.get(&profile.id).or_else(|| self.committed.get(&profile.id)) == Some(&bound) { return false; }
        self.pending.insert(profile.id.clone(), bound);
        self.temporary = None;
        true
    }
    fn favorite(&mut self, id: &str) -> bool {
        if !id.starts_with("favorite:")
            || self.pending_favorite.as_ref().or(self.favorite.as_ref()).is_some_and(|old| old.id == id) { return false; }
        self.pending_favorite = Some(FavoriteFocus { id: id.into(), digest: plx_data::ott::favorites_wire::hash(id.as_bytes()) });
        self.temporary = None;
        true
    }
    fn temporary(&mut self, profile: &ProviderProfile, owner: OwnerToken, category: &str, id: &str) -> bool {
        let value = Temporary { profile: profile.clone(), owner, category: category.into(), entry_id: id.into() };
        if self.temporary.as_ref() == Some(&value) { return false; }
        self.temporary = Some(value);
        true
    }
    fn selected(&self, profile: Option<&ProviderProfile>, owner: Option<OwnerToken>, category: &str, favorites: bool) -> Option<&str> {
        if favorites { return self.pending_favorite.as_ref().or(self.favorite.as_ref()).map(|value| value.id.as_str()); }
        let profile = profile?;
        if let Some(owner) = owner {
            return self.temporary.as_ref().filter(|value|
                value.profile == *profile && value.owner == owner && value.category == category)
                .map(|value| value.entry_id.as_str());
        }
        self.classic.as_ref().filter(|value| value.profile == *profile && value.record.category == category)
            .or_else(|| self.pending.get(&profile.id).or_else(|| self.committed.get(&profile.id)))
            .filter(|value| value.profile == *profile && value.record.category == category)
            .map(|value| value.record.entry_id.as_str())
    }
    /// Merge with the writer's candidate, not the older committed image. Only
    /// private identities and digests are serialized; no path grants or secrets.
    fn prepare(&self, saved: &mut Saved) -> Result<(), &'static str> {
        // Corrupt optional presentation metadata cannot block unrelated source,
        // Favorites, checkpoint or security settings. Rebuild this key alone.
        let mut stored = Stored::read(saved).unwrap_or_default();
        stored.profiles.retain(|id, record| current_profile(saved, id)
            .is_some_and(|profile| record.fingerprint == profile_fingerprint(profile)));
        if stored.favorite.as_ref().is_some_and(|digest| favorite_focus(saved, digest).is_none()) { stored.favorite = None; }
        for (id, value) in &self.pending {
            if current_profile(saved, id) == Some(&value.profile) {
                stored.profiles.insert(id.clone(), value.record.clone());
            }
        }
        if let Some(value) = self.pending_favorite.as_ref().filter(|value| favorite_focus(saved, &value.digest).is_some()) {
            stored.favorite = Some(value.digest.clone());
        }
        stored.write(saved)
    }
}

/// Preserve the separate browse hash domain (SHA-256 of `favorite:` + identity).
fn renamed_digest(plan: &profile_rename::Plan, digest: &str) -> Option<String> {
    plan.entries.iter().find_map(|(old, new)| {
        let old = format!("favorite:{}", old.identity());
        (plx_data::ott::favorites_wire::hash(old.as_bytes()) == digest).then(|| {
            let new = format!("favorite:{}", new.identity());
            plx_data::ott::favorites_wire::hash(new.as_bytes())
        })
    })
}

pub(super) fn rename_saved(saved: &mut Saved, plan: &profile_rename::Plan) -> Result<(), &'static str> {
    let mut stored = Stored::read(saved)?;
    if let Some(record) = stored.profiles.get_mut(&plan.before.id)
        .filter(|record| record.fingerprint == profile_fingerprint(&plan.before)) {
        record.fingerprint = profile_fingerprint(&plan.after);
    }
    if let Some(digest) = stored.favorite.as_deref().and_then(|value| renamed_digest(plan, value)) {
        stored.favorite = Some(digest);
    }
    let expected = stored.clone();
    stored.write(saved)?;
    // Optional ordinary writes may omit an oversized focus value. An identity
    // migration must instead refuse atomically, never retain the stale digest.
    if Stored::read(saved)? != expected { return Err("Renamed browse focus exceeds storage limit"); }
    Ok(())
}

pub(super) fn rename_pending(runtime: &mut Runtime, plan: &profile_rename::Plan) {
    let state = &mut runtime.browse_focus;
    for value in state.committed.values_mut().chain(state.pending.values_mut()).chain(state.classic.iter_mut()) {
        if value.profile == plan.before && value.record.fingerprint == profile_fingerprint(&plan.before) {
            value.profile = plan.after.clone();
            value.record.fingerprint = profile_fingerprint(&plan.after);
        }
    }
    for value in state.favorite.iter_mut().chain(state.pending_favorite.iter_mut()) {
        if let Some(id) = plan.selection(&value.id) {
            value.digest = plx_data::ott::favorites_wire::hash(id.as_bytes());
            value.id = id;
        }
    }
}

pub(super) fn reconcile(runtime: &mut Runtime) { runtime.browse_focus.reconcile(&runtime.saved); }
pub(super) fn prepare(runtime: &Runtime, saved: &mut Saved) -> Result<(), &'static str> { runtime.browse_focus.prepare(saved) }
pub(super) fn request_flush(runtime: &mut Runtime) {
    classic_bookmark::request_flush(runtime);
    runtime.browse_focus.flush_requested |= runtime.browse_focus.dirty();
}
pub(super) fn flush(runtime: &mut Runtime) {
    if !runtime.browse_focus.flush_requested || runtime.saving || !runtime.loaded || runtime.persistence_uncertain { return; }
    // One attempt per boundary. Failure retains RAM focus for a later explicit
    // boundary; it never becomes a retry loop or a claim of durable success.
    runtime.browse_focus.flush_requested = false;
    let _ = runtime.save(runtime.saved.clone(), None, false);
}

pub(super) fn record(runtime: &mut Runtime, id: &str) {
    let favorites = runtime.view_selection().1;
    if !favorites && classic_source::binding_profile(runtime).is_some() {
        let profile = classic_source::binding_profile(runtime).unwrap().clone();
        let category = runtime.effective_category().to_owned();
        if !id.is_empty() && id.len() <= MAX_TEXT && category.len() <= MAX_TEXT {
            runtime.browse_focus.classic = Some(BoundRecord { record: Record {
                fingerprint: profile_fingerprint(&profile), category, entry_id: id.into(),
            }, profile });
        }
        classic_bookmark::record_focus(runtime, id);
        runtime.changed();
        return;
    }
    let changed = if favorites {
        runtime.browse_focus.favorite(id)
    } else if let Some(profile) = runtime.effective_profile().cloned() {
        let category = runtime.effective_category().to_owned();
        if let Some(owner) = runtime.transient.accepted.as_ref().map(transient::TransientMedia::owner) {
            runtime.browse_focus.temporary(&profile, owner, &category, id)
        } else {
            runtime.browse_focus.profile(&profile, &category, id)
        }
    } else { false };
    if changed { runtime.changed(); }
}

pub(super) fn selected(runtime: &Runtime, rows: &[OttRow]) -> Option<String> {
    if !matches!(runtime.page, "channels" | "favorites") { return None; }
    let temporary = if !runtime.view_selection().1 && classic_source::binding_profile(runtime).is_some() {
        classic_bookmark::selected(runtime)
    } else { None };
    let id = temporary.as_deref().or_else(|| runtime.browse_focus.selected(runtime.effective_profile(),
        runtime.transient.accepted.as_ref().map(transient::TransientMedia::owner),
        runtime.effective_category(), runtime.view_selection().1))?;
    if let Some(row) = rows.iter().find(|row| row.id == id) {
        return row.enabled.then(|| id.to_owned());
    }
    // Keep the exact target through incomplete discovery. The screen uses it
    // once it arrives; no old row number is carried to a replacement list.
    (!runtime.selection_complete()).then(|| id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile(id: &str) -> ProviderProfile {
        ProviderProfile { id: id.into(), name: id.into(), kind: ProviderKind::M3u,
            source: format!("https://media.invalid/{id}?private=secret"), source_alias: id.into(), enabled: true }
    }
    fn saved() -> Saved {
        let mut saved = Saved::default();
        saved.profiles.profiles = vec![profile("a"), profile("b")];
        saved.profiles.active_profile = Some("a".into());
        saved
    }
    fn local_favorite(path: &str) -> Favorite {
        Favorite { title: "Video".into(), origin: plx_data::ott::favorites::Origin {
            provider: "local".into(), profile: "Local videos".into(), subprofile: None, source_alias: "Movies".into(),
        }, locator: plx_data::ott::favorites::Locator::Local { root_alias: "Movies".into(), relative_path: path.into() } }
    }
    #[test]
    fn focus_roundtrip_never_changes_playback_resume_profiles_or_category() {
        let mut saved = saved();
        saved.category = "News".into();
        saved.resume.insert("a".into(), SavedResume { entry_id: "playing".into(), position_ms: 12_500, source_fingerprint: profile_fingerprint(&profile("a")) });
        let original = serde_json::to_value(&saved).unwrap();
        let mut state = State::default();
        assert!(state.profile(&profile("a"), "News", "browsed"));
        state.prepare(&mut saved).unwrap();
        let mut persisted = serde_json::to_value(&saved).unwrap();
        persisted["settings"].as_object_mut().unwrap().remove(KEY);
        assert_eq!(persisted, original);
        let encoded = serde_json::to_string(&saved).unwrap();
        let reloaded: Saved = serde_json::from_str(&encoded).unwrap();
        let mut restarted = State::default(); restarted.reconcile(&reloaded);
        assert_eq!(restarted.selected(Some(&profile("a")), None, "News", false), Some("browsed"));
        assert_eq!(restarted.selected(Some(&profile("a")), None, "Movies", false), None);
        let focus = serde_json::to_string(&reloaded.settings[KEY]).unwrap();
        assert!(!focus.contains("private") && !focus.contains("https:"));
    }
    #[test]
    fn late_save_ack_does_not_erase_newer_focus_and_final_merge_keeps_other_mutations() {
        let saved = saved(); let mut state = State::default();
        state.profile(&profile("a"), "", "older");
        let mut in_flight = saved.clone(); state.prepare(&mut in_flight).unwrap();
        state.profile(&profile("a"), "", "newer");
        state.reconcile(&in_flight);
        assert_eq!(state.selected(Some(&profile("a")), None, "", false), Some("newer"));
        assert!(state.dirty());
        let mut latest = in_flight;
        latest.settings.insert("unrelated".into(), json!("keep"));
        state.prepare(&mut latest).unwrap();
        assert_eq!(latest.settings["unrelated"], "keep");
        state.reconcile(&latest);
        assert!(!state.dirty());
        assert_eq!(state.selected(Some(&profile("a")), None, "", false), Some("newer"));
    }
    #[test]
    fn equal_ids_in_other_profiles_or_replaced_sources_do_not_inherit_focus() {
        let mut saved = saved(); let mut state = State::default();
        state.profile(&profile("a"), "", "shared"); state.prepare(&mut saved).unwrap(); state.reconcile(&saved);
        assert_eq!(state.selected(Some(&profile("b")), None, "", false), None);
        let mut replacement = profile("a"); replacement.source = "https://replacement.invalid/list".into();
        assert_eq!(state.selected(Some(&replacement), None, "", false), None);
        saved.profiles.profiles[0] = replacement;
        state.reconcile(&saved); state.prepare(&mut saved).unwrap();
        assert!(saved.settings.get(KEY).is_none());
    }
    #[test]
    fn temporary_focus_stays_in_ram_but_global_favorite_is_installation_scoped() {
        let mut saved = saved(); let favorite = local_favorite("film.mp4");
        let id = format!("favorite:{}", favorite.identity()); saved.favorites.toggle(favorite).unwrap();
        let mut state = State::default();
        state.temporary(&profile("a"), OwnerToken(7), "Movies", "temporary");
        state.prepare(&mut saved).unwrap(); assert!(!saved.settings.contains_key(KEY));
        assert_eq!(state.selected(Some(&profile("a")), Some(OwnerToken(7)), "Movies", false), Some("temporary"));
        assert_eq!(state.selected(Some(&profile("a")), Some(OwnerToken(8)), "Movies", false), None);
        state.favorite(&id); state.prepare(&mut saved).unwrap();
        let mut restarted = State::default(); restarted.reconcile(&saved);
        assert_eq!(restarted.selected(Some(&profile("b")), None, "Other", true), Some(id.as_str()));
        assert_eq!(restarted.selected(None, None, "", true), Some(id.as_str()));
    }
    #[test]
    fn long_valid_favorite_identity_is_saved_as_private_digest_without_truncation() {
        let favorite = local_favorite(&("\"".repeat(6000) + ".mp4")); favorite.validate().unwrap();
        let id = format!("favorite:{}", favorite.identity()); assert!(id.len() > 8192);
        let mut saved = saved(); saved.favorites.toggle(favorite).unwrap();
        let mut state = State::default(); state.favorite(&id); state.prepare(&mut saved).unwrap();
        let focus = &saved.settings[KEY]; assert_eq!(focus["favorite"].as_str().unwrap().len(), 64);
        assert!(serde_json::to_vec(focus).unwrap().len() < 128);
        let mut restarted = State::default(); restarted.reconcile(&saved);
        assert_eq!(restarted.selected(None, None, "", true), Some(id.as_str()));
    }
    #[test]
    fn corrupt_optional_focus_is_discarded_without_rejecting_or_weakening_other_state() {
        let mut saved = saved(); saved.settings.insert(KEY.into(), json!({"unexpected":"bad"}));
        saved.settings.insert("unrelated".into(), json!(123));
        let mut state = State::default(); state.reconcile(&saved);
        assert_eq!(state.selected(Some(&profile("a")), None, "", false), None);
        state.prepare(&mut saved).unwrap();
        assert!(!saved.settings.contains_key(KEY)); assert_eq!(saved.settings["unrelated"], 123);
        assert_eq!(saved.profiles.profiles[0], profile("a"));
    }
}
