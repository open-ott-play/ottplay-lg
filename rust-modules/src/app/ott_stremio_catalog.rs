//! Pure Stremio movie catalogue adaptation, not a second provider runtime.
//!
//! The caller owns the configured add-on identity, admitted base URL, deadline,
//! cancellation and current generation. Plans contain relative paths only and
//! must execute under that same captured authority. A manifest cannot substitute
//! a host, a network client or a fresh operation deadline.
//!
//! Catalogue metadata is not playable. A stream request is the next metadata
//! step; its response still needs separate source validation and user selection.

use plx_plex::plex::urlenc_str;
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    InvalidManifest,
    ConfigurationRequired,
    Unsupported,
    SearchRequired,
    InvalidRequest,
    InvalidPage,
    ResponseLimit,
}

#[derive(Clone, Debug)]
pub(super) struct Catalog {
    pub id: String,
    pub title: String,
    pub can_browse: bool,
    pub can_search: bool,
    pub can_page: bool,
}

#[derive(Clone)]
struct StreamResource {
    prefixes: Option<Vec<String>>,
}

pub(super) struct Manifest {
    pub id: String,
    pub name: String,
    catalogs: Vec<Catalog>,
    streams: Vec<StreamResource>,
}

impl Manifest {
    /// Same bounded movie-only resource surface as the Classic add-on adapter.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > 65_536 {
            return Err(Error::ResponseLimit);
        }
        let value: Value = serde_json::from_slice(bytes).map_err(|_| Error::InvalidManifest)?;
        let id = text(&value["id"], 256).ok_or(Error::InvalidManifest)?;
        let name = text(&value["name"], 256).ok_or(Error::InvalidManifest)?;
        let types = strings(&value["types"], 32)?;
        let prefixes = optional_strings(value.get("idPrefixes"), 64)?;
        let resources = array(&value["resources"], 32)?;
        let catalogs = array(&value["catalogs"], 100)?;
        if value["behaviorHints"]["configurationRequired"] == Value::Bool(true) {
            return Err(Error::ConfigurationRequired);
        }
        let mut streams = Vec::new();
        for resource in resources {
            let selected = if resource.as_str() == Some("stream") {
                Some((types.clone(), prefixes.clone()))
            } else if resource["name"].as_str() == Some("stream") {
                Some((
                    strings(&resource["types"], 32)?,
                    optional_strings(resource.get("idPrefixes"), 64)?,
                ))
            } else {
                None
            };
            if let Some((types, prefixes)) = selected {
                if types.iter().any(|value| value == "movie") {
                    streams.push(StreamResource { prefixes });
                }
            }
        }
        let mut ids = BTreeSet::new();
        let mut admitted = Vec::new();
        for catalog in catalogs {
            if catalog["type"].as_str() != Some("movie") {
                continue;
            }
            let id = text(&catalog["id"], 256).ok_or(Error::InvalidManifest)?;
            if id != catalog["id"].as_str().unwrap_or_default() || !ids.insert(id.clone()) {
                return Err(Error::InvalidManifest);
            }
            let extras = extras(catalog)?;
            let other_required = extras
                .iter()
                .any(|(name, required)| *required && name != "search" && name != "skip");
            let search_required = extras
                .iter()
                .any(|(name, required)| name == "search" && *required);
            admitted.push(Catalog {
                title: text(&catalog["name"], 256).unwrap_or_else(|| id.clone()),
                id,
                can_browse: !other_required && !search_required,
                can_search: !other_required && extras.iter().any(|(name, _)| name == "search"),
                can_page: extras.iter().any(|(name, _)| name == "skip"),
            });
        }
        Ok(Self {
            id,
            name,
            catalogs: admitted,
            streams,
        })
    }

    pub fn catalogs(&self) -> &[Catalog] {
        &self.catalogs
    }

    pub fn has_movie_streams(&self) -> bool {
        !self.streams.is_empty()
    }

    pub fn catalog_request(
        &self,
        catalog_id: &str,
        query: Option<&str>,
        offset: usize,
    ) -> Result<CatalogRequest, Error> {
        let catalog = self
            .catalogs
            .iter()
            .find(|catalog| catalog.id == catalog_id)
            .ok_or(Error::Unsupported)?;
        if query.is_none() && !catalog.can_browse {
            return Err(if catalog.can_search {
                Error::SearchRequired
            } else {
                Error::Unsupported
            });
        }
        if query.is_some() && !catalog.can_search {
            return Err(Error::Unsupported);
        }
        if offset > plx_data::ott::MAX_CATALOG_ENTRIES || (offset > 0 && !catalog.can_page) {
            return Err(Error::InvalidRequest);
        }
        let mut parts = Vec::new();
        if let Some(query) = query {
            let query = bounded_text(query, 256).ok_or(Error::InvalidRequest)?;
            parts.push(format!("search={}", urlenc_str(&query)));
        }
        if catalog.can_page {
            parts.push(format!("skip={offset}"));
        }
        let extras = if parts.is_empty() {
            String::new()
        } else {
            format!("/{}", parts.join("&"))
        };
        Ok(CatalogRequest {
            path: format!("catalog/movie/{}{extras}.json", urlenc_str(&catalog.id)),
            catalog_id: catalog.id.clone(),
            offset,
            paged: catalog.can_page,
        })
    }

    /// Candidates are already-resolved movie identifiers in priority order
    /// (for example IMDb, then TMDB). Do not infer another film from its title.
    pub fn stream_request(&self, candidates: &[&str]) -> Result<Option<StreamRequest>, Error> {
        for candidate in candidates {
            let id = bounded_text(candidate, 256).ok_or(Error::InvalidRequest)?;
            if id != *candidate {
                return Err(Error::InvalidRequest);
            }
            if self.streams.iter().any(|resource| {
                resource
                    .prefixes
                    .as_ref()
                    .is_none_or(|prefixes| prefixes.iter().any(|prefix| id.starts_with(prefix)))
            }) {
                return Ok(Some(StreamRequest {
                    path: format!("stream/movie/{}.json", urlenc_str(&id)),
                    media_id: id,
                }));
            }
        }
        Ok(None)
    }
}

