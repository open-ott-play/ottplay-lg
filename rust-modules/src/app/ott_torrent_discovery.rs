//! Pure built-in movie discovery through an explicitly configured TMDB service.
//!
//! Relative requests omit the service's `/tmdb/s/` prefix. The existing worker
//! supplies the admitted service base, deadline, cancellation and HTTP reply.
//! Nothing here chooses an endpoint, reads an API key or starts playback.

use super::ott_stremio_catalog::{ArtworkProposal, Movie as CatalogMovie};
use super::ott_stremio_settings;
use plx_plex::plex::urlenc_str;
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    InvalidRequest,
    InvalidResponse,
    ResponseLimit,
    HttpStatus(u16),
    ApiError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CategoryRow {
    pub id: String,
    pub title: String,
}

/// The shared catalog presentation plus identifiers needed by stream add-ons.
/// These are metadata, not source URLs or persisted playable capabilities.
pub(super) struct Movie {
    pub metadata: CatalogMovie,
    pub imdb_id: Option<String>,
}

impl Movie {
    /// Feed this priority order to the existing Manifest::stream_request.
    pub fn stream_ids(&self) -> Result<Vec<String>, Error> {
        let number = movie_number(&self.metadata.id).ok_or(Error::InvalidRequest)?;
        let mut ids = Vec::new();
        if let Some(id) = &self.imdb_id {
            if !imdb_id(id) {
                return Err(Error::InvalidRequest);
            }
            ids.push(id.clone());
        }
        ids.push(format!("tmdb:{number}"));
        ids.push(self.metadata.id.clone());
        Ok(ids)
    }
}

pub(super) struct Page {
    pub movies: Vec<Movie>,
    pub next_page: Option<u16>,
    /// The existing pager uses raw receipt count and new IDs to stop repeats.
    pub received_count: usize,
}

pub(super) struct CategoriesRequest {
    path: String,
}

impl CategoriesRequest {
    pub fn new(locale: &str) -> Result<Self, Error> {
        Ok(Self {
            path: format!("genre/movie/list?language={}", locale_query(locale)?),
        })
    }

    pub fn relative_path(&self) -> &str {
        &self.path
    }

    pub fn decode(&self, status: u16, bytes: &[u8]) -> Result<Vec<CategoryRow>, Error> {
        let value = document(status, bytes)?;
        let genres = value["genres"].as_array().ok_or(Error::InvalidResponse)?;
        if genres.len() > 100 {
            return Err(Error::ResponseLimit);
        }
        let mut categories = vec![
            CategoryRow {
                id: "popular".into(),
                title: "Popular".into(),
            },
            CategoryRow {
                id: "new".into(),
                title: "New".into(),
            },
        ];
        let mut seen = BTreeSet::new();
        for genre in genres {
            let id = number_id(&genre["id"]).ok_or(Error::InvalidResponse)?;
            let title = text(&genre["name"], 128).ok_or(Error::InvalidResponse)?;
            if seen.insert(id.clone()) {
                categories.push(CategoryRow {
                    id: format!("genre:{id}"),
                    title,
                });
            }
        }
        Ok(categories)
    }
}

pub(super) struct PageRequest {
    path: String,
    page: u16,
}

impl PageRequest {
    pub fn new(
        category: &str,
        query: Option<&str>,
        page: u16,
        locale: &str,
    ) -> Result<Self, Error> {
        if !(1..=500).contains(&page) {
            return Err(Error::InvalidRequest);
        }
        let genre = if let Some(id) = category.strip_prefix("genre:") {
            if !numeric_id(id) {
                return Err(Error::InvalidRequest);
            }
            Some(id)
        } else if matches!(category, "popular" | "new") || (category == "search" && query.is_some())
        {
            None
        } else {
            return Err(Error::InvalidRequest);
        };
        let query = query
            .map(|value| plain(value, 256).ok_or(Error::InvalidRequest))
            .transpose()?;
        let route = if query.is_some() {
            "search/movie"
        } else if genre.is_some() {
            "discover/movie"
        } else if category == "new" {
            "movie/now_playing"
        } else {
            "movie/popular"
        };
        let mut path = format!(
            "{route}?language={}&page={page}&include_adult=false",
            locale_query(locale)?
        );
        if let Some(query) = query {
            path.push_str("&query=");
            path.push_str(&urlenc_str(&query));
        }
        if let Some(genre) = genre {
            path.push_str("&with_genres=");
            path.push_str(genre);
        }
        Ok(Self { path, page })
    }

    pub fn relative_path(&self) -> &str {
        &self.path
    }

