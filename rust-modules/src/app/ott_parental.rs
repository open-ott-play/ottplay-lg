//! Pure Classic-compatible PIN policy and addressed verification work.
//!
//! The Runtime supplies a fresh session nonce, monotonic time and a source generation,
//! runs `Work` on its existing off-frame worker, and is the sole Saved writer. PINs,
//! records, work and candidates intentionally implement neither Debug nor Serialize.
//! A prepared edit is not a save or a grant. Runtime saving/unknown-state gates remain
//! mandatory; after any write uncertainty call `write_uncertain` until a verified load.

use plx_base::task::OffFrame;
use serde_json::Value;
use std::collections::BTreeMap;

const PREFIX: &str = "pbkdf2-sha256$600000$";
const ITERATIONS: u32 = 600_000;
const GRANT_MS: u64 = 3_600_000;
// The public Classic bootstrap PIN is represented by a verified record too;
// neither loading settings nor checking policy derives a PIN on the UI thread.
const DEFAULT_RECORD: &str = "pbkdf2-sha256$600000$d31e359a309cba65c78598d5ef539f9f$478c66d8e80d7a82bd31b12e84856820ebb83159da63d7cf6c0f388abae81603";
pub(super) const REVISION: &str = "nativeParentalRevision";
type Settings = BTreeMap<String, Value>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    InvalidPin,
    InvalidRecord,
    InvalidPolicy,
    InvalidRevision,
    RevisionExhausted,
    GenerationExhausted,
    Busy,
    Restricted,
    Unavailable,
    Stale,
}

/// Four ASCII digits only. No plaintext is produced by the storage boundary.
pub(super) struct Pin([u8; 4]);
impl Pin {
    pub(super) fn parse(input: &str) -> Result<Self, Error> {
        let bytes: [u8; 4] = input.as_bytes().try_into().map_err(|_| Error::InvalidPin)?;
        if !bytes.iter().all(u8::is_ascii_digit) {
            return Err(Error::InvalidPin);
        }
        Ok(Self(bytes))
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Record {
    salt: [u8; 16],
    digest: [u8; 32],
}
impl Record {
    fn parse(input: &str) -> Result<Self, Error> {
        let body = input.strip_prefix(PREFIX).ok_or(Error::InvalidRecord)?;
        // Bound and validate bytes before slicing; non-ASCII input cannot panic.
        if body.len() != 97 || body.as_bytes()[32] != b'$' {
            return Err(Error::InvalidRecord);
        }
        Ok(Self {
            salt: decode_hex(&body.as_bytes()[..32])?,
            digest: decode_hex(&body.as_bytes()[33..])?,
        })
    }
    fn stored(&self) -> String {
        format!(
            "{PREFIX}{}${}",
            encode_hex(&self.salt),
            encode_hex(&self.digest)
        )
    }
}

fn decode_hex<const N: usize>(input: &[u8]) -> Result<[u8; N], Error> {
    if input.len() != N * 2 {
        return Err(Error::InvalidRecord);
    }
    fn digit(byte: u8) -> Result<u8, Error> {
        match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            _ => Err(Error::InvalidRecord),
        }
    }
    let mut output = [0; N];
    for (slot, pair) in output.iter_mut().zip(input.chunks_exact(2)) {
        *slot = digit(pair[0])? * 16 + digit(pair[1])?;
    }
    Ok(output)
}
fn encode_hex(input: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(input.len() * 2);
    for byte in input {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 15) as usize] as char);
    }
    output
}

