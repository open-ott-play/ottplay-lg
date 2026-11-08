//! Bounded immutable locale contexts and one atomic publication boundary.
use super::{Language, LocaleContext, Preference};
use std::sync::{atomic::{AtomicU64, Ordering}, OnceLock};

const SLOT_BITS: u32 = 8;
const SLOT_MASK: u64 = (1 << SLOT_BITS) - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrepareError { NotInitialized, Controlled, ForcedOverride, Unavailable }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishError { NotInitialized, Superseded, Exhausted }

/// Prepared compiled data, addressed to the publication that preceded it.
/// Only the application owner holding a durable settings result may publish it.
/// This token owns no dictionary and is intentionally not Clone.
#[derive(Debug)]
pub struct PreparedLocale { preference: Preference, previous: u64 }
impl PreparedLocale {
    pub fn preference(&self) -> Preference { self.preference }
}

/// Boot captures these values once. Preparing another preference performs no
/// environment, platform, clock, filesystem or network reads.
#[derive(Default)]
pub(super) struct BootInputs {
    pub ui: Option<String>,
    pub format: Option<String>,
    pub clock: Option<String>,
    pub environment: Option<String>,
    pub forced: Option<String>,
    pub(super) controlled: bool,
}
impl BootInputs {
    pub(super) fn controlled() -> Self {
        Self { ui: Some("en-US".into()), format: Some("en-US".into()),
            clock: Some("24".into()), controlled: true, ..Self::default() }
    }
    fn resolve(&self, preference: Preference) -> Result<LocaleContext, PrepareError> {
        let mut context = if let Some(forced) = &self.forced {
            LocaleContext::try_resolve(Preference::from_tag(forced), Some(forced),
                self.format.as_deref(), self.clock.as_deref(), None)?
        } else {
            LocaleContext::try_resolve(preference, self.ui.as_deref(), self.format.as_deref(),
                self.clock.as_deref(), self.environment.as_deref())?
        };
        if self.forced.as_deref() == Some("qps-ploc") { context.language = Language::Pseudo; }
        Ok(context)
    }
}

pub(super) struct Registry {
    inputs: BootInputs,
    contexts: [OnceLock<LocaleContext>; Preference::SHIPPED.len() + 1],
    // Low byte is the generated preference slot; upper bits are the monotonic
    // epoch. One acquire sees a coherent pair, including A -> B -> A changes.
    published: AtomicU64,
}
impl Registry {
    pub(super) fn new(preference: Preference, inputs: BootInputs) -> Result<Self, PrepareError> {
        let context = inputs.resolve(preference)?;
        let registry = Self { inputs, contexts: [const { OnceLock::new() }; Preference::SHIPPED.len() + 1],
            published: AtomicU64::new(u64::from(preference.slot())) };
        registry.contexts[usize::from(preference.slot())].set(context).map_err(|_| PrepareError::Unavailable)?;
        Ok(registry)
    }
    pub(super) fn current(&self) -> &LocaleContext {
        let slot = (self.published.load(Ordering::Acquire) & SLOT_MASK) as usize;
        self.contexts[slot].get().expect("published locale was prepared")
    }
    pub(super) fn epoch(&self) -> u64 { self.published.load(Ordering::Acquire) >> SLOT_BITS }
    pub(super) fn prepare(&self, preference: Preference) -> Result<PreparedLocale, PrepareError> {
        self.prepare_with(preference, BootInputs::resolve)
    }
    fn prepare_with(&self, preference: Preference,
        resolve: impl FnOnce(&BootInputs, Preference) -> Result<LocaleContext, PrepareError>)
        -> Result<PreparedLocale, PrepareError> {
        if self.inputs.controlled { return Err(PrepareError::Controlled); }
        if self.inputs.forced.is_some() { return Err(PrepareError::ForcedOverride); }
        let previous = self.published.load(Ordering::Acquire);
        let slot = &self.contexts[usize::from(preference.slot())];
        if slot.get().is_none() {
            let context = resolve(&self.inputs, preference)?;
            // Another preparer may have won this same finite slot. Its inputs
            // are identical; discard our temporary context, never leak it.
            let _ = slot.set(context);
        }
        Ok(PreparedLocale { preference, previous })
    }
    pub(super) fn publish(&self, candidate: PreparedLocale) -> Result<u64, PublishError> {
        let slot = u64::from(candidate.preference.slot());
        let epoch = candidate.previous >> SLOT_BITS;
        let next_epoch = if candidate.previous & SLOT_MASK == slot { epoch }
            else { epoch.checked_add(1).filter(|next| *next <= (u64::MAX >> SLOT_BITS))
                .ok_or(PublishError::Exhausted)? };
        let next = (next_epoch << SLOT_BITS) | slot;
        self.published.compare_exchange(candidate.previous, next, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| next_epoch).map_err(|_| PublishError::Superseded)
    }
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
