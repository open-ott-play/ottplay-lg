//! Authenticated classic MAG requests; called only from the provider worker.
use super::{ProviderHttp, Request};
use plx_data::ott::{
    self,
    stalker::{self, Config, Folder, Pages},
    CatalogEntry, MediaLocator, ProviderProfile,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

enum Failure {
    Authorization,
    Other(&'static str),
}
impl Failure {
    fn message(self) -> &'static str {
        match self {
            Self::Authorization => "Stalker authorization failed",
            Self::Other(message) => message,
        }
    }
}

struct Session<'a> {
    config: Config,
    http: &'a mut dyn ProviderHttp,
    token: String,
    refreshed: bool,
    remaining: usize,
}
impl<'a> Session<'a> {
    fn connect(
        profile: &ProviderProfile,
        http: &'a mut dyn ProviderHttp,
    ) -> Result<Self, &'static str> {
        profile.validate().map_err(|_| "Invalid Stalker profile")?;
        if profile.kind != ott::ProviderKind::Stalker {
            return Err("Invalid Stalker profile");
        }
        let config = Config::parse(&profile.source).map_err(|_| "Invalid Stalker profile")?;
        if config.dialect() != stalker::Dialect::Mag {
            return Err("Stalker dialect mismatch");
        }
        let mut session = Self {
            config,
            http,
            token: String::new(),
            refreshed: false,
            remaining: 4096,
        };
        session.handshake()?;
        let profile = session.call("stb", "get_profile", &session.config.profile_params())?;
        stalker::validate_profile(&profile)?;
        Ok(session)
    }
    fn raw(
        &mut self,
        kind: &str,
        action: &str,
        params: &[(&str, String)],
        authenticated: bool,
    ) -> Result<Value, Failure> {
        if self.http.cancelled() {
            return Err(Failure::Other("Provider request cancelled"));
        }
        if self.remaining == 0 {
            return Err(Failure::Other("Stalker request limit reached"));
        }
        self.remaining -= 1;
        let response = self
            .http
            .request(&Request {
                url: self.config.url(kind, action, params),
                headers: self.config.headers(if authenticated {
                    Some(&self.token)
                } else {
                    None
                }),
                body: None,
            })
            .map_err(|error| {
                if error == "Provider authorization failed" {
                    Failure::Authorization
                } else {
                    Failure::Other(error)
                }
            })?;
        if self.http.cancelled() {
            return Err(Failure::Other("Provider request cancelled"));
        }
        if matches!(response.status, 401 | 403) {
            return Err(Failure::Authorization);
        }
        if !(200..300).contains(&response.status) {
            return Err(Failure::Other("Stalker request rejected"));
        }
        if response.body.len() > ott::MAX_CATALOG_BYTES {
            return Err(Failure::Other("Stalker response exceeds limit"));
        }
        let response: Value = serde_json::from_slice(&response.body)
            .map_err(|_| Failure::Other("Invalid Stalker JSON response"))?;
        stalker::unwrap(&response).cloned().map_err(|error| {
            if error == "Stalker authorization failed" {
                Failure::Authorization
            } else {
                Failure::Other(error)
            }
        })
    }
    fn handshake(&mut self) -> Result<(), &'static str> {
        let response = self
            .raw("stb", "handshake", &[("token", String::new())], false)
            .map_err(Failure::message)?;
        self.token = stalker::token(&response)?;
        Ok(())
    }
    fn call(
        &mut self,
        kind: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> Result<Value, &'static str> {
        match self.raw(kind, action, params, true) {
            Ok(response) => Ok(response),
            Err(Failure::Authorization) if !self.refreshed => {
                self.refreshed = true;
                self.handshake()?;
                let profile = self
                    .raw("stb", "get_profile", &self.config.profile_params(), true)
                    .map_err(Failure::message)?;
                stalker::validate_profile(&profile)?;
                if kind == "stb" && action == "get_profile" {
                    return Ok(profile);
                }
                self.raw(kind, action, params, true)
                    .map_err(Failure::message)
            }
            Err(error) => Err(error.message()),
        }
    }
}

