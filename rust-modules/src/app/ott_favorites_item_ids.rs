//! Receiver-private row identities. Portable Favorites remain the existing Document.
//! Allocation occurs only in an already-admitted persistence worker, never on reads.
use super::*;
use plx_base::task::OffFrame;
use plx_data::ott::favorites::{Favorite, ENTRY_COUNT};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RowIds {
    version: u8,
    rows: Vec<RowId>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RowId {
    identity: String,
    id: String,
}

/// Missing is a legacy image; explicit null is malformed private metadata.
pub(super) fn present<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<RowIds>, D::Error> {
    RowIds::deserialize(deserializer).map(Some)
}

pub(super) fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            14 => b == b'4',
            19 => matches!(b, b'8' | b'9' | b'a' | b'b'),
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
        })
}
fn key(favorite: &Favorite) -> String {
    plx_data::ott::favorites_wire::hash(favorite.identity().as_bytes())
}
impl RowIds {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        let mut identities = BTreeSet::new();
        let mut ids = BTreeSet::new();
        if self.version != 1
            || self.rows.len() > ENTRY_COUNT
            || self.rows.iter().any(|row| {
                !digest(&row.identity)
                    || !uuid(&row.id)
                    || !identities.insert(&row.identity)
                    || !ids.insert(&row.id)
            })
        {
            return Err("Invalid private Favorites row identities");
        }
        Ok(())
    }
}

/// Exact completeness is required before exposing any list or accepting an ID.
/// A malformed map is not repaired silently; a missing legacy map is migrated by a writer.
pub(super) fn ordered(state: &Saved) -> Result<Vec<String>, &'static str> {
    state
        .favorites
        .validate()
        .map_err(|_| "storage_unavailable")?;
    let Some(map) = &state.favorite_items else {
        return if state.favorites.entries().is_empty() {
            Ok(Vec::new())
        } else {
            Err("storage_unavailable")
        };
    };
    map.validate().map_err(|_| "storage_unavailable")?;
    if map.rows.len() != state.favorites.entries().len() {
        return Err("storage_unavailable");
    }
    let rows: BTreeMap<_, _> = map
        .rows
        .iter()
        .map(|row| (row.identity.as_str(), &row.id))
        .collect();
    let mut keys = BTreeSet::new();
    state
        .favorites
        .entries()
        .iter()
        .map(|favorite| {
            let key = key(favorite);
            if !keys.insert(key.clone()) {
                return Err("storage_unavailable");
            }
            rows.get(key.as_str())
                .map(|id| (*id).clone())
                .ok_or("storage_unavailable")
        })
        .collect()
}

/// Rekey only already-complete receiver UUID rows; allocation remains in the
/// existing worker. The caller installs/validates the matching document next.
pub(super) fn rename(state: &mut Saved, entries: &[(Favorite, Favorite)]) -> Result<(), &'static str> {
    ordered(state)?;
    let Some(map) = state.favorite_items.as_mut() else { return Ok(()); };
    let keys: BTreeMap<_, _> = entries.iter().map(|(old, new)| (key(old), key(new))).collect();
    for row in &mut map.rows {
        if let Some(new) = keys.get(&row.identity) { row.identity = new.clone(); }
    }
    map.validate()
}

pub(super) fn prepare(state: &mut Saved, _off: &OffFrame) -> Result<bool, &'static str> {
    prepare_with(state, &mut || {
        plx_platform::storage::state::Generation::random()
            .map(|value| value.0)
            .map_err(|_| "Could not allocate Favorites row identities")
    })
}

/// Identity allocation is part of the same admitted write, not a deadline renewal.
/// Both checks observe the caller's original deadline and write permit.
pub(super) fn write_prepared_with(
    state: &mut Saved,
    mut valid: impl FnMut() -> bool,
    prepare: impl FnOnce(&mut Saved) -> Result<bool, &'static str>,
    persist: impl FnOnce(&Saved) -> Result<(), favorites::WriteFailure>,
) -> Result<(), favorites::WriteFailure> {
    if !valid() {
        return Err(favorites::WriteFailure::NotApplied);
    }
    prepare(state).map_err(|_| favorites::WriteFailure::NotApplied)?;
    if !valid() {
        return Err(favorites::WriteFailure::NotApplied);
    }
    persist(state)
}

