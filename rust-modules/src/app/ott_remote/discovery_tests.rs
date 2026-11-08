// These tests explicitly exercise the catalog branch; LAN behavior is covered in lan_discovery_tests.
//! New discovery API regressions shipped with the production cut.
//! No socket, storage, native process or clock is started.
use super::*;
use discovery::{CatalogProfile, ControllerBase, DiscoveryTarget};

fn document() -> Vec<u8> {
    br#"{"version":1,"servers":[{"id":"one._ottplay-ctrl._tcp.test.","domain":"test","address":"https://control.invalid/team"}]}"#.to_vec()
}
fn profile(body: &[u8]) -> CatalogProfile {
    let digest = plx_base::sha256::sha256(body).iter()
        .map(|byte| format!("{byte:02x}")).collect::<String>();
    CatalogProfile::from_config(Some("https://catalog.invalid/discovery.json"), Some(&digest))
        .unwrap().unwrap()
}
fn response(body: Vec<u8>) -> Outcome {
    Outcome::Http { response: Ok(protocol::Response { status: 200, body }), elapsed_ms: 1 }
}
fn configured(body: &[u8]) -> Remote {
    let mut remote = Remote::disabled_for_test("dev_test", "test-runtime");
    remote.catalog_profile = Ok(Some(profile(body)));
    remote
}
fn finish_find(remote: &mut Remote, body: Vec<u8>) {
    let Some(DiscoveryTarget::CatalogUrl(profile)) = remote.discover_address.take() else {
        panic!("Find must queue the typed catalog target");
    };
    remote.finish(Kind::Catalog(profile), response(body), 10);
}
fn paired_config() -> Config {
    Config::parse(json!({"enabled":true,"address":"https://old.invalid/control",
        "token":"A".repeat(32),"device_id":"dev_test"})).unwrap()
}

#[test]
fn discovery_profile_is_explicit_and_rejects_partial_or_unsafe_input() {
    assert!(CatalogProfile::from_config(None, None).unwrap().is_none());
    assert!(CatalogProfile::from_config(Some(""), Some("")).unwrap().is_none());
    let digest = "a".repeat(64);
    assert!(CatalogProfile::from_config(Some("https://catalog.invalid/file.json"), None).is_err());
    assert!(CatalogProfile::from_config(None, Some(&digest)).is_err());
    for url in ["http://catalog.invalid/file.json", "https://user@catalog.invalid/file.json",
        "https://catalog.invalid/file.json?token=x", "https://catalog.invalid/file.json#x",
        "https://catalog.invalid/%66ile", "https://catalog.invalid/../file", "https://catalog.invalid//file",
        "https://catalog.invalid/file/", "https://catalog.invalid/$(id)", "https://catalog.invalid/`id`",
        "https://catalog.invalid/a'b", "https://catalog.invalid/a\nb", "https://catalog.invalid:0443/file",
        "https://localhost/file", "https://127.00.0.1/file", "https://1.2.3/file"] {
        assert!(CatalogProfile::from_config(Some(url), Some(&digest)).is_err(), "{url}");
    }
    for hash in ["A".repeat(64), "a".repeat(63), "g".repeat(64)] {
        assert!(CatalogProfile::from_config(Some("https://catalog.invalid/file.json"), Some(&hash)).is_err());
    }
}

#[test]
fn discovery_profile_rejects_one_sided_empty_configuration() {
    for (url, digest) in [(Some(""), None), (None, Some(""))] {
        assert_eq!(CatalogProfile::from_config(url, digest), Err(discovery::INVALID_PROFILE));
    }
    // Explicitly empty pairs remain the documented unconfigured profile.
    assert_eq!(CatalogProfile::from_config(None, None), Ok(None));
    assert_eq!(CatalogProfile::from_config(Some(""), Some("")), Ok(None));
    let digest = "a".repeat(64);
    assert_eq!(CatalogProfile::from_config(Some(""), Some(&digest)), Err(discovery::INVALID_PROFILE));
    assert_eq!(CatalogProfile::from_config(Some("https://catalog.invalid/file.json"), Some("")),
        Err(discovery::INVALID_PROFILE));
}

