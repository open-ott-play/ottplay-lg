use super::*;
use serde_json::json;
use std::cell::Cell;

#[derive(Default)]
struct RestoreReader {
    calls: Vec<Vec<String>>,
    fail_at: Option<usize>,
}
impl Reader for RestoreReader {
    fn metadata(&mut self, ids: &[&str]) -> Result<Vec<Metadata>, remote::ReadError> {
        assert!(ids.len() <= queue::ITEM_LIMIT);
        self.calls.push(ids.iter().map(|id| (*id).into()).collect());
        if self.fail_at == Some(self.calls.len()) {
            return Err(remote::ReadError::Incomplete);
        }
        Ok(ids.iter().rev().map(|id| row(id, "movie")).collect())
    }
    fn libraries(&mut self) -> Result<Vec<remote::Library>, remote::ReadError> {
        panic!("restore enumerated libraries")
    }
    fn items(
        &mut self,
        _: &str,
        _: remote::LibraryKind,
        _: bool,
    ) -> Result<ReadItems, remote::ReadError> {
        panic!("restore enumerated a library")
    }
}

#[test]
fn restore_reads_one_thousand_ids_in_bounded_calls_on_the_same_reader() {
    let values: Vec<_> = (1..=1000)
        .map(|n| queue::DecimalId::parse(&n.to_string()).unwrap())
        .collect();
    let ids = queue::RestoreIds::new(values).unwrap();
    let mut reader = RestoreReader::default();
    let prepared = prepare_restore_with(&mut reader, &ids, &current).unwrap();
    assert_eq!(
        reader.calls.iter().map(Vec::len).collect::<Vec<_>>(),
        [500, 500]
    );
    let Prepared::Queue { kind, selection } = prepared else {
        panic!("queue required")
    };
    assert_eq!(kind, PrepareKind::Play);
    assert_eq!(selection.metadata.len(), 1000);
    assert_eq!(selection.occurrences.len(), 1000);
    for (i, record) in selection.prepared.records().iter().enumerate() {
        assert_eq!(record.id, (i + 1).to_string());
        assert_eq!(
            selection.metadata[selection.occurrences[i]].rating_key,
            record.id
        );
    }
}

#[test]
fn restore_duplicate_positions_do_not_duplicate_metadata_or_change_order() {
    let ids = queue::RestoreIds::new(ids(&["42", "42", "7"])).unwrap();
    let mut reader = RestoreReader::default();
    let Prepared::Queue { selection, .. } =
        prepare_restore_with(&mut reader, &ids, &current).unwrap()
    else {
        panic!("queue required")
    };
    assert_eq!(reader.calls, [vec![String::from("42"), String::from("7")]]);
    assert_eq!(selection.prepared.ids(), ["42", "42", "7"]);
    assert_eq!(selection.metadata.len(), 2);
    assert_eq!(selection.occurrences[0], selection.occurrences[1]);
    assert_ne!(selection.occurrences[1], selection.occurrences[2]);
}

#[test]
fn restore_never_returns_a_prefix_when_the_second_chunk_fails() {
    let ids = queue::RestoreIds::new(
        (1..=501)
            .map(|n| queue::DecimalId::parse(&n.to_string()).unwrap())
            .collect(),
    )
    .unwrap();
    let mut reader = RestoreReader {
        fail_at: Some(2),
        ..Default::default()
    };
    assert_eq!(
        prepare_restore_with(&mut reader, &ids, &current).err(),
        Some(WireError::Queue(QueueError::MissingItem))
    );
    assert_eq!(reader.calls.len(), 2);
}

#[test]
fn restore_rechecks_cancellation_before_advancing_to_the_next_chunk() {
    let ids = queue::RestoreIds::new(
        (1..=501)
            .map(|n| queue::DecimalId::parse(&n.to_string()).unwrap())
            .collect(),
    )
    .unwrap();
    let checks = Cell::new(0);
    let check = || {
        checks.set(checks.get() + 1);
        if checks.get() >= 3 {
            Err(WireError::Queue(QueueError::Cancelled))
        } else {
            Ok(())
        }
    };
    let mut reader = RestoreReader::default();
    assert_eq!(
        prepare_restore_with(&mut reader, &ids, &check).err(),
        Some(WireError::Queue(QueueError::Cancelled))
    );
    assert_eq!(reader.calls.len(), 1);
}

