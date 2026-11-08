use super::super::ott_companion::protocol::Category as WireCategory;
use super::*;

fn source() -> IndexerSource {
    IndexerSource {
        id: "jackett".into(),
        label: "Home indexer".into(),
    }
}

fn caps() -> IndexerCapabilities {
    IndexerCapabilities {
        source_id: "jackett".into(),
        search_available: true,
        max_limit: 100,
        categories: vec![
            WireCategory {
                id: 2000,
                name: "Movies".into(),
                parent_id: None,
            },
            WireCategory {
                id: 2040,
                name: "HD".into(),
                parent_id: Some(2000),
            },
        ],
    }
}

fn input(language: &str) -> SearchInput<'_> {
    SearchInput {
        query: "open film",
        offset: 0,
        limit: 50,
        preferred_language: language,
        generation: 7,
    }
}

fn plan(language: &str) -> SearchPlan {
    let caps = caps();
    let category = categories(&source(), Some(&caps)).unwrap().remove(1);
    search_plan(&category, &caps, input(language)).unwrap()
}

fn item(index: usize, languages: &[&str]) -> SearchItem {
    SearchItem {
        result_ref: format!("{index:032x}"),
        title: format!("Film {index}"),
        size: None,
        seeders: None,
        published_at: None,
        category_ids: vec![2000],
        languages: languages.iter().map(|value| (*value).into()).collect(),
    }
}

fn page(items: Vec<SearchItem>) -> SearchPage {
    SearchPage {
        source_id: "jackett".into(),
        items,
        next_offset: Some(50),
        total: Some(100),
    }
}

#[test]
fn source_and_category_ids_are_canonical_and_parent_order_is_retained() {
    let categories = categories(&source(), Some(&caps())).unwrap();
    assert_eq!(
        categories
            .iter()
            .map(|category| category.id.as_str())
            .collect::<Vec<_>>(),
        [
            "catalog:torznab:jackett:all",
            "catalog:torznab:jackett:2000",
            "catalog:torznab:jackett:2040"
        ]
    );
    assert_eq!(categories[0].title, "Home indexer");
    assert_eq!(categories[1].title, "Home indexer / Movies");
    assert_eq!(categories[2].category_ids, [2040]);
}

#[test]
fn absent_empty_and_unsupported_capabilities_do_not_invent_search() {
    let source = source();
    let missing = categories(&source, None).unwrap();
    assert_eq!(missing.len(), 1);
    assert!(!missing[0].search_supported);
    assert_eq!(
        search_plan(&missing[0], &caps(), input("en")).err(),
        Some("torznab_search_unavailable")
    );
    let mut caps = caps();
    caps.categories.clear();
    let empty = categories(&source, Some(&caps)).unwrap();
    assert_eq!(empty.len(), 1);
    assert!(search_plan(&empty[0], &caps, input("en")).is_ok());
    caps.search_available = false;
    let unsupported = categories(&source, Some(&caps)).unwrap();
    assert!(!unsupported[0].search_supported);
    assert_eq!(
        search_plan(&unsupported[0], &caps, input("en")).err(),
        Some("torznab_search_unavailable")
    );
}

#[test]
fn mismatched_source_or_removed_category_is_rejected() {
    let mut caps = caps();
    let category = categories(&source(), Some(&caps)).unwrap().remove(1);
    caps.categories.clear();
    assert_eq!(
        search_plan(&category, &caps, input("en")).err(),
        Some("torznab_invalid_category")
    );
    caps.source_id = "prowlarr".into();
    assert_eq!(
        categories(&source(), Some(&caps)).err(),
        Some("torznab_source_changed")
    );
    assert_eq!(
        search_plan(&category, &caps, input("en")).err(),
        Some("torznab_source_changed")
    );
}

#[test]
fn request_reuses_existing_wire_validation_and_page_budget() {
    let mut caps = caps();
    caps.max_limit = 20;
    let category = categories(&source(), Some(&caps)).unwrap().remove(1);
    let plan = search_plan(
        &category,
        &caps,
        SearchInput {
            offset: 200,
            ..input("en")
        },
    )
    .unwrap();
    let Request::Indexer(IndexerRequest::Search {
        source_id,
        query,
        category_ids,
        offset,
        limit,
    }) = plan.request()
    else {
        panic!("wrong request")
    };
    assert_eq!(source_id, "jackett");
    assert_eq!(query, "open film");
    assert_eq!(category_ids, [2000]);
    assert_eq!(offset, 200);
    assert_eq!(limit, 20);
    assert_eq!(
        search_plan(
            &category,
            &caps,
            SearchInput {
                limit: 0,
                ..input("en")
            }
        )
        .err(),
        Some("torznab_invalid_query")
    );
    assert_eq!(
        search_plan(
            &category,
            &caps,
            SearchInput {
                query: "bad\nquery",
                ..input("en")
            }
        )
        .err(),
        Some("torznab_invalid_query")
    );
}

