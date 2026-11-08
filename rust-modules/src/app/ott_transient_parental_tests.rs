//! Source-only authority tests: real temporary roots and the existing in-memory
//! Plex registry, with no worker, network request or decoder execution.
use super::*;
use crate::app::ott_runtime::tests::local_integration as fixture;
use plx_base::task::OffFrame;

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        plx_plex::plex::reset_servers_for_test();
        plx_plex::plex::grant::reset_for_test();
    }
}
fn capture(runtime: &Runtime) -> SourceAuthority {
    parental_source_authority(runtime)
        .unwrap()
        .expect("source authority")
}
fn accept_local(runtime: &mut Runtime, root: &BoundSource, owner: u64) {
    let request = local::LocalRequest::parse(&serde_json::json!({
        "op":"play", "runtime":runtime.remote.runtime(), "source":root.id,
        "session":true, "progressive":true
    }))
    .unwrap();
    let owner = local::OwnerToken(owner);
    let mut pending = local::PendingLocal::new(request, owner, u64::MAX);
    let items = [local::LocalItem {
        id: root.single_file_name().unwrap_or("first").into(),
        playable: true,
    }];
    let scan = local::CatalogSnapshot {
        source_id: &root.id,
        authority_ref: root.authority_ref,
        source_generation: root.generation,
        state: local::ScanState::Complete,
        items: &items,
    };
    let context = local::AdmissionContext {
        runtime: runtime.remote.runtime(),
        owner,
        invocation_current: true,
        now_ms: runtime.now_ms(),
        mutations_allowed: true,
    };
    let local::Admission::Propose(context) = pending.evaluate(&context, root, &scan, 1) else {
        panic!("local queue proposal")
    };
    let read = runtime
        .capture_local_source(&runtime.bind_local_selection(root).unwrap())
        .unwrap();
    runtime.transient.accepted = Some(TransientMedia::Local(LocalSession {
        read,
        context,
        profile: source_profile(root).unwrap(),
        borrowed: None, kiosk_resume: None,
    }));
}

#[test]
fn local_new_queue_and_unrelated_inventory_refresh_keep_the_same_source() {
    let _serial = plx_base::testlock::serial();
    let (_guard, mut app, root) = fixture::setup();
    let runtime = app.ott.as_mut().unwrap();
    let first = capture(runtime);
    runtime
        .local_inventory
        .install(runtime.local_inventory.sources.clone(), 8);
    // The old receipt is obsolete, but the same live source remains authorized.
    assert!(first.same_authority(&capture(runtime)));
    accept_local(runtime, &root, 99);
    assert!(first.same_authority(&capture(runtime)));
    assert_eq!(runtime.transient.accepted.as_ref().unwrap().owner().0, 99);
}

#[test]
fn local_reopened_same_path_retires_old_authority_and_does_not_break_reflexivity() {
    let _serial = plx_base::testlock::serial();
    let (_guard, mut app, root) = fixture::setup();
    let runtime = app.ott.as_mut().unwrap();
    let first = capture(runtime);
    let replacement =
        LocalSource::acquire_authorized(&OffFrame::for_test(), root.clone(), &mut || true).unwrap();
    runtime.local_inventory.install(vec![replacement], 8);
    assert!(
        parental_source_authority(runtime).is_err(),
        "old accepted handle cannot silently rebind"
    );
    assert!(
        first.same_authority(&first.clone()),
        "identity comparison remains pure after withdrawal"
    );
    accept_local(runtime, &root, 8);
    let second = capture(runtime);
    assert!(!first.same_authority(&second));
    let third =
        LocalSource::acquire_authorized(&OffFrame::for_test(), root.clone(), &mut || true).unwrap();
    runtime.local_inventory.install(vec![third], 9);
    accept_local(runtime, &root, 9);
    assert!(
        !first.same_authority(&capture(runtime)),
        "returning to the same path is not the original grant"
    );
}

