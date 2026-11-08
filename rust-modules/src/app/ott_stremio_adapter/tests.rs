use super::*;
use crate::app::ott_provider_http::Response;
use std::collections::VecDeque;

const MANIFEST: &[u8] = br#"{"id":"fixture.catalog","name":"Fixture catalog","types":["movie"],"resources":["catalog","stream"],"idPrefixes":["tt"],"catalogs":[{"id":"popular","type":"movie","name":"Popular","extra":[{"name":"skip"},{"name":"search"}]}]}"#;
const PAGE: &[u8] = br#"{"metas":[{"id":"tt123","type":"movie","name":"Fixture movie","description":"Complete first sentence. Complete second sentence.","poster":"https://images.example.invalid/poster.jpg"}]}"#;
const STREAMS: &[u8] = br#"{"streams":[{"name":"Direct","url":"https://media.example.invalid/film.mp4?signature=private-fixture"},{"name":"Torrent","infoHash":"0123456789012345678901234567890123456789","fileIdx":2}]}"#;

fn config() -> Config {
    Config {
        language: "en-US".into(),
        catalog_service_url: None,
        addons: vec![Addon {
            id: "fixture".into(),
            label: "Fixture".into(),
            manifest_url: "https://addon.example.invalid/private-fixture/manifest.json".into(),
            enabled: true,
        }],
    }
}
struct Http {
    replies: VecDeque<Result<Response, &'static str>>,
    paths: Vec<String>,
    cancelled: bool,
}
impl Http {
    fn new(replies: &[&[u8]]) -> Self {
        Self {
            replies: replies
                .iter()
                .map(|bytes| {
                    Ok(Response {
                        status: 200,
                        body: bytes.to_vec(),
                    })
                })
                .collect(),
            paths: Vec::new(),
            cancelled: false,
        }
    }
}
impl ProviderHttp for Http {
    fn request(&mut self, request: &HttpRequest) -> Result<Response, &'static str> {
        assert!(request.headers.is_empty() && request.body.is_none());
        self.paths.push(request.url.clone());
        self.replies.pop_front().expect("unexpected HTTP request")
    }
    fn cancelled(&self) -> bool {
        self.cancelled
    }
}
fn loaded() -> Loaded {
    Loaded {
        addon: config().addons.remove(0),
        manifest: Arc::new(Manifest::parse(MANIFEST).unwrap()),
    }
}
#[cfg(feature = "hostsim")]
fn selection() -> Selection {
    Selection {
        addon: loaded(),
        catalog: "popular".into(),
        query: None,
    }
}

