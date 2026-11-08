//! Installation-wide, credential-free OTT Favorites. This is the portable v1
//! document used by the other OTT players, not Plex's favorite-library setting.
//! All operations are pure; the application commits the resulting document.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Wire limits shared with the portable Favorites document.
pub const DOCUMENT_BYTES: usize = 1024 * 1024;
pub const ENTRY_COUNT: usize = 1000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub provider: String,
    pub profile: String,
    #[serde(deserialize_with = "required_option")]
    pub subprofile: Option<String>,
    #[serde(rename = "sourceAlias")]
    pub source_alias: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum Locator {
    #[serde(rename = "plex-media")]
    Plex {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "ratingKey")]
        rating_key: String,
    },
    #[serde(rename = "local-media")]
    Local {
        #[serde(rename = "rootAlias")]
        root_alias: String,
        #[serde(rename = "relativePath")]
        relative_path: String,
    },
    #[serde(rename = "vportal-media")]
    VPortal {
        #[serde(rename = "idField")]
        id_field: String,
        id: String,
    },
    #[serde(rename = "channel")]
    Channel {
        #[serde(rename = "channelId")]
        channel_id: String,
    },
    #[serde(rename = "channel-name")]
    ChannelName { name: String, #[serde(deserialize_with = "required_option")] group: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Favorite {
    pub title: String,
    pub origin: Origin,
    pub locator: Locator,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    #[serde(deserialize_with = "document_version")]
    version: u8,
    entries: Vec<Favorite>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error { InvalidJson, InvalidEntry, UnsupportedVersion, Duplicate, TooLarge }

// JavaScript has one JSON number representation: version 1, 1.0 and 1e0
// denote the same protocol. Keep the native representation and export integral.
fn document_version<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    let number = serde_json::Number::deserialize(deserializer)?;
    let value = number.as_f64().ok_or_else(|| serde::de::Error::custom("Invalid document version"))?;
    if value.fract() != 0.0 || !(0.0..=255.0).contains(&value) {
        return Err(serde::de::Error::custom("Invalid document version"));
    }
    Ok(value as u8)
}

fn required_option<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

fn text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.encode_utf16().count() <= limit
        && !value.contains("://")
        && !value.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
        && !value.starts_with(trim_space) && !value.ends_with(trim_space)
}
fn trim_space(c: char) -> bool {
    matches!(c, '\u{09}'..='\u{0d}' | '\u{20}' | '\u{a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
        | '\u{205f}' | '\u{3000}' | '\u{feff}')
}
fn identifier(value: &str, limit: usize) -> bool {
    value.len() <= limit && value.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}
fn provider(value: &str) -> bool {
    value.len() <= 64 && value.split('/').all(|part| {
        part.bytes().next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
    })
}

impl Favorite {
    pub fn validate(&self) -> Result<(), Error> {
        let o = &self.origin;
        if !text(&self.title, 1024) || !provider(&o.provider) || !text(&o.profile, 256)
            || !text(&o.source_alias, 256) || o.subprofile.as_ref().is_some_and(|s| !text(s, 256)) {
            return Err(Error::InvalidEntry);
        }
        let good = match (&*o.provider, &self.locator) {
            ("plex", Locator::Plex { server_id, rating_key }) => {
                identifier(server_id, 256) && !server_id.contains(':')
                    && rating_key.len() <= 20
                    && rating_key.bytes().next().is_some_and(|b| (b'1'..=b'9').contains(&b))
                    && rating_key.bytes().all(|b| b.is_ascii_digit())
            }
            ("local", Locator::Local { root_alias, relative_path }) => {
                text(root_alias, 256) && text(relative_path, 8192)
                    && !relative_path.starts_with('/') && !relative_path.contains('\\')
                    && !(relative_path.as_bytes().get(1) == Some(&b':')
                        && relative_path.as_bytes()[0].is_ascii_alphabetic())
                    && relative_path.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
            }
            ("vportal" | "m3u", Locator::VPortal { id_field, id }) => {
                ["id", "fid", "stream_id", "itemId", "media_id", "episode_id"].contains(&id_field.as_str())
                    && identifier(id, 256)
            }
            ("plex" | "local" | "vportal", _) => false,
            (_, Locator::Channel { channel_id }) => identifier(channel_id, 256)
                && !channel_id.strip_prefix("channel:").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())),
            (_, Locator::ChannelName { name, group }) => text(name, 1024)
                && group.as_ref().is_none_or(|g| text(g, 256)),
            _ => false,
        };
        if good { Ok(()) } else { Err(Error::InvalidEntry) }
    }

    /// Identical to the web codec's canonical locator+origin ordering. A title
    /// edit therefore cannot duplicate a favorite or change what it points to.
    pub fn identity(&self) -> String {
        serde_json::json!({"locator": self.locator, "origin": self.origin}).to_string()
    }
}

