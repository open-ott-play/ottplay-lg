//! Existing-API regression for an advertised identity the pairing validator cannot accept.
use super::*;

#[test]
fn manual_discovery_rejects_unpairable_identity_atomically() {
    for length in [254, 255] {
        let mut remote = Remote::disabled_for_test("dev_test", "runtime");
        let body = serde_json::to_vec(&json!({"version":1,"servers":[
            {"id":"one","address":"https://one.invalid"},
            {"id":"x".repeat(length),"address":"https://two.invalid"}
        ]})).unwrap();
        remote.finish(Kind::Discovery, Outcome::Http {
            response:Ok(protocol::Response { status:200,body }),elapsed_ms:1,
        },0);
        let state = remote.snapshot();
        if length == 254 {
            assert_eq!(state["state"], "choose");
            assert_eq!(state["servers"].as_array().unwrap().len(), 2);
        } else {
            assert_eq!(state["state"], "error");
            assert!(state["servers"].as_array().unwrap().is_empty());
        }
        assert!(remote.pairing.is_none() && remote.pending_save.is_none());
    }
}
