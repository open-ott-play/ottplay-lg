//! App-owned resource preparation for the existing SessionMachine commit pipeline.
//! No serialized owner command can manufacture the retained ImportProof.
use super::{Resources, SessionAdapter};
use plx_session::auth::owner::{CommitAdmission, CommitPermit, CommitPlan, CommitReply,
    CredentialPatch, ManagedActivation, ProfileScope, RegistryPlan};
use plx_plex::plex::{self, grant::ImportProof, session::SourceRef};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;
use plx_base::task::time::MainDeadline;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ManagedStage { Prepared, EffectsStarted, RegistryApplied, Published, Refused, Retired }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ManagedError { Busy, Invalid, Stale, Expired, AuthorizationConflict, Unavailable }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ManagedCompletion {
    pub operation: u64, pub req: u32, pub epoch: u64, pub arrival: u64,
    pub accepted: bool, pub error: Option<ManagedError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ManagedAdmissionRefusal {
    pub operation: u64, pub expected_epoch: u64, pub error: ManagedError,
}

#[derive(Default)]
pub(super) struct State {
    record: Option<Record>,
    completion: Option<ManagedCompletion>,
    admission_refusal: Option<ManagedAdmissionRefusal>,
    publication: Option<(u64, ProfileScope)>,
    #[cfg(test)]
    observation: Option<plx_base::task::time::ClockSample>,
}
struct Record {
    activation: ManagedActivation,
    source: SourceRef,
    proof: ImportProof,
    physical_credentials: CredentialPatch,
    deadline: MainDeadline,
    context_deadline: MainDeadline,
    cancelled: Arc<AtomicBool>,
    stage: ManagedStage,
    correlation: Option<(u32, u64, u64)>,
    registry: Option<RegistryOwnership>,
    settled: bool,
    error: Option<ManagedError>,
}
#[derive(Clone)]
enum RegistryOwnership {
    Owned(plex::ImportedLease),
    Borrowed { id: plex::ServerId, lifecycle: plx_session::auth::ClientLifecycle,
        expected: plx_session::auth::owner::ServerLifecycle },
}
impl RegistryOwnership {
    fn id(&self) -> plex::ServerId {
        match self { Self::Owned(lease) => lease.server_id(), Self::Borrowed { id, .. } => *id }
    }
    fn current(&self, source: &SourceRef) -> bool {
        let Some(origin) = source.origin() else { return false };
        match self {
            Self::Owned(lease) => plex::imported_lease_matches(lease, &source.machine_id, &origin, &source.token),
            Self::Borrowed { id, lifecycle, expected } => lifecycle.is_current(*expected)
                && plex::client_for(*id).is_some_and(|client|
                    client.machine_id() == source.machine_id && client.origin() == &origin && client.uses_token(&source.token)),
        }
    }
    fn retire(self) {
        if let Self::Owned(lease) = self { let _ = plex::retire_imported(&lease); }
    }
}
fn same_source(a: &SourceRef, b: &SourceRef) -> bool {
    match (serde_json::to_value(a), serde_json::to_value(b)) { (Ok(a), Ok(b)) => a == b, _ => false }
}
fn same_activation(a: &ManagedActivation, b: &ManagedActivation) -> bool {
    a.operation == b.operation && a.expected_epoch == b.expected_epoch && a.expected_disk == b.expected_disk
        && a.expected_runtime.same_credentials(&b.expected_runtime) && a.credentials.same_credentials(&b.credentials)
}
fn source_bound(value: &ManagedActivation, source: &SourceRef, proof: &ImportProof) -> bool {
    let patch = &value.credentials;
    source.origin().is_some_and(|origin| proof.matches_source(&origin, &source.machine_id))
        && !source.token.is_empty() && source.tier.is_some()
        && patch.sources.len() == 1 && same_source(&patch.sources[0], source)
        && patch.server.machine_id == source.machine_id && source.origin().is_some_and(|origin| patch.server.origin() == origin)
        && patch.server.token == source.token && patch.user.token == source.token
        && patch.user.account_scope == plex::session::AccountScope::PmsOnly
        && patch.account_token.is_empty() && patch.user.plex_tv_token.is_none()
        && patch.home_users.is_empty() && patch.profiles.is_empty()
        && !patch.client_id.is_empty() && patch.client_id == value.expected_runtime.client_id
}

impl State {
    pub(super) fn observed_publication(&mut self, epoch: u64, scope: ProfileScope) {
        self.publication = Some((epoch, scope));
    }
}
impl SessionAdapter {
    fn managed_remaining(&self, deadline: MainDeadline) -> Duration {
        #[cfg(test)]
        if let Some(sample) = self.managed.observation { return deadline.remaining_at_sample(sample); }
        deadline.remaining()
    }
    pub(crate) fn prepare_managed(&mut self, activation: ManagedActivation, source: SourceRef,
        proof: ImportProof, deadline: MainDeadline, context_deadline: MainDeadline, cancelled: Arc<AtomicBool>) -> Result<(), ManagedError> {
        let remaining = self.managed_remaining(deadline);
        let context_remaining = self.managed_remaining(context_deadline);
        if remaining.is_zero() || context_remaining.is_zero() { return Err(ManagedError::Expired); }
        if self.controlled_home || remaining > Duration::from_secs(60)
            || context_remaining > Duration::from_secs(120) || activation.operation == 0
            || !source_bound(&activation, &source, &proof) { return Err(ManagedError::Invalid); }
        if self.managed.record.as_ref().is_some_and(|record| record.stage != ManagedStage::Retired)
            || self.managed.completion.is_some() || self.managed.admission_refusal.is_some() { return Err(ManagedError::Busy); }
        if cancelled.load(Ordering::Acquire) || !proof.current() { return Err(ManagedError::Stale); }
        let physical_credentials = self.managed_disk_snapshot(&activation).ok_or(ManagedError::Stale)?;
        if self.managed_remaining(deadline).is_zero() || self.managed_remaining(context_deadline).is_zero() {
            return Err(ManagedError::Expired);
        }
        self.managed.publication = None;
        self.managed.record = Some(Record { activation, source, proof, physical_credentials,
            deadline, context_deadline, cancelled, stage: ManagedStage::Prepared,
            correlation: None, registry: None, settled: false, error: None });
        Ok(())
    }
    fn managed_disk_snapshot(&self, activation: &ManagedActivation) -> Option<CredentialPatch> {
        match &self.resources {
            Resources::Live { .. } => plex::session::peek_settled()
                .filter(|disk| activation.expected_disk.matches(disk))
                .map(|disk| CredentialPatch::of(&disk)),
            #[cfg(test)] Resources::Fixture(resources) => activation.expected_disk.matches(&resources.disk)
                .then(|| CredentialPatch::of(&resources.disk)),
        }
    }
    fn managed_disk_current(&self, record: &Record) -> bool {
        self.managed_disk_snapshot(&record.activation)
            .is_some_and(|now| now.same_credentials(&record.physical_credentials))
    }
    pub(crate) fn managed_stage(&self, operation: u64) -> Option<ManagedStage> {
        self.managed.record.as_ref().filter(|r| r.activation.operation == operation).map(|r| r.stage)
    }
    pub(crate) fn take_managed_completion(&mut self) -> Option<ManagedCompletion> {
        let completion = self.managed.completion.take()?;
        if completion.accepted {
            let record = self.managed.record.as_ref()?;
            if record.stage != ManagedStage::Published || record.cancelled.load(Ordering::Acquire)
                || !record.proof.current() || !self.managed_disk_current(record)
                || self.managed_remaining(record.deadline).is_zero()
                || !record.registry.as_ref().is_some_and(|registry|
                    registry.current(&record.source) && plex::current_server() == registry.id())
                || self.managed_remaining(record.deadline).is_zero() { return None; }
        }
        Some(completion)
    }
    pub(crate) fn take_managed_admission_refusal(&mut self) -> Option<ManagedAdmissionRefusal> {
        self.managed.admission_refusal.take()
    }
    pub(crate) fn managed_admission_refused(&mut self, operation: u64, expected_epoch: u64) {
        let Some(record) = self.managed.record.as_mut() else { return };
        if record.stage != ManagedStage::Prepared || record.settled
            || record.activation.operation != operation || record.activation.expected_epoch != expected_epoch { return; }
        record.stage = ManagedStage::Refused;
        record.settled = true;
        self.managed.admission_refusal = Some(ManagedAdmissionRefusal {
            operation, expected_epoch, error: ManagedError::Stale });
    }
    pub(crate) fn managed_operation(&self) -> Option<u64> {
        self.managed.record.as_ref().map(|record| record.activation.operation)
    }
    /// Terminal withdrawal. Owned imports are retired; borrowed account slots are untouched.
    /// A post-effect tombstone remains observable until a later explicit preparation.
    pub(crate) fn cancel_managed(&mut self, operation: u64) -> Option<ManagedStage> {
        let record = self.managed.record.as_mut().filter(|r| r.activation.operation == operation)?;
        let prior = record.stage;
        record.cancelled.store(true, Ordering::Release);
        if let Some(registry) = record.registry.take() { registry.retire(); }
        self.managed.completion = None;
        self.managed.admission_refusal = None;
        if matches!(prior, ManagedStage::Prepared | ManagedStage::Refused) {
            self.managed.record = None;
        } else { record.stage = ManagedStage::Retired; }
        Some(prior)
    }
    pub(crate) fn begin_managed_commit(&mut self, permit: CommitPermit<'_>, plan: &CommitPlan,
        activation: Option<&ManagedActivation>) -> CommitReply {
        let tuple = (permit.request(), permit.epoch(), permit.arrival());
        let Some(mut record) = self.managed.record.take() else { return permit.reply(CommitAdmission::StaleAuthority); };
        // Only the exact command prepared by the application can enter this resource lane.
        if record.stage != ManagedStage::Prepared
            || !matches!(plan.registry.as_slice(), [RegistryPlan::Managed { operation }] if *operation == record.activation.operation) {
            self.managed.record = Some(record);
            return permit.reply(CommitAdmission::StaleAuthority);
        }
        record.correlation = Some(tuple);
        let Some(activation) = activation.filter(|value| same_activation(&record.activation, value)) else {
            record.error = Some(ManagedError::Stale);
            self.managed.record = Some(record);
            return permit.reply(CommitAdmission::StaleAuthority);
        };
        let rejection = if plan.credentials.is_some() || plan.writes_durable || plan.lifecycle.is_some()
            || plan.expected_disk != activation.expected_disk || plan.registry_client_id != activation.credentials.client_id
            || !matches!(plan.registry.as_slice(), [RegistryPlan::Managed { operation }] if *operation == activation.operation) {
            Some(ManagedError::Invalid)
        } else if self.managed_remaining(record.deadline).is_zero()
            || self.managed_remaining(record.context_deadline).is_zero() {
            Some(ManagedError::Expired)
        } else if record.cancelled.load(Ordering::Acquire) || !record.proof.current() || !self.managed_disk_current(&record)
            || !source_bound(activation, &record.source, &record.proof) {
            Some(ManagedError::Stale)
        } else { None };
        if let Some(error) = rejection {
            record.error = Some(error);
            self.managed.record = Some(record);
            return permit.reply(CommitAdmission::StaleAuthority);
        }
        let origin = record.source.origin().expect("bound origin");
        let existing = plex::id_of_machine(&record.source.machine_id);
        if existing.is_some_and(|id| !plex::client_for(id).is_some_and(|client|
            client.origin() == &origin && client.uses_token(&record.source.token))) {
            record.error = Some(ManagedError::AuthorizationConflict);
            self.managed.record = Some(record);
            return permit.reply(CommitAdmission::StaleAuthority);
        }
        if record.cancelled.load(Ordering::Acquire) {
            record.error = Some(ManagedError::Stale);
            self.managed.record = Some(record);
            return permit.reply(CommitAdmission::StaleAuthority);
        }
        if self.managed_remaining(record.deadline).is_zero()
            || self.managed_remaining(record.context_deadline).is_zero() {
            record.error = Some(ManagedError::Expired);
            self.managed.record = Some(record);
            return permit.reply(CommitAdmission::StaleAuthority);
        }
        // Even a refused native registry call can have begun effects (e.g. grant/pin
        // installation). Preserve uncertainty instead of claiming prior-state rollback.
        record.stage = ManagedStage::EffectsStarted;
        record.registry = if existing.is_some() {
            plex::borrow_verified_import(&record.proof, &record.source.token).and_then(|id| {
                let lifecycle = plx_session::auth::ClientLifecycle::capture(plex::client_for(id)?);
                Some(RegistryOwnership::Borrowed { id, expected: lifecycle.logical(id.raw()), lifecycle })
            })
        } else {
            plex::register_imported(&record.proof, &record.source.token, &activation.credentials.client_id,
                plex::ConnectionFacts::new(record.source.tier, plex::IpVersion::of_host(&record.source.address)), None)
                .map(RegistryOwnership::Owned)
        };
        let admitted = !record.cancelled.load(Ordering::Acquire) && record.proof.current()
            && record.registry.as_ref().is_some_and(|registry| registry.current(&record.source)
                && plex::set_current(registry.id()));
        if admitted {
            record.stage = ManagedStage::RegistryApplied;
        } else { record.error = Some(ManagedError::Unavailable); }
        self.managed.record = Some(record);
        permit.reply(if admitted { CommitAdmission::RegistryOnly } else { CommitAdmission::StaleAuthority })
    }
    pub(crate) fn managed_published(&mut self, operation: u64, req: u32, epoch: u64, arrival: u64,
        credentials: &CredentialPatch, disk: &plx_session::auth::owner::Identity, scope: ProfileScope) {
        let Some(record) = self.managed.record.as_ref() else { return };
        if record.settled || record.activation.operation != operation || record.correlation != Some((req, epoch, arrival))
            || record.stage != ManagedStage::RegistryApplied || record.activation.expected_disk != *disk
            || self.managed_remaining(record.deadline).is_zero()
            || !record.activation.credentials.same_credentials(credentials)
            || self.managed.publication != Some((epoch, scope)) || record.cancelled.load(Ordering::Acquire) || !record.proof.current()
            || !self.managed_disk_current(record)
            || !record.registry.as_ref().is_some_and(|registry| registry.current(&record.source)
                && plex::current_server() == registry.id()) { return; }
        let published = match &self.resources {
            Resources::Live { publisher } => {
                let actual = publisher.snapshot();
                actual.generation == scope.0 && actual.stamp == plex::grant::account_key(&credentials.account_token)
                    && same_user(actual.user.as_ref(), &credentials.user)
            }
            #[cfg(test)] Resources::Fixture(resources) => resources.profile.as_ref().is_some_and(|actual|
                actual.epoch == epoch && actual.scope == scope
                    && actual.stamp == plex::grant::account_key(&credentials.account_token)
                    && same_user(actual.profile.as_ref(), &credentials.user)),
        };
        if !published || self.managed_remaining(record.deadline).is_zero() { return; }
        let record = self.managed.record.as_mut().unwrap();
        record.stage = ManagedStage::Published;
        record.settled = true;
        self.managed.completion = Some(ManagedCompletion { operation, req, epoch, arrival, accepted: true, error: None });
    }
    pub(crate) fn managed_refused(&mut self, operation: u64, req: u32, epoch: u64, arrival: u64) {
        let Some(record) = self.managed.record.as_mut() else { return };
        if record.settled || record.activation.operation != operation || record.correlation != Some((req, epoch, arrival)) { return; }
        record.settled = true;
        if record.stage == ManagedStage::Prepared { record.stage = ManagedStage::Refused; }
        self.managed.completion = Some(ManagedCompletion { operation, req, epoch, arrival,
            accepted: false, error: record.error.or(Some(ManagedError::Stale)) });
    }
}
fn same_user(actual: Option<&plex::session::UserRef>, expected: &plex::session::UserRef) -> bool {
    if !actual.is_some_and(|user| user.account_scope == expected.account_scope) { return false; }
    match (actual.map(serde_json::to_value), serde_json::to_value(expected)) {
        (Some(Ok(actual)), Ok(expected)) => actual == expected, _ => false,
    }
}

