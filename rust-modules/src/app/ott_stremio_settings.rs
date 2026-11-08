//! Private Stremio configuration and metadata-resource URL planning.
//!
//! This module performs no IO and owns no worker, timer or persistence queue.
//! The existing adapter stores the configuration through its verified writer
//! and retains request generation, configuration and deadlines in its pending
//! operation. URL syntax admission does not replace the provider transport's
//! destination/redirect policy.

use super::{ott_stremio_catalog::Manifest, ott_stremio_streams};
use plx_net::net::origin::Origin;
use plx_plex::plex::urlenc_str;
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    InvalidConfiguration,
    InvalidAddon,
    InvalidLanguage,
    InvalidResource,
    Limit,
}

/// IDs belong to this installation, not to the untrusted manifest's `id`.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Addon {
    pub id: String,
    pub label: String,
    pub manifest_url: String,
    pub enabled: bool,
}

impl std::fmt::Debug for Addon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StremioAddon(<private>)")
    }
}

impl Addon {
    pub fn validate(&self) -> Result<(), Error> {
        if self.id.is_empty()
            || self.id.len() > 64
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            || self.label.trim().is_empty()
            || self.label.len() > 256
            || self.label.chars().any(char::is_control)
            || self.manifest_url.contains('?')
            || !self.manifest_url.ends_with("/manifest.json")
            || !ott_stremio_streams::web_url(&self.manifest_url, 4096)
        {
            return Err(Error::InvalidAddon);
        }
        let origin = Origin::parse(&self.manifest_url).ok_or(Error::InvalidAddon)?;
        if !origin.is_tls()
            && !matches!(
                origin.host().to_ascii_lowercase().as_str(),
                "localhost" | "127.0.0.1" | "::1"
            )
        {
            return Err(Error::InvalidAddon);
        }
        Ok(())
    }

    /// Append only an already-planned relative resource, preserving the exact
    /// configured authority and private path prefix rather than URL-joining.
    pub fn resource_url(&self, relative: &str) -> Result<String, Error> {
        self.validate()?;
        if relative.is_empty()
            || relative.len() > 8192
            || relative.starts_with('/')
            || relative.contains("://")
            || relative.contains(['?', '#'])
            || !safe_segments(relative)
        {
            return Err(Error::InvalidResource);
        }
        let base = self
            .manifest_url
            .strip_suffix("manifest.json")
            .ok_or(Error::InvalidAddon)?;
        let url = format!("{base}{relative}");
        if !ott_stremio_streams::web_url(&url, 8192) {
            return Err(Error::InvalidResource);
        }
        Ok(url)
    }

    /// Only the unconfigured, known TMDB add-on has Classic's language-path
    /// contract. A generic or already configured add-on keeps its own locale.
    pub fn catalog_language_supported(&self, manifest: &Manifest) -> bool {
        self.manifest_url
            .strip_suffix("/manifest.json")
            .is_some_and(|base| base.eq_ignore_ascii_case("https://tmdb.elfhosted.com"))
            && ["tmdb.top", "tmdb.latest", "tmdb.search"]
                .iter()
                .all(|id| manifest.catalogs().iter().any(|catalog| catalog.id == *id))
    }

    /// Use the existing manifest's relative catalog plan. Manifest and stream
    /// URLs always use `resource_url`, without this catalog-only localization.
    pub fn catalog_resource_url(
        &self,
        manifest: &Manifest,
        relative: &str,
        locale: &str,
    ) -> Result<String, Error> {
        if !relative.starts_with("catalog/movie/") {
            return Err(Error::InvalidResource);
        }
        let locale = language(locale)?;
        if self.catalog_language_supported(manifest) {
            let config = urlenc_str(&json!({"language": locale}).to_string());
            self.resource_url(&format!("{config}/{relative}"))
        } else {
            self.resource_url(relative)
        }
    }
}

/// Reject dot-segment escapes, including those behind encoded separators.
/// Encoded slashes in legitimate catalog IDs and search text remain unchanged.
fn safe_segments(path: &str) -> bool {
    path.split('/').all(|segment| {
        let bytes = segment.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' {
                let Some(pair) = bytes.get(i + 1..i + 3) else {
                    return false;
                };
                let Some(a) = (pair[0] as char).to_digit(16) else {
                    return false;
                };
                let Some(b) = (pair[1] as char).to_digit(16) else {
                    return false;
                };
                decoded.push((a * 16 + b) as u8);
                i += 3;
            } else {
                decoded.push(bytes[i]);
                i += 1;
            }
        }
        decoded
            .split(|byte| *byte == b'/')
            .all(|part| part != b"." && part != b"..")
            && !decoded.contains(&b'\\')
    })
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Config {
    pub language: String,
    pub catalog_service_url: Option<String>,
    pub addons: Vec<Addon>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StremioConfig")
            .field("addon_count", &self.addons.len())
            .finish_non_exhaustive()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language: "en-US".into(),
            catalog_service_url: None,
            addons: Vec::new(),
        }
    }
}