#[derive(Default)]
struct FixtureReader {
    metadata: Option<Result<Vec<Metadata>, remote::ReadError>>,
    libraries: Option<Result<Vec<remote::Library>, remote::ReadError>>,
    items: Option<Result<ReadItems, remote::ReadError>>,
    calls: Vec<String>,
}

impl Reader for FixtureReader {
    fn metadata(&mut self, ids: &[&str]) -> Result<Vec<Metadata>, remote::ReadError> {
        self.calls.push(format!("metadata:{}", ids.join(",")));
        self.metadata.take().expect("unexpected metadata read")
    }
    fn libraries(&mut self) -> Result<Vec<remote::Library>, remote::ReadError> {
        self.calls.push("libraries".into());
        self.libraries.take().expect("unexpected library read")
    }
    fn items(
        &mut self,
        id: &str,
        kind: remote::LibraryKind,
        truncate: bool,
    ) -> Result<ReadItems, remote::ReadError> {
        self.calls.push(format!("items:{id}:{kind:?}:{truncate}"));
        self.items.take().expect("unexpected item read")
    }
}

fn identity(value: &str) -> String {
    value.into()
}
fn options() -> PrepareOptions<'static> {
    PrepareOptions {
        normalize: identity,
        permutation: None,
    }
}
fn current() -> Result<(), WireError> {
    Ok(())
}
fn ids(values: &[&str]) -> Vec<queue::DecimalId> {
    values
        .iter()
        .map(|v| queue::DecimalId::parse(v).unwrap())
        .collect()
}
fn request(kind: PrepareKind, values: &[&str]) -> PlexRequest {
    PlexRequest::Queue(QueueRequest::Prepare {
        kind,
        ids: ids(values),
    })
}
fn library_request(
    kind: PrepareKind,
    selector: queue::LibrarySelector,
    shuffle: bool,
    allow_truncate: bool,
) -> PlexRequest {
    PlexRequest::Library(LibraryRequest::Prepare {
        kind,
        selector,
        shuffle,
        allow_truncate,
    })
}
fn row(id: impl ToString, kind: &str) -> Metadata {
    let id = id.to_string();
    serde_json::from_value(json!({"ratingKey":id,"title":format!("Title {id}"),"type":kind,
        "librarySectionID":17,"Media":[{"Part":[{"key":format!("/library/parts/{id}/1/file.mp4?download=1"),
        "file":format!("/private-test-media/{id}.mp4"),"Stream":[{"id":9,"streamType":2,"language":"English"}]}]}]})).unwrap()
}
fn library(id: &str, title: &str, kind: &str) -> remote::Library {
    serde_json::from_value(json!({"key":id,"title":title,"type":kind})).unwrap()
}

#[test]
fn queue_duplicate_occurrences_share_one_full_metadata_record() {
    let mut reader = FixtureReader {
        metadata: Some(Ok(vec![row(1, "movie"), row(2, "episode")])),
        ..Default::default()
    };
    let prepared = prepare_with(
        &mut reader,
        &request(PrepareKind::Preview, &["2", "1", "2"]),
        &mut options(),
        &current,
    )
    .unwrap();
    let Prepared::Queue { kind, selection } = &prepared else {
        panic!("queue expected")
    };
    assert_eq!(*kind, PrepareKind::Preview);
    assert_eq!(selection.metadata.len(), 2);
    assert_eq!(selection.occurrences, [1, 0, 1]);
    assert_eq!(selection.prepared.ids(), ["2", "1", "2"]);
    assert_eq!(
        selection.metadata[1].media[0].part[0].file,
        "/private-test-media/2.mp4"
    );
    for (position, metadata_index) in selection.occurrences.iter().enumerate() {
        assert_eq!(
            selection.prepared.records()[position].id,
            selection.metadata[*metadata_index].rating_key
        );
    }
    assert_eq!(reader.calls, ["metadata:2,1,2"]);
    let wire = prepared.preview_value("webos-native").unwrap();
    assert_eq!(wire["ids"], json!(["2", "1", "2"]));
    assert!(!wire.to_string().contains("private-test-media"));
    assert!(!wire.to_string().contains("/library/parts/"));
    assert!(wire.get("Media").is_none());
}

