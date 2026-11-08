//! Synthetic immutable storage observations. These are protocol/ownership
//! regressions, not proof that protected storage was read or written on a device.
use super::*;

fn row(number: u32) -> DurableStatus {
    DurableStatus {
        operation_id: format!("{number:08x}-0000-4000-8000-000000000001"),
        operation_runtime: Some("old-runtime".into()),
        transaction_id: Some(format!("{number:08x}-0000-4000-8000-000000000002")),
        outcome: Outcome::Unknown,
        state: "unknown".into(),
        receipt: None,
    }
}
fn state(count: u32) -> Saved {
    let mut state = Saved::default();
    state.receipts = (1..=count).rev().map(row).collect();
    state
}
fn inspect(state: &Saved, query: &Query) -> Result<Observation> {
    inspect_value(
        Some(&serde_json::to_value(state).unwrap()),
        Some(&"1".repeat(64)),
        query,
        &"2".repeat(64),
    )
}
fn page_request(cursor: Value) -> Value {
    json!({"op":"inspect","runtime":"fixture","subject":"global","cursor":cursor})
}
fn receipt(status: &DurableStatus) -> Receipt {
    Receipt {
        changed: false,
        context_revision: "3".repeat(64),
        counts: wire::Counts::default(),
        expected_revision: "4".repeat(64),
        input_document_sha256: "4".repeat(64),
        new_revision: "4".repeat(64),
        operation_id: status.operation_id.clone(),
        operation_runtime: status.operation_runtime.clone().unwrap(),
        outcome: status.outcome,
        persistence: "persistent".into(),
        plan_digest: "5".repeat(64),
        required_kinds: vec![],
        result_document_sha256: "4".repeat(64),
        subject: "global".into(),
        transaction_id: status.transaction_id.clone().unwrap(),
        version: 1,
    }
}

#[test]
fn context_and_original_row_have_frozen_preimages() {
    const CONTEXT: &str = r#"["ottplay-native-favorites-recovery-context-v1","namespace-value-v1","ott-state",1,"com.example.fixture","dev_fixture"]"#;
    assert_eq!(
        wire::hash(CONTEXT.as_bytes()),
        "4970613fa0d12049e5720790aac07cd4217d3a633aec88d26602d98b2ea29417"
    );
    assert_eq!(
        context("com.example.fixture", "dev_fixture"),
        wire::hash(CONTEXT.as_bytes())
    );
    let mut original = row(1);
    original.transaction_id = Some("10000001-0000-4000-8000-000000000001".into());
    const ROW: &str = r#"{"operation_id":"00000001-0000-4000-8000-000000000001","operation_runtime":"old-runtime","transaction_id":"10000001-0000-4000-8000-000000000001","outcome":"unknown","state":"unknown","receipt":null}"#;
    assert_eq!(serde_json::to_vec(&original).unwrap(), ROW.as_bytes());
    assert_eq!(
        wire::hash(ROW.as_bytes()),
        "02710a44110b47cec5c8a927a36c19a0b2b4a02e3d952157879fbaaa7ea9ce04"
    );
    let mut saved = Saved::default();
    saved.receipts.push(original);
    assert_eq!(
        records(&saved).unwrap()[0]["original_entry_sha256"],
        "02710a44110b47cec5c8a927a36c19a0b2b4a02e3d952157879fbaaa7ea9ce04"
    );
    assert_ne!(
        context("another.app", "dev_fixture"),
        context("com.example.fixture", "dev_fixture")
    );
    assert_ne!(
        context("com.example.fixture", "dev_other"),
        context("com.example.fixture", "dev_fixture")
    );
}

#[test]
fn absence_is_only_the_physical_none_case() {
    let result = inspect_value(None, None, &Query::Page(None), &"2".repeat(64)).unwrap();
    assert_eq!(result.data["ledger_revision"], ABSENT_SHA256);
    assert_eq!(result.data["store_revision"], ABSENT_SHA256);
    assert_eq!(result.data["total"], 0);
    assert!(result.data["backend_proof"].is_null());
    let existing = inspect(&Saved::default(), &Query::Page(None)).unwrap();
    assert_eq!(existing.data["ledger_revision"], "1".repeat(64));
    assert_ne!(
        existing.data["ledger_revision"],
        result.data["ledger_revision"]
    );
    assert!(matches!(
        inspect_value(None, Some(&"1".repeat(64)), &Query::Page(None), "context"),
        Err("storage_unavailable")
    ));
    assert!(matches!(
        inspect_value(
            Some(&json!(null)),
            Some(&"1".repeat(64)),
            &Query::Page(None),
            "context"
        ),
        Err("unsupported_persistence")
    ));
}

