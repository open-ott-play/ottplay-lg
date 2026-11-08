use super::*;
use crate::plex::{Origin, ServerId};

#[test]
fn bounded_reads_respect_revocation_and_reject_non_numeric_item_identity_before_io() {
    let client = Client::new(
        ServerId::UNSET,
        "fixture",
        Origin::http("127.0.0.1", 1),
        "",
        "cid",
    );
    client.disable_data_io();
    for invalid in [
        "",
        "../23",
        "23?extra=1",
        "23/children",
        "123456789012345678901",
    ] {
        assert!(client.metadata_bounded(invalid).is_none());
    }
    assert_eq!(client.denied_data_requests(), 0);
    assert!(client.metadata_bounded("23").is_none());
    assert!(client.sections_bounded().is_none());
    assert!(client
        .section_items_query_bounded(&SectionQuery {
            section_key: 4,
            sort: "",
            filters: &[],
            start: 0,
            size: 128,
            include_meta: false,
        })
        .is_none());
    assert_eq!(client.denied_data_requests(), 3);
}

// Token-bearing HTTP is deliberately developer-only. Store builds keep the
// existing credential policy and TLS admission tests at the transport boundary.
#[cfg(feature = "devtriggers")]
fn response(body: String, inspect: impl FnOnce(&Client)) -> String {
    use std::io::{Read, Write};
    use std::time::Duration;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut socket, _) =
            plx_base::testnet::accept_within(&listener, Duration::from_secs(2)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut chunk = [0; 1024];
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let count = socket.read(&mut chunk).unwrap();
            assert!(count > 0 && request.len() + count <= 16 * 1024);
            request.extend_from_slice(&chunk[..count]);
        }
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        // Oversized responses may be closed before the fixture has sent its tail.
        let _ = socket.write_all(body.as_bytes());
        String::from_utf8(request).unwrap()
    });
    let client = Client::new(
        ServerId::UNSET,
        "fixture",
        Origin::http("127.0.0.1", port as i32),
        "fixture-token",
        "cid",
    );
    inspect(&client);
    server.join().unwrap()
}

#[cfg(feature = "devtriggers")]
#[test]
fn bounded_metadata_keeps_auth_and_complete_description_but_rejects_foreign_identity() {
    let body = serde_json::json!({"MediaContainer":{"Metadata":[{
        "ratingKey":"23", "title":"Film", "summary":"First sentence. Second sentence.",
        "thumb":"/library/metadata/23/thumb/1", "duration":12000
    }]}})
    .to_string();
    let request = response(body.clone(), |client| {
        let item = client.metadata_bounded("23").unwrap();
        assert_eq!(item.summary, "First sentence. Second sentence.");
        assert_eq!(item.thumb, "/library/metadata/23/thumb/1");
        assert_eq!(item.duration, 12000);
    });
    assert!(request.starts_with("GET /library/metadata/23?X-Plex-Token=fixture-token "));
    assert!(request
        .to_ascii_lowercase()
        .contains("accept: application/json\r\n"));
    response(body, |client| {
        assert!(client.metadata_bounded("24").is_none())
    });
}

#[cfg(feature = "devtriggers")]
#[test]
fn bounded_catalogue_preserves_typed_filters_and_paging() {
    let request = response(
        serde_json::json!({"MediaContainer":{"Directory":[{
            "key":"4", "type":"movie", "title":"Films"
        }]}})
        .to_string(),
        |client| {
            assert_eq!(
                client.sections_bounded().unwrap().directory[0].title,
                "Films"
            );
        },
    );
    assert!(request.starts_with("GET /library/sections?X-Plex-Token=fixture-token "));
    let request = response(serde_json::json!({"MediaContainer":{"offset":128,"totalSize":129,"Metadata":[{"ratingKey":"23"}]}}).to_string(), |client| {
        let filters = vec![("type".into(), "4".into())];
        let page = client.section_items_query_bounded(&SectionQuery {
            section_key: 4, sort: "titleSort", filters: &filters,
            start: 128, size: 128, include_meta: true,
        }).unwrap();
        assert_eq!(page.offset, 128);
        assert_eq!(page.total_size, 129);
    });
    for term in [
        "/library/sections/4/all?",
        "includeMeta=1",
        "sort=titleSort",
        "type=4",
        "X-Plex-Container-Start=128",
        "X-Plex-Container-Size=128",
        "X-Plex-Token=fixture-token",
    ] {
        assert!(request.contains(term), "missing typed query term: {term}");
    }
}

#[cfg(feature = "devtriggers")]
#[test]
fn bounded_metadata_discards_an_oversized_valid_envelope() {
    let body = serde_json::json!({"MediaContainer":{"Metadata":[{
        "ratingKey":"23", "summary":"x".repeat(8 * 1024 * 1024)
    }]}})
    .to_string();
    response(body, |client| {
        assert!(client.metadata_bounded("23").is_none())
    });
}
