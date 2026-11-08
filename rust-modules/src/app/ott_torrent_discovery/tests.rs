use super::*;
use serde_json::json;

fn bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec(&value).unwrap()
}

fn movie_value(id: Value) -> Value {
    json!({"id":id,"title":"Localized film","original_title":"Original film",
        "overview":"First paragraph.\nSecond paragraph.","poster_path":"/poster.jpg","release_date":"2026-11-03"})
}

fn page(rows: Vec<Value>, current: u16, total: Value) -> Vec<u8> {
    bytes(json!({"page":current,"total_pages":total,"results":rows}))
}

fn request(page: u16) -> PageRequest {
    PageRequest::new("popular", None, page, "en-US").unwrap()
}

#[test]
fn categories_and_lists_use_relative_paths_and_canonical_language() {
    assert_eq!(
        CategoriesRequest::new("RU-ru").unwrap().relative_path(),
        "genre/movie/list?language=ru-RU"
    );
    assert_eq!(
        PageRequest::new("popular", None, 1, "ru-RU")
            .unwrap()
            .relative_path(),
        "movie/popular?language=ru-RU&page=1&include_adult=false"
    );
    assert_eq!(
        PageRequest::new("new", None, 2, "zh-hant-tw")
            .unwrap()
            .relative_path(),
        "movie/now_playing?language=zh-Hant-TW&page=2&include_adult=false"
    );
}

#[test]
fn genre_browsing_does_not_filter_original_language() {
    let request = PageRequest::new("genre:28", None, 1, "de-DE").unwrap();
    assert_eq!(
        request.relative_path(),
        "discover/movie?language=de-DE&page=1&include_adult=false&with_genres=28"
    );
    assert!(!request.relative_path().contains("with_original_language"));
}

#[test]
fn search_encodes_text_as_one_query_value() {
    let request = PageRequest::new("search", Some(" Cat & Dog/猫 "), 3, "en-US").unwrap();
    assert_eq!(
        request.relative_path(),
        "search/movie?language=en-US&page=3&include_adult=false&query=Cat%20%26%20Dog%2F%E7%8C%AB"
    );
    assert_eq!(
        PageRequest::new("genre:16", Some("Cat"), 1, "en")
            .unwrap()
            .relative_path(),
        "search/movie?language=en&page=1&include_adult=false&query=Cat&with_genres=16"
    );
}

#[test]
fn request_validation_rejects_invalid_pages_categories_queries_and_language() {
    for page in [0, 501, u16::MAX] {
        assert!(PageRequest::new("popular", None, page, "en").is_err());
    }
    for category in [
        "",
        "unknown",
        "search",
        "genre:0",
        "genre:01",
        "genre:1&host=bad",
        "genre:9999999999999999",
    ] {
        assert!(
            PageRequest::new(category, None, 1, "en").is_err(),
            "{category}"
        );
    }
    for query in [
        "".to_owned(),
        " \t ".to_owned(),
        "a".repeat(257),
        "x\0y".to_owned(),
    ] {
        assert!(PageRequest::new("search", Some(&query), 1, "en").is_err());
    }
    assert!(CategoriesRequest::new("en?api_key=bad").is_err());
    assert!(DetailsRequest::new("tmdb:movie:1", "").is_err());
}