pub(super) fn visit(
    profile: &ProviderProfile,
    http: &mut dyn ProviderHttp,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let mut session = Session::connect(profile, http)?;
    let groups = stalker::categories(&session.call("itv", "get_genres", &[])?)?;
    let mut seen = BTreeSet::new();
    visit_folder(
        profile,
        &mut session,
        "itv",
        &groups,
        &Folder::default(),
        &mut seen,
        emit,
    )?;
    if !session.config.include_vod {
        return Ok(());
    }
    let categories = stalker::categories(&session.call("vod", "get_categories", &[])?)?;
    let mut pending: VecDeque<_> = categories
        .iter()
        .filter(|(id, _)| id.as_str() != "*")
        .map(|(category, name)| Folder {
            category: category.clone(),
            name: name.clone(),
            ..Folder::default()
        })
        .collect();
    // A portal can omit uncategorized records from get_categories.
    pending.push_back(Folder {
        category: "*".into(),
        name: "Movies".into(),
        ..Folder::default()
    });
    let mut visited = BTreeSet::new();
    while let Some(folder) = pending.pop_front() {
        if !visited.insert((
            if folder.movie.is_empty() {
                folder.category.clone()
            } else {
                String::new()
            },
            folder.movie.clone(),
            folder.season.clone(),
            folder.episode.clone(),
        )) {
            continue;
        }
        if visited.len() > 4096 {
            return Err("Stalker folder count exceeds limit");
        }
        let children = visit_folder(
            profile,
            &mut session,
            "vod",
            &categories,
            &folder,
            &mut seen,
            emit,
        )?;
        if pending.len().saturating_add(children.len()) > 4096 {
            return Err("Stalker folder count exceeds limit");
        }
        pending.extend(children);
    }
    Ok(())
}

fn visit_folder(
    profile: &ProviderProfile,
    session: &mut Session<'_>,
    kind: &str,
    groups: &BTreeMap<String, String>,
    folder: &Folder,
    seen: &mut BTreeSet<String>,
    emit: &mut dyn FnMut(Vec<CatalogEntry>) -> Result<(), &'static str>,
) -> Result<Vec<Folder>, &'static str> {
    let mut pages = Pages::default();
    let mut number = 1;
    let mut children = Vec::new();
    while !pages.done {
        let mut params = vec![
            ("genre", "*".into()),
            ("p", number.to_string()),
            ("fav", "0".into()),
            (
                "sortby",
                if kind == "itv" { "number" } else { "name" }.into(),
            ),
            ("hd", "0".into()),
        ];
        if kind == "vod" {
            params.push(("category", folder.category.clone()));
            for (key, value) in [
                ("movie_id", &folder.movie),
                ("season_id", &folder.season),
                ("episode_id", &folder.episode),
            ] {
                if !value.is_empty() {
                    params.push((key, value.clone()));
                }
            }
        }
        let data = session.call(kind, "get_ordered_list", &params)?;
        let page = pages.accept(&data)?;
        let (catalog, folders) =
            stalker::rows(profile, &session.config, kind, groups, folder, &page)?;
        if children.len().saturating_add(folders.len()) > 4096 {
            return Err("Stalker folder count exceeds limit");
        }
        children.extend(folders);
        let mut batch = Vec::new();
        for entry in catalog.entries {
            if session.http.cancelled() {
                return Err("Provider request cancelled");
            }
            if !seen.insert(entry.id.clone()) {
                continue;
            }
            if seen.len() > ott::MAX_CATALOG_ENTRIES {
                return Err("Stalker catalogue exceeds limit");
            }
            batch.push(entry);
            if batch.len() == 128 {
                emit(std::mem::take(&mut batch))?;
            }
        }
        if !batch.is_empty() {
            if session.http.cancelled() {
                return Err("Provider request cancelled");
            }
            emit(batch)?;
        }
        number += 1;
    }
    if session.http.cancelled() {
        return Err("Provider request cancelled");
    }
    Ok(children)
}