#[test]
fn queue_play_and_preview_never_invoke_a_permutation_provider() {
    for kind in [PrepareKind::Play, PrepareKind::Preview] {
        let mut reader = FixtureReader {
            metadata: Some(Ok(vec![row(1, "movie")])),
            ..Default::default()
        };
        let mut forbidden =
            |_| -> Result<Vec<usize>, WireError> { panic!("explicit queue order changed") };
        let mut opts = PrepareOptions {
            normalize: identity,
            permutation: Some(&mut forbidden),
        };
        let result =
            prepare_with(&mut reader, &request(kind, &["1"]), &mut opts, &current).unwrap();
        if kind == PrepareKind::Play {
            assert_eq!(
                result.preview_value("webos-native").err(),
                Some(WireError::InvalidRequest)
            );
        }
        assert_eq!(reader.calls, ["metadata:1"]);
    }
}

#[test]
fn list_returns_only_a_list_without_reading_items_or_shuffling() {
    let mut reader = FixtureReader {
        libraries: Some(Ok(vec![
            library("1", "Movies", "movie"),
            library("2", "Music", "artist"),
        ])),
        ..Default::default()
    };
    let mut forbidden = |_| -> Result<Vec<usize>, WireError> { panic!("list shuffled") };
    let mut opts = PrepareOptions {
        normalize: identity,
        permutation: Some(&mut forbidden),
    };
    let result = prepare_with(
        &mut reader,
        &PlexRequest::Library(LibraryRequest::List),
        &mut opts,
        &current,
    )
    .unwrap();
    assert!(matches!(result, Prepared::Libraries { .. }));
    let wire = result.preview_value("webos-native").unwrap();
    assert_eq!(wire["op"], "list");
    assert_eq!(wire["libraries"].as_array().unwrap().len(), 1);
    assert!(wire.get("queue").is_none());
    assert_eq!(reader.calls, ["libraries"]);
}

#[test]
fn control_commands_reject_before_any_reader_or_callback_is_used() {
    for command in [
        QueueRequest::Status,
        QueueRequest::Next,
        QueueRequest::Previous,
        QueueRequest::Stop,
    ] {
        let mut reader = FixtureReader::default();
        assert_eq!(
            prepare_with(
                &mut reader,
                &PlexRequest::Queue(command),
                &mut options(),
                &current
            )
            .err(),
            Some(WireError::InvalidRequest)
        );
        assert!(reader.calls.is_empty());
    }
}

#[test]
fn shuffle_without_an_order_provider_rejects_before_reads() {
    let mut reader = FixtureReader::default();
    let request = library_request(
        PrepareKind::Play,
        queue::LibrarySelector::Id(ids(&["1"]).remove(0)),
        true,
        false,
    );
    assert_eq!(
        prepare_with(&mut reader, &request, &mut options(), &current).err(),
        Some(WireError::InvalidRequest)
    );
    assert!(reader.calls.is_empty());
}

