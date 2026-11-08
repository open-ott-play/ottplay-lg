//! Private, source-scoped category organization. The raw catalog and portable
//! Favorites stay unchanged. This pure candidate model owns no IO or selection.
use plx_data::ott::{Catalog, MediaLocator, ProviderKind, ProviderProfile, ProviderProfiles};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const KEY: &str = "nativeCatalogOrganization";
pub(super) const ALL_ID: &str = "system:all";
// Format admissions, never truncation of a displayed catalog. The complete
// Saved.settings image retains its independent 1 MiB admission at the writer.
const ENCODED_BYTES: usize = 512 * 1024;
const PROFILE_COUNT: usize = 90;
const LABEL_BYTES: usize = 512;
const CATEGORY_BYTES: usize = 4096;
const ITEM_BYTES: usize = 160;
const PROVIDER_PREFIX: &str = "organization:provider:";
const CUSTOM_PREFIX: &str = "organization:custom:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Source {
    profile_id: String,
    kind: ProviderKind,
    fingerprint: String,
}
impl Source {
    pub fn new(profile: &ProviderProfile) -> Result<Self, Error> {
        profile.validate().map_err(|_| Error::Invalid)?;
        if !profile.enabled { return Err(Error::UnsupportedSource); }
        // A display rename is not another source. Runtime's addressed editor
        // additionally guards the full profile, including its display fields.
        let encoded = serde_json::to_vec(&(profile.kind, &profile.source))
            .map_err(|_| Error::Invalid)?;
        Ok(Self { profile_id: profile.id.clone(), kind: profile.kind,
            fingerprint: plx_data::ott::favorites_wire::hash(&encoded) })
    }
    fn matches(&self, overlay: &Overlay) -> bool {
        self.kind == overlay.kind && self.fingerprint == overlay.fingerprint
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Document {
    version: u8,
    profiles: BTreeMap<String, Overlay>,
}
impl Default for Document {
    fn default() -> Self { Self { version: 1, profiles: BTreeMap::new() } }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Overlay {
    kind: ProviderKind,
    fingerprint: String,
    next_group: u64,
    groups: Vec<Group>,
    order: Vec<String>,
    hidden: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    alphabetical: bool,
}
impl Overlay {
    fn new(source: &Source) -> Self {
        Self { kind: source.kind, fingerprint: source.fingerprint.clone(),
            next_group: 1, groups: Vec::new(), order: Vec::new(), hidden: Vec::new(), alphabetical: false }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Group {
    id: String,
    label: Option<String>,
    provider_label: Option<String>,
    // Structural providers own an opaque category key. Legacy groups omit it
    // and retain their original label-derived identity in version 1 documents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_key: Option<String>,
    // Legacy Local order overlays remain folder-clamped until an explicit
    // logical edit promotes this group; unrelated groups keep their contract.
    #[serde(default, skip_serializing_if = "is_false")]
    local_logical: bool,
    members: Members,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shuffle_before: Option<Vec<String>>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
enum Members {
    Inherited,
    Edited { members: Vec<String>, known: Vec<String> },
    Snapshot { members: Vec<String> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Direction { Up, Down }
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Operation {
    Create { label: String },
    Copy { category: String, label: String },
    Rename { category: String, label: String },
    Delete { category: String },
    MoveCategory { category: String, direction: Direction },
    AddMember { category: String, item: String },
    RemoveMember { category: String, item: String },
    MoveMember { category: String, item: String, direction: Direction },
    ShuffleOrder { category: String, seed: u32 },
    ResetOrder { category: String },
    SetAlphabetical { enabled: bool },
    Reset,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Change {
    pub category: Option<String>,
    pub changed: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Category {
    pub id: String,
    pub label: String,
    pub members: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Projection {
    /// Source IDs for the Add picker, including entries removed from All.
    /// Uses alphabetical view order when enabled, otherwise original raw order.
    pub available: Vec<String>,
    /// The user's projected All membership and order.
    pub all: Vec<String>,
    pub categories: Vec<Category>,
    // Legacy preference matching must use the original provider label, never a
    // renamed label, a row number or the currently selected UI profile.
    raw_labels: BTreeMap<String, String>,
}
impl Projection {
    pub fn category(&self, id: &str) -> Option<&Category> {
        self.categories.iter().find(|category| category.id == id)
    }
    /// Empty means All. Ambiguous legacy raw-label / new-ID collisions are not
    /// guessed; the caller retains its stored preference and shows All.
    pub fn selection_id(&self, value: &str) -> Option<&str> {
        if value.is_empty() || value == ALL_ID { return Some(""); }
        let exact = self.category(value).map(|category| category.id.as_str());
        let legacy = self.raw_labels.get(value).and_then(|id| self.category(id))
            .map(|category| category.id.as_str());
        match (exact, legacy) {
            (Some(a), Some(b)) if a != b => None,
            (Some(id), _) | (_, Some(id)) => Some(id),
            _ => None,
        }
    }
    pub fn ordered_ids(&self, selection: &str) -> &[String] {
        match self.selection_id(selection) {
            Some("") | None => &self.all,
            Some(id) => self.category(id).map_or(&self.all, |category| &category.members),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    Invalid, TooLarge, UnsupportedSource, MissingCategory, MissingItem,
    DuplicateLabel, InvalidLabel, IdentityCollision, Exhausted,
}
impl Error {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Invalid => "Invalid category organization",
            Self::TooLarge => "Category organization exceeds its 512 KiB storage limit",
            Self::UnsupportedSource => "This source does not support category editing",
            Self::MissingCategory => "The category is no longer available",
            Self::MissingItem => "The channel is no longer available",
            Self::DuplicateLabel => "A category with that name already exists",
            Self::InvalidLabel => "Enter a category name of at most 512 bytes without control characters",
            Self::IdentityCollision => "The catalog contains ambiguous identities",
            Self::Exhausted => "Category identifiers are exhausted",
        }
    }
}

struct Raw {
    all: Vec<String>,
    ids: BTreeSet<String>,
    groups: Vec<Category>,
    labels: BTreeMap<String, Option<String>>,
    provider_keys: BTreeMap<String, String>,
}
impl Raw {
    fn new(source: &Source, catalog: &Catalog) -> Result<Self, Error> {
        if catalog.entries.len() > plx_data::ott::MAX_CATALOG_ENTRIES { return Err(Error::Invalid); }
        let mut raw = Self { all: Vec::new(), ids: BTreeSet::new(),
            groups: vec![Category { id: ALL_ID.into(), label: "All".into(), members: Vec::new() }],
            labels: BTreeMap::new(), provider_keys: BTreeMap::new() };
        let mut positions = BTreeMap::new();
        for entry in &catalog.entries {
            if !valid_item(&source.profile_id, source.kind, &entry.id) { return Err(Error::Invalid); }
            if !matches!((source.kind, &entry.media),
                (ProviderKind::M3u, MediaLocator::Http { .. }) |
                (ProviderKind::Xtream, MediaLocator::Xtream { .. }) |
                (ProviderKind::Stalker, MediaLocator::Stalker { .. }) |
                (ProviderKind::Local, MediaLocator::Local { .. }) |
                (ProviderKind::Plex, MediaLocator::Plex { .. }) |
                (ProviderKind::VPortal, MediaLocator::VPortal { .. } | MediaLocator::Http { .. }))
            { return Err(Error::UnsupportedSource); }
            if let MediaLocator::Plex { server_id, rating_key } = &entry.media {
                let encoded = serde_json::to_vec(&(source.kind, server_id)).map_err(|_| Error::Invalid)?;
                if plx_data::ott::favorites_wire::hash(&encoded) != source.fingerprint
                    || entry.id.strip_prefix(source.profile_id.as_str()).and_then(|value| value.strip_prefix(':'))
                        != Some(rating_key.as_str())
                { return Err(Error::Invalid); }
            }
            if let MediaLocator::Local { relative_path } = &entry.media {
                if !plx_data::ott::valid_local_relative_path(relative_path) { return Err(Error::Invalid); }
            }
            if !raw.ids.insert(entry.id.clone()) { return Err(Error::IdentityCollision); }
            raw.all.push(entry.id.clone());
            raw.groups[0].members.push(entry.id.clone());
            if catalog.hierarchy.is_some() { continue; }
            if entry.category.is_empty() { continue; }
            if entry.category.len() > CATEGORY_BYTES { return Err(Error::Invalid); }
            let id = provider_id(&entry.category);
            let position = if let Some(&position) = positions.get(&id) { position } else {
                let position = raw.groups.len();
                positions.insert(id.clone(), position);
                alias(&mut raw.labels, &entry.category, &id);
                raw.groups.push(Category { id, label: entry.category.clone(), members: Vec::new() });
                position
            };
            if raw.groups[position].label != entry.category { return Err(Error::IdentityCollision); }
            raw.groups[position].members.push(entry.id.clone());
        }
        if catalog.hierarchy.is_some() {
            // Keep fresh graph/membership/model-budget admission for public
            // catalog fields, but borrow payloads and reuse the validated paths.
            for category in catalog.checked_category_groups().map_err(|_| Error::Invalid)? {
                let id = provider_key_id(&category.id);
                if raw.provider_keys.insert(id.clone(), category.id).is_some() {
                    return Err(Error::IdentityCollision);
                }
                alias(&mut raw.labels, &category.label, &id);
                raw.groups.push(Category { id, label: category.label,
                    members: category.indices.iter().map(|&index| catalog.entries[index].id.clone()).collect() });
            }
        }
        Ok(raw)
    }
}

impl Document {
    pub fn read(settings: &BTreeMap<String, Value>) -> Result<Self, Error> {
        let Some(value) = settings.get(KEY) else { return Ok(Self::default()); };
        if serde_json::to_vec(value).map_err(|_| Error::Invalid)?.len() > ENCODED_BYTES {
            return Err(Error::TooLarge);
        }
        let document: Self = serde_json::from_value(value.clone()).map_err(|_| Error::Invalid)?;
        document.validate()?;
        Ok(document)
    }
    pub fn write_into(&self, settings: &mut BTreeMap<String, Value>) -> Result<(), Error> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(|_| Error::Invalid)?;
        settings.insert(KEY.into(), value);
        Ok(())
    }
    /// Only a complete, validated profile image can remove dormant owners.
    /// Neither an incomplete catalog nor a filtered provider menu is authority.
    pub fn prune_deleted(&mut self, profiles: &ProviderProfiles) -> Result<usize, Error> {
        profiles.validate().map_err(|_| Error::Invalid)?;
        let before = self.profiles.len();
        self.profiles.retain(|id, _| profiles.profiles.iter().any(|profile| profile.id == *id));
        Ok(before - self.profiles.len())
    }
    fn overlay(&self, source: &Source) -> Option<&Overlay> {
        self.profiles.get(&source.profile_id).filter(|overlay| source.matches(overlay))
    }
    pub fn alphabetical(&self, source: &Source) -> bool {
        self.overlay(source).is_some_and(|overlay| overlay.alphabetical)
    }
    pub fn project(&self, source: &Source, catalog: &Catalog) -> Result<Projection, Error> {
        let raw = Raw::new(source, catalog)?;
        let mut categories = source_merged(source, self.overlay(source), &raw);
        for category in &mut categories {
            category.members.retain(|id| raw.ids.contains(id));
        }
        let mut labels = raw.labels;
        if let Some(overlay) = self.overlay(source) {
            for group in &overlay.groups {
                if source.kind == ProviderKind::Local && !group.local_logical { continue; }
                if let Some(label) = &group.provider_label {
                    alias(&mut labels, label, &group.id);
                }
            }
        }
        let mut raw_labels: BTreeMap<String, String> = labels.into_iter()
            .filter_map(|(label, id)| id.map(|id| (label, id))).collect();
        // Exact source IDs take precedence over legacy display labels, just as
        // Catalog::resolve_category does. No label is parsed into a path.
        for (id, key) in raw.provider_keys { raw_labels.insert(key, id); }
        if let Some(overlay) = self.overlay(source) {
            for group in &overlay.groups {
                if let Some(key) = &group.provider_key { raw_labels.insert(key.clone(), group.id.clone()); }
            }
        }
        let mut available = raw.all;
        if self.alphabetical(source) {
            // Native policy: lexicographic canonical casefold, accent-sensitive,
            // not natural numeric or locale collation. Stable ties keep each
            // vector's existing order; neither memberships nor category order
            // are rewritten. Compute one key per entry, shared by every group.
            let keys: BTreeMap<&str, String> = catalog.entries.iter().map(|entry|
                (entry.id.as_str(), plx_base::unicode::caseless_key(&entry.title))).collect();
            available.sort_by(|a, b| keys[a.as_str()].cmp(&keys[b.as_str()]));
            for category in &mut categories {
                category.members.sort_by(|a, b| keys[a.as_str()].cmp(&keys[b.as_str()]));
            }
        }
        let all = categories.iter().find(|category| category.id == ALL_ID)
            .map(|category| category.members.clone()).unwrap_or_default();
        categories.retain(|category| category.id != ALL_ID);
        Ok(Projection { available, all, categories, raw_labels })
    }
    /// Mutates only a caller-owned candidate and only on complete success.
    /// Runtime is responsible for current profile/catalog/UI/policy admission.
    pub fn apply(&mut self, source: &Source, catalog: &Catalog, operation: Operation)
        -> Result<Change, Error>
    {
        self.validate()?;
        if operation == Operation::Reset {
            if source.kind == ProviderKind::Local && self.overlay(source).is_none() {
                return Ok(Change { category: None, changed: false });
            }
            let changed = self.profiles.remove(&source.profile_id).is_some();
            return Ok(Change { category: None, changed });
        }
        let raw = Raw::new(source, catalog)?;
        let visible = source_merged(source, self.overlay(source), &raw);
        let mut next = self.clone();
        if !next.profiles.get(&source.profile_id).is_some_and(|overlay| source.matches(overlay)) {
            next.profiles.insert(source.profile_id.clone(), Overlay::new(source));
        }
        let overlay = next.profiles.get_mut(&source.profile_id).ok_or(Error::Invalid)?;
        let mut result = None;
        match operation {
            Operation::SetAlphabetical { enabled } => { overlay.alphabetical = enabled; }
            Operation::Create { label } => {
                result = Some(create_group(overlay, &visible, &label, &[])?);
            }
            Operation::Copy { category, label } => {
                let original = require_category(&visible, &category)?;
                result = Some(create_group(overlay, &visible, &label, &original.members)?);
            }
            Operation::Rename { category, label } => {
                let current = require_editable_category(&visible, &category)?;
                let label = checked_label(&visible, &label, Some(&category))?;
                if current.label != label {
                    let local = overlay.kind == ProviderKind::Local;
                    let group = edit_group(overlay, current, &raw)?;
                    if local && !group.local_logical {
                        // A rename makes this a logical group. Start from the
                        // visible folder-clamped membership, never latent IDs
                        // in an old order record. Keep the original reset baseline.
                        group.members = Members::Edited { members: current.members.clone(),
                            known: raw.groups.iter().find(|raw| raw.id == current.id)
                                .map(|raw| raw.members.clone()).unwrap_or_default() };
                        group.local_logical = true;
                    }
                    group.label = Some(label);
                }
                result = Some(category);
            }
            Operation::Delete { category } => {
                require_editable_category(&visible, &category)?;
                overlay.groups.retain(|group| group.id != category);
                overlay.order.retain(|id| id != &category);
                if is_provider(&category) && !overlay.hidden.contains(&category) {
                    overlay.hidden.push(category);
                }
            }
            Operation::MoveCategory { category, direction } => {
                require_editable_category(&visible, &category)?;
                // Missing provider categories retain their remembered position.
                let mut known: BTreeSet<&str> = overlay.order.iter().map(String::as_str).collect();
                known.extend(visible.iter().filter(|group| group.id != ALL_ID).map(|group| group.id.as_str()));
                let lower = known.iter().fold(0usize, |size, id|
                    size.saturating_add(id.len().saturating_add(3)));
                if lower > ENCODED_BYTES { return Err(Error::TooLarge); }
                let mut order = overlay.order.clone();
                let mut added: BTreeSet<String> = order.iter().cloned().collect();
                for group in visible.iter().filter(|group| group.id != ALL_ID) {
                    if added.insert(group.id.clone()) { order.push(group.id.clone()); }
                }
                let present: BTreeSet<&str> = visible.iter().filter(|group| group.id != ALL_ID)
                    .map(|group| group.id.as_str()).collect();
                move_present(&mut order, &category, direction, |id| present.contains(id))?;
                overlay.order = order;
                result = Some(category);
            }
            Operation::AddMember { category, item } => {
                change_member(overlay, &raw, &visible, &category, &item, MemberEdit::Add)?;
                result = Some(category);
            }
            Operation::RemoveMember { category, item } => {
                change_member(overlay, &raw, &visible, &category, &item, MemberEdit::Remove)?;
                result = Some(category);
            }
            Operation::MoveMember { category, item, direction } => {
                change_member(overlay, &raw, &visible, &category, &item, MemberEdit::Move(direction))?;
                result = Some(category);
            }
            Operation::ShuffleOrder { category, seed } => {
                if seed == 0 { return Err(Error::Invalid); }
                let current = require_category(&visible, &category)?;
                if current.members.len() < 2 { return Ok(Change { category: None, changed: false }); }
                let group = edit_group(overlay, current, &raw)?;
                let baseline = group.shuffle_before.get_or_insert_with(|| current.members.clone());
                // Local retains discovery order. Other providers keep only the
                // first baseline; Reset appends later items in their current
                // order, including explicit moves after a repeated shuffle.
                if source.kind == ProviderKind::Local {
                    let seen: BTreeSet<&str> = baseline.iter().map(String::as_str).collect();
                    let added: Vec<String> = current.members.iter().filter(|id| !seen.contains(id.as_str())).cloned().collect();
                    baseline.extend(added);
                }
                let mut members = current.members.clone();
                super::super::ott_local_session::shuffle_order(&mut members, seed);
                set_member_order(group, members);
                overlay.alphabetical = false;
                result = Some(current.id.clone());
            }
            Operation::ResetOrder { category } => {
                let current = require_category(&visible, &category)?;
                let group = edit_group(overlay, current, &raw)?;
                let Some(baseline) = group.shuffle_before.take() else {
                    return Ok(Change { category: None, changed: false });
                };
                let present: BTreeSet<&str> = current.members.iter().map(String::as_str).collect();
                let mut members: Vec<String> = baseline.into_iter().filter(|id| present.contains(id.as_str())).collect();
                let seen: BTreeSet<&str> = members.iter().map(String::as_str).collect();
                let added: Vec<String> = current.members.iter().filter(|id| !seen.contains(id.as_str())).cloned().collect();
                members.extend(added);
                set_member_order(group, members);
                overlay.alphabetical = false;
                result = Some(current.id.clone());
            }
            Operation::Reset => unreachable!("reset handled above"),
        }
        if next.profiles.get(&source.profile_id).is_some_and(|overlay|
            overlay.groups.is_empty() && overlay.order.is_empty() && overlay.hidden.is_empty()
                && overlay.next_group == 1 && !overlay.alphabetical)
        {
            match self.profiles.get(&source.profile_id) {
                // Preserve dormant owners on a no-op against another source.
                // Disabling our own alpha-only overlay must not restore true.
                Some(previous) if !source.matches(previous) || !previous.alphabetical => {
                    next.profiles.insert(source.profile_id.clone(), previous.clone());
                }
                _ => { next.profiles.remove(&source.profile_id); }
            }
        }
        next.validate()?;
        let changed = *self != next;
        *self = next;
        Ok(Change { category: result, changed })
    }
    pub fn can_reset_order(&self, source: &Source, category: &str) -> bool {
        let category = if category.is_empty() { ALL_ID } else { category };
        self.overlay(source).is_some_and(|overlay| overlay.groups.iter().any(|group|
            group.id == category && group.shuffle_before.is_some()))
    }
    fn validate(&self) -> Result<(), Error> {
        if self.version != 1 { return Err(Error::Invalid); }
        if self.profiles.len() > PROFILE_COUNT { return Err(Error::TooLarge); }
        for (profile_id, overlay) in &self.profiles {
            if !valid_profile_id(profile_id) || !digest(&overlay.fingerprint)
                || overlay.next_group == 0
            { return Err(Error::Invalid); }
            let mut seen: BTreeSet<&str> = BTreeSet::new();
            for group in &overlay.groups {
                if !seen.insert(group.id.as_str()) || !valid_category_id(&group.id) { return Err(Error::Invalid); }
                if overlay.kind != ProviderKind::Local && group.local_logical { return Err(Error::Invalid); }
                let legacy_local = overlay.kind == ProviderKind::Local && !group.local_logical;
                if legacy_local && (group.label.is_some()
                    || (group.id != ALL_ID && !is_provider(&group.id))) { return Err(Error::Invalid); }
                if legacy_local && matches!(&group.members,
                    Members::Edited { known, .. } if !known.is_empty()) { return Err(Error::Invalid); }
                if let Some(baseline) = &group.shuffle_before { valid_items(profile_id, overlay.kind, baseline)?; }
                if let Some(label) = &group.label { label_valid(label)?; }
                if let Some(label) = &group.provider_label {
                    if label.is_empty() || label.len() > CATEGORY_BYTES { return Err(Error::Invalid); }
                    let expected = if let Some(key) = &group.provider_key {
                        if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) { return Err(Error::Invalid); }
                        provider_key_id(key)
                    } else { provider_id(label) };
                    if expected != group.id { return Err(Error::Invalid); }
                }
                if group.provider_key.is_some() && group.provider_label.is_none() { return Err(Error::Invalid); }
                match (&group.members, custom_number(&group.id)) {
                    (Members::Inherited, None) if group.id == ALL_ID
                        && group.label.is_none() && group.provider_label.is_none() => {}
                    (Members::Edited { members, known }, None) if group.id == ALL_ID
                        && group.label.is_none() && group.provider_label.is_none() => {
                        valid_items(profile_id, overlay.kind, members)?;
                        valid_items(profile_id, overlay.kind, known)?;
                    }
                    (Members::Inherited, None) if is_provider(&group.id) && group.provider_label.is_some() => {}
                    (Members::Edited { members, known }, None) if is_provider(&group.id) && group.provider_label.is_some() => {
                        valid_items(profile_id, overlay.kind, members)?;
                        valid_items(profile_id, overlay.kind, known)?;
                    }
                    (Members::Snapshot { members }, Some(number)) if number < overlay.next_group
                        && group.label.is_some() && group.provider_label.is_none() => valid_items(profile_id, overlay.kind, members)?,
                    _ => return Err(Error::Invalid),
                }
            }
            unique(&overlay.order)?;
            if overlay.order.iter().any(|id| id == ALL_ID || (!is_provider(id) && !seen.contains(id.as_str()))) { return Err(Error::Invalid); }
            unique(&overlay.hidden)?;
            if overlay.hidden.iter().any(|id| !is_provider(id)) { return Err(Error::Invalid); }
        }
        if serde_json::to_vec(self).map_err(|_| Error::Invalid)?.len() > ENCODED_BYTES {
            return Err(Error::TooLarge);
        }
        Ok(())
    }
}

fn is_false(value: &bool) -> bool { !*value }
fn provider_id(raw: &str) -> String {
    format!("{PROVIDER_PREFIX}{}", plx_data::ott::favorites_wire::hash(raw.as_bytes()))
}
fn provider_key_id(key: &str) -> String {
    // Domain separation from the legacy label-derived IDs, including labels
    // which happen to contain a source category's spelling.
    provider_id(&format!("\0category-key\0{key}"))
}
fn alias(labels: &mut BTreeMap<String, Option<String>>, label: &str, id: &str) {
    match labels.entry(label.into()) {
        std::collections::btree_map::Entry::Vacant(entry) => { entry.insert(Some(id.into())); }
        std::collections::btree_map::Entry::Occupied(mut entry) => {
            if entry.get().as_deref() != Some(id) { entry.insert(None); }
        }
    }
}
fn custom_number(id: &str) -> Option<u64> {
    let suffix = id.strip_prefix(CUSTOM_PREFIX)?;
    let number = suffix.parse::<u64>().ok()?;
    (number > 0 && suffix == number.to_string()).then_some(number)
}
fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn is_provider(id: &str) -> bool { id.strip_prefix(PROVIDER_PREFIX).is_some_and(digest) }
fn valid_category_id(id: &str) -> bool { id == ALL_ID || is_provider(id) || custom_number(id).is_some() }
fn valid_profile_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_:.".contains(&byte))
}
fn valid_item(profile: &str, kind: ProviderKind, id: &str) -> bool {
    let Some(value) = id.strip_prefix(profile).and_then(|value| value.strip_prefix(':')) else { return false; };
    if kind == ProviderKind::Plex {
        // The native Plex adapter deliberately retains profile:ratingKey,
        // including nonnumeric provider keys, rather than hashing a new row ID.
        !value.is_empty() && value.len() <= 256
            && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    } else {
        id.len() <= ITEM_BYTES && value.len() == 16 && value.bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}
fn valid_items(profile: &str, kind: ProviderKind, items: &[String]) -> Result<(), Error> {
    unique(items)?;
    if items.iter().any(|id| !valid_item(profile, kind, id)) { return Err(Error::Invalid); }
    Ok(())
}
fn unique(items: &[String]) -> Result<(), Error> {
    let mut seen = BTreeSet::new();
    if items.iter().any(|item| !seen.insert(item)) { return Err(Error::Invalid); }
    Ok(())
}
fn label_valid(label: &str) -> Result<(), Error> {
    if label.is_empty() || label.trim() != label || label.len() > LABEL_BYTES
        || label.chars().any(char::is_control) { Err(Error::InvalidLabel) } else { Ok(()) }
}
fn checked_label(visible: &[Category], label: &str, except: Option<&str>) -> Result<String, Error> {
    let label = label.trim();
    label_valid(label)?;
    if visible.iter().any(|group| Some(group.id.as_str()) != except && group.label == label) {
        return Err(Error::DuplicateLabel);
    }
    Ok(label.into())
}
fn require_category<'a>(visible: &'a [Category], id: &str) -> Result<&'a Category, Error> {
    let id = if id.is_empty() { ALL_ID } else { id };
    visible.iter().find(|group| group.id == id).ok_or(Error::MissingCategory)
}
fn require_editable_category<'a>(visible: &'a [Category], id: &str) -> Result<&'a Category, Error> {
    if id.is_empty() || id == ALL_ID { return Err(Error::MissingCategory); }
    require_category(visible, id)
}
fn edit_group<'a>(overlay: &'a mut Overlay, current: &Category, raw: &Raw) -> Result<&'a mut Group, Error> {
    let id = current.id.as_str();
    let position = match overlay.groups.iter().position(|group| group.id == id) {
        Some(position) => position,
        None if is_provider(id) || id == ALL_ID => {
            overlay.groups.push(Group { id: id.into(), label: None,
                provider_label: (id != ALL_ID).then(|| current.label.clone()),
                provider_key: raw.provider_keys.get(id).cloned(), local_logical: false,
                members: Members::Inherited, shuffle_before: None });
            overlay.groups.len() - 1
        }
        None => return Err(Error::MissingCategory),
    };
    Ok(&mut overlay.groups[position])
}

fn set_member_order(group: &mut Group, order: Vec<String>) {
    match &mut group.members {
        Members::Inherited => group.members = Members::Edited { members: order, known: Vec::new() },
        Members::Edited { members, .. } | Members::Snapshot { members } => *members = order,
    }
}

/// Legacy Local order records stay clamped to source folders. Explicit logical
/// groups retain generic membership; neither kind moves files on disk.
fn source_merged(source: &Source, overlay: Option<&Overlay>, raw: &Raw) -> Vec<Category> {
    let mut categories = merged(overlay, raw);
    if source.kind == ProviderKind::Local {
        let logical: BTreeSet<&str> = overlay.into_iter().flat_map(|overlay| overlay.groups.iter())
            .filter(|group| group.local_logical).map(|group| group.id.as_str()).collect();
        let membership: BTreeMap<&str, BTreeSet<&str>> = raw.groups.iter()
            .map(|group| (group.id.as_str(), group.members.iter().map(String::as_str).collect()))
            .collect();
        categories.retain_mut(|category| {
            if logical.contains(category.id.as_str()) { return true; }
            let Some(members) = membership.get(category.id.as_str()) else { return false; };
            category.members.retain(|id| members.contains(id.as_str()));
            true
        });
    }
    categories
}

/// Internal membership includes unavailable IDs. They remain dormant, rather
/// than being erased by a temporary catalog omission or subsequent user edit.
fn merged(overlay: Option<&Overlay>, raw: &Raw) -> Vec<Category> {
    let Some(overlay) = overlay else { return raw.groups.clone(); };
    let stored: BTreeMap<&str, &Group> = overlay.groups.iter().map(|group| (group.id.as_str(), group)).collect();
    let hidden: BTreeSet<&str> = overlay.hidden.iter().map(String::as_str).collect();
    let mut groups = Vec::new();
    let mut present = BTreeSet::new();
    for live in &raw.groups {
        if hidden.contains(live.id.as_str()) { continue; }
        present.insert(live.id.clone());
        let Some(group) = stored.get(live.id.as_str()) else { groups.push(live.clone()); continue; };
        let mut merged = Category { id: live.id.clone(),
            label: group.label.clone().unwrap_or_else(|| live.label.clone()), members: live.members.clone() };
        if let Members::Edited { members, known } = &group.members {
            merged.members = members.clone();
            let seen: BTreeSet<&str> = members.iter().chain(known).map(String::as_str).collect();
            merged.members.extend(live.members.iter().filter(|id| !seen.contains(id.as_str())).cloned());
        }
        groups.push(merged);
    }
    for group in &overlay.groups {
        if present.contains(&group.id) || hidden.contains(group.id.as_str()) { continue; }
        match &group.members {
            Members::Snapshot { members } => groups.push(Category { id: group.id.clone(),
                label: group.label.clone().unwrap_or_default(), members: members.clone() }),
            Members::Edited { members, .. } => groups.push(Category { id: group.id.clone(),
                label: group.label.clone().or_else(|| group.provider_label.clone()).unwrap_or_default(),
                members: members.clone() }),
            Members::Inherited => groups.push(Category { id: group.id.clone(),
                label: group.label.clone().or_else(|| group.provider_label.clone()).unwrap_or_default(),
                members: Vec::new() }),
        }
    }
    let mut by_id: BTreeMap<String, Category> = groups.iter().cloned().map(|group| (group.id.clone(), group)).collect();
    let mut ordered = Vec::new();
    for id in &overlay.order { if let Some(group) = by_id.remove(id) { ordered.push(group); } }
    for group in groups { if let Some(group) = by_id.remove(&group.id) { ordered.push(group); } }
    ordered
}

fn create_group(overlay: &mut Overlay, visible: &[Category], label: &str,
    members: &[String]) -> Result<String, Error>
{
    let label = checked_label(visible, label, None)?;
    let lower = members.iter().fold(0usize, |size, id| size.saturating_add(id.len().saturating_add(3)));
    if lower > ENCODED_BYTES { return Err(Error::TooLarge); }
    let next = overlay.next_group.checked_add(1).ok_or(Error::Exhausted)?;
    let id = format!("{CUSTOM_PREFIX}{}", overlay.next_group);
    if overlay.groups.iter().any(|group| group.id == id) { return Err(Error::IdentityCollision); }
    overlay.next_group = next;
    overlay.groups.push(Group { id: id.clone(), label: Some(label), provider_label: None, provider_key: None,
        local_logical: overlay.kind == ProviderKind::Local,
        members: Members::Snapshot { members: members.to_vec() }, shuffle_before: None });
    // An explicit existing order is extended; otherwise raw provider order is
    // followed by custom groups in creation order.
    if !overlay.order.is_empty() { overlay.order.push(id.clone()); }
    Ok(id)
}

#[derive(Clone, Copy)]
enum MemberEdit { Add, Remove, Move(Direction) }
fn change_member(overlay: &mut Overlay, raw: &Raw, visible: &[Category], category: &str,
    item: &str, edit: MemberEdit) -> Result<(), Error>
{
    let current = require_category(visible, category)?;
    let category = current.id.as_str();
    if !raw.ids.contains(item) { return Err(Error::MissingItem); }
    let contains = current.members.iter().any(|id| id == item);
    match edit {
        MemberEdit::Add if contains => return Ok(()),
        MemberEdit::Remove if !contains => return Ok(()),
        MemberEdit::Move(_) if !contains => return Err(Error::MissingItem),
        _ => {}
    }
    let live = raw.groups.iter().find(|group| group.id == category);
    // Cheap lower bound before copying large member/known vectors. Final JSON
    // size validation also counts escaping, metadata and all other profiles.
    let known = overlay.groups.iter().find(|group| group.id == category).and_then(|group|
        if let Members::Edited { known, .. } = &group.members { Some(known.as_slice()) } else { None });
    let known_ids: BTreeSet<&str> = known.unwrap_or_default().iter()
        .chain(live.into_iter().flat_map(|group| group.members.iter()))
        .map(String::as_str).collect();
    let lower = current.members.iter().map(String::as_str).chain(known_ids.iter().copied())
        .fold(0usize, |size, id| size.saturating_add(id.len().saturating_add(3)));
    if lower > ENCODED_BYTES { return Err(Error::TooLarge); }
    let mut members = current.members.clone();
    match edit {
        MemberEdit::Add if !members.iter().any(|id| id == item) => members.push(item.into()),
        MemberEdit::Remove => members.retain(|id| id != item),
        MemberEdit::Move(direction) => {
            move_present(&mut members, item, direction, |id| raw.ids.contains(id))?;
        }
        MemberEdit::Add => {}
    }
    if overlay.kind == ProviderKind::Local && matches!(edit, MemberEdit::Move(_))
        && members == current.members { return Ok(()); }
    let mut known: BTreeSet<String> = known.unwrap_or_default().iter().cloned().collect();
    if let Some(live) = live { known.extend(live.members.iter().cloned()); }
    let local = overlay.kind == ProviderKind::Local;
    let group = edit_group(overlay, current, raw)?;
    group.members = if is_provider(category) || category == ALL_ID {
        Members::Edited { members, known: known.into_iter().collect() }
    } else { Members::Snapshot { members } };
    if local { group.local_logical = true; }
    Ok(())
}
fn move_present(values: &mut [String], selected: &str, direction: Direction,
    present: impl Fn(&str) -> bool) -> Result<(), Error>
{
    let indices: Vec<usize> = values.iter().enumerate().filter_map(|(index, id)| present(id).then_some(index)).collect();
    let position = indices.iter().position(|&index| values[index] == selected).ok_or(Error::MissingItem)?;
    if indices.len() < 2 { return Ok(()); }
    let target = match direction { Direction::Up => (position + indices.len() - 1) % indices.len(),
        Direction::Down => (position + 1) % indices.len() };
    // Moving, rather than swapping endpoints, preserves intermediate order at
    // the wrap boundary; dormant members retain their slots in the full list.
    let item = values[indices[position]].clone();
    if position < target {
        for at in position..target { values[indices[at]] = values[indices[at + 1]].clone(); }
    } else {
        for at in (target + 1..=position).rev() { values[indices[at]] = values[indices[at - 1]].clone(); }
    }
    values[indices[target]] = item;
    Ok(())
}

#[cfg(test)]
#[path = "ott_catalog_organization_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "ott_catalog_order_tests.rs"]
mod order_tests;

#[cfg(test)]
#[path = "ott_catalog_alphabetical_tests.rs"]
mod alphabetical_tests;

#[cfg(test)]
#[path = "ott_catalog_local_logical_tests.rs"]
mod local_logical_tests;
