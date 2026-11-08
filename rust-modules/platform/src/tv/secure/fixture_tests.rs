use super::*;
use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};
use std::sync::Arc;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let mut entropy = [0u8; 16];
        File::open("/dev/urandom")
            .unwrap()
            .read_exact(&mut entropy)
            .unwrap();
        let name: String = entropy.iter().map(|n| format!("{n:02x}")).collect();
        let path = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("ott-host-fixture-{name}"));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        Self(path)
    }
    fn prepare(&self, create: bool) -> Result<Context, &'static str> {
        Context::prepare(
            self.0.clone(),
            private_directory(&self.0)?,
            "test.install",
            create,
        )
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn parser_requires_complete_opt_in_and_only_fixture_loopback() {
    let args = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    for invalid in [
        vec![],
        vec!["--fixture-state", "/tmp/x"],
        vec!["--fixture-reuse", "/tmp/x", "--fixture-state"],
        vec![
            "--fixture-state",
            "/tmp/x",
            "--fixture-create",
            "10.0.0.1",
            "1234",
        ],
        vec![
            "--fixture-state",
            "/tmp/x",
            "--fixture-create",
            "127.0.0.1",
            "0",
        ],
        vec![
            "--fixture-state",
            "/tmp/x",
            "--fixture-create",
            "--fixture-reuse",
        ],
    ] {
        assert!(parse_args(&args(&invalid)).is_err());
    }
    let valid = args(&[
        "--fixture-state",
        "/tmp/x",
        "--fixture-reuse",
        "127.0.0.1",
        "1234",
    ]);
    let (_, create, ca, local, plex, rest) = parse_args(&valid).unwrap();
    assert!(plex.is_none());
    assert!(ca.is_none());
    assert!(local.is_none());
    assert!(!create);
    assert_eq!(rest, ["127.0.0.1", "1234"]);
    assert!(activate_from_args(&args(&["--hud-schema"]))
        .unwrap()
        .is_none());
}

#[test]
fn fixture_roundtrip_is_authenticated_but_explicitly_not_confidential() {
    let root = Scratch::new();
    let fixture = root.prepare(true).unwrap();
    let plain = br#"{"synthetic":true,"profile":"test only"}"#;
    let sealed = fixture.seal(Namespace::OttState, plain).unwrap();
    assert_eq!(sealed.backend, Backend::HostFixture);
    assert_eq!(b64::decode(&sealed.data).unwrap(), plain);
    assert_eq!(fixture.open(Namespace::OttState, &sealed).unwrap(), plain);
    assert!(fixture.open(Namespace::OttControl, &sealed).is_none());
    let wire = serde_json::to_value(&sealed).unwrap();
    assert_eq!(wire["backend"], "host_fixture");
    assert!(fixture
        .open(
            Namespace::OttState,
            &Sealed {
                backend: Backend::Keymanager3,
                ..sealed.clone()
            }
        )
        .is_none());
    for mutate in [0, 1, 2] {
        let mut bad = sealed.clone();
        match mutate {
            0 => bad.data = b64::encode(b"different payload"),
            1 => bad.iv = b64::encode(&[0u8; 32]),
            _ => bad.key.push('x'),
        }
        assert!(fixture.open(Namespace::OttState, &bad).is_none());
    }
    // The adapter authenticates bytes, not permissive JSON or normalized values.
    let opaque = fixture.seal(Namespace::OttState, b"not JSON\xff").unwrap();
    assert_eq!(
        fixture.open(Namespace::OttState, &opaque).unwrap(),
        b"not JSON\xff"
    );
}

#[test]
fn restart_reuses_key_without_regeneration_and_excludes_parallel_owner() {
    let root = Scratch::new();
    let first = root.prepare(true).unwrap();
    let sealed = first.seal(Namespace::OttState, b"before restart").unwrap();
    assert!(root.prepare(false).is_err());
    drop(first);
    let next = root.prepare(false).unwrap();
    assert_eq!(
        next.open(Namespace::OttState, &sealed).unwrap(),
        b"before restart"
    );
    assert!(root.prepare(true).is_err());
    drop(next);
    std::fs::remove_file(root.0.join("key")).unwrap();
    assert!(root.prepare(false).is_err());
    assert!(!root.0.join("key").exists());
}

