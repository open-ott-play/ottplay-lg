use super::*;
fn record(source: &str, id: &str, start_s: i64) -> Record {
    Record {
        key: Key {
            source_id: source.into(),
            channel_id: "channel-a".into(),
            programme_id: id.into(),
            start_s,
        },
        end_s: start_s + 120,
        title: "Programme".into(),
    }
}
fn context() -> Context<'static> {
    Context {
        source_id: "source-a",
        revision: 1,
        active: true,
    }
}
fn book(records: Vec<Record>, now: i64) -> Book {
    Book::new(
        Document {
            version: 1,
            records,
        },
        now,
        7,
    )
    .unwrap()
}
fn prompt(book: &mut Book, now: i64) -> PromptToken {
    book.poll(now, context(), 0, |_| true)
        .unwrap()
        .into_iter()
        .find_map(|e| match e {
            Fx::PromptSwitch { token, .. } => Some(token),
            _ => None,
        })
        .unwrap()
}
#[test]
fn roundtrip_keeps_source_channel_programme_identity_without_positions() {
    let doc = Document {
        version: 1,
        records: vec![record("source-a", "programme-a", 100)],
    };
    assert_eq!(Document::parse(&doc.encode().unwrap()).unwrap(), doc);
    let value = serde_json::to_value(doc).unwrap();
    assert!(value["records"][0]["key"].get("position").is_none());
}
#[test]
fn invalid_or_duplicate_records_are_not_silently_empty() {
    let r = record("source-a", "p", 100);
    let duplicate = serde_json::to_vec(&Document {
        version: 1,
        records: vec![r.clone(), r],
    })
    .unwrap();
    assert_eq!(Document::parse(&duplicate), Err(Error::InvalidDocument));
    assert_eq!(Document::parse(b"not json"), Err(Error::InvalidDocument));
    assert_eq!(
        Document::parse(br#"{"version":2,"records":[]}"#),
        Err(Error::InvalidDocument)
    );
}
#[test]
fn bounded_document_and_identifiers() {
    assert_eq!(
        Document::parse(&vec![b' '; DOCUMENT_BYTES + 1]),
        Err(Error::TooLarge)
    );
    let mut r = record("source-a", "p", 100);
    r.key.channel_id = "x".repeat(ID_BYTES + 1);
    assert!(book(Vec::new(), 0).upsert(r, 0).is_err());
    let records = (0..=RECORD_LIMIT)
        .map(|i| record("source-a", &i.to_string(), 100))
        .collect();
    assert!(matches!(
        Book::new(
            Document {
                version: 1,
                records
            },
            0,
            7
        ),
        Err(Error::TooLarge)
    ));
}
#[test]
fn full_reminder_store_refuses_addition_without_losing_rows_or_blocking_replacement() {
    let records = (0..RECORD_LIMIT)
        .map(|i| record("source-a", &i.to_string(), 100))
        .collect();
    let mut b = book(records, 0);
    let committed = b.document().clone();
    assert!(matches!(
        b.upsert(record("source-a", "one-too-many", 100), 0),
        Err(Error::TooLarge)
    ));
    assert_eq!(b.document(), &committed);
    assert!(b.pending.is_none());

    let mut replacement = record("source-a", "0", 100);
    replacement.title = "Updated title".into();
    let candidate = b.upsert(replacement.clone(), 0).unwrap();
    assert_eq!(candidate.document.records().len(), RECORD_LIMIT);
    assert_eq!(b.document(), &committed);
    b.commit(candidate.token, CommitOutcome::Committed).unwrap();
    assert_eq!(b.document().records().len(), RECORD_LIMIT);
    assert!(b.document().records().contains(&replacement));
}
#[test]
fn save_acknowledgement_is_required_before_scheduling() {
    let mut b = book(Vec::new(), 0);
    let candidate = b.upsert(record("source-a", "p", 100), 0).unwrap();
    assert!(b.document().records().is_empty());
    assert!(b.poll(100, context(), 0, |_| true).unwrap().is_empty());
    b.commit(candidate.token, CommitOutcome::Committed).unwrap();
    assert_eq!(b.document().records().len(), 1);
    assert!(matches!(
        &b.poll(100, context(), 0, |_| true).unwrap()[0],
        Fx::PromptSwitch { .. }
    ));
}
#[test]
fn rejected_save_does_not_replace_active_document() {
    let mut b = book(vec![record("source-a", "old", 100)], 0);
    let candidate = b.upsert(record("source-a", "new", 200), 0).unwrap();
    b.commit(candidate.token, CommitOutcome::Rejected).unwrap();
    assert_eq!(b.document().records().len(), 1);
    assert_eq!(b.document().records()[0].key.programme_id, "old");
    assert!(b.writable());
}
#[test]
fn unknown_save_closes_prompt_and_requires_authoritative_reload() {
    let r = record("source-a", "p", 100);
    let mut b = book(vec![r.clone()], 0);
    let token = prompt(&mut b, 100);
    let candidate = b.remove(&r.key).unwrap();
    assert_eq!(
        b.commit(candidate.token, CommitOutcome::Unknown).unwrap(),
        vec![Fx::ClosePrompt(token)]
    );
    assert!(!b.writable());
    assert_eq!(b.remove(&r.key), Err(Error::ReadOnly));
    assert!(b.accept(token, 101, context(), true).is_none());
}
#[test]
fn single_save_flight_and_stale_completion_cannot_publish() {
    let mut b = book(Vec::new(), 0);
    let first = b.upsert(record("source-a", "a", 100), 0).unwrap();
    assert_eq!(b.upsert(record("source-a", "b", 200), 0), Err(Error::Busy));
    b.commit(first.token, CommitOutcome::Rejected).unwrap();
    let second = b.upsert(record("source-a", "b", 200), 0).unwrap();
    assert_eq!(
        b.commit(first.token, CommitOutcome::Committed),
        Err(Error::Stale)
    );
    b.commit(second.token, CommitOutcome::Committed).unwrap();
    assert_eq!(b.document().records()[0].key.programme_id, "b");
}
#[test]
fn reload_does_not_replay_a_programme_already_started() {
    let mut b = book(vec![record("source-a", "p", 100)], 101);
    assert!(b.poll(101, context(), 60, |_| true).unwrap().is_empty());
}
#[test]
fn lead_occurs_once_and_start_requires_explicit_acceptance() {
    let mut b = book(vec![record("source-a", "p", 100)], 0);
    assert!(b.poll(39, context(), 60, |_| true).unwrap().is_empty());
    let notice = b.poll(40, context(), 60, |_| true).unwrap();
    assert!(matches!(&notice[0], Fx::LeadNotice { minutes: 1, .. }));
    assert!(b.poll(99, context(), 60, |_| true).unwrap().is_empty());
    let token = prompt(&mut b, 100);
    assert!(b.accept(token, 100, context(), true).is_some());
    assert!(b.accept(token, 100, context(), true).is_none());
    assert!(b.poll(101, context(), 60, |_| true).unwrap().is_empty());
}
#[test]
fn missing_channel_does_not_start_a_different_list_position() {
    let mut b = book(vec![record("source-a", "p", 100)], 0);
    assert!(b.poll(100, context(), 0, |_| false).unwrap().is_empty());
    let token = prompt(&mut b, 101);
    assert!(b.accept(token, 101, context(), false).is_none());
}
#[test]
fn source_or_catalog_change_retires_old_prompt() {
    for changed in [
        Context {
            source_id: "source-b",
            ..context()
        },
        Context {
            revision: 2,
            ..context()
        },
    ] {
        let mut b = book(vec![record("source-a", "p", 100)], 0);
        let token = prompt(&mut b, 100);
        assert_eq!(
            b.poll(101, changed, 0, |_| true).unwrap(),
            vec![Fx::ClosePrompt(token)]
        );
        assert!(b.accept(token, 101, context(), true).is_none());
    }
}
#[test]
fn standby_never_wakes_and_resume_skips_missed_start() {
    let mut b = book(vec![record("source-a", "p", 100)], 0);
    assert!(b
        .poll(
            100,
            Context {
                active: false,
                ..context()
            },
            0,
            |_| true
        )
        .unwrap()
        .is_empty());
    assert!(b.poll(101, context(), 0, |_| true).unwrap().is_empty());
}
#[test]
fn expired_or_rewound_prompt_cannot_be_accepted() {
    for now in [99, 220] {
        let mut b = book(vec![record("source-a", "p", 100)], 0);
        let token = prompt(&mut b, 100);
        assert!(b.accept(token, now, context(), true).is_none());
        assert_eq!(
            b.poll(now, context(), 0, |_| true).unwrap(),
            vec![Fx::ClosePrompt(token)]
        );
        assert!(b.poll(100, context(), 0, |_| true).unwrap().is_empty());
    }
}
#[test]
fn simultaneous_programmes_use_one_prompt_and_deterministic_order() {
    let mut b = book(
        vec![record("source-a", "b", 100), record("source-a", "a", 100)],
        0,
    );
    let effects = b.poll(100, context(), 0, |_| true).unwrap();
    let Fx::PromptSwitch { token, record } = &effects[0] else {
        panic!("expected prompt")
    };
    assert_eq!(effects.len(), 1);
    assert_eq!(record.key.programme_id, "a");
    assert!(b.dismiss(*token));
    let Fx::PromptSwitch { record, .. } = &b.poll(101, context(), 0, |_| true).unwrap()[0]
    else {
        panic!("expected next prompt")
    };
    assert_eq!(record.key.programme_id, "b");
}
#[test]
fn committed_removal_invalidates_existing_accept_token() {
    let r = record("source-a", "p", 100);
    let mut b = book(vec![r.clone()], 0);
    let token = prompt(&mut b, 100);
    let candidate = b.remove(&r.key).unwrap();
    assert_eq!(
        b.commit(candidate.token, CommitOutcome::Committed).unwrap(),
        vec![Fx::ClosePrompt(token)]
    );
    assert!(b.accept(token, 101, context(), true).is_none());
}
#[test]
fn old_book_receipt_and_prompt_cannot_cross_reload() {
    let r = record("source-a", "p", 100);
    let mut old = book(vec![r.clone()], 0);
    let old_prompt = prompt(&mut old, 100);
    let old_save = old.remove(&r.key).unwrap();
    let mut new = Book::new(
        Document {
            version: 1,
            records: vec![r.clone()],
        },
        0,
        8,
    )
    .unwrap();
    let new_save = new.remove(&r.key).unwrap();
    assert_eq!(
        new.commit(old_save.token, CommitOutcome::Committed),
        Err(Error::Stale)
    );
    new.commit(new_save.token, CommitOutcome::Rejected).unwrap();
    let new_prompt = prompt(&mut new, 100);
    assert_ne!(old_prompt, new_prompt);
    assert!(new.accept(old_prompt, 100, context(), true).is_none());
}
#[test]
fn invalid_time_and_nonfuture_additions_are_rejected() {
    let mut b = book(Vec::new(), 0);
    assert_eq!(
        b.upsert(record("source-a", "p", 100), 100),
        Err(Error::NotFuture)
    );
    assert_eq!(b.poll(-1, context(), 0, |_| true), Err(Error::InvalidTime));
    assert_eq!(
        b.poll(0, context(), LEAD_SECONDS_LIMIT + 1, |_| true),
        Err(Error::InvalidTime)
    );
}
#[test]
fn legacy_import_binds_source_and_resolves_identity_not_current_position() {
    let bytes = br#"[{"ci":42,"t":100,"te":220,"n":"Programme"}]"#;
    let imported = migrate_legacy("source-a", None, bytes, |id, start| {
        assert_eq!(id, "42");
        Some(record("source-a", "stable-programme", start).key)
    })
    .unwrap();
    assert_eq!(imported.claim_source, "source-a");
    assert_eq!(imported.document.records()[0].key.channel_id, "channel-a");
    assert_eq!(
        migrate_legacy("source-a", Some("source-b"), bytes, |_, _| panic!(
            "foreign source must not resolve"
        )),
        Err(Error::ForeignSource)
    );
}
#[test]
fn unresolved_or_mismatched_legacy_import_cannot_claim_or_erase_input() {
    let bytes = br#"[{"ci":42,"t":100,"te":220,"n":"Programme"}]"#;
    assert_eq!(
        migrate_legacy("source-a", None, bytes, |_, _| None),
        Err(Error::UnresolvedLegacy)
    );
    assert_eq!(
        migrate_legacy("source-a", None, bytes, |_, start| Some(
            record("source-b", "p", start).key
        )),
        Err(Error::ForeignSource)
    );
}
#[test]
fn pending_removal_blocks_acceptance_until_the_known_commit_result() {
    let r = record("source-a", "p", 100);
    let mut b = book(vec![r.clone()], 0);
    let token = prompt(&mut b, 100);
    let candidate = b.remove(&r.key).unwrap();
    assert!(b.accept(token, 101, context(), true).is_none());
    b.commit(candidate.token, CommitOutcome::Rejected).unwrap();
    assert!(b.accept(token, 101, context(), true).is_some());
}
