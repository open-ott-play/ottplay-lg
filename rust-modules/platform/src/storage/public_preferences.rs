//! Nonsecret installation preferences. The TV uses the existing authenticated OTT
//! helper and CAS; only the host port uses JsonStore. No secret fallback exists.
use super::*;
pub use crate::storage::wire::ott::{PublicPatch as Patch, PublicPreferences as Preferences};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub value: Preferences,
    pub revision: u64,
    pub epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveError {
    NotApplied(Error),
    /// A dispatched operation or its readback was inconclusive. Do not publish
    /// the candidate, report "not saved", or admit another write as if it failed.
    Uncertain,
}
impl From<Error> for SaveError {
    fn from(error: Error) -> Self { Self::NotApplied(error) }
}
impl From<StoreError> for SaveError {
    fn from(error: StoreError) -> Self { Self::NotApplied(Error::Store(error)) }
}

pub fn load() -> Result<Option<Snapshot>, Error> {
    check_fixture_authority()?;
    if USE_HELPER { return helper::public_load(); }
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    let path = root();
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Error::Invalid),
        Ok(_) => (),
    }
    let store = JsonStore::new(path)?;
    if erasure_state(&store)? == ErasureState::Pending { return Err(Error::Revoked); }
    let result = host_snapshot(store.load(RecordKey::OttPreferences)?)?;
    check_fixture_authority()?;
    Ok(result)
}

pub fn patch_guarded(patch: Patch, permit: &WritePermit)
    -> Result<(Snapshot, CommitReceipt), SaveError>
{
    patch.validate().map_err(|_| Error::Invalid)?;
    if !permit.is_valid() { return Err(Error::Revoked.into()); }
    check_fixture_authority()?;
    if USE_HELPER { return helper::public_patch(patch, permit); }
    ensure_parent()?;
    let _writer = WRITER.lock().unwrap_or_else(|error| error.into_inner());
    let store = open_or_create(&root())?;
    if erasure_state(&store)? == ErasureState::Pending { return Err(Error::Revoked.into()); }
    let record = store.load(RecordKey::OttPreferences)?;
    let before = host_snapshot(record.clone())?;
    let value = before.as_ref().map(|snapshot| snapshot.value.clone()).unwrap_or_default()
        .apply(&patch).map_err(|_| Error::Invalid)?;
    let revision = record.as_ref().map_or(0, |record| record.revision)
        .checked_add(1).ok_or(Error::Invalid)?;
    let epoch = match before {
        Some(snapshot) => snapshot.epoch,
        None => {
            // A storage epoch, never a device identity. Erase and recreation
            // cannot reuse a prior lifetime's publication stamp.
            let random = crate::storage::state::Generation::random().map_err(|_| Error::Invalid)?;
            u64::from_le_bytes(random.0[..8].try_into().map_err(|_| Error::Invalid)?).max(1)
        }
    };
    let snapshot = Snapshot { value, revision, epoch };
    let payload = serde_json::to_string(&snapshot).map_err(|_| Error::Invalid)?;
    if !permit.is_valid() { return Err(Error::Revoked.into()); }
    match store.commit(RecordKey::OttPreferences, &Record::data(revision, payload))? {
        CommitReceipt::Durable => (),
        CommitReceipt::Uncertain { .. } => return Err(SaveError::Uncertain),
    }
    let observed = store.load(RecordKey::OttPreferences).map_err(|_| SaveError::Uncertain)
        .and_then(|record| host_snapshot(record).map_err(|_| SaveError::Uncertain))?;
    if observed.as_ref() != Some(&snapshot) || !permit.is_valid()
        || check_fixture_authority().is_err()
    { return Err(SaveError::Uncertain); }
    Ok((snapshot, CommitReceipt::Durable))
}

fn host_snapshot(record: Option<Record>) -> Result<Option<Snapshot>, Error> {
    let Some(record) = record else { return Ok(None); };
    let RecordState::Data { payload } = record.state else { return Ok(None); };
    let snapshot: Snapshot = serde_json::from_str(&payload).map_err(|_| Error::Invalid)?;
    snapshot.value.validate().map_err(|_| Error::Invalid)?;
    if snapshot.revision == 0 || snapshot.epoch == 0 || snapshot.revision != record.revision {
        return Err(Error::Invalid);
    }
    Ok(Some(snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_host_record_rejects_secret_fields_and_revision_mismatch() {
        let valid = Snapshot { value: Preferences { interface_mode: Some(0),
            ..Preferences::default() }, revision: 3, epoch: 1 };
        let payload = serde_json::to_string(&valid).unwrap();
        assert_eq!(host_snapshot(Some(Record::data(3, payload.clone()))), Ok(Some(valid)));
        assert_eq!(host_snapshot(Some(Record::data(2, payload))), Err(Error::Invalid));
        let payload = r#"{"value":{"version":1,"token":"secret"},"revision":3,"epoch":1}"#;
        assert_eq!(host_snapshot(Some(Record::data(3, payload.into()))), Err(Error::Invalid));
        assert_eq!(host_snapshot(Some(Record::cleared(3))), Ok(None));
    }

    #[test]
    fn public_preferences_preserve_optional_defaults_and_do_not_expose_debug_values() {
        let before = Preferences::default();
        assert_eq!(before.interface_mode, None);
        let address = "https://control.invalid:443/private-path";
        let next = before.apply(&Patch::RemoteAddress(address.into())).unwrap()
            .apply(&Patch::InterfaceMode(0)).unwrap();
        assert_eq!(next.device_id, None);
        assert_eq!(next.remote_address.as_deref(), Some(address));
        assert_eq!(next.interface_mode, Some(0));
        assert!(!format!("{next:?}").contains(address));
        assert!(!format!("{:?}", Patch::RemoteAddress(address.into())).contains(address));
    }
}
