//! Pure completed/pending worker fixtures; no actual resolver or storage IO.
use super::*;
use plx_net::control_discovery::{ControlServer, Error};
fn home() -> ControlServer { ControlServer { id:"home._ottplay-ctrl._tcp.lan.example.".into(),
    domain:"lan.example".into(),address:"https://control.example/ott-control".into() } }
fn start(remote: &mut Remote) {
    remote.queue_find(10,true).unwrap();
    remote.lan_pending=false;
}
#[test]
fn local_find_is_available_without_a_catalog_and_never_auto_pairs() {
    let mut remote=Remote::disabled_for_test("dev_test","runtime");
    remote.catalog_profile=Ok(None);
    assert_eq!(remote.snapshot()["find_configured"],true);
    start(&mut remote);
    remote.finish(Kind::LanDiscovery,Outcome::LanDiscovery(Ok(vec![home()])),20);
    assert_eq!(remote.snapshot()["find_state"],"choose");
    assert!(remote.pairing.is_none() && remote.pending_save.is_none() && !remote.config.enabled);
    let choice=remote.snapshot()["servers"][0]["choice"].as_str().unwrap().to_owned();
    remote.select_discovered_choice(&choice,21).unwrap();
    assert!(remote.pairing.is_some());assert!(!remote.config.enabled);
}
#[test]
fn malformed_local_answers_never_fall_back_to_a_catalog_or_replace_saved_config() {
    for error in [Error::Malformed,Error::Unavailable,Error::TimedOut,Error::Unsupported] {
        let mut remote=Remote::disabled_for_test("dev_test","runtime");
        let config=Config::parse(json!({"enabled":true,"address":"https://saved.example/control",
            "token":"A".repeat(32),"device_id":"dev_test"})).unwrap();
        remote.apply(config,true);
        let original=serde_json::to_value(&remote.config).unwrap();
        remote.catalog_profile=CatalogProfile::from_config(Some("https://catalog.example/file.json"),Some(&"a".repeat(64)));
        start(&mut remote);
        remote.finish(Kind::LanDiscovery,Outcome::LanDiscovery(Err(error)),20);
        assert_eq!(remote.snapshot()["find_state"],"error");
        assert!(remote.discover_address.is_none() && remote.pending_save.is_none() && remote.pairing.is_none());
        assert_eq!(serde_json::to_value(&remote.config).unwrap(),original);
    }
}
#[test]
fn only_successful_empty_local_result_queues_the_pinned_catalog() {
    let mut remote=Remote::disabled_for_test("dev_test","runtime");
    remote.catalog_profile=CatalogProfile::from_config(Some("https://catalog.example/file.json"),Some(&"a".repeat(64)));
    start(&mut remote);
    remote.finish(Kind::LanDiscovery,Outcome::LanDiscovery(Ok(vec![])),20);
    assert!(matches!(remote.discover_address,Some(DiscoveryTarget::CatalogUrl(_))));
    assert!(remote.pending_save.is_none() && remote.pairing.is_none());
}
#[test]
fn ui_timeout_holds_the_single_worker_slot_and_discards_late_results() {
    let mut remote=Remote::disabled_for_test("dev_test","runtime");start(&mut remote);
    let (task,sender)=transport::Task::pending_for_test();
    remote.flight=Some(Flight {kind:Kind::LanDiscovery,generation:remote.generation,task});
    remote.tick(3010,json!({}));
    assert_eq!(remote.snapshot()["find_state"],"error");
    assert!(remote.flight.is_some());
    assert!(remote.queue_find(3011,true).is_err());
    assert!(sender.send(Outcome::LanDiscovery(Ok(vec![home()]))).is_ok());
    remote.tick(3012,json!({}));
    assert!(remote.flight.is_none());
    assert!(remote.snapshot()["servers"].as_array().unwrap().is_empty());
    assert!(remote.pairing.is_none() && remote.pending_save.is_none());
    remote.queue_find(3013,true).unwrap();assert!(remote.lan_pending);
}
#[test]
fn cancel_or_settings_change_retires_local_lookup_without_releasing_its_slot() {
    let mut remote=Remote::disabled_for_test("dev_test","runtime");start(&mut remote);
    let (task,sender)=transport::Task::pending_for_test();
    remote.flight=Some(Flight {kind:Kind::LanDiscovery,generation:remote.generation,task});
    remote.cancel_pairing();
    assert!(remote.flight.is_some());
    assert!(sender.send(Outcome::LanDiscovery(Ok(vec![home()]))).is_ok());
    remote.tick(20,json!({}));
    assert!(remote.flight.is_none() && remote.discover_address.is_none());
    assert!(remote.snapshot()["servers"].as_array().unwrap().is_empty());
}
#[test]
fn pending_poll_cannot_extend_the_three_second_find_ui_deadline() {
    let mut remote=Remote::disabled_for_test("dev_test","runtime");
    let (task,_sender)=transport::Task::pending_for_test();
    remote.flight=Some(Flight {kind:Kind::Discovery,generation:remote.generation,task});
    remote.queue_find(10,true).unwrap();remote.tick(3010,json!({}));
    assert_eq!(remote.snapshot()["find_state"],"error");
    assert!(remote.flight.is_some() && !remote.lan_pending);
}
#[test]
fn protected_bootstrap_failure_still_blocks_find() {
    let mut remote=Remote::disabled_for_test("dev_test","runtime");remote.persisted=false;
    assert_eq!(remote.snapshot()["find_configured"],false);
    assert!(remote.queue_find(0,true).is_err());assert!(!remote.lan_pending);
    remote.persisted=true;remote.write_permit.revoke();
    assert!(remote.queue_find(0,true).is_err());assert!(!remote.lan_pending);
}
