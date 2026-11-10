//! Loopback acceptance tests for the real provider transport. No provider credentials or services.
use super::*;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{mpsc, Arc};

fn ready() {
    assert!(
        plx_net::net::global_init() && plx_net::net::available(),
        "provider transport tests require host libcurl"
    );
}

fn with_server<T>(
    serve: impl FnOnce(TcpStream) + Send + 'static,
    check: impl FnOnce(String) -> T,
) -> T {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!(
        "http://127.0.0.1:{}/list.m3u",
        listener.local_addr().unwrap().port()
    );
    let worker = std::thread::spawn(move || {
        let capability = OffFrame::for_test();
        let budget = Budget::start(&capability, Duration::from_secs(5));
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        budget.wait(
                            &capability,
                            Duration::from_millis(2),
                            Duration::from_millis(2),
                            &|| false
                        ),
                        "loopback request did not arrive"
                    );
                }
                Err(error) => panic!("loopback accept failed: {error}"),
            }
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        let mut chunk = [0u8; 1024];
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let n = socket.read(&mut chunk).unwrap();
            assert!(n > 0 && request.len() < 16_384, "invalid fixture request");
            request.extend_from_slice(&chunk[..n]);
        }
        serve(socket);
    });
    let result = check(url);
    worker.join().unwrap();
    result
}

