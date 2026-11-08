//! Actual existing TLS peer + verifier/registry, never a synthetic verification result.
//! The test CA is scoped by the shared fixture; production TLS policy is unchanged.
use super::*;
use plx_net::net::{self, Reply, TestCaGuard};
use plx_plex::plex::{self, ResolvePin};
use std::sync::Arc;

const HOST: &str = "127-0-0-1.manual-fixture.plex.direct";
struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        *IMPORT_AUTHORITY.lock().unwrap() = ImportAuthority::default();
        plex::reset_servers_for_test();
        plex::grant::reset_for_test();
        net::resolve::clear();
    }
}
fn reset() -> Reset {
    *IMPORT_AUTHORITY.lock().unwrap() = ImportAuthority::default();
    plex::reset_servers_for_test();
    plex::grant::reset_for_test();
    net::resolve::clear();
    assert!(net::global_init());
    Reset
}
fn peer(machine: &str, library: Reply) -> (net::Observed, TestCaGuard) {
    let cert = Arc::new(net::mint_cert(&[HOST]));
    let ca = TestCaGuard::install(&cert.pem, "manual-import");
    let server = net::spawn_scripted(cert, [
        ("/identity", Reply::ok(serde_json::to_vec(&serde_json::json!({
            "MediaContainer":{"machineIdentifier":machine}
        })).unwrap())),
        ("/library/sections", library),
    ]);
    (server, ca)
}
fn profile(port: u16) -> Profile {
    Profile::parse(serde_json::to_vec(&serde_json::json!({
        "address":format!("https://{HOST}:{port}"),"token":"manual-only-token",
        "serverId":"manual-fixture-machine","name":"Manual fixture"
    })).unwrap().as_slice()).unwrap()
}
fn requests(server: &net::Observed) -> Vec<String> {
    server.requests.lock().unwrap().iter().map(|bytes|
        String::from_utf8(bytes.clone()).unwrap().to_ascii_lowercase()).collect()
}
fn library() -> Reply { Reply::ok(br#"{"MediaContainer":{"Directory":[]}}"#.to_vec()) }

#[test]
fn manual_tls_source_keeps_pin_through_verification_registry_and_restore() {
    let _serial = plx_base::testlock::serial();
    let worker = plx_base::task::OffFrame::for_test();
    let _reset = reset();
    let (server, _ca) = peer("manual-fixture-machine", library());
    let profile = profile(server.port);
    let verified = verify_profile(&worker, &profile).expect("real TLS identity and authorized library");
    assert_eq!(verified.source.address, "127.0.0.1");
    let origin = verified.source.origin().unwrap();
    assert_eq!(origin.host(), HOST);
    assert_eq!(origin.port(), i32::from(server.port));
    let pin = ResolvePin::for_origin(&origin, "127.0.0.1").unwrap();
    assert_eq!(verified.source.resolve_pin(), Some(pin.clone()));
    let captured = requests(&server);
    assert_eq!(captured.len(), 2);
    assert!(captured[0].starts_with("get /identity "));
    assert!(!captured[0].contains("manual-only-token"));
    assert!(captured[1].starts_with("get /library/sections "));
    assert!(captured[1].contains("x-plex-token: manual-only-token"));
    let lease = plex::register_imported(&verified.proof, &profile.token, "manual-client",
        plex::ConnectionFacts::default(), None).unwrap();
    let client = plex::client_for(lease.server_id()).unwrap();
    assert_eq!(client.resolve_pin(), Some(&pin));
    assert_eq!(net::resolve::lookup(HOST, origin.port()), Some(pin.clone()));
    assert!(client.sections_bounded().is_some());
    let stored: SourceRef = serde_json::from_value(serde_json::to_value(&verified.source).unwrap()).unwrap();
    assert_eq!(stored.resolve_pin(), Some(pin));
    let result = restore_sources_with(vec![stored], || false, |profile| verify_profile(&worker, profile));
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].as_ref().unwrap().source.address, "127.0.0.1");
}

#[test]
fn managed_tls_primary_and_source_both_keep_the_same_numeric_address() {
    let _serial = plx_base::testlock::serial();
    let worker = plx_base::task::OffFrame::for_test();
    let _reset = reset();
    let (server, _ca) = peer("manual-fixture-machine", library());
    let directory = std::env::temp_dir().join(format!("manual-pin-primary-{}-{:?}",
        std::process::id(), std::thread::current().id()));
    std::fs::create_dir(&directory).unwrap();
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch { fn drop(&mut self) { plx_base::testscratch::remove_tree(&self.0); } }
    let _scratch = Scratch(directory.clone());
    let path = directory.join("profile.json");
    std::fs::write(&path, serde_json::to_vec(&serde_json::json!({
        "address":format!("https://{HOST}:{}",server.port),"token":"manual-only-token",
        "serverId":"manual-fixture-machine"
    })).unwrap()).unwrap();
    let saved: Session = serde_json::from_value(serde_json::json!({"client_id":"manual-client",
        "account_token":"account-token","future_setting":"keep"})).unwrap();
    let candidate = read_and_verify(&worker, &path, &saved,
        &std::sync::atomic::AtomicBool::new(false)).unwrap().unwrap();
    let restored = candidate.credentials.merge_into(&saved);
    assert_eq!(candidate.verified.source.address, restored.server.address);
    assert_eq!(candidate.verified.source.resolve_pin(), restored.server.resolve_pin());
    assert_eq!(restored.server.address, "127.0.0.1");
    assert_eq!(restored.sources[0].address, restored.server.address);
    assert_eq!(restored.server.resolve_pin(), restored.sources[0].resolve_pin());
    assert!(restored.server.resolve_pin().is_some());
    assert!(restored.account_token.is_empty());
    assert_eq!(restored.server.token, "manual-only-token");
    assert_eq!(serde_json::to_value(restored).unwrap()["future_setting"], "keep");
}

