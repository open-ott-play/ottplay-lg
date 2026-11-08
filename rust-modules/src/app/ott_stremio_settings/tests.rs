use super::*;

fn addon(id: &str) -> Addon {
    Addon {
        id: id.into(),
        label: format!("Library {id}"),
        manifest_url: format!("https://example.test/{id}/manifest.json"),
        enabled: true,
    }
}

fn configured() -> Config {
    Config {
        language: "en-US".into(),
        catalog_service_url: None,
        addons: vec![addon("one"), addon("two")],
    }
}

fn catalogs(ids: &[&str]) -> Manifest {
    let entries = ids
        .iter()
        .map(|id| {
            json!({
                "id": id, "type":"movie", "name":id,
                "extra":[{"name":"search"},{"name":"skip"}]
            })
        })
        .collect::<Vec<_>>();
    Manifest::parse(
        &serde_json::to_vec(&json!({
            "id":"org.example.catalog", "name":"Movie catalog", "types":["movie"],
            "resources":["catalog","stream"], "catalogs":entries
        }))
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn absent_settings_default_but_malformed_settings_do_not_erase_configuration() {
    assert_eq!(Config::parse(&Value::Null).unwrap(), Config::default());
    assert_eq!(Config::parse(&json!({})).unwrap(), Config::default());
    for value in [
        json!([]),
        json!(false),
        json!({"addons":null}),
        json!({"language":null}),
        json!({"other":true}),
    ] {
        assert!(Config::parse(&value).is_err());
    }
}

#[test]
fn private_configuration_round_trips_and_debug_redacts_urls_and_labels() {
    let mut config = configured();
    config.addons[0].manifest_url = "https://example.test/private-token/manifest.json".into();
    config.addons[0].label = "Private library label".into();
    assert_eq!(Config::parse(&config.to_value()).unwrap(), config);
    let debug = format!("{config:?} {:?}", config.addons[0]);
    for private in ["example.test", "private-token", "Private library label"] {
        assert!(!debug.contains(private));
    }
}

#[test]
fn exact_configured_prefix_including_escaped_token_bytes_is_preserved() {
    let mut value = addon("one");
    value.manifest_url =
        "https://Example.test:443/%7B%22key%22%3A%22a%2Fb%22%7D/manifest.json".into();
    value.validate().unwrap();
    assert_eq!(value.resource_url("catalog/movie/popular/search=hello%20world&skip=0.json").unwrap(),
        "https://Example.test:443/%7B%22key%22%3A%22a%2Fb%22%7D/catalog/movie/popular/search=hello%20world&skip=0.json");
}

#[test]
fn addon_metadata_allows_only_https_or_explicit_loopback_http() {
    for url in [
        "https://example.test/manifest.json",
        "http://127.0.0.1:9000/manifest.json",
        "http://localhost:9000/prefix/manifest.json",
        "http://[::1]:9000/manifest.json",
    ] {
        let mut value = addon("one");
        value.manifest_url = url.into();
        value.validate().unwrap();
    }
    for url in [
        "http://example.test/manifest.json",
        "http://192.168.1.2/manifest.json",
        "http://127.1/manifest.json",
        "http://localhost.example.test/manifest.json",
        "ftp://example.test/manifest.json",
    ] {
        let mut value = addon("one");
        value.manifest_url = url.into();
        assert!(value.validate().is_err(), "{url}");
    }
}

#[test]
fn manifests_reject_queries_fragments_authority_credentials_and_encoded_controls() {
    for url in [
        "https://example.test/manifest.json?token=one",
        "https://example.test/manifest.json#part",
        "https://user:password@example.test/manifest.json",
        "https://example.test/%0a/manifest.json",
        "https://example.test/%5c/manifest.json",
        "https://example.test/%ff/manifest.json",
        "https://example.test/%/manifest.json",
        " https://example.test/manifest.json",
        "https://example.test/manifest.json/",
        "https://example.test/Manifest.json",
    ] {
        let mut value = addon("one");
        value.manifest_url = url.into();
        assert!(value.validate().is_err(), "{url}");
    }
}

#[test]
fn resource_path_cannot_replace_authority_or_escape_the_configured_prefix() {
    let value = addon("one");
    for path in [
        "",
        "/stream/movie/tt1.json",
        "//other.test/stream.json",
        "https://other.test/stream.json",
        "../stream.json",
        "catalog/../stream.json",
        "catalog/%2e%2e/stream.json",
        "catalog/movie%2f..%2fstream.json",
        "stream.json?host=other",
        "stream.json#part",
    ] {
        assert!(value.resource_url(path).is_err(), "{path}");
    }
    assert_eq!(
        value.resource_url("stream/movie/tt123.json").unwrap(),
        "https://example.test/one/stream/movie/tt123.json"
    );
    assert_eq!(
        value.resource_url(&"a".repeat(8193)),
        Err(Error::InvalidResource)
    );
}

#[test]
fn real_catalog_plan_preserves_encoded_ids_and_search_text() {
    let manifest = catalogs(&["open/catalog"]);
    let request = manifest
        .catalog_request("open/catalog", Some("A & B/猫"), 50)
        .unwrap();
    let value = addon("one");
    assert_eq!(value.resource_url(request.relative_path()).unwrap(),
        "https://example.test/one/catalog/movie/open%2Fcatalog/search=A%20%26%20B%2F%E7%8C%AB&skip=50.json");
}

#[test]
fn known_unconfigured_tmdb_catalog_applies_classic_language_path_only() {
    let manifest = catalogs(&["tmdb.top", "tmdb.latest", "tmdb.search"]);
    let mut value = addon("tmdb");
    value.manifest_url = "https://tmdb.elfhosted.com/manifest.json".into();
    assert!(value.catalog_language_supported(&manifest));
    let request = manifest.catalog_request("tmdb.top", None, 0).unwrap();
    assert_eq!(value.catalog_resource_url(&manifest, request.relative_path(), "FR-fr").unwrap(),
        "https://tmdb.elfhosted.com/%7B%22language%22%3A%22fr-FR%22%7D/catalog/movie/tmdb.top/skip=0.json");
    assert_eq!(
        value.resource_url("stream/movie/tt123.json").unwrap(),
        "https://tmdb.elfhosted.com/stream/movie/tt123.json"
    );
    assert_eq!(
        value.catalog_resource_url(&manifest, "stream/movie/tt123.json", "en-US"),
        Err(Error::InvalidResource)
    );
}

#[test]
fn unrelated_configured_or_partial_tmdb_catalogs_never_gain_a_language_prefix() {
    let manifest = catalogs(&["tmdb.top", "tmdb.latest", "tmdb.search"]);
    let request = manifest.catalog_request("tmdb.top", None, 0).unwrap();
    for url in [
        "https://example.test/manifest.json",
        "https://tmdb.elfhosted.com:443/manifest.json",
        "https://tmdb.elfhosted.com/private-config/manifest.json",
        "https://tmdb.elfhosted.com.example.test/manifest.json",
    ] {
        let mut value = addon("one");
        value.manifest_url = url.into();
        assert!(!value.catalog_language_supported(&manifest), "{url}");
        assert_eq!(
            value.catalog_resource_url(&manifest, request.relative_path(), "de-DE"),
            value.resource_url(request.relative_path())
        );
    }
    let partial = catalogs(&["tmdb.top", "tmdb.latest"]);
    let mut value = addon("one");
    value.manifest_url = "https://tmdb.elfhosted.com/manifest.json".into();
    assert!(!value.catalog_language_supported(&partial));
    assert_eq!(
        value.catalog_resource_url(&partial, request.relative_path(), "de-DE"),
        value.resource_url(request.relative_path())
    );
}

#[test]
fn at_most_eight_entries_including_disabled_entries_can_be_saved() {
    let mut config = Config {
        language: "en".into(),
        catalog_service_url: None,
        addons: (0..8).map(|n| addon(&n.to_string())).collect(),
    };
    config.addons[0].enabled = false;
    Config::parse(&config.to_value()).unwrap();
    config.addons.push(addon("nine"));
    assert_eq!(Config::parse(&config.to_value()).unwrap_err(), Error::Limit);
}

#[test]
fn the_combined_encoded_url_list_has_the_classic_byte_limit() {
    let mut config = configured();
    config.addons[0].manifest_url =
        format!("https://example.test/{}/manifest.json", "a".repeat(2100));
    config.addons[1].manifest_url =
        format!("https://example.test/{}/manifest.json", "b".repeat(2100));
    assert!(config.addons.iter().all(|a| a.validate().is_ok()));
    assert_eq!(config.validate(), Err(Error::Limit));
}

#[test]
fn url_list_storage_accepts_exactly_4096_encoded_bytes() {
    let mut value = addon("one");
    let prefix = "https://example.test/";
    let suffix = "/manifest.json";
    value.manifest_url = format!(
        "{prefix}{}{suffix}",
        "x".repeat(4092 - prefix.len() - suffix.len())
    );
    let mut config = Config {
        language: "en-US".into(),
        catalog_service_url: None,
        addons: vec![value],
    };
    assert_eq!(
        serde_json::to_vec(&vec![&config.addons[0].manifest_url])
            .unwrap()
            .len(),
        4096
    );
    assert_eq!(config.validate(), Ok(()));
    config.addons[0].manifest_url.insert(prefix.len(), 'x');
    assert_eq!(config.validate(), Err(Error::Limit));
}

#[test]
fn duplicate_ids_or_exact_urls_fail_instead_of_creating_ambiguous_rows() {
    let mut config = configured();
    config.addons[1].id = config.addons[0].id.clone();
    assert_eq!(config.validate(), Err(Error::InvalidConfiguration));
    config = configured();
    config.addons[1].manifest_url = config.addons[0].manifest_url.clone();
    assert_eq!(config.validate(), Err(Error::InvalidConfiguration));
}

#[test]
fn addon_record_requires_an_exact_bounded_shape() {
    let valid = configured().to_value();
    for (field, replacement) in [
        ("id", json!("../one")),
        ("id", json!("a".repeat(65))),
        ("label", json!("")),
        ("label", json!("a".repeat(257))),
        ("enabled", json!(null)),
    ] {
        let mut value = valid.clone();
        value["addons"][0][field] = replacement;
        assert!(Config::parse(&value).is_err());
    }
    let mut value = valid;
    value["addons"][0]["unexpected"] = json!(true);
    assert!(Config::parse(&value).is_err());
}

#[test]
fn language_is_canonicalized_without_inventing_addon_url_parameters() {
    for (input, expected) in [
        ("EN-us", "en-US"),
        ("zh-hant-tw", "zh-Hant-TW"),
        ("es-419", "es-419"),
        ("eng", "eng"),
    ] {
        let config = Config::parse(&json!({"language":input})).unwrap();
        assert_eq!(config.language, expected);
    }
    for value in [
        "", "english", "en_US", "en--US", "en-", "en\n", "éñ", "en-x-a",
    ] {
        assert_eq!(language(value), Err(Error::InvalidLanguage));
    }
    let mut config = configured();
    config.language = "fr-FR".into();
    assert_eq!(
        config.addons[0]
            .resource_url("stream/movie/tt1.json")
            .unwrap(),
        "https://example.test/one/stream/movie/tt1.json"
    );
}

#[test]
fn explicit_catalog_service_roundtrips_privately_and_legacy_settings_stay_unchanged() {
    let legacy = configured();
    assert!(legacy.to_value().get("catalogServiceUrl").is_none());
    let mut value = legacy.to_value();
    value["catalogServiceUrl"] = json!("https://catalog.example/private-prefix/tmdb/s/");
    let config = Config::parse(&value).unwrap();
    assert_eq!(config.catalog_service_url.as_deref(),Some("https://catalog.example/private-prefix/tmdb/s/"));
    assert_eq!(config.to_value(),value);
    assert!(!format!("{config:?}").contains("private-prefix"));
    assert_eq!(config.discovery_url("movie/popular?language=en-US&page=1").unwrap(),
        "https://catalog.example/private-prefix/tmdb/s/movie/popular?language=en-US&page=1");
    assert!(legacy.discovery_url("movie/popular?language=en-US").is_err());
}
#[test]
fn catalog_service_rejects_keys_wrong_routes_and_authority_escape() {
    for url in ["https://catalog.example/tmdb/s/?api_key=secret", "https://catalog.example/tmdb/s/#fragment",
        "https://catalog.example/other/", "https://catalog.example/../tmdb/s/", "https://catalog.example/%2e%2e/tmdb/s/"] {
        let mut value = configured().to_value();
        value["catalogServiceUrl"] = json!(url);
        assert!(Config::parse(&value).is_err());
    }
    let mut config = configured();
    config.catalog_service_url = Some("http://127.0.0.1:8080/tmdb/s/".into());
    config.validate().unwrap();
    for path in ["/movie/popular", "https://other.example/movie/popular", "../command", "%2e%2e/command", "movie/42#fragment"] {
        assert!(config.discovery_url(path).is_err());
    }
    let mut value = config.to_value();
    value["catalogServiceUrl"] = Value::Null;
    assert!(Config::parse(&value).is_err());
}
