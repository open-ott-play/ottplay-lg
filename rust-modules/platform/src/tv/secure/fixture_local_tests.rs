//! Source-only definitions for the explicit HostFixture adapter. No device or
//! playback claims: these tests use private temporary directories and real FDs.
use super::*;
use std::io::Read;
use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};

struct Scratch {
    root: PathBuf,
    declaration: PathBuf,
    volume: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let mut entropy = [0u8; 16];
        File::open("/dev/urandom")
            .unwrap()
            .read_exact(&mut entropy)
            .unwrap();
        let suffix: String = entropy.iter().map(|n| format!("{n:02x}")).collect();
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("ott-local-observation-{suffix}"));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        let volume = root.join("volume");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&volume)
            .unwrap();
        let result = Self {
            declaration: root.join("declaration.json"),
            root,
            volume,
        };
        result.write_value(result.value());
        result
    }

    fn value(&self) -> Value {
        json!({"version":1,"observe_control":true,
            "volumes":[{"path":self.volume,"uuid":"fixture-volume-A"}]})
    }

    fn write_value(&self, value: Value) {
        self.write(&serde_json::to_vec(&value).unwrap());
    }

    fn write(&self, bytes: &[u8]) {
        std::fs::write(&self.declaration, bytes).unwrap();
        std::fs::set_permissions(&self.declaration, std::fs::Permissions::from_mode(0o600))
            .unwrap();
    }

    fn read(&self) -> Result<LocalFixture, &'static str> {
        LocalFixture::read(&self.declaration, &[])
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn declaration_requires_explicit_closed_bounded_schema() {
    let scratch = Scratch::new();
    assert!(scratch.read().is_ok());
    for value in [
        json!({"version":1,"observe_control":false,"volumes":[]}),
        json!({"version":2,"observe_control":true,"volumes":[]}),
        json!({"version":1,"observe_control":true,"volumes":[]}),
        json!({"version":1,"observe_control":true,"volumes":null}),
        json!({"version":1,"observe_control":true,"volumes":[],"extra":true}),
    ] {
        scratch.write_value(value);
        assert!(scratch.read().is_err());
    }
    let mut oversized = scratch.value();
    oversized["volumes"] = json!(vec![oversized["volumes"][0].clone(); VOLUME_LIMIT + 1]);
    scratch.write_value(oversized);
    assert!(scratch.read().is_err());
    scratch.write(br#"{"version":1,"version":1,"observe_control":true,"volumes":[]}"#);
    assert!(scratch.read().is_err());
    scratch.write(&vec![b' '; DECLARATION_BYTES + 1]);
    assert!(scratch.read().is_err());
}

#[test]
fn declaration_and_roots_are_private_canonical_unaliased_and_disjoint() {
    let scratch = Scratch::new();
    let link = scratch.root.join("alias.json");
    symlink(&scratch.declaration, &link).unwrap();
    assert!(LocalFixture::read(&link, &[]).is_err());
    let hardlink = scratch.root.join("hardlink.json");
    std::fs::hard_link(&scratch.declaration, &hardlink).unwrap();
    assert!(scratch.read().is_err());
    std::fs::remove_file(hardlink).unwrap();
    std::fs::set_permissions(&scratch.declaration, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(scratch.read().is_err());
    scratch.write_value(scratch.value());
    std::fs::set_permissions(&scratch.volume, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(scratch.read().is_err());
    std::fs::set_permissions(&scratch.volume, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(LocalFixture::read(&scratch.declaration, &[&scratch.volume]).is_err());
    assert!(LocalFixture::read(&scratch.declaration, &[&scratch.root]).is_err());
    let volume_alias = scratch.root.join("volume-alias");
    symlink(&scratch.volume, &volume_alias).unwrap();
    let mut value = scratch.value();
    value["volumes"][0]["path"] = json!(volume_alias);
    scratch.write_value(value);
    assert!(scratch.read().is_err());
}

#[test]
fn observation_uses_actual_held_directory_device_and_exact_identity_snapshot() {
    let scratch = Scratch::new();
    let fixture = scratch.read().unwrap();
    let off = OffFrame::for_test();
    let mounts = fixture.mounts(&off, &mut || true).unwrap();
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].mount_point(), scratch.volume.to_str().unwrap());
    assert_eq!(mounts[0].root(), "/");
    assert_eq!(
        mounts[0].device(),
        directory_device(&off, &File::open(&scratch.volume).unwrap()).unwrap()
    );
    let rows = fixture.identities(&off, &mounts, &mut || true).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].mount() == &mounts[0]);
    assert_eq!(rows[0].identity().uuid(), "fixture-volume-A");
    assert_eq!(rows[0].identity().filesystem(), "ext4");
    assert_eq!(
        fixture.identities(&off, &[], &mut || true).err(),
        Some(IdentityError::Changed)
    );
}

