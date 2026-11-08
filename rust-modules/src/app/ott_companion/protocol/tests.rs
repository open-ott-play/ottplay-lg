use super::*;

fn scoped_caps() -> Request {
    Request::Indexer(IndexerRequest::ScopedCapabilities {
        source_id: "home".into(),
        indexer_id: "ChildA".into(),
    })
}

fn scoped_search() -> Request {
    Request::Indexer(IndexerRequest::ScopedSearch {
        source_id: "home".into(),
        indexer_id: "ChildA".into(),
        query: "open film".into(),
        category_ids: vec![2000],
        offset: 0,
        limit: 50,
    })
}

fn children_request() -> Request {
    Request::Indexer(IndexerRequest::Indexers {
        source_id: "home".into(),
    })
}

fn children_caps_reply() -> Value {
    json!({"schema":1,"sourceId":"home","indexerId":"ChildA","searchAvailable":true,"maxLimit":100,
        "categories":[{"id":2000,"name":"Movies","parentId":null}]})
}

fn children_search_reply() -> Value {
    json!({"schema":1,"sourceId":"home","indexerId":"ChildA","nextOffset":null,"total":null,
        "items":[{"resultRef":"0123456789abcdef0123456789abcdef","title":"Open film",
            "size":null,"seeders":null,"publishedAt":null,"categoryIds":[2000],"languages":["en"]}]})
}

#[test]
fn children_wire_paths_preserve_case_and_keep_aggregate_unchanged() {
    assert_eq!(
        children_request().prepare().unwrap().route,
        "torrent/sources/home/indexers"
    );
    assert_eq!(
        scoped_caps().prepare().unwrap().route,
        "torrent/sources/home/caps?indexerId=ChildA"
    );
    let prepared = scoped_search().prepare().unwrap();
    assert_eq!(prepared.route, "torrent/search");
    assert_eq!(
        serde_json::from_slice::<Value>(&prepared.body).unwrap(),
        json!({
            "sourceId":"home","indexerId":"ChildA","query":"open film","categoryIds":[2000],"offset":0,"limit":50
        })
    );
    let aggregate = Request::Indexer(IndexerRequest::Capabilities { id: "home".into() });
    assert_eq!(
        aggregate.prepare().unwrap().route,
        "torrent/sources/home/caps"
    );
    let aggregate = Request::Indexer(IndexerRequest::Search {
        source_id: "home".into(),
        query: "open film".into(),
        category_ids: vec![2000],
        offset: 0,
        limit: 50,
    });
    assert!(
        serde_json::from_slice::<Value>(&aggregate.prepare().unwrap().body)
            .unwrap()
            .get("indexerId")
            .is_none()
    );
}

#[test]
fn children_ids_reject_aggregate_paths_filters_and_encoded_values() {
    for id in [
        "",
        "all",
        "ALL",
        "All",
        "../x",
        "x/y",
        "x?z=1",
        "x#frag",
        "x%2Fy",
        "filter:all",
        " ChildA",
        "ChildA\n",
        "é",
    ] {
        let caps = Request::Indexer(IndexerRequest::ScopedCapabilities {
            source_id: "home".into(),
            indexer_id: id.into(),
        });
        assert!(caps.validate().is_err(), "{id:?}");
        let Request::Indexer(IndexerRequest::ScopedSearch {
            source_id,
            query,
            category_ids,
            offset,
            limit,
            ..
        }) = scoped_search()
        else {
            unreachable!()
        };
        assert!(Request::Indexer(IndexerRequest::ScopedSearch {
            source_id,
            indexer_id: id.into(),
            query,
            category_ids,
            offset,
            limit
        })
        .validate()
        .is_err());
    }
    for id in ["filter", "Filter", "ChildA", "child-a_1", "9tracker"] {
        assert!(Request::Indexer(IndexerRequest::ScopedCapabilities {
            source_id: "home".into(),
            indexer_id: id.into()
        })
        .validate()
        .is_ok());
    }
    assert!(!child_id(&"a".repeat(129)));
    assert!(child_id(&"a".repeat(128)));
}