    pub fn decode(&self, status: u16, bytes: &[u8]) -> Result<Page, Error> {
        let value = document(status, bytes)?;
        if integer(&value["page"]) != Some(u64::from(self.page)) {
            return Err(Error::InvalidResponse);
        }
        let total_pages = integer(&value["total_pages"]).ok_or(Error::InvalidResponse)?;
        let rows = value["results"].as_array().ok_or(Error::InvalidResponse)?;
        if rows.len() > 100 {
            return Err(Error::ResponseLimit);
        }
        let mut seen = BTreeSet::new();
        let mut movies = Vec::new();
        for row in rows {
            let movie = movie(row)?;
            if seen.insert(movie.metadata.id.clone()) {
                movies.push(movie);
            }
        }
        Ok(Page {
            movies,
            next_page: (total_pages > u64::from(self.page) && self.page < 500)
                .then_some(self.page + 1),
            received_count: rows.len(),
        })
    }
}

pub(super) struct DetailsRequest {
    path: String,
    id: String,
}

impl DetailsRequest {
    pub fn new(id: &str, locale: &str) -> Result<Self, Error> {
        let number = movie_number(id).ok_or(Error::InvalidRequest)?;
        Ok(Self {
            path: format!(
                "movie/{number}?language={}&append_to_response=external_ids",
                locale_query(locale)?
            ),
            id: id.into(),
        })
    }

    pub fn relative_path(&self) -> &str {
        &self.path
    }

    pub fn decode(&self, status: u16, bytes: &[u8]) -> Result<Movie, Error> {
        let movie = movie(&document(status, bytes)?)?;
        if movie.metadata.id != self.id {
            return Err(Error::InvalidResponse);
        }
        Ok(movie)
    }
}

fn locale_query(value: &str) -> Result<String, Error> {
    ott_stremio_settings::language(value)
        .map(|locale| urlenc_str(&locale))
        .map_err(|_| Error::InvalidRequest)
}

fn document(status: u16, bytes: &[u8]) -> Result<Value, Error> {
    if !(200..300).contains(&status) {
        return Err(Error::HttpStatus(status));
    }
    if bytes.len() > 524_288 {
        return Err(Error::ResponseLimit);
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| Error::InvalidResponse)?;
    if !value.is_object() {
        return Err(Error::InvalidResponse);
    }
    if value["success"] == false {
        return Err(Error::ApiError);
    }
    Ok(value)
}

fn movie(value: &Value) -> Result<Movie, Error> {
    let id = number_id(&value["id"]).ok_or(Error::InvalidResponse)?;
    let title = text(&value["title"], 512)
        .or_else(|| text(&value["original_title"], 512))
        .ok_or(Error::InvalidResponse)?;
    let artwork = text(&value["poster_path"], 1024)
        .filter(|path| {
            path.starts_with('/')
                && path.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'/' | b'-')
                })
        })
        .map(|path| {
            ArtworkProposal::from_candidate(format!("https://image.tmdb.org/t/p/w342{path}"))
        });
    let release_info = text(&value["release_date"], 64).and_then(|date| {
        let bytes = date.as_bytes();
        if bytes.len() >= 4
            && matches!(bytes[0], b'1' | b'2')
            && bytes[1..4].iter().all(u8::is_ascii_digit)
            && bytes.get(4).is_none_or(|byte| !byte.is_ascii_digit())
        {
            Some(date.chars().take(4).collect())
        } else {
            None
        }
    });
    let imdb_id = value["imdb_id"]
        .as_str()
        .filter(|value| imdb_id(value))
        .or_else(|| {
            value["external_ids"]["imdb_id"]
                .as_str()
                .filter(|value| imdb_id(value))
        })
        .map(str::to_owned);
    Ok(Movie {
        metadata: CatalogMovie {
            id: format!("tmdb:movie:{id}"),
            title,
            description: text(&value["overview"], 8192).unwrap_or_default(),
            artwork,
            release_info,
        },
        imdb_id,
    })
}

fn text(value: &Value, limit: usize) -> Option<String> {
    value.as_str().and_then(|value| plain(value, limit))
}

fn plain(value: &str, limit: usize) -> Option<String> {
    (value.len() <= limit
        && !value
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t')))
    .then(|| value.trim().to_owned())
    .filter(|value| !value.is_empty())
}

fn numeric_id(value: &str) -> bool {
    (1..=15).contains(&value.len())
        && value.as_bytes()[0] != b'0'
        && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn number_id(value: &Value) -> Option<String> {
    if let Some(value) = value.as_str() {
        return numeric_id(value).then(|| value.to_owned());
    }
    integer(value)
        .filter(|id| (1..=999_999_999_999_999).contains(id))
        .map(|id| id.to_string())
}

fn integer(value: &Value) -> Option<u64> {
    if let Some(number) = value.as_u64() {
        return Some(number);
    }
    value
        .as_f64()
        .filter(|number| {
            number.is_finite()
                && *number >= 0.0
                && number.fract() == 0.0
                && *number < u64::MAX as f64
        })
        .map(|number| number as u64)
}

fn movie_number(value: &str) -> Option<&str> {
    value
        .strip_prefix("tmdb:movie:")
        .filter(|number| numeric_id(number))
}

fn imdb_id(value: &str) -> bool {
    value.strip_prefix("tt").is_some_and(|digits| {
        (7..=10).contains(&digits.len()) && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}

#[cfg(test)]
#[path = "ott_torrent_discovery/tests.rs"]
mod tests;