#[test]
fn library_matching_uses_full_titles_and_the_supplied_unicode_key() {
    fn fixture_key(value: &str) -> String {
        // Deliberately not a Unicode implementation: prove that the supplied
        // shared normalizer is used before complete-title matching.
        value.replace("Straße", "strasse")
    }
    let prefix = "x".repeat(180);
    let title = format!("{prefix} Straße");
    let mut reader = FixtureReader {
        libraries: Some(Ok(vec![
            library("1", &format!("{prefix} other"), "movie"),
            library("99999999999999999999", &title, "show"),
        ])),
        items: Some(Ok(ReadItems {
            metadata: vec![row("99999999999999999998", "episode")],
            truncated: false,
        })),
        ..Default::default()
    };
    let mut opts = PrepareOptions {
        normalize: fixture_key,
        permutation: None,
    };
    let request = library_request(
        PrepareKind::Preview,
        queue::LibrarySelector::Query(format!("{prefix} strasse")),
        false,
        false,
    );
    let result = prepare_with(&mut reader, &request, &mut opts, &current).unwrap();
    let Prepared::Library {
        library, selection, ..
    } = &result
    else {
        panic!("library expected")
    };
    assert_eq!(library.id, "99999999999999999999");
    assert_eq!(library.title, title);
    assert_eq!(selection.prepared.ids(), ["99999999999999999998"]);
    assert_eq!(
        reader.calls,
        ["libraries", "items:99999999999999999999:Show:false"]
    );
    let wire = result.preview_value("webos-native").unwrap();
    assert!(wire["library"]["title"].as_str().unwrap().len() <= 160);
    assert_eq!(wire["library"]["id"], "99999999999999999999");
}

#[test]
fn shuffle_runs_once_after_verified_truncation_and_keeps_metadata_unique() {
    let mut reader = FixtureReader {
        libraries: Some(Ok(vec![library("1", "Movies", "movie")])),
        items: Some(Ok(ReadItems {
            metadata: (1..=500).map(|id| row(id, "movie")).collect(),
            truncated: true,
        })),
        ..Default::default()
    };
    let calls = Cell::new(0);
    let mut order = |count| {
        calls.set(calls.get() + 1);
        assert_eq!(count, 500);
        Ok((0..count).rev().collect())
    };
    let mut opts = PrepareOptions {
        normalize: identity,
        permutation: Some(&mut order),
    };
    let request = library_request(
        PrepareKind::Preview,
        queue::LibrarySelector::Id(ids(&["1"]).remove(0)),
        true,
        true,
    );
    let result = prepare_with(&mut reader, &request, &mut opts, &current).unwrap();
    let Prepared::Library {
        truncated,
        selection,
        ..
    } = &result
    else {
        panic!("library expected")
    };
    assert!(*truncated);
    assert_eq!(calls.get(), 1);
    assert_eq!(selection.metadata.len(), 500);
    assert_eq!(selection.occurrences, (0..500).rev().collect::<Vec<_>>());
    assert_eq!(selection.prepared.records()[0].id, "500");
    assert_eq!(selection.metadata[0].rating_key, "1");
    assert_eq!(
        result.preview_value("webos-native").unwrap()["truncated"],
        true
    );
}

#[test]
fn unshuffled_library_never_calls_the_provider_and_retains_pms_order() {
    let mut reader = FixtureReader {
        libraries: Some(Ok(vec![library("1", "Movies", "movie")])),
        items: Some(Ok(ReadItems {
            metadata: vec![row(2, "movie"), row(1, "movie")],
            truncated: false,
        })),
        ..Default::default()
    };
    let mut forbidden = |_| -> Result<Vec<usize>, WireError> { panic!("unrequested shuffle") };
    let mut opts = PrepareOptions {
        normalize: identity,
        permutation: Some(&mut forbidden),
    };
    let request = library_request(
        PrepareKind::Play,
        queue::LibrarySelector::Id(ids(&["1"]).remove(0)),
        false,
        false,
    );
    let result = prepare_with(&mut reader, &request, &mut opts, &current).unwrap();
    let Prepared::Library { selection, .. } = result else {
        panic!("library expected")
    };
    assert_eq!(selection.prepared.ids(), ["2", "1"]);
    assert_eq!(selection.occurrences, [0, 1]);
}

#[test]
fn invalid_permutation_is_rejected_by_the_shared_helper() {
    for bad_order in [vec![0, 0], vec![0, 2], vec![0]] {
        let mut reader = FixtureReader {
            libraries: Some(Ok(vec![library("1", "Movies", "movie")])),
            items: Some(Ok(ReadItems {
                metadata: vec![row(1, "movie"), row(2, "movie")],
                truncated: false,
            })),
            ..Default::default()
        };
        let mut order = |_| Ok(bad_order.clone());
        let mut opts = PrepareOptions {
            normalize: identity,
            permutation: Some(&mut order),
        };
        let request = library_request(
            PrepareKind::Play,
            queue::LibrarySelector::Id(ids(&["1"]).remove(0)),
            true,
            false,
        );
        assert_eq!(
            prepare_with(&mut reader, &request, &mut opts, &current).err(),
            Some(WireError::InvalidRequest)
        );
    }
}

