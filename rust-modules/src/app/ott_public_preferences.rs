//! The narrow, credential-free preference door. Protected state keeps its own
//! authority and is never replaced with a public document after a failed read.
use super::*;
use plx_platform::storage::{public_preferences as store, CommitReceipt};
pub(super) use store::{Patch, Snapshot};

const ADDRESS: &str = "nativeRemoteServerAddress";
const MODE: &str = "interfaceMode";
const MODE_REVISION: &str = "settingRevision:interfaceMode";

#[derive(Default)]
pub(super) struct State {
    snapshot: Option<Snapshot>,
    loaded: bool,
    failure: Option<&'static str>,
    uncertain: bool,
}
impl State {
    pub(super) fn accept(&mut self, snapshot: Option<Snapshot>) {
        if let Some(old) = &self.snapshot {
            // A load begun before an admitted write cannot roll that write back.
            if snapshot
                .as_ref()
                .is_none_or(|new| (new.epoch, new.revision) < (old.epoch, old.revision))
            {
                return;
            }
        }
        self.snapshot = snapshot;
        self.loaded = true;
        self.failure = None;
    }
}

pub(super) enum WriteFailure {
    Rejected(private::Error),
    Expired,
    Uncertain,
}

pub(super) fn load() -> Result<Option<Snapshot>, &'static str> {
    std::panic::catch_unwind(store::load)
        .map_err(|_| "Could not read player preferences; restart the player to retry")?
        .map_err(|_| "Could not read player preferences; restart the player to retry")
}
pub(super) fn write(patch: Patch, permit: &private::WritePermit) -> Result<Snapshot, WriteFailure> {
    match store::patch_guarded(patch, permit) {
        Ok((snapshot, CommitReceipt::Durable)) => Ok(snapshot),
        Ok((_, CommitReceipt::Uncertain { .. })) | Err(store::SaveError::Uncertain) => {
            Err(WriteFailure::Uncertain)
        }
        Err(store::SaveError::NotApplied(error)) => Err(WriteFailure::Rejected(error)),
    }
}

/// Only explicit, validated fields can cross the public door. In particular,
/// a simultaneous provider, token, source, Favorites or playback edit cannot.
pub(super) fn validate_candidate(
    before: &Saved,
    candidate: &Saved,
    patch: &Patch,
) -> Result<(), &'static str> {
    if matches!(patch, Patch::NotificationsEnabled(_)) {
        let before = serde_json::to_value(before).map_err(|_| "Invalid player state")?;
        let candidate = serde_json::to_value(candidate).map_err(|_| "Invalid player state")?;
        return if before == candidate { Ok(()) }
            else { Err("A protected change cannot use player preferences") };
    }
    if let Patch::ClassicBookmark(bookmark) = patch {
        bookmark.validate().map_err(|_| "Invalid Classic bookmark")?;
        let before = serde_json::to_value(before).map_err(|_| "Invalid player state")?;
        let candidate = serde_json::to_value(candidate).map_err(|_| "Invalid player state")?;
        return if before == candidate { Ok(()) }
            else { Err("A protected change cannot use player preferences") };
    }
    let (field, expected) = match patch {
        Patch::RemoteAddress(address) => {
            if Remote::normalize_address(address)? != *address {
                return Err("Invalid server address");
            }
            (ADDRESS, json!(address))
        }
        Patch::InterfaceMode(mode) if *mode <= 1 => (MODE, json!(mode)),
        _ => return Err("Unsupported player preference"),
    };
    if candidate.settings.get(field) != Some(&expected) {
        return Err("Player preference changed before saving");
    }
    let mut old = serde_json::to_value(before).map_err(|_| "Invalid player state")?;
    let mut next = serde_json::to_value(candidate).map_err(|_| "Invalid player state")?;
    for state in [&mut old, &mut next] {
        let settings = state["settings"]
            .as_object_mut()
            .ok_or("Invalid player state")?;
        settings.remove(field);
        if field == MODE {
            settings.remove(MODE_REVISION);
        }
    }
    if old != next {
        return Err("A protected change cannot use player preferences");
    }
    Ok(())
}