pub(super) struct CatalogRequest {
    path: String,
    pub catalog_id: String,
    offset: usize,
    paged: bool,
}

impl CatalogRequest {
    /// Append to the previously admitted add-on base without replacing its
    /// authority or interpreting the path as another URL.
    pub fn relative_path(&self) -> &str {
        &self.path
    }

    /// The caller checks its pending operation's generation/deadline before
    /// publishing. Offset follows raw provider rows, not filtered/deduped rows.
    pub fn decode_page(&self, bytes: &[u8]) -> Result<Page, Error> {
        if bytes.len() > 524_288 {
            return Err(Error::ResponseLimit);
        }
        let value: Value = serde_json::from_slice(bytes).map_err(|_| Error::InvalidPage)?;
        let rows = value["metas"]
            .as_array()
            .filter(|rows| rows.len() <= 1000)
            .ok_or(Error::InvalidPage)?;
        let mut seen = BTreeSet::new();
        let mut movies = Vec::new();
        for row in rows {
            if let Some(kind) = row.get("type") {
                match kind {
                    Value::Null => {}
                    Value::String(kind) if kind == "movie" => {}
                    Value::String(_) => continue,
                    _ => return Err(Error::InvalidPage),
                }
            }
            let id = text(&row["id"], 256).ok_or(Error::InvalidPage)?;
            if id != row["id"].as_str().unwrap_or_default() {
                return Err(Error::InvalidPage);
            }
            let title = text(&row["name"], 512)
                .or_else(|| text(&row["title"], 512))
                .ok_or(Error::InvalidPage)?;
            if !seen.insert(id.clone()) {
                continue;
            }
            movies.push(Movie {
                id,
                title,
                description: row["description"]
                    .as_str()
                    .filter(|value| {
                        value.len() <= 8192
                            && !value
                                .chars()
                                .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
                    })
                    .unwrap_or_default()
                    .trim()
                    .into(),
                artwork: text(&row["poster"], 2048).map(|candidate| ArtworkProposal { candidate }),
                release_info: text(&row["releaseInfo"], 64),
            });
        }
        let next_offset = if self.paged && !rows.is_empty() {
            self.offset
                .checked_add(rows.len())
                .filter(|offset| *offset <= plx_data::ott::MAX_CATALOG_ENTRIES)
        } else {
            None
        };
        Ok(Page {
            movies,
            next_offset,
            received_count: rows.len(),
        })
    }
}