#[test]
fn children_discovery_is_bounded_and_preserves_nullable_metadata() {
    let Reply::Indexers(children) = read(
        &children_request(),
        json!({"schema":1,"sourceId":"home","supported":true,"indexers":[
            {"id":"ChildA","label":"Home feed","language":"en-US","type":"public"},
            {"id":"childa","label":"Other feed","language":null,"type":null}
        ]}),
    )
    .unwrap() else {
        panic!("wrong reply")
    };
    assert!(children.supported);
    assert_eq!(children.indexers[0].id, "ChildA");
    assert_eq!(children.indexers[0].language.as_deref(), Some("en-US"));
    assert_eq!(children.indexers[0].kind.as_deref(), Some("public"));
    assert_eq!(children.indexers[1].language, None);
    assert_eq!(children.indexers[1].kind, None);
    let entries = (0..257)
        .map(|id| json!({"id":format!("child{id}"),"label":"Feed","language":null,"type":null}))
        .collect::<Vec<_>>();
    assert!(read(
        &children_request(),
        json!({"schema":1,"sourceId":"home","supported":true,"indexers":entries})
    )
    .is_err());
}

#[test]
fn children_discovery_rejects_duplicates_and_malformed_metadata() {
    let entry = json!({"id":"ChildA","label":"Home feed","language":null,"type":null});
    assert!(read(&children_request(),json!({"schema":1,"sourceId":"home","supported":true,"indexers":[entry.clone(),entry.clone()]})).is_err());
    for (field, value) in [
        ("id", json!("ALL")),
        ("label", json!("bad\nlabel")),
        ("language", json!("a".repeat(33))),
        ("type", json!("a".repeat(33))),
    ] {
        let mut invalid = entry.clone();
        invalid[field] = value;
        assert!(read(
            &children_request(),
            json!({"schema":1,"sourceId":"home","supported":true,"indexers":[invalid]})
        )
        .is_err());
    }
    assert!(read(
        &children_request(),
        json!({"schema":1,"sourceId":"other","supported":true,"indexers":[]})
    )
    .is_err());
}

#[test]
fn children_unsupported_source_is_explicit_and_never_fabricates_children() {
    assert!(matches!(
        read(
            &children_request(),
            json!({"schema":1,"sourceId":"home","supported":false,"indexers":[]})
        )
        .unwrap(),
        Reply::Indexers(IndexerChildren {
            supported: false,
            ..
        })
    ));
    assert!(read(&children_request(),json!({"schema":1,"sourceId":"home","supported":false,"indexers":[{"id":"ChildA","label":"Feed","language":null,"type":null}]})).is_err());
}

#[test]
fn children_scoped_caps_and_search_require_the_exact_echo() {
    let Reply::ScopedCategories {
        indexer_id,
        capabilities,
    } = read(&scoped_caps(), children_caps_reply()).unwrap()
    else {
        panic!("wrong reply")
    };
    assert_eq!(indexer_id, "ChildA");
    assert_eq!(capabilities.source_id, "home");
    let Reply::ScopedSearch { indexer_id, page } =
        read(&scoped_search(), children_search_reply()).unwrap()
    else {
        panic!("wrong reply")
    };
    assert_eq!(indexer_id, "ChildA");
    assert_eq!(page.source_id, "home");
    for echo in [
        Value::Null,
        json!("childa"),
        json!("Other"),
        json!("ALL"),
        json!("ChildA%00"),
    ] {
        let mut caps = children_caps_reply();
        caps["indexerId"] = echo.clone();
        let mut search = children_search_reply();
        search["indexerId"] = echo;
        assert!(read(&scoped_caps(), caps).is_err());
        assert!(read(&scoped_search(), search).is_err());
    }
    let mut missing = children_caps_reply();
    missing.as_object_mut().unwrap().remove("indexerId");
    assert!(read(&scoped_caps(), missing).is_err());
}

#[test]
fn children_scoped_payloads_reuse_all_aggregate_validation() {
    let mut caps = children_caps_reply();
    caps["maxLimit"] = json!(0);
    assert!(read(&scoped_caps(), caps).is_err());
    let mut caps = children_caps_reply();
    caps["categories"][0]["id"] = json!(0);
    assert!(read(&scoped_caps(), caps).is_err());
    let mut search = children_search_reply();
    search["items"][0]["resultRef"] = json!("bad");
    assert!(read(&scoped_search(), search).is_err());
    let mut search = children_search_reply();
    search["nextOffset"] = json!(0);
    assert!(read(&scoped_search(), search).is_err());
    let mut search = children_search_reply();
    search["sourceId"] = json!("other");
    assert!(read(&scoped_search(), search).is_err());
}

