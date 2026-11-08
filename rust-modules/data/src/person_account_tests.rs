// Included inside person::tests. No network service or compiler was run in the
// authoring lane; the existing host transport counters are the effect boundary.

/// Fixture-only host input: production captures this in the app adapter.
fn current_input() -> PersonInput {
    if crate::stores::tape::active() { return PersonInput::controlled(); }
    let profile = plx_plex::plex::session::current_snapshot();
    let session = plx_plex::plex::session::peek_settled();
    let credential = session.as_ref().and_then(|_|
        profile.user.as_ref().and_then(plx_plex::plex::session::plex_tv_credential));
    PersonInput::live(profile, session.as_ref().map(|s| s.client_id.as_str()), credential)
}

fn published_input(generation: u32, pms_only: bool) -> PersonInput {
    use plx_plex::plex::session::{AccountScope, CurrentProfile, UserRef};
    PersonInput::live(Arc::new(CurrentProfile {
        user: Some(UserRef { id: 7, uuid: "person-user".into(), token: "pms-secret".into(),
            account_scope: if pms_only { AccountScope::PmsOnly } else { AccountScope::Account },
            ..Default::default() }),
        generation, stamp: "account-stamp-secret".into(),
    }), Some("person-client"), Some("account-credential-secret".into()))
}

#[test]
fn pms_only_person_input_emits_no_account_job_and_settles_both_reads() {
    let _serial = plx_base::testlock::serial();
    let _tape = TapeOff;
    crate::stores::tape::reset_for_test();
    plx_plex::plex::reset_servers_for_test();
    let input = published_input(2, true);
    assert!(input.refused());
    assert!(input.authority.as_ref().unwrap().credential.is_none());
    let mut owner = Owner::default();
    owner.open(S0, "7", "5d77682aeb5d26001f1de4b0", "Actor", "");
    for _ in 0..3 { owner.step_input(&input); assert!(owner.jobs().is_empty()); }
    let person = owner.current().unwrap();
    assert!(person.profiled && person.profile_tried && person.credited);
    assert!(person.bio.is_empty() && person.credits.is_empty());
}

#[test]
fn queued_person_account_jobs_revalidate_publication_before_transport() {
    use std::sync::atomic::Ordering::SeqCst;
    let _serial = plx_base::testlock::serial();
    transport_asks::PROFILE.store(0, SeqCst);
    transport_asks::CREDITS.store(0, SeqCst);
    let captured = published_input(1, false);
    let replacements = [published_input(2, true), published_input(2, false)];
    let adapter = Arc::new(PersonAdapter::default());
    for replacement in replacements {
      for slot in [F_PROFILE, F_CREDITS] {
        let identity = captured.identity().unwrap();
        let work = if slot == F_PROFILE { PersonWork::Profile(identity) } else { PersonWork::Credits(identity) };
        let request = req(slot, 1, &["5d77682aeb5d26001f1de4b0"], "5d77682aeb5d26001f1de4b0", work);
        adapter.mailbox(slot).claim(1);
        finish_person_job(&PersonPort::to(&adapter, slot, 1), person_addr(slot as u32), None, request, &replacement);
        assert!(!adapter.mailbox(slot).busy());
        assert!(!adapter.mailbox(slot).has_mail());
      }
    }
    assert_eq!(transport_asks::PROFILE.load(SeqCst), 0);
    assert_eq!(transport_asks::CREDITS.load(SeqCst), 0);
}

#[test]
fn person_profile_change_discards_late_results_without_a_disk_identity_change() {
    let _serial = plx_base::testlock::serial();
    let _tape = TapeOff;
    crate::stores::tape::reset_for_test();
    plx_plex::plex::reset_servers_for_test();
    let before = published_input(1, false);
    let after = published_input(2, true);
    let mut owner = Owner::default();
    owner.open(S0, "7", "5d77682aeb5d26001f1de4b0", "Actor", "");
    owner.step_input(&before);
    let old_generation = owner.gen();
    let _old_jobs = owner.jobs();
    owner.apply(F_PROFILE, Landing::Profile(Some(profile("OLD ACCOUNT BIO", "", ""))));
    owner.land(F_PROFILE, old_generation, Landing::Profile(Some(profile("LATE ACCOUNT BIO", "", ""))));
    owner.step_input(&after);
    assert!(owner.gen() > old_generation);
    assert!(owner.current().unwrap().bio.is_empty());
    assert!(owner.jobs().is_empty());
    owner.land(F_PROFILE, old_generation, Landing::Profile(Some(profile("STILL LATE", "", ""))));
    owner.step_input(&after);
    assert!(owner.current().unwrap().bio.is_empty());
    assert!(owner.jobs().is_empty());
}

#[test]
fn absent_publication_is_stable_anonymous_and_never_borrows_a_credential() {
    use plx_plex::plex::session::CurrentProfile;
    let absent = || PersonInput::live(Arc::new(CurrentProfile {
        user: None, generation: 0, stamp: String::new(),
    }), Some("person-client"), Some("must-not-borrow".into()));
    let first = absent();
    let second = absent();
    assert_eq!(first, second, "separate absent snapshot allocations are the same identity");
    let Some(Identity::Settled(authority)) = first.identity() else { panic!("anonymous read remains available") };
    assert!(authority.credential.is_none());
    assert!(!first.refused());
    let debug = format!("{:?} {:?}", published_input(1, false), Identity::Settled(authority));
    for private in ["person-client", "pms-secret", "account-stamp-secret", "account-credential-secret", "must-not-borrow"] {
        assert!(!debug.contains(private));
    }
}

#[test]
fn unchanged_person_account_and_anonymous_jobs_can_reach_the_existing_transport_seam() {
    use plx_plex::plex::session::CurrentProfile;
    use std::sync::atomic::Ordering::SeqCst;
    let _serial = plx_base::testlock::serial();
    transport_asks::PROFILE.store(0, SeqCst);
    let account = published_input(1, false);
    let anonymous = PersonInput::live(Arc::new(CurrentProfile {
        user: None, generation: 0, stamp: String::new(),
    }), Some("person-client"), None);
    let adapter = Arc::new(PersonAdapter::default());
    for (index, input) in [account, anonymous].into_iter().enumerate() {
        let generation = index as u32 + 1;
        let request = req(F_PROFILE, generation, &["5d77682aeb5d26001f1de4b0"], "5d77682aeb5d26001f1de4b0",
            PersonWork::Profile(input.identity().unwrap()));
        adapter.mailbox(F_PROFILE).claim(generation);
        finish_person_job(&PersonPort::to(&adapter, F_PROFILE, generation), person_addr(generation), None, request, &input);
        assert!(adapter.mailbox(F_PROFILE).has_mail());
        adapter.clear_all();
    }
    assert_eq!(transport_asks::PROFILE.load(SeqCst), 2);
}