pub(super) struct StreamRequest {
    path: String,
    pub media_id: String,
}
impl StreamRequest {
    pub fn relative_path(&self) -> &str {
        &self.path
    }
}

/// A candidate from untrusted metadata, not an admitted image fetch URL. The
/// common artwork loader must apply its usual URI/authority policy before IO.
#[derive(Clone)]
pub(super) struct ArtworkProposal {
    candidate: String,
}
impl ArtworkProposal {
    /// Preserve an untrusted metadata candidate for the existing artwork gate.
    /// Constructing this value neither admits a destination nor starts a fetch.
    pub fn from_candidate(candidate: String) -> Self {
        Self { candidate }
    }

    pub fn candidate(&self) -> &str {
        &self.candidate
    }
}

pub(super) struct Movie {
    /// Namespace with the configured add-on identity, never manifest.id alone.
    pub id: String,
    pub title: String,
    pub description: String,
    pub artwork: Option<ArtworkProposal>,
    pub release_info: Option<String>,
}

pub(super) struct Page {
    pub movies: Vec<Movie>,
    pub next_offset: Option<usize>,
    /// Lets the existing pager detect no-progress/repeated-page behaviour.
    pub received_count: usize,
}

fn bounded_text(value: &str, limit: usize) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty() && value.len() <= limit && !value.chars().any(char::is_control))
        .then(|| trimmed.into())
}
fn text(value: &Value, limit: usize) -> Option<String> {
    value.as_str().and_then(|value| bounded_text(value, limit))
}
fn array(value: &Value, limit: usize) -> Result<&Vec<Value>, Error> {
    value
        .as_array()
        .filter(|values| values.len() <= limit)
        .ok_or(Error::InvalidManifest)
}
fn strings(value: &Value, limit: usize) -> Result<Vec<String>, Error> {
    array(value, limit)?
        .iter()
        .map(|value| text(value, 128).ok_or(Error::InvalidManifest))
        .collect()
}
fn optional_strings(value: Option<&Value>, limit: usize) -> Result<Option<Vec<String>>, Error> {
    value.map(|value| strings(value, limit)).transpose()
}
fn extras(catalog: &Value) -> Result<Vec<(String, bool)>, Error> {
    if let Some(extra) = catalog.get("extra") {
        return array(extra, 32)?
            .iter()
            .map(|extra| {
                let name = text(&extra["name"], 128).ok_or(Error::InvalidManifest)?;
                let required = match extra.get("isRequired") {
                    None => false,
                    Some(value) => value.as_bool().ok_or(Error::InvalidManifest)?,
                };
                Ok((name, required))
            })
            .collect();
    }
    let supported = optional_strings(catalog.get("extraSupported"), 32)?.unwrap_or_default();
    let required = optional_strings(catalog.get("extraRequired"), 32)?.unwrap_or_default();
    if required.iter().any(|name| !supported.contains(name)) {
        return Err(Error::InvalidManifest);
    }
    Ok(supported
        .into_iter()
        .map(|name| {
            let is_required = required.contains(&name);
            (name, is_required)
        })
        .collect())
}

#[cfg(test)]
#[path = "ott_stremio_catalog/tests.rs"]
mod tests;
