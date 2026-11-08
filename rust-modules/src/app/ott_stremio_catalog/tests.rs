use super::*;
use serde_json::json;

fn manifest_value() -> Value {
    json!({"id":"org.example.open","name":"Open films","types":["movie"],
        "resources":["catalog",{"name":"stream","types":["movie"],"idPrefixes":["tt","open:"]}],
        "catalogs":[{"id":"open/catalog","name":"Open films","type":"movie",
            "extra":[{"name":"search"},{"name":"skip"}]}]})
}
fn parse(value: Value) -> Result<Manifest, Error> {
    Manifest::parse(&serde_json::to_vec(&value).unwrap())
}
fn manifest() -> Manifest {
    parse(manifest_value()).unwrap()
}
fn page(request: &CatalogRequest, rows: Value) -> Result<Page, Error> {
    request.decode_page(&serde_json::to_vec(&json!({"metas":rows})).unwrap())
}
fn movie(id: &str) -> Value {
    json!({"id":id,"type":"movie","name":"Open film"})
}

#[test]
fn manifest_admission_exposes_movie_capabilities_without_io() {
    let manifest = manifest();
    assert_eq!(manifest.id, "org.example.open");
    assert_eq!(manifest.name, "Open films");
    assert!(manifest.has_movie_streams());
    assert_eq!(manifest.catalogs().len(), 1);
    let catalog = &manifest.catalogs()[0];
    assert_eq!(catalog.id, "open/catalog");
    assert_eq!(catalog.title, "Open films");
    assert!(catalog.can_browse && catalog.can_search && catalog.can_page);
}

#[test]
fn configuration_required_remains_a_distinct_actionable_error() {
    let mut value = manifest_value();
    value["behaviorHints"] = json!({"configurationRequired":true});
    assert_eq!(parse(value).err(), Some(Error::ConfigurationRequired));
}

#[test]
fn malformed_and_oversized_manifest_documents_are_rejected() {
    for field in ["id", "name", "types", "resources", "catalogs"] {
        let mut value = manifest_value();
        value.as_object_mut().unwrap().remove(field);
        assert_eq!(parse(value).err(), Some(Error::InvalidManifest), "{field}");
    }
    assert_eq!(
        Manifest::parse(&vec![b' '; 65_537]).err(),
        Some(Error::ResponseLimit)
    );
    let mut value = manifest_value();
    value["catalogs"] = json!(vec![value["catalogs"][0].clone(); 101]);
    assert_eq!(parse(value).err(), Some(Error::InvalidManifest));
    let mut value = manifest_value();
    value["catalogs"][0]["id"] = json!(" padded ");
    assert_eq!(parse(value).err(), Some(Error::InvalidManifest));
    let mut value = manifest_value();
    value["resources"][1]["idPrefixes"] = Value::Null;
    assert_eq!(parse(value).err(), Some(Error::InvalidManifest));
}

#[test]
fn duplicate_catalog_ids_cannot_select_an_ambiguous_definition() {
    let mut value = manifest_value();
    let first = value["catalogs"][0].clone();
    value["catalogs"].as_array_mut().unwrap().push(first);
    assert_eq!(parse(value).err(), Some(Error::InvalidManifest));
}

#[test]
fn search_and_skip_are_encoded_as_stremio_path_extras() {
    let request = manifest()
        .catalog_request("open/catalog", Some("A & B/猫"), 50)
        .unwrap();
    assert_eq!(
        request.relative_path(),
        "catalog/movie/open%2Fcatalog/search=A%20%26%20B%2F%E7%8C%AB&skip=50.json"
    );
    assert_eq!(request.catalog_id, "open/catalog");
    assert_eq!(
        manifest()
            .catalog_request("open/catalog", None, 0)
            .unwrap()
            .relative_path(),
        "catalog/movie/open%2Fcatalog/skip=0.json"
    );
    assert_eq!(
        manifest()
            .catalog_request("open/catalog", Some(""), 0)
            .err(),
        Some(Error::InvalidRequest)
    );
}