#[test]
fn language_preference_is_stable_and_does_not_filter() {
    let projected = plan("en-US")
        .project(
            page(vec![
                item(1, &["Russian"]),
                item(2, &[]),
                item(3, &["ENG"]),
                item(4, &["French"]),
                item(5, &["English"]),
                item(6, &[]),
            ]),
            7,
        )
        .unwrap();
    assert_eq!(
        projected
            .rows
            .iter()
            .map(|row| row.title.as_str())
            .collect::<Vec<_>>(),
        ["Film 3", "Film 5", "Film 2", "Film 6", "Film 1", "Film 4"]
    );
}

#[test]
fn language_fallback_preserves_order_and_accepts_regional_tags() {
    let rows = vec![
        item(1, &["ru"]),
        item(2, &[]),
        item(3, &["en"]),
        item(4, &["PT_br"]),
    ];
    let absent = plan("").project(page(rows.clone()), 7).unwrap();
    assert_eq!(
        absent
            .rows
            .iter()
            .map(|row| row.title.as_str())
            .collect::<Vec<_>>(),
        ["Film 1", "Film 2", "Film 3", "Film 4"]
    );
    let regional = plan("pt-PT").project(page(rows), 7).unwrap();
    assert_eq!(regional.rows[0].title, "Film 4");
    assert_eq!(regional.rows[1].title, "Film 2");
}

#[test]
fn metadata_facts_preserve_zeroes_and_unknowns_without_english_labels() {
    let mut known = item(1, &["en", "ru"]);
    known.size = Some(1_073_741_824);
    known.seeders = Some(0);
    known.published_at = Some("2026-11-03T12:00:00Z".into());
    let projected = plan("")
        .project(page(vec![known, item(2, &[])]), 7)
        .unwrap();
    assert_eq!(
        projected.rows[0].facts,
        Facts {
            size_bytes: Some(1_073_741_824),
            seeders: Some(0),
            languages: vec!["en".into(), "ru".into()],
            published_at: Some("2026-11-03T12:00:00Z".into()),
        }
    );
    assert_eq!(projected.rows[0].category, "Home indexer / Movies");
    assert_eq!(
        projected.rows[1].facts,
        Facts {
            size_bytes: None,
            seeders: None,
            languages: vec![],
            published_at: None,
        }
    );
}

#[test]
fn duplicate_results_keep_first_row_and_source_offset() {
    let first = item(1, &["en"]);
    let mut duplicate = first.clone();
    duplicate.title = "A repeated result must not replace this page's first row".into();
    let projected = plan("en")
        .project(page(vec![first, duplicate, item(2, &[])]), 7)
        .unwrap();
    assert_eq!(projected.rows.len(), 2);
    assert_eq!(projected.rows[0].title, "Film 1");
    assert_eq!(projected.next_offset, Some(50));
    assert_eq!(projected.total, Some(100));
}

#[test]
fn empty_page_preserves_explicit_end_and_unknown_total() {
    let projected = plan("en")
        .project(
            SearchPage {
                next_offset: None,
                total: None,
                ..page(vec![])
            },
            7,
        )
        .unwrap();
    assert!(projected.rows.is_empty());
    assert_eq!(projected.next_offset, None);
    assert_eq!(projected.total, None);
}

#[test]
fn pagination_is_not_capped_at_512_rows_and_pages_are_not_reordered_together() {
    let caps = caps();
    let category = categories(&source(), Some(&caps)).unwrap().remove(1);
    let later = search_plan(
        &category,
        &caps,
        SearchInput {
            offset: 550,
            ..input("en")
        },
    )
    .unwrap();
    let projected = later
        .project(
            SearchPage {
                next_offset: Some(600),
                total: Some(20_000),
                ..page(vec![item(1, &["en"])])
            },
            7,
        )
        .unwrap();
    assert_eq!(projected.next_offset, Some(600));
    assert_eq!(projected.total, Some(20_000));
}

#[test]
fn unexpected_source_nonadvancing_offset_and_oversized_page_fail_closed() {
    assert_eq!(
        plan("en")
            .project(
                SearchPage {
                    source_id: "other".into(),
                    ..page(vec![])
                },
                7
            )
            .err(),
        Some("torznab_source_changed")
    );
    for offset in [0, 1_000_001] {
        assert_eq!(
            plan("en")
                .project(
                    SearchPage {
                        next_offset: Some(offset),
                        ..page(vec![])
                    },
                    7
                )
                .err(),
            Some("companion_receipt_invalid")
        );
    }
    assert_eq!(
        plan("en")
            .project(page((0..51).map(|index| item(index, &[])).collect()), 7)
            .err(),
        Some("companion_receipt_invalid")
    );
}