#[test]
fn detach_and_same_inode_return_are_distinct_from_same_path_replacement() {
    let scratch = Scratch::new();
    let fixture = scratch.read().unwrap();
    let off = OffFrame::for_test();
    let before = fixture.mounts(&off, &mut || true).unwrap();
    let parked = scratch.root.join("parked");
    std::fs::rename(&scratch.volume, &parked).unwrap();
    assert!(fixture.mounts(&off, &mut || true).unwrap().is_empty());
    assert_eq!(
        fixture.identities(&off, &before, &mut || true).err(),
        Some(IdentityError::Changed)
    );
    std::fs::rename(&parked, &scratch.volume).unwrap();
    assert!(fixture.mounts(&off, &mut || true).unwrap() == before);
    assert!(fixture.identities(&off, &before, &mut || true).is_ok());
    std::fs::rename(&scratch.volume, &parked).unwrap();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&scratch.volume)
        .unwrap();
    assert_eq!(
        fixture.mounts(&off, &mut || true).err(),
        Some(ReadError::Unavailable)
    );
    assert_eq!(
        fixture.identities(&off, &before, &mut || true).err(),
        Some(IdentityError::Unavailable)
    );
}

#[test]
fn cancellation_at_final_observation_boundary_discards_the_whole_result() {
    let scratch = Scratch::new();
    let fixture = scratch.read().unwrap();
    let off = OffFrame::for_test();
    assert_eq!(
        fixture.mounts(&off, &mut || false).err(),
        Some(ReadError::Cancelled)
    );
    let mut calls = 0;
    assert_eq!(
        fixture
            .mounts(&off, &mut || {
                calls += 1;
                calls < 2
            })
            .err(),
        Some(ReadError::Cancelled)
    );
    assert_eq!(calls, 2);
    let mounts = fixture.mounts(&off, &mut || true).unwrap();
    let mut calls = 0;
    assert_eq!(
        fixture
            .identities(&off, &mounts, &mut || {
                calls += 1;
                calls < 3
            })
            .err(),
        Some(IdentityError::Cancelled)
    );
    assert_eq!(calls, 3);
}

#[test]
fn received_evidence_exposes_known_fields_but_not_paths_handles_or_unknown_names() {
    let bytes = serde_json::to_vec(&json!({"server_time":123,"requests":[{
        "id":"0123456789abcdef0123456789abcdef","action":"local_media","expires_at":130,
        "params":{"runtime":"fixture-runtime","op":"play","source_handle":"PRIVATE_HANDLE",
            "path":"/PRIVATE/PATH","PRIVATE_SECRET_KEY":"PRIVATE_VALUE","local_media_handles":true}
    }]}))
    .unwrap();
    let events = received(&bytes).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["id"], "0123456789abcdef0123456789abcdef");
    assert_eq!(events[0]["action"], "local_media");
    assert_eq!(events[0]["op"], "play");
    assert_eq!(events[0]["runtime_sha256"], digest(b"fixture-runtime"));
    assert_eq!(events[0]["unknown_params_count"], 1);
    assert_eq!(events[0]["source_handle_sha256"], digest(b"PRIVATE_HANDLE"));
    assert_eq!(events[0]["extensions"]["local_media_handles"], true);
    assert_eq!(events[0]["envelope_sha256"], digest(&bytes));
    assert!(!serde_json::to_string(&events).unwrap().contains("PRIVATE"));
}

