//! Safe remote diagnostics at the actual response/snapshot boundary, without IO.
use super::*;

const PRIVATE_BODY: &str = "https://user:SECRET@private.invalid/?token=SECRET";

fn response(status: u16, value: Value) -> Outcome {
    Outcome::Http {
        response: Ok(protocol::Response {
            status,
            body: serde_json::to_vec(&value).unwrap(),
        }),
        elapsed_ms: 10,
    }
}

#[test]
fn discovery_reports_http_status_without_exposing_server_error_body() {
    let mut remote = Remote::disabled_for_test("dev_test", "runtime");
    remote.finish(
        Kind::Discovery,
        response(403, json!({"error":PRIVATE_BODY})),
        100,
    );
    let snapshot = remote.snapshot();
    assert_eq!(snapshot["state"], "error");
    assert!(snapshot["message"].as_str().unwrap().contains("HTTP 403"));
    assert!(!snapshot.to_string().contains("SECRET"));
    assert!(!snapshot.to_string().contains("private.invalid"));
}

#[test]
fn pairing_reports_http_failure_even_when_response_body_is_not_a_pairing_document() {
    let mut remote = Remote::disabled_for_test("dev_test", "runtime");
    remote
        .begin_pairing("https://control.invalid", "home", 0)
        .unwrap();
    remote.finish(
        Kind::Pairing,
        response(502, json!({"error":PRIVATE_BODY})),
        100,
    );
    let snapshot = remote.snapshot();
    assert_eq!(snapshot["state"], "error");
    assert!(snapshot["message"].as_str().unwrap().contains("HTTP 502"));
    assert!(remote.pairing.is_none());
    assert!(!snapshot.to_string().contains("SECRET"));
    assert!(!snapshot.to_string().contains("private.invalid"));
}

fn failure(cause: plx_net::net::RequestError, code: Option<i32>) -> plx_net::net::RequestFailure {
    plx_net::net::RequestFailure {
        cause,
        curl_rc: code,
        status: None,
        body_limit: None,
        verify: None,
    }
}

fn failed(failure: plx_net::net::RequestFailure) -> Outcome {
    Outcome::Http {
        response: Err(failure),
        elapsed_ms: 8000,
    }
}

#[test]
fn typed_transport_evidence_maps_to_bounded_safe_labels() {
    use plx_net::net::RequestError::{TimedOut, Transport};
    for (cause, code, label) in [
        (TimedOut, Some(28), "Request timed out"),
        (TimedOut, None, "Request timed out"),
        (Transport, Some(5), "DNS lookup failed"),
        (Transport, Some(6), "DNS lookup failed"),
        (Transport, Some(7), "Connection failed"),
        (Transport, Some(35), "TLS handshake failed"),
        (Transport, Some(51), "TLS certificate verification failed"),
        (Transport, Some(60), "TLS certificate verification failed"),
        (Transport, Some(58), "TLS client certificate failed"),
        (Transport, Some(77), "TLS trust store unavailable"),
        (Transport, Some(90), "TLS public key verification failed"),
        (Transport, Some(999), "Network request failed"),
        (Transport, None, "Network request failed"),
    ] {
        let diagnostic = transport::failure_message(&failure(cause, code));
        assert!(diagnostic.starts_with(label), "{diagnostic}");
        if let Some(code) = code {
            assert!(diagnostic.contains(&format!("transport code {code}")));
        }
        assert!(diagnostic.len() < 160);
        assert!(!diagnostic.contains("http://") && !diagnostic.contains("https://"));
    }
}

#[test]
fn certificate_verification_and_partial_http_evidence_are_not_lost() {
    use plx_net::net::RequestError::Transport;
    let mut tls = failure(Transport, Some(60));
    tls.verify = Some(20);
    let mut remote = Remote::disabled_for_test("dev_test", "runtime");
    remote.finish(Kind::Discovery, failed(tls), 100);
    let text = remote.snapshot()["message"].as_str().unwrap().to_owned();
    assert!(text.contains("TLS certificate verification failed"));
    assert!(text.contains("transport code 60") && text.contains("verification code 20"));

    let mut partial = failure(Transport, Some(23));
    partial.status = Some(503);
    partial.body_limit = Some(protocol::BODY_LIMIT);
    partial.verify = Some(20); // ignored unless curl reported certificate verification failure
    let diagnostic = transport::failure_message(&partial);
    assert!(diagnostic.contains("Response exceeds the size limit"));
    assert!(diagnostic.contains("HTTP 503") && diagnostic.contains("transport code 23"));
    assert!(!diagnostic.contains("verification"));
}