#[test]
fn malformed_refs_never_become_playable_selections() {
    for value in [
        "",
        "http://example.invalid/file",
        "../other",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "abcd",
    ] {
        let mut invalid = item(1, &[]);
        invalid.result_ref = value.into();
        assert_eq!(
            plan("en").project(page(vec![invalid]), 7).err(),
            Some("torznab_reference_missing")
        );
    }
}

#[test]
fn row_identity_and_prepare_are_generation_scoped() {
    let projected = plan("en").project(page(vec![item(1, &[])]), 7).unwrap();
    let row = &projected.rows[0];
    assert_eq!(row.id, "torznab:7:jackett:00000000000000000000000000000001");
    assert_eq!(
        row.selection.prepare_request(8).err().unwrap(),
        "torznab_reference_missing"
    );
    let Request::Indexer(IndexerRequest::Prepare {
        source_id,
        result_ref,
    }) = row.selection.prepare_request(7).unwrap()
    else {
        panic!("wrong request")
    };
    assert_eq!(source_id, "jackett");
    assert_eq!(result_ref, "00000000000000000000000000000001");
    assert_eq!(
        plan("en").project(page(vec![item(1, &[])]), 8).err(),
        Some("torznab_reference_missing")
    );
    assert!(!format!("{:?}", row.selection).contains(&result_ref));
}

#[test]
fn same_handle_in_a_new_generation_has_a_different_row_identity() {
    let old = plan("en").project(page(vec![item(1, &[])]), 7).unwrap();
    let caps = caps();
    let category = categories(&source(), Some(&caps)).unwrap().remove(1);
    let refreshed = search_plan(
        &category,
        &caps,
        SearchInput {
            generation: 8,
            ..input("en")
        },
    )
    .unwrap();
    let new = refreshed.project(page(vec![item(1, &[])]), 8).unwrap();
    assert_ne!(old.rows[0].id, new.rows[0].id);
}

#[test]
fn shared_iso_registry_ranks_all_supported_code_forms() {
    for (preferred, supplied) in [
        ("pt-BR", "por"),
        ("nl", "nld"),
        ("nl-BE", "dut"),
        ("tr", "tur"),
        ("ko-KR", "kor"),
        ("zh-Hant", "chi"),
        ("zh", "zho"),
        ("ja", "jpn"),
        ("uk", "ukr"),
        ("is", "ice"),
        ("cs", "cze"),
        ("el", "gre"),
        ("hy", "arm"),
        ("sq", "alb"),
        ("fa", "per"),
    ] {
        let projected = plan(preferred)
            .project(page(vec![item(1, &[]), item(2, &[supplied])]), 7)
            .unwrap();
        assert_eq!(
            projected.rows[0].title, "Film 2",
            "{preferred} versus {supplied}"
        );
    }
}

#[test]
fn unsupported_language_codes_keep_stable_exact_matching_fallback() {
    let projected = plan("fil-PH")
        .project(
            page(vec![item(1, &["en"]), item(2, &["fil"]), item(3, &[])]),
            7,
        )
        .unwrap();
    assert_eq!(
        projected
            .rows
            .iter()
            .map(|row| row.title.as_str())
            .collect::<Vec<_>>(),
        ["Film 2", "Film 3", "Film 1"]
    );
}

#[test]
fn child_scope_is_exact_in_request_and_cannot_be_dropped_from_reply() {
    let caps = caps();
    let category = categories_scoped(&source(), Some(&caps), Some("ChildA"))
        .unwrap()
        .remove(1);
    let plan = search_plan(&category, &caps, input("ru")).unwrap();
    assert!(
        matches!(plan.request(),Request::Indexer(IndexerRequest::ScopedSearch {indexer_id,category_ids,..})
        if indexer_id == "ChildA" && category_ids == vec![2000])
    );
    assert!(plan.project(page(vec![item(1, &["ru"])]), 7).is_err());
    assert!(plan
        .project_scoped(page(vec![item(1, &["ru"])]), Some("childa"), 7)
        .is_err());
    assert!(plan
        .project_scoped(page(vec![item(1, &["ru"])]), Some("ChildA"), 8)
        .is_err());
    let projected = plan
        .project_scoped(page(vec![item(1, &["ru"])]), Some("ChildA"), 7)
        .unwrap();
    assert_eq!(projected.rows.len(), 1);
    assert!(
        matches!(projected.rows[0].selection.prepare_request(7).unwrap(),
        Request::Indexer(IndexerRequest::Prepare {source_id,..}) if source_id == "jackett")
    );
}
#[test]
fn aggregate_plan_rejects_child_reply_and_reserved_child_is_invalid() {
    assert!(plan("")
        .project_scoped(page(vec![item(1, &[])]), Some("ChildA"), 7)
        .is_err());
    for child in ["all", "ALL", "", "ChildA/other"] {
        assert!(categories_scoped(&source(), Some(&caps()), Some(child)).is_err());
    }
}
