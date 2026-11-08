//! Pure programme reminders. The existing runtime supplies time, resolves channel
//! identities and commits documents through its protected-state writer. Nothing
//! here schedules a timer, reads a clock, starts playback or performs IO.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const DOCUMENT_BYTES: usize = 256 * 1024;
pub const RECORD_LIMIT: usize = 128;
pub const ID_BYTES: usize = 512;
pub const TITLE_BYTES: usize = 2048;
pub const LEAD_SECONDS_LIMIT: u32 = 86_400;
const LAST_SECOND: i64 = 253_402_300_799;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Key {
    /// Opaque stable source identity supplied by the owner; never a raw URL.
    pub source_id: String,
    pub channel_id: String,
    pub programme_id: String,
    pub start_s: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub key: Key,
    pub end_s: i64,
    pub title: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    version: u8,
    records: Vec<Record>,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            version: 1,
            records: Vec::new(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidDocument,
    TooLarge,
    InvalidTime,
    NotFuture,
    Busy,
    ReadOnly,
    Stale,
    Exhausted,
    ForeignSource,
    UnresolvedLegacy,
}
fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
fn valid_record(record: &Record) -> bool {
    text(&record.key.source_id, ID_BYTES)
        && text(&record.key.channel_id, ID_BYTES)
        && text(&record.key.programme_id, ID_BYTES)
        && (0..=LAST_SECOND).contains(&record.key.start_s)
        && record.end_s > record.key.start_s
        && record.end_s <= LAST_SECOND
        && record.title.len() <= TITLE_BYTES
        && !record
            .title
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
}
impl Document {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > DOCUMENT_BYTES {
            return Err(Error::TooLarge);
        }
        let document: Self = serde_json::from_slice(bytes).map_err(|_| Error::InvalidDocument)?;
        document.validate()?;
        Ok(document)
    }
    pub fn records(&self) -> &[Record] {
        &self.records
    }
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|_| Error::InvalidDocument)
    }
    fn validate(&self) -> Result<(), Error> {
        if self.version != 1 {
            return Err(Error::InvalidDocument);
        }
        if self.records.len() > RECORD_LIMIT {
            return Err(Error::TooLarge);
        }
        let mut keys = BTreeSet::new();
        if self
            .records
            .iter()
            .any(|r| !valid_record(r) || !keys.insert(&r.key))
        {
            return Err(Error::InvalidDocument);
        }
        if serde_json::to_vec(self)
            .map_err(|_| Error::InvalidDocument)?
            .len()
            > DOCUMENT_BYTES
        {
            return Err(Error::TooLarge);
        }
        Ok(())
    }
}
/// One atomic import candidate. The owner commits this document and source
/// claim together; it must not erase or claim the legacy input on failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migration {
    pub document: Document,
    pub claim_source: String,
}
pub fn migrate_legacy(
    source_id: &str,
    claimed_source: Option<&str>,
    bytes: &[u8],
    resolve: impl Fn(&str, i64) -> Option<Key>,
) -> Result<Migration, Error> {
    if !text(source_id, ID_BYTES) {
        return Err(Error::InvalidDocument);
    }
    if claimed_source.is_some_and(|claim| claim != source_id) {
        return Err(Error::ForeignSource);
    }
    if bytes.len() > DOCUMENT_BYTES {
        return Err(Error::TooLarge);
    }
    let values: Vec<serde_json::Value> =
        serde_json::from_slice(bytes).map_err(|_| Error::InvalidDocument)?;
    if values.len() > RECORD_LIMIT {
        return Err(Error::TooLarge);
    }
    let mut records = Vec::new();
    for value in values {
        let channel = value["ci"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| value["ci"].as_i64().map(|n| n.to_string()))
            .ok_or(Error::InvalidDocument)?;
        let start_s = value["t"].as_i64().ok_or(Error::InvalidDocument)?;
        let key = resolve(&channel, start_s).ok_or(Error::UnresolvedLegacy)?;
        if key.source_id != source_id || key.start_s != start_s {
            return Err(Error::ForeignSource);
        }
        records.push(Record {
            key,
            end_s: value["te"].as_i64().ok_or(Error::InvalidDocument)?,
            title: value["n"].as_str().ok_or(Error::InvalidDocument)?.into(),
        });
    }
    let document = Document {
        version: 1,
        records,
    };
    document.validate()?;
    Ok(Migration {
        document,
        claim_source: source_id.into(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommitToken {
    owner: u64,
    serial: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromptToken {
    owner: u64,
    serial: u64,
}
/// `Committed` means the runtime authenticated/read back this exact candidate,
/// not merely that a write was dispatched or a helper accepted a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitOutcome {
    Committed,
    Rejected,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub token: CommitToken,
    pub document: Document,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Context<'a> {
    pub source_id: &'a str,
    /// Changes whenever channel resolution/admission authority changes.
    pub revision: u64,
    pub active: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fx {
    LeadNotice { record: Record, minutes: u32 },
    PromptSwitch { token: PromptToken, record: Record },
    ClosePrompt(PromptToken),
}
struct Prompt {
    token: PromptToken,
    record: Record,
    revision: u64,
}
/// A bounded projection of the runtime's committed document. Reload with a new
/// owner token after uncertain persistence; never treat an unreadable store as
/// `Document::default()`.
pub struct Book {
    owner: u64,
    serial: u64,
    document: Document,
    pending: Option<Candidate>,
    writable: bool,
    notified: BTreeSet<Key>,
    prompted: BTreeSet<Key>,
    prompt: Option<Prompt>,
    context: Option<(String, u64, bool)>,
}
impl Book {
    pub fn new(document: Document, wall_now_s: i64, owner: u64) -> Result<Self, Error> {
        document.validate()?;
        if !(0..=LAST_SECOND).contains(&wall_now_s) || owner == 0 {
            return Err(Error::InvalidTime);
        }
        let passed: BTreeSet<_> = document
            .records
            .iter()
            .filter(|r| r.key.start_s <= wall_now_s)
            .map(|r| r.key.clone())
            .collect();
        Ok(Self {
            owner,
            serial: 0,
            document,
            pending: None,
            writable: true,
            notified: passed.clone(),
            prompted: passed,
            prompt: None,
            context: None,
        })
    }
    pub fn document(&self) -> &Document {
        &self.document
    }
    pub fn writable(&self) -> bool {
        self.writable
    }
    fn next_serial(&mut self) -> Result<u64, Error> {
        self.serial = self.serial.checked_add(1).ok_or(Error::Exhausted)?;
        Ok(self.serial)
    }
    fn propose(&mut self, document: Document) -> Result<Candidate, Error> {
        if !self.writable {
            return Err(Error::ReadOnly);
        }
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        document.validate()?;
        let token = CommitToken {
            owner: self.owner,
            serial: self.next_serial()?,
        };
        let candidate = Candidate { token, document };
        self.pending = Some(candidate.clone());
        Ok(candidate)
    }
    pub fn upsert(&mut self, record: Record, wall_now_s: i64) -> Result<Candidate, Error> {
        if !valid_record(&record) {
            return Err(Error::InvalidDocument);
        }
        if !(0..=LAST_SECOND).contains(&wall_now_s) {
            return Err(Error::InvalidTime);
        }
        if record.key.start_s <= wall_now_s {
            return Err(Error::NotFuture);
        }
        let mut document = self.document.clone();
        document.records.retain(|r| r.key != record.key);
        document.records.push(record);
        self.propose(document)
    }
    pub fn remove(&mut self, key: &Key) -> Result<Candidate, Error> {
        let mut document = self.document.clone();
        document.records.retain(|r| &r.key != key);
        self.propose(document)
    }
    pub fn commit(
        &mut self,
        token: CommitToken,
        outcome: CommitOutcome,
    ) -> Result<Vec<Fx>, Error> {
        if self.pending.as_ref().map(|p| p.token) != Some(token) {
            return Err(Error::Stale);
        }
        let candidate = self.pending.take().ok_or(Error::Stale)?;
        if outcome == CommitOutcome::Unknown {
            self.writable = false;
            let mut effects = Vec::new();
            self.close_prompt(&mut effects);
            return Ok(effects);
        }
        if outcome != CommitOutcome::Committed {
            return Ok(Vec::new());
        }
        let unchanged: BTreeSet<_> = candidate
            .document
            .records
            .iter()
            .filter(|r| self.document.records.contains(r))
            .map(|r| r.key.clone())
            .collect();
        self.notified.retain(|key| unchanged.contains(key));
        self.prompted.retain(|key| unchanged.contains(key));
        self.document = candidate.document;
        let mut effects = Vec::new();
        if self
            .prompt
            .as_ref()
            .is_some_and(|p| !self.document.records.contains(&p.record))
        {
            self.close_prompt(&mut effects);
        }
        Ok(effects)
    }
    fn close_prompt(&mut self, effects: &mut Vec<Fx>) {
        if let Some(prompt) = self.prompt.take() {
            effects.push(Fx::ClosePrompt(prompt.token));
        }
    }
    /// `resolves` is a pure lookup in the already loaded, current catalogue. It
    /// must not start provider work. Category list positions never enter a key.
    pub fn poll(
        &mut self,
        wall_now_s: i64,
        context: Context<'_>,
        lead_s: u32,
        resolves: impl Fn(&Key) -> bool,
    ) -> Result<Vec<Fx>, Error> {
        if !(0..=LAST_SECOND).contains(&wall_now_s) || lead_s > LEAD_SECONDS_LIMIT {
            return Err(Error::InvalidTime);
        }
        let mut effects = Vec::new();
        let changed = self
            .context
            .as_ref()
            .is_some_and(|(source, revision, active)| {
                source != context.source_id
                    || *revision != context.revision
                    || *active != context.active
            });
        if changed {
            self.close_prompt(&mut effects);
        }
        // A source/activity transition is not a reason to replay a missed start.
        if changed || !context.active {
            for record in &self.document.records {
                if record.key.start_s <= wall_now_s {
                    self.prompted.insert(record.key.clone());
                    self.notified.insert(record.key.clone());
                }
            }
        }
        self.context = Some((context.source_id.into(), context.revision, context.active));
        if self.prompt.as_ref().is_some_and(|p| {
            p.record.end_s <= wall_now_s
                || p.record.key.start_s > wall_now_s
                || !resolves(&p.record.key)
        }) {
            self.close_prompt(&mut effects);
        }
        if !context.active || !self.writable || self.pending.is_some() {
            return Ok(effects);
        }
        let mut due: Vec<_> = self
            .document
            .records
            .iter()
            .filter(|r| {
                r.key.source_id == context.source_id && r.end_s > wall_now_s && resolves(&r.key)
            })
            .cloned()
            .collect();
        due.sort_by(|a, b| {
            a.key
                .start_s
                .cmp(&b.key.start_s)
                .then_with(|| a.key.cmp(&b.key))
        });
        for record in due {
            if record.key.start_s > wall_now_s {
                if lead_s > 0
                    && record.key.start_s - wall_now_s <= i64::from(lead_s)
                    && self.notified.insert(record.key.clone())
                {
                    let minutes = ((record.key.start_s - wall_now_s + 59) / 60) as u32;
                    effects.push(Fx::LeadNotice { record, minutes });
                }
            } else if self.prompt.is_none() && !self.prompted.contains(&record.key) {
                let token = PromptToken {
                    owner: self.owner,
                    serial: self.next_serial()?,
                };
                self.prompted.insert(record.key.clone());
                self.prompt = Some(Prompt {
                    token,
                    record: record.clone(),
                    revision: context.revision,
                });
                effects.push(Fx::PromptSwitch { token, record });
            }
        }
        Ok(effects)
    }
    pub fn dismiss(&mut self, token: PromptToken) -> bool {
        if self.prompt.as_ref().map(|p| p.token) != Some(token) {
            return false;
        }
        self.prompt = None;
        true
    }
    /// Returns only an intent. The owner must recheck ordinary parental/kiosk,
    /// playback and source admission before its existing play path executes it.
    pub fn accept(
        &mut self,
        token: PromptToken,
        wall_now_s: i64,
        context: Context<'_>,
        resolves: bool,
    ) -> Option<Record> {
        let prompt = self.prompt.as_ref()?;
        if prompt.token != token
            || self.pending.is_some()
            || !self.writable
            || !context.active
            || !resolves
            || prompt.revision != context.revision
            || prompt.record.key.source_id != context.source_id
            || wall_now_s < prompt.record.key.start_s
            || wall_now_s >= prompt.record.end_s
            || !self.document.records.contains(&prompt.record)
        {
            return None;
        }
        self.prompt.take().map(|p| p.record)
    }
}
#[cfg(test)]
#[path = "reminders_tests.rs"]
mod tests;