impl SessionAdapter {
    /// Refresh only an already admitted exact resource. This grants no new row,
    /// creates no selection context and publishes no credentials or profile.
    pub(crate) fn refresh_managed(&mut self, operation: u64, proof: ImportProof) -> bool {
        let Some(mut record) = self.managed.record.take() else { return false };
        let valid = record.activation.operation == operation && record.stage == ManagedStage::Published
            && !record.cancelled.load(Ordering::Acquire) && self.managed_disk_current(&record)
            && !matches!(record.proof.freshness(), plex::grant::ImportFreshness::Revoked)
            && proof.current() && record.source.origin().is_some_and(|origin|
                proof.matches_source(&origin, &record.source.machine_id));
        if !valid { self.managed.record = Some(record); return false; }
        let replacement = match record.registry.as_ref() {
            Some(RegistryOwnership::Owned(lease)) => {
                plex::register_imported(&proof, &record.source.token, &record.activation.credentials.client_id,
                    plex::ConnectionFacts::new(record.source.tier, plex::IpVersion::of_host(&record.source.address)), Some(lease))
                    .map(RegistryOwnership::Owned)
            }
            Some(RegistryOwnership::Borrowed { id: expected, lifecycle, expected: logical })
                if lifecycle.is_current(*logical) => {
                plex::borrow_verified_import(&proof, &record.source.token).filter(|id| id == expected).and_then(|id| {
                    let lifecycle = plx_session::auth::ClientLifecycle::capture(plex::client_for(id)?);
                    Some(RegistryOwnership::Borrowed { id, expected: lifecycle.logical(id.raw()), lifecycle })
                })
            }
            _ => None,
        };
        let refreshed = !record.cancelled.load(Ordering::Acquire) && proof.current()
            && replacement.as_ref().is_some_and(|registry| registry.current(&record.source));
        if refreshed {
            record.registry = replacement;
            record.proof = proof;
        }
        self.managed.record = Some(record);
        refreshed
    }
}

