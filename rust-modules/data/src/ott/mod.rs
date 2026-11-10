//! Provider-neutral input to the native catalogue. Parsers do not fetch, persist or start playback.
//! The app runner owns IO and supplies complete, bounded responses or directory batches.

pub mod favorites;
pub mod favorites_wire;
pub mod local_metadata;
pub mod xtream;
pub mod edem;
pub mod operators;
mod operator_ids;
pub use operator_ids::OperatorId;
pub mod stalker;
pub mod epg;
pub mod guide;
pub mod hosted_epg;
pub mod reminders;
pub mod archive;
pub mod records;
pub mod stalker_api;
mod local;
mod m3u;
mod profiles;
mod urls;
mod vportal;
mod vportal_text;
mod categories;

pub use local::{catalog_from_local_files, LocalFile, is_local_video_path, valid_local_relative_path, valid_local_source_label};
pub use m3u::{parse_m3u, M3uParser, epg_urls};
pub use profiles::{m3u_source_kind, M3uSourceKind, parse_profiles_json, ProfileCapacityError, ProviderKind, ProviderProfile, ProviderProfiles};
pub use urls::{http_url, resolve_http_url};
pub use categories::{CategoryChoice, CategoryGroup, CategoryHierarchy, CategoryMembership, CategoryNode, CategorySourceIdentity, PlexCategoryKind};
pub use vportal::{parse_vportal_link, parse_vportal_response, PortalLink, PortalPage, PortalRequest};

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Admissions, not promises about a server's total catalogue size.
pub const MAX_CATALOG_ENTRIES: usize = 50_000;
pub const MAX_CATALOG_MODEL_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_CATALOG_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_CATALOG_LINE_BYTES: usize = 64 * 1024;
pub const MAX_METADATA_BYTES: usize = 128 * 1024;
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogError { InvalidProfile, InvalidFormat, InvalidUrl, InvalidPath, LimitExceeded, Truncated }

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidProfile => "Invalid provider profile",
            Self::InvalidFormat => "Invalid catalogue response",
            Self::InvalidUrl => "Invalid HTTP media address",
            Self::InvalidPath => "Invalid relative media path",
            Self::LimitExceeded => "Catalogue response exceeds its limit",
            Self::Truncated => "Catalogue response is incomplete",
        })
    }
}
impl std::error::Error for CatalogError {}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum MediaLocator {
    Http { url: String },
    Local { relative_path: String },
    VPortal { request: serde_json::Value },
    Plex { server_id: String, rating_key: String },
    Xtream { stream_id: String, stream_kind: String, extension: String, #[serde(default)] series_id: Option<String>, #[serde(default)] has_direct_source: bool },
    Stalker { cmd: String, stream_kind: String, #[serde(default)] series: String },
    StalkerApi { channel_id: String },
    ServerLibrary {source_id:String,item_id:String,#[serde(default)] relative_path:Option<String>,#[serde(default)] root_alias:Option<String>},
}

// HTTP URLs and portal requests may contain credentials. Ordinary debug output must not.
impl std::fmt::Debug for MediaLocator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self { Self::Http { .. } => "Http(<private>)", Self::Local { .. } => "Local(<private>)", Self::VPortal { .. } => "VPortal(<private>)", Self::Plex { .. } => "Plex(<private>)", Self::Xtream { .. } => "Xtream(<private>)", Self::Stalker { .. } => "Stalker(<private>)",Self::StalkerApi {..}=>"StalkerApi(<private>)",Self::ServerLibrary {..}=>"ServerLibrary(<private>)" })
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogEntry {
    pub id: String,
    pub title: String,
    pub category: String,
    pub description: String,
    /// Source-owned synopsis boundary; the full description retains generated facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synopsis: Option<SynopsisSpan>,
    pub poster: String,
    pub media: MediaLocator,
    pub duration_ms: Option<u64>,
    /// A provider-attested ID (e.g. tvg-id), separate from the catalogue's collision-safe key.
    pub provider_id: Option<String>,
    #[serde(default)] pub provider_id_field: Option<String>,
    #[serde(default)] pub live: Option<guide::ChannelMetadata>,
}

/// A byte boundary attested by the parser, never inferred from title-like prose.
/// The fingerprint detects metadata replacement; it is not authentication.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SynopsisSpan {
    byte_start: usize,
    text_bytes: usize,
    text_fingerprint: u64,
    /// Validation belongs to one immutable UI snapshot, never persisted input.
    #[serde(skip)]
    validated: bool,
}

impl SynopsisSpan {
    pub fn new(description: &str, byte_start: usize) -> Option<Self> {
        description.get(byte_start..)?;
        (description.len() <= MAX_METADATA_BYTES).then(|| Self {
            byte_start,
            text_bytes: description.len(),
            text_fingerprint: synopsis_fingerprint(description),
            validated: false,
        })
    }