pub(super) fn write_refusal(runtime: &Runtime) -> Option<&'static str> {
    let state = &runtime.public_preferences;
    if !runtime.write_permit.is_valid() {
        return Some("Player preferences are no longer writable");
    }
    if state.uncertain {
        return Some("Preference write needs reconciliation; restart the player");
    }
    if let Some(error) = state.failure {
        return Some(error);
    }
    if !state.loaded {
        return Some("Player preferences are still loading");
    }
    runtime
        .saving
        .then_some("Another settings operation is in progress")
}

/// Missing/unreadable storage is distinct from the default enabled policy.
pub(super) fn notifications_enabled(state: &State) -> Option<bool> {
    (state.loaded && state.failure.is_none() && !state.uncertain).then(||
        state.snapshot.as_ref().and_then(|s| s.value.notifications_enabled).unwrap_or(true))
}

/// None means not loaded/readable yet; Some(None) means no bookmark for this source.
pub(super) fn classic_bookmark(state: &State, source: &str)
    -> Option<Option<plx_platform::storage::wire::ott::ClassicBookmark>>
{
    if !state.loaded || state.failure.is_some() || state.uncertain { return None; }
    Some(state.snapshot.as_ref().and_then(|snapshot| snapshot.value.classic_bookmarks.iter()
        .find(|bookmark| bookmark.source == source).cloned()))
}

/// Post-loop collection uses the same uncertainty policy as the frame callback.
pub(super) fn accept_write(runtime: &mut Runtime, result: Result<Snapshot, WriteFailure>) {
    match result {
        Ok(snapshot) => runtime.public_preferences.accept(Some(snapshot)),
        Err(WriteFailure::Uncertain) => runtime.public_preferences.uncertain = true,
        Err(_) => (),
    }
}

pub(super) fn overlay(state: &State, saved: &mut Saved) {
    let Some(snapshot) = &state.snapshot else {
        return;
    };
    if let Some(address) = &snapshot.value.remote_address {
        saved.settings.insert(ADDRESS.into(), json!(address));
    }
    if let Some(mode) = snapshot.value.interface_mode {
        saved.settings.insert(MODE.into(), json!(mode));
        // The durable value, not a stale protected snapshot, owns this revision.
        // A new runtime already has a distinct settings context/nonce.
        saved.settings.insert(
            MODE_REVISION.into(),
            json!(format!(
                "public-mode:{}:{}",
                snapshot.epoch, snapshot.revision
            )),
        );
    }
}

pub(super) fn loaded(
    runtime: &mut Runtime,
    app: &mut App,
    result: Result<Option<Snapshot>, &'static str>,
) {
    if !runtime.write_permit.is_valid() {
        return;
    }
    match result {
        Ok(snapshot) => runtime.public_preferences.accept(snapshot),
        Err(error) if !runtime.public_preferences.loaded => {
            runtime.public_preferences.failure = Some(error)
        }
        Err(_) => return,
    }
    overlay(&runtime.public_preferences, &mut runtime.saved);
    app.bridge.apply_ui_mode(runtime.interface_mode());
    runtime.changed();
}

