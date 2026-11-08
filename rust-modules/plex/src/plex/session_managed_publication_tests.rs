//! Pure in-memory authority/publication regressions; no fixture file or account service.
use super::*;
struct Restore {
    cache: Cached,
    profile: Option<std::sync::Arc<CurrentProfile>>,
}
impl Drop for Restore {
    fn drop(&mut self) {
        *CACHE.lock().unwrap() = self.cache.clone();
        *CURRENT.lock().unwrap() = self.profile.clone();
    }
}
#[test]
fn pms_only_publication_cannot_borrow_blank_id_saved_account_authority() {
    let _serial = plx_base::testlock::serial();
    let _restore = Restore { cache: CACHE.lock().unwrap().clone(), profile: CURRENT.lock().unwrap().clone() };
    let stored = Session { account_token: "saved-account".into(), ..Default::default() };
    *CACHE.lock().unwrap() = Cached::Settled(std::sync::Arc::new(ReadState::Ready {
        session: std::sync::Arc::new(stored.clone()), plaintext: false, retry_canonical: false }));
    assert_eq!(plex_tv_credential(&stored.user).as_deref(), Some("saved-account"));
    let mt = unsafe { plx_base::task::MainThread::assume() };
    let mut publisher = ProfilePublisher::new(&mt);
    publisher.publish_pms_only(Some(UserRef { token: "imported-pms".into(), ..Default::default() }), 9, "nonempty".into());
    let captured = current().unwrap();
    assert_eq!(captured.account_scope, AccountScope::PmsOnly);
    assert_eq!(plex_tv_credential(&captured), None);
    assert_eq!(current_seat(), Seat::Unseated);
    assert!(!is_seated_as(""));
    let active = Session { user: captured.clone(), ..stored.clone() };
    assert!(!active.active_profile_is_admin());
    let mut explicit = captured.clone(); explicit.plex_tv_token = Some("must-not-use".into());
    assert_eq!(plex_tv_credential(&explicit), None);
    publisher.publish(Some(stored.user.clone()), 10, "genuine-account".into());
    assert_eq!(plex_tv_credential(&captured), None, "a retained snapshot keeps its denial after a later publication");
    assert_eq!(plex_tv_credential(&stored.user).as_deref(), Some("saved-account"));
    assert_eq!(peek().account_token, "saved-account", "publication leaves the physical cache unchanged");
}
#[test]
fn pms_only_scope_is_clone_stable_but_not_a_persisted_schema_field() {
    let user = UserRef { account_scope: AccountScope::PmsOnly, ..Default::default() };
    assert_eq!(user.clone().account_scope, AccountScope::PmsOnly);
    let encoded = serde_json::to_value(&user).unwrap();
    assert!(encoded.get("account_scope").is_none());
    let decoded: UserRef = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.account_scope, AccountScope::Account);
}
