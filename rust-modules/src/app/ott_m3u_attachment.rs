//! Private lazy media attachments to M3U profiles. This module only transforms
//! captured values; Saved owns persistence and the existing VPortal runner owns IO.
use plx_data::ott::{
    m3u_source_kind, parse_vportal_link, M3uSourceKind, ProviderKind, ProviderProfile, ProviderProfiles,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const SETTINGS_KEY: &str = "nativeM3uAttachments";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    InvalidRegistry,
    InvalidParent,
    StaleParent,
    DuplicateIdentity,
    InvalidLink,
    InvalidLegacySource,
    UnsupportedMedia,
    InvalidPlaylist,
    MissingId,
    StorageLimit,
    RevisionExhausted,
}

impl Error {
    pub(super) const fn message(self) -> &'static str {
        match self {
            Self::InvalidRegistry => "Invalid stored M3U media attachments",
            Self::InvalidParent => "M3U media attachment parent is unavailable",
            Self::StaleParent => "M3U profile changed before the attachment was saved",
            Self::DuplicateIdentity => "M3U media attachment identity is already in use",
            Self::InvalidLink => "Invalid attached VPortal link",
            Self::InvalidLegacySource => "Invalid legacy media source identity",
            Self::UnsupportedMedia => "This legacy media source is not supported",
            Self::InvalidPlaylist => "Invalid legacy M3U playlist",
            Self::MissingId => "A new M3U media attachment needs a local identity",
            Self::StorageLimit => "M3U media attachments exceed the settings limit",
            Self::RevisionExhausted => "M3U media source revision is exhausted",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Vportal,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Attachment {
    id: String,
    parent_profile_id: String,
    kind: Kind,
    link: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    legacy_source_id: Option<String>,
}

impl std::fmt::Debug for Attachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Attachment(<private>)")
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}

/// Durable identity of one attached source, not permission to resolve it.
/// The current Authorities receipt still owns in-process admission and ABA checks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct KioskBinding {
    pub(super) parent_profile_id: String,
    pub(super) attachment_id: String,
    pub(super) source_fingerprint: String,
}

impl KioskBinding {
    fn validate_fields(parent: &str, attachment: &str, fingerprint: &str) -> Result<(), Error> {
        if !valid_id(parent) || !valid_id(attachment)
            || fingerprint.len() != 64
            || !fingerprint.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::InvalidRegistry);
        }
        if parent == attachment { return Err(Error::DuplicateIdentity); }
        Ok(())
    }

    pub(super) fn validate(&self) -> Result<(), Error> {
        Self::validate_fields(&self.parent_profile_id, &self.attachment_id, &self.source_fingerprint)
    }

    /// The existing borrowed kiosk projection must reject malformed metadata
    /// without allocating a second owned binding on its hot read path.
    pub(super) fn validate_json(value: &Value) -> Result<(), Error> {
        let object = value.as_object().filter(|object| object.len() == 3)
            .ok_or(Error::InvalidRegistry)?;
        let text = |key| object.get(key).and_then(Value::as_str).ok_or(Error::InvalidRegistry);
        Self::validate_fields(text("parent_profile_id")?, text("attachment_id")?, text("source_fingerprint")?)
    }
}

fn valid_legacy_source(value: Option<&str>) -> bool {
    value.is_none_or(|value| value.len() <= 4096 && !value.chars().any(char::is_control))
}

impl Attachment {
    pub(super) fn id(&self) -> &str {
        &self.id
    }

    pub(super) fn legacy_source_id(&self) -> Option<&str> {
        self.legacy_source_id.as_deref()
    }

    fn validate(&self) -> Result<(), Error> {
        if !valid_id(&self.id) || !valid_id(&self.parent_profile_id) {
            return Err(Error::InvalidRegistry);
        }
        if self.id == self.parent_profile_id {
            return Err(Error::DuplicateIdentity);
        }
        if self.link.len() > 16384 || self.link.chars().any(char::is_control) {
            return Err(Error::InvalidLink);
        }
        parse_vportal_link(&self.link).map_err(|_| Error::InvalidLink)?;
        if !valid_legacy_source(self.legacy_source_id.as_deref()) {
            return Err(Error::InvalidLegacySource);
        }
        Ok(())
    }

