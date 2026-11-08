//! Pure projection of the existing companion's Torznab replies into provider rows.
//!
//! The runtime remains the only catalogue, queue and persistence owner. It submits
//! `SearchPlan::request` through the existing companion adapter, preserving that
//! adapter's captured deadline, generation and cancellation. No method here does
//! IO, renews a deadline or authorizes a playback route.
//!
//! Result handles expire in the companion and are deliberately NOT serializable.
//! A row ID is valid only within one captured catalogue generation. Preparing a
//! result yields a separate, companion-local source reference through the existing
//! protocol; neither reference is a portable Favorites identity.

use super::ott_companion::protocol::{
    IndexerCapabilities, IndexerRequest, IndexerSource, Request, SearchItem, SearchPage,
};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Category {
    pub id: String,
    pub title: String,
    pub search_supported: bool,
    source_id: String,
    indexer_id: Option<String>,
    category_ids: Vec<u64>,
}

/// Receives already-decoded companion metadata. Missing capabilities keep the
/// source visible, but cannot make it searchable or fabricate any category IDs.
pub(super) fn categories(
    source: &IndexerSource,
    caps: Option<&IndexerCapabilities>,
) -> Result<Vec<Category>, &'static str> {
    categories_scoped(source, caps, None)
}

/// Child selection is explicit. A missing or rejected child never broadens to
/// the aggregate source; callers must supply the exact scope on every page.
pub(super) fn categories_scoped(
    source: &IndexerSource,
    caps: Option<&IndexerCapabilities>,
    indexer_id: Option<&str>,
) -> Result<Vec<Category>, &'static str> {
    Request::Indexer(match indexer_id {
        Some(indexer_id) => IndexerRequest::ScopedCapabilities {
            source_id: source.id.clone(),
            indexer_id: indexer_id.into(),
        },
        None => IndexerRequest::Capabilities {
            id: source.id.clone(),
        },
    })
    .validate()
    .map_err(|error| error.code())?;
    if caps.is_some_and(|caps| caps.source_id != source.id) {
        return Err("torznab_source_changed");
    }
    let searchable = caps.is_some_and(|caps| caps.search_available);
    let mut result = vec![Category {
        id: format!("catalog:torznab:{}:all", source.id),
        title: source.label.clone(),
        search_supported: searchable,
        source_id: source.id.clone(),
        indexer_id: indexer_id.map(str::to_owned),
        category_ids: Vec::new(),
    }];
    if let Some(caps) = caps {
        result.extend(caps.categories.iter().map(|category| Category {
            id: format!("catalog:torznab:{}:{}", source.id, category.id),
            title: format!("{} / {}", source.label, category.name),
            search_supported: searchable,
            source_id: source.id.clone(),
            indexer_id: indexer_id.map(str::to_owned),
            category_ids: vec![category.id],
        }));
    }
    Ok(result)
}

pub(super) struct SearchInput<'a> {
    pub query: &'a str,
    pub offset: u64,
    /// The runtime supplies its remaining admission budget. There is no second
    /// catalogue cap here; the existing protocol still caps each response page.
    pub limit: usize,
    pub preferred_language: &'a str,
    pub generation: u64,
}

#[derive(Debug)]
pub(super) struct SearchPlan {
    request: Request,
    category: Category,
    language: String,
    generation: u64,
    offset: u64,
    limit: usize,
}

/// Captures a search selection, not network or playback authority. The caller
/// keeps this value in its existing pending operation, rather than a new queue.
pub(super) fn search_plan(
    category: &Category,
    caps: &IndexerCapabilities,
    input: SearchInput<'_>,
) -> Result<SearchPlan, &'static str> {
    if category.source_id != caps.source_id {
        return Err("torznab_source_changed");
    }
    if !caps.search_available || !category.search_supported {
        return Err("torznab_search_unavailable");
    }
    if category
        .category_ids
        .iter()
        .any(|id| !caps.categories.iter().any(|category| category.id == *id))
    {
        return Err("torznab_invalid_category");
    }
    let limit = input.limit.min(caps.max_limit);
    let request = Request::Indexer(match &category.indexer_id {
        Some(indexer_id) => IndexerRequest::ScopedSearch {
            source_id: category.source_id.clone(),
            indexer_id: indexer_id.clone(),
            query: input.query.into(),
            category_ids: category.category_ids.clone(),
            offset: input.offset,
            limit,
        },
        None => IndexerRequest::Search {
            source_id: category.source_id.clone(),
            query: input.query.into(),
            category_ids: category.category_ids.clone(),
            offset: input.offset,
            limit,
        },
    });
    request.validate().map_err(|error| error.code())?;
    Ok(SearchPlan {
        request,
        category: category.clone(),
        language: input.preferred_language.into(),
        generation: input.generation,
        offset: input.offset,
        limit,
    })
}

/// A transient result selection can only be used for immediate preparation. It
/// contains no stream URL, credentials or durable source reference.
#[derive(Clone)]
pub(super) struct ResultSelection {
    source_id: String,
    result_ref: String,
    generation: u64,
}

