//! Addressed edits of saved provider sources share the existing editor protocol.
//! Admission is checked against the published row before this module opens a form.
use super::*;
use super::ui_projection::row;

/// The existing one-writer completion owns this intent after the form closes.
/// It contains no authority to activate the newly saved source.
pub(super) struct SaveOnly {
    editor: EditorSession,
    profiles: plx_data::ott::ProviderProfiles,
    active: Option<ProviderProfile>,
}

pub(super) fn supported(kind: ProviderKind) -> bool {
    matches!(kind, ProviderKind::M3u | ProviderKind::Xtream | ProviderKind::Stalker
        | ProviderKind::VPortal | ProviderKind::Local)
}

pub(super) fn rows(runtime: &Runtime, profile: &ProviderProfile) -> Vec<OttRow> {
    let mut load = row(profile.id.clone(), profile.name.clone(), profile.kind.canonical_id().into(),
        OttAction::SelectProfile(profile.id.clone()), false);
    load.enabled = !profile.source.is_empty();
    let mut edit = supported(profile.kind).then(|| {
        let context = runtime.profile_edit_context(profile);
        let mut item = row(format!("action/profile.edit/{context}"),
            format!("Edit {}", profile.name), String::new(),
            OttAction::EditProfile { context, target: profile.id.clone() }, false);
        item.content_title = Some(profile.name.clone());
        item.enabled = runtime.profile_edit_epoch != 0 && runtime.settings_write_refusal().is_none();
        item
    });
    if runtime.interface_mode().index() == 0
        && matches!(profile.kind, ProviderKind::Stalker | ProviderKind::M3u)
        && runtime.saved.profiles.active_profile.as_ref() == Some(&profile.id)
    {
        if let Some(edit) = &mut edit {
            let load_action = load.action.clone();
            let load_enabled = load.enabled;
            load.action = edit.action.clone();
            load.enabled = edit.enabled;
            if profile.kind == ProviderKind::M3u {
                edit.id = format!("action/profile.load/{}", runtime.profile_edit_context(profile));
                edit.title = "Load playlist".into();
                edit.action = load_action;
                edit.enabled = load_enabled;
            }
        }
    }
    std::iter::once(load).chain(edit).collect()
}

impl Runtime {
    pub(super) fn open_profile_edit(&mut self, app: &mut App, profile: ProviderProfile)
        -> Result<(), &'static str>
    {
        if !supported(profile.kind) { return Err("This provider uses its own connection flow"); }
        let target = profile.id.clone();
        if matches!(profile.kind, ProviderKind::Xtream | ProviderKind::Stalker) {
            self.open_setup(profile, false)?;
        } else {
            let save_only = profile.kind == ProviderKind::M3u;
            self.open_source_profile_editor(app, profile.clone(), Some(profile), save_only)?;
        }
        self.profile_focus = Some(target);
        Ok(())
    }