#[test]
fn categories_are_localized_deduplicated_and_keep_provider_order() {
    let result = CategoriesRequest::new("ru")
        .unwrap()
        .decode(
            200,
            &bytes(json!({"genres":[
                {"id":28,"name":"Боевик"},{"id":16,"name":"Анимация"},{"id":28,"name":"Duplicate"}
            ]})),
        )
        .unwrap();
    assert_eq!(
        result
            .iter()
            .map(|row| (row.id.as_str(), row.title.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("popular", "Popular"),
            ("new", "New"),
            ("genre:28", "Боевик"),
            ("genre:16", "Анимация")
        ]
    );
}

#[test]
fn category_shapes_and_bounds_are_strict() {
    let request = CategoriesRequest::new("en").unwrap();
    for value in [
        json!({}),
        json!({"genres":null}),
        json!({"genres":[null]}),
        json!({"genres":[{"id":0,"name":"Genre"}]}),
        json!({"genres":[{"id":1,"name":" "}]}),
    ] {
        assert_eq!(
            request.decode(200, &bytes(value)).err(),
            Some(Error::InvalidResponse)
        );
    }
    let one = json!({"id":1,"name":"Genre"});
    assert_eq!(
        request
            .decode(200, &bytes(json!({"genres":vec![one;101]})))
            .err(),
        Some(Error::ResponseLimit)
    );
}

#[test]
fn page_projects_shared_movie_metadata_without_playback_sources() {
    let result = request(1)
        .decode(200, &page(vec![movie_value(json!(603))], 1, json!(5)))
        .unwrap();
    assert_eq!(result.received_count, 1);
    assert_eq!(result.next_page, Some(2));
    let movie = &result.movies[0];
    assert_eq!(movie.metadata.id, "tmdb:movie:603");
    assert_eq!(movie.metadata.title, "Localized film");
    assert_eq!(
        movie.metadata.description,
        "First paragraph.\nSecond paragraph."
    );
    assert_eq!(movie.metadata.release_info.as_deref(), Some("2026"));
    assert_eq!(
        movie.metadata.artwork.as_ref().unwrap().candidate(),
        "https://image.tmdb.org/t/p/w342/poster.jpg"
    );
    assert_eq!(
        movie.stream_ids().unwrap(),
        vec!["tmdb:603", "tmdb:movie:603"]
    );
}

#[test]
fn duplicate_rows_do_not_change_raw_count_or_page_cursor() {
    let result = request(2)
        .decode(
            200,
            &page(
                vec![
                    movie_value(json!(1)),
                    movie_value(json!(1)),
                    movie_value(json!(2)),
                ],
                2,
                json!(4),
            ),
        )
        .unwrap();
    assert_eq!(result.movies.len(), 2);
    assert_eq!(result.received_count, 3);
    assert_eq!(result.next_page, Some(3));
}

#[test]
fn empty_and_final_pages_keep_explicit_completion_information() {
    let result = request(1).decode(200, &page(vec![], 1, json!(0))).unwrap();
    assert!(result.movies.is_empty());
    assert_eq!(result.received_count, 0);
    assert_eq!(result.next_page, None);
    let result = request(4)
        .decode(200, &page(vec![movie_value(json!(1))], 4, json!(4)))
        .unwrap();
    assert_eq!(result.next_page, None);
    let result = request(500)
        .decode(200, &page(vec![movie_value(json!(1))], 500, json!(999)))
        .unwrap();
    assert_eq!(result.next_page, None);
}

#[test]
fn response_page_identity_and_total_are_not_coerced_from_strings() {
    for body in [
        bytes(json!({"page":2,"total_pages":3,"results":[]})),
        bytes(json!({"page":"1","total_pages":3,"results":[]})),
        bytes(json!({"page":1,"total_pages":"3","results":[]})),
        bytes(json!({"page":1,"total_pages":-1,"results":[]})),
        bytes(json!({"page":1,"total_pages":1.5,"results":[]})),
        bytes(json!({"page":1,"results":[]})),
        bytes(json!({"page":1,"total_pages":1,"results":null})),
    ] {
        assert_eq!(
            request(1).decode(200, &body).err(),
            Some(Error::InvalidResponse)
        );
    }
}

#[test]
fn page_limits_reject_the_complete_oversize_response() {
    assert_eq!(
        request(1)
            .decode(200, &page(vec![movie_value(json!(1)); 101], 1, json!(1)))
            .err(),
        Some(Error::ResponseLimit)
    );
    assert_eq!(
        request(1).decode(200, &vec![b' '; 524_289]).err(),
        Some(Error::ResponseLimit)
    );
}

#[test]
fn invalid_movie_cannot_turn_a_partial_page_into_success() {
    assert_eq!(
        request(1)
            .decode(
                200,
                &page(vec![movie_value(json!(1)), json!({"id":2})], 1, json!(1))
            )
            .err(),
        Some(Error::InvalidResponse)
    );
    for id in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!(u64::MAX),
        json!(1.0e30),
        json!("01"),
        json!("1/2"),
        json!("9999999999999999"),
        Value::Null,
    ] {
        assert_eq!(
            request(1)
                .decode(200, &page(vec![movie_value(id)], 1, json!(1)))
                .err(),
            Some(Error::InvalidResponse)
        );
    }
}

#[test]
fn integer_and_string_numeric_ids_match_classic_identity() {
    for id in [json!(603), json!(603.0), json!("603")] {
        let result = request(1)
            .decode(200, &page(vec![movie_value(id)], 1, json!(1)))
            .unwrap();
        assert_eq!(result.movies[0].metadata.id, "tmdb:movie:603");
    }
    let result = request(1)
        .decode(
            200,
            &page(
                vec![movie_value(json!(999_999_999_999_999u64))],
                1,
                json!(1),
            ),
        )
        .unwrap();
    assert_eq!(result.movies[0].metadata.id, "tmdb:movie:999999999999999");
}

#[test]
fn original_title_is_a_fallback_and_unsafe_optional_fields_stay_empty() {
    let mut value = movie_value(json!(1));
    value["title"] = json!(" ");
    value["overview"] = json!("bad\0description");
    value["poster_path"] = json!("https://other.test/poster.jpg");
    value["release_date"] = json!("not a date");
    let result = request(1)
        .decode(200, &page(vec![value], 1, json!(1)))
        .unwrap();
    let movie = &result.movies[0];
    assert_eq!(movie.metadata.title, "Original film");
    assert_eq!(movie.metadata.description, "");
    assert!(movie.metadata.artwork.is_none());
    assert!(movie.metadata.release_info.is_none());
}