#[test]
fn children_reply_cannot_be_misread_as_an_aggregate_page() {
    let caps = Request::Indexer(IndexerRequest::Capabilities { id: "home".into() });
    assert!(read(&caps, children_caps_reply()).is_err());
    let search = Request::Indexer(IndexerRequest::Search {
        source_id: "home".into(),
        query: "open film".into(),
        category_ids: vec![],
        offset: 0,
        limit: 50,
    });
    assert!(read(&search, children_search_reply()).is_err());
    let mut page = children_search_reply();
    page.as_object_mut().unwrap().remove("indexerId");
    assert!(matches!(read(&search, page).unwrap(), Reply::Search(_)));
}

#[test]
fn children_capability_is_opt_in_and_requires_local_torznab() {
    let base = json!({"schema":1,"torrent":true,"plugins":false,"localTorznab":true,"fileSelection":true,
        "torrentSources":["magnet","local-reference"],"maxSessions":1,"maxStreams":2,"cacheBytes":67108864,
        "inboundPeerListener":false,"pluginNetwork":false,"pluginNetworkDefault":false});
    let Reply::Capabilities(caps) = read(&Request::Capabilities, base.clone()).unwrap() else {
        panic!("wrong reply")
    };
    assert!(!caps.local_torznab_children);
    let mut enabled = base;
    enabled["localTorznabChildren"] = json!(true);
    let Reply::Capabilities(caps) = read(&Request::Capabilities, enabled.clone()).unwrap() else {
        panic!("wrong reply")
    };
    assert!(caps.local_torznab_children);
    enabled["localTorznab"] = json!(false);
    assert!(read(&Request::Capabilities, enabled).is_err());
}

fn endpoint() -> Endpoint {
    Endpoint::new(8793, "local-capability-test-0123456789abcdef".into()).unwrap()
}
fn read(request: &Request, value: Value) -> Result<Reply, Error> {
    decode(
        &endpoint(),
        request,
        200,
        &serde_json::to_vec(&value).unwrap(),
    )
}
fn ready() -> Value {
    json!({"id":"lease-1", "state":"ready", "files":[{"index":0,"name":"video.mp4","length":4096}],
        "selected":0,"streamPath":"sessions/lease-1/stream","downloaded":128,"total":4096})
}
pub(crate) fn session(value: Value) -> Result<Session, Error> {
    match read(
        &Request::Session {
            id: "lease-1".into(),
        },
        value,
    )? {
        Reply::Session(session) => Ok(session),
        _ => panic!("wrong reply"),
    }
}

#[test]
fn endpoint_only_accepts_owned_loopback_identity() {
    assert!(Endpoint::new(0, "a".repeat(32)).is_err());
    for token in [
        "a".repeat(31),
        "a".repeat(129),
        format!("{}?remote=bad", "a".repeat(32)),
        "ä".repeat(32),
    ] {
        assert!(Endpoint::new(8793, token).is_err());
    }
    assert!(Endpoint::new(8793, "a_-9".repeat(8)).is_ok());
    assert!(!format!("{:?}", endpoint()).contains("local-capability"));
}

#[test]
fn request_paths_cannot_escape_the_capability_root() {
    for id in ["../x", "abc/stream", "x?secret=y", "x#tail", "", "x%2fy"] {
        assert!(Request::Session { id: id.into() }.prepare().is_err());
        assert!(Request::Delete { id: id.into() }.prepare().is_err());
        assert!(
            Request::Indexer(IndexerRequest::Capabilities { id: id.into() })
                .prepare()
                .is_err()
        );
    }
    let prepared = Request::Select {
        id: "lease-1".into(),
        file_index: 3,
    }
    .prepare()
    .unwrap();
    assert_eq!(prepared.method, "POST");
    assert_eq!(prepared.route, "sessions/lease-1/select");
    assert_eq!(
        serde_json::from_slice::<Value>(&prepared.body).unwrap(),
        json!({"fileIndex":3})
    );
}