    /// A temporary runner input, never an extra entry in ProviderProfiles.
    pub(super) fn project(&self, parent: &ProviderProfile) -> Result<ProviderProfile, Error> {
        self.validate()?;
        if parent.id != self.parent_profile_id
            || parent.kind != ProviderKind::M3u
            || !parent.enabled
        {
            return Err(Error::InvalidParent);
        }
        parent.validate().map_err(|_| Error::InvalidParent)?;
        if m3u_source_kind(&parent.source) == Ok(M3uSourceKind::PortalOnly)
            && self.link != parent.source.trim()
        {
            return Err(Error::InvalidLink);
        }
        let profile = ProviderProfile {
            id: self.id.clone(),
            name: parent.name.clone(),
            kind: ProviderKind::VPortal,
            source: self.link.clone(),
            source_alias: parent.source_alias.clone(),
            enabled: true,
        };
        profile.validate().map_err(|_| Error::InvalidLink)?;
        Ok(profile)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Registry {
    version: u32,
    entries: Vec<Attachment>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: 1,
            entries: Vec::new(),
        }
    }
}

/// An explicit trusted media-field update to the captured receiving parent.
/// Unrelated settings writes never enter here. This is not a portable value.
pub(super) enum Change<'a> {
    Clear,
    Set {
        link: &'a str,
        legacy_source_id: Option<&'a str>,
    },
}

impl Registry {
    pub(super) fn read(
        settings: &BTreeMap<String, Value>,
        profiles: &ProviderProfiles,
    ) -> Result<Self, Error> {
        let registry = match settings.get(SETTINGS_KEY) {
            Some(value) => {
                serde_json::from_value(value.clone()).map_err(|_| Error::InvalidRegistry)?
            }
            None => Self::default(),
        };
        registry.validate(profiles)?;
        Ok(registry)
    }

    fn validate_structure(&self) -> Result<(), Error> {
        if self.version != 1 || self.entries.len() > 15 {
            return Err(Error::InvalidRegistry);
        }
        let mut ids = BTreeSet::new();
        let mut parents = BTreeSet::new();
        for entry in &self.entries {
            entry.validate()?;
            if !ids.insert(&entry.id) || !parents.insert(&entry.parent_profile_id) {
                return Err(Error::DuplicateIdentity);
            }
        }
        Ok(())
    }

    pub(super) fn validate(&self, profiles: &ProviderProfiles) -> Result<(), Error> {
        self.validate_structure()?;
        profiles.validate().map_err(|_| Error::InvalidParent)?;
        for entry in &self.entries {
            if profiles.profiles.iter().any(|p| p.id == entry.id) {
                return Err(Error::DuplicateIdentity);
            }
            if !profiles
                .profiles
                .iter()
                .any(|p| p.id == entry.parent_profile_id && p.kind == ProviderKind::M3u)
            {
                return Err(Error::InvalidParent);
            }
        }
        for parent in &profiles.profiles {
            if parent.kind == ProviderKind::M3u
                && m3u_source_kind(&parent.source) == Ok(M3uSourceKind::PortalOnly)
                && !self.for_parent(&parent.id)
                    .is_some_and(|entry| entry.link == parent.source.trim())
            {
                return Err(Error::InvalidLink);
            }
        }
        Ok(())
    }

