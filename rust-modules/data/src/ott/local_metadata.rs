//! Explicit display metadata for existing Local files. This module performs no
//! IO, matching by title, path rebinding, Plex lookup or artwork request.
use super::{
    valid_local_relative_path, CatalogEntry, MediaLocator, MAX_CATALOG_BYTES, MAX_CATALOG_ENTRIES,
    MAX_CATALOG_MODEL_BYTES, MAX_METADATA_BYTES,
};
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidDocument,
    UnsupportedVersion,
    LimitExceeded,
    InvalidEntry,
    DuplicatePath,
}

/// The path is relative to the already-authorized Local root. A record cannot
/// create a source, choose a root, or turn a Plex identity into a Local identity.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalMetadata {
    pub relative_path: String,
    #[serde(default, deserialize_with = "present")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub duration_ms: Option<u64>,
    /// Metadata only. No current Local renderer is granted access to this path.
    /// It must never be assigned directly to CatalogEntry.poster as a file URL.
    #[serde(default, deserialize_with = "present")]
    pub poster_relative_path: Option<String>,
}

// Deserialize the inner value, not Option<T>: a present null is invalid, and
// therefore cannot hide a duplicate field from serde's generated duplicate gate.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    #[serde(deserialize_with = "bounded_items")]
    items: Vec<LocalMetadata>,
}

fn bounded_items<'de, D>(deserializer: D) -> Result<Vec<LocalMetadata>, D::Error>
where
    D: Deserializer<'de>,
{
    struct Items;
    impl<'de> Visitor<'de> for Items {
        type Value = Vec<LocalMetadata>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a bounded list of Local metadata records")
        }
        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut items = Vec::new();
            while let Some(item) = sequence.next_element()? {
                if items.len() >= MAX_CATALOG_ENTRIES {
                    return Err(de::Error::custom("Local metadata item limit exceeded"));
                }
                items.push(item);
            }
            Ok(items)
        }
    }
    deserializer.deserialize_seq(Items)
}

fn clean_text(value: &str, limit: usize, multiline: bool) -> bool {
    value.len() <= limit
        && !value
            .chars()
            .any(|ch| ch.is_control() && !(multiline && matches!(ch, '\n' | '\t')))
}

fn relative_file(path: &str) -> bool {
    valid_local_relative_path(path)
        // The shared Local path validator also admits ordinary POSIX colons.
        // Sidecars are portable data: URL syntax is never an artwork authority.
        && !path.contains(':')
}

impl LocalMetadata {
    fn validate(&self) -> Result<(), Error> {
        if !relative_file(&self.relative_path)
            || !super::is_local_video_path(&self.relative_path)
            || self
                .title
                .as_ref()
                .is_some_and(|value| value.trim().is_empty() || !clean_text(value, 1024, false))
            || self
                .description
                .as_ref()
                .is_some_and(|value| !clean_text(value, MAX_METADATA_BYTES, true))
            || self
                .duration_ms
                .is_some_and(|value| value == 0 || value > i64::MAX as u64)
            || self.poster_relative_path.as_ref().is_some_and(|path| {
                !relative_file(path)
                    || !matches!(
                        path.rsplit('.')
                            .next()
                            .unwrap_or("")
                            .to_ascii_lowercase()
                            .as_str(),
                        "jpg" | "jpeg" | "png"
                    )
            })
            || (self.title.is_none()
                && self.description.is_none()
                && self.duration_ms.is_none()
                && self.poster_relative_path.is_none())
        {
            return Err(Error::InvalidEntry);
        }
        Ok(())
    }

    fn model_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.relative_path.len()
            + self.title.as_ref().map_or(0, String::len)
            + self.description.as_ref().map_or(0, String::len)
            + self.poster_relative_path.as_ref().map_or(0, String::len)
    }
}

/// A completely validated mapping. Partial parsing cannot change a catalog.
/// It deliberately has no Serialize or Debug implementation: source paths and
/// descriptions are not diagnostic output or an implicit export format.
pub struct LocalMetadataMap {
    items: BTreeMap<String, LocalMetadata>,
}

impl LocalMetadataMap {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_CATALOG_BYTES {
            return Err(Error::LimitExceeded);
        }
        let document: Document =
            serde_json::from_slice(bytes).map_err(|_| Error::InvalidDocument)?;
        if document.version != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let mut items = BTreeMap::new();
        let mut model_bytes = 0usize;
        for item in document.items {
            item.validate()?;
            // The key is owned separately by the ordered map. Account for both
            // path strings before storing a record; the raw document has its
            // independent byte limit above.
            model_bytes = model_bytes
                .checked_add(item.model_bytes())
                .and_then(|bytes| bytes.checked_add(item.relative_path.len()))
                .ok_or(Error::LimitExceeded)?;
            if model_bytes > MAX_CATALOG_MODEL_BYTES {
                return Err(Error::LimitExceeded);
            }
            let path = item.relative_path.clone();
            if items.insert(path, item).is_some() {
                return Err(Error::DuplicatePath);
            }
        }
        Ok(Self { items })
    }

    /// Exact UTF-8 path lookup: no basename, case folding or Unicode rewriting.
    pub fn lookup(&self, relative_path: &str) -> Option<&LocalMetadata> {
        self.items.get(relative_path)
    }

    /// Overlay supplied display fields only. Identity, locator, category,
    /// provider fields and poster are untouched, and no row is created/reordered.
    /// Apply to a staged candidate, then admit the batch through Catalog::merge.
    /// This per-row helper cannot enforce the complete catalog's model budget;
    /// mutating live catalog rows directly would bypass that aggregate check.
    /// The caller may retain the returned poster path as pending metadata, but
    /// still needs a separate, root-bound artwork capability to display it.
    pub fn apply_to<'a>(&'a self, entry: &mut CatalogEntry) -> Option<&'a LocalMetadata> {
        let MediaLocator::Local { relative_path } = &entry.media else {
            return None;
        };
        let metadata = self.lookup(relative_path)?;
        if let Some(title) = &metadata.title {
            entry.title.clone_from(title);
        }
        if let Some(description) = &metadata.description {
            entry.description.clone_from(description);
        }
        if let Some(duration_ms) = metadata.duration_ms {
            entry.duration_ms = Some(duration_ms);
        }
        Some(metadata)
    }
}

#[cfg(test)]
#[path = "local_metadata_tests.rs"]
mod tests;