impl std::fmt::Debug for ResultSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResultSelection")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl ResultSelection {
    /// Even a matching generation does not prove the handle is unexpired. The
    /// companion is authoritative; `torznab_reference_missing` means refresh the
    /// search, never retry via a different source or persist this handle.
    pub fn prepare_request(&self, current_generation: u64) -> Result<Request, &'static str> {
        if current_generation != self.generation {
            return Err("torznab_reference_missing");
        }
        let request = Request::Indexer(IndexerRequest::Prepare {
            source_id: self.source_id.clone(),
            result_ref: self.result_ref.clone(),
        });
        request.validate().map_err(|error| error.code())?;
        Ok(request)
    }
}

/// Locale-neutral metadata; the common renderer owns labels and formatting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Facts {
    pub size_bytes: Option<u64>,
    pub seeders: Option<u64>,
    pub languages: Vec<String>,
    pub published_at: Option<String>,
}

#[derive(Clone)]
pub(super) struct Row {
    /// Ephemeral catalogue key; never save as a Favorite, resume or profile ID.
    pub id: String,
    pub title: String,
    pub facts: Facts,
    pub category: String,
    pub selection: ResultSelection,
}

impl std::fmt::Debug for Row {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Row")
            .field("category", &self.category)
            .field("selection", &self.selection)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(super) struct Page {
    pub rows: Vec<Row>,
    pub next_offset: Option<u64>,
    pub total: Option<u64>,
}

impl SearchPlan {
    pub fn request(&self) -> Request {
        self.request.clone()
    }

    /// Project an admitted protocol reply after the existing pending operation
    /// has checked its captured generation/cancellation/deadline. The explicit
    /// generation check also prevents accidental reuse by a refreshed catalogue.
    pub fn project(&self, page: SearchPage, current_generation: u64) -> Result<Page, &'static str> {
        self.project_scoped(page, None, current_generation)
    }

    pub fn project_scoped(
        &self,
        page: SearchPage,
        indexer_id: Option<&str>,
        current_generation: u64,
    ) -> Result<Page, &'static str> {
        if current_generation != self.generation {
            return Err("torznab_reference_missing");
        }
        if indexer_id != self.category.indexer_id.as_deref() {
            return Err("torznab_indexer_changed");
        }
        if page.source_id != self.category.source_id {
            return Err("torznab_source_changed");
        }
        if page.items.len() > self.limit
            || page
                .next_offset
                .is_some_and(|offset| offset <= self.offset || offset > 1_000_000)
        {
            return Err("companion_receipt_invalid");
        }
        let mut seen = BTreeSet::new();
        let mut rows = Vec::new();
        for item in page.items {
            let selection = ResultSelection {
                source_id: page.source_id.clone(),
                result_ref: item.result_ref.clone(),
                generation: self.generation,
            };
            selection.prepare_request(current_generation)?;
            if !seen.insert(item.result_ref.clone()) {
                continue;
            }
            let rank = language_rank(&item, &self.language);
            rows.push((
                rank,
                Row {
                    id: format!(
                        "torznab:{}:{}:{}",
                        self.generation, page.source_id, item.result_ref
                    ),
                    title: item.title.clone(),
                    facts: Facts {
                        size_bytes: item.size,
                        seeders: item.seeders,
                        languages: item.languages.clone(),
                        published_at: item.published_at.clone(),
                    },
                    category: self.category.title.clone(),
                    selection,
                },
            ));
        }
        // Stable sort preserves the indexer's order within each language group.
        // Only this page is ranked: later arrivals cannot jump ahead of playback
        // or keyboard focus by reordering previously admitted catalogue rows.
        rows.sort_by_key(|(rank, _)| *rank);
        Ok(Page {
            rows: rows.into_iter().map(|(_, row)| row).collect(),
            next_offset: page.next_offset,
            total: page.total,
        })
    }
}

fn language_rank(item: &SearchItem, preferred: &str) -> u8 {
    let preferred = language(preferred);
    if preferred.is_none() {
        return 0;
    }
    if item
        .languages
        .iter()
        .any(|value| language(value) == preferred)
    {
        0
    } else if item.languages.is_empty() {
        1
    } else {
        2
    }
}

pub(super) fn language(value: &str) -> Option<std::borrow::Cow<'static, str>> {
    // Classic Torznab accepts these full English language names in addition to
    // codes. All ISO code aliases and regional tags use the native shared table,
    // also used by audio/subtitle selection; do not grow a second ISO registry.
    let normalized = value.trim().to_ascii_lowercase();
    let value = match normalized.as_str() {
        "german" => "de",
        "english" => "en",
        "spanish" => "es",
        "french" => "fr",
        "italian" => "it",
        "russian" => "ru",
        value => value,
    };
    plx_data::metadata::lang_key(value)
}

#[cfg(test)]
#[path = "ott_torrent_provider/tests.rs"]
mod tests;