#[test]
fn complete_zero_sixteen_seventeen_and_thirty_two_row_pages() {
    for count in [0, 16, 17, 32] {
        let saved = state(count);
        let first = inspect(&saved, &Query::Page(None)).unwrap().data;
        assert_eq!(first["total"], count);
        let rows = first["records"].as_array().unwrap();
        assert_eq!(rows.len(), (count as usize).min(16));
        assert!(rows
            .windows(2)
            .all(|pair| pair[0]["operation_id"].as_str() < pair[1]["operation_id"].as_str()));
        if count <= 16 {
            assert!(first["next_cursor"].is_null());
        } else {
            let query = parse(&page_request(first["next_cursor"].clone())).unwrap();
            let second = inspect(&saved, &query).unwrap().data;
            assert_eq!(
                second["records"].as_array().unwrap().len(),
                count as usize - 16
            );
            assert!(second["next_cursor"].is_null());
            assert_eq!(second["records"][0]["operation_id"], row(17).operation_id);
            for key in [
                "ledger_revision",
                "store_revision",
                "context_revision",
                "backend_proof",
            ] {
                assert_eq!(first[key], second[key]);
            }
        }
    }
}

#[test]
fn continuation_rechecks_every_revision_and_exact_boundary() {
    let saved = state(32);
    let first = inspect(&saved, &Query::Page(None)).unwrap().data;
    for (key, value, expected) in [
        (
            "ledger_revision",
            json!("8".repeat(64)),
            "revision_conflict",
        ),
        ("store_revision", json!("8".repeat(64)), "revision_conflict"),
        ("context_revision", json!("8".repeat(64)), "context_changed"),
        ("backend_proof", json!("8".repeat(64)), "context_changed"),
        (
            "after_operation_id",
            json!(row(15).operation_id),
            "invalid_request",
        ),
        (
            "after_operation_id",
            json!(row(32).operation_id),
            "invalid_request",
        ),
    ] {
        let mut cursor = first["next_cursor"].clone();
        cursor[key] = value;
        assert_eq!(
            inspect(&saved, &parse(&page_request(cursor)).unwrap()).err(),
            Some(expected)
        );
    }
    let query = parse(&page_request(first["next_cursor"].clone())).unwrap();
    let physical = serde_json::to_value(saved).unwrap();
    assert_eq!(
        inspect_value(
            Some(&physical),
            Some(&"9".repeat(64)),
            &query,
            &"2".repeat(64)
        )
        .err(),
        Some("revision_conflict")
    );
}

#[test]
fn selected_unknown_and_missing_are_both_ineligible_for_ack() {
    let mut saved = state(1);
    let mut terminal = row(2);
    terminal.state = "aborted".into();
    terminal.outcome = Outcome::NotApplied;
    saved.receipts.push(terminal);
    for number in [1, 2, 3] {
        let reply = inspect(&saved, &Query::Selected(row(number).operation_id))
            .unwrap()
            .data;
        assert_eq!(reply["acknowledge_allowed"], false);
        for key in [
            "backend_proof",
            "acknowledgement_id",
            "after_ledger_sha256",
            "plan_sha256",
        ] {
            assert!(reply[key].is_null());
        }
        assert_eq!(reply["record"].is_null(), number != 1);
    }
    let original = records(&saved).unwrap()[0].clone();
    saved.receipts[0].receipt = Some(receipt(&saved.receipts[0]));
    assert_ne!(
        records(&saved).unwrap()[0]["original_entry_sha256"],
        original["original_entry_sha256"]
    );
    assert_eq!(records(&saved).unwrap()[0].as_object().unwrap().len(), 7);
}

#[test]
fn malformed_terminal_or_duplicate_rows_fail_the_whole_journal() {
    let mut saved = state(1);
    let mut terminal = row(2);
    terminal.outcome = Outcome::Verified;
    terminal.state = "committed".into();
    terminal.receipt = Some(receipt(&terminal));
    saved.receipts.push(terminal);
    assert_eq!(records(&saved).unwrap().len(), 1);
    for change in 0..7 {
        let mut damaged = saved.clone();
        match change {
            0 => damaged.receipts[1].operation_id = damaged.receipts[0].operation_id.clone(),
            1 => damaged.receipts[1].operation_runtime = None,
            2 => damaged.receipts[1].transaction_id = Some("not-a-uuid".into()),
            3 => damaged.receipts[1].receipt = None,
            4 => damaged.receipts[1].receipt.as_mut().unwrap().subject = "local".into(),
            5 => damaged.receipts[1].receipt.as_mut().unwrap().operation_id = row(9).operation_id,
            _ => damaged.receipts[1].receipt.as_mut().unwrap().new_revision = "0".repeat(64),
        }
        assert_eq!(
            inspect(&damaged, &Query::Page(None)).err(),
            Some("storage_unavailable")
        );
    }
}

