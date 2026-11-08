use super::{http_url, parse_vportal_link, CatalogError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    M3u,
    Xtream,
    Stalker,
    #[serde(rename = "vportal")]
    VPortal,
    #[serde(alias = "offline")]
    Local,
    Plex,
}

impl ProviderKind {
    pub const fn canonical_id(self) -> &'static str {
        match self {
            Self::M3u => "m3u",
            Self::Xtream => "xtream",
            Self::Stalker => "stalker",
            Self::VPortal => "vportal",
            Self::Local => "local",
            Self::Plex => "plex",
        }
    }
}

// Existing version-1 profile-format limits, shared by loading and creation.
const MAX_PROFILES_PER_PROVIDER: usize = 15;
const MAX_PROFILES: usize = 90;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileCapacityError {
    Provider,
    Total,
}
impl ProfileCapacityError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Provider => "This provider already has 15 profiles",
            Self::Total => "There are already 90 profiles",
        }
    }
}

/// The actual source of an M3U profile. A portal-only source keeps the M3U
/// portable origin but has no television playlist to fetch. This classification
/// carries no private source value in Debug or a public settings payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum M3uSourceKind {
    Playlist,
    PortalOnly,
}

pub fn m3u_source_kind(source: &str) -> Result<M3uSourceKind, CatalogError> {
    if source.len() > 16384 || source.chars().any(char::is_control) {
        return Err(CatalogError::InvalidProfile);
    }
    if http_url(source).is_some() {
        Ok(M3uSourceKind::Playlist)
    } else if parse_vportal_link(source).is_ok() {
        Ok(M3uSourceKind::PortalOnly)
    } else {
        Err(CatalogError::InvalidProfile)
    }
}

fn enabled() -> bool {
    true
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderProfile {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    /// Private URL, portal link, local root or Plex server ID. Never a portable identity.
    pub source: String,
    #[serde(default)]
    pub source_alias: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

impl std::fmt::Debug for ProviderProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderProfile")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("source", &"<private>")
            .finish_non_exhaustive()
    }
}

impl ProviderProfile {
    pub fn alias(&self) -> &str {
        if self.source_alias.is_empty() {
            &self.name
        } else {
            &self.source_alias
        }
    }
    /// Validate a source that may be fetched or played, rather than an empty slot.
    pub fn validate(&self) -> Result<(), CatalogError> {
        self.validate_stored()?;
        if self.source.is_empty() { return Err(CatalogError::InvalidProfile); }
        Ok(())
    }