#[test]
fn pairing_transport_failure_is_visible_immediately_without_changing_retry_policy() {
    use plx_net::net::RequestError::Transport;
    let mut remote = Remote::disabled_for_test("dev_test", "runtime");
    remote
        .begin_pairing("https://control.invalid", "home", 0)
        .unwrap();
    remote.finish(Kind::Pairing, failed(failure(Transport, Some(6))), 100);
    let pairing = remote
        .pairing
        .as_ref()
        .expect("first failure remains retryable");
    assert_eq!(pairing.failures, 1);
    assert_eq!(pairing.next_ms, 2100);
    assert!(remote.snapshot()["message"]
        .as_str()
        .unwrap()
        .contains("DNS lookup failed"));
    for n in 2..=6 {
        remote.finish(Kind::Pairing, failed(failure(Transport, Some(6))), n * 100);
    }
    assert!(
        remote.pairing.is_none(),
        "the existing six-failure limit remains"
    );
    assert_eq!(remote.snapshot()["state"], "error");
    let before = remote.snapshot()["message"].clone();
    remote.finish(Kind::CancelPairing, response(204, Value::Null), 1000);
    assert_eq!(
        remote.snapshot()["message"],
        before,
        "cleanup cannot erase useful failure evidence"
    );
}

#[test]
fn successful_pairing_response_and_new_discovery_clear_stale_network_evidence() {
    use plx_net::net::RequestError::TimedOut;
    let mut remote = Remote::disabled_for_test("dev_test", "runtime");
    remote
        .begin_pairing("https://control.invalid", "home", 0)
        .unwrap();
    remote.finish(Kind::Pairing, failed(failure(TimedOut, Some(28))), 100);
    remote.finish(
        Kind::Pairing,
        response(
            201,
            json!({
                "id":"0123456789abcdef0123456789abcdef", "secret":"c".repeat(32),
                "code":"ABCD1234", "expires_in":60
            }),
        ),
        200,
    );
    assert_eq!(remote.snapshot()["state"], "waiting");
    assert_eq!(
        remote.snapshot()["message"],
        "Approve the pairing code with ott"
    );
    remote.finish(
        Kind::Pairing,
        response(202, json!({"status":"pending"})),
        300,
    );
    assert!(!remote.snapshot()["message"]
        .as_str()
        .unwrap()
        .contains("HTTP 202"));
    remote.finish(Kind::Pairing, failed(failure(TimedOut, Some(28))), 400);
    remote
        .discover_and_pair("https://new-control.invalid", 500)
        .unwrap();
    assert_eq!(remote.snapshot()["message"], "Finding command server");
}

#[test]
fn delivery_preserves_retry_and_expired_receipt_rules_while_showing_http_code() {
    let mut remote = Remote::disabled_for_test("dev_test", "runtime");
    remote.finish(
        Kind::Delivery(SendKind::Poll),
        response(500, json!({"error":PRIVATE_BODY})),
        100,
    );
    assert_eq!(remote.failures, 1);
    assert_eq!(remote.next_ms, 2100);
    assert!(remote.snapshot()["message"]
        .as_str()
        .unwrap()
        .contains("HTTP 500"));
    assert!(!remote.snapshot().to_string().contains("SECRET"));
    remote.finish(
        Kind::Delivery(SendKind::Response),
        response(404, Value::Null),
        300,
    );
    assert_eq!(
        remote.failures, 1,
        "expired response handling is not a transport retry"
    );
    assert_eq!(remote.next_ms, 1300);
    assert!(remote.snapshot()["message"]
        .as_str()
        .unwrap()
        .contains("HTTP 404"));
    remote.apply(Config::disabled("dev_test".into()), true);
    assert_eq!(remote.snapshot()["message"], "Remote control disabled");
}