#[test]
fn direct_and_local_reference_match_existing_create_contract() {
    let request = Request::Create {
        source: CreateSource::Direct("https://media.example/test.torrent".into()),
        file_index: Some(2),
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&request.prepare().unwrap().body).unwrap(),
        json!({"source":"https://media.example/test.torrent","fileIndex":2})
    );
    let request = Request::Create {
        source: CreateSource::LocalReference {
            source_id: "home".into(),
            source_ref: "abcdef0123456789abcdef0123456789".into(),
        },
        file_index: None,
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&request.prepare().unwrap().body).unwrap(),
        json!({"sourceId":"home","sourceRef":"abcdef0123456789abcdef0123456789"})
    );
    assert!(Request::Create {
        source: CreateSource::Direct("file:///etc/passwd".into()),
        file_index: None
    }
    .prepare()
    .is_err());
}

#[test]
fn indexer_credentials_do_not_escape_debug_and_public_addresses_fail() {
    let request = Request::Indexer(IndexerRequest::Configure {
        id: "home".into(),
        label: "Home".into(),
        endpoint: "http://192.168.1.2:9696/api".into(),
        api_key: "secret-value".into(),
    });
    let prepared = request.prepare().unwrap();
    assert_eq!(prepared.route, "torrent/sources");
    assert!(serde_json::from_slice::<Value>(&prepared.body).unwrap()["apiKey"] == "secret-value");
    assert!(!format!("{request:?}").contains("secret-value"));
    for endpoint in [
        "https://8.8.8.8/api",
        "http://[2001:4860:4860::8888]/api",
        "http://user:pass@192.168.1.2/api",
        "http://192.168.1.2/api?apikey=x",
        "file:///tmp/a",
    ] {
        assert!(!valid_indexer_url(endpoint));
    }
}

#[test]
fn ready_stream_is_bound_to_the_exact_endpoint_and_receipt() {
    let mut receipt = session(ready()).unwrap();
    let url = receipt.stream_url(&endpoint()).unwrap();
    assert!(url.starts_with("http://127.0.0.1:8793/companion/"));
    assert!(url.ends_with("/v1/sessions/lease-1/stream"));
    assert!(receipt
        .stream_url(&Endpoint::new(8794, endpoint().token).unwrap())
        .is_err());
    assert!(receipt
        .stream_url(&Endpoint::new(8793, "b".repeat(32)).unwrap())
        .is_err());
    assert!(!format!("{receipt:?}").contains("local-capability"));
    receipt.id = "../../other".into();
    assert!(receipt.stream_url(&endpoint()).is_err());
}

#[test]
fn unready_receipt_cannot_be_promoted_into_a_stream() {
    let mut receipt = session(json!({"id":"lease-1","state":"metadata","files":[]})).unwrap();
    assert!(receipt.stream_url(&endpoint()).is_err());
    receipt.state = SessionState::Ready;
    assert!(receipt.stream_url(&endpoint()).is_err());
}

#[test]
fn cross_origin_or_mismatched_media_receipts_fail() {
    for path in [
        "https://media.example/stream",
        "//media.example/stream",
        "sessions/lease-2/stream",
        "sessions/lease-1/stream?token=x",
        "sessions/../stream",
    ] {
        let mut value = ready();
        value["streamPath"] = json!(path);
        assert!(session(value).is_err());
    }
    let mut value = ready();
    value["id"] = json!("lease-2");
    assert!(session(value).is_err());
    let mut value = ready();
    value["selected"] = json!(1);
    assert!(session(value).is_err());
    let mut value = ready();
    value["files"][0]["length"] = json!(0);
    assert!(session(value).is_err());
    let mut value = ready();
    let duplicate = value["files"][0].clone();
    value["files"].as_array_mut().unwrap().push(duplicate);
    assert!(session(value).is_err());
}

#[test]
fn unknown_errors_never_echo_untrusted_text() {
    let response = br#"{"schema":1,"error":"https://private.invalid/?key=secret"}"#;
    let error = decode(&endpoint(), &Request::Capabilities, 502, response)
        .err()
        .unwrap();
    assert_eq!(error.code(), "companion_request_failed");
    assert!(!format!("{error:?} {error}").contains("secret"));
    assert_eq!(
        decode(
            &endpoint(),
            &Request::Capabilities,
            504,
            br#"{"schema":1,"error":"torznab_timeout"}"#
        )
        .err()
        .unwrap()
        .code(),
        "torznab_timeout"
    );
    assert_eq!(
        decode(&endpoint(), &Request::Capabilities, 302, b"")
            .err()
            .unwrap()
            .code(),
        "companion_redirect_denied"
    );
}

