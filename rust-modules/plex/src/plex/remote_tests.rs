use super::*;
use serde_json::{json, Value};
use std::cell::Cell;

fn item(id: impl ToString, kind: &str) -> Value {
    let id = id.to_string();
    json!({"ratingKey": id, "type": kind, "title": format!("Item {id}"),
        "Media": [{"Part": [{"key": format!("/library/parts/{id}/1/file.mp4")}]}]})
}

fn page(start: usize, rows: Vec<Value>, total: Option<usize>, libraries: bool) -> Vec<u8> {
    let mut container = json!({"size": rows.len(), "offset": start});
    container[if libraries { "Directory" } else { "Metadata" }] = Value::Array(rows);
    if let Some(total) = total {
        container["totalSize"] = json!(total);
    }
    serde_json::to_vec(&json!({"MediaContainer": container})).unwrap()
}

fn window(path: &str) -> (usize, usize) {
    let query = path.split_once('?').unwrap().1;
    let field = |name| {
        query
            .split('&')
            .find_map(|p| {
                let (key, value) = p.split_once('=')?;
                (key == name).then(|| value.parse::<usize>().unwrap())
            })
            .unwrap()
    };
    (
        field("X-Plex-Container-Start"),
        field("X-Plex-Container-Size"),
    )
}

fn catalog(total: usize, known: bool, path: &str) -> Vec<u8> {
    let (start, size) = window(path);
    let rows = (start..(start + size).min(total))
        .map(|n| item(n + 1, "movie"))
        .collect();
    page(start, rows, known.then_some(total), false)
}

#[test]
fn metadata_keeps_media_and_full_twenty_digit_identifiers() {
    let large = "99999999999999999999";
    let records = metadata_with(&[large], |path| {
        assert_eq!(path, "/library/metadata/99999999999999999999");
        Ok(page(0, vec![item(large, "movie")], Some(1), false))
    })
    .unwrap();
    assert_eq!(records[0].rating_key, large);
    assert_eq!(
        records[0].media[0].part[0].key,
        format!("/library/parts/{large}/1/file.mp4")
    );
}

#[test]
fn invalid_ids_reject_before_any_request() {
    let cases: Value = serde_json::from_str(include_str!("remote_read_cases.json")).unwrap();
    for id in cases["invalid_ids"].as_array().unwrap() {
        assert_eq!(
            metadata_with(&[id.as_str().unwrap()], |_| panic!("invalid ID sent")).err(),
            Some(ReadError::InvalidIds)
        );
    }
    assert_eq!(
        metadata_with(&[], |_| panic!("empty request sent")).err(),
        Some(ReadError::InvalidIds)
    );
    assert_eq!(
        metadata_with(&vec!["1"; 501], |_| panic!("oversize request sent")).err(),
        Some(ReadError::InvalidIds)
    );
}

#[test]
fn metadata_chunks_bounded_urls_and_never_strips_media_or_summary() {
    let ids: Vec<String> = (0..500).map(|n| format!("900000000000000{n:05}")).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut requests = Vec::new();
    let records = metadata_with(&refs, |path| {
        assert!(path.len() <= 2048);
        assert!(!path.contains('?'));
        let chunk: Vec<_> = path
            .strip_prefix("/library/metadata/")
            .unwrap()
            .split(',')
            .collect();
        requests.push(path.to_owned());
        Ok(page(
            0,
            chunk.iter().rev().map(|id| item(id, "movie")).collect(),
            Some(chunk.len()),
            false,
        ))
    })
    .unwrap();
    assert!(requests.len() > 1);
    assert_eq!(records.len(), 500);
    let actual: HashSet<_> = records.iter().map(|m| m.rating_key.as_str()).collect();
    assert_eq!(actual, refs.into_iter().collect());
}

#[test]
fn metadata_deduplicates_only_requests_not_the_callers_occurrence_list() {
    let input = ["2", "1", "2"];
    let records = metadata_with(&input, |path| {
        assert_eq!(path, "/library/metadata/2,1");
        Ok(page(
            0,
            vec![item("1", "episode"), item("2", "movie")],
            Some(2),
            false,
        ))
    })
    .unwrap();
    assert_eq!(input, ["2", "1", "2"]);
    assert_eq!(
        records
            .iter()
            .map(|m| m.rating_key.as_str())
            .collect::<Vec<_>>(),
        ["1", "2"]
    );
}