#[cfg(test)]
#[path = "session_managed_tests.rs"]
mod tests;

impl SessionAdapter {
    pub(crate) fn managed_read(&self, operation: u64) -> Option<ManagedRead> {
        let record = self.managed.record.as_ref()?;
        if !self.managed_active_current(operation, &record.activation.credentials, &record.activation.expected_disk) { return None; }
        let registry = record.registry.as_ref()?.clone();
        let client = plex::client_for(registry.id())?;
        let lifecycle = plx_session::auth::ClientLifecycle::capture(client);
        let expected = lifecycle.logical(registry.id().raw());
        let read = client.capture_bounded_read(expected.token_gen, &record.source.token)?;
        let captured = ManagedRead { registry, source: record.source.clone(), proof: record.proof.clone(),
            cancelled: record.cancelled.clone(), lifecycle, expected, read };
        captured.current().then_some(captured)
    }
    pub(crate) fn managed_active_current(&self, operation: u64, credentials: &CredentialPatch,
        disk: &plx_session::auth::owner::Identity) -> bool {
        self.managed.record.as_ref().is_some_and(|record|
            record.activation.operation == operation && record.stage == ManagedStage::Published
                && record.activation.credentials.same_credentials(credentials)
                && record.activation.expected_disk == *disk && self.managed_disk_current(record)
                && !record.cancelled.load(Ordering::Acquire) && record.proof.current()
                && record.registry.as_ref().is_some_and(|registry| registry.current(&record.source)))
    }
    pub(crate) fn resume_managed(&mut self, operation: u64) -> bool {
        let Some(record) = self.managed.record.as_ref() else { return false };
        if !self.managed_active_current(operation, &record.activation.credentials, &record.activation.expected_disk) { return false; }
        let Some(registry) = &record.registry else { return false };
        plex::set_current(registry.id()) && plex::current_server() == registry.id()
            && registry.current(&record.source) && record.proof.current()
            && !record.cancelled.load(Ordering::Acquire)
    }
}