#[test]
fn production_metadata_flow_keeps_private_prefix_and_never_autoplays_stream_proposals() {
    let config = config();
    let mut http = Http::new(&[MANIFEST, PAGE, STREAMS]);
    let Payload::Catalogs { loaded, .. } =
        perform(&mut http, &config, &Operation::Catalogs).unwrap()
    else {
        panic!("catalogs")
    };
    assert_eq!(loaded.len(), 1);
    let selection = Selection {
        addon: loaded[0].clone(),
        catalog: "popular".into(),
        query: Some("two words".into()),
    };
    let Payload::Page { page, .. } = perform(
        &mut http,
        &config,
        &Operation::Page {
            selection,
            offset: 0,
        },
    )
    .unwrap() else {
        panic!("page")
    };
    assert_eq!(page.movies.len(), 1);
    let Payload::Streams {
        candidates,
        failed,
        unsupported,
        ..
    } = perform(
        &mut http,
        &config,
        &Operation::Streams {
            addons: loaded,
            media_ids: vec![page.movies[0].id.clone()],
        },
    )
    .unwrap()
    else {
        panic!("streams")
    };
    assert_eq!((candidates.len(), failed, unsupported), (2, 0, 0));
    assert!(http
        .paths
        .iter()
        .all(|url| url.starts_with("https://addon.example.invalid/private-fixture/")));
    assert!(http.paths[1].contains("search=two%20words"));
    assert!(http.paths[2].ends_with("stream/movie/tt123.json"));
    assert!(
        matches!(&candidates[0].source,SourceProposal::Http{url} if url.ends_with("signature=private-fixture"))
    );
    let SourceProposal::Torrent(proposal) = &candidates[1].source else {
        panic!("torrent")
    };
    let Request::Create {
        source: CreateSource::Direct(magnet),
        file_index,
    } = torrent_request(proposal).unwrap()
    else {
        panic!("create")
    };
    assert_eq!(file_index, Some(2));
    assert!(magnet.starts_with("magnet:?xt=urn:btih:0123456789012345678901234567890123456789"));
    assert!(http.replies.is_empty());
}
#[test]
fn addon_partial_failure_preserves_other_sources_but_rejects_unsupported_features() {
    let mut second = loaded();
    second.addon.id = "second".into();
    let mut http=Http::new(&[STREAMS,br#"{"streams":[{"url":"https://media.example.invalid/no.mp4","behaviorHints":{"proxyHeaders":{"request":{"Authorization":"private"}}}}]}"#]);
    let result = perform(
        &mut http,
        &config(),
        &Operation::Streams {
            addons: vec![loaded(), second],
            media_ids: vec!["tt123".into()],
        },
    )
    .unwrap();
    let Payload::Streams {
        candidates,
        unsupported,
        ..
    } = result
    else {
        panic!("streams")
    };
    assert_eq!(candidates.len(), 2);
    assert_eq!(unsupported, 1);
    let mut http = Http::new(&[]);
    http.cancelled = true;
    assert!(perform(&mut http, &config(), &Operation::Catalogs).is_err());
    assert!(http.paths.is_empty());
}
#[test]
fn torrent_file_choice_is_exact_and_out_of_range_is_not_silently_largest() {
    for (raw, index) in [(2, Some(2)), (4095, Some(4095)), (4096, None)] {
        let bytes=serde_json::to_vec(&json!({"streams":[{"infoHash":"0123456789012345678901234567890123456789","fileIdx":raw}]})).unwrap();
        let parsed = streams::parse(&bytes, "Fixture").unwrap();
        let SourceProposal::Torrent(proposal) = &parsed.candidates[0].source else {
            panic!("torrent")
        };
        match index {
            Some(index) => assert!(
                matches!(torrent_request(proposal),Ok(Request::Create{file_index:Some(i),..}) if i==index)
            ),
            None => assert!(torrent_request(proposal).is_err()),
        }
    }
}
#[test]
fn unowned_media_cannot_impersonate_loopback_capability() {
    for url in [
        "http://127.0.0.1:8080/companion/sessions/fixture",
        "http://localhost/companion/fixture",
        "http://[::1]/companion/fixture",
    ] {
        assert!(media_url(url).is_none(), "{url}");
    }
    assert!(!admitted_source(&ExternalSource::OwnedHttp(
        "http://127.0.0.1/companion/fixture".into()
    )));
    assert!(admitted_source(&ExternalSource::Http(
        "https://media.example.invalid/video.mp4".into()
    )));
}

#[cfg(feature = "hostsim")]
fn runtime() -> Runtime {
    let mut runtime = Runtime::unloaded(Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
    let worker = plx_base::task::OffFrame::for_test();
    runtime.clock_sample = Some(ClockSample::capture(&worker).unwrap());
    runtime.loaded = true;
    runtime
        .saved
        .settings
        .insert(KEY.into(), config().to_value());
    let adapter = runtime.companion_adapter.as_mut().unwrap();
    adapter.stremio.config = config();
    adapter.stremio.observed = Some(config().to_value());
    runtime
}
#[cfg(feature = "hostsim")]
fn deliver(runtime: &mut Runtime, payload: Result<Payload, &'static str>) {
    let configuration = saved_config(runtime).unwrap();
    let expires = runtime.now_ms().saturating_add(UI_TIMEOUT);
    let mut adapter = runtime.companion_adapter.take().unwrap();
    let generation = adapter.generation;
    adapter.stremio.flight = Some(Flight {
        generation,
        expires,
        config: configuration.clone(),
    });
    complete(
        &mut adapter,
        runtime,
        Fetched {
            generation,
            expires,
            config: configuration.clone(),
            result: payload,
        },
    );
    runtime.companion_adapter = Some(adapter);
}
#[cfg(feature = "hostsim")]
#[test]
fn real_view_publishes_full_metadata_and_source_choice_without_starting_a_session() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.page = "stremio-categories";
    deliver(
        &mut runtime,
        Ok(Payload::Catalogs {
            loaded: vec![loaded()],
            failed: 0,
            builtin: Vec::new(),
            builtin_failed: false,
        }),
    );
    assert!(view(&runtime)
        .unwrap()
        .rows
        .iter()
        .any(|row| row.title == "Popular"));
    let page = selection()
        .addon
        .manifest
        .catalog_request("popular", None, 0)
        .unwrap()
        .decode_page(PAGE)
        .unwrap();
    deliver(
        &mut runtime,
        Ok(Payload::Page {
            selection: selection(),
            offset: 0,
            page,
        }),
    );
    let rendered = view(&runtime).unwrap();
    let movie = rendered
        .rows
        .iter()
        .find(|row| row.title.contains("Fixture movie"))
        .unwrap();
    assert!(movie.description.contains("Complete second sentence."));
    assert!(matches!(&movie.poster,Some(OttPoster::Http{url}) if url.ends_with("poster.jpg")));
    let candidates = streams::parse(STREAMS, "Fixture").unwrap().candidates;
    deliver(
        &mut runtime,
        Ok(Payload::Streams {
            metadata: None,
            candidates,
            failed: 0,
            unsupported: 0,
            duplicates: 0,
            rejected: Vec::new(),
        }),
    );
    let adapter = runtime.companion_adapter.as_ref().unwrap();
    assert!(adapter.session.is_none() && !adapter.inspecting && !adapter.playing);
    assert!(view(&runtime)
        .unwrap()
        .rows
        .iter()
        .any(|row| row.title == "Torrent" && !row.enabled));
}
#[cfg(feature = "hostsim")]
#[test]
fn completion_retires_loading_row_keys_before_a_new_movie_can_be_selected() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.page = "stremio-movies";
    let old = key(runtime.companion_adapter.as_ref().unwrap(), "movie", 0);
    let page = selection()
        .addon
        .manifest
        .catalog_request("popular", None, 0)
        .unwrap()
        .decode_page(PAGE)
        .unwrap();
    deliver(
        &mut runtime,
        Ok(Payload::Page {
            selection: selection(),
            offset: 0,
            page,
        }),
    );
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    assert!(action(&mut runtime, &mut app, &OttAction::EditSetting(old))
        .unwrap()
        .is_err());
    let adapter = runtime.companion_adapter.as_ref().unwrap();
    assert!(adapter.stremio.flight.is_none() && adapter.session.is_none());
    assert_eq!(runtime.page, "stremio-movies");
}
#[cfg(feature = "hostsim")]
#[test]
fn cancel_and_changed_configuration_discard_already_queued_completion() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.page = "stremio-streams";
    let expires = runtime.now_ms().saturating_add(UI_TIMEOUT);
    let mut adapter = runtime.companion_adapter.take().unwrap();
    let generation = adapter.generation;
    adapter.stremio.flight = Some(Flight {
        generation,
        expires,
        config: config(),
    });
    let old_cancel = adapter.cancelled.clone();
    adapter.cancel_media(&mut runtime);
    assert!(old_cancel.load(Ordering::Acquire));
    complete(
        &mut adapter,
        &mut runtime,
        Fetched {
            generation,
            expires,
            config: config(),
            result: Err("must not appear"),
        },
    );
    assert_ne!(runtime.status, "must not appear");
    let generation = adapter.generation;
    adapter.stremio.flight = Some(Flight {
        generation,
        expires,
        config: config(),
    });
    runtime
        .saved
        .settings
        .insert(KEY.into(), Config::default().to_value());
    complete(
        &mut adapter,
        &mut runtime,
        Fetched {
            generation,
            expires,
            config: config(),
            result: Err("changed authority must not appear"),
        },
    );
    assert_ne!(runtime.status, "changed authority must not appear");
}
#[cfg(feature = "hostsim")]
#[test]
fn visible_torrent_back_returns_to_stremio_sources_and_preserves_choice_candidates() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.page = "torrent-files";
    let adapter = runtime.companion_adapter.as_mut().unwrap();
    adapter.media_provider = "stremio";
    adapter.stremio.candidates = streams::parse(STREAMS, "Fixture").unwrap().candidates;
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    action(
        &mut runtime,
        &mut app,
        &OttAction::EditSetting("torrent.back".into()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(runtime.page, "stremio-streams");
    assert_eq!(
        runtime
            .companion_adapter
            .as_ref()
            .unwrap()
            .stremio
            .candidates
            .len(),
        2
    );
}
#[cfg(feature = "hostsim")]
#[test]
fn back_from_sources_resets_retry_to_the_visible_catalog_and_fences_pager_action() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.page = "stremio-streams";
    let adapter = runtime.companion_adapter.as_mut().unwrap();
    adapter.stremio.selected = Some(selection());
    adapter.stremio.retry = Some(Operation::Streams {
        addons: vec![loaded()],
        media_ids: vec!["tt123".into()],
    });
    let old = key(adapter, "more", 0);
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    action(&mut runtime, &mut app, &OttAction::Back)
        .unwrap()
        .unwrap();
    assert_eq!(runtime.page, "stremio-movies");
    assert!(matches!(
        &runtime.companion_adapter.as_ref().unwrap().stremio.retry,
        Some(Operation::Page { offset: 0, .. })
    ));
    assert!(action(&mut runtime, &mut app, &OttAction::EditSetting(old))
        .unwrap()
        .is_err());
}
#[cfg(feature = "hostsim")]
#[test]
fn repeated_page_removes_more_and_expiry_remains_cancellable() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.page = "stremio-movies";
    for offset in [0, 1] {
        let page = selection()
            .addon
            .manifest
            .catalog_request("popular", None, offset)
            .unwrap()
            .decode_page(PAGE)
            .unwrap();
        deliver(
            &mut runtime,
            Ok(Payload::Page {
                selection: selection(),
                offset,
                page,
            }),
        );
    }
    assert!(runtime
        .companion_adapter
        .as_ref()
        .unwrap()
        .stremio
        .next
        .is_none());
    assert!(runtime.status.contains("repeated"));
    let expires = runtime.now_ms();
    let mut adapter = runtime.companion_adapter.take().unwrap();
    adapter.stremio.flight = Some(Flight {
        generation: adapter.generation,
        expires,
        config: config(),
    });
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    tick(&mut adapter, &mut runtime, &mut app);
    assert!(adapter.stremio.flight.is_none());
    assert!(runtime.status.contains("timed out"));
}
#[cfg(feature = "hostsim")]
#[test]
fn unrelated_torrent_playback_is_not_cancelled_by_stremio_settings_commit() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.page = "torrent-files";
    let mut adapter = runtime.companion_adapter.take().unwrap();
    adapter.media_provider = "torrent";
    let generation = adapter.generation;
    let cancel = adapter.cancelled.clone();
    runtime
        .saved
        .settings
        .insert(KEY.into(), Config::default().to_value());
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    tick(&mut adapter, &mut runtime, &mut app);
    assert_eq!(adapter.generation, generation);
    assert!(!cancel.load(Ordering::Acquire));
}