    /// Validate once at projection, after the final description has been chosen.
    pub fn validated_for(&self, description: &str) -> Option<Self> {
        (description.len() == self.text_bytes
            && description.len() <= MAX_METADATA_BYTES
            && description.get(self.byte_start..).is_some()
            && synopsis_fingerprint(description) == self.text_fingerprint)
            .then(|| Self { validated: true, ..self.clone() })
    }

    /// Constant-time access for the immutable snapshot validated at projection.
    /// Replacing that snapshot's description requires a new `validated_for` call.
    pub fn get<'a>(&self, description: &'a str) -> Option<&'a str> {
        (self.validated && description.len() == self.text_bytes)
            .then(|| description.get(self.byte_start..)).flatten()
    }
}

fn synopsis_fingerprint(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325_u64, |hash, byte|
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3))
}

impl std::fmt::Debug for CatalogEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatalogEntry").field("id", &self.id).field("category", &self.category).field("media", &self.media).finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub entries: Vec<CatalogEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hierarchy: Option<CategoryHierarchy>,
}

impl Catalog {
    /// First-seen order is retained. The UI adds its reserved All and Favorites choices.
    pub fn categories(&self) -> Vec<String> {
        self.category_choices().into_iter().map(|choice| choice.label).collect()
    }

    /// Atomic bounded merge of background batches. Repeated page entries replace their old row.
    pub fn merge(&mut self, batch: Catalog) -> Result<(), CatalogError> {
        self.merge_with_cost(batch, model_bytes)
    }

    fn merge_with_cost(&mut self, batch: Catalog, cost: impl FnMut(&CatalogEntry) -> usize)
        -> Result<(), CatalogError>
    {
        // Catalog is publicly mutable: rebuild from the actual rows, never trust
        // a cached count or a previous validation. One borrowed index records
        // both each destination and its final winner (including repeated pages).
        let (destinations, hierarchy) = {
            let mut rows: std::collections::BTreeMap<&str, (usize, &CatalogEntry)> = self.entries
                .iter().enumerate().map(|(index, entry)| (entry.id.as_str(), (index, entry))).collect();
            let mut next = self.entries.len();
            let destinations: Vec<_> = batch.entries.iter().map(|entry| {
                let row = rows.entry(entry.id.as_str()).or_insert_with(|| {
                    let index = next;
                    next += 1;
                    (index, entry)
                });
                row.1 = entry;
                row.0
            }).collect();
            if rows.len() > MAX_CATALOG_ENTRIES { return Err(CatalogError::LimitExceeded); }
            let mut hierarchy = self.hierarchy.clone();
            if let Some(incoming) = &batch.hierarchy {
                hierarchy.get_or_insert_with(CategoryHierarchy::default).merge(incoming)?;
            }
            let rows_bytes = row_model_bytes_with(rows.values().map(|(_, entry)| *entry), cost)?;
            let hierarchy_bytes = if let Some(hierarchy) = &hierarchy {
                let ids: BTreeSet<&str> = rows.keys().copied().collect();
                hierarchy.validated_model_bytes(&ids)?
            } else { 0 };
            if rows_bytes.checked_add(hierarchy_bytes).is_none_or(|bytes| bytes > MAX_CATALOG_MODEL_BYTES) {
                return Err(CatalogError::LimitExceeded);
            }
            (destinations, hierarchy)
        };
        // All fallible admissions completed before either rows or graph change.
        for (entry, index) in batch.entries.into_iter().zip(destinations) {
            if index < self.entries.len() { self.entries[index] = entry; }
            else { self.entries.push(entry); }
        }
        self.hierarchy = hierarchy;
        Ok(())
    }
}

