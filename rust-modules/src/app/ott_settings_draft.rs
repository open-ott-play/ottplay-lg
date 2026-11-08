//! Pure ownership and field-level conflict checks for a local settings group.
//! The runner validates values with existing settings helpers, merges only the
//! returned fields into current Saved, and admits the existing durable writer.
use super::InstanceId;
use serde_json::Value;
use std::collections::BTreeMap;

type Settings = BTreeMap<String, Value>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Address {
    pub id: u64,
    pub owner: InstanceId,
    pub navigation: u64,
}

/// Static, trusted group inventory. A compound value is one conflict domain.
#[derive(Clone, Copy)]
pub(super) struct Field {
    pub key: &'static str,
    pub revision_key: Option<&'static str>,
}

struct Baseline {
    field: Field,
    value: Option<Value>,
    revision: Option<Value>,
}

struct Draft {
    address: Address,
    group: &'static str,
    base: BTreeMap<&'static str, Baseline>,
    dirty: BTreeMap<&'static str, Value>,
}

/// Contains only requested fields, never a Saved snapshot or a source/profile.
pub(super) struct Change {
    pub field: Field,
    pub value: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    Busy,
    AlreadyOpen,
    Stale,
    UnknownField,
    InvalidFields,
    Exhausted,
    Conflict(&'static str),
    InvalidValue(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Applied,
    Rejected,
    Uncertain,
}

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    draft: Option<Draft>,
    pending: Option<Address>,
}

impl State {
    pub fn begin(&mut self, group: &'static str, owner: InstanceId, navigation: u64,
        fields: &[Field], current: &Settings) -> Result<Address, Error>
    {
        if self.pending.is_some() { return Err(Error::Busy); }
        if self.draft.is_some() { return Err(Error::AlreadyOpen); }
        if group.is_empty() || fields.is_empty() { return Err(Error::InvalidFields); }
        let mut base = BTreeMap::new();
        for &field in fields {
            if field.key.is_empty() || field.revision_key == Some("")
                || base.contains_key(field.key) { return Err(Error::InvalidFields); }
            base.insert(field.key, Baseline {
                field,
                value: current.get(field.key).cloned(),
                revision: field.revision_key.and_then(|key| current.get(key)).cloned(),
            });
        }
        let id = self.serial.checked_add(1).ok_or(Error::Exhausted)?;
        let address = Address { id, owner, navigation };
        self.serial = id;
        self.draft = Some(Draft { address, group, base, dirty: BTreeMap::new() });
        Ok(address)
    }

    pub fn address(&self) -> Option<Address> { self.draft.as_ref().map(|d| d.address) }
    pub fn group(&self) -> Option<&'static str> { self.draft.as_ref().map(|d| d.group) }
    pub fn pending(&self) -> Option<Address> { self.pending }

    /// None is an absent stored value, for which the shared descriptor supplies a default.
    pub fn get(&self, key: &str) -> Option<&Value> {
        let draft = self.draft.as_ref()?;
        draft.dirty.get(key).or_else(|| draft.base.get(key)?.value.as_ref())
    }

    pub fn stage(&mut self, address: Address, key: &str, value: Value,
        validate: impl FnOnce(&str, &Value) -> Result<(), &'static str>) -> Result<(), Error>
    {
        if self.pending.is_some() { return Err(Error::Busy); }
        let draft = self.draft.as_mut().filter(|d| d.address == address).ok_or(Error::Stale)?;
        let baseline = draft.base.get(key).ok_or(Error::UnknownField)?;
        validate(key, &value).map_err(Error::InvalidValue)?;
        if baseline.value.as_ref() == Some(&value) {
            draft.dirty.remove(key);
        } else {
            draft.dirty.insert(baseline.field.key, value);
        }
        Ok(())
    }

    /// Validate every dirty field before returning any patch. Unchanged fields,
    /// profiles, remote configuration and all other Saved members are untouched.
    /// The runner must revalidate returned values and merge into its CURRENT Saved.
    pub fn changes(&self, address: Address, current: &Settings) -> Result<Vec<Change>, Error> {
        if self.pending.is_some() { return Err(Error::Busy); }
        let draft = self.draft.as_ref().filter(|d| d.address == address).ok_or(Error::Stale)?;
        for key in draft.dirty.keys() {
            let baseline = &draft.base[key];
            if current.get(*key) != baseline.value.as_ref()
                || baseline.field.revision_key.and_then(|key| current.get(key)) != baseline.revision.as_ref()
            { return Err(Error::Conflict(*key)); }
        }
        Ok(draft.dirty.iter().map(|(key, value)| Change {
            field: draft.base[key].field, value: value.clone(),
        }).collect())
    }

    /// Call only AFTER the existing writer accepted this exact candidate.
    pub fn mark_saving(&mut self, address: Address) -> Result<(), Error> {
        if self.pending.is_some() { return Err(Error::Busy); }
        if self.address() != Some(address) { return Err(Error::Stale); }
        self.pending = Some(address);
        Ok(())
    }

    /// Cancels the UI draft, not an already accepted durable write.
    pub fn cancel(&mut self, address: Address) -> bool {
        if self.address() != Some(address) { return false; }
        self.draft = None;
        true
    }

    /// Rejection retains a still-owned draft for retry. A retired draft is never
    /// resurrected. Unknown durability terminates editing; runtime owns quarantine.
    pub fn settle(&mut self, address: Address, outcome: Outcome) -> bool {
        if self.pending != Some(address) { return false; }
        self.pending = None;
        if self.address() == Some(address) && outcome != Outcome::Rejected {
            self.draft = None;
        }
        true
    }
}

#[cfg(test)]
#[path = "ott_settings_draft_tests.rs"]
mod tests;