/// One retained resource snapshot. Cloning this does not acquire or retire a
/// registry slot. The main thread still checks Session ownership at publication.
#[derive(Clone)]
pub(crate) struct ManagedRead {
    registry: RegistryOwnership,
    source: SourceRef,
    proof: ImportProof,
    cancelled: Arc<AtomicBool>,
    lifecycle: plx_session::auth::ClientLifecycle,
    expected: plx_session::auth::owner::ServerLifecycle,
    read: plex::BoundedRead,
}
impl ManagedRead {
    pub(crate) fn current(&self) -> bool {
        !self.cancelled.load(Ordering::Acquire) && self.proof.current()
            && self.lifecycle.is_current(self.expected) && self.registry.current(&self.source)
            && self.read.current()
    }
    pub(crate) fn matches(&self, machine: &str) -> bool { self.source.machine_id == machine }
    pub(crate) fn sections(&self) -> Option<plex::MediaContainer> {
        if !self.current() { return None; }
        let result = self.read.sections();
        self.current().then_some(result).flatten()
    }
    pub(crate) fn section_items(&self, query: &plex::SectionQuery) -> Option<plex::MediaContainer> {
        if !self.current() { return None; }
        let result = self.read.section_items(query);
        self.current().then_some(result).flatten()
    }
    pub(crate) fn metadata(&self, rating_key: &str) -> Option<plex::Metadata> {
        if !self.current() { return None; }
        let result = self.read.metadata(rating_key);
        self.current().then_some(result).flatten()
    }
}