    /// Reconcile an explicitly edited parent set against the last accepted
    /// document. Only source changes may create or replace an implicit portal
    /// descriptor; an unchanged malformed document is never repaired on read.
    pub(super) fn reconcile_sources(
        &self,
        previous: &ProviderProfiles,
        profiles: &ProviderProfiles,
        new_id: &mut dyn FnMut() -> Result<String, Error>,
    ) -> Result<Self, Error> {
        self.validate(previous)?;
        profiles.validate().map_err(|_| Error::InvalidParent)?;
        let mut next = self.clone();
        for parent in &profiles.profiles {
            if parent.kind != ProviderKind::M3u
                || m3u_source_kind(&parent.source) != Ok(M3uSourceKind::PortalOnly)
            {
                continue;
            }
            if next.for_parent(&parent.id)
                .is_some_and(|entry| entry.link == parent.source.trim())
            {
                continue;
            }
            let old = previous.profiles.iter().find(|old| old.id == parent.id);
            if old.is_some_and(|old| old.kind == parent.kind && old.source == parent.source) {
                return Err(Error::InvalidLink);
            }
            if let Some(entry) = next.entries.iter_mut()
                .find(|entry| entry.parent_profile_id == parent.id)
            {
                entry.link = parent.source.trim().into();
            } else {
                next.entries.push(Attachment {
                    id: new_id()?,
                    parent_profile_id: parent.id.clone(),
                    kind: Kind::Vportal,
                    link: parent.source.trim().into(),
                    legacy_source_id: None,
                });
            }
        }
        // This is the owning parent-edit transaction; deletion or a replacement
        // by a different provider kind may remove its old attachment here.
        next.prune(profiles)?;
        Ok(next)
    }

    pub(super) fn for_parent(&self, id: &str) -> Option<&Attachment> {
        self.entries
            .iter()
            .find(|entry| entry.parent_profile_id == id)
    }

    /// Only the parent's explicit delete/replace candidate calls this. Reading
    /// malformed or dangling stored state must never silently prune it.
    pub(super) fn prune(&mut self, profiles: &ProviderProfiles) -> Result<bool, Error> {
        self.validate_structure()?;
        profiles.validate().map_err(|_| Error::InvalidParent)?;
        let mut next = self.clone();
        next.entries.retain(|entry| {
            profiles
                .profiles
                .iter()
                .any(|p| p.id == entry.parent_profile_id && p.kind == ProviderKind::M3u)
        });
        next.validate(profiles)?;
        let changed = *self != next;
        *self = next;
        Ok(changed)
    }

    pub(super) fn apply(
        &self,
        profiles: &ProviderProfiles,
        captured_parent: &ProviderProfile,
        change: Change<'_>,
        new_id: Option<&str>,
    ) -> Result<Self, Error> {
        self.validate(profiles)?;
        let current = profiles
            .profiles
            .iter()
            .find(|p| p.id == captured_parent.id)
            .ok_or(Error::StaleParent)?;
        if current != captured_parent {
            return Err(Error::StaleParent);
        }
        if current.kind != ProviderKind::M3u || !current.enabled {
            return Err(Error::InvalidParent);
        }
        let mut next = self.clone();
        match change {
            Change::Clear => next
                .entries
                .retain(|entry| entry.parent_profile_id != current.id),
            Change::Set {
                link,
                legacy_source_id,
            } => {
                let id = self
                    .for_parent(&current.id)
                    .map(|entry| entry.id.as_str())
                    .or(new_id)
                    .ok_or(Error::MissingId)?;
                let entry = Attachment {
                    id: id.into(),
                    parent_profile_id: current.id.clone(),
                    kind: Kind::Vportal,
                    link: link.trim().into(),
                    legacy_source_id: legacy_source_id.map(str::to_owned),
                };
                entry.validate()?;
                if let Some(old) = next
                    .entries
                    .iter_mut()
                    .find(|entry| entry.parent_profile_id == current.id)
                {
                    *old = entry;
                } else {
                    next.entries.push(entry);
                }
            }
        }
        next.validate(profiles)?;
        Ok(next)
    }