#[test]
fn forwarded_capability_flags_and_unknown_fields_cannot_look_stripped() {
    let bytes = serde_json::to_vec(&json!({"requests":[{
        "id":"0123456789abcdef0123456789abcdef","action":"capabilities",
        "params":{"local_media_session":true,"local_media_progressive":true,
            "PRIVATE_UNKNOWN_FLAG":true}
    }]}))
    .unwrap();
    let events = received(&bytes).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["extensions"]["local_media_session"], true);
    assert_eq!(events[0]["extensions"]["local_media_progressive"], true);
    let keys = events[0]["params_keys"].as_array().unwrap();
    assert!(keys.contains(&json!("local_media_session")));
    assert!(keys.contains(&json!("local_media_progressive")));
    assert_eq!(events[0]["unknown_params_count"], 1);
    assert!(!events[0].to_string().contains("PRIVATE_UNKNOWN_FLAG"));
}

#[test]
fn response_evidence_uses_closed_values_and_never_copies_arbitrary_error_text() {
    let body = json!({"id":"0123456789abcdef0123456789abcdef","status":"rejected",
        "data":{"version":1,"runtime":"fixture-runtime","op":"select","state":"error",
            "error":"PRIVATE_ERROR_TEXT","PRIVATE_SECRET_KEY":"PRIVATE_VALUE"}})
    .to_string();
    let event = sent(&body, Some(202)).unwrap();
    assert_eq!(event["id"], "0123456789abcdef0123456789abcdef");
    assert_eq!(event["status"], "rejected");
    assert_eq!(event["state"], "error");
    assert_eq!(event["op"], "select");
    assert_eq!(event["http_status"], 202);
    assert_eq!(event["body_sha256"], digest(body.as_bytes()));
    assert_eq!(event["runtime_sha256"], digest(b"fixture-runtime"));
    assert_eq!(event["unknown_data_count"], 1);
    assert!(event["error"].is_null());
    assert!(!event.to_string().contains("PRIVATE"));
}

#[test]
fn observer_rejects_malformed_oversized_and_excessive_poll_records() {
    assert!(received(b"{").is_err());
    assert!(received(&vec![b' '; WIRE_BYTES + 1]).is_err());
    assert!(received(br#"{"requests":true}"#).is_err());
    assert!(received(
        &serde_json::to_vec(&json!({"requests":vec![Value::Null; REQUEST_LIMIT+1]})).unwrap()
    )
    .is_err());
    assert!(sent("{", None).is_err());
    assert!(sent(&" ".repeat(WIRE_BYTES + 1), None).is_err());
}

#[test]
fn only_finite_explicit_lifecycle_tokens_are_accepted() {
    let scratch = Scratch::new();
    let fixture = scratch.read().unwrap();
    for token in [
        "background",
        "fixture:exit",
        "fixture:foreground ",
        "fixture:PRIVATE",
    ] {
        assert_eq!(fixture.lifecycle(token), None);
    }
    for index in 0..LIFECYCLE_LIMIT {
        let (token, event) = if index % 2 == 0 {
            ("fixture:background", 0x104)
        } else {
            ("fixture:foreground", 0x106)
        };
        assert_eq!(fixture.lifecycle(token), Some(event));
    }
    assert_eq!(fixture.lifecycle("fixture:background"), None);
    assert_eq!(fixture.lifecycle("fixture:foreground"), None);
}
