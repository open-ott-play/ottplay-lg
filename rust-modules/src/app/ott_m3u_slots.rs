//! Classic's fifteen numbered M3U slots project the existing wire ownership.
//! Empty rows are private editor intents, never placeholder Saved profiles.
use super::*;
use super::ui_projection::row;

const CHANGED: &str = "This profile changed; reopen its setup before saving";

pub(super) struct Draft {
    editor: EditorSession,
    profile_id: String,
    number: usize,
    epoch: u64,
    inventory: String,
}

pub(super) enum Target {
    Saved(ProviderProfile),
    Empty(usize),
}

pub(super) fn settings_changed(before: &Saved, after: &Saved) -> bool {
    before.settings.iter().filter(|(key, _)| key.starts_with("slot:"))
        .ne(after.settings.iter().filter(|(key, _)| key.starts_with("slot:")))
}

pub(super) fn target(runtime: &Runtime, app: &App, context: &str, target: &str)
    -> Result<Target, &'static str>
{
    if let Some(number) = target.strip_prefix("m3u-slot/") {
        let number: usize = number.parse().map_err(|_| CHANGED)?;
        runtime.profile_edit_surface(app)?;
        if !runtime.classic_m3u_slots() || !(1..=15).contains(&number)
            || target != format!("m3u-slot/{number}")
            || context != runtime.m3u_slot_context(number) || !runtime.m3u_slot_empty(number)
        { return Err(CHANGED); }
        if let Some(error) = runtime.profile_add_refusal() { return Err(error); }
        Ok(Target::Empty(number))
    } else {
        runtime.profile_edit_target(app, context, target).map(Target::Saved)
    }
}

pub(super) fn open(runtime: &mut Runtime, app: &mut App, target: Target)
    -> Result<(), &'static str>
{
    match target {
        Target::Saved(profile) => runtime.open_profile_edit(app, profile),
        Target::Empty(number) => {
            // The shared allocator reserves only a private draft ID; it does not
            // mutate the numbered inventory before a durable Save succeeds.
            let mut profile = runtime.new_profile_draft()?;
            profile.name = format!("m3u {number}");
            let profile_id = profile.id.clone();
            runtime.open_source_profile_editor(app, profile, None, true)?;
            let editor = runtime.edit_session.clone().ok_or(CHANGED)?;
            runtime.m3u_slot_draft = Some(Draft {
                editor, profile_id, number, epoch: runtime.profile_edit_epoch,
                inventory: runtime.m3u_slot_inventory(),
            });
            runtime.profile_focus = Some(format!("m3u-slot/{number}"));
            Ok(())
        }
    }
}

impl Runtime {
    pub(super) fn classic_m3u_slots(&self) -> bool {
        self.provider == ProviderKind::M3u && self.interface_mode().index() == 0
    }

    fn m3u_slot_inventory(&self) -> String {
        let slots: Vec<_> = self.saved.settings.iter().filter(|(key, _)| key.starts_with("slot:")).collect();
        plx_data::ott::favorites_wire::hash(json!([self.saved.profiles, slots]).to_string().as_bytes())
    }

    fn m3u_slot_context(&self, number: usize) -> String {
        plx_data::ott::favorites_wire::hash(json!([
            self.remote.runtime(), self.profile_edit_epoch, self.navigation_epoch,
            self.page, self.provider, number, self.m3u_slot_inventory(),
        ]).to_string().as_bytes())
    }

    fn m3u_slot_empty(&self, number: usize) -> bool {
        !self.saved.profiles.profiles.iter().filter(|profile| profile.kind == ProviderKind::M3u)
            .enumerate().any(|(ordinal, profile)|
                commands::profile_slot_number(&self.saved, profile, ordinal) == number as u64)
    }

    pub(super) fn m3u_slot_rows(&self) -> Vec<OttRow> {
        let profiles: Vec<_> = self.saved.profiles.profiles.iter()
            .filter(|profile| profile.kind == ProviderKind::M3u).enumerate()
            .map(|(ordinal, profile)| (commands::profile_slot_number(&self.saved, profile, ordinal), profile))
            .collect();
        let mut represented = vec![false; profiles.len()];
        let mut rows = Vec::new();
        // Hash the bounded settings image once, not once per empty row.
        let inventory = self.m3u_slot_inventory();
        for number in 1..=15 {
            let claims: Vec<_> = profiles.iter().enumerate().filter(|(_, (slot, _))| *slot == number as u64).collect();
            if let [(index, (_, profile))] = claims.as_slice() {
                represented[*index] = true;
                let mut items = profile_edit::rows(self, profile);
                items[0].title = format!("{number}: {}", profile.name);
                // Only the companion owns an Edit/Load label. The primary keeps
                // its number and exact saved display name through localization.
                items[0].content_title = None;
                if !profile.enabled || managed::shadows_profile(self, profile) {
                    for item in &mut items { item.enabled = false; }
                }
                rows.extend(items);
            } else {
                let target = format!("m3u-slot/{number}");
                let context = plx_data::ott::favorites_wire::hash(json!([
                    self.remote.runtime(), self.profile_edit_epoch, self.navigation_epoch,
                    self.page, self.provider, number, inventory,
                ]).to_string().as_bytes());
                let detail = claims.iter().map(|(_, (_, profile))| profile.name.as_str()).collect::<Vec<_>>().join(", ");
                let mut item = row(target.clone(), format!("{number}: —"), detail,
                    OttAction::EditProfile { context, target }, false);
                item.enabled = claims.is_empty() && self.profile_edit_epoch != 0
                    && self.settings_write_refusal().is_none() && self.profile_add_refusal().is_none();
                rows.push(item);
            }
        }
        // Duplicated and out-of-range claims keep their exact IDs and actions;
        // the view does not pick a winner or silently normalize Saved metadata.
        for (index, (_, profile)) in profiles.iter().enumerate() {
            if represented[index] { continue; }
            let mut items = profile_edit::rows(self, profile);
            if !profile.enabled || managed::shadows_profile(self, profile) {
                for item in &mut items { item.enabled = false; }
            }
            rows.extend(items);
        }
        rows
    }

    pub(super) fn m3u_slot_candidate(&self, editor: &EditorSession, profile: &ProviderProfile,
        original: Option<&ProviderProfile>) -> Result<Option<usize>, &'static str>
    {
        let Some(draft) = self.m3u_slot_draft.as_ref() else { return Ok(None); };
        if draft.editor != *editor || self.edit_session.as_ref() != Some(editor)
            || self.profile_edit_save_only.as_ref() != Some(editor)
            || original.is_some() || profile.id != draft.profile_id || profile.kind != ProviderKind::M3u
            || draft.epoch != self.profile_edit_epoch || draft.inventory != self.m3u_slot_inventory()
            || !self.m3u_slot_empty(draft.number)
        { return Err(CHANGED); }
        Ok(Some(draft.number))
    }

    pub(super) fn complete_m3u_slot(&mut self, editor: &EditorSession) {
        if self.m3u_slot_draft.as_ref().is_some_and(|draft| draft.editor == *editor) {
            let draft = self.m3u_slot_draft.take().expect("matched slot draft");
            self.profile_focus = Some(draft.profile_id);
        }
    }
}