#[test]
fn discovery_targets_preserve_exact_catalog_url_and_manual_api_suffix() {
    let catalog = DiscoveryTarget::CatalogUrl(profile(&document()));
    let request = catalog.request();
    assert_eq!(request.url, "https://catalog.invalid/discovery.json");
    assert_eq!(request.method, "GET");
    assert!(request.token.is_empty() && request.body.is_none());
    assert_eq!(catalog.body_limit(), 8192);
    let manual = DiscoveryTarget::ControllerBase(ControllerBase::new("https://control.invalid/team/").unwrap());
    assert_eq!(manual.request().url, "https://control.invalid:443/team/api/discovery");
    assert_eq!(manual.body_limit(), 65536);
}

#[test]
fn catalog_digest_precedes_json_and_descriptor_validation() {
    let trusted = document();
    let configured = profile(&trusted);
    assert_eq!(configured.parse(b"not json").unwrap_err(), "Discovery catalog digest mismatch");
    let malformed = b"not json";
    assert_eq!(profile(malformed).parse(malformed).unwrap_err(), "Invalid discovery catalog");
    let parsed = configured.parse(&trusted).unwrap();
    assert_eq!(parsed[0].id, "one._ottplay-ctrl._tcp.test.");
    assert_eq!(parsed[0].domain, "test");
    assert_eq!(parsed[0].address, "https://control.invalid:443/team");
    let oversized = vec![b' '; 8193];
    assert!(profile(&oversized).parse(&oversized).is_err());
}

#[test]
fn discovery_descriptor_refusals_are_atomic_and_match_pairing_id_bound() {
    let valid = json!({"id":"one","domain":"test","address":"https://control.invalid"});
    for invalid in [json!({"id":"","domain":"test","address":"https://control.invalid"}),
        json!({"id":"x".repeat(255),"domain":"test","address":"https://control.invalid"}),
        json!({"id":"two","domain":"bad\ndomain","address":"https://control.invalid"}),
        json!({"id":"two","address":"https://control.invalid"}),
        json!({"id":"two","domain":"test","address":"http://control.invalid"}),
        valid.clone()] {
        let body = serde_json::to_vec(&json!({"version":1,"servers":[valid,invalid]})).unwrap();
        assert!(profile(&body).parse(&body).is_err());
    }
    let boundary = serde_json::to_vec(&json!({"version":1,"servers":[
        {"id":"x".repeat(254),"domain":"test","address":"https://control.invalid"}]})).unwrap();
    assert!(profile(&boundary).parse(&boundary).is_ok());
    let too_many = serde_json::to_vec(&json!({"version":1,"servers":(0..17)
        .map(|i| json!({"id":format!("id{i}"),"domain":"test","address":"https://control.invalid"}))
        .collect::<Vec<_>>()})).unwrap();
    assert!(profile(&too_many).parse(&too_many).is_err());
}

#[test]
fn find_singleton_waits_for_explicit_bound_choice_without_enabling_control() {
    let body = document();
    let mut remote = configured(&body);
    remote.queue_find(0,false).unwrap();
    finish_find(&mut remote, body);
    let snapshot = remote.snapshot();
    assert_eq!(snapshot["find_state"], "choose");
    assert!(remote.pairing.is_none() && remote.pending_save.is_none() && remote.flight.is_none());
    assert!(!remote.config.enabled && remote.config.token.is_empty());
    let choice = snapshot["servers"][0]["choice"].as_str().unwrap();
    assert!(!choice.contains("control.invalid") && !choice.contains("one."));
    remote.select_discovered_choice(choice, 20).unwrap();
    let request = remote.pairing.as_ref().unwrap().request(false);
    assert_eq!(request.url, "https://control.invalid:443/team/api/pairings");
    assert_eq!(request.method, "POST");
    assert!(request.token.is_empty());
    assert_eq!(serde_json::from_str::<Value>(request.body.as_deref().unwrap()).unwrap(),
        json!({"device_id":"dev_test","server_id":"one._ottplay-ctrl._tcp.test."}));
    assert_eq!(remote.snapshot()["find_message"], "");
    assert!(!remote.config.enabled);
}

