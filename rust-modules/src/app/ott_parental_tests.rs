use super::*;
use serde_json::json;

// Public, synthetic vector: PIN 2468, salt bytes 0..15. Cross-checked using
// Python hashlib.pbkdf2_hmac("sha256", ..., 600000), not a household record.
const RECORD: &str = "pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65";

fn settings() -> Settings {
    BTreeMap::from([("parentPin".into(), json!(RECORD))])
}
fn context(settings: &Settings) -> Context {
    Context::new(1, Policy::read(settings).unwrap())
}
fn gate() -> Gate {
    Gate::new([7; 16])
}
fn input() -> Pin {
    Pin::parse("2468").unwrap()
}
// State-machine fixtures model a retired worker's addressed result. Actual
// PBKDF2 and constant-time comparison are exercised separately below.
fn result(work: Work, matched: bool) -> Completion {
    Completion {
        ticket: work.ticket(),
        result: WorkResult::Verified(matched),
    }
}
fn authorize(gate: &mut Gate, context: &Context, now: u64) {
    let work = gate.verify(context, input(), now).unwrap();
    assert!(matches!(
        gate.complete(context, result(work, true), now),
        Outcome::Granted
    ));
}
fn created(work: Work) -> Completion {
    Completion {
        ticket: work.ticket(),
        result: WorkResult::Created(Record::parse(RECORD).unwrap()),
    }
}

#[test]
fn accepts_only_exact_four_ascii_digits_and_the_versioned_record() {
    for pin in ["0000", "2468", "9999"] {
        assert!(Pin::parse(pin).is_ok());
    }
    for pin in [
        "",
        "123",
        "12345",
        " 2468",
        "2468\n",
        "１２３４",
        "abcd",
        "24\0x",
    ] {
        assert!(matches!(Pin::parse(pin), Err(Error::InvalidPin)));
    }
    assert!(Record::parse(RECORD).is_ok());
    let invalid = [
        "2468".into(),
        "*".into(),
        RECORD.to_uppercase(),
        format!("{RECORD}\n"),
        RECORD.replace("600000", "20000"),
        RECORD.replace("600000", "0600000"),
        RECORD.replace("00010203", "gg010203"),
        RECORD[..RECORD.len() - 1].into(),
        format!("{PREFIX}{}${}", "é".repeat(16), "0".repeat(64)),
        "x".repeat(2048),
    ];
    for record in invalid {
        assert!(matches!(Record::parse(&record), Err(Error::InvalidRecord)));
    }
}

#[test]
fn missing_pin_is_disabled_but_malformed_or_plaintext_never_resets_policy() {
    let empty = Settings::new();
    let policy = Policy::read(&empty).unwrap();
    assert!(policy.record.is_none());
    assert!(policy.channels && !policy.settings && !policy.providers);
    let mut map = empty.clone();
    map.insert("parentPin".into(), json!("*"));
    assert!(Policy::read(&map).unwrap() == policy);
    for value in [Value::Null, json!(2468), json!("2468"), json!("invalid")] {
        map.insert("parentPin".into(), value);
        assert!(matches!(Policy::read(&map), Err(Error::InvalidRecord)));
    }
    for key in ["psChannels", "psOptions", "requirePinForProviderSelection"] {
        for value in [
            Value::Null,
            json!(true),
            json!(2),
            json!(-1),
            json!(1.0),
            json!("1"),
        ] {
            let mut map = settings();
            map.insert(key.into(), value);
            assert!(matches!(Policy::read(&map), Err(Error::InvalidPolicy)));
        }
    }
    let mut map = settings();
    map.insert(REVISION.into(), json!("0"));
    assert!(matches!(Policy::read(&map), Err(Error::InvalidRevision)));
}

#[test]
fn classic_policy_protects_only_marked_channels_and_explicit_settings_providers() {
    let mut map = settings();
    let mut gate = gate();
    let ctx = context(&map);
    assert_eq!(
        gate.access(&ctx, Target::Channel { marked: true }, 0),
        Access::PinRequired
    );
    for target in [
        Target::Channel { marked: false },
        Target::Settings,
        Target::Providers,
    ] {
        assert_eq!(gate.access(&ctx, target, 0), Access::Allowed);
    }
    assert_eq!(gate.access(&ctx, Target::Control, 0), Access::PinRequired);
    map.insert("psChannels".into(), json!(0));
    map.insert("psOptions".into(), json!(1));
    map.insert("requirePinForProviderSelection".into(), json!(1));
    let ctx = context(&map);
    assert_eq!(
        gate.access(&ctx, Target::Channel { marked: true }, 0),
        Access::Allowed
    );
    assert_eq!(gate.access(&ctx, Target::Settings, 0), Access::PinRequired);
    assert_eq!(gate.access(&ctx, Target::Providers, 0), Access::PinRequired);
}