#[test]
fn retirement_during_permutation_cannot_return_a_proposal() {
    for error in [
        QueueError::Cancelled,
        QueueError::TimedOut,
        QueueError::ContextChanged,
    ] {
        let mut reader = FixtureReader {
            libraries: Some(Ok(vec![library("1", "Movies", "movie")])),
            items: Some(Ok(ReadItems {
                metadata: vec![row(1, "movie")],
                truncated: false,
            })),
            ..Default::default()
        };
        let retired = Cell::new(false);
        let check = || {
            if retired.get() {
                Err(WireError::Queue(error))
            } else {
                Ok(())
            }
        };
        let mut order = |_| {
            retired.set(true);
            Ok(vec![0])
        };
        let mut opts = PrepareOptions {
            normalize: identity,
            permutation: Some(&mut order),
        };
        let request = library_request(
            PrepareKind::Play,
            queue::LibrarySelector::Id(ids(&["1"]).remove(0)),
            true,
            false,
        );
        assert_eq!(
            prepare_with(&mut reader, &request, &mut opts, &check).err(),
            Some(WireError::Queue(error))
        );
    }
}

#[test]
fn every_read_error_maps_to_fixed_wire_vocabulary() {
    use remote::ReadError as E;
    let cases = [
        (
            E::InvalidIds,
            WireError::InvalidRequest,
            WireError::InvalidRequest,
        ),
        (
            E::InvalidLibrary,
            WireError::InvalidRequest,
            WireError::InvalidRequest,
        ),
        (
            E::Cancelled,
            WireError::Queue(QueueError::Cancelled),
            WireError::Queue(QueueError::Cancelled),
        ),
        (
            E::Deadline,
            WireError::Queue(QueueError::TimedOut),
            WireError::Queue(QueueError::TimedOut),
        ),
        (
            E::SourceChanged,
            WireError::Queue(QueueError::ContextChanged),
            WireError::Queue(QueueError::ContextChanged),
        ),
        (
            E::Transport,
            WireError::Queue(QueueError::ServerUnavailable),
            WireError::Queue(QueueError::ServerUnavailable),
        ),
        (
            E::Unauthorized,
            WireError::Queue(QueueError::ServerUnavailable),
            WireError::Queue(QueueError::ServerUnavailable),
        ),
        (
            E::Http,
            WireError::Queue(QueueError::ServerUnavailable),
            WireError::Queue(QueueError::ServerUnavailable),
        ),
        (
            E::NotFound,
            WireError::Queue(QueueError::MissingItem),
            WireError::Library(LibraryError::NotFound),
        ),
        (
            E::Malformed,
            WireError::Queue(QueueError::MissingItem),
            WireError::Library(LibraryError::Incomplete),
        ),
        (
            E::Incomplete,
            WireError::Queue(QueueError::MissingItem),
            WireError::Library(LibraryError::Incomplete),
        ),
        (
            E::ResponseTooLarge,
            WireError::Queue(QueueError::MissingItem),
            WireError::Library(LibraryError::Incomplete),
        ),
        (
            E::Unplayable,
            WireError::Queue(QueueError::MissingItem),
            WireError::Library(LibraryError::Incomplete),
        ),
        (
            E::UnsupportedLibrary,
            WireError::Queue(QueueError::MissingItem),
            WireError::Library(LibraryError::Unsupported),
        ),
        (
            E::TooManyItems,
            WireError::Queue(QueueError::MissingItem),
            WireError::Library(LibraryError::TooManyItems),
        ),
        (
            E::TooManyLibraries,
            WireError::Queue(QueueError::MissingItem),
            WireError::Library(LibraryError::TooManyLibraries),
        ),
    ];
    for (error, queue, library) in cases {
        assert_eq!(map_queue_read(error), queue);
        assert_eq!(map_library_read(error), library);
    }
}