#[test]
fn missing_duplicate_unexpected_or_nonplayable_metadata_never_partially_succeeds() {
    for rows in [
        vec![item(1, "movie")],
        vec![item(1, "movie"), item(1, "movie")],
        vec![item(1, "movie"), item(3, "movie")],
    ] {
        assert_eq!(
            metadata_with(&["1", "2"], |_| Ok(page(0, rows.clone(), None, false))).err(),
            Some(ReadError::Incomplete)
        );
    }
    for record in [json!({"ratingKey":"1", "type":"movie"}), item(1, "show")] {
        assert_eq!(
            metadata_with(&["1"], |_| Ok(page(0, vec![record.clone()], None, false))).err(),
            Some(ReadError::Unplayable)
        );
    }
}

#[test]
fn numeric_or_duplicate_identifier_fields_are_not_rounded_or_last_wins() {
    for raw in [
        r#"{"MediaContainer":{"Metadata":[{"ratingKey":99999999999999999999,"type":"movie"}]}}"#,
        r#"{"MediaContainer":{"Metadata":[{"ratingKey":"1","ratingKey":"2","type":"movie"}]}}"#,
    ] {
        assert_eq!(
            metadata_with(&["1"], |_| Ok(raw.as_bytes().to_vec())).err(),
            Some(ReadError::Malformed)
        );
    }
}