pub(super) fn resolve(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    http: &mut dyn ProviderHttp,
) -> Result<String, &'static str> {
    let MediaLocator::Stalker {
        cmd,
        stream_kind,
        series,
    } = &entry.media
    else {
        return Err("Invalid Stalker media locator");
    };
    validate_locator(cmd, stream_kind, series)?;
    let mut session = Session::connect(profile, http)?;
    let response = session.call(
        stream_kind,
        "create_link",
        &[
            ("cmd", cmd.clone()),
            (
                "series",
                if series.is_empty() {
                    "0".into()
                } else {
                    series.clone()
                },
            ),
            ("forced_storage", "0".into()),
            ("disable_ad", "0".into()),
            ("download", "0".into()),
        ],
    )?;
    stalker::playback_url(&session.config, &response)
}

fn validate_locator(cmd: &str, kind: &str, series: &str) -> Result<(), &'static str> {
    if !matches!(kind, "itv" | "vod")
        || cmd.is_empty()
        || cmd.len() > 8192
        || cmd.chars().any(char::is_control)
        || series.len() > 20
        || !series.bytes().all(|c| c.is_ascii_digit())
    {
        return Err("Invalid Stalker media locator");
    }
    Ok(())
}

/// Channel-scoped guide retrieval never delays catalogue admission.
pub(super) fn metadata(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    http: &mut dyn ProviderHttp,
) -> Result<Value, &'static str> {
    let MediaLocator::Stalker {
        cmd,
        stream_kind,
        series,
    } = &entry.media
    else {
        return Err("Invalid Stalker media locator");
    };
    validate_locator(cmd, stream_kind, series)?;
    if stream_kind == "vod" {
        return Ok(
            json!({"title":entry.title,"description":entry.description,"poster":entry.poster,"programmes":[]}),
        );
    }
    let rows = short_guide(profile, entry, http)?;
    Ok(json!({"programmes":rows}))
}

fn short_guide(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    http: &mut dyn ProviderHttp,
) -> Result<Vec<ott::epg::Programme>, &'static str> {
    let MediaLocator::Stalker {
        cmd,
        stream_kind,
        series,
    } = &entry.media
    else {
        return Err("Invalid Stalker media locator");
    };
    validate_locator(cmd, stream_kind, series)?;
    if stream_kind != "itv" {
        return Err("Guide requires a live channel");
    }
    let id = entry
        .provider_id
        .as_deref()
        .filter(|id| !id.is_empty() && id.len() <= 1024 && !id.chars().any(char::is_control))
        .ok_or("Stalker channel has no guide identity")?;
    let mut session = Session::connect(profile, http)?;
    let response = session.call(
        "itv",
        "get_short_epg",
        &[("ch_id", id.into()), ("size", "100".into())],
    )?;
    Ok(stalker::short_epg(id, &response)?
        .into_iter()
        .map(|row| ott::epg::Programme {
            channel: id.into(),
            start: row.start,
            end: row.stop,
            title: row.title,
            description: row.description,
        })
        .collect())
}

pub(super) fn guide(
    profile: &ProviderProfile,
    entry: &CatalogEntry,
    request: &ott::guide::Request,
    http: &mut dyn ProviderHttp,
) -> Result<ott::guide::ResultSet, &'static str> {
    request
        .query
        .validate()
        .map_err(|_| "Invalid guide interval")?;
    if request.context.profile_id != profile.id || request.context.entry_id != entry.id {
        return Err("Guide context mismatch");
    }
    let rows = short_guide(profile, entry, http)?;
    let channel = entry
        .provider_id
        .as_deref()
        .ok_or("Missing guide identity")?;
    let programmes = ott::guide::select(&rows, channel, request.query, 0)
        .map_err(|_| "Invalid Stalker guide")?;
    Ok(ott::guide::ResultSet {
        context: request.context.clone(),
        query: request.query,
        coverage: ott::guide::Coverage::ProviderShortGuide,
        programmes,
    })
}

#[cfg(test)]
mod tests {
    use super::super::Response;
    use super::*;
    use plx_data::ott::ProviderKind;
    use std::cell::Cell;
    use std::rc::Rc;