/// Application-wide Classic keys. Missing PIN retains the Classic bootstrap;
/// an explicit `*` is the authoritative disabled policy.
/// Legacy plaintext is rejected here; any import migration must explicitly derive
/// and durably save a fresh record before it can become authoritative.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Policy {
    record: Option<Record>,
    channels: bool,
    settings: bool,
    providers: bool,
    revision: u64,
}
impl Policy {
    pub(super) fn read(settings: &Settings) -> Result<Self, Error> {
        let record = match settings.get("parentPin") {
            None => Some(Record::parse(DEFAULT_RECORD)?),
            Some(Value::String(value)) if value == "*" => None,
            Some(Value::String(value)) => Some(Record::parse(value)?),
            _ => return Err(Error::InvalidRecord),
        };
        fn flag(settings: &Settings, key: &str, default: bool) -> Result<bool, Error> {
            match settings.get(key) {
                None => Ok(default),
                Some(value) => match value.as_u64() {
                    Some(0) => Ok(false),
                    Some(1) => Ok(true),
                    _ => Err(Error::InvalidPolicy),
                },
            }
        }
        Ok(Self {
            record,
            channels: flag(settings, "psChannels", true)?,
            settings: flag(settings, "psOptions", false)?,
            providers: flag(settings, "requirePinForProviderSelection", false)?,
            revision: match settings.get(REVISION) {
                None => 0,
                Some(value) => value.as_u64().ok_or(Error::InvalidRevision)?,
            },
        })
    }
    pub(super) fn protects(&self, target: Target) -> bool {
        self.record.is_some()
            && match target {
                Target::Channel { marked } => marked && self.channels,
                Target::Settings => self.settings,
                Target::Providers => self.providers,
                Target::Control => true,
            }
    }
}