#[cfg(feature = "hostsim")]
#[test]
fn settings_commit_cancels_initial_create_before_any_session_reply_exists() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.page = "torrent-files";
    let deadline = runtime.now_ms().saturating_add(UI_TIMEOUT);
    let mut adapter = runtime.companion_adapter.take().unwrap();
    adapter.media_provider = "stremio";
    adapter.media_deadline = deadline;
    let cancelled = Arc::new(AtomicBool::new(false));
    adapter.pending.push(Pending {
        id: Some(1),
        request: Request::Create {
            source: CreateSource::Direct(
                "magnet:?xt=urn:btih:0123456789012345678901234567890123456789".into(),
            ),
            file_index: Some(2),
        },
        target: Target::Session,
        deadline,
        transport_deadline: operation_deadline(runtime.clock_sample, deadline).unwrap(),
        generation: adapter.generation,
        cancelled: cancelled.clone(),
    });
    runtime
        .saved
        .settings
        .insert(KEY.into(), Config::default().to_value());
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    tick(&mut adapter, &mut runtime, &mut app);
    assert!(cancelled.load(Ordering::Acquire));
    assert!(adapter.pending.is_empty());
    assert_eq!(adapter.media_deadline, 0);
    assert!(adapter.session.is_none());
}
#[cfg(feature = "hostsim")]
#[test]
fn service_settings_refresh_returns_without_starting_metadata_in_the_wrong_page() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.page = "torrent-settings";
    let adapter = runtime.companion_adapter.as_mut().unwrap();
    adapter.stremio.service_return = true;
    adapter.stremio.retry = Some(Operation::Streams {
        addons: vec![loaded()],
        media_ids: vec!["tt123".into()],
    });
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    action(&mut runtime, &mut app, &OttAction::Refresh)
        .unwrap()
        .unwrap();
    assert_eq!(runtime.page, "stremio-streams");
    let adapter = runtime.companion_adapter.as_ref().unwrap();
    assert!(adapter.stremio.flight.is_none());
    assert!(!adapter.stremio.service_return);
}
#[test]
fn cancelled_worker_still_owns_its_completion_slot_until_it_finishes() {
    let adapter = Adapter::default();
    adapter.worker_done.store(false, Ordering::Release);
    adapter.cancelled.store(true, Ordering::Release);
    assert!(adapter
        .launch(Work::Inspect(ExternalSource::Http(
            "https://media.example.invalid/video.mp4".into()
        )))
        .is_err());
    assert!(!adapter.worker_done.load(Ordering::Acquire));
}

#[test]
fn explicitly_selected_local_media_and_public_paths_keep_the_common_media_policy() {
    assert!(media_url("http://127.0.0.1:9000/media/movie.mp4").is_some());
    assert!(media_url("https://media.example.invalid/companion/movie.mp4").is_some());
}
#[cfg(feature = "hostsim")]
#[test]
fn addon_poster_cannot_enter_the_companion_control_namespace() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.page = "stremio-movies";
    let bytes=br#"{"metas":[{"id":"tt123","name":"Fixture","poster":"http://127.0.0.1:8793/companion/private/v1/plugins"}]}"#;
    let page = selection()
        .addon
        .manifest
        .catalog_request("popular", None, 0)
        .unwrap()
        .decode_page(bytes)
        .unwrap();
    deliver(
        &mut runtime,
        Ok(Payload::Page {
            selection: selection(),
            offset: 0,
            page,
        }),
    );
    assert!(view(&runtime)
        .unwrap()
        .rows
        .iter()
        .find(|row| row.title.contains("Fixture"))
        .unwrap()
        .poster
        .is_none());
}

