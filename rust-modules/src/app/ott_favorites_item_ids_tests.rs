use super::*;
use plx_data::ott::favorites::{Locator, Origin};

fn favorite(id: &str) -> Favorite {
    Favorite {
        title: format!("Film {id}"),
        origin: Origin {
            provider: "plex".into(),
            profile: "Home".into(),
            subprofile: None,
            source_alias: "Server".into(),
        },
        locator: Locator::Plex {
            server_id: "synthetic-server".into(),
            rating_key: id.into(),
        },
    }
}
fn document(entries: Vec<Favorite>) -> Document {
    serde_json::from_value(json!({"version":1,"entries":entries})).unwrap()
}
fn allocate(state: &mut Saved, first: u8) {
    let mut next = first;
    prepare_with(state, &mut || {
        next += 1;
        Ok([next; 16])
    })
    .unwrap();
}
#[test]
fn row_ids_survive_title_reorder_restart_and_unrelated_settings() {
    let mut state = Saved::default();
    state.favorites = document(vec![favorite("1"), favorite("2")]);
    allocate(&mut state, 0);
    let before = ordered(&state).unwrap();
    let mut changed = favorite("2");
    changed.title = "New title".into();
    state.favorites = document(vec![changed, favorite("1")]);
    state.settings.insert("unrelated".into(), json!(true));
    assert!(!prepare_with(&mut state, &mut || panic!(
        "stable identities do not allocate"
    ))
    .unwrap());
    let restarted: Saved = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    assert_eq!(
        ordered(&restarted).unwrap(),
        vec![before[1].clone(), before[0].clone()]
    );
}
#[test]
fn remove_then_readd_and_changed_source_allocate_new_ids() {
    let mut state = Saved::default();
    state.favorites = document(vec![favorite("1")]);
    allocate(&mut state, 0);
    let first = ordered(&state).unwrap()[0].clone();
    state.favorites = Document::default();
    allocate(&mut state, 10);
    state.favorites = document(vec![favorite("1")]);
    allocate(&mut state, 20);
    assert_ne!(ordered(&state).unwrap()[0], first);
    let second = ordered(&state).unwrap()[0].clone();
    let mut moved = favorite("1");
    moved.origin.source_alias = "Other server".into();
    state.favorites = document(vec![moved]);
    allocate(&mut state, 30);
    assert_ne!(ordered(&state).unwrap()[0], second);
}
#[test]
fn collision_and_entropy_failure_are_atomic_and_bounded() {
    let mut state = Saved::default();
    state.favorites = document(vec![favorite("1"), favorite("2")]);
    let before = serde_json::to_value(&state).unwrap();
    let mut calls = 0;
    assert!(prepare_with(&mut state, &mut || {
        calls += 1;
        Ok([1; 16])
    })
    .is_err());
    assert_eq!(calls, 2);
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    assert!(prepare_with(&mut state, &mut || Err("unavailable")).is_err());
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
}
#[test]
fn malformed_duplicate_map_rejects_without_repair() {
    let mut state = Saved::default();
    state.favorites = document(vec![favorite("1"), favorite("2")]);
    allocate(&mut state, 0);
    let first = state.favorite_items.as_ref().unwrap().rows[0].id.clone();
    state.favorite_items.as_mut().unwrap().rows[1].id = first;
    assert!(state.validate().is_err());
    assert!(ordered(&state).is_err());
    assert!(prepare_with(&mut state, &mut || panic!("invalid map cannot allocate")).is_err());
}
#[test]
fn migration_refusal_or_uncertainty_keeps_the_readable_original() {
    let mut original = Saved::default();
    original.favorites = document(vec![favorite("1")]);
    let image = serde_json::to_value(&original).unwrap();
    for (failure, expected) in [
        (favorites::WriteFailure::NotApplied, false),
        (favorites::WriteFailure::Unknown, true),
    ] {
        let (found, uncertain) = migrate_with(
            original.clone(),
            |state| prepare_with(state, &mut || Ok([1; 16])),
            |candidate| {
                assert!(ordered(candidate).is_ok());
                Err(failure)
            },
        );
        assert_eq!(uncertain, expected);
        assert_eq!(serde_json::to_value(found).unwrap(), image);
    }
    let (found, uncertain) = migrate_with(
        original,
        |state| prepare_with(state, &mut || Ok([1; 16])),
        |_| Ok(()),
    );
    assert!(!uncertain);
    assert!(ordered(&found).is_ok());
}
#[test]
fn unknown_operation_never_migrates_or_writes_ids() {
    let mut state = Saved::default();
    state.favorites = document(vec![favorite("1")]);
    state.receipts.push(wire_status());
    let (found, uncertain) = migrate_with(
        state,
        |_| panic!("unknown blocks migration"),
        |_| panic!("unknown blocks write"),
    );
    assert!(!uncertain);
    assert!(found.favorite_items.is_none());
    assert_eq!(found.favorites.entries().len(), 1);
}
fn wire_status() -> plx_data::ott::favorites_wire::DurableStatus {
    plx_data::ott::favorites_wire::DurableStatus {
        operation_id: "00000000-0000-4000-8000-000000000001".into(),
        operation_runtime: Some("fixture".into()),
        transaction_id: None,
        outcome: plx_data::ott::favorites_wire::Outcome::Unknown,
        state: "unknown".into(),
        receipt: None,
    }
}
#[test]
fn membership_may_be_transitional_but_reads_need_a_complete_map() {
    let mut state = Saved::default();
    state.favorites = document(vec![favorite("1")]);
    allocate(&mut state, 0);
    state.favorites = document(vec![favorite("1"), favorite("2")]);
    assert!(state.validate().is_ok());
    assert!(ordered(&state).is_err());
    allocate(&mut state, 3);
    assert_eq!(ordered(&state).unwrap().len(), 2);
}
#[test]
fn portable_roundtrip_does_not_import_receiver_row_ids() {
    let mut sender = Saved::default();
    sender.favorites = document(vec![favorite("1")]);
    allocate(&mut sender, 0);
    let exported = sender.favorites.encode().unwrap();
    let mut receiver = Saved::default();
    receiver.favorites = Document::parse(&exported).unwrap();
    allocate(&mut receiver, 10);
    assert_ne!(ordered(&sender).unwrap(), ordered(&receiver).unwrap());
    assert_eq!(sender.favorites, receiver.favorites);
}
#[test]
fn complete_container_preflight_includes_receiver_metadata_overhead() {
    let mut state = Saved::default();
    state.favorites = document((1..=1000).map(|i| favorite(&i.to_string())).collect());
    let base = serde_json::to_vec(&state).unwrap().len();
    state.settings.insert(
        "fixture-padding".into(),
        json!("x".repeat(1024 * 1024 - base - 2048)),
    );
    assert!(
        favorites::preflight(&state).is_ok(),
        "the pre-migration image fits the real Plain bound"
    );
    let mut serial = 0u64;
    prepare_with(&mut state, &mut || {
        serial += 1;
        let mut bytes = [0; 16];
        bytes[8..].copy_from_slice(&serial.to_be_bytes());
        Ok(bytes)
    })
    .unwrap();
    assert!(
        favorites::preflight(&state).is_err(),
        "IDs must be preflighted before a pending journal is written"
    );
}

#[test]
fn explicit_null_row_metadata_is_not_a_legacy_missing_field() {
    let value = serde_json::to_value(Saved::default()).unwrap();
    assert!(value.get("favorite_items").is_none());
    assert!(serde_json::from_value::<Saved>(value.clone()).is_ok());
    let mut bad = value;
    bad["favorite_items"] = Value::Null;
    assert!(serde_json::from_value::<Saved>(bad).is_err());
}

#[test]
fn expiry_during_identity_preparation_refuses_the_write() {
    let mut state = Saved::default();
    state.favorites = document(vec![favorite("1")]);
    let valid = std::cell::Cell::new(true);
    let result = write_prepared_with(
        &mut state,
        || valid.get(),
        |state| {
            let changed = prepare_with(state, &mut || Ok([1; 16]))?;
            valid.set(false);
            Ok(changed)
        },
        |_| panic!("expired preparation must not reach the protected writer"),
    );
    assert_eq!(result, Err(favorites::WriteFailure::NotApplied));
    assert!(
        ordered(&state).is_ok(),
        "the test expired only after actual identity allocation"
    );
}