#[test]
fn receipt_counts_kinds_and_original_correlations_are_validated() {
    let mut saved = state(1);
    saved.receipts[0].receipt = Some(receipt(&saved.receipts[0]));
    assert!(records(&saved).is_ok());
    for change in 0..6 {
        let mut damaged = saved.clone();
        let receipt = damaged.receipts[0].receipt.as_mut().unwrap();
        match change {
            0 => receipt.counts.retained = 1,
            1 => receipt.counts.duplicates_skipped = 1001,
            2 => receipt.required_kinds = vec!["local-media".into(), "local-media".into()],
            3 => receipt.required_kinds = vec!["unknown".into()],
            4 => receipt.outcome = Outcome::Verified,
            _ => receipt.changed = true,
        }
        assert_eq!(records(&damaged).err(), Some("storage_unavailable"));
    }
}

#[test]
fn bounds_and_unsupported_saved_shape_never_become_empty_success() {
    assert_eq!(
        inspect(&state(33), &Query::Page(None)).err(),
        Some("limits_exceeded")
    );
    let mut saved = state(1);
    saved.receipts[0].state = "x".repeat(JOURNAL_BYTES);
    assert_eq!(records(&saved).err(), Some("limits_exceeded"));
    let mut value = serde_json::to_value(state(0)).unwrap();
    value["version"] = json!(2);
    assert_eq!(
        inspect_value(
            Some(&value),
            Some(&"1".repeat(64)),
            &Query::Page(None),
            "context"
        )
        .err(),
        Some("unsupported_persistence")
    );
    value["version"] = json!(1);
    value["unexpected"] = json!(true);
    assert_eq!(
        inspect_value(
            Some(&value),
            Some(&"1".repeat(64)),
            &Query::Page(None),
            "context"
        )
        .err(),
        Some("storage_unavailable")
    );
}

#[test]
fn closed_request_schema_rejects_extra_keys_and_bad_scalar_types() {
    for request in [
        json!({}),
        json!({"op":"inspect","runtime":"fixture","subject":"local","cursor":null}),
        json!({"op":"inspect","runtime":"fixture","subject":"global","cursor":null,"operation_id":row(1).operation_id}),
        json!({"op":"inspect","runtime":"fixture","subject":"global","cursor":false}),
        json!({"op":"inspect","runtime":"fixture","subject":"global","operation_id":"not-uuid"}),
        json!({"op":"inspect","runtime":"bad/runtime","subject":"global","cursor":null}),
        json!({"op":"inspect","runtime":"fixture","subject":"global","cursor":null,"operation":"inspect"}),
    ] {
        assert!(matches!(parse(&request), Err("invalid_request")));
    }
    let mut huge = page_request(Value::Null);
    huge["unused"] = json!("x".repeat(ENVELOPE_BYTES));
    assert!(matches!(parse(&huge), Err("envelope_too_large")));
}

#[test]
fn unrelated_navigation_drift_does_not_change_recovery_relevance() {
    let saved = state(1);
    let mut live = saved.clone();
    live.category = "another category".into();
    live.favorites_selected = true;
    live.resume.insert(
        "other-profile".into(),
        SavedResume {
            entry_id: "other-item".into(),
            position_ms: 3000,
            source_fingerprint: String::new(),
        },
    );
    live.profiles.active_profile = Some("gone-profile".into());
    assert_eq!(relevant(&saved).unwrap(), relevant(&live).unwrap());
    live.receipts[0].state = "pending".into();
    assert_ne!(relevant(&saved).unwrap(), relevant(&live).unwrap());
}