#[cfg(feature = "hostsim")]
#[test]
fn expired_direct_inspection_does_not_leave_opening_video_on_the_stremio_page() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.loaded = false;
    runtime.page = "stremio-streams";
    runtime.status = "Opening video…".into();
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    // A fresh App does not clear the process-global native acquisition slot.
    struct RouteReset;
    impl Drop for RouteReset {
        fn drop(&mut self) {
            let mut idle = plx_media::route::PlaybackSession::IDLE;
            plx_media::route::cancel_play(&mut idle);
        }
    }
    let _route_reset = RouteReset;
    plx_media::route::cancel_play(&mut app.player.session);
    assert!(!plx_media::route::play_pending());
    let deadline = runtime.now_ms();
    let generation = runtime.play_generation;
    let adapter = runtime.companion_adapter.as_mut().unwrap();
    adapter.media_provider = "stremio";
    adapter.media_generation = generation;
    // Match the state established by play's direct HTTP branch. Without a
    // direct source this fixture describes a torrent start on the wrong page.
    adapter.direct_source = Some(ExternalSource::Http(
        "https://media.example.invalid/film.mp4".into()));
    adapter.route_ticket = Some(plx_media::route::worker_ticket());
    adapter.media_deadline = deadline.saturating_add(120_000);
    adapter.inspecting = true;
    super::super::tick(&mut runtime, &mut app);
    assert_eq!(runtime.status, "Opening video…");
    assert!(runtime.companion_adapter.as_ref().unwrap().inspecting);

    runtime.companion_adapter.as_mut().unwrap().media_deadline = deadline;
    super::super::tick(&mut runtime, &mut app);
    assert_eq!(runtime.status, "Playback request cancelled");
    assert!(!runtime.companion_adapter.as_ref().unwrap().inspecting);
}

const BUILTIN_GENRES: &[u8] = br#"{"genres":[{"id":28,"name":"Action"}]}"#;
const BUILTIN_PAGE: &[u8] = br#"{"page":1,"total_pages":2,"results":[{"id":42,"title":"Built-in film","overview":"First sentence. Second sentence.","poster_path":"/poster.jpg","release_date":"2024-10-01"}]}"#;
const BUILTIN_DETAILS: &[u8] = br#"{"id":42,"title":"Built-in film","overview":"Full movie description from details.","poster_path":"/poster.jpg","release_date":"2024-10-01","external_ids":{"imdb_id":"tt0000123"}}"#;
fn builtin_config() -> Config {
    let mut config = config();
    config.language = "ru-RU".into();
    config.catalog_service_url = Some("https://catalog.example.invalid/private/tmdb/s/".into());
    config
}
fn builtin_selection() -> BuiltinSelection {
    BuiltinSelection { category:"popular".into(),query:None }
}