    struct Http {
        replies: VecDeque<(u16, Value)>,
        urls: Vec<String>,
        headers: Vec<Vec<String>>,
        cancelled: Rc<Cell<bool>>,
    }
    impl Http {
        fn new(replies: Vec<(u16, Value)>) -> Self {
            Self {
                replies: replies.into(),
                urls: Vec::new(),
                headers: Vec::new(),
                cancelled: Rc::new(Cell::new(false)),
            }
        }
    }
    impl ProviderHttp for Http {
        fn request(&mut self, request: &Request) -> Result<Response, &'static str> {
            self.urls.push(request.url.clone());
            self.headers.push(request.headers.clone());
            let (status, body) = self.replies.pop_front().ok_or("Unexpected request")?;
            Ok(Response {
                status,
                body: serde_json::to_vec(&body).unwrap(),
            })
        }
        fn cancelled(&self) -> bool {
            self.cancelled.get()
        }
    }
    fn profile(vod: bool) -> ProviderProfile {
        ProviderProfile {id: "fixture".into(), name: "Fixture".into(), kind: ProviderKind::Stalker,
        source: json!({"portal":"https://portal.invalid/c/","mac":"00:1A:79:00:00:01","include_vod":vod}).to_string(), source_alias: String::new(), enabled: true}
    }
    fn auth() -> Vec<(u16, Value)> {
        vec![
            (200, json!({"js":{"token":"fixture-token"}})),
            (200, json!({"js":{"id":1,"status":0}})),
        ]
    }
    fn page(id: u32, total: u32) -> Value {
        json!({"js":{"total_items":total,"max_page_items":1,"data":[{"id":id,"name":format!("Channel {id}"),"cmd":format!("ffmpeg http://localhost/ch/{id}"),"tv_genre_id":"2"}]}})
    }
    #[test]
    fn live_pages_emit_before_completion_and_cancel_at_batch_boundary() {
        let mut replies = auth();
        replies.extend([
            (200, json!({"js":[{"id":2,"title":"News"}]})),
            (200, page(1, 2)),
            (200, page(2, 2)),
        ]);
        let mut http = Http::new(replies);
        let cancel = http.cancelled.clone();
        let mut rows = Vec::new();
        let result = visit(&profile(false), &mut http, &mut |batch| {
            rows.extend(batch);
            cancel.set(true);
            Ok(())
        });
        assert_eq!(result, Err("Provider request cancelled"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].category, "News");
        assert_eq!(http.replies.len(), 1);
        assert!(!http.headers[0]
            .iter()
            .any(|h| h.starts_with("Authorization:")));
        assert!(http.headers[1]
            .iter()
            .any(|h| h == "Authorization: Bearer fixture-token"));
    }
    #[test]
    fn expired_token_is_refreshed_once_then_refused() {
        let mut replies = auth();
        replies.push((403, json!({})));
        replies.extend(auth());
        replies.push((401, json!({})));
        let mut http = Http::new(replies);
        assert_eq!(
            visit(&profile(false), &mut http, &mut |_| Ok(())),
            Err("Stalker authorization failed")
        );
        assert_eq!(
            http.urls
                .iter()
                .filter(|url| url.contains("action=handshake"))
                .count(),
            2
        );
    }
    #[test]
    fn blocked_profile_never_reads_catalogue() {
        let mut http = Http::new(vec![
            (200, json!({"js":{"token":"fixture"}})),
            (200, json!({"js":{"id":1,"status":1}})),
        ]);
        assert!(visit(&profile(false), &mut http, &mut |_| Ok(())).is_err());
        assert_eq!(http.urls.len(), 2);
    }
    #[test]
    fn create_link_encodes_command_and_series_and_normalizes_url() {
        let p = profile(false);
        let c = Config::parse(&p.source).unwrap();
        let entry = stalker::rows(&p,&c,"vod",&BTreeMap::new(),&Folder::default(),&[json!({"id":1,"name":"Movie","cmd":"ffmpeg http://localhost/movie?id=1&x=2","series_number":3})]).unwrap().0.entries.remove(0);
        let mut replies = auth();
        replies.push((
            200,
            json!({"js":{"cmd":"ffmpeg https://media.invalid/movie.m3u8"}}),
        ));
        let mut http = Http::new(replies);
        assert_eq!(
            resolve(&p, &entry, &mut http).unwrap(),
            "https://media.invalid:443/movie.m3u8"
        );
        assert!(http.urls[2].contains("series=3"));
        assert!(http.urls[2].contains("%26x%3D2"));
    }
    #[test]
    fn vod_folders_are_flattened_into_named_categories() {
        let mut replies = auth();
        replies.extend([(200,json!({"js":[]})),(200,json!({"js":{"total_items":0,"data":[]}})),
            (200,json!({"js":[{"id":"7","title":"Series"}]})),
            (200,json!({"js":{"total_items":1,"data":[{"id":"8","name":"Show","is_series":1}]}})),
            (200,json!({"js":{"total_items":1,"data":[{"id":"8","name":"Show","is_series":1}]}})),
            (200,json!({"js":{"total_items":1,"data":[{"id":"9","name":"Episode","cmd":"ffmpeg /movie/9"}]}}))]);
        let mut http = Http::new(replies);
        let mut rows = Vec::new();
        visit(&profile(true), &mut http, &mut |batch| {
            rows.extend(batch);
            Ok(())
        })
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].category, "Series / Show");
        assert!(http.urls.last().unwrap().contains("movie_id=8"));
    }
    #[test]
    fn guide_is_channel_scoped_and_normalized() {
        let p = profile(false);
        let c = Config::parse(&p.source).unwrap();
        let entry = stalker::rows(
            &p,
            &c,
            "itv",
            &BTreeMap::new(),
            &Folder::default(),
            &[json!({"id":2,"name":"Channel","cmd":"ffmpeg /ch/2"})],
        )
        .unwrap()
        .0
        .entries
        .remove(0);
        let mut replies = auth();
        replies.push((200,json!({"js":[{"name":"Programme","start_timestamp":"1700000000","end_timestamp":"1700001800","descr":"Full description"}]})));
        let mut http = Http::new(replies);
        let result = metadata(&p, &entry, &mut http).unwrap();
        assert_eq!(result["programmes"][0]["end"], 1700001800i64);
        assert_eq!(result["programmes"][0]["description"], "Full description");
        assert!(http.urls[2].contains("action=get_short_epg"));
        assert!(http.urls[2].contains("ch_id=2"));
    }
    #[test]
    fn wildcard_keeps_uncategorized_vod_without_duplicating_entries() {
        let mut replies = auth();
        replies.extend([(200,json!({"js":[]})),(200,json!({"js":{"total_items":0,"data":[]}})),
            (200,json!({"js":[{"id":"7","title":"Cinema"}]})),
            (200,json!({"js":{"total_items":1,"data":[{"id":"8","name":"Named","cmd":"ffmpeg /movie/8"}]}})),
            (200,json!({"js":{"total_items":2,"data":[{"id":"8","name":"Named","cmd":"ffmpeg /movie/8"},{"id":"9","name":"Ungrouped","cmd":"ffmpeg /movie/9"}]}}))]);
        let mut http = Http::new(replies);
        let mut rows = Vec::new();
        visit(&profile(true), &mut http, &mut |batch| {
            rows.extend(batch);
            Ok(())
        })
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].category, "Cinema");
        assert_eq!(rows[1].title, "Ungrouped");
    }
    #[test]
    fn invalid_imported_locator_never_reaches_server() {
        let p = profile(false);
        let c = Config::parse(&p.source).unwrap();
        let mut entry = stalker::rows(
            &p,
            &c,
            "itv",
            &BTreeMap::new(),
            &Folder::default(),
            &[json!({"id":2,"name":"Channel","cmd":"ffmpeg /ch/2"})],
        )
        .unwrap()
        .0
        .entries
        .remove(0);
        entry.media = MediaLocator::Stalker {
            cmd: "ffmpeg /ch/2".into(),
            stream_kind: "delete".into(),
            series: "0".into(),
        };
        let mut http = Http::new(Vec::new());
        assert!(resolve(&p, &entry, &mut http).is_err());
        assert!(http.urls.is_empty());
    }
}