#[test]
fn worker_receives_same_immutable_authority() {
    let root = Scratch::new();
    let fixture = Arc::new(root.prepare(true).unwrap());
    let worker = Arc::clone(&fixture);
    let sealed = std::thread::spawn(move || worker.seal(Namespace::OttControl, b"worker").unwrap())
        .join()
        .unwrap();
    assert_eq!(
        fixture.open(Namespace::OttControl, &sealed).unwrap(),
        b"worker"
    );
}

#[test]
fn root_and_install_are_bound_even_when_private_files_are_copied() {
    let root = Scratch::new();
    let fixture = root.prepare(true).unwrap();
    let sealed = fixture.seal(Namespace::OttState, b"bound").unwrap();
    drop(fixture);
    let other = Scratch::new();
    for name in ["lock", "key", "marker"] {
        std::fs::copy(root.0.join(name), other.0.join(name)).unwrap();
    }
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(other.0.join("records"))
        .unwrap();
    assert!(other.prepare(false).is_err());
    assert!(Context::prepare(
        root.0.clone(),
        private_directory(&root.0).unwrap(),
        "different.install",
        false
    )
    .is_err());
    let fresh = Scratch::new();
    assert!(fresh
        .prepare(true)
        .unwrap()
        .open(Namespace::OttState, &sealed)
        .is_none());
}

