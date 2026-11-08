//! Pure draft tests; storage, App routing and renderer publication are not exercised here.
use super::*;
use serde_json::json;

const FIELDS: [Field; 2] = [
    Field { key: "volumeStep", revision_key: Some("settingRevision:volumeStep") },
    Field { key: "pageSize", revision_key: Some("settingRevision:pageSize") },
];

fn settings() -> Settings {
    BTreeMap::from([
        ("volumeStep".into(), json!(5)),
        ("settingRevision:volumeStep".into(), json!("volume-before")),
        ("pageSize".into(), json!(25)),
        ("settingRevision:pageSize".into(), json!("page-before")),
        ("nativeRemoteServerAddress".into(), json!("https://example.invalid")),
    ])
}
fn begin(state: &mut State, current: &Settings) -> Address {
    state.begin("interface", InstanceId(7), 11, &FIELDS, current).unwrap()
}
fn valid(_: &str, _: &Value) -> Result<(), &'static str> { Ok(()) }

#[test]
fn opening_and_reading_a_group_does_not_create_a_patch_or_mutate_storage() {
    let current = settings();
    let original = current.clone();
    let mut state = State::default();
    let address = begin(&mut state, &current);
    assert_eq!(state.group(), Some("interface"));
    assert_eq!(state.address(), Some(address));
    assert_eq!(state.get("volumeStep"), Some(&json!(5)));
    assert!(state.get("unknown").is_none());
    assert!(state.changes(address, &current).unwrap().is_empty());
    assert_eq!(current, original);
    assert_eq!(state.begin("interface", InstanceId(7), 11, &FIELDS, &current), Err(Error::AlreadyOpen));
}

#[test]
fn invalid_value_unknown_field_and_stale_addresses_leave_the_draft_intact() {
    let current = settings();
    let mut state = State::default();
    let address = begin(&mut state, &current);
    assert_eq!(state.stage(address, "volumeStep", json!(999), |key, _| {
        assert_eq!(key, "volumeStep"); Err("outside shared setting range")
    }), Err(Error::InvalidValue("outside shared setting range")));
    assert_eq!(state.stage(address, "profile.source", json!("unused"), |_, _| panic!("unknown key reached validator")), Err(Error::UnknownField));
    for stale in [Address { id: address.id + 1, ..address },
        Address { owner: InstanceId(8), ..address }, Address { navigation: 12, ..address }] {
        assert_eq!(state.stage(stale, "volumeStep", json!(6), |_, _| panic!("stale request reached validator")), Err(Error::Stale));
        assert!(!state.cancel(stale));
    }
    assert_eq!(state.get("volumeStep"), Some(&json!(5)));
    assert!(state.changes(address, &current).unwrap().is_empty());
}

#[test]
fn unrelated_remote_changes_are_preserved_and_only_dirty_fields_are_returned() {
    let mut current = settings();
    let mut state = State::default();
    let address = begin(&mut state, &current);
    state.stage(address, "volumeStep", json!(6), valid).unwrap();
    current.insert("pageSize".into(), json!(15));
    current.insert("settingRevision:pageSize".into(), json!("remote-new"));
    current.insert("nativeRemoteServerAddress".into(), json!("https://new.example.invalid"));
    current.insert("newer-profile-data".into(), json!({"retained":true}));
    let changes = state.changes(address, &current).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].field.key, "volumeStep");
    assert_eq!(changes[0].field.revision_key, Some("settingRevision:volumeStep"));
    assert_eq!(changes[0].value, json!(6));
    // This models only the field merge. Real revision generation belongs to the
    // shared settings candidate helper, not this pure state machine.
    let before_merge = current.clone();
    for change in changes { current.insert(change.field.key.into(), change.value); }
    let mut expected = before_merge;
    expected.insert("volumeStep".into(), json!(6));
    assert_eq!(current, expected);
}

#[test]
fn dirty_field_conflicts_include_revision_only_aba_and_missing_to_present() {
    for revision_only in [false, true] {
        let mut current = settings();
        let mut state = State::default();
        let address = begin(&mut state, &current);
        state.stage(address, "volumeStep", json!(6), valid).unwrap();
        if revision_only { current.insert("settingRevision:volumeStep".into(), json!("after-A-B-A")); }
        else { current.insert("volumeStep".into(), json!(7)); }
        assert!(matches!(state.changes(address, &current), Err(Error::Conflict("volumeStep"))));
        assert_eq!(state.get("volumeStep"), Some(&json!(6)));
        assert_eq!(state.pending(), None);
    }
    let mut current = Settings::new();
    let mut state = State::default();
    let address = begin(&mut state, &current);
    state.stage(address, "volumeStep", json!(6), valid).unwrap();
    current.insert("volumeStep".into(), json!(5));
    assert!(matches!(state.changes(address, &current), Err(Error::Conflict("volumeStep"))));
}