#[test]
fn errors_or_ambiguous_libraries_do_not_read_items() {
    let mut reader = FixtureReader {
        libraries: Some(Err(remote::ReadError::Unauthorized)),
        ..Default::default()
    };
    let request = library_request(
        PrepareKind::Play,
        queue::LibrarySelector::Query("Movies".into()),
        false,
        false,
    );
    assert_eq!(
        prepare_with(&mut reader, &request, &mut options(), &current).err(),
        Some(WireError::Queue(QueueError::ServerUnavailable))
    );
    assert_eq!(reader.calls, ["libraries"]);
    let mut reader = FixtureReader {
        libraries: Some(Ok(vec![
            library("1", "Movies", "movie"),
            library("2", "Movies", "movie"),
        ])),
        ..Default::default()
    };
    assert_eq!(
        prepare_with(&mut reader, &request, &mut options(), &current).err(),
        Some(WireError::Library(LibraryError::Ambiguous))
    );
    assert_eq!(reader.calls, ["libraries"]);
}

#[test]
fn duplicate_full_metadata_is_rejected_even_when_projections_match() {
    let first = row(1, "movie");
    let mut second = row(1, "movie");
    second.media[0].part[0].key = "/library/parts/99/1/other.mp4".into();
    let mut reader = FixtureReader {
        metadata: Some(Ok(vec![first, second])),
        ..Default::default()
    };
    assert_eq!(
        prepare_with(
            &mut reader,
            &request(PrepareKind::Preview, &["1"]),
            &mut options(),
            &current
        )
        .err(),
        Some(WireError::Queue(QueueError::MissingItem))
    );
}

#[test]
fn missing_parts_and_unrequested_metadata_cannot_survive_projection() {
    let mut missing = row(1, "movie");
    missing.media.clear();
    for rows in [vec![missing], vec![row(1, "movie"), row(2, "movie")]] {
        let mut reader = FixtureReader {
            metadata: Some(Ok(rows)),
            ..Default::default()
        };
        assert_eq!(
            prepare_with(
                &mut reader,
                &request(PrepareKind::Preview, &["1"]),
                &mut options(),
                &current
            )
            .err(),
            Some(WireError::Queue(QueueError::MissingItem))
        );
    }
}

#[test]
fn a_retired_request_does_not_touch_the_reader() {
    let mut reader = FixtureReader::default();
    let cancelled = || Err(WireError::Queue(QueueError::Cancelled));
    assert_eq!(
        prepare_with(
            &mut reader,
            &request(PrepareKind::Play, &["1"]),
            &mut options(),
            &cancelled
        )
        .err(),
        Some(WireError::Queue(QueueError::Cancelled))
    );
    assert!(reader.calls.is_empty());
}

#[test]
fn authority_retirement_during_normalization_prevents_item_reads() {
    thread_local! { static RETIRED: Cell<bool> = const { Cell::new(false) }; }
    fn retiring_key(value: &str) -> String {
        RETIRED.with(|flag| flag.set(true));
        value.to_owned()
    }
    RETIRED.with(|flag| flag.set(false));
    let mut reader = FixtureReader {
        libraries: Some(Ok(vec![library("1", "Movies", "movie")])),
        items: Some(Ok(ReadItems {
            metadata: vec![row(1, "movie")],
            truncated: false,
        })),
        ..Default::default()
    };
    let check = || {
        if RETIRED.with(Cell::get) {
            Err(WireError::Queue(QueueError::ContextChanged))
        } else {
            Ok(())
        }
    };
    let mut opts = PrepareOptions {
        normalize: retiring_key,
        permutation: None,
    };
    let request = library_request(
        PrepareKind::Play,
        queue::LibrarySelector::Query("Movies".into()),
        false,
        false,
    );
    assert_eq!(
        prepare_with(&mut reader, &request, &mut opts, &check).err(),
        Some(WireError::Queue(QueueError::ContextChanged))
    );
    assert_eq!(reader.calls, ["libraries"]);
    assert!(reader.items.is_some());
    RETIRED.with(|flag| flag.set(false));
}
