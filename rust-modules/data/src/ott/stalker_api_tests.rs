use super::*;
fn config() -> Config {
    Config::parse(
        r#"{"portal":"https://portal.invalid/stalker_portal/api/","mac":"00:1a:79:01:02:03"}"#,
    )
    .unwrap()
}
fn profile() -> ProviderProfile {
    ProviderProfile {
        id: "stalker-one".into(),
        name: "Portal".into(),
        kind: super::super::ProviderKind::Stalker,
        source: serde_json::to_string(&config()).unwrap(),
        source_alias: String::new(),
        enabled: true,
    }
}
#[test]
fn explicit_api_profile_and_rpc_requests_preserve_mac() {
    let c = config();
    assert_eq!(c.dialect(), Dialect::JsonRpc);
    assert!(profile().validate().is_ok());
    assert_eq!(
        request(&c, Operation::Handshake).unwrap(),
        json!({"jsonrpc":"2.0","id":1,"method":"handshake","params":{"mac":"00:1a:79:01:02:03"}})
    );
    assert_eq!(
        request(
            &c,
            Operation::Guide {
                channel: "7",
                from: 100,
                to: 200
            }
        )
        .unwrap(),
        json!({"jsonrpc":"2.0","id":1,"method":"get_epg","params":{"mac":"00:1a:79:01:02:03","ch_id":"7","from":100,"to":200}})
    );
    assert!(Config::parse(
        r#"{"portal":"https://portal.invalid/api/?secret=bad","mac":"00:1a:79:01:02:03"}"#
    )
    .is_err());
}
#[test]
fn discovery_then_fresh_resolve_never_reuses_an_old_url() {
    let c = config();
    let p = profile();
    let first = json!({"data":[{"id":7,"name":"News","genre":["World"],"url":"https://stream.invalid/first"}]});
    let cat = catalog(&p, &c, &first).unwrap();
    assert_eq!(cat.entries.len(), 1);
    assert!(
        matches!(&cat.entries[0].media, MediaLocator::StalkerApi { channel_id } if channel_id == "7")
    );
    assert_eq!(
        resolve(
            &c,
            &json!([{"id":7,"name":"News","url":"https://stream.invalid/refreshed"}]),
            "7"
        )
        .unwrap(),
        "https://stream.invalid:443/refreshed"
    );
    assert!(resolve(&c, &json!([]), "7").is_err());
    assert_eq!(
        resolve(&c, &json!([{"id":"7","name":"News"}]), "7").unwrap(),
        "https://portal.invalid:443/stalker_portal/stream/7.m3u8?mac=00%3A1a%3A79%3A01%3A02%3A03"
    );
}
#[test]
fn refused_malformed_duplicate_and_unsafe_streams_are_rejected() {
    for value in [
        json!({"result":false}),
        json!({"result":null}),
        json!({"result":[],"id":2}),
        json!({"result":[],"error":{"code":-1}}),
    ] {
        assert!(result(&value).is_err());
    }
    let c = config();
    let p = profile();
    for rows in [
        json!([{"id":7,"name":"News","url":"file:///etc/private"}]),
        json!([{"id":7,"name":"News"},{"id":7,"name":"Other"}]),
        json!([{"name":"Missing"}]),
    ] {
        assert!(catalog(&p, &c, &rows).is_err());
    }
    assert!(request(
        &c,
        Operation::Guide {
            channel: "7\n",
            from: 1,
            to: 2
        }
    )
    .is_err());
    assert!(guide("7", &json!([{"start":20,"end":10}])).is_err());
}
#[test]
fn mag_dialect_stays_separate() {
    let mag = Config::parse(r#"{"portal":"https://portal.invalid/c/","mac":"00:1a:79:01:02:03"}"#)
        .unwrap();
    assert_eq!(mag.dialect(), Dialect::Mag);
    assert_eq!(mag.mac, "00:1A:79:01:02:03");
    assert!(request(&mag, Operation::Handshake).is_err());
}

#[test]
fn handshake_accepts_attested_integer_one_without_general_truthiness() {
    assert!(handshake(&json!(1)).is_ok());
    for value in [
        json!(0),
        json!(2),
        json!(-1),
        json!(1.5),
        json!("1"),
        json!(false),
        Value::Null,
    ] {
        assert!(handshake(&value).is_err());
    }
}
