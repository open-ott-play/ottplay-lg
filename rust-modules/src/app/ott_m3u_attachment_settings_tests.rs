//! Settings contract and candidate seams, not an end-to-end RPC dispatch test.
//! No App, storage writer, worker, transport or native playback is constructed.
use super::*;
use crate::app::ott_m3u_attachment::{Change, Registry};

const LINK: &str = "portal::[key:fixture-private-key]https://portal.fixture.invalid/api/";
const OTHER_LINK: &str = "portal::[key:other-private-key]https://other.fixture.invalid/api/";

fn fixture() -> Runtime {
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = Remote::disabled_for_test("dev_fixture", "fixture");
    runtime.write_permit.revoke();
    runtime.loaded = true;
    runtime.entropy = Some([7; 16]);
    runtime.saved.profiles.profiles.push(ProviderProfile {
        id: "m3u-parent".into(),
        name: "Home".into(),
        kind: ProviderKind::M3u,
        source: "https://tv.fixture.invalid/private.m3u".into(),
        source_alias: "Home".into(),
        enabled: true,
    });
    runtime.saved.profiles.active_profile = Some("m3u-parent".into());
    runtime
        .saved
        .settings
        .insert("slot:m3u-parent".into(), json!(7));
    runtime.observe_m3u_attachments();
    runtime
}

fn context(runtime: &Runtime, navigation: u64) -> Value {
    let mut context = setting_context(
        &runtime.saved,
        ProviderKind::M3u,
        "runtime:fixture",
        &[7; 16],
    );
    media_scope(
        &mut context,
        runtime.m3u_attachments.context_revision(),
        navigation,
    );
    context
}

fn attach(runtime: &mut Runtime, link: &str) {
    runtime.saved = runtime
        .attachment_candidate(
            &runtime.saved.profiles.profiles[0],
            link,
            Some("attachment-1"),
        )
        .unwrap();
    runtime.observe_m3u_attachments();
}

#[test]
fn media_setting_descriptor_snapshot_and_context_do_not_expose_private_material() {
    let mut runtime = fixture();
    let registry = Registry::default()
        .apply(
            &runtime.saved.profiles,
            &runtime.saved.profiles.profiles[0],
            Change::Set {
                link: LINK,
                legacy_source_id: Some("private-legacy-id"),
            },
            Some("attachment-1"),
        )
        .unwrap();
    registry
        .put_into(&mut runtime.saved.settings, &runtime.saved.profiles)
        .unwrap();
    runtime.observe_m3u_attachments();
    let descriptor = Setting::M3uMedia.descriptor();
    assert_eq!(descriptor["sensitive"], true);
    assert_eq!(descriptor["access"]["read"], false);
    assert_eq!(descriptor["access"]["write"], true);
    let stored = stored(&runtime.saved, Setting::M3uMedia);
    assert_eq!(stored["redacted"], true);
    assert_eq!(stored["configured"], true);
    assert!(stored.get("value").is_none());
    assert!(stored.get("persisted_value").is_none());
    let context = context(&runtime, 1);
    assert_eq!(context["profile"], 7);
    let public = json!([
        descriptor,
        stored,
        context,
        signature(&runtime.saved, Setting::M3uMedia)
    ])
    .to_string();
    for secret in [
        "fixture-private-key",
        "portal.fixture.invalid",
        "private-legacy-id",
        "tv.fixture.invalid",
    ] {
        assert!(!public.contains(secret));
    }
}

#[test]
fn media_setting_accepts_shared_portal_syntax_and_rejects_invalid_or_over_limit_values() {
    let setting = Setting::find("m3uMediaUrl").unwrap();
    assert!(setting.writable());
    for value in [
        json!(""),
        json!("   "),
        json!(LINK),
        json!("portal::%5Bkey:opaque%2Fkey%5Dhttps://portal.fixture.invalid/api/?q=%25"),
    ] {
        assert!(setting.valid(&value));
    }
    for value in [
        Value::Null,
        json!(12),
        json!(true),
        json!("https://portal.fixture.invalid/api/"),
        json!("portal::[key:]https://portal.fixture.invalid/"),
        json!("portal::[key:private key]https://portal.fixture.invalid/"),
        json!("portal::[key:private\nkey]https://portal.fixture.invalid/"),
        json!("portal::[key:fixture]https://user:password@portal.fixture.invalid/"),
        json!("portal::[key:fixture]https://portal.fixture.invalid/#fragment"),
        json!(format!(
            "portal::[key:{}]https://portal.fixture.invalid/",
            "a".repeat(1025)
        )),
        json!(format!(
            "portal::[key:fixture]https://portal.fixture.invalid/{}",
            "a".repeat(4096)
        )),
    ] {
        assert!(!setting.valid(&value));
    }
}