fn model_bytes(entry:&CatalogEntry)->usize {
    let media=match &entry.media {
        MediaLocator::Http {url}=>url.len(),MediaLocator::Local {relative_path}=>relative_path.len(),
        MediaLocator::VPortal {request}=>json_model_bytes(request),
        MediaLocator::Plex {server_id,rating_key}=>server_id.len()+rating_key.len(),
        MediaLocator::Xtream {stream_id,stream_kind,extension,series_id,..}=>stream_id.len()+stream_kind.len()+extension.len()+series_id.as_ref().map_or(0,String::len),
        MediaLocator::Stalker {cmd,stream_kind,series}=>cmd.len()+stream_kind.len()+series.len(),
        MediaLocator::StalkerApi {channel_id}=>channel_id.len(),
        MediaLocator::ServerLibrary {source_id,item_id,relative_path,root_alias}=>source_id.len()+item_id.len()+relative_path.as_ref().map_or(0,String::len)+root_alias.as_ref().map_or(0,String::len),
    };
    (entry.id.len()+entry.title.len()+entry.category.len()+entry.description.len()+entry.poster.len()
        +entry.provider_id.as_ref().map_or(0,String::len)+entry.provider_id_field.as_ref().map_or(0,String::len)+entry.live.as_ref().map_or(0,|live|live.tvg_name.len()+live.catchup.as_ref().map_or(0,|catchup|catchup.source.len()))+std::mem::size_of::<CatalogEntry>()).saturating_add(media)
}

// Shared with the borrowed checked-category projection. Its caller supplies
// the same final row winners that merge would retain for accounting.
fn row_model_bytes<'a>(entries: impl IntoIterator<Item = &'a CatalogEntry>) -> Result<usize, CatalogError> {
    row_model_bytes_with(entries, model_bytes)
}

fn row_model_bytes_with<'a>(entries: impl IntoIterator<Item = &'a CatalogEntry>,
    mut cost: impl FnMut(&CatalogEntry) -> usize) -> Result<usize, CatalogError>
{
    entries.into_iter().try_fold(0usize, |total, entry| total.checked_add(cost(entry)))
        .ok_or(CatalogError::LimitExceeded)
}

#[derive(Default)]
struct JsonLength(usize);
impl std::io::Write for JsonLength {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

fn json_model_bytes(value: &serde_json::Value) -> usize {
    // Use the exact compact serializer, including escaping and number formats,
    // but retain only its length. No private request is copied into an output buffer.
    let mut length = JsonLength::default();
    match serde_json::to_writer(&mut length, value) {
        Ok(()) => length.0,
        Err(_) => usize::MAX,
    }
}
pub(super) fn text(value: &str, limit: usize) -> String {
    let mut output = String::new();
    for ch in value.trim().chars() {
        let ch = if ch.is_control() && ch != '\n' && ch != '\t' { ' ' } else { ch };
        if output.len() + ch.len_utf8() > limit { break; }
        output.push(ch);
    }
    output
}

/// Deterministic non-security fingerprint; never used for authentication or credential hiding.
pub(super) fn identity(profile: &str, value: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.bytes() { hash ^= u64::from(byte); hash = hash.wrapping_mul(0x100000001b3); }
    format!("{profile}:{hash:016x}")
}

#[cfg(test)]
#[path = "catalog_merge_tests.rs"]
mod merge_tests;

#[cfg(test)]
mod synopsis_tests;