fn prepare_with(
    state: &mut Saved,
    random: &mut impl FnMut() -> Result<[u8; 16], &'static str>,
) -> Result<bool, &'static str> {
    state
        .favorites
        .validate()
        .map_err(|_| "Invalid Favorites document")?;
    if let Some(map) = &state.favorite_items {
        map.validate()?;
    }
    if ordered(state).is_ok() {
        return Ok(false);
    }
    let prior: BTreeMap<_, _> = state
        .favorite_items
        .as_ref()
        .into_iter()
        .flat_map(|map| &map.rows)
        .map(|row| (row.identity.as_str(), row.id.as_str()))
        .collect();
    // Also reject a collision with an ID being removed in this transaction.
    let mut ids: BTreeSet<String> = prior.values().map(|id| (*id).to_owned()).collect();
    let mut identities = BTreeSet::new();
    let mut rows = Vec::with_capacity(state.favorites.entries().len());
    for favorite in state.favorites.entries() {
        let identity = key(favorite);
        if !identities.insert(identity.clone()) {
            return Err("Favorites identity collision");
        }
        let id = if let Some(id) = prior.get(identity.as_str()) {
            (*id).to_owned()
        } else {
            // One bounded entropy request per new row. A collision is a refusal,
            // not an unbounded retry or a change to an existing row identity.
            let mut bytes = random()?;
            bytes[6] = (bytes[6] & 15) | 64;
            bytes[8] = (bytes[8] & 63) | 128;
            let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            let id = format!(
                "{}-{}-{}-{}-{}",
                &hex[..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..]
            );
            if !ids.insert(id.clone()) {
                return Err("Favorites row identity collision");
            }
            id
        };
        rows.push(RowId { identity, id });
    }
    state.favorite_items = Some(RowIds { version: 1, rows });
    Ok(true)
}

/// Migration failure never replaces a readable document with a default one.
/// The caller carries an uncertain attempt into the existing persistence fence.
pub(super) fn migrate_with(
    mut state: Saved,
    prepare: impl FnOnce(&mut Saved) -> Result<bool, &'static str>,
    persist: impl FnOnce(&Saved) -> Result<(), favorites::WriteFailure>,
) -> (Saved, bool) {
    if state
        .receipts
        .iter()
        .any(|receipt| receipt.outcome == plx_data::ott::favorites_wire::Outcome::Unknown)
        || ordered(&state).is_ok()
    {
        return (state, false);
    }
    let original = state.clone();
    match prepare(&mut state) {
        Ok(true) => match persist(&state) {
            Ok(()) => (state, false),
            Err(favorites::WriteFailure::NotApplied | favorites::WriteFailure::NotAppliedStorage(_)) => {
                (original, false)
            }
            Err(favorites::WriteFailure::Unknown) => (original, true),
        },
        _ => (original, false),
    }
}

#[cfg(test)]
#[path = "ott_favorites_item_ids_tests.rs"]
mod tests;


#[cfg(test)]
mod storage_diagnostic_tests {
    use super::*;
    use plx_data::ott::favorites::{Locator, Origin};

    fn legacy_state() -> Saved {
        let mut state = Saved::default();
        state.settings.insert("test-setting".into(), json!("preserve"));
        state.category = "selected".into();
        state.favorites_selected = true;
        state.favorites.toggle(Favorite {
            title: "Film".into(),
            origin: Origin {
                provider: "local".into(), profile: "Home".into(), subprofile: None,
                source_alias: "Movies".into(),
            },
            locator: Locator::Local {
                root_alias: "Movies".into(), relative_path: "Film.mp4".into(),
            },
        }).unwrap();
        state
    }

    fn storage_refusal() -> favorites::WriteFailure {
        favorites::WriteFailure::NotAppliedStorage(private::Error::Store(
            plx_platform::storage::StoreError::Io {
                stage: plx_platform::storage::CommitStage::CreateTemp,
                errno: 13,
            },
        ))
    }

    #[test]
    fn prepared_write_preserves_the_typed_storage_refusal() {
        let mut state = legacy_state();
        assert!(ordered(&state).is_err());
        let mut calls = 0;
        let result = write_prepared_with(
            &mut state,
            || true,
            |candidate| prepare_with(candidate, &mut || Ok([1; 16])),
            |candidate| {
                calls += 1;
                assert_eq!(ordered(candidate).unwrap().len(), 1);
                Err(storage_refusal())
            },
        );
        assert_eq!(calls, 1);
        assert_eq!(result, Err(storage_refusal()));
    }

    #[test]
    fn typed_migration_refusal_retains_the_original_without_uncertainty() {
        let state = legacy_state();
        let expected = serde_json::to_value(&state).unwrap();
        assert!(ordered(&state).is_err());
        let mut calls = 0;
        let (saved, uncertain) = migrate_with(
            state,
            |candidate| prepare_with(candidate, &mut || Ok([1; 16])),
            |candidate| {
                calls += 1;
                assert_eq!(ordered(candidate).unwrap().len(), 1);
                Err(storage_refusal())
            },
        );
        assert_eq!(calls, 1);
        assert!(!uncertain);
        assert!(saved.favorite_items.is_none());
        assert_eq!(serde_json::to_value(saved).unwrap(), expected);
    }
}