#[test]
fn source_and_navigation_contexts_reject_old_settings_receipts() {
    let mut runtime = fixture();
    attach(&mut runtime, LINK);
    let first = context(&runtime, 4);
    let first_signature = signature(&runtime.saved, Setting::M3uMedia);
    let mut receipts = Receipts::default();
    let navigation_receipt = receipts.record(
        "media:navigation",
        first_signature.clone(),
        first["token"].as_str().unwrap().into(),
        0,
        10,
    );
    let moved = context(&runtime, 5);
    assert_ne!(first["token"], moved["token"]);
    assert!(!receipts.consume(
        &navigation_receipt,
        &first_signature,
        moved["token"].as_str().unwrap(),
        0,
        11
    ));
    let source_receipt = receipts.record(
        "media:source",
        first_signature.clone(),
        first["token"].as_str().unwrap().into(),
        0,
        10,
    );
    attach(&mut runtime, OTHER_LINK);
    assert_ne!(
        first_signature,
        signature(&runtime.saved, Setting::M3uMedia)
    );
    attach(&mut runtime, LINK);
    let restored = context(&runtime, 4);
    assert_eq!(
        first_signature,
        signature(&runtime.saved, Setting::M3uMedia),
        "same bytes require the monotonic context fence"
    );
    assert_ne!(first["token"], restored["token"]);
    assert!(!receipts.consume(
        &source_receipt,
        &signature(&runtime.saved, Setting::M3uMedia),
        restored["token"].as_str().unwrap(),
        0,
        11
    ));
}

#[test]
fn projected_candidate_context_matches_commit_without_early_activation() {
    let mut runtime = fixture();
    attach(&mut runtime, LINK);
    let old = runtime.m3u_attachments.capture("m3u-parent").unwrap();
    let old_context = context(&runtime, 8);
    let candidate = runtime
        .attachment_candidate(&runtime.saved.profiles.profiles[0], OTHER_LINK, None)
        .unwrap();
    let registry = Registry::read(&candidate.settings, &candidate.profiles).unwrap();
    let mut projected = runtime.m3u_attachments.clone();
    projected.observe(&registry, &candidate.profiles).unwrap();
    let mut predicted = setting_context(&candidate, ProviderKind::M3u, "runtime:fixture", &[7; 16]);
    media_scope(&mut predicted, projected.context_revision(), 8);
    assert!(runtime.m3u_attachments.is_current(&old));
    assert_eq!(context(&runtime, 8), old_context);
    assert_ne!(predicted["token"], old_context["token"]);
    assert!(!runtime.saving);
    assert!(runtime.resolver_next.is_none());
    assert!(runtime.catalog_next.is_none());
    runtime.saved = candidate;
    runtime.observe_m3u_attachments();
    assert_eq!(context(&runtime, 8), predicted);
    assert!(!runtime.m3u_attachments.is_current(&old));
}