    /// URL slots may retain their identity and metadata after an explicit clear.
    /// This does not make the empty slot a usable source; `validate` stays strict.
    pub fn validate_stored(&self) -> Result<(), CatalogError> {
        if self.id.is_empty()
            || self.id.len() > 128
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.'))
            || !label(&self.name)
            || (!self.source_alias.is_empty() && !(if self.kind == ProviderKind::Local {
                super::valid_local_source_label(&self.source_alias)
            } else { label(&self.source_alias) }))
            || self.source.len() > 16384
            || self.source.chars().any(char::is_control)
        {
            return Err(CatalogError::InvalidProfile);
        }
        let valid = match self.kind {
            ProviderKind::M3u => self.source.is_empty() || m3u_source_kind(&self.source).is_ok(),
            ProviderKind::Xtream => super::xtream::Config::parse(&self.source).is_ok(),
            ProviderKind::Stalker => super::stalker::Config::parse(&self.source).is_ok(),
            ProviderKind::VPortal => self.source.is_empty() || parse_vportal_link(&self.source).is_ok(),
            ProviderKind::Local => {
                http_url(&self.source).is_some()
                    || (self.source.starts_with('/')
                        && !self.source.contains('\\')
                        && !self.source.split('/').any(|s| s == ".."))
            }
            ProviderKind::Plex => !self.source.is_empty() && self.source.len() <= 256,
        };
        if valid {
            Ok(())
        } else {
            Err(CatalogError::InvalidProfile)
        }
    }
}
fn label(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderProfiles {
    pub version: u32,
    pub active_profile: Option<String>,
    pub profiles: Vec<ProviderProfile>,
}
impl Default for ProviderProfiles {
    fn default() -> Self {
        Self {
            version: 1,
            active_profile: None,
            profiles: Vec::new(),
        }
    }
}
impl ProviderProfiles {
    /// Preflight one new profile before asking for credentials. Disabled profiles
    /// also occupy their persisted slot; editing an existing profile needs none.
    pub fn check_addition(&self, kind: ProviderKind) -> Result<(), ProfileCapacityError> {
        self.check_capacity(kind, 1)
    }

    fn check_capacity(
        &self,
        kind: ProviderKind,
        additional: usize,
    ) -> Result<(), ProfileCapacityError> {
        if self.profiles.len().saturating_add(additional) > MAX_PROFILES {
            return Err(ProfileCapacityError::Total);
        }
        if self
            .profiles
            .iter()
            .filter(|profile| profile.kind == kind)
            .count()
            .saturating_add(additional)
            > MAX_PROFILES_PER_PROVIDER
        {
            return Err(ProfileCapacityError::Provider);
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), CatalogError> {
        if self.version != 1 {
            return Err(CatalogError::InvalidProfile);
        }
        for kind in [
            ProviderKind::M3u,
            ProviderKind::Xtream,
            ProviderKind::Stalker,
            ProviderKind::VPortal,
            ProviderKind::Local,
            ProviderKind::Plex,
        ] {
            self.check_capacity(kind, 0)
                .map_err(|_| CatalogError::InvalidProfile)?;
        }
        let mut ids = BTreeSet::new();
        for profile in &self.profiles {
            profile.validate_stored()?;
            if !ids.insert(&profile.id) {
                return Err(CatalogError::InvalidProfile);
            }
        }
        if self
            .active_profile
            .as_ref()
            .is_some_and(|id| !self.profiles.iter().any(|p| p.enabled && &p.id == id))
        {
            return Err(CatalogError::InvalidProfile);
        }
        Ok(())
    }
}
pub fn parse_profiles_json(input: &str) -> Result<ProviderProfiles, CatalogError> {
    if input.len() > 1024 * 1024 {
        return Err(CatalogError::LimitExceeded);
    }
    let result: ProviderProfiles =
        serde_json::from_str(input).map_err(|_| CatalogError::InvalidFormat)?;
    result.validate()?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleared_source_slots_roundtrip_but_are_not_playable_profiles() {
        for kind in [ProviderKind::M3u, ProviderKind::VPortal] {
            let profile = ProviderProfile { id: "slot-7".into(), name: "Keep this name".into(),
                kind, source: String::new(), source_alias: "Keep this alias".into(), enabled: true };
            let saved = ProviderProfiles { version: 1, active_profile: Some(profile.id.clone()),
                profiles: vec![profile.clone()] };
            assert_eq!(parse_profiles_json(&serde_json::to_string(&saved).unwrap()).unwrap(), saved);
            assert!(profile.validate().is_err(), "a stored empty slot is not a playable source");
            for source in [" ", "\t", "invalid", "file:///tmp/list.m3u", "https://fixture.invalid/\n"] {
                let mut bad = saved.clone();
                bad.profiles[0].source = source.into();
                assert!(bad.validate().is_err(), "nonempty invalid sources must still fail");
            }
        }
        for kind in [ProviderKind::Xtream, ProviderKind::Stalker, ProviderKind::Local, ProviderKind::Plex] {
            let profile = ProviderProfile { id: "other".into(), name: "Other".into(), kind,
                source: String::new(), source_alias: String::new(), enabled: true };
            let saved = ProviderProfiles { version: 1, active_profile: None, profiles: vec![profile] };
            assert!(saved.validate().is_err(), "empty-source storage is limited to URL slots");
        }
    }

    #[test]
    fn m3u_source_kind_reuses_portal_parser_without_accepting_blank_profiles() {
        let portal = "portal::[key:fixture-secret]https://fixture.invalid/api/";
        assert_eq!(m3u_source_kind(portal), Ok(M3uSourceKind::PortalOnly));
        assert_eq!(m3u_source_kind("https://fixture.invalid/list.m3u"), Ok(M3uSourceKind::Playlist));
        for source in ["", "  ", "portal:", "portal::[key:x]file:///tmp/media", "file:///tmp/list.m3u", "https://fixture.invalid/\n"] {
            assert!(m3u_source_kind(source).is_err());
        }
        assert!(m3u_source_kind(&format!("{portal}{}", "x".repeat(16384))).is_err());
        let profile = ProviderProfile {
            id: "m3u-1".into(), name: "Media".into(), kind: ProviderKind::M3u,
            source: portal.into(), source_alias: "Portable".into(), enabled: true,
        };
        assert!(profile.validate().is_ok());
        let profiles = ProviderProfiles { version: 1, active_profile: Some(profile.id.clone()), profiles: vec![profile] };
        let loaded = parse_profiles_json(&serde_json::to_string(&profiles).unwrap()).unwrap();
        assert_eq!(loaded, profiles);
        assert_eq!(loaded.profiles[0].kind, ProviderKind::M3u);
        assert!(!format!("{:?}", loaded.profiles[0]).contains("fixture-secret"));
    }

    #[test]
    fn independent_provider_slots_allow_ninety_profiles_but_not_sixteen_per_kind() {
        let mut profiles = ProviderProfiles::default();
        for (kind, source) in [
            (ProviderKind::M3u, "https://fixture.invalid/list.m3u"),
            (
                ProviderKind::Xtream,
                r#"{"server":"https://fixture.invalid","username":"fixture","password":"fixture"}"#,
            ),
            (
                ProviderKind::Stalker,
                r#"{"portal":"https://fixture.invalid","mac":"00:1A:79:00:00:01"}"#,
            ),
            (
                ProviderKind::VPortal,
                "portal::[key:fixture]https://fixture.invalid/portal",
            ),
            (ProviderKind::Local, "/media/videos"),
            (ProviderKind::Plex, "fixture-server"),
        ] {
            for slot in 1..=15 {
                profiles.profiles.push(ProviderProfile {
                    id: format!("{}-{slot}", kind.canonical_id()),
                    name: format!("Fixture {slot}"),
                    kind,
                    source: source.into(),
                    source_alias: String::new(),
                    enabled: true,
                });
            }
        }
        assert_eq!(profiles.profiles.len(), 90);
        assert!(profiles.validate().is_ok());
        assert_eq!(
            profiles.check_addition(ProviderKind::Plex),
            Err(ProfileCapacityError::Total)
        );
        profiles.profiles.pop();
        assert!(profiles.check_addition(ProviderKind::Plex).is_ok());
        assert_eq!(
            profiles.check_addition(ProviderKind::M3u),
            Err(ProfileCapacityError::Provider)
        );
        profiles.profiles.push(ProviderProfile {
            id: "m3u-16".into(),
            name: "Fixture 16".into(),
            kind: ProviderKind::M3u,
            source: "https://fixture.invalid/list.m3u".into(),
            source_alias: String::new(),
            enabled: true,
        });
        assert_eq!(profiles.validate(), Err(CatalogError::InvalidProfile));
    }

    #[test]
    fn addition_capacity_counts_disabled_slots_without_blocking_existing_profile_edits() {
        let mut profiles = ProviderProfiles::default();
        for slot in 1..=14 {
            profiles.profiles.push(ProviderProfile {
                id: format!("m3u-{slot}"),
                name: format!("Fixture {slot}"),
                kind: ProviderKind::M3u,
                source: "https://fixture.invalid/list.m3u".into(),
                source_alias: String::new(),
                enabled: false,
            });
        }
        assert!(profiles.check_addition(ProviderKind::M3u).is_ok());
        let mut last = profiles.profiles[0].clone();
        last.id = "m3u-15".into();
        profiles.profiles.push(last);
        assert_eq!(
            profiles.check_addition(ProviderKind::M3u),
            Err(ProfileCapacityError::Provider)
        );
        assert!(profiles.check_addition(ProviderKind::Xtream).is_ok());
        profiles.profiles[0].name = "Edited existing profile".into();
        assert!(profiles.validate().is_ok(), "full does not mean invalid");
        profiles.profiles.remove(0);
        assert!(profiles.check_addition(ProviderKind::M3u).is_ok());
    }
}