#[test]
fn required_search_and_unknown_required_extras_never_make_invalid_requests() {
    let mut value = manifest_value();
    value["catalogs"][0]["extra"] = json!([{"name":"search","isRequired":true}]);
    let manifest = parse(value.clone()).unwrap();
    assert_eq!(
        manifest.catalog_request("open/catalog", None, 0).err(),
        Some(Error::SearchRequired)
    );
    assert!(manifest
        .catalog_request("open/catalog", Some("open"), 0)
        .is_ok());
    assert_eq!(
        manifest
            .catalog_request("open/catalog", Some("open"), 1)
            .err(),
        Some(Error::InvalidRequest)
    );
    value["catalogs"][0]["extra"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"genre","isRequired":true}));
    let manifest = parse(value).unwrap();
    assert_eq!(
        manifest
            .catalog_request("open/catalog", Some("open"), 0)
            .err(),
        Some(Error::Unsupported)
    );
}

#[test]
fn legacy_extra_supported_and_required_work_like_modern_extras() {
    let mut value = manifest_value();
    value["catalogs"][0]
        .as_object_mut()
        .unwrap()
        .remove("extra");
    value["catalogs"][0]["extraSupported"] = json!(["search", "skip"]);
    value["catalogs"][0]["extraRequired"] = json!(["search"]);
    let manifest = parse(value.clone()).unwrap();
    assert_eq!(
        manifest.catalog_request("open/catalog", None, 0).err(),
        Some(Error::SearchRequired)
    );
    assert!(manifest
        .catalog_request("open/catalog", Some("film"), 10)
        .is_ok());
    value["catalogs"][0]["extraRequired"] = json!(["unknown"]);
    assert_eq!(parse(value).err(), Some(Error::InvalidManifest));
}

#[test]
fn non_movie_catalogs_are_not_promoted_to_supported_sources() {
    let mut value = manifest_value();
    value["catalogs"][0]["type"] = json!("series");
    let manifest = parse(value).unwrap();
    assert!(manifest.catalogs().is_empty());
    assert_eq!(
        manifest.catalog_request("open/catalog", None, 0).err(),
        Some(Error::Unsupported)
    );
}

#[test]
fn unpaged_catalogs_never_guess_a_next_request() {
    let mut value = manifest_value();
    value["catalogs"][0]["extra"] = json!([]);
    let manifest = parse(value).unwrap();
    let request = manifest.catalog_request("open/catalog", None, 0).unwrap();
    assert_eq!(request.relative_path(), "catalog/movie/open%2Fcatalog.json");
    assert_eq!(
        page(&request, json!([movie("open:1")]))
            .unwrap()
            .next_offset,
        None
    );
    assert_eq!(
        manifest
            .catalog_request("open/catalog", Some("film"), 0)
            .err(),
        Some(Error::Unsupported)
    );
    assert_eq!(
        manifest.catalog_request("open/catalog", None, 1).err(),
        Some(Error::InvalidRequest)
    );
}

#[test]
fn page_keeps_complete_metadata_as_non_playable_proposals() {
    let request = manifest().catalog_request("open/catalog", None, 0).unwrap();
    let page = page(
        &request,
        json!([{"id":"open:1","name":"Open film","description":"First sentence.\nSecond sentence.",
        "poster":"https://images.example/poster.jpg","releaseInfo":"2026"}]),
    )
    .unwrap();
    let movie = &page.movies[0];
    assert_eq!(movie.id, "open:1");
    assert_eq!(movie.title, "Open film");
    assert_eq!(movie.description, "First sentence.\nSecond sentence.");
    assert_eq!(
        movie.artwork.as_ref().unwrap().candidate(),
        "https://images.example/poster.jpg"
    );
    assert_eq!(movie.release_info.as_deref(), Some("2026"));
}

