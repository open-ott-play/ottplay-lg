//! Programme-reminder projection owned by the existing OTT runtime.
//!
//! `Saved` remains the only persistence authority. Proposals go through its
//! verified writer, and the existing clock tick supplies calendar time. Effects
//! belong to the shared UI owner; accepting one yields an intent that still has
//! to pass the ordinary UI/playback admission path.
use super::Saved;
use plx_data::ott::reminders::{
    Book, Candidate, CommitOutcome, CommitToken, Context, Error, Fx, Key, PromptToken, Record,
    LEAD_SECONDS_LIMIT,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SaveFailure {
    Rejected,
    Unknown,
}

pub(super) struct Finished {
    pub outcome: CommitOutcome,
    pub effects: Vec<Fx>,
}

#[derive(Default)]
pub(super) struct State {
    book: Option<Book>,
    pending: Option<Candidate>,
    last_owner: u64,
    prompt: Option<(PromptToken, Key)>,
}

fn calendar_seconds(wall_ms: u64) -> Result<i64, &'static str> {
    i64::try_from(wall_ms / 1000).map_err(|_| "Reminder calendar time is unavailable")
}

fn message(error: Error) -> &'static str {
    match error {
        Error::TooLarge => "Reminder storage limit reached",
        Error::NotFuture => "Only a future programme can be remembered",
        Error::Busy => "A reminder change is still being saved",
        Error::ReadOnly => "Reminder storage must be reloaded before editing",
        Error::Stale => "This reminder request is no longer current",
        Error::InvalidTime => "Reminder calendar time is unavailable",
        Error::Exhausted => "Reminder request identity is exhausted",
        Error::InvalidDocument | Error::ForeignSource | Error::UnresolvedLegacy => {
            "Reminder data is unavailable"
        }
    }
}

impl State {
    /// Only a verified load may call this. The UI owner closes its old reminder
    /// card before replacement, and supplies a fresh nonzero owner identity.
    /// Invalid input leaves this projection unavailable, never an empty book.
    pub(super) fn load_committed(
        &mut self,
        saved: &Saved,
        wall_ms: u64,
        owner: u64,
    ) -> Result<(), &'static str> {
        self.invalidate();
        if owner == 0 || owner <= self.last_owner {
            return Err("Reminder load identity is no longer current");
        }
        self.last_owner = owner;
        self.book = Some(
            Book::new(saved.reminders.clone(), calendar_seconds(wall_ms)?, owner)
                .map_err(message)?,
        );
        Ok(())
    }

    /// Called on uncertain global persistence and teardown as well as load.
    /// The UI owner must also withdraw the old card. Its token cannot be used
    /// against an unavailable or subsequently reloaded book.
    pub(super) fn invalidate(&mut self) {
        self.book = None;
        self.pending = None;
        self.prompt = None;
    }

    pub(super) fn contains(&self, key: &Key) -> bool {
        self.book
            .as_ref()
            .is_some_and(|book| book.document().records().iter().any(|r| &r.key == key))
    }

    /// The caller has resolved the selected guide row under current source
    /// authority. No list index, URL or metadata-only revision enters a key.
    /// A dispatch refusal must finish this token as Rejected immediately.
    pub(super) fn toggle(
        &mut self,
        saved: &Saved,
        record: Record,
        wall_ms: u64,
    ) -> Result<(Saved, CommitToken), &'static str> {
        let book = self
            .book
            .as_mut()
            .ok_or("Reminder storage is unavailable")?;
        if self.pending.is_some() {
            return Err(message(Error::Busy));
        }
        // An unrelated or stale snapshot must never overwrite committed rows.
        if book.document() != &saved.reminders {
            return Err("Reminder storage changed; reload it before editing");
        }
        let candidate = if book
            .document()
            .records()
            .iter()
            .any(|r| r.key == record.key)
        {
            book.remove(&record.key)
        } else {
            book.upsert(record, calendar_seconds(wall_ms)?)
        }
        .map_err(message)?;
        let mut next = saved.clone();
        next.reminders = candidate.document.clone();
        let token = candidate.token;
        self.pending = Some(candidate);
        Ok((next, token))
    }

    /// This token belongs to the existing writer's terminal completion. Even a
    /// successful writer result cannot commit a different reminder document.
    /// An exact-document mismatch is Unknown, so its close effect is delivered
    /// and the projection remains read-only until an authoritative reload.
    pub(super) fn finish_save(
        &mut self,
        token: CommitToken,
        result: Result<&Saved, SaveFailure>,
    ) -> Result<Finished, &'static str> {
        let candidate = self
            .pending
            .as_ref()
            .filter(|p| p.token == token)
            .ok_or(message(Error::Stale))?;
        let outcome = match result {
            Ok(saved) if saved.reminders == candidate.document => CommitOutcome::Committed,
            Ok(_) | Err(SaveFailure::Unknown) => CommitOutcome::Unknown,
            Err(SaveFailure::Rejected) => CommitOutcome::Rejected,
        };
        let effects = self
            .book
            .as_mut()
            .ok_or("Reminder storage is unavailable")?
            .commit(token, outcome)
            .map_err(message)?;
        self.pending = None;
        self.track_prompt(&effects);
        Ok(Finished { outcome, effects })
    }

    /// Called only from the existing tick, with raw calendar time and the
    /// source/admission generation, not the metadata-driven catalog revision.
    /// `resolves` must only inspect already loaded rows; it must not fetch.
    pub(super) fn poll(
        &mut self,
        wall_ms: u64,
        context: Context<'_>,
        lead_minutes: u32,
        resolves: impl Fn(&Key) -> bool,
    ) -> Result<Vec<Fx>, &'static str> {
        let Some(book) = self.book.as_mut() else {
            return Ok(Vec::new());
        };
        let lead_s = lead_minutes.saturating_mul(60).min(LEAD_SECONDS_LIMIT);
        let effects = book
            .poll(calendar_seconds(wall_ms)?, context, lead_s, resolves)
            .map_err(message)?;
        self.track_prompt(&effects);
        Ok(effects)
    }

    fn track_prompt(&mut self, effects: &[Fx]) {
        for effect in effects {
            match effect {
                Fx::PromptSwitch { token, record } => {
                    self.prompt = Some((*token, record.key.clone()))
                }
                Fx::ClosePrompt(token)
                    if self
                        .prompt
                        .as_ref()
                        .is_some_and(|(current, _)| current == token) =>
                {
                    self.prompt = None
                }
                _ => {}
            }
        }
    }

    /// Resolve this stable key before accepting the token. Book remains the authority.
    pub(super) fn prompt_key(&self, token: PromptToken) -> Option<&Key> {
        self.prompt
            .as_ref()
            .filter(|(current, _)| *current == token)
            .map(|(_, key)| key)
    }

    /// A single-use intent, not permission to start playback. The caller must
    /// take the ordinary admitted UI play path with this current stable key.
    pub(super) fn accept(
        &mut self,
        token: PromptToken,
        wall_ms: u64,
        context: Context<'_>,
        resolves: bool,
    ) -> Option<Record> {
        let accepted =
            self.book
                .as_mut()?
                .accept(token, calendar_seconds(wall_ms).ok()?, context, resolves);
        if accepted.is_some() {
            self.prompt = None;
        }
        accepted
    }

    pub(super) fn dismiss(&mut self, token: PromptToken) -> bool {
        let dismissed = self.book.as_mut().is_some_and(|book| book.dismiss(token));
        if dismissed {
            self.prompt = None;
        }
        dismissed
    }
}

#[cfg(test)]
#[path = "ott_reminders_runtime_tests.rs"]
mod tests;
