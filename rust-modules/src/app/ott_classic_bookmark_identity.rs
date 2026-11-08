//! A nonsecret bookmark scope from the current native session owner. The caller
//! separately proves that `machine` belongs to its current ClassicRead binding.
use super::*;
use plx_plex::plex::session::{self, AccountScope, HomeUserRef, UserRef};
use plx_session::auth::{owner::{BootstrapAuthority, SessionInit, SessionSnapshot}, Phase};

/// Capture only at binding admission. The full private snapshot is neither
/// retained nor serialized; future writes still need the binding's live guard.
pub(super) fn capture(app: &App, machine: &str) -> Option<String> {
    if !app.bridge.session_ready_current() { return None; }
    let init = app.bridge.snapshot_session_init();
    scope(&init, app.bridge.auth_read().0, machine)
}

fn identifier(value: &str, empty: bool) -> bool {
    (empty || !value.is_empty()) && value.len() <= 256
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

fn named_user(user: &HomeUserRef) -> bool {
    user.id >= 0 && (user.id > 0 || !user.uuid.is_empty()) && identifier(&user.uuid, true)
}

/// Empty id/uuid is the legacy account-owner representation, but only after the
/// native owner actually published Some(profile) in Ready. Never infer it from
/// a missing publication. Unrelated incomplete roster rows do not disqualify it.
fn home_user<'a>(roster: &'a [HomeUserRef], current: &UserRef) -> Option<(&'a HomeUserRef, String)> {
    let mut admins = roster.iter().filter(|user| user.admin);
    let admin = admins.next()?;
    if admins.next().is_some() || !named_user(admin) { return None; }
    let owner = session::owner_identity(roster)?;
    if current.id < 0 || !identifier(&current.uuid, true) { return None; }
    let chosen = if current.id == 0 && current.uuid.is_empty() {
        admin
    } else {
        let mut matches = roster.iter().filter(|user| {
            if current.uuid.is_empty() { user.id == current.id }
            else { user.uuid == current.uuid }
        });
        let chosen = matches.next()?;
        if matches.next().is_some() || !named_user(chosen)
            || (current.id > 0 && chosen.id > 0 && current.id != chosen.id)
        { return None; }
        chosen
    };
    // A duplicate numeric identity under a different UUID is also ambiguous.
    if chosen.id > 0 && roster.iter().filter(|user| user.id == chosen.id).count() != 1 {
        return None;
    }
    Some((chosen, owner))
}