// Real work-factor verification is exercised once through the Runtime editor/worker test.

#[test]
fn grant_expires_after_one_hour_and_clock_rollback_revokes_it() {
    let ctx = context(&settings());
    let mut gate = gate();
    authorize(&mut gate, &ctx, 100);
    assert_eq!(
        gate.access(&ctx, Target::Control, 100 + GRANT_MS - 1),
        Access::Allowed
    );
    assert_eq!(
        gate.access(&ctx, Target::Control, 100 + GRANT_MS),
        Access::PinRequired
    );
    authorize(&mut gate, &ctx, 200 + GRANT_MS);
    assert_eq!(
        gate.access(&ctx, Target::Control, 199 + GRANT_MS),
        Access::PinRequired
    );
}

#[test]
fn cancellation_holds_the_worker_slot_until_its_actual_completion() {
    let ctx = context(&settings());
    let mut gate = gate();
    let work = gate.verify(&ctx, input(), 0).unwrap();
    gate.cancel();
    assert!(matches!(gate.verify(&ctx, input(), 1), Err(Error::Busy)));
    assert!(matches!(
        gate.complete(&ctx, result(work, true), 2),
        Outcome::Ignored
    ));
    assert_eq!(gate.access(&ctx, Target::Control, 2), Access::PinRequired);
    authorize(&mut gate, &ctx, 3);
}

#[test]
fn foreign_and_duplicate_results_cannot_release_or_grant_current_work() {
    let ctx = context(&settings());
    let mut gate = gate();
    let work = gate.verify(&ctx, input(), 0).unwrap();
    let ticket = work.ticket();
    let mut other = Gate::new([8; 16]);
    let foreign = other.verify(&ctx, input(), 0).unwrap();
    assert!(matches!(
        gate.complete(&ctx, result(foreign, true), 1),
        Outcome::Ignored
    ));
    assert!(matches!(gate.verify(&ctx, input(), 1), Err(Error::Busy)));
    assert!(matches!(
        gate.complete(&ctx, result(work, true), 2),
        Outcome::Granted
    ));
    gate.cancel();
    assert!(matches!(
        gate.complete(
            &ctx,
            Completion {
                ticket,
                result: WorkResult::Verified(true)
            },
            3
        ),
        Outcome::Ignored
    ));
    assert_eq!(gate.access(&ctx, Target::Control, 3), Access::PinRequired);
}

#[test]
fn source_and_policy_aba_cannot_reuse_an_old_challenge() {
    let map = settings();
    let ctx = context(&map);
    let mut gate = gate();
    let work = gate.verify(&ctx, input(), 0).unwrap();
    let b = Context::new(2, ctx.policy.clone());
    assert_eq!(gate.access(&b, Target::Control, 1), Access::PinRequired);
    assert_eq!(gate.access(&ctx, Target::Control, 2), Access::PinRequired);
    assert!(matches!(
        gate.complete(&ctx, result(work, true), 3),
        Outcome::Ignored
    ));

    let work = gate.verify(&ctx, input(), 4).unwrap();
    let mut changed = map.clone();
    let mut editor = Gate::new([9; 16]);
    for (now, value) in [(5, true), (6, false)] {
        let owner = context(&changed);
        authorize(&mut editor, &owner, now);
        let edit = editor
            .prepare(&owner, PolicyEdit::Settings(value), now)
            .unwrap();
        changed = editor.candidate(&owner, edit, &changed, now).unwrap();
    }
    assert_eq!(changed.get("psOptions"), Some(&json!(0)));
    assert_eq!(changed.get(REVISION), Some(&json!(2)));
    let owner = context(&changed);
    assert!(matches!(
        gate.complete(&owner, result(work, true), 7),
        Outcome::Ignored
    ));
    assert_eq!(gate.access(&owner, Target::Control, 7), Access::PinRequired);
}