#[test]
fn manual_singleton_preserves_existing_pairing_behavior() {
    let mut remote = Remote::disabled_for_test("dev_test", "runtime");
    remote.discover_and_pair("https://control.invalid", 0).unwrap();
    remote.discover_address.take();
    // Legacy/manual v1 responses did not require the domain field.
    remote.finish(Kind::Discovery, response(br#"{"version":1,"servers":[{"id":"one","address":"https://control.invalid"}]}"#.to_vec()), 1);
    assert!(remote.pairing.is_some());
    assert_eq!(remote.snapshot()["find_message"], "");
}

#[test]
fn find_status_survives_old_connection_polling_and_bad_results_keep_saved_config() {
    let body = document();
    let mut remote = configured(&body);
    remote.apply(paired_config(), true);
    let original = serde_json::to_value(&remote.config).unwrap();
    remote.queue_find(0,false).unwrap();
    finish_find(&mut remote, body);
    let choices = remote.snapshot()["servers"].clone();
    remote.finish(Kind::Delivery(SendKind::Poll), Outcome::Http {
        response: Ok(protocol::Response { status: 503, body: b"private error".to_vec() }), elapsed_ms: 1,
    }, 11);
    assert_eq!(remote.snapshot()["find_state"], "choose");
    assert_eq!(remote.snapshot()["servers"], choices);
    remote.finish(Kind::Delivery(SendKind::Poll), response(serde_json::to_vec(
        &json!({"commands":[],"requests":[],"server_time":1000})).unwrap()), 12);
    assert_eq!(remote.snapshot()["find_state"], "choose");
    assert_eq!(remote.snapshot()["servers"], choices);
    remote.queue_find(13,false).unwrap();
    finish_find(&mut remote, b"malformed and digest mismatch".to_vec());
    assert_eq!(remote.snapshot()["find_state"], "error");
    assert_eq!(serde_json::to_value(&remote.config).unwrap(), original);
    assert!(remote.pairing.is_none() && remote.pending_save.is_none());
    assert!(remote.snapshot()["servers"].as_array().unwrap().is_empty());
}

#[test]
fn stale_choice_after_retry_cancel_or_config_change_cannot_pair() {
    for transition in 0..5 {
        let body = document();
        let mut remote = configured(&body);
        remote.queue_find(0,false).unwrap();
        finish_find(&mut remote, body.clone());
        let old = remote.snapshot()["servers"][0]["choice"].as_str().unwrap().to_owned();
        match transition {
            0 => { remote.queue_find(1,false).unwrap(); finish_find(&mut remote, body); }
            1 => remote.cancel_pairing(),
            2 => remote.configure(serde_json::to_value(paired_config()).unwrap()).unwrap(),
            3 => remote.disable().unwrap(),
            _ => remote.retire_for_erasure(),
        }
        assert!(remote.select_discovered_choice(&old, 2).is_err());
        if transition == 0 {
            assert_eq!(remote.snapshot()["find_message"], discovery::EXPIRED_CHOICE);
            assert!(remote.snapshot()["servers"].as_array().unwrap().is_empty());
        }
        assert!(remote.pairing.is_none());
    }
}

#[test]
fn find_empty_catalog_is_retryable_and_does_not_fallback() {
    let body = br#"{"version":1,"servers":[]}"#.to_vec();
    let mut remote = configured(&body);
    remote.queue_find(0,false).unwrap();
    finish_find(&mut remote, body);
    assert_eq!(remote.snapshot()["find_state"], "empty");
    assert!(remote.discover_address.is_none() && remote.pairing.is_none() && remote.pending_save.is_none());
    remote.queue_find(20,false).unwrap();
    assert!(matches!(remote.discover_address, Some(DiscoveryTarget::CatalogUrl(_))));
}

#[test]
fn find_needs_persisted_identity_and_valid_profile_without_disturbing_pairing() {
    let body = document();
    let mut remote = configured(&body);
    remote.persisted = false;
    assert!(remote.queue_find(0,false).is_err());
    assert!(remote.discover_address.is_none());
    remote.persisted = true;
    remote.begin_pairing("https://chosen.invalid", "chosen", 1).unwrap();
    remote.catalog_profile = Err("Discovery configuration is invalid. Enter a server address.");
    assert!(remote.queue_find(2,false).is_err());
    assert_eq!(remote.pairing.as_ref().unwrap().base, "https://chosen.invalid:443");
    assert_eq!(remote.snapshot()["find_configured"], true, "Find remains available independently of optional catalog validity");
}

#[test]
fn selected_approval_requires_protected_save_before_replacing_saved_connection() {
    for saved in [false, true] {
        let body = document();
        let mut remote = configured(&body);
        remote.apply(paired_config(), true);
        let original = serde_json::to_value(&remote.config).unwrap();
        remote.queue_find(0,false).unwrap();
        finish_find(&mut remote, body);
        let choice = remote.snapshot()["servers"][0]["choice"].as_str().unwrap().to_owned();
        remote.select_discovered_choice(&choice, 20).unwrap();
        remote.finish(Kind::Pairing, Outcome::Http { response: Ok(protocol::Response { status:201,
            body:serde_json::to_vec(&json!({"id":"0123456789abcdef0123456789abcdef",
                "secret":"B".repeat(32),"code":"ABCD1234","expires_in":60})).unwrap() }),elapsed_ms:1 },21);
        remote.finish(Kind::Pairing, response(serde_json::to_vec(&json!({"status":"approved",
            "device_id":"dev_test","address":"https://control.invalid/team","token":"C".repeat(32)})).unwrap()),22);
        assert_eq!(serde_json::to_value(&remote.config).unwrap(), original);
        assert!(!remote.snapshot().to_string().contains(&"B".repeat(32)));
        assert!(!remote.snapshot().to_string().contains(&"C".repeat(32)));
        let candidate = remote.pending_save.take().expect("approved config waits for writer");
        remote.finish(Kind::Save(candidate), Outcome::Saved(if saved { Ok(()) } else { Err("Protected control storage is unavailable") }),23);
        if saved { assert_eq!(remote.config.address, "https://control.invalid:443/team"); }
        else { assert_eq!(serde_json::to_value(&remote.config).unwrap(), original); }
    }
}

#[cfg(feature = "hostsim")]
#[test]
fn canceled_catalog_completion_cannot_resurrect_choice_or_launch_another_worker() {
    let body = document();
    let mut remote = configured(&body);
    remote.queue_find(0,false).unwrap();
    let Some(DiscoveryTarget::CatalogUrl(profile)) = remote.discover_address.take() else { panic!("catalog") };
    let (task, sender) = transport::Task::pending_for_test();
    remote.flight = Some(Flight { kind:Kind::Catalog(profile),generation:remote.generation,task });
    remote.queue_find(1,false).unwrap();
    assert!(remote.tick(2,json!({})).is_empty());
    assert!(remote.flight.is_some(), "canceled worker retains the single slot until completion");
    remote.cancel_pairing();
    sender.send(response(body)).unwrap_or_else(|_| panic!("held receiver"));
    assert!(remote.tick(3,json!({})).is_empty());
    assert!(remote.flight.is_none() && remote.discover_address.is_none());
    assert_eq!(remote.snapshot()["find_message"], "");
    assert!(remote.snapshot()["servers"].as_array().unwrap().is_empty());
}

#[test]
fn unavailable_profile_and_refused_worker_never_start_fallback_work() {
    let body = document();
    let mut remote = configured(&body);
    remote.catalog_profile = Ok(None);
    assert!(remote.queue_find(0,false).is_err());
    assert!(remote.tick(1,json!({})).is_empty());
    assert!(remote.flight.is_none() && remote.discover_address.is_none());
    remote.catalog_profile = Ok(Some(profile(&body)));
    remote.queue_find(2,false).unwrap();
    let Some(DiscoveryTarget::CatalogUrl(profile)) = remote.discover_address.take() else { panic!("catalog") };
    remote.launch(Kind::Catalog(profile),None,3);
    assert_eq!(remote.snapshot()["find_state"],"error");
    assert!(remote.tick(4,json!({})).is_empty());
    assert!(remote.flight.is_none() && remote.discover_address.is_none());
}

#[test]
fn manual_singleton_ignores_optional_domain_validation_when_pairing() {
    for domain in ["Living room".to_owned(), "Living\nroom".to_owned(), "x".repeat(254)] {
        let body = serde_json::to_vec(&json!({"version":1,"servers":[{
            "id":"one","domain":domain,"address":"https://control.invalid/team"
        }]})).unwrap();
        let mut remote = Remote::disabled_for_test("dev_test", "runtime");
        remote.discover_and_pair("https://control.invalid/team", 0).unwrap();
        assert!(matches!(remote.discover_address.take(), Some(DiscoveryTarget::ControllerBase(_))));
        remote.finish(Kind::Discovery, response(body), 1);
        let request = remote.pairing.as_ref()
            .expect("manual discovery must not reject an optional domain string").request(false);
        assert_eq!(request.url, "https://control.invalid:443/team/api/pairings");
        assert_eq!(serde_json::from_str::<Value>(request.body.as_deref().unwrap()).unwrap(),
            json!({"device_id":"dev_test","server_id":"one"}));
        assert!(!remote.config.enabled && remote.pending_save.is_none());
    }
}

#[test]
fn catalog_requires_valid_domains_even_when_manual_discovery_accepts_them() {
    let valid = json!({"id":"one","domain":"example.invalid","address":"https://one.invalid"});
    for domain in [Some(json!("Living room")), Some(json!("Living\nroom")),
        Some(json!("x".repeat(254))), Some(Value::Null), None] {
        let mut invalid = json!({"id":"two","address":"https://two.invalid"});
        if let Some(domain) = domain { invalid["domain"] = domain; }
        let body = serde_json::to_vec(&json!({"version":1,"servers":[valid,invalid]})).unwrap();
        assert_eq!(profile(&body).parse(&body).unwrap_err(), "Invalid discovery catalog");
    }
}

#[test]
fn catalog_accepts_a_valid_domain_at_the_existing_length_boundary() {
    let domain = "x".repeat(253);
    let body = serde_json::to_vec(&json!({"version":1,"servers":[{
        "id":"one","domain":domain,"address":"https://control.invalid/team"
    }]})).unwrap();
    let candidates = profile(&body).parse(&body).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].domain, domain);
    assert_eq!(candidates[0].id, "one");
    assert_eq!(candidates[0].address, "https://control.invalid:443/team");
}