fn response(status: u16, body: Vec<u8>, check: impl FnOnce(String)) {
    with_server(
        move |mut socket| {
            write!(
                socket,
                "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            // A refused or oversized response may be closed before all bytes are written.
            let _ = socket.write_all(&body);
        },
        check,
    );
}

fn profile(source: String) -> ProviderProfile {
    ProviderProfile {
        id: "fixture".into(),
        name: "Fixture".into(),
        kind: ProviderKind::M3u,
        source,
        source_alias: "Fixture".into(),
        enabled: true,
    }
}

fn row(id: usize) -> String {
    format!("#EXTINF:-1 tvg-id=\"{id}\",Channel {id}\nhttp://127.0.0.1/video/{id}.ts\n")
}

#[test]
fn native_m3u_publishes_first_row_before_response_finishes() {
    let _serial = plx_base::testlock::serial();
    ready();
    let (published, received) = mpsc::channel();
    let tail_written = Arc::new(AtomicBool::new(false));
    let server_tail = tail_written.clone();
    with_server(
        move |mut socket| {
            let first = format!("#EXTM3U\n{}", row(1));
            let tail = row(2);
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{first}",
                first.len() + tail.len()
            )
            .unwrap();
            socket.flush().unwrap();
            received
                .recv_timeout(Duration::from_secs(3))
                .expect("first row was buffered until HTTP completion");
            server_tail.store(true, Ordering::Release);
            socket.write_all(tail.as_bytes()).unwrap();
        },
        |url| {
            let cancel = AtomicBool::new(false);
            let mut entries = Vec::new();
            let mut notify = Some(published);
            visit(
                &OffFrame::for_test(),
                &profile(url),
                &cancel,
                &mut |batch| {
                    if let Some(notify) = notify.take() {
                        assert_eq!(batch.len(), 1);
                        assert!(!tail_written.load(Ordering::Acquire));
                        notify.send(()).unwrap();
                    }
                    entries.extend(batch);
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(entries.len(), 2);
        },
    );
}

#[test]
fn native_m3u_cancellation_stops_remaining_rows_in_the_same_chunk() {
    let _serial = plx_base::testlock::serial();
    ready();
    let body = format!("#EXTM3U\n{}", (1..=130).map(row).collect::<String>()).into_bytes();
    response(200, body, |url| {
        let cancel = AtomicBool::new(false);
        let mut entries = Vec::new();
        let result = visit(
            &OffFrame::for_test(),
            &profile(url),
            &cancel,
            &mut |batch| {
                entries.extend(batch);
                cancel.store(true, Ordering::Release);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(
            entries.len(),
            1,
            "cancelled rows escaped from the current network chunk"
        );
    });
}

#[test]
fn native_cancelled_request_does_not_open_a_socket() {
    let _serial = plx_base::testlock::serial();
    ready();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let cancel = AtomicBool::new(true);
    let mut http = NativeHttp {
        worker: &OffFrame::for_test(),
        cancel: &cancel,
    };
    let request = Request {
        url: format!("http://{}/", listener.local_addr().unwrap()),
        headers: Vec::new(),
        body: None,
    };
    assert!(http.request(&request).is_err());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn native_http_errors_never_publish_a_successful_looking_playlist() {
    let _serial = plx_base::testlock::serial();
    ready();
    for status in [401, 403, 500] {
        response(status, format!("#EXTM3U\n{}", row(1)).into_bytes(), |url| {
            let cancel = AtomicBool::new(false);
            let mut count = 0;
            assert!(visit(
                &OffFrame::for_test(),
                &profile(url),
                &cancel,
                &mut |batch| {
                    count += batch.len();
                    Ok(())
                }
            )
            .is_err());
            assert_eq!(count, 0, "HTTP {status} body must not become a catalogue");
        });
    }
}

#[test]
fn native_xtream_refused_auth_does_not_start_catalogue_requests() {
    let _serial = plx_base::testlock::serial();
    ready();
    response(
        200,
        br#"{"user_info":{"auth":0,"status":"Disabled"}}"#.to_vec(),
        |url| {
            let mut p = profile(
                serde_json::json!({"server":url,"username":"fixture","password":"fixture-only"})
                    .to_string(),
            );
            p.kind = ProviderKind::Xtream;
            let cancel = AtomicBool::new(false);
            let mut count = 0;
            let error = visit(&OffFrame::for_test(), &p, &cancel, &mut |batch| {
                count += batch.len();
                Ok(())
            })
            .unwrap_err();
            assert!(error.contains("account") || error.contains("authentication"));
            assert_eq!(count, 0);
            assert!(!error.contains("fixture-only"));
        },
    );
}

#[test]
fn native_redirect_does_not_forward_credentials_or_dial_the_target() {
    let _serial = plx_base::testlock::serial();
    ready();
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let destination = target.local_addr().unwrap();
    with_server(
        move |mut socket| {
            write!(socket, "HTTP/1.1 302 Found\r\nLocation: http://{destination}/other\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        },
        |url| {
            let cancel = AtomicBool::new(false);
            let mut http = NativeHttp {
                worker: &OffFrame::for_test(),
                cancel: &cancel,
            };
            if let Ok(response) = http.request(&Request {
                url,
                headers: vec!["Authorization: Bearer fixture-only".into()],
                body: None,
            }) {
                assert_eq!(
                    response.status, 302,
                    "redirect must remain visible to the caller"
                );
            }
        },
    );
    assert_eq!(
        target.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn native_transport_and_m3u_enforce_byte_bounds() {
    let _serial = plx_base::testlock::serial();
    ready();
    response(200, vec![b'x'; ott::MAX_CATALOG_LINE_BYTES + 1], |url| {
        let cancel = AtomicBool::new(false);
        assert!(visit(
            &OffFrame::for_test(),
            &profile(url),
            &cancel,
            &mut |_| panic!("oversized line published")
        )
        .is_err());
    });
    response(200, vec![b' '; ott::MAX_CATALOG_BYTES + 1], |url| {
        let cancel = AtomicBool::new(false);
        let mut http = NativeHttp {
            worker: &OffFrame::for_test(),
            cancel: &cancel,
        };
        assert!(http
            .request(&Request {
                url,
                headers: Vec::new(),
                body: None
            })
            .is_err());
    });
}

#[test]
fn m3u_identity_survives_token_rotation_and_source_reordering() {
    let p = profile("http://127.0.0.1/list.m3u".into());
    let a = ott::parse_m3u("#EXTM3U\n#EXTINF:-1 tvg-id=\"news\" group-title=\"World\",News\nhttp://127.0.0.1/news.ts?token=first\n#EXTINF:-1,Film\nhttp://127.0.0.1/film.mp4\n", &p).unwrap();
    let b = ott::parse_m3u("#EXTM3U\n#EXTINF:-1,Film\nhttp://127.0.0.1/film.mp4\n#EXTINF:-1 tvg-id=\"news\" group-title=\"World\",News\nhttp://127.0.0.1/news.ts?token=second\n", &p).unwrap();
    assert_eq!(a.entries[0].id, b.entries[1].id);
    assert_eq!(a.entries[1].id, b.entries[0].id);
}

#[test]
fn m3u_distinct_streams_with_duplicate_labels_remain_selectable() {
    let p = profile("http://127.0.0.1/list.m3u".into());
    let input = "#EXTM3U\n#EXTINF:-1 tvg-id=\"news\" group-title=\"World\",News\nhttp://127.0.0.1/first.ts\n#EXTINF:-1 tvg-id=\"news\" group-title=\"World\",News\nhttp://127.0.0.1/second.ts\n#EXTINF:-1 tvg-id=\"news\" group-title=\"World\",News\nhttp://127.0.0.1/first.ts\n";
    let entries = ott::parse_m3u(input, &p).unwrap().entries;
    assert_eq!(
        entries.len(),
        2,
        "deduplication must remove exact repeats, not distinct streams"
    );
    assert_ne!(entries[0].id, entries[1].id);
}

#[test]
fn native_http_keeps_unauthorized_status_for_session_refresh() {
    let _serial = plx_base::testlock::serial();
    ready();
    for status in [401, 403] {
        response(
            status,
            br#"{"js":{"not_valid_token":true}}"#.to_vec(),
            |url| {
                let cancel = AtomicBool::new(false);
                let reply = NativeHttp {
                    worker: &OffFrame::for_test(),
                    cancel: &cancel,
                }
                .request(&Request {
                    url,
                    headers: vec!["Authorization: Bearer fixture-only".into()],
                    body: None,
                })
                .expect(
                    "the session must receive HTTP status instead of a generic transport error",
                );
                assert_eq!(reply.status, status);
            },
        );
    }
}

struct GuideFixture {
    playlist: String,
    chunks: VecDeque<Vec<u8>>,
    replies: VecDeque<(String, Response)>,
    streams: usize,
    chunks_read: usize,
    calls: Vec<String>,
    stopped: bool,
    cancelled: bool,
    cancel_after_reply: bool,
}

impl GuideFixture {
    fn new(playlist: &str, header: &str) -> Self {
        Self {
            playlist: playlist.into(),
            chunks: vec![header.as_bytes().to_vec()].into(),
            replies: VecDeque::new(),
            streams: 0,
            chunks_read: 0,
            calls: Vec::new(),
            stopped: false,
            cancelled: false,
            cancel_after_reply: false,
        }
    }
    fn reply(&mut self, path: &str, status: u16, body: &str) {
        self.replies.push_back((
            ott::resolve_http_url(&self.playlist, path).unwrap(),
            Response {
                status,
                body: body.as_bytes().to_vec(),
            },
        ));
    }
}

impl ProviderHttp for GuideFixture {
    fn request(&mut self, request: &Request) -> Result<Response, &'static str> {
        assert!(
            !self.cancelled,
            "cancelled metadata must not request another guide"
        );
        assert!(
            request.headers.is_empty(),
            "playlist credentials must not leak to a separate guide origin"
        );
        assert!(request.body.is_none());
        self.calls.push(request.url.clone());
        let (url, reply) = self.replies.pop_front().expect("unexpected guide request");
        assert_eq!(request.url, url);
        self.cancelled = self.cancel_after_reply;
        Ok(reply)
    }
    fn cancelled(&self) -> bool {
        self.cancelled
    }
    fn stream(
        &mut self,
        request: &Request,
        consume: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<(), &'static str> {
        assert!(
            !self.cancelled,
            "cancelled metadata must not read the playlist header"
        );
        assert_eq!(request.url, self.playlist);
        self.streams += 1;
        while let Some(chunk) = self.chunks.pop_front() {
            self.chunks_read += 1;
            if !consume(&chunk) {
                self.stopped = true;
                return Err("Provider request cancelled");
            }
        }
        Ok(())
    }
}

fn guide_entry(profile: &ProviderProfile) -> CatalogEntry {
    ott::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 tvg-id=\"news\",News\nhttp://127.0.0.1/news.ts\n",
        profile,
    )
    .unwrap()
    .entries
    .remove(0)
}

fn programme(channel: &str, title: &str, start: u32, end: u32) -> String {
    let time = |seconds: u32| {
        format!(
            "19700101{:02}{:02}{:02} +0000",
            seconds / 3600,
            (seconds / 60) % 60,
            seconds % 60
        )
    };
    format!("<programme channel='{channel}' start='{}' stop='{}'><title>{title}</title><desc>Full description</desc></programme>", time(start), time(end))
}

#[test]
fn m3u_guide_stops_after_header_and_selects_current_and_next_programmes() {
    let p = profile("http://127.0.0.1/source.m3u".into());
    let entry = guide_entry(&p);
    let mut fixture = GuideFixture::new(&p.source, "");
    fixture.chunks = vec![
        b"\r\n".to_vec(),
        "\u{feff}#EXT".as_bytes().to_vec(),
        b"M3U x-tvg-url='/guide.xml'\r\n".to_vec(),
        b"must never scan the large channel catalogue".to_vec(),
    ]
    .into();
    let mut xml = String::from("<tv>");
    // Deliberately unsorted and mixed with another channel: admission must be exact.
    for n in (1..=9).rev() {
        xml.push_str(&programme(
            "news",
            &format!("Next {n}"),
            60 + n * 60,
            120 + n * 60,
        ));
    }
    xml.push_str(&programme("other", "Wrong channel", 60, 120));
    xml.push_str(&programme("news", "Expired", 0, 60));
    xml.push_str(&programme("news", "Current", 60, 120));
    xml.push_str("</tv>");
    fixture.reply("/guide.xml", 200, &xml);
    let result = guide_with(&p, &entry, &guide_request(&p, &entry, 90), &mut fixture).unwrap();
    let rows = &result.programmes;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].title, "Current");
    assert_eq!(rows[0].start, 60);
    assert_eq!(rows[0].end, 120);
    assert_eq!(rows[0].description, "Full description");
    assert_eq!(rows[1].title, "Next 1");
    assert_eq!(fixture.streams, 1);
    assert_eq!(fixture.chunks_read, 3);
    assert_eq!(fixture.chunks.len(), 1);
    assert!(fixture.stopped);
    assert_eq!(fixture.calls.len(), 1);
}

#[test]
fn m3u_guide_tries_next_declared_guide_after_http_or_xml_failure() {
    let p = profile("http://127.0.0.1/source.m3u".into());
    let entry = guide_entry(&p);
    let mut fixture = GuideFixture::new(
        &p.source,
        "#EXTM3U url-tvg='/refused.xml,/invalid.xml,/working.xml'\n",
    );
    fixture.reply("/refused.xml", 503, "temporarily unavailable");
    fixture.reply("/invalid.xml", 200, "<html>not a programme guide</html>");
    fixture.reply(
        "/working.xml",
        200,
        &format!("<tv>{}</tv>", programme("news", "Fallback", 60, 120)),
    );
    let result = guide_with(&p, &entry, &guide_request(&p, &entry, 90), &mut fixture).unwrap();
    assert_eq!(result.programmes[0].title, "Fallback");
    assert_eq!(fixture.calls.len(), 3);
    assert!(fixture.replies.is_empty());
}

#[test]
fn cancelled_metadata_does_no_playlist_or_guide_io() {
    let p = profile("http://127.0.0.1/source.m3u".into());
    let entry = guide_entry(&p);
    let mut fixture = GuideFixture::new(&p.source, "#EXTM3U url-tvg='/guide.xml'\n");
    fixture.cancelled = true;
    assert!(guide_with(&p, &entry, &guide_request(&p, &entry, 90), &mut fixture).is_err());
    assert_eq!(fixture.streams, 0);
    assert!(fixture.calls.is_empty());
}

#[test]
fn cancellation_during_guide_response_discards_metadata_and_fallback() {
    let p = profile("http://127.0.0.1/source.m3u".into());
    let entry = guide_entry(&p);
    for first_body in [
        "<tv/>".to_string(),
        format!("<tv>{}</tv>", programme("news", "Stale", 60, 120)),
    ] {
        let mut fixture = GuideFixture::new(&p.source, "#EXTM3U url-tvg='/first.xml,/later.xml'\n");
        fixture.reply("/first.xml", 200, &first_body);
        fixture.reply("/later.xml", 200, "<tv/>");
        fixture.cancel_after_reply = true;
        assert!(guide_with(&p, &entry, &guide_request(&p, &entry, 90), &mut fixture).is_err());
        assert_eq!(fixture.calls.len(), 1);
        assert_eq!(fixture.replies.len(), 1);
    }
}

fn guide_request(profile: &ProviderProfile, entry: &CatalogEntry, now: i64) -> ott::guide::Request {
    ott::guide::Request {
        context: ott::guide::Context {
            generation: 7,
            profile_id: profile.id.clone(),
            entry_id: entry.id.clone(),
        },
        query: ott::guide::Query::CurrentNext { now },
    }
}

struct DispatchFixture {
    replies: VecDeque<(&'static str, Value)>,
    calls: Vec<String>,
}
impl ProviderHttp for DispatchFixture {
    fn request(&mut self, request: &Request) -> Result<Response, &'static str> {
        assert!(
            request.body.is_none(),
            "MAG/Xtream must stay on their GET protocol"
        );
        let (expected, body) = self.replies.pop_front().expect("unexpected provider call");
        assert!(request.url.contains(expected), "unexpected provider action");
        self.calls.push(request.url.clone());
        Ok(Response {
            status: 200,
            body: serde_json::to_vec(&body).unwrap(),
        })
    }
    fn cancelled(&self) -> bool {
        false
    }
}

#[test]
fn dispatcher_preserves_mag_catalog_create_link_and_short_guide() {
    use serde_json::json;
    let p=ProviderProfile {
        id:"mag".into(),name:"MAG".into(),kind:ProviderKind::Stalker,
        source:json!({"portal":"https://portal.invalid/c/","mac":"00:1a:79:01:02:03","include_vod":false}).to_string(),
        source_alias:String::new(),enabled:true,
    };
    let mut h=DispatchFixture {
        replies:vec![
            ("action=handshake",json!({"js":{"token":"fixture-token"}})),
            ("action=get_profile",json!({"js":{"id":1,"status":0}})),
            ("action=get_genres",json!({"js":[{"id":2,"title":"News"}]})),
            ("action=get_ordered_list",json!({"js":{"total_items":1,"data":[{"id":7,"name":"News","cmd":"ffmpeg /ch/7","tv_genre_id":"2"}]}})),
            ("action=handshake",json!({"js":{"token":"fresh-token"}})),
            ("action=get_profile",json!({"js":{"id":1,"status":0}})),
            ("action=create_link",json!({"js":{"cmd":"ffmpeg https://stream.invalid/fresh.m3u8"}})),
            ("action=handshake",json!({"js":{"token":"guide-token"}})),
            ("action=get_profile",json!({"js":{"id":1,"status":0}})),
            ("action=get_short_epg",json!({"js":[
                {"name":"Expired","start_timestamp":"100","end_timestamp":"200"},
                {"name":"Current","start_timestamp":"200","end_timestamp":"300"},
                {"name":"Next","start_timestamp":"300","end_timestamp":"400"}
            ]})),
        ].into(),calls:Vec::new(),
    };
    let mut rows = Vec::new();
    visit_with(&p, &mut h, &mut |batch| {
        rows.extend(batch);
        Ok(())
    })
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].media, MediaLocator::Stalker { .. }));
    assert_eq!(
        resolve_with(&p, &rows[0], &mut h).unwrap(),
        "https://stream.invalid:443/fresh.m3u8"
    );
    let r = guide_request(&p, &rows[0], 250);
    let guide = guide_with(&p, &rows[0], &r, &mut h).unwrap();
    assert_eq!(guide.coverage, ott::guide::Coverage::ProviderShortGuide);
    assert_eq!(
        guide
            .programmes
            .iter()
            .map(|p| p.title.as_str())
            .collect::<Vec<_>>(),
        ["Current", "Next"]
    );
    assert!(h.replies.is_empty());
}

#[test]
fn dispatcher_preserves_xtream_catalog_resolution_and_metadata() {
    use serde_json::json;
    let p = ProviderProfile {
        id: "xtream".into(),
        name: "Xtream".into(),
        kind: ProviderKind::Xtream,
        source:
            json!({"server":"https://xtream.invalid","username":"fixture","password":"fixture"})
                .to_string(),
        source_alias: String::new(),
        enabled: true,
    };
    let account = json!({"user_info":{"auth":1,"status":"Active"}});
    let mut h=DispatchFixture {
        replies:vec![
            ("player_api.php?",account.clone()),
            ("action=get_live_categories",json!([])),
            ("action=get_live_streams",json!([{"stream_id":42,"name":"News"}])),
            ("action=get_vod_categories",json!([])),
            ("action=get_vod_streams",json!([])),
            ("action=get_series_categories",json!([])),
            ("action=get_series",json!([])),
            ("player_api.php?",account.clone()),
            ("player_api.php?",account),
            ("action=get_short_epg",json!({"epg_listings":[{"start_timestamp":"200","stop_timestamp":"300","title":"TmV3cw==","description":"RGVzY3JpcHRpb24="}]})),
        ].into(),calls:Vec::new(),
    };
    let mut rows = Vec::new();
    visit_with(&p, &mut h, &mut |batch| {
        rows.extend(batch);
        Ok(())
    })
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].media, MediaLocator::Xtream { .. }));
    assert!(resolve_with(&p, &rows[0], &mut h)
        .unwrap()
        .contains("/live/fixture/fixture/42."));
    let result = metadata_with(&p, &rows[0], 250, &mut h).unwrap();
    assert_eq!(result["programmes"][0]["title"], "News");
    assert!(h.replies.is_empty());
}