#[test]
fn media_edit_preserves_existing_legacy_identity_and_explicit_clear_removes_it() {
    let mut runtime = fixture();
    let registry = Registry::default()
        .apply(
            &runtime.saved.profiles,
            &runtime.saved.profiles.profiles[0],
            Change::Set {
                link: LINK,
                legacy_source_id: Some("keep-private-legacy"),
            },
            Some("attachment-1"),
        )
        .unwrap();
    registry
        .put_into(&mut runtime.saved.settings, &runtime.saved.profiles)
        .unwrap();
    runtime.observe_m3u_attachments();
    let changed = runtime
        .attachment_candidate(&runtime.saved.profiles.profiles[0], OTHER_LINK, None)
        .unwrap();
    let next = Registry::read(&changed.settings, &changed.profiles).unwrap();
    let attachment = next.for_parent("m3u-parent").unwrap();
    assert_eq!(attachment.id(), "attachment-1");
    assert_eq!(attachment.legacy_source_id(), Some("keep-private-legacy"));
    let cleared = runtime
        .attachment_candidate(&runtime.saved.profiles.profiles[0], "", None)
        .unwrap();
    assert_eq!(stored(&cleared, Setting::M3uMedia)["configured"], false);
    assert_eq!(
        stored(&runtime.saved, Setting::M3uMedia)["configured"],
        true
    );
}

#[test]
fn media_attachment_changes_leave_ordinary_settings_and_their_context_unchanged() {
    let mut runtime = fixture();
    runtime
        .saved
        .settings
        .insert("uiFontScale".into(), json!(120));
    let normal_context = setting_context(
        &runtime.saved,
        ProviderKind::M3u,
        "runtime:fixture",
        &[7; 16],
    );
    let font = stored(&runtime.saved, Setting::FontScale);
    let font_signature = signature(&runtime.saved, Setting::FontScale);
    attach(&mut runtime, LINK);
    attach(&mut runtime, OTHER_LINK);
    assert_eq!(stored(&runtime.saved, Setting::FontScale), font);
    assert_eq!(
        signature(&runtime.saved, Setting::FontScale),
        font_signature
    );
    assert_eq!(
        setting_context(
            &runtime.saved,
            ProviderKind::M3u,
            "runtime:fixture",
            &[7; 16]
        ),
        normal_context
    );
    assert_eq!(Setting::FontScale.descriptor()["access"]["read"], true);
    assert_eq!(
        Setting::Player.descriptor()["choices"][0]["label"],
        "Native"
    );
    assert_eq!(
        Setting::Interface.descriptor()["choices"][1]["label"],
        "Modern"
    );
}

#[test]
fn remote_profile_a_b_a_does_not_restore_a_media_settings_context() {
    let mut runtime = fixture();
    attach(&mut runtime, LINK);
    let mut second = runtime.saved.profiles.profiles[0].clone();
    second.id = "second-m3u".into();
    second.name = "Second".into();
    runtime.saved.profiles.profiles.push(second);
    runtime.observe_m3u_attachments();
    let first = context(&runtime, 0);
    let receipt = runtime.m3u_attachments.capture("m3u-parent").unwrap();
    runtime.saved.profiles.active_profile = Some("second-m3u".into());
    runtime.observe_m3u_attachments();
    runtime.saved.profiles.active_profile = Some("m3u-parent".into());
    runtime.observe_m3u_attachments();
    assert_ne!(context(&runtime, 0)["token"], first["token"]);
    assert!(runtime.m3u_attachments.is_current(&receipt));
}

#[test]
fn media_storage_distinguishes_absent_explicit_empty_and_malformed_registry() {
    let runtime = fixture();
    let empty = stored(&runtime.saved, Setting::M3uMedia);
    assert_eq!(empty["storage"], "default");
    assert_eq!(empty["configured"], false);
    let mut cleared = runtime.attachment_candidate(&runtime.saved.profiles.profiles[0], "", None).unwrap();
    assert!(cleared.settings.contains_key(crate::app::ott_m3u_attachment::SETTINGS_KEY));
    let explicit = stored(&cleared, Setting::M3uMedia);
    assert_eq!(explicit["storage"], "stored");
    assert_eq!(explicit["configured"], false);
    assert_eq!(explicit["redacted"], true);
    assert!(explicit.get("value").is_none());
    assert!(explicit.get("persisted_value").is_none());
    cleared.settings.insert(crate::app::ott_m3u_attachment::SETTINGS_KEY.into(), json!({"version":99,"entries":[]}));
    let invalid = stored(&cleared, Setting::M3uMedia);
    assert_eq!(invalid["storage"], "invalid");
    assert_eq!(invalid["configured"], false);
}
