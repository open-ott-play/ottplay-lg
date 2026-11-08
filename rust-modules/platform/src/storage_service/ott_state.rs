//! Pure state for the independent, encrypted OTT domain. No key or IO authority.
use super::{ott, ErrorCode, Flavor};
use super::super::super::wire::{control, saved};
use serde::{Deserialize, Serialize};

const LEDGER_CAPACITY: usize = 16;
// The outer IPC/DB8 frame remains the final, stricter bound after JSON escaping.
const ENCODED_BYTES: usize = ott::FRAME_BYTES;

pub(super) fn object_id(flavor: Flavor) -> &'static str {
    match flavor {
        Flavor::Stable => "ottpriv.stable",
        Flavor::Debug => "ottpriv.debug",
        Flavor::Nightly => "ottpriv.night",
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct State {
    format: String,
    schema: u8,
    flavor: Flavor,
    pub revision: u64,
    pub epoch: u64,
    pub barrier: ott::Barrier,
    pub control: Option<String>,
    pub state: Option<String>,
    #[serde(default = "ott::initialized", skip_serializing_if = "ott::is_initialized")]
    pub private_initialized: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_preferences: Option<ott::PublicPreferences>,
    operations: Vec<Entry>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    operation_id: String,
    digest: String,
    revision: u64,
    epoch: u64,
}

pub(super) fn valid_hex(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn validate_record(
    value: &str,
    namespace: ott::Namespace,
    flavor: Flavor,
) -> Result<(), ErrorCode> {
    if namespace == ott::Namespace::Control && control::is_protected(value) {
        let envelope: control::Envelope = serde_json::from_str(value).map_err(|_| ErrorCode::Invalid)?;
        if Flavor::from_app_id(&envelope.install) != Some(flavor) {
            return Err(ErrorCode::Invalid);
        }
        return control::validate_record(value, &envelope.install);
    }
    if namespace == ott::Namespace::State && saved::is_protected(value) {
        let envelope: saved::Envelope = serde_json::from_str(value).map_err(|_| ErrorCode::Invalid)?;
        if Flavor::from_app_id(&envelope.install) != Some(flavor) {
            return Err(ErrorCode::Invalid);
        }
        return saved::validate_record(value, &envelope.install);
    }
    validate_legacy_record(value, namespace, flavor)
}
fn validate_legacy_record(value: &str, namespace: ott::Namespace, flavor: Flavor) -> Result<(), ErrorCode> {
    let app = ott::envelope_app(value, namespace)?;
    if Flavor::from_app_id(&app) != Some(flavor) {
        return Err(ErrorCode::Invalid);
    }
    Ok(())
}
impl State {
    pub fn decode(text: &str, flavor: Flavor) -> Result<Self, ErrorCode> {
        if text.len() > ENCODED_BYTES {
            return Err(ErrorCode::Corrupt);
        }
        let value: Self = serde_json::from_str(text).map_err(|_| ErrorCode::Corrupt)?;
        value.validate(flavor).map_err(|_| ErrorCode::Corrupt)?;
        Ok(value)
    }
    pub fn encode(&self) -> Result<String, ErrorCode> {
        self.validate(self.flavor)?;
        let text = serde_json::to_string(self).map_err(|_| ErrorCode::Invalid)?;
        if text.len() > ENCODED_BYTES {
            return Err(ErrorCode::Invalid);
        }
        Ok(text)
    }
    fn validate(&self, flavor: Flavor) -> Result<(), ErrorCode> {
        if self.format != "ott-private"
            || self.schema != 1
            || self.flavor != flavor
            || self.revision == 0
            || self.epoch == 0
            || self.epoch > self.revision
            || self.operations.is_empty()
            || self.operations.len() > LEDGER_CAPACITY
            || (self.barrier == ott::Barrier::Pending
                && (self.control.is_some() || self.state.is_some() || self.public_preferences.is_some()))
            || (!self.private_initialized && (self.barrier != ott::Barrier::Open
                || self.control.is_some() || self.state.is_some()))
            || self.public_preferences.as_ref().is_some_and(|prefs| prefs.validate().is_err())
        {
            return Err(ErrorCode::Invalid);
        }
        for (namespace, value) in [
            (ott::Namespace::Control, &self.control),
            (ott::Namespace::State, &self.state),
        ] {
            if let Some(value) = value {
                validate_record(value, namespace, flavor)?;
            }
        }
        let mut previous = 0;
        let mut previous_epoch = 0;
        for (index, op) in self.operations.iter().enumerate() {
            if !valid_hex(&op.operation_id, 32)
                || !valid_hex(&op.digest, 64)
                || op.revision <= previous
                || op.revision > self.revision
                || op.epoch == 0
                || op.epoch < previous_epoch
                || op.epoch > self.epoch
                || self.operations[..index]
                    .iter()
                    .any(|old| old.operation_id == op.operation_id)
            {
                return Err(ErrorCode::Invalid);
            }
            previous = op.revision;
            previous_epoch = op.epoch;
        }
        if previous != self.revision || previous_epoch != self.epoch {
            return Err(ErrorCode::Invalid);
        }
        Ok(())
    }
    pub fn status(&self, db_rev: u64) -> ott::Status {
        ott::Status {
            db_rev: db_rev.to_string(),
            revision: self.revision,
            epoch: self.epoch,
            barrier: self.barrier,
            control_present: self.control.is_some(),
            state_present: self.state.is_some(),
            private_initialized: self.private_initialized,
        }
    }
    pub fn receipt(
        &self,
        id: &str,
        digest: &str,
        verified: bool,
    ) -> Result<Option<ott::Receipt>, ErrorCode> {
        let Some(entry) = self.operations.iter().find(|op| op.operation_id == id) else {
            return Ok(None);
        };
        if entry.digest != digest {
            return Err(ErrorCode::Invalid);
        }
        Ok(Some(ott::Receipt {
            operation_id: id.to_owned(),
            digest: digest.to_owned(),
            revision: entry.revision,
            epoch: entry.epoch,
            verified,
        }))
    }
    pub fn candidate(
        current: Option<&Self>,
        flavor: Flavor,
        mutation: ott::Mutation,
        id: &str,
        digest: &str,
    ) -> Result<Self, ErrorCode> {
        if !valid_hex(id, 32) || !valid_hex(digest, 64) {
            return Err(ErrorCode::Invalid);
        }
        let mut next = current.cloned().unwrap_or(Self {
            format: "ott-private".into(),
            schema: 1,
            flavor,
            revision: 0,
            epoch: 1,
            barrier: ott::Barrier::Open,
            control: None,
            state: None,
            private_initialized: false,
            public_preferences: None,
            operations: Vec::new(),
        });
        match mutation {
            ott::Mutation::Initialize {
                control,
                state,
                barrier,
            } => {
                if current.is_some()
                    || (barrier == ott::Barrier::Pending && (control.is_some() || state.is_some()))
                {
                    return Err(ErrorCode::Invalid);
                }
                if let Some(value) = &control {
                    validate_legacy_record(&value.0, ott::Namespace::Control, flavor)?;
                }
                if let Some(value) = &state {
                    validate_legacy_record(&value.0, ott::Namespace::State, flavor)?;
                }
                next.private_initialized = true;
                next.control = control.map(|v| v.0);
                next.state = state.map(|v| v.0);
                next.barrier = barrier;
            }
            ott::Mutation::CompletePrivateImport { control, state, barrier } => {
                if current.is_none() || next.private_initialized || next.barrier != ott::Barrier::Open
                    || (barrier == ott::Barrier::Pending && (control.is_some() || state.is_some()))
                { return Err(ErrorCode::Invalid); }
                if let Some(value) = &control {
                    validate_legacy_record(&value.0, ott::Namespace::Control, flavor)?;
                }
                if let Some(value) = &state {
                    validate_legacy_record(&value.0, ott::Namespace::State, flavor)?;
                }
                next.private_initialized = true;
                next.control = control.map(|value| value.0);
                next.state = state.map(|value| value.0);
                next.barrier = barrier;
                if barrier == ott::Barrier::Pending { next.public_preferences = None; }
            }
            ott::Mutation::PatchPublic { patch } => {
                if next.barrier == ott::Barrier::Pending { return Err(ErrorCode::Invalid); }
                next.public_preferences = Some(next.public_preferences.clone().unwrap_or_default().apply(&patch)?);
            }
            ott::Mutation::Put { namespace, value } => {
                if current.is_none() || !next.private_initialized || next.barrier == ott::Barrier::Pending {
                    return Err(ErrorCode::Invalid);
                }
                match namespace {
                    ott::Namespace::Control => {
                        validate_legacy_record(&value.0, ott::Namespace::Control, flavor)?;
                        if next.control.as_deref().is_some_and(control::is_protected) {
                            return Err(ErrorCode::Invalid);
                        }
                        next.control = Some(value.0);
                    }
                    ott::Namespace::State => {
                        validate_legacy_record(&value.0, ott::Namespace::State, flavor)?;
                        if next.state.as_deref().is_some_and(saved::is_protected) {
                            return Err(ErrorCode::Invalid);
                        }
                        next.state = Some(value.0);
                    }
                }
            }
            ott::Mutation::BeginErase {} => {
                if current.is_some() && next.barrier != ott::Barrier::Pending {
                    next.epoch = next.epoch.checked_add(1).ok_or(ErrorCode::Invalid)?;
                }
                next.control = None;
                next.state = None;
                next.private_initialized = true;
                next.public_preferences = None;
                next.barrier = ott::Barrier::Pending;
            }
            ott::Mutation::FinishErase {} => {
                if current.is_none() || next.barrier != ott::Barrier::Pending {
                    return Err(ErrorCode::Invalid);
                }
                next.barrier = ott::Barrier::Complete;
            }
        }
        next.finish(flavor, id, digest)
    }

    /// Only typed adapters may publish helper-encrypted values into their fixed slot.
    pub fn protected_candidate(current: Option<&Self>, flavor: Flavor, namespace: ott::Namespace,
        value: String, id: &str, digest: &str) -> Result<Self, ErrorCode> {
        let protected = match namespace {
            ott::Namespace::Control => control::is_protected(&value),
            ott::Namespace::State => saved::is_protected(&value),
        };
        if !valid_hex(id, 32) || !valid_hex(digest, 64) || !protected {
            return Err(ErrorCode::Invalid);
        }
        if current.is_some_and(|state| !state.private_initialized || state.barrier == ott::Barrier::Pending) {
            return Err(ErrorCode::Invalid);
        }
        validate_record(&value, namespace, flavor)?;
        let mut next = current.cloned().unwrap_or(Self {
            format: "ott-private".into(), schema: 1, flavor, revision: 0, epoch: 1,
            barrier: ott::Barrier::Open, control: None, state: None,
            private_initialized: true, public_preferences: None, operations: Vec::new(),
        });
        match namespace {
            ott::Namespace::Control => next.control = Some(value),
            ott::Namespace::State => next.state = Some(value),
        }
        next.finish(flavor, id, digest)
    }

    fn finish(mut self, flavor: Flavor, id: &str, digest: &str) -> Result<Self, ErrorCode> {
        self.revision = self.revision.checked_add(1).ok_or(ErrorCode::Invalid)?;
        self.operations.push(Entry {
            operation_id: id.to_owned(),
            digest: digest.to_owned(),
            revision: self.revision,
            epoch: self.epoch,
        });
        if self.operations.len() > LEDGER_CAPACITY {
            self.operations.remove(0);
        }
        self.validate(flavor)?;
        Ok(self)
    }
}