#[test]
fn portal_only_m3u_never_requests_a_tv_catalogue_guide_or_direct_stream() {
    let tv = profile("https://fixture.invalid/list.m3u".into());
    let entry = guide_entry(&tv);
    let portal = profile("portal::[key:fixture-secret]https://fixture.invalid/api/".into());
    assert!(portal.validate().is_ok());
    let mut http = GuideFixture::new(&portal.source, "");
    let mut emitted = 0;
    visit_with(&portal, &mut http, &mut |rows| { emitted += rows.len(); Ok(()) }).unwrap();
    assert_eq!(emitted, 0);
    let metadata = metadata_with(&portal, &entry, 0, &mut http).unwrap();
    assert!(metadata.get("programmes").is_none());
    let request = guide_request(&portal, &entry, 0);
    assert!(matches!(guide_with(&portal, &entry, &request, &mut http),
        Err("Provider uses another metadata adapter")));
    assert_eq!(resolve_with(&portal, &entry, &mut http), Err("Provider uses another playback adapter"));
    assert!(http.calls.is_empty());
    assert_eq!(http.streams, 0);
    assert_eq!(http.chunks_read, 0);
}

#[test]
fn edem_dispatch_uses_latest_stream_host_and_separates_channel_from_guide_id() {
    struct TemplateHttp { calls: Vec<String> }
    impl ProviderHttp for TemplateHttp {
        fn cancelled(&self)->bool {false}
        fn request(&mut self, request:&Request)->Result<Response,&'static str> {
            self.calls.push(request.url.clone());
            assert_eq!(request.url,"https://epg.drm-play.com/edem/edem_epg_ico1.m3u8");
            assert_eq!(request.headers, vec!["User-Agent: Mozilla/5.0"]);
            assert!(request.body.is_none());
            Ok(Response {status:200,body:b"#EXTM3U\n#EXTINF:-1 tvg-id=\"743\" tvg-rec=\"7\" group-title=\"News\",News HD\nhttp://localhost/iptv/000000000000/407/index.m3u8\n".to_vec()})
        }
    }
    let mut profile=profile(r#"{"key":"first_key","list":1}"#.into());
    profile.kind=ProviderKind::Edem;
    let mut http=TemplateHttp{calls:Vec::new()};
    let mut rows=Vec::new();
    visit_with(&profile,&mut http,&mut |batch|{rows.extend(batch);Ok(())}).unwrap();
    assert_eq!(rows.len(),1);
    let row=&rows[0];
    assert_eq!(row.provider_id.as_deref(),Some("407"));
    assert_eq!(row.live.as_ref().unwrap().guide_id.as_deref(),Some("743"));
    assert_eq!(row.id,"fixture:edem:407");
    let url=resolve_with(&profile,row,&mut http).unwrap();
    assert_eq!(url,format!("http://{}/iptv/first_key/407/index.m3u8",ott::edem::STREAM_HOST));
    assert!(!http.calls.iter().any(|url|url.contains("first_key")));
    let catchup=row.live.as_ref().unwrap().catchup.as_ref().unwrap();
    assert_eq!(catchup.retention_seconds,7*86400);
    let archive=ott::archive::plan(catchup,&url,ott::archive::ArchiveInterval{start:100,end:200},300).unwrap();
    assert!(archive.url.ends_with("?utc=100&lutc=300"));
    profile.source=r#"{"key":"second_key","list":1}"#.into();
    let mut updated=Vec::new();
    visit_with(&profile,&mut http,&mut |batch|{updated.extend(batch);Ok(())}).unwrap();
    assert_eq!(updated[0].id,row.id,"key changes preserve stable channel identity");
}