#[test]
fn manual_singleton_ignores_non_string_optional_domains() {
    for domain in [json!(17), json!(true), json!(["legacy"]), json!({"name":"legacy"}), Value::Null] {
        let body = serde_json::to_vec(&json!({"version":1,"servers":[{
            "id":"one","domain":domain,"address":"https://control.invalid/team"
        }]})).unwrap();
        let mut remote = Remote::disabled_for_test("dev_test", "runtime");
        remote.discover_and_pair("https://control.invalid/team", 0).unwrap();
        assert!(matches!(remote.discover_address.take(), Some(DiscoveryTarget::ControllerBase(_))));
        remote.finish(Kind::Discovery, response(body), 1);
        let request = remote.pairing.as_ref()
            .expect("manual discovery must ignore a non-string optional domain").request(false);
        assert_eq!(request.url, "https://control.invalid:443/team/api/pairings");
        assert_eq!(serde_json::from_str::<Value>(request.body.as_deref().unwrap()).unwrap(),
            json!({"device_id":"dev_test","server_id":"one"}));
        assert!(!remote.config.enabled && remote.pending_save.is_none());
    }
}

#[test]
fn catalog_rejects_non_string_domains_atomically() {
    let valid = json!({"id":"one","domain":"example.invalid","address":"https://one.invalid"});
    for domain in [json!(17), json!(true), json!(["legacy"]), json!({"name":"legacy"}), Value::Null] {
        let body = serde_json::to_vec(&json!({"version":1,"servers":[valid,{
            "id":"two","domain":domain,"address":"https://two.invalid"
        }]})).unwrap();
        assert_eq!(profile(&body).parse(&body).unwrap_err(), "Invalid discovery catalog");
    }
}

#[test]
fn manual_discovery_retains_identity_and_endpoint_validation() {
    let valid = json!({"id":"one","domain":null,"address":"https://one.invalid"});
    let baseline = serde_json::to_vec(&json!({"version":1,"servers":[valid]})).unwrap();
    assert_eq!(discovery::parse_controller(&baseline).unwrap().len(), 1);
    for (id, address) in [
        ("", "https://two.invalid"),
        ("one", "https://two.invalid"),
        ("bad id", "https://two.invalid"),
        ("two", "http://two.invalid"),
        ("two", "https://user:password@two.invalid"),
        ("two", "https://two.invalid/path?token=synthetic"),
        ("two", "https://two.invalid/../path"),
    ] {
        let body = serde_json::to_vec(&json!({"version":1,"servers":[valid,{
            "id":id,"domain":null,"address":address
        }]})).unwrap();
        assert!(discovery::parse_controller(&body).is_err(), "{id:?} {address}");
    }
}