impl Config {
    /// Absent settings use the default. Explicit malformed fields are errors;
    /// they never silently replace an existing installation with empty state.
    pub fn parse(value: &Value) -> Result<Self, Error> {
        if value.is_null() {
            return Ok(Self::default());
        }
        let object = value.as_object().ok_or(Error::InvalidConfiguration)?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "language" | "addons" | "catalogServiceUrl"))
        {
            return Err(Error::InvalidConfiguration);
        }
        let language = match object.get("language") {
            None => Self::default().language,
            Some(value) => language(value.as_str().ok_or(Error::InvalidLanguage)?)?,
        };
        let addons = match object.get("addons") {
            None => Vec::new(),
            Some(value) => {
                let values = value.as_array().ok_or(Error::InvalidConfiguration)?;
                if values.len() > 8 {
                    return Err(Error::Limit);
                }
                values
                    .iter()
                    .map(|value| {
                        let object = value.as_object().ok_or(Error::InvalidAddon)?;
                        if object.len() != 4
                            || object.keys().any(|key| {
                                !matches!(key.as_str(), "id" | "label" | "manifestUrl" | "enabled")
                            })
                        {
                            return Err(Error::InvalidAddon);
                        }
                        Ok(Addon {
                            id: value["id"].as_str().ok_or(Error::InvalidAddon)?.into(),
                            label: value["label"].as_str().ok_or(Error::InvalidAddon)?.into(),
                            manifest_url: value["manifestUrl"]
                                .as_str()
                                .ok_or(Error::InvalidAddon)?
                                .into(),
                            enabled: value["enabled"].as_bool().ok_or(Error::InvalidAddon)?,
                        })
                    })
                    .collect::<Result<Vec<_>, Error>>()?
            }
        };
        let catalog_service_url = object.get("catalogServiceUrl").map(|value| {
            let url = value.as_str().ok_or(Error::InvalidConfiguration)?;
            validate_catalog_service(url)?;
            Ok::<String, Error>(url.to_owned())
        }).transpose()?;
        let config = Self { language, addons, catalog_service_url };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), Error> {
        language(&self.language)?;
        if let Some(url) = &self.catalog_service_url { validate_catalog_service(url)?; }
        if self.addons.len() > 8 {
            return Err(Error::Limit);
        }
        let mut ids = BTreeSet::new();
        let mut urls = BTreeSet::new();
        for addon in &self.addons {
            addon.validate()?;
            if !ids.insert(&addon.id) || !urls.insert(&addon.manifest_url) {
                return Err(Error::InvalidConfiguration);
            }
        }
        // The Classic stream-addon list bounds its complete encoded URL list,
        // not each URL independently. Disabled entries still consume storage.
        if serde_json::to_vec(
            &self
                .addons
                .iter()
                .map(|addon| &addon.manifest_url)
                .collect::<Vec<_>>(),
        )
        .map_err(|_| Error::InvalidConfiguration)?
        .len()
            > 4096
        {
            return Err(Error::Limit);
        }
        Ok(())
    }

    pub fn to_value(&self) -> Value {
        let mut value = json!({"language":self.language,"addons":self.addons.iter().map(|addon| {
            json!({"id":addon.id,"label":addon.label,"manifestUrl":addon.manifest_url,"enabled":addon.enabled})
        }).collect::<Vec<_>>()});
        if let Some(url) = &self.catalog_service_url { value["catalogServiceUrl"] = json!(url); }
        value
    }

    /// Only the explicitly configured metadata service may resolve built-in
    /// discovery paths. A paired command server is never a fallback authority.
    pub fn discovery_url(&self, relative: &str) -> Result<String, Error> {
        let base = self.catalog_service_url.as_deref().ok_or(Error::InvalidConfiguration)?;
        validate_catalog_service(base)?;
        let path = relative.split('?').next().ok_or(Error::InvalidResource)?;
        if relative.is_empty() || relative.len() > 4096 || relative.starts_with('/')
            || relative.contains("://") || relative.contains('#') || !safe_segments(path) {
            return Err(Error::InvalidResource);
        }
        let url = format!("{base}{relative}");
        if !ott_stremio_streams::web_url(&url,8192) { return Err(Error::InvalidResource); }
        Ok(url)
    }
}

fn validate_catalog_service(url: &str) -> Result<(), Error> {
    if !url.ends_with("/tmdb/s/") || url.contains(['?', '#'])
        || !ott_stremio_streams::web_url(url,4096) || !safe_segments(url) {
        return Err(Error::InvalidConfiguration);
    }
    Ok(())
}

/// Canonical BCP-47-shaped language setting; this is not injected into arbitrary
/// add-on URLs. Add-ons may encode their own language in their configured path.
pub(super) fn language(value: &str) -> Result<String, Error> {
    if value.len() > 35 {
        return Err(Error::InvalidLanguage);
    }
    let mut parts = value.split('-');
    let first = parts.next().ok_or(Error::InvalidLanguage)?;
    if !(2..=3).contains(&first.len()) || !first.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(Error::InvalidLanguage);
    }
    let mut result = first.to_ascii_lowercase();
    for part in parts {
        if !(2..=8).contains(&part.len()) || !part.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(Error::InvalidLanguage);
        }
        result.push('-');
        if part.len() == 2 && part.bytes().all(|b| b.is_ascii_alphabetic()) {
            result.push_str(&part.to_ascii_uppercase());
        } else if part.len() == 4 && part.bytes().all(|b| b.is_ascii_alphabetic()) {
            result.push_str(&part[..1].to_ascii_uppercase());
            result.push_str(&part[1..].to_ascii_lowercase());
        } else {
            result.push_str(&part.to_ascii_lowercase());
        }
    }
    Ok(result)
}

#[cfg(test)]
#[path = "ott_stremio_settings/tests.rs"]
mod tests;