#[test]
fn builtin_discovery_loads_details_then_matches_imdb_without_starting_media() {
    let config = builtin_config();
    let mut http = Http::new(&[MANIFEST,BUILTIN_GENRES,BUILTIN_PAGE,BUILTIN_DETAILS,STREAMS]);
    let Payload::Catalogs { loaded,builtin,builtin_failed,.. } = perform(&mut http,&config,&Operation::Catalogs).unwrap()
        else { panic!("catalogs") };
    assert!(!builtin_failed);
    assert_eq!(builtin.iter().map(|row|row.id.as_str()).collect::<Vec<_>>(),["popular","new","genre:28"]);
    let Payload::BuiltinPage { result,.. } = perform(&mut http,&config,&Operation::BuiltinPage {selection:builtin_selection(),page:1}).unwrap()
        else { panic!("builtin page") };
    assert_eq!(result.next_page,Some(2));
    assert_eq!(result.movies[0].metadata.id,"tmdb:movie:42");
    let Payload::Streams { metadata,candidates,.. } = perform(&mut http,&config,&Operation::BuiltinStreams {
        addons:loaded,stable_id:result.movies[0].metadata.id.clone(),
    }).unwrap() else { panic!("streams") };
    assert_eq!(metadata.unwrap().description,"Full movie description from details.");
    assert_eq!(candidates.len(),2);
    assert!(http.paths[1].starts_with("https://catalog.example.invalid/private/tmdb/s/genre/movie/list?language=ru-RU"));
    assert!(http.paths[2].contains("movie/popular?language=ru-RU&page=1"));
    assert!(http.paths[3].contains("movie/42?language=ru-RU&append_to_response=external_ids"));
    assert!(http.paths[4].ends_with("stream/movie/tt0000123.json"));
    assert!(http.replies.is_empty());
}
#[test]
fn absent_or_failed_discovery_service_never_falls_back_to_command_or_addon_authority() {
    let mut http = Http::new(&[]);
    assert!(perform(&mut http,&config(),&Operation::BuiltinPage {selection:builtin_selection(),page:1}).is_err());
    assert!(http.paths.is_empty());
    let mut http = Http::new(&[MANIFEST,br#"{"success":false,"status_message":"private failure"}"#]);
    let Payload::Catalogs {loaded,builtin,builtin_failed,..} = perform(&mut http,&builtin_config(),&Operation::Catalogs).unwrap()
        else {panic!("catalogs")};
    assert_eq!(loaded.len(),1);
    assert!(builtin.is_empty() && builtin_failed);
    assert_eq!(http.paths.len(),2);
    let mut http = Http::new(&[br#"{"id":43,"title":"Different movie"}"#]);
    assert!(perform(&mut http,&builtin_config(),&Operation::BuiltinStreams {
        addons:vec![loaded.into_iter().next().unwrap()],stable_id:"tmdb:movie:42".into(),
    }).is_err());
    assert_eq!(http.paths.len(),1,"wrong details must never request another film's streams");
}
#[test]
fn cancellation_after_details_prevents_the_following_stream_request() {
    struct CancelAfterResponse { inner: Http, done: bool }
    impl ProviderHttp for CancelAfterResponse {
        fn request(&mut self,request:&HttpRequest)->Result<Response,&'static str>{
            let reply=self.inner.request(request);self.done=true;reply
        }
        fn cancelled(&self)->bool {self.done}
    }
    let mut http=CancelAfterResponse{inner:Http::new(&[BUILTIN_DETAILS]),done:false};
    assert!(perform(&mut http,&builtin_config(),&Operation::BuiltinStreams {
        addons:vec![loaded()],stable_id:"tmdb:movie:42".into(),
    }).is_err());
    assert_eq!(http.inner.paths.len(),1);
}
#[cfg(feature="hostsim")]
fn builtin_runtime()->Runtime {
    let mut runtime=runtime();let config=builtin_config();
    runtime.saved.settings.insert(KEY.into(),config.to_value());
    let adapter=runtime.companion_adapter.as_mut().unwrap();
    adapter.stremio.config=config.clone();adapter.stremio.observed=Some(config.to_value());runtime
}
#[cfg(feature="hostsim")]
#[test]
fn builtin_and_addon_categories_share_real_view_and_back_preserves_builtin_pager() {
    let _serial=plx_base::testlock::serial();
    let mut runtime=builtin_runtime();runtime.page="stremio-categories";
    let mut http=Http::new(&[MANIFEST,BUILTIN_GENRES]);
    let payload=perform(&mut http,&builtin_config(),&Operation::Catalogs).unwrap();
    deliver(&mut runtime,Ok(payload));
    let rows=view(&runtime).unwrap().rows;
    assert!(rows.iter().any(|row|row.title=="Action" && row.detail=="Built-in catalog"));
    assert!(rows.iter().any(|row|row.title=="Popular" && row.detail.contains("fixture.catalog")));
    let mut http=Http::new(&[BUILTIN_PAGE]);
    let payload=perform(&mut http,&builtin_config(),&Operation::BuiltinPage {selection:builtin_selection(),page:1}).unwrap();
    deliver(&mut runtime,Ok(payload));
    let old_more=view(&runtime).unwrap().rows.into_iter().find(|row|row.title=="Load more").unwrap().action;
    let adapter=runtime.companion_adapter.as_mut().unwrap();
    assert!(adapter.stremio.selected.is_none());
    assert!(adapter.stremio.selected_builtin.is_some());
    runtime.page="stremio-streams";
    let mut app=crate::app::run::lifecycle_regression_tests::app();
    action(&mut runtime,&mut app,&OttAction::Back).unwrap().unwrap();
    assert_eq!(runtime.page,"stremio-movies");
    assert!(matches!(&runtime.companion_adapter.as_ref().unwrap().stremio.retry,
        Some(Operation::BuiltinPage {page:1,..})));
    assert!(action(&mut runtime,&mut app,&old_more).unwrap().is_err());
}
#[cfg(feature="hostsim")]
#[test]
fn leaving_metadata_for_another_provider_cancels_old_deadline_without_changing_its_status() {
    let _serial=plx_base::testlock::serial();
    let mut runtime=builtin_runtime();runtime.page="stremio-movies";
    let mut adapter=runtime.companion_adapter.take().unwrap();
    let generation=adapter.generation;let old_cancel=adapter.cancelled.clone();
    let expires=runtime.now_ms();
    adapter.stremio.flight=Some(Flight{generation,expires,config:builtin_config()});
    runtime.page="channels";runtime.status="Other provider is playing".into();
    let mut app=crate::app::run::lifecycle_regression_tests::app();
    tick(&mut adapter,&mut runtime,&mut app);
    assert!(old_cancel.load(Ordering::Acquire));
    assert!(adapter.stremio.flight.is_none());
    assert_ne!(adapter.generation,generation);
    complete(&mut adapter,&mut runtime,Fetched{generation,expires,config:builtin_config(),result:Err("late metadata")});
    assert_eq!(runtime.status,"Other provider is playing");
}
#[cfg(feature="hostsim")]
#[test]
fn builtin_service_change_discards_reply_from_the_old_authority() {
    let _serial=plx_base::testlock::serial();
    let mut runtime=builtin_runtime();runtime.page="stremio-movies";
    let mut adapter=runtime.companion_adapter.take().unwrap();
    let generation=adapter.generation;let expires=runtime.now_ms().saturating_add(UI_TIMEOUT);
    adapter.stremio.flight=Some(Flight{generation,expires,config:builtin_config()});
    let mut changed=builtin_config();changed.catalog_service_url=Some("https://new.example.invalid/tmdb/s/".into());
    runtime.saved.settings.insert(KEY.into(),changed.to_value());
    complete(&mut adapter,&mut runtime,Fetched{generation,expires,config:builtin_config(),result:Err("old service response")});
    assert_ne!(runtime.status,"old service response");
    let mut app=crate::app::run::lifecycle_regression_tests::app();
    tick(&mut adapter,&mut runtime,&mut app);
    assert!(adapter.stremio.flight.is_none());
}

#[cfg(feature="hostsim")]
#[test]
fn metadata_only_installation_explains_that_a_stream_source_must_be_added() {
    let _serial=plx_base::testlock::serial();
    let mut runtime=builtin_runtime();runtime.page="stremio-movies";
    let mut config=builtin_config();config.addons.clear();
    runtime.saved.settings.insert(KEY.into(),config.to_value());
    let mut http=Http::new(&[BUILTIN_PAGE]);
    let payload=perform(&mut http,&config,&Operation::BuiltinPage {selection:builtin_selection(),page:1}).unwrap();
    deliver(&mut runtime,Ok(payload));
    let movie=view(&runtime).unwrap().rows.into_iter().find(|row|row.title.contains("Built-in film")).unwrap().action;
    let mut app=crate::app::run::lifecycle_regression_tests::app();
    action(&mut runtime,&mut app,&movie).unwrap().unwrap();
    assert_eq!(runtime.page,"stremio-streams");
    assert_eq!(runtime.status,"Add a stream source in Settings to play this movie");
    let adapter=runtime.companion_adapter.as_ref().unwrap();
    assert!(adapter.stremio.flight.is_none() && adapter.pending.is_empty() && !adapter.inspecting);
    assert!(view(&runtime).unwrap().rows.iter().any(|row|row.title=="Settings"));
}

#[cfg(feature="hostsim")]
#[test]
fn changed_settings_before_tick_reject_metadata_without_touching_the_worker() {
    let _serial=plx_base::testlock::serial();
    let mut runtime=runtime();runtime.page="stremio-movies";
    let mut changed=config();changed.language="fr-FR".into();
    runtime.saved.settings.insert(KEY.into(),changed.to_value());
    let mut adapter=runtime.companion_adapter.take().unwrap();
    let generation=adapter.generation;let cancel=adapter.cancelled.clone();
    let mut app=crate::app::run::lifecycle_regression_tests::app();
    assert!(begin(&mut adapter,&mut runtime,&mut app,Operation::Page {selection:selection(),offset:0}).is_err());
    assert_eq!(adapter.generation,generation);
    assert!(!cancel.load(Ordering::Acquire));
    assert!(adapter.stremio.flight.is_none() && adapter.pending.is_empty());
    assert!(adapter.worker_done.load(Ordering::Acquire));
}
#[cfg(feature="hostsim")]
#[test]
fn reused_addon_id_does_not_authorize_an_old_url_even_after_settings_are_observed() {
    let _serial=plx_base::testlock::serial();
    let mut runtime=runtime();runtime.page="stremio-movies";
    let mut changed=config();changed.addons[0].manifest_url="https://replacement.example.invalid/manifest.json".into();
    runtime.saved.settings.insert(KEY.into(),changed.to_value());
    let mut adapter=runtime.companion_adapter.take().unwrap();
    adapter.stremio.observed=Some(changed.to_value());adapter.stremio.config=changed;
    let mut app=crate::app::run::lifecycle_regression_tests::app();
    assert!(begin(&mut adapter,&mut runtime,&mut app,Operation::Page {selection:selection(),offset:0}).is_err());
    assert!(begin(&mut adapter,&mut runtime,&mut app,Operation::Streams {
        addons:vec![loaded()],media_ids:vec!["tt123".into()],
    }).is_err());
    assert!(adapter.stremio.flight.is_none() && adapter.pending.is_empty());
    assert!(adapter.worker_done.load(Ordering::Acquire));
}
#[cfg(feature="hostsim")]
#[test]
fn cached_source_click_after_settings_changed_cannot_begin_media() {
    let _serial=plx_base::testlock::serial();
    let mut runtime=runtime();runtime.page="stremio-streams";
    let adapter=runtime.companion_adapter.as_mut().unwrap();
    adapter.stremio.candidates=streams::parse(STREAMS,"Fixture").unwrap().candidates;
    let action=OttAction::EditSetting(key(adapter,"source",0));
    let generation=adapter.generation;
    let mut changed=config();changed.addons.clear();runtime.saved.settings.insert(KEY.into(),changed.to_value());
    let mut app=crate::app::run::lifecycle_regression_tests::app();
    assert!(super::action(&mut runtime,&mut app,&action).unwrap().is_err());
    let adapter=runtime.companion_adapter.as_ref().unwrap();
    assert_eq!(adapter.generation,generation);
    assert!(adapter.pending.is_empty() && !adapter.inspecting && adapter.direct_source.is_none());
}

#[cfg(feature="hostsim")]
#[test]
fn source_action_queued_before_show_providers_cannot_start_media_on_another_page() {
    let _serial=plx_base::testlock::serial();
    let mut runtime=runtime();runtime.page="stremio-streams";
    let adapter=runtime.companion_adapter.as_mut().unwrap();
    adapter.stremio.candidates=streams::parse(STREAMS,"Fixture").unwrap().candidates;
    let generation=adapter.generation;
    let stale=OttAction::EditSetting(key(adapter,"source",0));
    let mut app=crate::app::run::lifecycle_regression_tests::app();
    super::super::super::handle_action(&mut runtime,&mut app,OttAction::ShowProviders).unwrap();
    assert_eq!(runtime.page,"providers");
    assert!(super::super::super::handle_action(&mut runtime,&mut app,stale).is_err());
    let adapter=runtime.companion_adapter.as_ref().unwrap();
    assert_eq!(adapter.generation,generation);
    assert!(adapter.pending.is_empty() && !adapter.inspecting && adapter.direct_source.is_none());
}
#[cfg(feature="hostsim")]
#[test]
fn settings_input_queued_before_leaving_screen_cannot_change_the_saved_service() {
    let _serial=plx_base::testlock::serial();
    let mut runtime=runtime();runtime.page="stremio-settings";
    let saved=runtime.saved.settings.clone();
    let stale=OttAction::SubmitSetting {session:0,key:key(runtime.companion_adapter.as_ref().unwrap(),"catalog-service",0),
        value:"https://other.example.invalid/tmdb/s/".into()};
    let mut app=crate::app::run::lifecycle_regression_tests::app();
    super::super::super::handle_action(&mut runtime,&mut app,OttAction::ShowProviders).unwrap();
    assert!(super::super::super::handle_action(&mut runtime,&mut app,stale).is_err());
    assert_eq!(runtime.saved.settings,saved);
    assert!(!runtime.saving);
}

#[cfg(feature = "hostsim")]
fn movie_after_playback_choice(builtin: bool) -> (Runtime, App, OttAction) {
    let mut runtime = if builtin { builtin_runtime() } else { runtime() };
    runtime.page = "stremio-streams";
    let adapter = runtime.companion_adapter.as_mut().unwrap();
    adapter.stremio.loaded = vec![loaded()];
    adapter.stremio.candidates = streams::parse(STREAMS, "Fixture").unwrap().candidates;
    adapter.stremio.choice = Some(0);
    adapter.stremio.retry = Some(if builtin {
        Operation::BuiltinStreams { addons: vec![loaded()], stable_id: "tmdb:movie:42".into() }
    } else {
        Operation::Streams { addons: vec![loaded()], media_ids: vec!["tt123".into()] }
    });
    adapter.stremio.movies = vec![Movie {
        id: if builtin { "tmdb:movie:43" } else { "tt456" }.into(),
        title: "Second film".into(),
        description: String::new(),
        artwork: None,
        release_info: None,
    }];
    if builtin {
        adapter.stremio.selected_builtin = Some(builtin_selection());
    } else {
        adapter.stremio.selected = Some(selection());
    }
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    action(&mut runtime, &mut app, &OttAction::Back).unwrap().unwrap();
    let movie = view(&runtime).unwrap().rows.into_iter()
        .find(|row| row.title.contains("Second film")).unwrap().action;
    // Exercise the real movie/lookup transition without starting any worker.
    // A canceled worker can still own the slot when the next film is selected.
    runtime.companion_adapter.as_ref().unwrap().worker_done.store(false, Ordering::Release);
    (runtime, app, movie)
}

#[cfg(feature = "hostsim")]
fn change_movie_after_playback_choice(builtin: bool) -> (Runtime, App) {
    let (mut runtime, mut app, movie) = movie_after_playback_choice(builtin);
    assert_eq!(action(&mut runtime, &mut app, &movie).unwrap(),
        Err("Previous provider request is finishing. Retry shortly."));
    assert_eq!(runtime.page, "stremio-streams");
    (runtime, app)
}

#[cfg(feature = "hostsim")]
fn assert_second_movie_metadata_retry(runtime: &Runtime, builtin: bool) -> OttAction {
    let adapter = runtime.companion_adapter.as_ref().unwrap();
    assert!(adapter.stremio.choice.is_none(), "the first film's playback choice is retired");
    assert!(adapter.stremio.candidates.is_empty());
    match adapter.stremio.retry.as_ref().unwrap() {
        Operation::BuiltinStreams { stable_id, .. } if builtin => {
            assert_eq!(stable_id, "tmdb:movie:43");
        }
        Operation::Streams { media_ids, .. } if !builtin => {
            assert_eq!(media_ids, &["tt456"]);
        }
        _ => panic!("retry must retain the second film's metadata lookup"),
    }
    let rows = view(runtime).unwrap().rows;
    assert!(!rows.iter().any(|row| row.title == "Retry playback"));
    rows.into_iter().find(|row| row.title == "Retry")
        .expect("the second film must offer a metadata retry").action
}

#[cfg(feature = "hostsim")]
#[test]
fn new_movie_lookup_launch_failure_retries_metadata_instead_of_old_playback() {
    let _serial = plx_base::testlock::serial();
    for builtin in [false, true] {
        let (mut runtime, mut app) = change_movie_after_playback_choice(builtin);
        let retry = assert_second_movie_metadata_retry(&runtime, builtin);
        assert_eq!(action(&mut runtime, &mut app, &retry).unwrap(),
            Err("Previous provider request is finishing. Retry shortly."));
        assert_second_movie_metadata_retry(&runtime, builtin);
    }
}

#[cfg(feature = "hostsim")]
#[test]
fn new_movie_lookup_error_and_timeout_keep_the_new_metadata_retry() {
    let _serial = plx_base::testlock::serial();
    for builtin in [false, true] {
        for timeout in [false, true] {
            let (mut runtime, mut app) = change_movie_after_playback_choice(builtin);
            // Inject the addressed completion/deadline of that lookup, as the
            // existing metadata tests do, without using HTTP or a live worker.
            if timeout {
                let config = saved_config(&runtime).unwrap();
                let expires = runtime.now_ms();
                let mut adapter = runtime.companion_adapter.take().unwrap();
                adapter.stremio.flight = Some(Flight {
                    generation: adapter.generation,
                    expires,
                    config,
                });
                tick(&mut adapter, &mut runtime, &mut app);
                runtime.companion_adapter = Some(adapter);
                assert_eq!(runtime.status, "Add-on request timed out. Retry or go back.");
            } else {
                deliver(&mut runtime, Err("Second film lookup failed"));
                assert_eq!(runtime.status, "Second film lookup failed");
            }
            let retry = assert_second_movie_metadata_retry(&runtime, builtin);
            assert_eq!(action(&mut runtime, &mut app, &retry).unwrap(),
                Err("Previous provider request is finishing. Retry shortly."));
            assert_second_movie_metadata_retry(&runtime, builtin);
        }
    }
}

#[cfg(feature = "hostsim")]
#[test]
fn same_movie_playback_retry_preserves_the_selected_source() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.page = "torrent-files";
    let adapter = runtime.companion_adapter.as_mut().unwrap();
    adapter.media_provider = "stremio";
    adapter.stremio.candidates = streams::parse(STREAMS, "Fixture").unwrap().candidates;
    adapter.stremio.choice = Some(0);
    adapter.stremio.retry = Some(Operation::Streams {
        addons: vec![loaded()],
        media_ids: vec!["tt123".into()],
    });
    adapter.worker_done.store(false, Ordering::Release);
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    assert_eq!(action(&mut runtime, &mut app, &OttAction::Refresh).unwrap(),
        Err("Previous provider request is finishing. Retry shortly."));
    action(&mut runtime, &mut app, &OttAction::Back).unwrap().unwrap();
    assert_eq!(runtime.page, "stremio-streams");
    let adapter = runtime.companion_adapter.as_ref().unwrap();
    assert_eq!(adapter.stremio.choice, Some(0));
    assert_eq!(adapter.stremio.candidates.len(), 2);
    let retry = view(&runtime).unwrap().rows.into_iter()
        .find(|row| row.title == "Retry playback").unwrap().action;
    assert_eq!(action(&mut runtime, &mut app, &retry).unwrap(),
        Err("Previous provider request is finishing. Retry shortly."));
    assert_eq!(runtime.companion_adapter.as_ref().unwrap().stremio.choice, Some(0));
}

#[cfg(feature = "hostsim")]
#[test]
fn new_movie_save_refusal_retries_its_metadata_after_saving_finishes() {
    let _serial = plx_base::testlock::serial();
    for builtin in [false, true] {
        let (mut runtime, mut app, movie) = movie_after_playback_choice(builtin);
        runtime.saving = true;
        assert_eq!(action(&mut runtime, &mut app, &movie).unwrap(),
            Err("Wait for settings to finish saving"));
        let retry = assert_second_movie_metadata_retry(&runtime, builtin);
        let saved = json!(runtime.saved);
        runtime.saving = false;
        // Completion of an unrelated save neither looks up nor plays a film.
        let adapter = runtime.companion_adapter.as_ref().unwrap();
        assert!(adapter.stremio.flight.is_none());
        assert!(adapter.pending.is_empty() && adapter.session.is_none() && !adapter.inspecting);

        assert_eq!(action(&mut runtime, &mut app, &retry).unwrap(),
            Err("Previous provider request is finishing. Retry shortly."));
        assert_second_movie_metadata_retry(&runtime, builtin);
        // Exercise the captured retry through the real metadata decoder. The
        // worker slot remains held, so no network, companion or decoder runs.
        let operation = runtime.companion_adapter.as_ref().unwrap().stremio.retry.clone().unwrap();
        let details = br#"{"id":43,"title":"Second film","external_ids":{"imdb_id":"tt0000456"}}"#;
        let mut http = if builtin { Http::new(&[details, STREAMS]) } else { Http::new(&[STREAMS]) };
        let payload = perform(&mut http, &saved_config(&runtime).unwrap(), &operation).unwrap();
        assert!(http.paths.last().unwrap().ends_with(if builtin {
            "stream/movie/tt0000456.json"
        } else { "stream/movie/tt456.json" }));
        deliver(&mut runtime, Ok(payload));
        let adapter = runtime.companion_adapter.as_ref().unwrap();
        assert_eq!(adapter.stremio.candidates.len(), 2);
        assert!(adapter.stremio.choice.is_none());
        assert!(adapter.pending.is_empty() && adapter.session.is_none() && !adapter.inspecting);
        assert_eq!(json!(runtime.saved), saved);
    }
}

#[cfg(feature = "hostsim")]
#[test]
fn new_movie_configuration_refusal_cannot_retry_the_previous_playback_choice() {
    let _serial = plx_base::testlock::serial();
    for builtin in [false, true] {
        for changed_catalog in [false, true] {
            let (mut runtime, mut app, movie) = movie_after_playback_choice(builtin);
            let mut config = saved_config(&runtime).unwrap();
            config.addons[0].enabled = false;
            runtime.saved.settings.insert(KEY.into(), config.to_value());
            let expected = if changed_catalog {
                runtime.companion_adapter.as_mut().unwrap().stremio.observed = Some(config.to_value());
                "Catalog source changed. Reload categories."
            } else { "Catalog settings changed. Reload categories." };
            assert_eq!(action(&mut runtime, &mut app, &movie).unwrap(), Err(expected));
            let retry = assert_second_movie_metadata_retry(&runtime, builtin);
            assert_eq!(action(&mut runtime, &mut app, &retry).unwrap(), Err(expected));
            let adapter = runtime.companion_adapter.as_ref().unwrap();
            assert!(adapter.pending.is_empty() && adapter.session.is_none() && !adapter.inspecting);
            assert!(adapter.stremio.flight.is_none());
        }
    }
}

#[test]
fn malformed_stream_entries_do_not_count_as_unsupported_features() {
    let hash = "0123456789012345678901234567890123456789";
    let trackers = (0..24)
        .map(|i| format!("tracker:https://tracker{i}.example.invalid/{}", "a".repeat(768)))
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&json!({"streams":[
        null,
        {"url":17},
        {"infoHash":hash,"fileIdx":-1},
        {"infoHash":hash,"sources":[false]},
        {"infoHash":hash,"sources":trackers},
        {"zipUrls":["https://media.example.invalid/archive.zip"]}
    ]})).unwrap();
    let mut http = Http::new(&[&bytes]);
    let Payload::Streams { candidates, failed, unsupported, rejected, .. } = perform(
        &mut http, &config(),
        &Operation::Streams { addons:vec![loaded()], media_ids:vec!["tt123".into()] },
    ).unwrap() else { panic!("streams") };
    assert!(candidates.is_empty());
    assert_eq!(failed, 0);
    assert_eq!(unsupported, 1, "only the archive requires an unsupported feature");
    assert_eq!(rejected, vec![
        (0, streams::Rejection::InvalidShape),
        (1, streams::Rejection::InvalidSource),
        (2, streams::Rejection::InvalidFileIndex),
        (3, streams::Rejection::InvalidTrackers),
        (4, streams::Rejection::SourceLimit),
        (5, streams::Rejection::Unsupported(streams::Feature::Archive)),
    ]);
}

#[cfg(feature = "hostsim")]
#[test]
fn stream_status_distinguishes_invalid_sources_features_empty_and_failed_replies() {
    let _serial = plx_base::testlock::serial();
    let response = |status, body: Vec<u8>| Ok(Response { status, body });
    for (reply, expected, count) in [
        (response(200, br#"{"streams":[null]}"#.to_vec()),
            "Stream add-ons returned invalid sources. Retry or check Settings. · Source 1: invalid source", 0),
        (response(200, br#"{"streams":[{"url":"https://media.example.invalid/video.mp4"},null]}"#.to_vec()),
            "0 unavailable add-on(s), 0 unsupported source(s), 1 invalid source(s) · Source 2: invalid source", 1),
        (response(200, br#"{"streams":[{"zipUrls":["https://media.example.invalid/archive.zip"]}]}"#.to_vec()),
            "The returned sources require unsupported playback features · Source 1: archive source not supported", 0),
        (response(200, br#"{"streams":[]}"#.to_vec()), "No sources available for this video", 0),
        (Err("Network request failed"), "Stream add-ons unavailable. Retry or check Settings.", 0),
        (response(503, br#"{"error":"Unavailable"}"#.to_vec()),
            "Stream add-ons unavailable. Retry or check Settings.", 0),
        (response(200, b"not JSON".to_vec()), "Stream add-ons unavailable. Retry or check Settings.", 0),
        (response(200, vec![b' '; 524_289]), "Stream add-ons unavailable. Retry or check Settings.", 0),
        (response(200, serde_json::to_vec(&json!({"streams":vec![Value::Null; 257]})).unwrap()),
            "Stream add-ons unavailable. Retry or check Settings.", 0),
    ] {
        let mut http = Http::new(&[]);
        http.replies.push_back(reply);
        let payload = perform(&mut http, &config(), &Operation::Streams {
            addons:vec![loaded()], media_ids:vec!["tt123".into()],
        }).unwrap();
        let mut runtime = runtime();
        runtime.page = "stremio-streams";
        deliver(&mut runtime, Ok(payload));
        assert_eq!(view(&runtime).unwrap().status, expected);
        let adapter = runtime.companion_adapter.as_ref().unwrap();
        assert_eq!(adapter.stremio.candidates.len(), count);
        assert!(adapter.session.is_none() && !adapter.inspecting && adapter.pending.is_empty());
    }
}

#[cfg(feature = "hostsim")]
#[test]
fn stream_only_source_explains_missing_catalog_without_losing_stream_capability() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.page = "stremio-categories";
    let bytes = br#"{"id":"fixture.stream","name":"Streams","types":["movie"],"resources":["stream"],"catalogs":[]}"#;
    let mut http = Http::new(&[bytes]);
    let payload = perform(&mut http, &config(), &Operation::Catalogs).unwrap();
    deliver(&mut runtime, Ok(payload));
    let screen = view(&runtime).unwrap();
    assert_eq!(screen.page_id, "stremio-categories");
    assert!(screen.status.contains("catalog") && screen.status.contains("Settings"));
    assert!(screen.rows.iter().any(|row| row.id == "stremio.settings" && row.enabled));
    let adapter = runtime.companion_adapter.as_ref().unwrap();
    assert!(adapter.stremio.loaded[0].manifest.has_movie_streams());
    assert!(adapter.pending.is_empty() && adapter.session.is_none() && !adapter.inspecting);
}

#[cfg(feature = "hostsim")]
#[test]
fn a_search_only_catalog_is_usable_and_does_not_get_missing_catalog_guidance() {
    let _serial = plx_base::testlock::serial();
    let mut runtime = runtime();
    runtime.page = "stremio-categories";
    let bytes = br#"{"id":"fixture.search","name":"Search","types":["movie"],"resources":["catalog"],"catalogs":[{"id":"search","type":"movie","extra":[{"name":"search","isRequired":true}]}]}"#;
    let mut http = Http::new(&[bytes]);
    let payload = perform(&mut http, &config(), &Operation::Catalogs).unwrap();
    deliver(&mut runtime, Ok(payload));
    let screen = view(&runtime).unwrap();
    assert!(screen.status.is_empty());
    assert!(screen.rows.iter().any(|row| row.id.starts_with("stremio.catalog:") && row.enabled));
}
