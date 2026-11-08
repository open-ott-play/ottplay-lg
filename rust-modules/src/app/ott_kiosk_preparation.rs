//! Opaque address of an existing Local/Plex kiosk preparation attempt.
//! Recovery owns this receipt. No extra request, worker, counter or durable state.
use super::ott_runtime::{LocalKioskPreparation, PlexKioskPreparation};

#[derive(Clone)]
pub(super) struct KioskPreparation(Kind);

#[derive(Clone)]
enum Kind {
    Local(LocalKioskPreparation),
    Plex(PlexKioskPreparation),
}

impl KioskPreparation {
    pub(super) fn from_local(value: LocalKioskPreparation) -> Self {
        Self(Kind::Local(value))
    }
    pub(super) fn from_plex(value: PlexKioskPreparation) -> Self {
        Self(Kind::Plex(value))
    }
    pub(super) fn local(&self) -> Option<&LocalKioskPreparation> {
        match &self.0 {
            Kind::Local(value) => Some(value),
            Kind::Plex(_) => None,
        }
    }
    pub(super) fn plex(&self) -> Option<&PlexKioskPreparation> {
        match &self.0 {
            Kind::Plex(value) => Some(value),
            Kind::Local(_) => None,
        }
    }
    /// Only the addressed attempt may finish Recovery. A refreshed native ticket
    /// on successful admission does not manufacture another attempt identity.
    pub(super) fn same_attempt(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Kind::Local(a), Kind::Local(b)) => a.same_attempt(b),
            (Kind::Plex(a), Kind::Plex(b)) => a.same_attempt(b),
            _ => false,
        }
    }
}