fn scope(init: &SessionInit, read: &SessionSnapshot, machine: &str) -> Option<String> {
    if !identifier(machine, false) || !matches!(&init.authority, BootstrapAuthority::Account { .. })
        || init.managed.is_some() || init.phase != Phase::Ready || read.phase != Phase::Ready
        || init.apply_pending || init.pending_erase.is_some()
        || init.persistence_warning.is_some() || read.persistence_warning.is_some()
        || init.epoch != read.flow_epoch || init.profile_scope != read.scope
        || init.persisted.account_token.is_empty()
    { return None; }
    let current = init.active_profile.as_ref()?;
    let persisted = &init.persisted.user;
    if current.account_scope != AccountScope::Account || persisted.account_scope != AccountScope::Account
        || current.id != persisted.id || current.uuid != persisted.uuid
        || read.profile.as_ref()?.uuid != current.uuid
    { return None; }
    let (home, owner) = home_user(&init.persisted.home_users, current)?;
    // Only structural public identities enter the digest. No token, endpoint,
    // client id, display label, stamp or process-local scope is an input.
    let encoded = serde_json::to_vec(&("ott-classic-bookmark-source-v1", owner,
        home.id, home.uuid.as_str(), machine)).ok()?;
    Some(plx_data::ott::favorites_wire::hash(&encoded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_session::auth::owner::{ProfileScope, SessionMachine};

    fn fixture() -> SessionInit {
        let current = UserRef { id: 12, uuid: "home-12".into(), ..Default::default() };
        let mut init = SessionInit::captured(session::Session {
            account_token: "account-secret-a".into(), user: current.clone(),
            home_users: vec![
                HomeUserRef { id: 7, uuid: "owner-7".into(), admin: true, ..Default::default() },
                HomeUserRef { id: 12, uuid: "home-12".into(), ..Default::default() },
            ], ..Default::default()
        });
        init.active_profile = Some(current);
        init.phase = Phase::Ready;
        init.epoch = 11;
        init.profile_scope = ProfileScope(9);
        init
    }
    fn key(init: &SessionInit, machine: &str) -> Option<String> {
        let machine_owner = SessionMachine::from_init(init.clone());
        scope(init, machine_owner.read().0, machine)
    }

    #[test]
    fn structural_scope_survives_new_tokens_labels_and_runtime_generations() {
        let mut init = fixture();
        let first = key(&init, "server-shared").unwrap();
        init.persisted.account_token = "account-secret-b".into();
        init.persisted.client_id = "another-install-id".into();
        init.persisted.user.token = "pms-secret-b".into();
        init.active_profile.as_mut().unwrap().token = "pms-secret-b".into();
        init.persisted.user.title = "Changed name".into();
        init.active_profile.as_mut().unwrap().title = "Changed name".into();
        init.persisted.home_users[0].title = "Renamed owner".into();
        init.persisted.server.machine_id = "different-primary-server".into();
        init.epoch += 1;
        init.profile_scope.0 += 1;
        assert_eq!(key(&init, "server-shared"), Some(first.clone()));
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(key(&init, "another-server"), Some(first.clone()));
        init.persisted.home_users[0].id = 8;
        assert_ne!(key(&init, "server-shared"), Some(first));
    }

    #[test]
    fn published_legacy_owner_resolves_to_the_roster_owner_without_guessing_no_profile() {
        let mut init = fixture();
        init.persisted.user = UserRef::default();
        init.active_profile = Some(UserRef::default());
        // A broken unrelated row must not reject a valid default account owner.
        init.persisted.home_users.push(HomeUserRef::default());
        let legacy = key(&init, "server-1").unwrap();
        init.persisted.user.id = 7;
        init.persisted.user.uuid = "owner-7".into();
        init.active_profile = Some(init.persisted.user.clone());
        assert_eq!(key(&init, "server-1"), Some(legacy));
        // A legacy roster may know only the owner's UUID, not the numeric id.
        init.persisted.home_users[0].id = 0;
        assert!(key(&init, "server-1").is_some());
        init.active_profile = None;
        assert_eq!(key(&init, "server-1"), None);
        init.active_profile = Some(init.persisted.user.clone());
        init.persisted.home_users.clear();
        assert_eq!(key(&init, "server-1"), None);
    }

    #[test]
    fn unknown_ambiguous_or_mismatched_home_identity_has_no_bookmark_scope() {
        for mode in 0..8 {
            let mut init = fixture();
            match mode {
                0 => init.persisted.home_users[0].admin = false,
                1 => init.persisted.home_users.push(init.persisted.home_users[0].clone()),
                2 => init.persisted.home_users.push(init.persisted.home_users[1].clone()),
                3 => init.persisted.home_users[1].uuid = "another-home".into(),
                4 => init.persisted.home_users[1].id = 13,
                5 => init.active_profile.as_mut().unwrap().uuid = "another-home".into(),
                6 => init.persisted.home_users[0].id = -1,
                _ => init.persisted.home_users[1].uuid = "x".repeat(257),
            }
            assert_eq!(key(&init, "server-1"), None, "case {mode}");
        }
        let mut other = fixture();
        other.persisted.home_users.push(HomeUserRef { id: 13, uuid: "home-13".into(), ..Default::default() });
        let before = key(&other, "server-1");
        other.persisted.user.id = 13;
        other.persisted.user.uuid = "home-13".into();
        other.active_profile = Some(other.persisted.user.clone());
        assert!(key(&other, "server-1").is_some());
        assert_ne!(key(&other, "server-1"), before);
    }

    #[test]
    fn nonready_dev_pms_only_or_retired_authority_cannot_capture() {
        for mode in 0..9 {
            let mut init = fixture();
            match mode {
                0 => init.phase = Phase::Profiles,
                1 => init.apply_pending = true,
                2 => init.pending_erase = Some((init.epoch, true)),
                3 => init.authority = BootstrapAuthority::DevPms { primary: Default::default(), extras: Vec::new() },
                4 => init.persisted.account_token.clear(),
                5 => init.active_profile.as_mut().unwrap().account_scope = AccountScope::PmsOnly,
                6 => init.persisted.user.account_scope = AccountScope::PmsOnly,
                7 => init.persistence_warning = Some(plx_session::auth::owner::PersistenceWarning {
                    key: plx_session::auth::owner::PersistenceWarningKey { epoch: init.epoch, req: 1 },
                    site: plx_session::auth::owner::PersistenceWarningSite::Final,
                    helper: None, candidate_errnos: [None; 8], persistence: None,
                }),
                _ => init.managed = Some(plx_session::auth::owner::ManagedProvenance {
                    operation: 1, req: 1, epoch: init.epoch, arrival: 1,
                    credentials: init.committed_credentials.clone(), disk: init.disk_identity.clone(),
                    prior_credentials: init.committed_credentials.clone(),
                }),
            }
            assert_eq!(key(&init, "server-1"), None, "case {mode}");
        }
        let init = fixture();
        for machine in ["", "https://server.invalid", "server?token=private", "server name", &"x".repeat(257)] {
            assert_eq!(key(&init, machine), None);
        }
        for mode in 0..3 {
            let mut old = init.clone();
            match mode {
                0 => old.epoch -= 1,
                1 => old.profile_scope.0 -= 1,
                _ => old.active_profile.as_mut().unwrap().uuid = "old-user".into(),
            }
            let owner = SessionMachine::from_init(old);
            assert_eq!(scope(&init, owner.read().0, "server-1"), None);
        }
    }
}
