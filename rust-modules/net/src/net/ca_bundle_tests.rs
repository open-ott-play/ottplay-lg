//! Real libcurl/TLS regression for the request-scoped strict bundle entrypoint.
use super::*;
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::PathBuf, sync::Arc};

struct CaFile(PathBuf);
impl CaFile {
    fn new(pem: &str) -> Self {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("ott-scoped-ca-{}-{nonce}.pem", std::process::id()));
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).unwrap();
        file.write_all(pem.as_bytes()).unwrap();
        Self(path)
    }
    fn path(&self) -> &str { self.0.to_str().unwrap() }
}
impl Drop for CaFile {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
}
fn bounded() -> Timeouts {
    Timeouts { connect_s: 2, total_s: 3, total_ms: 3000, low_speed_bps: 0, low_speed_s: 0 }
}
fn scoped(url: &str, bundle: &str) -> Result<Resp, RequestFailure> {
    request_ca_bundle_evidence(url, &[], "GET", None, bounded(), false, Some(1024), bundle)
}

#[test]
fn explicit_bundle_accepts_valid_chain_without_installing_global_trust() {
    let _serial = plx_base::testlock::serial();
    assert!(global_init() && available(), "requires real host libcurl");
    assert!(test_ca_bundle::get().is_none());
    let cert = Arc::new(mint_ca_issued_cert(&["127.0.0.1"], ymd_from_now(-1), ymd_from_now(1)));
    let ca = CaFile::new(&cert.pem);
    let peer = spawn_scripted(cert, [("/health", Reply::ok(b"ok".to_vec())),
        ("/pair", Reply::ok(br#"{"code":"SYNTHETIC"}"#.to_vec()))]);
    let origin = format!("https://127.0.0.1:{}", peer.port);
    assert!(request_evidence(&format!("{origin}/health"), &[], "GET", None,
        bounded(), false, Some(1024), None).is_err(), "private CA is not system trust");
    assert_eq!(scoped(&format!("{origin}/health"), ca.path()).unwrap().body, b"ok");
    let body = br#"{"device":"synthetic-fixture"}"#;
    let response = request_ca_bundle_evidence(&format!("{origin}/pair"),
        &["Content-Type: application/json".into()], "POST", Some(body), bounded(), false,
        Some(1024), ca.path()).unwrap();
    assert_eq!(response.status, 200);
    let requests = peer.requests.lock().unwrap();
    assert!(requests.iter().any(|request| request.starts_with(b"POST /pair ") && request.ends_with(body)));
    drop(requests);
    assert!(test_ca_bundle::get().is_none());
    assert!(request_evidence(&format!("{origin}/health"), &[], "GET", None,
        bounded(), false, Some(1024), None).is_err(), "explicit call does not affect later ordinary calls");
}

#[test]
fn explicit_bundle_preserves_hostname_dates_and_chain_verification() {
    let _serial = plx_base::testlock::serial();
    assert!(global_init() && available(), "requires real host libcurl");
    for (names, before, after) in [
        (vec!["wrong.example.test"], -1, 1),
        (vec!["127.0.0.1"], -90, -30),
        (vec!["127.0.0.1"], 30, 90),
    ] {
        let cert = Arc::new(mint_ca_issued_cert(&names, ymd_from_now(before), ymd_from_now(after)));
        let ca = CaFile::new(&cert.pem);
        let peer = spawn_scripted(cert, [("/pair", Reply::ok(b"must not arrive".to_vec()))]);
        let failure = scoped(&format!("https://127.0.0.1:{}/pair", peer.port), ca.path()).err().expect("verification must refuse");
        assert_eq!(failure.curl_rc, Some(60));
        assert!(peer.requests.lock().unwrap().is_empty(), "TLS refusal precedes application data");
    }
    let cert = Arc::new(mint_ca_issued_cert(&["127.0.0.1"], ymd_from_now(-1), ymd_from_now(1)));
    let unrelated = mint_ca_issued_cert(&["127.0.0.1"], ymd_from_now(-1), ymd_from_now(1));
    let correct_ca = CaFile::new(&cert.pem);
    let other_ca = CaFile::new(&unrelated.pem);
    let body = b"verified with the correct issuer";
    let peer = spawn_scripted(cert, [("/pair", Reply::ok(body.to_vec()))]);
    let url = format!("https://127.0.0.1:{}/pair", peer.port);
    // Hold the listener/leaf constant: only the client's trusted issuer changes.
    let accepted = scoped(&url, correct_ca.path()).unwrap();
    assert_eq!(accepted.status, 200);
    assert_eq!(accepted.body, body);
    assert_eq!(peer.requests.lock().unwrap().len(), 1);
    let rejected = scoped(&url, other_ca.path()).err().expect("unrelated issuer must fail closed");
    assert_eq!(rejected.cause, RequestError::Transport);
    // TLS backends can report an issuer/signature refusal as handshake failure
    // (35) or certificate verification failure (60). Neither may send HTTP bytes.
    assert!(matches!(rejected.curl_rc, Some(35 | 60)), "{rejected:?}");
    assert_eq!(rejected.status, None);
    assert_eq!(peer.requests.lock().unwrap().len(), 1);
    let accepted_again = scoped(&url, correct_ca.path()).unwrap();
    assert_eq!(accepted_again.status, 200);
    assert_eq!(accepted_again.body, body);
    assert_eq!(peer.requests.lock().unwrap().len(), 2);
}

#[test]
fn explicit_bundle_refuses_missing_bundle_and_does_not_follow_redirects() {
    let _serial = plx_base::testlock::serial();
    assert!(global_init() && available(), "requires real host libcurl");
    let cert = Arc::new(mint_ca_issued_cert(&["127.0.0.1"], ymd_from_now(-1), ymd_from_now(1)));
    let ca = CaFile::new(&cert.pem);
    let target = spawn_scripted(Arc::clone(&cert), [("/pair", Reply::ok(b"must not arrive".to_vec()))]);
    let peer = spawn_scripted(cert, [("/pair", Reply::redirect(302,
        &format!("https://127.0.0.1:{}/pair", target.port)))]);
    let url = format!("https://127.0.0.1:{}/pair", peer.port);
    let response = scoped(&url, ca.path()).unwrap();
    assert_eq!(response.status, 302);
    assert_eq!(target.accepted(), 0, "no second-origin connection");
    let deleted_path = ca.path().to_owned(); drop(ca);
    assert!(scoped(&url, &deleted_path).is_err(), "no fallback from missing explicit bundle");
}

#[test]
fn explicit_bundle_never_uses_a_remembered_or_already_latched_key() {
    let _serial = plx_base::testlock::serial();
    assert!(global_init() && available(), "requires real host libcurl");
    let cert = expired_leaf(&["127.0.0.1"]);
    let ca = CaFile::new(&cert.pem);
    let peer = spawn_scripted(Arc::clone(&cert), [("/pair", Reply::ok(b"must not arrive".to_vec()))]);
    let _remembered = remember(peer.port, &cert);
    let url = format!("https://127.0.0.1:{}/pair", peer.port);
    // A remembered key must not turn a date refusal into a second, pinned attempt.
    assert_eq!(scoped(&url, ca.path()).err().expect("expired even with remembered key").curl_rc, Some(60));
    // Nor may a latch created by some other caller start this request in key mode.
    let key = key_of_port(peer.port);
    keypin::key_established(&key, &leaf_pin(&cert), Some(10));
    assert!(matches!(keypin::begin(&key), keypin::Mode::Key { .. }));
    assert_eq!(scoped(&url, ca.path()).err().expect("expired even with latched key").curl_rc, Some(60));
    assert!(peer.requests.lock().unwrap().is_empty());
    assert!(matches!(keypin::begin(&key), keypin::Mode::Key { .. }), "strict scoped call leaves other callers' state unchanged");
}