    /// Converts one explicitly mapped, captured Classic slot. Refusal leaves the
    /// borrowed original and this registry untouched; no raw input enters errors.
    pub(super) fn migrate(
        &self,
        profiles: &ProviderProfiles,
        captured_parent: &ProviderProfile,
        slot: LegacySlot<'_>,
        new_id: Option<&str>,
    ) -> Result<Self, Error> {
        if classify_legacy_slot(slot) == LegacyKind::MediaOnly
            && (m3u_source_kind(&captured_parent.source) != Ok(M3uSourceKind::PortalOnly)
                || captured_parent.source.trim() != slot.media_url.unwrap_or("").trim())
        {
            return Err(Error::InvalidPlaylist);
        }
        let change = match classify_legacy_slot(slot) {
            LegacyKind::NoLink => Change::Clear,
            LegacyKind::Portal | LegacyKind::MediaOnly => Change::Set {
                link: slot.media_url.ok_or(Error::InvalidLink)?,
                legacy_source_id: slot.media_source_id,
            },

            LegacyKind::Unsupported => return Err(Error::UnsupportedMedia),
            LegacyKind::InvalidPortal => return Err(Error::InvalidLink),
            LegacyKind::InvalidPlaylist => return Err(Error::InvalidPlaylist),
            LegacyKind::InvalidLegacySource => return Err(Error::InvalidLegacySource),
        };
        self.apply(profiles, captured_parent, change, new_id)
    }