pub(super) fn saved(
    runtime: &mut Runtime,
    app: &mut App,
    result: Result<Snapshot, WriteFailure>,
    invocation: Option<Invocation>,
    mut response: Value,
) {
    runtime.saving = false;
    runtime.save_navigation = SaveNavigation::Preserve;
    runtime.save_ui_owner = None;
    if !runtime.write_permit.is_valid() {
        return;
    }
    let bookmark_write = classic_bookmark::take_write(runtime);
    let notification_write = invocation.as_ref().is_some_and(|inv| inv.action == "notifications");
    let success = result.is_ok();
    let mut error = match result {
        Ok(snapshot) => {
            runtime.public_preferences.accept(Some(snapshot));
            overlay(&runtime.public_preferences, &mut runtime.saved);
            if bookmark_write.is_none() && !notification_write {
                app.bridge.apply_ui_mode(runtime.interface_mode());
                runtime.status.clear();
            }
            None
        }
        Err(WriteFailure::Rejected(error)) => Some(save_error::preference_message(error)),
        Err(WriteFailure::Expired) => Some("Settings request expired".into()),
        Err(WriteFailure::Uncertain) => {
            runtime.public_preferences.uncertain = true;
            Some("Preference write needs reconciliation; restart the player".into())
        }
    };
    if let Some(message) = error.as_ref().filter(|_| !notification_write) {
        runtime.status = message.clone();
    }
    if let Some(sent) = bookmark_write {
        let applied = if sent.category_choice {
            let allowed = classic_bookmark::current(runtime, app)
                && invocation.as_ref().is_none_or(|inv| commands::category_select_completion_allowed(runtime, inv));
            runtime.finish_category_choice(app, success && allowed)
        } else { false };
        classic_bookmark::acknowledged(runtime, &sent.value, success);
        if sent.category_choice && applied { runtime.status.clear(); }
        if let Some(invocation) = invocation {
            let response = if success { response } else { rejected(error.as_deref().unwrap_or("Could not save category")) };
            let response = commands::category_select_settled(runtime, &invocation, success, applied, response);
            runtime.remote.complete(invocation, response);
        }
        runtime.changed();
        return;
    }
    if let Some(session) = runtime.pending_editor_save.take() {
        error = remote_settings::after_saved(runtime, app, &session, error);
        runtime.editor_result(app, &session, error.clone());
    }
    if let Some(invocation) = invocation {
        if error.is_none() {
            commands::settings_public_saved(runtime, &invocation, &mut response);
        }
        let response = if let Some(error) = error {
            if invocation.action == "notifications" { notification_settings::failure(&invocation.params, "storage_unavailable") }
            else { commands::settings_failure(&invocation, "storage_failed")
                .unwrap_or_else(|| rejected(&error)) }
        } else {
            response
        };
        runtime.remote.complete(invocation, response);
    }
    runtime.changed();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_default_on_only_after_known_storage_and_never_enter_provider_state() {
        let mut state = State::default();
        assert_eq!(notifications_enabled(&state), None);
        state.accept(None);
        assert_eq!(notifications_enabled(&state), Some(true));
        let before = Saved::default();
        assert!(validate_candidate(&before, &before, &Patch::NotificationsEnabled(false)).is_ok());
        let mut changed = before.clone();
        changed.settings.insert("sNotificationsEnabled".into(), json!(false));
        assert!(validate_candidate(&before, &changed, &Patch::NotificationsEnabled(false)).is_err());
        let value = store::Preferences::default().apply(&Patch::NotificationsEnabled(false)).unwrap();
        state.accept(Some(Snapshot { value, revision: 1, epoch: 1 }));
        assert_eq!(notifications_enabled(&state), Some(false));
        let mut saved = before.clone(); overlay(&state, &mut saved);
        assert_eq!(serde_json::to_value(saved).unwrap(),serde_json::to_value(before).unwrap());
        state.uncertain = true;
        assert_eq!(notifications_enabled(&state), None);
    }

    #[test]
    fn public_door_rejects_simultaneous_secret_or_provider_edits() {
        let before = Saved::default();
        let mut candidate = before.clone();
        candidate.settings.insert(MODE.into(), json!(0));
        assert!(validate_candidate(&before, &candidate, &Patch::InterfaceMode(0)).is_ok());
        candidate
            .settings
            .insert("m3uMediaUrl".into(), json!("https://media.invalid/secret"));
        assert!(validate_candidate(&before, &candidate, &Patch::InterfaceMode(0)).is_err());
    }

    #[test]
    fn public_address_rejects_credentials_queries_and_fragments() {
        let before = Saved::default();
        for address in [
            "https://user:secret@control.invalid",
            "https://control.invalid/?token=secret",
            "https://control.invalid/#secret",
            "https://control.invalid/%73ecret",
        ] {
            let mut candidate = before.clone();
            candidate.settings.insert(ADDRESS.into(), json!(address));
            assert!(
                validate_candidate(&before, &candidate, &Patch::RemoteAddress(address.into()))
                    .is_err()
            );
        }
    }
}