#[test]
fn pinned_manual_identity_mismatch_never_sends_the_token_or_registers() {
    let _serial = plx_base::testlock::serial();
    let worker = plx_base::task::OffFrame::for_test();
    let _reset = reset();
    let (server, _ca) = peer("different-machine", library());
    assert_eq!(verify_profile(&worker, &profile(server.port)).err(), Some("server identity does not match the profile"));
    let captured = requests(&server);
    assert_eq!(captured.len(), 1);
    assert!(!captured[0].contains("manual-only-token"));
    assert!(plex::id_of_machine("manual-fixture-machine").is_none());
    assert!(net::resolve::lookup(HOST, i32::from(server.port)).is_none());
}

#[test]
fn pinned_manual_library_denial_and_redirect_do_not_authorize_or_follow() {
    let _serial = plx_base::testlock::serial();
    let worker = plx_base::task::OffFrame::for_test();
    let _reset = reset();
    for reply in [Reply::status(401), Reply::redirect(302, "https://credential-target.invalid/library/sections")] {
        let (server, _ca) = peer("manual-fixture-machine", reply);
        assert_eq!(verify_profile(&worker, &profile(server.port)).err(), Some("Plex server did not authorize library access"));
        let captured = requests(&server);
        assert_eq!(captured.len(), 2);
        assert!(!captured[0].contains("manual-only-token"));
        assert!(captured[1].contains("x-plex-token: manual-only-token"));
        assert!(plex::id_of_machine("manual-fixture-machine").is_none());
    }
}

#[test]
fn pinned_manual_origin_still_requires_the_certificate_hostname() {
    let _serial = plx_base::testlock::serial();
    let worker = plx_base::task::OffFrame::for_test();
    let _reset = reset();
    let cert = Arc::new(net::mint_cert(&["wrong-name.invalid"]));
    let _ca = TestCaGuard::install(&cert.pem, "manual-import-wrong-name");
    let server = net::spawn_scripted(cert, [("/identity", Reply::ok(
        br#"{"MediaContainer":{"machineIdentifier":"manual-fixture-machine"}}"#.to_vec()))]);
    assert_eq!(verify_profile(&worker, &profile(server.port)).err(), Some("server identity could not be verified"));
    assert!(requests(&server).is_empty());
    assert!(plex::id_of_machine("manual-fixture-machine").is_none());
}


#[test]
fn verified_manual_borrow_repairs_routing_without_taking_the_account_slot() {
    let _serial = plx_base::testlock::serial();
    let worker = plx_base::task::OffFrame::for_test();
    let _reset = reset();
    let (server, _ca) = peer("manual-fixture-machine", library());
    let profile = profile(server.port);
    let origin = Origin::parse(&profile.address).unwrap();
    let id = plex::register_pinned_with_client_id("manual-fixture-machine", &origin,
        &profile.token,None,"account-client",plex::ConnectionFacts::default());
    let client = plex::client_for(id).unwrap();
    assert!(client.resolve_pin().is_none());
    let identity = (client.instance_gen(),client.token_gen(),client.grant_epoch());
    let verified = verify_profile(&worker, &profile).unwrap();
    set_authorized_sources(std::slice::from_ref(&verified.source)).unwrap();
    assert_eq!(register_verified(&verified).unwrap(),id);
    assert!(client.resolve_pin().is_some());
    assert_eq!((client.instance_gen(),client.token_gen(),client.grant_epoch()),identity);
    assert!(IMPORT_AUTHORITY.lock().unwrap().owned.is_empty());
    set_authorized_sources(&[]).unwrap();
    assert!(client.uses_token(&profile.token),"removing a borrowed import cannot revoke the account");
}

#[test]
fn restored_manual_route_accepts_legacy_host_but_rejects_conflicting_address_or_port() {
    let origin = Origin::parse("https://127-0-0-1.manual.plex.direct:32400").unwrap();
    let source = SourceRef { machine_id:"manual".into(), token:"token".into(),
        address:origin.host().into(),origin_url:origin.base(),port:32400,..Default::default() };
    for address in [origin.host(),"127.0.0.1"] {
        let mut source=source.clone();source.address=address.into();
        let result=restore_sources_with(vec![source],||false,|profile| {
            assert_eq!(profile.address,origin.base());Ok(())
        });
        assert_eq!(result,vec![Ok(())]);
    }
    for (address,port) in [("127.0.0.2",32400),("other.invalid",32400),("127.0.0.1",32401)] {
        let mut source=source.clone();source.address=address.into();source.port=port;
        let result:Vec<Result<(),_>>=restore_sources_with(vec![source],||false,
            |_| panic!("conflicting saved routing must fail before identity or token IO"));
        assert_eq!(result,vec![Err("stored Plex address does not match its origin")]);
    }
}