    /// Alters only a candidate map. Saved still owns durable commit/uncertainty.
    pub(super) fn put_into(
        &self,
        settings: &mut BTreeMap<String, Value>,
        profiles: &ProviderProfiles,
    ) -> Result<(), Error> {
        self.validate(profiles)?;
        let mut next = settings.clone();
        next.insert(
            SETTINGS_KEY.into(),
            serde_json::to_value(self).map_err(|_| Error::InvalidRegistry)?,
        );
        if serde_json::to_vec(&next)
            .map_err(|_| Error::InvalidRegistry)?
            .len()
            > 1024 * 1024
        {
            return Err(Error::StorageLimit);
        }
        *settings = next;
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(super) struct LegacySlot<'a> {
    pub(super) playlist: &'a str,
    pub(super) media_url: Option<&'a str>,
    pub(super) media_source_id: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LegacyKind {
    NoLink,
    Portal,
    MediaOnly,
    Unsupported,
    InvalidPortal,
    InvalidPlaylist,
    InvalidLegacySource,
}

pub(super) fn classify_legacy_slot(slot: LegacySlot<'_>) -> LegacyKind {
    let link = slot.media_url.unwrap_or("").trim();
    if link.is_empty() {
        return LegacyKind::NoLink;
    }
    if !link
        .get(..7)
        .is_some_and(|head| head.eq_ignore_ascii_case("portal:"))
    {
        return LegacyKind::Unsupported;
    }
    if parse_vportal_link(link).is_err() {
        return LegacyKind::InvalidPortal;
    }
    if !valid_legacy_source(slot.media_source_id) {
        return LegacyKind::InvalidLegacySource;
    }
    if slot.playlist.trim().is_empty() {
        LegacyKind::MediaOnly
    } else {
        match m3u_source_kind(slot.playlist.trim()) {
            Ok(M3uSourceKind::Playlist) => LegacyKind::Portal,
            Ok(M3uSourceKind::PortalOnly) if slot.playlist.trim() == link => LegacyKind::MediaOnly,
            _ => LegacyKind::InvalidPlaylist,
        }
    }
}

/// Private in-memory proof, never serialized into Saved or portable Favorites.
#[derive(Clone, PartialEq)]
pub(super) struct Receipt {
    revision: u64,
    parent: ProviderProfile,
    attachment: Attachment,
}

impl std::fmt::Debug for Receipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("M3uAttachmentReceipt")
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

impl Receipt {
    pub(super) fn parent(&self) -> &ProviderProfile {
        &self.parent
    }

    pub(super) fn attachment(&self) -> &Attachment {
        &self.attachment
    }

    #[cfg(test)]
    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn effective_profile(&self) -> Result<ProviderProfile, Error> {
        self.attachment.project(&self.parent)
    }

    pub(super) fn kiosk_binding(&self) -> Result<KioskBinding, Error> {
        self.effective_profile()?;
        // Hash the exact private configuration, including source and attachment
        // identity. Only its digest survives; no URL, key or ephemeral revision
        // is copied into the durable pin.
        let bytes = serde_json::to_vec(&(
            "ottplay-native-kiosk-m3u-attachment-v1", &self.parent, &self.attachment,
        )).map_err(|_| Error::InvalidRegistry)?;
        let binding = KioskBinding {
            parent_profile_id: self.parent.id.clone(),
            attachment_id: self.attachment.id.clone(),
            source_fingerprint: plx_data::ott::favorites_wire::hash(&bytes),
        };
        binding.validate()?;
        Ok(binding)
    }
}

/// One long-lived owner per runtime. Observe every committed Saved transition,
/// including intermediate A→B→A values. Navigation owns its own additional epoch.
#[derive(Clone, Default)]
pub(super) struct Authorities {
    next_revision: u64,
    active: BTreeMap<String, Receipt>,
    context_counter: u64,
    context_valid: bool,
    observed_parents: BTreeMap<String, ProviderProfile>,
    observed_registry: Registry,
    observed_active_profile: Option<String>,
}

impl Authorities {
    /// Transactional: errors leave this owner unchanged. The runtime must call
    /// invalidate_all on an unobservable committed snapshot before accepting IO.
    /// The result reports active receipt changes, not unrelated settings-context
    /// changes; those are observed through context_revision.
    pub(super) fn observe(
        &mut self,
        registry: &Registry,
        profiles: &ProviderProfiles,
    ) -> Result<bool, Error> {
        registry.validate(profiles)?;
        let parents: BTreeMap<_, _> = profiles
            .profiles
            .iter()
            .filter(|parent| parent.kind == ProviderKind::M3u)
            .map(|parent| (parent.id.clone(), parent.clone()))
            .collect();
        let context_changed = !self.context_valid
            || parents != self.observed_parents
            || *registry != self.observed_registry
            || profiles.active_profile != self.observed_active_profile;
        let context_counter = if context_changed {
            self.context_counter
                .checked_add(1)
                .ok_or(Error::RevisionExhausted)?
        } else {
            self.context_counter
        };
        let mut next_revision = self.next_revision;
        let mut next = BTreeMap::new();
        for attachment in &registry.entries {
            let parent = profiles
                .profiles
                .iter()
                .find(|p| p.id == attachment.parent_profile_id)
                .ok_or(Error::InvalidParent)?;
            if !parent.enabled {
                continue;
            }
            let receipt = match self.active.get(&parent.id) {
                Some(previous)
                    if previous.parent == *parent && previous.attachment == *attachment =>
                {
                    previous.clone()
                }
                _ => {
                    next_revision = next_revision
                        .checked_add(1)
                        .ok_or(Error::RevisionExhausted)?;
                    Receipt {
                        revision: next_revision,
                        parent: parent.clone(),
                        attachment: attachment.clone(),
                    }
                }
            };
            next.insert(parent.id.clone(), receipt);
        }
        let changed = self.active != next;
        self.next_revision = next_revision;
        self.active = next;
        self.context_counter = context_counter;
        self.context_valid = true;
        self.observed_parents = parents;
        self.observed_registry = registry.clone();
        self.observed_active_profile = profiles.active_profile.clone();
        Ok(changed)
    }

    /// Settings CAS includes parents with no attachment yet. Zero is invalid and
    /// must never be accepted as a captured settings revision.
    pub(super) fn context_revision(&self) -> u64 {
        if self.context_valid {
            self.context_counter
        } else {
            0
        }
    }

    pub(super) fn receipt(&self, parent_id: &str) -> Option<&Receipt> {
        self.active.get(parent_id)
    }

    pub(super) fn capture(&self, parent_id: &str) -> Option<Receipt> {
        self.receipt(parent_id).cloned()
    }

    pub(super) fn is_current(&self, receipt: &Receipt) -> bool {
        self.active.get(&receipt.parent.id) == Some(receipt)
    }

    pub(super) fn invalidate_all(&mut self) {
        self.active.clear();
        self.context_valid = false;
    }
}

#[cfg(test)]
#[path = "ott_m3u_attachment_tests.rs"]
mod tests;