#[test]
fn compound_menu_value_is_one_conflict_domain_and_cancel_does_not_rewrite_it() {
    let fields = [Field { key: "nativeMenuVisibility", revision_key: None }];
    let original = json!({"version":1,"revision":1,"hidden":[]});
    let mut current = BTreeMap::from([("nativeMenuVisibility".into(), original)]);
    let mut state = State::default();
    let address = state.begin("menu", InstanceId(7), 11, &fields, &current).unwrap();
    state.stage(address, "nativeMenuVisibility", json!({"version":1,"revision":2,"hidden":["search"]}), valid).unwrap();
    current.insert("nativeMenuVisibility".into(), json!({"version":1,"revision":2,"hidden":["profiles"]}));
    assert!(matches!(state.changes(address, &current), Err(Error::Conflict("nativeMenuVisibility"))));
    let before = current.clone();
    assert!(state.cancel(address));
    assert_eq!(current, before);
    assert_eq!(state.address(), None);
}

#[test]
fn returning_to_original_value_removes_only_that_dirty_field() {
    let current = settings();
    let mut state = State::default();
    let address = begin(&mut state, &current);
    state.stage(address, "volumeStep", json!(6), valid).unwrap();
    state.stage(address, "pageSize", json!(20), valid).unwrap();
    state.stage(address, "volumeStep", json!(5), valid).unwrap();
    let changes = state.changes(address, &current).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].field.key, "pageSize");
}

#[test]
fn rejected_save_retains_current_draft_but_cancelled_save_cannot_revive_it() {
    let current = settings();
    let mut state = State::default();
    let address = begin(&mut state, &current);
    state.stage(address, "volumeStep", json!(6), valid).unwrap();
    state.mark_saving(address).unwrap();
    assert_eq!(state.pending(), Some(address));
    assert_eq!(state.mark_saving(address), Err(Error::Busy));
    assert_eq!(state.stage(address, "volumeStep", json!(7), valid), Err(Error::Busy));
    assert!(state.settle(address, Outcome::Rejected));
    assert_eq!(state.get("volumeStep"), Some(&json!(6)));
    assert_eq!(state.changes(address, &current).unwrap().len(), 1);
    state.mark_saving(address).unwrap();
    assert!(state.cancel(address));
    assert_eq!(state.begin("interface", InstanceId(8), 12, &FIELDS, &current), Err(Error::Busy));
    assert!(state.settle(address, Outcome::Rejected));
    assert_eq!(state.address(), None);
    let next = begin(&mut state, &current);
    assert!(next.id > address.id);
    assert!(!state.settle(address, Outcome::Applied));
    assert!(!state.cancel(address));
    assert_eq!(state.address(), Some(next));
}

#[test]
fn applied_or_uncertain_outcome_terminates_current_draft_exactly_once() {
    for outcome in [Outcome::Applied, Outcome::Uncertain] {
        let current = settings();
        let mut state = State::default();
        let address = begin(&mut state, &current);
        state.stage(address, "volumeStep", json!(6), valid).unwrap();
        state.mark_saving(address).unwrap();
        let stale = Address { id: address.id + 1, ..address };
        assert!(!state.settle(stale, outcome));
        assert_eq!(state.pending(), Some(address));
        assert!(state.settle(address, outcome));
        assert_eq!(state.pending(), None);
        assert_eq!(state.address(), None);
        assert!(!state.settle(address, outcome));
    }
}

#[test]
fn invalid_inventory_and_exhausted_ids_do_not_admit_partial_drafts() {
    let current = settings();
    let mut state = State::default();
    for fields in [&[][..], &[FIELDS[0], FIELDS[0]][..]] {
        assert_eq!(state.begin("interface", InstanceId(7), 11, fields, &current), Err(Error::InvalidFields));
        assert_eq!(state.address(), None);
    }
    state.serial = u64::MAX;
    assert_eq!(state.begin("interface", InstanceId(7), 11, &FIELDS, &current), Err(Error::Exhausted));
    assert_eq!(state.address(), None);
}