#[test]
fn strict_counter_fixtures_reject_lossy_null_boolean_or_duplicate_values() {
    let cases: Value = serde_json::from_str(include_str!("remote_read_cases.json")).unwrap();
    for value in cases["invalid_counts"].as_array().unwrap() {
        for field in ["size", "offset", "totalSize"] {
            let raw = format!(r#"{{"MediaContainer":{{"{field}":{value},"Metadata":[]}}}}"#);
            assert_eq!(
                item_page(raw.as_bytes()).err(),
                Some(ReadError::Malformed),
                "{raw}"
            );
        }
    }
    for field in ["size", "offset", "totalSize"] {
        let raw = format!(r#"{{"MediaContainer":{{"{field}":0,"{field}":0,"Metadata":[]}}}}"#);
        assert_eq!(item_page(raw.as_bytes()).err(), Some(ReadError::Malformed));
    }
    let page =
        item_page(br#"{"MediaContainer":{"size":"0","offset":"0","totalSize":"0","Metadata":[]}}"#)
            .unwrap();
    assert_eq!(
        (page.size, page.offset, page.total),
        (Some(0), Some(0), Some(0))
    );
}

#[test]
fn missing_count_is_distinct_from_zero_and_null_rows_are_not_empty() {
    let absent = item_page(br#"{"MediaContainer":{"Metadata":[]}}"#).unwrap();
    assert_eq!(
        (absent.size, absent.offset, absent.total),
        (None, None, None)
    );
    assert_eq!(
        item_page(br#"{"MediaContainer":{"size":0,"Metadata":null}}"#).err(),
        Some(ReadError::Malformed)
    );
    assert_eq!(
        item_page(br#"{"MediaContainer":{"size":0}}"#)
            .unwrap()
            .rows
            .len(),
        0
    );
    assert_eq!(
        item_page(br#"{"MediaContainer":{}}"#).err(),
        Some(ReadError::Malformed)
    );
}

#[test]
fn exactly_five_hundred_is_complete_without_false_truncation() {
    for known in [true, false] {
        let mut starts = Vec::new();
        let result = library_items_with("7", LibraryKind::Movie, true, |path| {
            starts.push(window(path).0);
            Ok(catalog(500, known, path))
        })
        .unwrap();
        assert_eq!(result.items().len(), 500);
        assert!(!result.truncated());
        assert_eq!(
            starts,
            if known {
                vec![0, 200, 400]
            } else {
                vec![0, 200, 400, 500]
            }
        );
    }
}

#[test]
fn truncation_requires_observed_unique_compatible_501st_item() {
    for known in [true, false] {
        let result = library_items_with("7", LibraryKind::Movie, true, |path| {
            Ok(catalog(501, known, path))
        })
        .unwrap();
        assert!(result.truncated());
        assert_eq!(result.items()[0].rating_key, "1");
        assert_eq!(result.items()[499].rating_key, "500");
        assert_eq!(result.into_items().len(), 500);
    }
    let result = library_items_with("7", LibraryKind::Movie, true, |path| {
        let (start, _) = window(path);
        let mut body: Value = serde_json::from_slice(&catalog(501, true, path)).unwrap();
        if start == 400 {
            body["MediaContainer"]["Metadata"][100]
                .as_object_mut()
                .unwrap()
                .remove("Media");
        }
        Ok(serde_json::to_vec(&body).unwrap())
    })
    .unwrap();
    assert!(
        result.truncated(),
        "the omitted item is identified but never played"
    );
}

#[test]
fn claimed_overflow_with_missing_501st_item_is_incomplete() {
    let mut starts = Vec::new();
    let result = library_items_with("7", LibraryKind::Movie, true, |path| {
        starts.push(window(path).0);
        let mut body: Value = serde_json::from_slice(&catalog(500, true, path)).unwrap();
        body["MediaContainer"]["totalSize"] = json!(501);
        Ok(serde_json::to_vec(&body).unwrap())
    });
    assert_eq!(result.err(), Some(ReadError::Incomplete));
    assert_eq!(starts, [0, 200, 400, 500]);
}

#[test]
fn overflow_without_permission_returns_error_not_partial_queue() {
    for known in [true, false] {
        assert_eq!(
            library_items_with("7", LibraryKind::Movie, false, |path| Ok(catalog(
                501, known, path
            )))
            .err(),
            Some(ReadError::TooManyItems)
        );
    }
}

#[test]
fn changed_totals_bad_offsets_counts_and_huge_offsets_fail() {
    for (field, value) in [
        ("offset", json!(0)),
        ("offset", json!(u64::MAX)),
        ("size", json!(199)),
        ("totalSize", json!(301)),
        ("totalSize", json!(200)),
    ] {
        let result = library_items_with("7", LibraryKind::Movie, false, |path| {
            let (start, _) = window(path);
            let mut body: Value = serde_json::from_slice(&catalog(300, true, path)).unwrap();
            if start != 0 {
                body["MediaContainer"][field] = value.clone();
            }
            Ok(serde_json::to_vec(&body).unwrap())
        });
        assert_eq!(result.err(), Some(ReadError::Incomplete), "{field}={value}");
    }
}

#[test]
fn duplicate_pages_or_ids_and_incompatible_lookahead_are_rejected() {
    for mode in [0, 1, 2] {
        let result = library_items_with("7", LibraryKind::Movie, true, |path| {
            let (start, _) = window(path);
            let mut body: Value = serde_json::from_slice(&catalog(501, true, path)).unwrap();
            if start == 400 {
                if mode == 0 {
                    body["MediaContainer"]["Metadata"][0] = item(1, "movie");
                }
                if mode == 1 {
                    body["MediaContainer"]["Metadata"][100] = item(1, "movie");
                }
                if mode == 2 {
                    body["MediaContainer"]["Metadata"][100] = item(501, "episode");
                }
            }
            Ok(serde_json::to_vec(&body).unwrap())
        });
        assert_eq!(
            result.err(),
            Some(if mode == 2 {
                ReadError::Unplayable
            } else {
                ReadError::Incomplete
            })
        );
    }
}

#[test]
fn library_ids_and_titles_stay_exact_and_show_reads_request_episodes() {
    let large = "99999999999999999999";
    let libraries = libraries_with(|path| {
        assert_eq!(path, "/library/sections");
        Ok(page(
            0,
            vec![
                json!({"key":large,"title":"Straße Σειρά","type":"show"}),
                json!({"key":"2","title":"Audio","type":"artist"}),
            ],
            Some(2),
            true,
        ))
    })
    .unwrap();
    assert_eq!(libraries[0].id(), large);
    assert_eq!(libraries[0].title(), "Straße Σειρά");
    assert_eq!(libraries[1].kind(), LibraryKind::Other);
    let result = library_items_with(large, libraries[0].kind(), false, |path| {
        assert!(path.starts_with("/library/sections/99999999999999999999/all?type=4&"));
        assert!(!path.contains("sort"));
        Ok(page(
            0,
            vec![item(2, "episode"), item(1, "episode")],
            Some(2),
            false,
        ))
    })
    .unwrap();
    assert_eq!(
        result
            .items()
            .iter()
            .map(|m| m.rating_key.as_str())
            .collect::<Vec<_>>(),
        ["2", "1"]
    );
    assert_eq!(
        library_items_with("2", LibraryKind::Other, false, |_| panic!(
            "unsupported type requested"
        ))
        .err(),
        Some(ReadError::UnsupportedLibrary)
    );
}

#[test]
fn library_limit_or_duplicates_never_return_partial_list() {
    assert_eq!(
        libraries_with(|_| Ok(page(
            0,
            vec![json!({"key":"1","title":"A","type":"movie"}); 2],
            Some(2),
            true
        )))
        .err(),
        Some(ReadError::Incomplete)
    );
    assert_eq!(
        libraries_with(|_| Ok(page(0, vec![], Some(1001), true))).err(),
        Some(ReadError::TooManyLibraries)
    );
}

#[test]
fn unsafe_part_paths_never_count_as_playable() {
    let cases: Value = serde_json::from_str(include_str!("remote_read_cases.json")).unwrap();
    for key in cases["invalid_part_keys"].as_array().unwrap() {
        let mut row = item(1, "movie");
        row["Media"][0]["Part"][0]["key"] = key.clone();
        assert_eq!(
            metadata_with(&["1"], |_| Ok(page(0, vec![row.clone()], None, false))).err(),
            Some(ReadError::Unplayable),
            "{key}"
        );
    }
    assert!(valid_part_key("/library/parts/1/1/a%20b.mp4"));
    assert!(valid_part_key(
        "/library/parts/1/1/file.mp4?download=1&filename=a%20b.mp4"
    ));
    let mut safe = item(1, "movie");
    safe["Media"][0]["Part"][0]["key"] = json!("/library/parts/1/1/file.mp4?download=1");
    assert!(metadata_with(&["1"], |_| Ok(page(0, vec![safe.clone()], None, false))).is_ok());
}

#[test]
fn cancellation_and_authority_retirement_discard_response_and_prevent_next_request() {
    let off = OffFrame::for_test();
    let cancelled = Cell::new(false);
    let current = Cell::new(true);
    let cancel = || cancelled.get();
    let mut state = ReadState::new(&off, Deadline::after(&off, Duration::from_secs(5)), &cancel);
    assert_eq!(
        state
            .read(
                || current.get(),
                |_, _| {
                    current.set(false);
                    Ok(vec![1])
                }
            )
            .err(),
        Some(ReadError::SourceChanged)
    );
    current.set(true);
    assert_eq!(
        state
            .read(
                || current.get(),
                |_, _| {
                    cancelled.set(true);
                    Ok(vec![1])
                }
            )
            .err(),
        Some(ReadError::Cancelled)
    );
    assert_eq!(
        state
            .read(|| true, |_, _| panic!("cancelled request issued"))
            .err(),
        Some(ReadError::Cancelled)
    );
}

#[test]
fn one_deadline_and_byte_budget_cover_metadata_and_following_pages() {
    let off = OffFrame::for_test();
    let cancel = || false;
    let mut state = ReadState::new(&off, Deadline::after(&off, Duration::from_secs(2)), &cancel);
    state
        .read(
            || true,
            |limit, allowance| {
                assert_eq!(limit, 8 * 1024 * 1024);
                assert!(allowance <= Duration::from_secs(2));
                Ok(vec![0; limit - 3])
            },
        )
        .unwrap();
    state
        .read(
            || true,
            |limit, allowance| {
                assert_eq!(limit, 3);
                assert!(allowance <= Duration::from_secs(2));
                Ok(vec![0; 3])
            },
        )
        .unwrap();
    assert_eq!(
        state
            .read(|| true, |_, _| panic!("byte budget reset"))
            .err(),
        Some(ReadError::ResponseTooLarge)
    );
}

#[test]
fn cancellation_expiry_and_read_failure_between_pages_have_no_partial_success() {
    let off = OffFrame::for_test();
    let cancelled = Cell::new(false);
    let cancel = || cancelled.get();
    for error in [
        ReadError::Cancelled,
        ReadError::Deadline,
        ReadError::Transport,
    ] {
        cancelled.set(false);
        let mut state =
            ReadState::new(&off, Deadline::after(&off, Duration::from_secs(5)), &cancel);
        let mut calls = 0;
        let result = library_items_with("7", LibraryKind::Movie, false, |path| {
            if calls == 1 {
                if error == ReadError::Cancelled {
                    cancelled.set(true);
                }
                if error == ReadError::Deadline {
                    state.deadline = Deadline::after(&off, Duration::ZERO);
                }
            }
            state.read(
                || true,
                |_, _| {
                    calls += 1;
                    if calls == 2 {
                        return Err(ReadError::Transport);
                    }
                    Ok(catalog(300, true, path))
                },
            )
        });
        assert_eq!(result.err(), Some(error));
        assert_eq!(calls, if error == ReadError::Transport { 2 } else { 1 });
    }
}

#[test]
fn expired_transaction_and_oversize_response_are_rejected_at_request_boundary() {
    let off = OffFrame::for_test();
    let cancel = || false;
    let mut expired = ReadState::new(&off, Deadline::after(&off, Duration::ZERO), &cancel);
    assert_eq!(
        expired
            .read(|| true, |_, _| panic!("expired request issued"))
            .err(),
        Some(ReadError::Deadline)
    );
    let mut state = ReadState::new(&off, Deadline::after(&off, Duration::from_secs(5)), &cancel);
    assert_eq!(
        state.read(|| true, |limit, _| Ok(vec![0; limit + 1])).err(),
        Some(ReadError::ResponseTooLarge)
    );
}

#[test]
fn ordinary_unpaged_section_directory_is_read_once_and_not_repeated() {
    let mut calls = 0;
    let result = libraries_with(|path| {
        calls += 1;
        assert_eq!(calls, 1);
        assert_eq!(path, "/library/sections");
        Ok(br#"{"MediaContainer":{"size":2,"Directory":[{"key":"1","type":"movie","title":"Movies"},{"key":"2","type":"show","title":"Shows"}]}}"#.to_vec())
    }).unwrap();
    assert_eq!(result.len(), 2);
    let incomplete = br#"{"MediaContainer":{"size":1,"totalSize":2,"Directory":[{"key":"1","type":"movie","title":"Movies"}]}}"#;
    assert_eq!(
        libraries_with(|_| Ok(incomplete.to_vec())).err(),
        Some(ReadError::Incomplete)
    );
}

#[test]
fn retained_total_rejects_overrun_when_later_pages_omit_it() {
    for actual in [500, 501] {
        let mut starts = Vec::new();
        let result = library_items_with("7", LibraryKind::Movie, true, |path| {
            let (start, _) = window(path);
            starts.push(start);
            let mut body: Value = serde_json::from_slice(&catalog(actual, false, path)).unwrap();
            if start == 0 {
                body["MediaContainer"]["totalSize"] = json!(450);
            }
            Ok(serde_json::to_vec(&body).unwrap())
        });
        assert_eq!(result.err(), Some(ReadError::Incomplete));
        assert_eq!(
            starts,
            [0, 200, 400],
            "overrun must fail on its own page before admitting rows or requesting another page"
        );
    }
}

#[test]
fn retained_total_allows_consistent_pages_when_later_totals_are_omitted() {
    let mut starts = Vec::new();
    let result = library_items_with("7", LibraryKind::Movie, true, |path| {
        let (start, _) = window(path);
        starts.push(start);
        let mut body: Value = serde_json::from_slice(&catalog(450, false, path)).unwrap();
        if start == 0 {
            body["MediaContainer"]["totalSize"] = json!(450);
        }
        Ok(serde_json::to_vec(&body).unwrap())
    })
    .unwrap();
    assert_eq!(result.items().len(), 450);
    assert!(!result.truncated());
    assert_eq!(starts, [0, 200, 400]);
}