#[test]
fn poster_paths_cannot_become_an_external_authority_or_query() {
    for path in [
        "https://other.test/a.jpg",
        "/a.jpg?token=1",
        "/a.jpg#part",
        "/a%2f.jpg",
        "/a\\b.jpg",
    ] {
        let mut value = movie_value(json!(1));
        value["poster_path"] = json!(path);
        let result = request(1)
            .decode(200, &page(vec![value], 1, json!(1)))
            .unwrap();
        assert!(result.movies[0].metadata.artwork.is_none(), "{path}");
    }
}

#[test]
fn details_append_external_ids_and_reject_another_movie() {
    let request = DetailsRequest::new("tmdb:movie:603", "FR-fr").unwrap();
    assert_eq!(
        request.relative_path(),
        "movie/603?language=fr-FR&append_to_response=external_ids"
    );
    assert_eq!(
        request.decode(200, &bytes(movie_value(json!(604)))).err(),
        Some(Error::InvalidResponse)
    );
    for id in [
        "603",
        "tmdb:603",
        "tmdb:movie:0",
        "tmdb:movie:01",
        "tmdb:movie:1?api_key=bad",
        "imdb:movie:tt0000001",
    ] {
        assert!(DetailsRequest::new(id, "en").is_err(), "{id}");
    }
}

#[test]
fn details_supply_imdb_priority_to_existing_stream_resource_plans() {
    let mut value = movie_value(json!(603));
    value["external_ids"] = json!({"imdb_id":"tt0133093"});
    let movie = DetailsRequest::new("tmdb:movie:603", "en")
        .unwrap()
        .decode(200, &bytes(value))
        .unwrap();
    assert_eq!(
        movie.stream_ids().unwrap(),
        vec!["tt0133093", "tmdb:603", "tmdb:movie:603"]
    );
    let manifest = super::super::ott_stremio_catalog::Manifest::parse(&bytes(json!({
        "id":"test","name":"Sources","types":["movie"],"catalogs":[],
        "resources":[{"name":"stream","types":["movie"],"idPrefixes":["tt"]}]
    })))
    .unwrap();
    let ids = movie.stream_ids().unwrap();
    let candidates = ids.iter().map(String::as_str).collect::<Vec<_>>();
    assert_eq!(
        manifest
            .stream_request(&candidates)
            .unwrap()
            .unwrap()
            .relative_path(),
        "stream/movie/tt0133093.json"
    );
}

#[test]
fn direct_imdb_id_has_priority_and_malformed_ids_are_not_forwarded() {
    let mut value = movie_value(json!(1));
    value["imdb_id"] = json!("tt0000001");
    value["external_ids"] = json!({"imdb_id":"tt0000002"});
    let movie = DetailsRequest::new("tmdb:movie:1", "en")
        .unwrap()
        .decode(200, &bytes(value))
        .unwrap();
    assert_eq!(movie.imdb_id.as_deref(), Some("tt0000001"));
    for id in ["tt123", "tt0000001\n", "tt0000001/path", "tmdb:1"] {
        let mut value = movie_value(json!(1));
        value["imdb_id"] = json!(id);
        let movie = DetailsRequest::new("tmdb:movie:1", "en")
            .unwrap()
            .decode(200, &bytes(value))
            .unwrap();
        assert_eq!(movie.imdb_id, None);
    }
}

#[test]
fn changed_public_movie_identity_cannot_be_forwarded_to_stream_addons() {
    let mut movie = DetailsRequest::new("tmdb:movie:1", "en")
        .unwrap()
        .decode(200, &bytes(movie_value(json!(1))))
        .unwrap();
    movie.metadata.id = "https://other.test/stream".into();
    assert_eq!(movie.stream_ids(), Err(Error::InvalidRequest));
}

#[test]
fn http_and_api_errors_are_distinct_from_empty_catalogs() {
    let categories = CategoriesRequest::new("en").unwrap();
    let details = DetailsRequest::new("tmdb:movie:1", "en").unwrap();
    for status in [301, 401, 403, 429, 503] {
        assert_eq!(
            categories.decode(status, b"private upstream body").err(),
            Some(Error::HttpStatus(status))
        );
        assert_eq!(
            request(1).decode(status, b"private upstream body").err(),
            Some(Error::HttpStatus(status))
        );
        assert_eq!(
            details.decode(status, b"private upstream body").err(),
            Some(Error::HttpStatus(status))
        );
    }
    let api =
        bytes(json!({"success":false,"status_code":7,"status_message":"Private upstream text"}));
    assert_eq!(categories.decode(200, &api).err(), Some(Error::ApiError));
    assert_eq!(request(1).decode(200, &api).err(), Some(Error::ApiError));
    assert_eq!(details.decode(200, &api).err(), Some(Error::ApiError));
    assert!(!format!("{:?}", Error::ApiError).contains("Private"));
}

#[test]
fn malformed_json_and_non_object_documents_are_rejected() {
    for body in [b"{".as_slice(), b"null", b"[]", b"false"] {
        assert_eq!(
            request(1).decode(200, body).err(),
            Some(Error::InvalidResponse)
        );
    }
}