    pub(super) fn new_profile_draft(&self) -> Result<ProviderProfile, &'static str> {
        let index = (1..=self.saved.profiles.profiles.len() + 1).find(|index| {
            let id = format!("{}-{index}", self.provider.canonical_id());
            !self.saved.profiles.profiles.iter().any(|profile| profile.id == id)
        }).ok_or("Unable to allocate a new profile")?;
        Ok(ProviderProfile { id: format!("{}-{index}", self.provider.canonical_id()),
            name: format!("{} {index}", self.provider.canonical_id()), kind: self.provider,
            source: String::new(), source_alias: String::new(), enabled: true })
    }

    pub(super) fn open_source_profile_editor(&mut self, app: &mut App, profile: ProviderProfile,
        original: Option<ProviderProfile>, save_only: bool) -> Result<(), &'static str>
    {
        self.edit_serial.checked_add(1)
            .ok_or("Editor session limit reached; reopen the player")?;
        let value = profile.source.clone();
        self.cancel_setup();
        self.edit_profile_original = original;
        self.edit_profile = Some(profile);
        self.open_setting_editor(app, OttEditor {
            kind: plx_screens::ott::OttEditorKind::Setting, session: 0,
            title: "Source address or folder".into(), key: "profile.source".into(),
            value, secret: true, limit: 16384, text_policy: OttTextPolicy::Literal,
        })?;
        if save_only { self.profile_edit_save_only = self.edit_session.clone(); }
        Ok(())
    }

    /// The common delivery path for generic settings and addressed source forms.
    pub(super) fn open_setting_editor(&mut self, app: &mut App, mut editor: OttEditor)
        -> Result<(), &'static str>
    {
        if let Some(instance) = self.reply {
            let session = self.edit_serial.checked_add(1)
                .ok_or("Editor session limit reached; reopen the player")?;
            self.edit_serial = session;
            self.edit_session = Some(EditorSession {
                id: session,
                owner: instance,
                key: editor.key.clone(),
                purpose: search::EditorPurpose::Setting,
            });
            self.profile_edit_save_only = None;
            self.m3u_slot_draft = None;
            editor.session = session;
            // Keep semantic labels raw; the shared screen resolves the current locale.
            app.pages.emit(
                MachineId::Nav,
                Fx::Deliver(
                    MachineId::Instance(instance),
                    Delivery::Screen(plx_ui::screen::ScreenEvent::App(AppMsg::OttEditor(
                        Arc::new(editor),
                    ))),
                ),
            );
        }
        Ok(())
    }

    /// Only this exact addressed M3U editor has field-save semantics. Existing
    /// generic forms and the Local binding path retain their Save and load flow.
    pub(super) fn profile_editor_candidate(&mut self, editor: &EditorSession,
        profile: ProviderProfile, original: Option<&ProviderProfile>)
        -> Result<(Saved, bool), &'static str>
    {
        let save_only = profile.kind == ProviderKind::M3u
            && self.edit_session.as_ref() == Some(editor)
            && self.profile_edit_save_only.as_ref() == Some(editor);
        let slot = self.m3u_slot_candidate(editor, &profile, original)?;
        let id = profile.id.clone();
        let mut candidate = self.profile_candidate(profile, original)?;
        if let Some(slot) = slot { candidate.settings.insert(format!("slot:{id}"), json!(slot)); }
        if save_only {
            candidate.profiles.active_profile = self.saved.profiles.active_profile.clone();
        }
        // The writer captures this exact intent separately from the live form.
        Ok((candidate, !save_only))
    }

    pub(super) fn save_profile_editor(&mut self, editor: &EditorSession,
        profile: ProviderProfile, original: Option<&ProviderProfile>) -> Result<(), &'static str>
    {
        let (candidate, reload) = self.profile_editor_candidate(editor, profile, original)?;
        let receipt = (!reload).then(|| SaveOnly {
            editor: editor.clone(), profiles: candidate.profiles.clone(),
            active: self.active().filter(|profile| profile.kind == ProviderKind::M3u).cloned(),
        });
        self.save(candidate, None, reload)?;
        self.profile_edit_pending = receipt;
        Ok(())
    }

    fn ordinary_profile_for_edit(&self) -> Option<&ProviderProfile> {
        if self.transient.accepted.is_some() || managed::profile(self).is_some()
            || classic_source::profile(self).is_some() { return None; }
        self.active().filter(|profile| profile.kind == ProviderKind::M3u)
    }

    /// A successful field save never becomes Load, including after navigation
    /// or replacement of the browsed source. Only the still-owned ordinary
    /// catalogue is invalidated; already bound playback stays independent.
    pub(super) fn finish_profile_edit(&mut self, saved: Option<&Saved>,
        invocation: Option<&Invocation>, reload: bool) -> bool
    {
        let Some(receipt) = self.profile_edit_pending.take() else { return false; };
        let Some(saved) = saved else { return false; };
        if invocation.is_some() || reload || saved.profiles != receipt.profiles
            || self.pending_editor_save.as_ref() != Some(&receipt.editor) { return false; }
        let next = saved.profiles.profiles.iter().find(|profile|
            Some(&profile.id) == saved.profiles.active_profile.as_ref());
        if let Some(old) = receipt.active.as_ref().filter(|old|
            Some(*old) != next && self.ordinary_profile_for_edit() == Some(*old))
        {
            // A pending source has already cancelled the ordinary producer and
            // owns its new generation. Keep that admission while removing only
            // the old projection. Its rollback also checks the full profile.
            let interrupted = match self.transient.candidate.as_ref() {
                Some(transient::Candidate::Local(candidate)) => Some(&candidate.interrupted),
                Some(transient::Candidate::Plex(candidate)) => candidate.interrupted.as_ref(),
                None => None,
            };
            let pending_source = interrupted.is_some_and(|scan|
                scan.owner.is_none() && scan.profile.as_ref() == Some(old));
            if !pending_source { self.cancel_catalog(); }
            self.catalog_return = None;
            self.catalog = Catalog::default();
            self.catalog_revision = self.catalog_revision.wrapping_add(1);
            self.catalog_next = None;
            self.catalog_loading = false;
            self.catalog_complete = true;
            self.catalog_dirty = false;
            self.selected = None;
            self.resume_pending = None;
            self.continuity = continuity::Continuity::new(self.generation, None);
            self.changed();
        }
        true
    }
}