#[test]
fn capabilities_require_the_versioned_schema() {
    let mut value = json!({"schema":1,"torrent":true,"plugins":true,"localTorznab":true,"torrentSources":["magnet","http","https","local-reference"],"fileSelection":true,"maxSessions":2,"cacheBytes":536870912,"inboundPeerListener":false,"pluginNetwork":true,"pluginNetworkDefault":false});
    assert!(read(&Request::Capabilities, value.clone()).is_ok());
    value["schema"] = json!(2);
    assert!(read(&Request::Capabilities, value).is_err());
}

#[test]
fn torznab_response_identity_and_limits_are_enforced() {
    let request = Request::Indexer(IndexerRequest::Search {
        source_id: "home".into(),
        query: "film".into(),
        category_ids: vec![2000],
        offset: 0,
        limit: 1,
    });
    let item = json!({"resultRef":"0123456789abcdef0123456789abcdef","title":"Film","size":2048,"seeders":2,"publishedAt":null,"categoryIds":[2000],"languages":["en"]});
    let value =
        json!({"schema":1,"sourceId":"home","items":[item.clone()],"nextOffset":1,"total":2});
    assert!(read(&request, value.clone()).is_ok());
    let mut changed = value.clone();
    changed["sourceId"] = json!("other");
    assert!(read(&request, changed).is_err());
    let mut changed = value.clone();
    changed["nextOffset"] = json!(0);
    assert!(read(&request, changed).is_err());
    let mut changed = value;
    changed["items"].as_array_mut().unwrap().push(item);
    assert!(read(&request, changed).is_err());
    assert!(read(
        &Request::Indexer(IndexerRequest::Prepare {
            source_id: "home".into(),
            result_ref: "a".repeat(32)
        }),
        json!({"schema":1,"sourceId":"home","sourceRef":"../escape"})
    )
    .is_err());
}

#[test]
fn limits_bound_plugin_input_and_all_response_bodies() {
    let request = Request::ResolvePlugin {
        id: "direct-url".into(),
        input: json!({"value":"a".repeat(REQUEST_BYTES)}),
    };
    assert_eq!(
        request.prepare().err().unwrap().code(),
        "companion_request_limit"
    );
    let response = vec![b' '; RESPONSE_BYTES + 1];
    assert_eq!(
        decode(&endpoint(), &Request::Capabilities, 200, &response)
            .err()
            .unwrap()
            .code(),
        "companion_response_limit"
    );
}

#[test]
fn plugin_streams_remain_untrusted_sources_not_leases() {
    let request = Request::ResolvePlugin {
        id: "direct-url".into(),
        input: json!({}),
    };
    match read(
        &request,
        json!({"streams":[{"title":"Video","url":"https://media.example/video.mp4?token=secret"}]}),
    )
    .unwrap()
    {
        Reply::Resolved(streams) => {
            assert_eq!(streams.len(), 1);
            assert!(!format!("{streams:?}").contains("secret"));
        }
        _ => panic!("wrong reply"),
    }
    assert!(read(
        &request,
        json!({"streams":[{"title":"Video","url":"file:///tmp/private"}]})
    )
    .is_err());
}

#[test]
fn plugin_proposals_cannot_export_the_current_helper_capability() {
    let request = Request::ResolvePlugin {
        id: "direct-url".into(),
        input: json!({}),
    };
    let owned = endpoint();
    let encoded: String = owned.token.bytes().map(|b| format!("%{b:02X}")).collect();
    for (title, url) in [
        ("Video".to_owned(), owned.stream_url("lease-1")),
        (
            "Video".to_owned(),
            format!("https://media.example/?key={}", owned.token),
        ),
        (
            "Video".to_owned(),
            format!("http://127.0.0.1:8793/companion/{encoded}/v1/sessions/lease-1/stream"),
        ),
        (
            owned.token.clone(),
            "https://media.example/video.mp4".to_owned(),
        ),
    ] {
        let result = read(&request, json!({"streams":[{"title":title,"url":url}]}));
        assert_eq!(result.err().unwrap().code(), "plugin_invalid_output");
    }
}