#[cfg(feature = "hostsim")]
mod runtime_tests {
    use super::*;
    fn runtime() -> Runtime {
        let mut runtime =
            Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
        runtime.loaded = true;
        runtime.clock_sample = Some(
            plx_base::task::time::ClockSample::capture(&plx_base::task::OffFrame::for_test())
                .unwrap(),
        );
        runtime
    }
    fn request(runtime: &mut Runtime, serial: u32) -> Invocation {
        let now = runtime.now_ms();
        let raw = json!({"commands":[],"server_time":1000,"requests":[{"id":format!("{serial:032x}"),
            "action":"favorites_global_recovery","params":page_request(Value::Null),"expires_at":1020}]}).to_string();
        let mut invocations = runtime.remote.raw_poll_for_test(&raw, now);
        assert_eq!(invocations.len(), 1);
        let invocation = invocations.pop().unwrap();
        assert!(runtime.remote.invocation_is_current(&invocation));
        assert_eq!(invocation.deadline_ms, now + 20_000);
        invocation
    }
    fn held(runtime: &mut Runtime, invocation: &Invocation) -> Work {
        let mut captured = None;
        assert!(command_with(runtime, invocation, |work, _, _| {
            captured = Some(work);
            true
        })
        .is_none());
        assert!(!runtime.favorites_recovery_done.load(Ordering::Acquire));
        captured.unwrap()
    }
    fn complete(runtime: &mut Runtime, work: Work, saved: &Saved) -> Value {
        let id = work.invocation.id.clone();
        let result = inspect_value(
            Some(&serde_json::to_value(saved).unwrap()),
            Some(&"1".repeat(64)),
            &work.query,
            &work.context,
        );
        finish(runtime, Completion { work, result });
        runtime.remote.response_for_id_for_test(&id).unwrap()
    }
    #[test]
    fn held_reader_blocks_overlap_and_landing_does_not_release_worker_early() {
        let _serial = plx_base::testlock::serial();
        let mut runtime = runtime();
        let first = request(&mut runtime, 1);
        let work = held(&mut runtime, &first);
        let second = request(&mut runtime, 2);
        let reply =
            command_with(&mut runtime, &second, |_, _, _| panic!("overlapping read")).unwrap();
        assert_eq!(reply["data"]["error"], "busy");
        let saved = runtime.saved.clone();
        assert_eq!(complete(&mut runtime, work, &saved)["status"], "ok");
        assert!(!runtime.favorites_recovery_done.load(Ordering::Acquire));
        runtime
            .favorites_recovery_done
            .store(true, Ordering::Release); // actual worker terminal in this held fixture
        let third = request(&mut runtime, 3);
        let reply = command_with(&mut runtime, &third, |_, _, _| false).unwrap();
        assert_eq!(reply["data"]["error"], "storage_unavailable");
        assert!(runtime.favorites_recovery_done.load(Ordering::Acquire));
    }
    #[test]
    fn landing_rechecks_physical_and_live_membership_and_entire_journal() {
        let _serial = plx_base::testlock::serial();
        for physical_changed in [false, true] {
            let mut runtime = runtime();
            runtime.saved = state(1);
            let request = request(&mut runtime, 1);
            let work = held(&mut runtime, &request);
            let mut physical = runtime.saved.clone();
            if physical_changed {
                physical.receipts.push(row(2));
            } else {
                runtime.saved.receipts.push(row(2));
            }
            assert_eq!(
                complete(&mut runtime, work, &physical)["data"]["error"],
                "revision_conflict"
            );
        }
        let mut runtime = runtime();
        let request = request(&mut runtime, 1);
        let work = held(&mut runtime, &request);
        let physical = runtime.saved.clone();
        runtime.saved.category = "new-category".into();
        let result = complete(&mut runtime, work, &physical);
        assert_eq!(result["status"], "ok");
        assert_eq!(runtime.saved.category, "new-category");
    }
    #[test]
    fn late_writer_uncertainty_authority_or_expiry_cannot_publish_success() {
        let _serial = plx_base::testlock::serial();
        for variant in 0..4 {
            let mut runtime = runtime();
            let request = request(&mut runtime, 1);
            let mut work = held(&mut runtime, &request);
            let physical = runtime.saved.clone();
            let expected = match variant {
                0 => {
                    runtime.saving = true;
                    "busy"
                }
                1 => {
                    runtime.persistence_uncertain = true;
                    "outcome_unknown"
                }
                2 => {
                    runtime.remote = Remote::disabled_for_test("dev_other", "fixture");
                    "context_changed"
                }
                _ => {
                    work.invocation.deadline_ms = runtime.now_ms();
                    "expired"
                }
            };
            let result = complete(&mut runtime, work, &physical);
            assert_eq!(result["status"], "rejected");
            assert_eq!(result["data"]["error"], expected);
            assert_eq!(result["data"].as_object().unwrap().len(), 5);
        }
    }
}