#[test]
fn durable_local_profile_reuses_only_one_existing_live_root() {
    let _serial = plx_base::testlock::serial();
    let (_guard, mut app, root) = fixture::setup();
    let runtime = app.ott.as_mut().unwrap();
    let original = capture(runtime);
    let profile = source_profile(&root).unwrap();
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.saved.profiles.profiles.push(profile);
    runtime.transient.accepted = None;
    assert!(original.same_authority(&capture(runtime)));
    let mut duplicate = root.clone();
    duplicate.id = "other-root".into();
    duplicate.authority_ref = local::AuthorityRef(2);
    let duplicate =
        LocalSource::acquire_authorized(&OffFrame::for_test(), duplicate, &mut || true).unwrap();
    let mut roots = runtime.local_inventory.sources.clone();
    roots.push(duplicate);
    runtime.local_inventory.install(roots, 8);
    assert!(parental_source_authority(runtime).is_err());
    runtime.local_inventory.install(Vec::new(), 9);
    assert!(
        parental_source_authority(runtime).is_err(),
        "path alone never grants access"
    );
}

#[test]
fn none_is_reserved_for_providers_without_extra_source_authority() {
    let _serial = plx_base::testlock::serial();
    let (_guard, mut app, _) = fixture::setup();
    let runtime = app.ott.as_mut().unwrap();
    runtime.transient.accepted = None;
    assert!(parental_source_authority(runtime).unwrap().is_none());
    runtime.saved.profiles.active_profile = None;
    runtime.provider = ProviderKind::Local;
    assert!(parental_source_authority(runtime).is_err());
    runtime.provider = ProviderKind::Plex;
    assert!(parental_source_authority(runtime).is_err());
}

fn plex_source(runtime: &mut Runtime, token: &str) -> &'static plx_plex::plex::Client {
    let origin = plx_plex::plex::Origin::parse("https://parental-source.invalid:443").unwrap();
    let sid = plx_plex::plex::register_pinned_with_client_id(
        "parental-source",
        &origin,
        token,
        None,
        "fixture",
        Default::default(),
    );
    let profile = ProviderProfile {
        id: "plex-source".into(),
        name: "Plex source".into(),
        kind: ProviderKind::Plex,
        source: "parental-source".into(),
        source_alias: "Plex source".into(),
        enabled: true,
    };
    runtime.transient.accepted = None;
    runtime.saved.profiles.profiles = vec![profile.clone()];
    runtime.saved.profiles.active_profile = Some(profile.id);
    let source = plx_plex::plex::session::SourceRef {
        machine_id: "parental-source".into(),
        name: "Plex source".into(),
        address: origin.host().into(),
        port: i64::from(origin.port()),
        token: token.into(),
        origin_url: origin.base(),
        ..Default::default()
    };
    runtime.saved.settings.insert(
        "nativePlexSources".into(),
        serde_json::to_value(vec![source]).unwrap(),
    );
    plx_plex::plex::client_for(sid).unwrap()
}

#[test]
fn separate_plex_capture_arcs_compare_by_authority_not_allocation() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_guard, mut app, _) = fixture::setup();
    let runtime = app.ott.as_mut().unwrap();
    plex_source(runtime, "token-a");
    let first = capture(runtime);
    let second = capture(runtime);
    let (Authority::Plex(a), Authority::Plex(b)) = (&first.0, &second.0) else {
        panic!("Plex")
    };
    assert!(!Arc::ptr_eq(a, b));
    assert!(first.same_authority(&second));
}

#[test]
fn plex_credential_mismatch_fails_and_a_b_a_does_not_restore_old_authority() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_guard, mut app, _) = fixture::setup();
    let runtime = app.ott.as_mut().unwrap();
    let client = plex_source(runtime, "token-a");
    let first = capture(runtime);
    client.set_token("token-b");
    assert!(
        parental_source_authority(runtime).is_err(),
        "configured token must still match"
    );
    client.set_token("token-a");
    assert!(
        !first.same_authority(&capture(runtime)),
        "existing ClientKey includes credential generation"
    );
}

#[test]
fn retired_plex_client_never_becomes_a_provider_without_authority() {
    let _serial = plx_base::testlock::serial();
    let _reset = Reset;
    let (_guard, mut app, _) = fixture::setup();
    let runtime = app.ott.as_mut().unwrap();
    let client = plex_source(runtime, "token-a");
    let first = capture(runtime);
    client.disable_data_io();
    assert!(parental_source_authority(runtime).is_err());
    assert!(first.same_authority(&first.clone()));
}

#[path = "ott_transient_file_authority_tests.rs"]
mod single_file;