#[test]
fn edem_guide_uses_template_epg_id_and_current_list_namespace() {
    struct JsonHttp;
    impl ProviderHttp for JsonHttp {
        fn cancelled(&self)->bool {false}
        fn request(&mut self,request:&Request)->Result<Response,&'static str> {
            assert_eq!(request.url,"https://epg.drm-play.com/iptv-e2-soveni/epg/743.json");
            assert_eq!(request.headers,vec!["User-Agent: Mozilla/5.0"]);
            Ok(Response{status:200,body:br#"{"epg_data":[{"time":100,"time_to":200,"name":"Now","descr":"Full synopsis"},{"time":"200","time_to":"300","name":"Next"}]}"#.to_vec()})
        }
    }
    let mut p=profile(r#"{"key":"test_key","list":1}"#.into());p.kind=ProviderKind::Edem;
    let mut row=ott::parse_m3u("#EXTM3U\n#EXTINF:-1 tvg-id=\"407\",News\nhttp://media.invalid/video\n",&ott::edem::Config::parse(&p.source).unwrap().template_profile(&p)).unwrap().entries.remove(0);
    row.live.as_mut().unwrap().guide_id=Some("743".into());
    let request=guide_request(&p,&row,150);
    let result=guide_with(&p,&row,&request,&mut JsonHttp).unwrap();
    assert_eq!(result.programmes.len(),2);
    assert_eq!(result.programmes[0].description,"Full synopsis");
}