impl Default for Document {
    fn default() -> Self { Self { version: 1, entries: Vec::new() } }
}

impl Document {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > DOCUMENT_BYTES { return Err(Error::TooLarge); }
        let document: Self = serde_json::from_slice(bytes).map_err(|_| Error::InvalidJson)?;
        document.validate()?;
        Ok(document)
    }
    pub fn entries(&self) -> &[Favorite] { &self.entries }
    pub fn validate(&self) -> Result<(), Error> {
        if self.version != 1 { return Err(Error::UnsupportedVersion); }
        if self.entries.len() > ENTRY_COUNT { return Err(Error::TooLarge); }
        let mut seen = HashSet::new();
        for row in &self.entries {
            row.validate()?;
            if !seen.insert(row.identity()) { return Err(Error::Duplicate); }
        }
        if serde_json::to_vec(self).map_err(|_| Error::InvalidJson)?.len() > DOCUMENT_BYTES {
            return Err(Error::TooLarge);
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        // The browser codec emits keys in lexical order at every level. Keep
        // export hashes and portable identities identical on native clients.
        serde_json::to_vec(&serde_json::to_value(self).map_err(|_| Error::InvalidJson)?)
            .map_err(|_| Error::InvalidJson)
    }
    pub fn contains(&self, row: &Favorite) -> bool {
        self.entries.iter().any(|entry| entry.origin == row.origin && entry.locator == row.locator)
    }
    /// Apply atomically: rejected input leaves the existing list untouched.
    /// Return true when added, false when removed.
    pub fn toggle(&mut self, row: Favorite) -> Result<bool, Error> {
        row.validate()?;
        if let Some(index) = self.entries.iter().position(|e| e.origin == row.origin && e.locator == row.locator) {
            self.entries.remove(index);
            return Ok(false);
        }
        let mut next = self.clone();
        next.entries.push(row);
        next.validate()?;
        *self = next;
        Ok(true)
    }
    pub fn remove(&mut self, identity: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|row| row.identity() != identity);
        before != self.entries.len()
    }
    /// Import does not require a provider to be connected. Unresolved entries
    /// remain visible and can resolve after a matching profile becomes available.
    pub fn merge(&mut self, imported: &Document) -> Result<usize, Error> {
        imported.validate()?;
        let mut next = self.clone();
        let mut seen: HashSet<String> = next.entries.iter().map(Favorite::identity).collect();
        let before = next.entries.len();
        for row in &imported.entries {
            if seen.insert(row.identity()) { next.entries.push(row.clone()); }
        }
        next.validate()?;
        let added = next.entries.len() - before;
        *self = next;
        Ok(added)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_browser_codec_conformance_cases() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!("fixtures/favorites-conformance.json")).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let id = case["id"].as_str().unwrap();
            let result = Document::parse(case["json"].as_str().unwrap().as_bytes());
            assert_eq!(result.is_ok(), case["accepted"].as_bool().unwrap(), "{id}: {result:?}");
            if let Ok(document) = result {
                assert_eq!(String::from_utf8(document.encode().unwrap()).unwrap(), case["canonical"].as_str().unwrap(), "{id}");
                let identities: Vec<String> = document.entries().iter().map(Favorite::identity).collect();
                assert_eq!(serde_json::to_value(identities).unwrap(), case["identities"], "{id}");
            }
        }
    }

    #[test]
    fn integral_json_version_spellings_have_the_same_canonical_document() {
        for version in ["1", "1.0", "1e0", "1.00e+0"] {
            let raw = format!(r#"{{"version":{version},"entries":[]}}"#);
            assert_eq!(Document::parse(raw.as_bytes()).unwrap().encode().unwrap(), br#"{"entries":[],"version":1}"#);
        }
        for version in ["1.1", "true", "null", "\"1\"", "-1", "256"] {
            let raw = format!(r#"{{"version":{version},"entries":[]}}"#);
            assert!(Document::parse(raw.as_bytes()).is_err(), "{raw}");
        }
    }

    #[test]
    fn shared_browser_fixture_has_identical_canonical_bytes_and_identities() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!("fixtures/favorites-portable.json")).unwrap();
        let document = Document::parse(fixture["document"].to_string().as_bytes()).unwrap();
        assert_eq!(String::from_utf8(document.encode().unwrap()).unwrap(), fixture["canonical"].as_str().unwrap());
        for (i, row) in document.entries().iter().enumerate() {
            assert_eq!(row.identity(), fixture["identities"][i].as_str().unwrap());
        }
    }
    fn row() -> Favorite {
        Favorite { title: "Film".into(), origin: Origin { provider: "plex".into(), profile: "Home".into(), subprofile: None, source_alias: "Media".into() },
            locator: Locator::Plex { server_id: "server-1".into(), rating_key: "42".into() } }
    }
    #[test]
    fn portable_json_roundtrips_and_identity_matches_web_order() {
        let item = row();
        assert_eq!(item.identity(), r#"{"locator":{"kind":"plex-media","ratingKey":"42","serverId":"server-1"},"origin":{"profile":"Home","provider":"plex","sourceAlias":"Media","subprofile":null}}"#);
        let mut doc = Document::default();
        assert_eq!(doc.toggle(item), Ok(true));
        assert_eq!(Document::parse(&doc.encode().unwrap()).unwrap(), doc);
    }
    #[test]
    fn rename_does_not_duplicate_and_profiles_are_independent() {
        let mut doc = Document::default();
        doc.toggle(row()).unwrap();
        let mut renamed = row(); renamed.title = "New title".into();
        assert!(doc.contains(&renamed));
        assert_eq!(doc.toggle(renamed), Ok(false));
        doc.toggle(row()).unwrap();
        let mut another = row(); another.origin.profile = "Other".into();
        doc.toggle(another).unwrap();
        assert_eq!(doc.entries().len(), 2);
    }
    #[test]
    fn merge_retains_unavailable_providers_and_is_idempotent() {
        let mut first = Document::default(); first.toggle(row()).unwrap();
        let mut second = Document::default();
        let mut local = row(); local.origin.provider = "local".into();
        local.locator = Locator::Local { root_alias: "Movies".into(), relative_path: "Family/Фильм.mp4".into() };
        second.toggle(local).unwrap();
        assert_eq!(first.merge(&second), Ok(1));
        assert_eq!(first.merge(&second), Ok(0));
        assert_eq!(first.entries().len(), 2);
    }
    #[test]
    fn unknown_fields_duplicates_credentials_and_unsafe_paths_fail() {
        let doc = r#"{"version":1,"version":1,"entries":[]}"#;
        assert_eq!(Document::parse(doc.as_bytes()), Err(Error::InvalidJson));
        let mut item = serde_json::to_value(row()).unwrap(); item["token"] = "secret".into();
        assert!(Document::parse(serde_json::json!({"version":1,"entries":[item]}).to_string().as_bytes()).is_err());
        let item = row();
        assert_eq!(Document::parse(serde_json::json!({"version":1,"entries":[item,item]}).to_string().as_bytes()), Err(Error::Duplicate));
        for bad in ["../film.mp4", "/film.mp4", "C:/film.mp4", "a//film.mp4", "https://secret/film.mp4"] {
            let mut item = row(); item.origin.provider = "local".into();
            item.locator = Locator::Local { root_alias: "Movies".into(), relative_path: bad.into() };
            assert_eq!(item.validate(), Err(Error::InvalidEntry));
        }
    }
    #[test]
    fn a_rejected_edit_never_changes_the_list() {
        let mut doc = Document::default(); doc.toggle(row()).unwrap(); let before = doc.clone();
        let mut invalid = row(); invalid.origin.source_alias = "https://secret.invalid".into();
        assert_eq!(doc.toggle(invalid), Err(Error::InvalidEntry));
        assert_eq!(doc, before);
    }
    #[test]
    fn document_limits_and_invalid_utf8_are_rejected_without_mutation() {
        assert_eq!(Document::parse(&vec![b' '; DOCUMENT_BYTES + 1]), Err(Error::TooLarge));
        assert_eq!(Document::parse(b"{\"version\":1,\"entries\":[],\"\xff\":0}"), Err(Error::InvalidJson));
        let mut entries = Vec::new();
        for index in 1..=ENTRY_COUNT {
            let mut item = row();
            item.locator = Locator::Plex { server_id: "server-1".into(), rating_key: index.to_string() };
            entries.push(item);
        }
        let mut document = Document { version: 1, entries };
        assert!(document.validate().is_ok());
        let before = document.clone();
        let mut item = row();
        item.locator = Locator::Plex { server_id: "server-1".into(), rating_key: (ENTRY_COUNT + 1).to_string() };
        assert_eq!(document.toggle(item), Err(Error::TooLarge));
        assert_eq!(document, before);
    }
}