#[test]
fn paging_uses_raw_count_and_preserves_first_seen_movie_order() {
    let request = manifest()
        .catalog_request("open/catalog", None, 50)
        .unwrap();
    let page=page(&request,json!([movie("open:1"),movie("open:1"),{"id":"s:1","type":"series","name":"Series"},movie("open:2")])).unwrap();
    assert_eq!(page.received_count, 4);
    assert_eq!(page.next_offset, Some(54));
    assert_eq!(
        page.movies
            .iter()
            .map(|movie| movie.id.as_str())
            .collect::<Vec<_>>(),
        ["open:1", "open:2"]
    );
}

#[test]
fn empty_pages_stop_and_catalog_limit_is_respected() {
    let request = manifest().catalog_request("open/catalog", None, 0).unwrap();
    assert_eq!(page(&request, json!([])).unwrap().next_offset, None);
    let request = manifest()
        .catalog_request("open/catalog", None, plx_data::ott::MAX_CATALOG_ENTRIES)
        .unwrap();
    assert_eq!(
        page(&request, json!([movie("open:1")]))
            .unwrap()
            .next_offset,
        None
    );
    assert_eq!(
        manifest()
            .catalog_request("open/catalog", None, plx_data::ott::MAX_CATALOG_ENTRIES + 1)
            .err(),
        Some(Error::InvalidRequest)
    );
}

#[test]
fn page_response_limits_and_required_movie_identity_are_enforced() {
    let request = manifest().catalog_request("open/catalog", None, 0).unwrap();
    assert_eq!(
        request.decode_page(&vec![b' '; 524_289]).err(),
        Some(Error::ResponseLimit)
    );
    assert_eq!(
        page(&request, json!(vec![movie("open:1"); 1001])).err(),
        Some(Error::InvalidPage)
    );
    assert_eq!(
        page(&request, json!([{"name":"No identity"}])).err(),
        Some(Error::InvalidPage)
    );
    assert_eq!(
        page(&request, json!([{"id":"open:1"}])).err(),
        Some(Error::InvalidPage)
    );
}

#[test]
fn optional_movie_type_matches_catalogue_defaults_but_rejects_invalid_shapes() {
    let request = manifest().catalog_request("open/catalog", None, 0).unwrap();
    let mut value = movie("open:1");
    value["type"] = Value::Null;
    assert_eq!(
        page(&request, json!([value.clone()])).unwrap().movies.len(),
        1
    );
    value["type"] = json!({"name":"movie"});
    assert_eq!(
        page(&request, json!([value])).err(),
        Some(Error::InvalidPage)
    );
}

#[test]
fn stream_request_chooses_first_matching_candidate_without_inventing_sources() {
    let manifest = manifest();
    let request = manifest
        .stream_request(&["tmdb:123", "tt1234567", "open:film"])
        .unwrap()
        .unwrap();
    assert_eq!(request.media_id, "tt1234567");
    assert_eq!(request.relative_path(), "stream/movie/tt1234567.json");
    let request = manifest
        .stream_request(&["open:film/a?b=c"])
        .unwrap()
        .unwrap();
    assert_eq!(
        request.relative_path(),
        "stream/movie/open%3Afilm%2Fa%3Fb%3Dc.json"
    );
    assert!(manifest.stream_request(&["tmdb:123"]).unwrap().is_none());
    assert_eq!(
        manifest.stream_request(&["tt123\n"]).err(),
        Some(Error::InvalidRequest)
    );
}

#[test]
fn global_and_resource_scoped_stream_prefixes_keep_their_own_types() {
    let mut value = manifest_value();
    value["resources"] = json!(["stream"]);
    value["idPrefixes"] = json!(["tt"]);
    assert!(parse(value.clone())
        .unwrap()
        .stream_request(&["tt1234567"])
        .unwrap()
        .is_some());
    value["types"] = json!(["series"]);
    assert!(parse(value.clone())
        .unwrap()
        .stream_request(&["tt1234567"])
        .unwrap()
        .is_none());
    value["resources"] = json!([{"name":"stream","types":["movie"],"idPrefixes":["open:"]}]);
    let manifest = parse(value).unwrap();
    assert!(manifest.stream_request(&["tt1234567"]).unwrap().is_none());
    assert!(manifest.stream_request(&["open:1"]).unwrap().is_some());
}