#[test]
fn complete_policy_comparison_also_catches_unversioned_import_changes() {
    let mut map = settings();
    let ctx = context(&map);
    let mut gate = gate();
    authorize(&mut gate, &ctx, 1);
    map.insert("psOptions".into(), json!(1));
    assert_eq!(
        gate.access(&context(&map), Target::Control, 2),
        Access::PinRequired
    );
}

#[test]
fn refused_verification_in_a_disabled_source_still_retires_the_old_grant() {
    let ctx = context(&settings());
    let mut gate = gate();
    authorize(&mut gate, &ctx, 0);
    let disabled = Context::new(2, Policy::read(&Settings::new()).unwrap());
    assert!(matches!(
        gate.verify(&disabled, input(), 1),
        Err(Error::Restricted)
    ));
    assert_eq!(gate.access(&ctx, Target::Control, 2), Access::PinRequired);
}

#[test]
fn failed_worker_never_grants_and_releases_only_its_ticket() {
    let ctx = context(&settings());
    let mut gate = gate();
    let work = gate.verify(&ctx, input(), 0).unwrap();
    let failure = Completion::failed(work.ticket());
    drop(work); // Models refused worker launch; no computation remains running.
    assert!(matches!(
        gate.complete(&ctx, failure, 1),
        Outcome::Unavailable
    ));
    assert_eq!(gate.access(&ctx, Target::Control, 1), Access::PinRequired);
    assert!(gate.verify(&ctx, input(), 2).is_ok());
}

#[test]
fn enablement_produces_only_a_candidate_and_unknown_write_blocks_even_old_disabled_policy() {
    let map = Settings::new();
    let ctx = context(&map);
    let mut gate = gate();
    let work = gate.create(&ctx, input(), [0; 16], 0).unwrap();
    let Outcome::Prepared(change) = gate.complete(&ctx, created(work), 1) else {
        panic!("prepared")
    };
    let candidate = gate.candidate(&ctx, change, &map, 2).unwrap();
    assert_eq!(candidate.get("parentPin"), Some(&json!(RECORD)));
    assert_eq!(candidate.get(REVISION), Some(&json!(1)));
    assert!(map.is_empty());
    assert!(gate.expires.is_none());
    gate.write_uncertain();
    assert_eq!(gate.access(&ctx, Target::Control, 3), Access::Unavailable);
    assert!(matches!(
        gate.create(&ctx, input(), [0; 16], 3),
        Err(Error::Restricted)
    ));
    let installed = context(&candidate);
    assert_eq!(
        gate.access(&installed, Target::Control, 4),
        Access::Unavailable
    );
    gate.confirmed_policy(&installed, 5);
    assert_eq!(
        gate.access(&installed, Target::Control, 5),
        Access::PinRequired
    );
}

#[test]
fn unknown_write_cancels_pending_verification_until_verified_load() {
    let ctx = context(&settings());
    let mut gate = gate();
    let work = gate.verify(&ctx, input(), 1).unwrap();
    gate.write_uncertain();
    gate.confirmed_policy(&ctx, 2);
    assert!(matches!(gate.verify(&ctx, input(), 2), Err(Error::Busy)));
    assert!(matches!(
        gate.complete(&ctx, result(work, true), 3),
        Outcome::Ignored
    ));
    assert_eq!(gate.access(&ctx, Target::Control, 3), Access::PinRequired);
}

#[test]
fn definite_write_failure_keeps_the_authoritative_policy_and_revokes_grant() {
    let map = settings();
    let ctx = context(&map);
    let mut gate = gate();
    authorize(&mut gate, &ctx, 0);
    let change = gate.prepare(&ctx, PolicyEdit::Disable, 0).unwrap();
    let candidate = gate.candidate(&ctx, change, &map, 0).unwrap();
    assert_eq!(candidate.get("parentPin"), Some(&json!("*")));
    // Models the hook for a known NotApplied result, not a storage test.
    gate.cancel();
    assert!(Policy::read(&map).unwrap() == ctx.policy);
    assert_eq!(gate.access(&ctx, Target::Control, 1), Access::PinRequired);
    assert!(!map.contains_key(REVISION));
}

