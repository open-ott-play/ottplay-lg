//! Target-bound private M3U attachment editing through the ordinary editor/Save owner.
use super::*;
use super::ui_projection::row;
use crate::app::ott_m3u_attachment::Registry;

pub(super) const KEY: &str = "profile.media";
const CHANGED: &str = "M3U media settings changed; reopen the editor";
const INVALID: &str = "Invalid attached VPortal link";

pub(super) struct Editor {
    parent: ProviderProfile,
    attachment_revision: u64,
}

impl Runtime {
    pub(super) fn m3u_media_context(&self, parent: &ProviderProfile) -> Option<String> {
        let revision = self.m3u_attachments.context_revision();
        if revision == 0 { return None; }
        let entropy = self.entropy.as_ref()?;
        Some(plx_data::ott::favorites_wire::hash(json!([
            entropy, self.remote.runtime(), self.navigation_epoch, self.page,
            self.provider, revision, parent
        ]).to_string().as_bytes()))
    }

    pub(super) fn m3u_media_ready(&self) -> bool {
        self.provider == ProviderKind::M3u && self.settings_write_refusal().is_none()
            && self.entropy.is_some() && self.m3u_attachments.context_revision() != 0
    }

    pub(super) fn m3u_media_target(&self, app: &App, profile: &str, context: &str)
        -> Result<ProviderProfile, &'static str>
    {
        if !self.m3u_media_ready() || self.page != "m3u-media-links"
            || self.reply.is_none() || self.reply != current_ui_owner(app)
            || app.pages.has_pending_navigation()
        { return Err(CHANGED); }
        let mut matches = self.saved.profiles.profiles.iter().filter(|p| p.id == profile);
        let parent = matches.next().ok_or(CHANGED)?;
        if matches.next().is_some() || parent.kind != ProviderKind::M3u || !parent.enabled
            || self.m3u_media_context(parent).as_deref() != Some(context)
        { return Err(CHANGED); }
        Ok(parent.clone())
    }

    pub(super) fn m3u_media_rows(&self) -> Vec<OttRow> {
        let mut rows: Vec<_> = self.saved.profiles.profiles.iter()
            .filter(|p| p.kind == ProviderKind::M3u && p.enabled)
            .map(|parent| {
                let context = self.m3u_media_context(parent);
                let mut item = row(format!("action/m3u-media/{}", context.clone().unwrap_or_else(|| format!("unavailable:{}", parent.id))), parent.name.clone(), String::new(),
                    OttAction::EditM3uMediaLink { profile: parent.id.clone(),
                        context: context.clone().unwrap_or_default() }, false);
                item.enabled = self.m3u_media_ready() && context.is_some();
                item
            }).collect();
        rows.push(row("m3u-media.back".into(), "Back".into(), String::new(),
            OttAction::ShowProfiles, false));
        rows
    }

    pub(super) fn begin_m3u_media(&mut self, app: &mut App, parent: ProviderProfile)
        -> Result<(), &'static str>
    {
        let registry = Registry::read(&self.saved.settings, &self.saved.profiles)
            .map_err(|error| error.message())?;
        let value = registry.for_parent(&parent.id).map(|attachment|
            attachment.project(&parent).map(|profile| profile.source))
            .transpose().map_err(|error| error.message())?.unwrap_or_default();
        let owner = self.reply.ok_or(CHANGED)?;
        let id = self.edit_serial.checked_add(1)
            .ok_or("Editor session limit reached; reopen the player")?;
        let revision = self.m3u_attachments.context_revision();
        if revision == 0 { return Err(CHANGED); }
        self.cancel_setup();
        self.profile_focus = Some(parent.id.clone());
        self.m3u_media_editor = Some(Editor { parent, attachment_revision: revision });
        self.edit_serial = id;
        self.edit_session = Some(EditorSession { id, owner, key: KEY.into(),
            purpose: search::EditorPurpose::Setting });
        let editor = OttEditor { kind: plx_screens::ott::OttEditorKind::Setting,
            session: id, key: KEY.into(), title: "Media library link".into(),
            value, secret: true, limit: 16384, text_policy: OttTextPolicy::Literal };
        app.pages.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(owner),
            Delivery::Screen(plx_ui::screen::ScreenEvent::App(AppMsg::OttEditor(Arc::new(editor))))));
        Ok(())
    }

    pub(super) fn m3u_media_candidate(&mut self, value: &str) -> Result<Saved, &'static str> {
        if value.len() > 16384 || (!value.trim().is_empty()
            && plx_data::ott::parse_vportal_link(value).is_err()) { return Err(INVALID); }
        let editor = self.m3u_media_editor.as_ref().ok_or(CHANGED)?;
        if editor.attachment_revision == 0
            || self.m3u_attachments.context_revision() != editor.attachment_revision
            || self.saved.profiles.profiles.iter().find(|p| p.id == editor.parent.id) != Some(&editor.parent)
        { return Err(CHANGED); }
        let parent = editor.parent.clone();
        let registry = Registry::read(&self.saved.settings, &self.saved.profiles)
            .map_err(|error| error.message())?;
        let new_id = if !value.trim().is_empty() && registry.for_parent(&parent.id).is_none() {
            Some(favorites::nonce(self).map_err(|error| match error {
                favorites::NonceError::NotReady => "Operation identities are not ready",
                favorites::NonceError::Exhausted => "Operation identity limit reached",
            })?)
        } else { None };
        self.attachment_candidate(&parent, value, new_id.as_deref())
    }

    // Called only inside complete_profile_editor's exact-session, persisted receipt gate.
    pub(super) fn complete_m3u_media_editor(&mut self, session: &EditorSession) {
        if session.key != KEY { return; }
        if let Some(editor) = self.m3u_media_editor.as_mut() {
            if let Some(parent) = self.saved.profiles.profiles.iter().find(|p| p.id == editor.parent.id) {
                editor.parent = parent.clone();
                editor.attachment_revision = self.m3u_attachments.context_revision();
            }
        }
    }
}