#[test]
fn unsafe_roots_symlinks_hardlinks_and_nonfixture_contents_are_rejected() {
    let root = Scratch::new();
    std::fs::write(root.0.join("auth.json"), b"do not import").unwrap();
    assert!(root.prepare(true).is_err());
    assert!(root.prepare(false).is_err());
    let target = Scratch::new();
    let link = root.0.join("alias");
    symlink(&target.0, &link).unwrap();
    assert!(private_directory(&link).is_err());
    std::fs::set_permissions(&target.0, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(target.prepare(true).is_err());
    let safe = Scratch::new();
    let fixture = safe.prepare(true).unwrap();
    let sealed = fixture.seal(Namespace::OttState, b"private").unwrap();
    std::fs::hard_link(safe.0.join("key"), safe.0.join("extra")).unwrap();
    assert!(fixture.open(Namespace::OttState, &sealed).is_none());
    drop(fixture);
    std::fs::remove_file(safe.0.join("extra")).unwrap();
    let external = root.0.join("external-key");
    std::fs::rename(safe.0.join("key"), &external).unwrap();
    symlink(external, safe.0.join("key")).unwrap();
    assert!(safe.prepare(false).is_err());
}

#[test]
fn tampered_key_marker_or_root_cannot_be_used_after_activation() {
    for change in ["key", "marker", "lock", "root", "records"] {
        let root = Scratch::new();
        let fixture = root.prepare(true).unwrap();
        let sealed = fixture.seal(Namespace::OttState, b"original").unwrap();
        if change == "root" {
            std::fs::set_permissions(&root.0, std::fs::Permissions::from_mode(0o755)).unwrap();
        } else if change == "records" {
            std::fs::remove_dir(root.0.join("records")).unwrap();
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(root.0.join("records"))
                .unwrap();
        } else if change == "lock" {
            std::fs::remove_file(root.0.join("lock")).unwrap();
            let _ = open_file_at(&fixture.directory, "lock", true).unwrap();
        } else {
            std::fs::write(root.0.join(change), b"tampered").unwrap();
        }
        assert!(fixture.open(Namespace::OttState, &sealed).is_none());
        assert!(fixture.seal(Namespace::OttState, b"new").is_none());
    }
}

#[test]
fn payload_bound_and_base64_canonicality_fail_closed() {
    let root = Scratch::new();
    let fixture = root.prepare(true).unwrap();
    assert!(fixture
        .seal(Namespace::OttState, &vec![0; PAYLOAD_BYTES + 1])
        .is_none());
    let mut sealed = fixture.seal(Namespace::OttState, b"x").unwrap();
    // The shared decoder stops at '='; the adapter rejects trailing garbage.
    sealed.data.push_str("AAAA");
    assert!(fixture.open(Namespace::OttState, &sealed).is_none());
}

fn environment(runtime: &Path, assets: &Path) -> Vec<(OsString, OsString)> {
    vec![
        ("PLXNATIVE_RUNTIME_DIR".into(), runtime.as_os_str().into()),
        ("PLXNATIVE_APP_DIR".into(), assets.as_os_str().into()),
        ("PLXNATIVE_WIN".into(), "1920x1080".into()),
        (
            "PLXNATIVE_SHOT".into(),
            runtime.join("capture.png").into_os_string(),
        ),
    ]
}

#[test]
fn isolated_runtime_and_allowlisted_assets_admit_no_inherited_profiles() {
    let runtime = Scratch::new();
    let assets = Scratch::new();
    let env = environment(&runtime.0, &assets.0);
    let directory = private_directory(&runtime.0).unwrap();
    let _ = open_file_at(&directory, "plxnative-focus", true).unwrap();
    std::fs::write(assets.0.join("appfont.ttf"), b"public fixture bytes").unwrap();
    std::fs::set_permissions(
        assets.0.join("appfont.ttf"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(validate_environment_values(&env).is_ok());
    for name in [
        "OTTPLAY_PLEX_PROFILE",
        "PLXNATIVE_PMS_HOST",
        "PLXNATIVE_INITIAL",
        "PLXNATIVE_SIM_FFMPEG",
        "OTTP_STATE",
        "PLX_TOKEN",
    ] {
        let mut dirty = env.clone();
        dirty.push((name.into(), "ignored-even-if-empty".into()));
        assert!(validate_environment_values(&dirty).is_err());
    }
    std::fs::write(runtime.0.join("auth.json"), b"not a fixture runtime").unwrap();
    assert!(validate_environment_values(&env).is_err());
    std::fs::remove_file(runtime.0.join("auth.json")).unwrap();
    std::fs::write(assets.0.join("ottplay-profile.json"), b"not a public asset").unwrap();
    assert!(validate_environment_values(&env).is_err());
    std::fs::remove_file(assets.0.join("ottplay-profile.json")).unwrap();
    symlink(runtime.0.join("plxnative-focus"), assets.0.join("icon.png")).unwrap();
    assert!(validate_environment_values(&env).is_err());
}

#[test]
fn environment_rejects_external_capture_and_malformed_window() {
    let runtime = Scratch::new();
    let assets = Scratch::new();
    for (name, value) in [
        ("PLXNATIVE_SHOT", "/tmp/external.png"),
        ("PLXNATIVE_WIN", "999999x2"),
        ("PLXNATIVE_WIN", "secret"),
        ("PLXNATIVE_WIN", "0x2"),
    ] {
        let mut env = environment(&runtime.0, &assets.0);
        env.retain(|(key, _)| key != name);
        env.push((name.into(), value.into()));
        assert!(validate_environment_values(&env).is_err());
    }
    assert!(overlap(&runtime.0, &runtime.0.join("nested")));
    assert!(overlap(&runtime.0.join("nested"), &runtime.0));
    assert!(!overlap(&runtime.0, &assets.0));
}

#[test]
fn actual_commit_after_key_invalidation_is_classified_unknown() {
    use crate::storage::{CommitReceipt, CommitStage, JsonStore, Record, RecordKey, RecordStore};
    let root = Scratch::new();
    let fixture = root.prepare(true).unwrap();
    let bytes = br#"{"version":1,"install":"test.install","namespace":"ott-state","value":{"synthetic":true}}"#;
    let sealed = fixture.seal(Namespace::OttState, bytes).unwrap();
    let payload = serde_json::json!({"version":1,"sealed":sealed}).to_string();
    let record = Record::data(1, payload);
    let store = JsonStore::new(root.0.join("records")).unwrap();
    // A deterministic ordering, not a simulated fsync race: the seal succeeded,
    // authority then changed, and the normal store really writes durable bytes.
    std::fs::write(root.0.join("key"), [0u8; 32]).unwrap();
    let raw = store.commit(RecordKey::OttState, &record).unwrap();
    assert_eq!(raw, CommitReceipt::Durable);
    assert_eq!(store.load(RecordKey::OttState).unwrap(), Some(record));
    assert!(fixture.check().is_err());
    assert_eq!(
        crate::storage::private::fixture_commit_receipt(raw, fixture.check().is_ok()),
        CommitReceipt::Uncertain {
            stage: CommitStage::Readback,
            errno: libc::ESTALE
        }
    );
}

#[test]
fn empty_private_clock_marker_is_admitted_with_focus_before_host_boot() {
    let runtime = Scratch::new();
    let assets = Scratch::new();
    let env = environment(&runtime.0, &assets.0);
    let directory = private_directory(&runtime.0).unwrap();
    drop(open_file_at(&directory, "plxnative-focus", true).unwrap());
    drop(open_file_at(&directory, "plxnative-clocksink", true).unwrap());
    assert!(validate_environment_values(&env).is_ok());
    // The explicit allowlist is not permission for other dev triggers.
    for name in [
        "plxnative-simvideo-extra",
        "plxnative-noidle",
        "plxnative-token",
        "plxnative-clocksink-extra",
    ] {
        drop(open_file_at(&directory, name, true).unwrap());
        assert!(validate_environment_values(&env).is_err(), "{name}");
        std::fs::remove_file(runtime.0.join(name)).unwrap();
    }
}

#[test]
fn clock_marker_rejects_content_links_directories_and_public_permissions() {
    let runtime = Scratch::new();
    let assets = Scratch::new();
    let env = environment(&runtime.0, &assets.0);
    let directory = private_directory(&runtime.0).unwrap();
    let marker = runtime.0.join("plxnative-clocksink");
    drop(open_file_at(&directory, "plxnative-focus", true).unwrap());
    drop(open_file_at(&directory, "plxnative-clocksink", true).unwrap());
    std::fs::write(&marker, b"1").unwrap();
    assert!(
        validate_environment_values(&env).is_err(),
        "nonempty marker"
    );
    std::fs::write(&marker, b"").unwrap();
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(validate_environment_values(&env).is_err(), "public marker");
    std::fs::remove_file(&marker).unwrap();
    symlink(runtime.0.join("plxnative-focus"), &marker).unwrap();
    assert!(validate_environment_values(&env).is_err(), "symlink marker");
    std::fs::remove_file(&marker).unwrap();
    std::fs::hard_link(runtime.0.join("plxnative-focus"), &marker).unwrap();
    assert!(
        validate_environment_values(&env).is_err(),
        "hard-linked marker"
    );
    std::fs::remove_file(&marker).unwrap();
    std::fs::create_dir(&marker).unwrap();
    assert!(
        validate_environment_values(&env).is_err(),
        "directory marker"
    );
}

// The adapter checks PEM framing. The real TLS regression in plx_net uses minted
// certificates; it, not a second certificate parser here, grades trust semantics.
const CA_FRAME: &[u8] = b"-----BEGIN CERTIFICATE-----\nAQ==\n-----END CERTIFICATE-----\n";
fn ca_input(root: &Scratch, bytes: &[u8]) -> PathBuf {
    let path = root.0.join("input.pem");
    std::fs::write(&path, bytes).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    path
}

#[test]
fn ca_parser_requires_explicit_paired_flags_and_exact_loopback_origin() {
    let args = |tail: &[&str]| ["--fixture-state", "/tmp/state", "--fixture-create"]
        .into_iter().chain(tail.iter().copied()).map(str::to_owned).collect::<Vec<_>>();
    let valid = args(&["--fixture-ca", "/tmp/input.pem", "--fixture-origin",
        "https://127.0.0.1:9443", "127.0.0.1", "32400"]);
    let (_, _, grant, _, _, rest) = parse_args(&valid).unwrap();
    assert_eq!(grant, Some((PathBuf::from("/tmp/input.pem"), "https://127.0.0.1:9443")));
    assert_eq!(rest, ["127.0.0.1", "32400"]);
    for tail in [
        vec!["--fixture-ca"], vec!["--fixture-ca", "/tmp/input.pem"],
        vec!["--fixture-origin", "https://127.0.0.1:9443"],
        vec!["--fixture-ca", "/tmp/input.pem", "--fixture-origin"],
        vec!["--fixture-ca", "/tmp/input.pem", "--fixture-origin", "https://127.0.0.1:9443", "--fixture-ca", "again"],
    ] { assert!(parse_args(&args(&tail)).is_err()); }
    for origin in [
        "http://127.0.0.1:9443", "https://localhost:9443", "https://[::1]:9443",
        "https://127.0.0.2:9443", "https://example.test:9443", "https://127.0.0.1",
        "https://127.0.0.1:0", "https://127.0.0.1:65536", "https://127.0.0.1:09443",
        "https://127.0.0.1:+9443", "https://127.0.0.1:9443/", "https://127.0.0.1:9443?q=x",
        "https://127.0.0.1:9443#x", "https://user@127.0.0.1:9443", "https://127.0.0.1:9443@elsewhere",
    ] {
        assert!(parse_args(&args(&["--fixture-ca", "/tmp/input.pem", "--fixture-origin", origin])).is_err(), "{origin}");
    }
}

#[test]
fn ca_snapshot_is_origin_scoped_read_only_and_does_not_track_source_mutations() {
    let root = Scratch::new();
    let input = Scratch::new();
    let source = ca_input(&input, CA_FRAME);
    let mut fixture = root.prepare(true).unwrap();
    assert_eq!(fixture.ca_bundle("https://127.0.0.1:9443/pair"), Ok(None));
    fixture.install_ca(&source, "https://127.0.0.1:9443").unwrap();
    let snapshot = fixture.ca_bundle("https://127.0.0.1:9443/pair").unwrap().unwrap();
    assert_eq!(std::fs::read(snapshot).unwrap(), CA_FRAME);
    assert_eq!(std::fs::metadata(snapshot).unwrap().mode() & 0o777, 0o400);
    std::fs::write(&source, b"replaced caller input").unwrap();
    assert_eq!(std::fs::read(fixture.ca_bundle("https://127.0.0.1:9443/poll?q=1").unwrap().unwrap()).unwrap(), CA_FRAME);
    for url in ["http://127.0.0.1:9443/pair", "https://127.0.0.1:94430/pair",
        "https://127.0.0.1:9444/pair", "https://localhost:9443/pair",
        "https://127.0.0.1:9443@elsewhere/pair", "https://127.0.0.1:9443\\@elsewhere/pair",
        "https://127.0.0.1:9443/pair\n", "https://example.test/pair"] {
        assert_eq!(fixture.ca_bundle(url), Ok(None), "{url}");
    }
    assert!(fixture.install_ca(&source, "https://127.0.0.1:9443").is_err());
    drop(fixture);
    assert!(!root.0.join(ca::NAME).exists(), "Context drop owns snapshot cleanup");
    assert_eq!(root.prepare(false).unwrap().ca_bundle("https://127.0.0.1:9443/pair"), Ok(None));
}

#[test]
fn ca_tampering_fails_closed_and_unrelated_origin_keeps_normal_trust() {
    let root = Scratch::new(); let input = Scratch::new();
    let mut fixture = root.prepare(true).unwrap();
    fixture.install_ca(&ca_input(&input, CA_FRAME), "https://127.0.0.1:9443").unwrap();
    let snapshot = root.0.join(ca::NAME);
    std::fs::set_permissions(&snapshot, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(fixture.ca_bundle("https://127.0.0.1:9443/pair").is_err());
    std::fs::write(&snapshot, b"different").unwrap();
    std::fs::set_permissions(&snapshot, std::fs::Permissions::from_mode(0o400)).unwrap();
    assert!(fixture.ca_bundle("https://127.0.0.1:9443/pair").is_err(), "same inode, changed bytes");
    assert_eq!(fixture.ca_bundle("https://example.test/pair"), Ok(None));
    std::fs::remove_file(&snapshot).unwrap();
    std::fs::write(&snapshot, CA_FRAME).unwrap();
    std::fs::set_permissions(&snapshot, std::fs::Permissions::from_mode(0o400)).unwrap();
    assert!(fixture.ca_bundle("https://127.0.0.1:9443/pair").is_err(), "same bytes, substituted inode");
}

#[test]
fn ca_input_rejects_links_unsafe_permissions_oversize_and_non_pem() {
    let input = Scratch::new();
    let source = ca_input(&input, CA_FRAME);
    let link = input.0.join("symlink.pem"); symlink(&source, &link).unwrap();
    let root = Scratch::new(); let mut fixture = root.prepare(true).unwrap();
    assert!(fixture.install_ca(&link, "https://127.0.0.1:9443").is_err());
    let hard = input.0.join("hard.pem"); std::fs::hard_link(&source, &hard).unwrap();
    assert!(fixture.install_ca(&source, "https://127.0.0.1:9443").is_err());
    std::fs::remove_file(&hard).unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(fixture.install_ca(&source, "https://127.0.0.1:9443").is_err());
    for bytes in [vec![], b"not a PEM".to_vec(), b"-----BEGIN PRIVATE KEY-----\nsecret\n-----END PRIVATE KEY-----".to_vec(), vec![b'x'; 256 * 1024 + 1]] {
        let path = ca_input(&input, &bytes);
        assert!(fixture.install_ca(&path, "https://127.0.0.1:9443").is_err());
        assert!(!root.0.join(ca::NAME).exists());
    }
}

#[test]
fn ca_residual_is_removed_under_lock_without_restoring_authority() {
    let root = Scratch::new();
    drop(root.prepare(true).unwrap());
    // Model _exit leaving a finished public copy; do not leak a test Context/lock.
    let path = root.0.join(ca::NAME);
    std::fs::write(&path, CA_FRAME).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
    let fixture = root.prepare(false).unwrap();
    assert!(!path.exists());
    assert_eq!(fixture.ca_bundle("https://127.0.0.1:9443/pair"), Ok(None));
    assert!(root.prepare(false).is_err(), "existing owner retains exclusive lock");
}

#[test]
fn simvideo_marker_requires_the_validated_clock_sink_marker() {
    let runtime = Scratch::new();
    let assets = Scratch::new();
    let env = environment(&runtime.0, &assets.0);
    let directory = private_directory(&runtime.0).unwrap();
    drop(open_file_at(&directory, "plxnative-focus", true).unwrap());
    drop(open_file_at(&directory, "plxnative-simvideo", true).unwrap());
    assert!(matches!(
        validate_environment_values(&env),
        Err("simvideo requires clocksink")
    ));
    drop(open_file_at(&directory, "plxnative-clocksink", true).unwrap());
    assert!(validate_environment_values(&env).is_ok());
    std::fs::remove_file(runtime.0.join("plxnative-clocksink")).unwrap();
    assert!(matches!(
        validate_environment_values(&env),
        Err("simvideo requires clocksink")
    ));
}

#[test]
fn simvideo_marker_rejects_content_links_directories_and_public_permissions() {
    let runtime = Scratch::new();
    let assets = Scratch::new();
    let env = environment(&runtime.0, &assets.0);
    let directory = private_directory(&runtime.0).unwrap();
    let marker = runtime.0.join("plxnative-simvideo");
    drop(open_file_at(&directory, "plxnative-focus", true).unwrap());
    drop(open_file_at(&directory, "plxnative-clocksink", true).unwrap());
    drop(open_file_at(&directory, "plxnative-simvideo", true).unwrap());
    assert!(validate_environment_values(&env).is_ok());
    std::fs::write(&marker, b"1").unwrap();
    assert!(
        validate_environment_values(&env).is_err(),
        "nonempty marker"
    );
    std::fs::write(&marker, b"").unwrap();
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(validate_environment_values(&env).is_err(), "public marker");
    std::fs::remove_file(&marker).unwrap();
    symlink(runtime.0.join("plxnative-focus"), &marker).unwrap();
    assert!(validate_environment_values(&env).is_err(), "symlink marker");
    std::fs::remove_file(&marker).unwrap();
    std::fs::hard_link(runtime.0.join("plxnative-focus"), &marker).unwrap();
    assert!(
        validate_environment_values(&env).is_err(),
        "hard-linked marker"
    );
    std::fs::remove_file(&marker).unwrap();
    std::fs::create_dir(&marker).unwrap();
    assert!(
        validate_environment_values(&env).is_err(),
        "directory marker"
    );
}

#[path = "fixture_plex_tests.rs"]
mod plex_tests;

#[cfg(target_os = "linux")]
#[test]
fn linux_renderer_library_keeps_the_existing_asset_authority_checks() {
    let runtime = Scratch::new();
    let assets = Scratch::new();
    let env = environment(&runtime.0, &assets.0);
    let library = assets.0.join("libass-plx-host.so.0");
    std::fs::write(&library, b"synthetic library asset; not loaded").unwrap();
    std::fs::set_permissions(&library, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(validate_environment_values(&env).is_ok());

    std::fs::set_permissions(&library, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(validate_environment_values(&env).is_err());
    std::fs::remove_file(&library).unwrap();
    symlink(runtime.0.join("missing-library"), &library).unwrap();
    assert!(validate_environment_values(&env).is_err());
    std::fs::remove_file(&library).unwrap();

    std::fs::write(assets.0.join("unlisted.so"), b"not an allowed asset").unwrap();
    assert!(validate_environment_values(&env).is_err());
}