#[test]
fn policy_changes_require_control_grant_preserve_unrelated_data_and_advance_revision() {
    let mut map = settings();
    map.insert("unrelated".into(), json!({"kept":true}));
    let mut gate = gate();
    for (now, edit, key, value) in [
        (0, PolicyEdit::Channels(false), "psChannels", json!(0)),
        (1, PolicyEdit::Settings(true), "psOptions", json!(1)),
        (
            2,
            PolicyEdit::Providers(true),
            "requirePinForProviderSelection",
            json!(1),
        ),
        (3, PolicyEdit::Disable, "parentPin", json!("*")),
    ] {
        let ctx = context(&map);
        assert!(matches!(
            gate.prepare(&ctx, edit, now),
            Err(Error::Restricted)
        ));
        authorize(&mut gate, &ctx, now);
        let change = gate.prepare(&ctx, edit, now).unwrap();
        let next = gate.candidate(&ctx, change, &map, now).unwrap();
        assert_eq!(next.get(key), Some(&value));
        assert_eq!(next.get(REVISION), Some(&json!(now + 1)));
        assert_eq!(next.get("unrelated"), map.get("unrelated"));
        assert!(gate.expires.is_none());
        map = next;
    }
}

#[test]
fn prepared_change_is_rejected_after_cancel_source_change_or_grant_expiry() {
    let map = settings();
    let ctx = context(&map);
    for reason in 0..3 {
        let mut gate = gate();
        authorize(&mut gate, &ctx, 0);
        let change = gate.prepare(&ctx, PolicyEdit::Disable, 0).unwrap();
        let (current, now) = match reason {
            0 => {
                gate.cancel();
                (ctx.clone(), 1)
            }
            1 => (Context::new(2, ctx.policy.clone()), 1),
            _ => (ctx.clone(), GRANT_MS),
        };
        assert!(gate.candidate(&current, change, &map, now).is_err());
        assert_eq!(map.get("parentPin"), Some(&json!(RECORD)));
    }
}

#[test]
fn pin_creation_cannot_replace_locked_pin_or_outlive_prior_grant() {
    let map = settings();
    let ctx = context(&map);
    let mut gate = gate();
    assert!(matches!(
        gate.create(&ctx, input(), [0; 16], 0),
        Err(Error::Restricted)
    ));
    authorize(&mut gate, &ctx, 1);
    let work = gate.create(&ctx, input(), [0; 16], 1).unwrap();
    assert!(matches!(
        gate.complete(&ctx, created(work), 1 + GRANT_MS),
        Outcome::Unavailable
    ));
}

#[test]
fn revision_overflow_rejects_changes_but_allows_exact_noop() {
    let mut map = settings();
    map.insert(REVISION.into(), json!(u64::MAX));
    let ctx = context(&map);
    let mut gate = gate();
    authorize(&mut gate, &ctx, 0);
    let edit = gate.prepare(&ctx, PolicyEdit::Channels(true), 0).unwrap();
    assert_eq!(gate.candidate(&ctx, edit, &map, 0).unwrap(), map);
    authorize(&mut gate, &ctx, 1);
    let edit = gate.prepare(&ctx, PolicyEdit::Channels(false), 1).unwrap();
    assert!(matches!(
        gate.candidate(&ctx, edit, &map, 1),
        Err(Error::RevisionExhausted)
    ));
}

#[test]
fn generation_and_clock_overflow_cannot_restore_or_extend_authority() {
    let ctx = context(&settings());
    let mut gate = gate();
    gate.access(&ctx, Target::Control, 0);
    gate.generation = u64::MAX - 1;
    let work = gate.verify(&ctx, input(), 0).unwrap();
    gate.cancel();
    assert!(matches!(
        gate.complete(&ctx, result(work, true), 1),
        Outcome::Ignored
    ));
    assert!(matches!(
        gate.verify(&ctx, input(), 2),
        Err(Error::GenerationExhausted)
    ));
    let mut gate = Gate::new([8; 16]);
    let work = gate.verify(&ctx, input(), u64::MAX - 1).unwrap();
    assert!(matches!(
        gate.complete(&ctx, result(work, true), u64::MAX),
        Outcome::Unavailable
    ));
    assert_eq!(
        gate.access(&ctx, Target::Control, u64::MAX),
        Access::PinRequired
    );
}

#[test]
fn authoritative_map_must_still_match_when_prepared_change_reaches_writer() {
    let map = settings();
    let ctx = context(&map);
    let mut gate = gate();
    authorize(&mut gate, &ctx, 0);
    let change = gate.prepare(&ctx, PolicyEdit::Disable, 0).unwrap();
    let mut replaced = map.clone();
    replaced.insert("psOptions".into(), json!(1));
    assert!(matches!(
        gate.candidate(&ctx, change, &replaced, 1),
        Err(Error::Stale)
    ));
}