/// The owner must advance source_generation on any provider/profile identity change,
/// including A -> B -> A. Catalog repaint alone need not replace this owner.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Context {
    source_generation: u64,
    policy: Policy,
}
impl Context {
    pub(super) fn new(source_generation: u64, policy: Policy) -> Self {
        Self {
            source_generation,
            policy,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Channel { marked: bool },
    Settings,
    Providers,
    Control,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Access {
    Allowed,
    PinRequired,
    Unavailable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PolicyEdit {
    Disable,
    Channels(bool),
    Settings(bool),
    Providers(bool),
}
enum Edit {
    Pin(Record),
    Policy(PolicyEdit),
}

/// Opaque identity only; safe to copy into the addressed result/error closure.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Ticket {
    session: [u8; 16],
    generation: u64,
}
enum WorkKind {
    Verify(Record),
    Create([u8; 16]),
}
pub(super) struct Work {
    ticket: Ticket,
    pin: Pin,
    kind: WorkKind,
}
impl Work {
    pub(super) fn ticket(&self) -> Ticket {
        self.ticket
    }
    /// No IO, worker creation or timers. The shared PBKDF2 is intentionally not
    /// interruptible; cancellation drops authority while retaining the busy slot.
    pub(super) fn run(self, _worker: &OffFrame) -> Completion {
        let result = match self.kind {
            WorkKind::Verify(record) => {
                let digest =
                    plx_base::sha256::pbkdf2_hmac_sha256(&self.pin.0, &record.salt, ITERATIONS);
                WorkResult::Verified(plx_base::sha256::ct_eq(&digest, &record.digest))
            }
            WorkKind::Create(salt) => WorkResult::Created(Record {
                salt,
                digest: plx_base::sha256::pbkdf2_hmac_sha256(&self.pin.0, &salt, ITERATIONS),
            }),
        };
        Completion {
            ticket: self.ticket,
            result,
        }
    }
}
enum WorkResult {
    Verified(bool),
    Created(Record),
    Failed,
}
pub(super) struct Completion {
    ticket: Ticket,
    result: WorkResult,
}
impl Completion {
    pub(super) fn ticket(&self) -> Ticket {
        self.ticket
    }
    #[cfg(all(test, feature = "hostsim"))]
    pub(super) fn verified_fixture(ticket: Ticket) -> Self {
        Self {
            ticket,
            result: WorkResult::Verified(true),
        }
    }

    /// Only send this after the addressed worker was refused or actually ended.
    /// Sending it for a still-running worker would incorrectly free its busy slot.
    pub(super) fn failed(ticket: Ticket) -> Self {
        Self {
            ticket,
            result: WorkResult::Failed,
        }
    }
}
pub(super) enum Outcome {
    Granted,
    Denied,
    Prepared(Change),
    Unavailable,
    Ignored,
}
pub(super) struct Change {
    ticket: Ticket,
    context: Context,
    edit: Edit,
}

/// One instance lives for the Runtime lifetime. Supply a fresh random session nonce
/// from the existing entropy owner, not the stable installation identity or PIN salt.
pub(super) struct Gate {
    session: [u8; 16],
    generation: u64,
    exhausted: bool,
    context: Option<Context>,
    work: Option<Ticket>,
    expires: Option<u64>,
    last_now: Option<u64>,
    uncertain: bool,
}
impl Gate {
    pub(super) fn new(session: [u8; 16]) -> Self {
        Self {
            session,
            generation: 0,
            exhausted: false,
            context: None,
            work: None,
            expires: None,
            last_now: None,
            uncertain: false,
        }
    }
    fn advance(&mut self) {
        match self.generation.checked_add(1) {
            Some(next) => self.generation = next,
            None => {
                self.exhausted = true;
                self.expires = None;
            }
        }
    }
    fn revoke(&mut self) {
        self.expires = None;
        self.advance();
    }
    fn observe(&mut self, context: &Context, now: u64) {
        if self.context.as_ref() != Some(context)
            || self.last_now.is_some_and(|previous| now < previous)
        {
            self.revoke();
            self.context = Some(context.clone());
        }
        self.last_now = Some(now);
        if self.expires.is_some_and(|expires| now >= expires) {
            self.expires = None;
        }
    }
    pub(super) fn access(&mut self, context: &Context, target: Target, now: u64) -> Access {
        self.observe(context, now);
        if self.uncertain {
            Access::Unavailable
        } else if context.policy.protects(target) && self.expires.is_none() {
            Access::PinRequired
        } else {
            Access::Allowed
        }
    }
    fn ticket(&self) -> Ticket {
        Ticket {
            session: self.session,
            generation: self.generation,
        }
    }
    fn start(
        &mut self,
        context: &Context,
        pin: Pin,
        kind: WorkKind,
        now: u64,
    ) -> Result<Work, Error> {
        self.observe(context, now);
        if self.uncertain {
            return Err(Error::Unavailable);
        }
        if self.work.is_some() {
            return Err(Error::Busy);
        }
        if self.exhausted {
            return Err(Error::GenerationExhausted);
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(Error::GenerationExhausted)?;
        let ticket = self.ticket();
        self.work = Some(ticket);
        Ok(Work { ticket, pin, kind })
    }
    pub(super) fn verify(&mut self, context: &Context, pin: Pin, now: u64) -> Result<Work, Error> {
        self.observe(context, now);
        let record = context.policy.record.clone().ok_or(Error::Restricted)?;
        self.start(context, pin, WorkKind::Verify(record), now)
    }
    /// First enablement is allowed only while the authoritative policy is disabled.
    /// Replacing a configured PIN requires an existing grant for Control.
    pub(super) fn create(
        &mut self,
        context: &Context,
        pin: Pin,
        fresh_salt: [u8; 16],
        now: u64,
    ) -> Result<Work, Error> {
        if self.access(context, Target::Control, now) != Access::Allowed {
            return Err(Error::Restricted);
        }
        self.start(context, pin, WorkKind::Create(fresh_salt), now)
    }
    /// A real worker ended while its source/state was unavailable. Retire only
    /// that outstanding ticket, and never grant or publish its result.
    pub(super) fn retire_completion(&mut self, completion: Completion) {
        if self.work == Some(completion.ticket) {
            self.work = None;
            self.revoke();
        }
    }
    pub(super) fn complete(
        &mut self,
        context: &Context,
        completion: Completion,
        now: u64,
    ) -> Outcome {
        self.observe(context, now);
        if self.work != Some(completion.ticket) {
            return Outcome::Ignored;
        }
        self.work = None;
        if completion.ticket != self.ticket() || self.uncertain || self.exhausted {
            return Outcome::Ignored;
        }
        match completion.result {
            WorkResult::Verified(true) => match now.checked_add(GRANT_MS) {
                Some(expires) => {
                    self.expires = Some(expires);
                    Outcome::Granted
                }
                None => {
                    self.revoke();
                    Outcome::Unavailable
                }
            },
            WorkResult::Verified(false) => {
                self.revoke();
                Outcome::Denied
            }
            WorkResult::Created(record) => {
                if self.access(context, Target::Control, now) != Access::Allowed {
                    self.revoke();
                    return Outcome::Unavailable;
                }
                self.advance();
                if self.exhausted {
                    return Outcome::Unavailable;
                }
                Outcome::Prepared(Change {
                    ticket: self.ticket(),
                    context: context.clone(),
                    edit: Edit::Pin(record),
                })
            }
            WorkResult::Failed => {
                self.revoke();
                Outcome::Unavailable
            }
        }
    }
    pub(super) fn prepare(
        &mut self,
        context: &Context,
        edit: PolicyEdit,
        now: u64,
    ) -> Result<Change, Error> {
        if self.access(context, Target::Control, now) != Access::Allowed {
            return Err(Error::Restricted);
        }
        if self.work.is_some() {
            return Err(Error::Busy);
        }
        if self.exhausted {
            return Err(Error::GenerationExhausted);
        }
        Ok(Change {
            ticket: self.ticket(),
            context: context.clone(),
            edit: Edit::Policy(edit),
        })
    }
    /// Produces a same-store candidate only, preserving every unrelated setting.
    /// Call before Runtime.save, then consume the ordinary durable Saved result.
    pub(super) fn candidate(
        &mut self,
        context: &Context,
        change: Change,
        settings: &Settings,
        now: u64,
    ) -> Result<Settings, Error> {
        self.observe(context, now);
        if self.uncertain {
            return Err(Error::Unavailable);
        }
        if self.exhausted {
            return Err(Error::GenerationExhausted);
        }
        if self.access(context, Target::Control, now) != Access::Allowed {
            return Err(Error::Restricted);
        }
        if change.ticket != self.ticket()
            || change.context != *context
            || Policy::read(settings)? != context.policy
        {
            return Err(Error::Stale);
        }
        let mut next = context.policy.clone();
        let (key, value) = match change.edit {
            Edit::Pin(record) => {
                let value = Value::String(record.stored());
                next.record = Some(record);
                ("parentPin", value)
            }
            Edit::Policy(PolicyEdit::Disable) => {
                next.record = None;
                ("parentPin", Value::String("*".into()))
            }
            Edit::Policy(PolicyEdit::Channels(value)) => {
                next.channels = value;
                ("psChannels", Value::from(u8::from(value)))
            }
            Edit::Policy(PolicyEdit::Settings(value)) => {
                next.settings = value;
                ("psOptions", Value::from(u8::from(value)))
            }
            Edit::Policy(PolicyEdit::Providers(value)) => {
                next.providers = value;
                (
                    "requirePinForProviderSelection",
                    Value::from(u8::from(value)),
                )
            }
        };
        let mut candidate = settings.clone();
        if next != context.policy {
            let revision = next
                .revision
                .checked_add(1)
                .ok_or(Error::RevisionExhausted)?;
            candidate.insert(key.into(), value);
            candidate.insert(REVISION.into(), Value::from(revision));
        }
        self.revoke();
        Ok(candidate)
    }
    /// Cancel/back, background, standby, screen-off and definite write failure.
    /// An outstanding worker remains occupied until its real completion arrives.
    pub(super) fn cancel(&mut self) {
        self.revoke();
    }
    pub(super) fn write_uncertain(&mut self) {
        self.uncertain = true;
        self.revoke();
    }
    /// Only the durable load/save owner may confirm authoritative policy. Never
    /// call this for an unverified candidate or Unknown write. Confirmation grants
    /// no access; changed policy and all previous challenges are invalidated.
    pub(super) fn confirmed_policy(&mut self, context: &Context, now: u64) {
        self.revoke();
        self.context = Some(context.clone());
        self.last_now = Some(now);
        self.uncertain = false;
    }
}

#[cfg(test)]
#[path = "ott_parental_tests.rs"]
mod tests;
