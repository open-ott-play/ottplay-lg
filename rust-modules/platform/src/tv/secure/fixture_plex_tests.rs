//! Fixture admission tests: no server, credential publication or worker is started.
use super::*;
use super::super::plex;
use plx_base::sha256::sha256;

const VALID: &[u8] = br#"{"baseUrl":"http://127.0.0.1:32401","token":"ott-fixture-plex-token","serverId":"ott-fixture-plex","name":"Fixture Plex","allowInsecureHttp":true}"#;

fn digest(bytes: &[u8]) -> String {
    sha256(bytes).iter().map(|b| format!("{b:02x}")).collect()
}
fn input(parent: &Scratch, bytes: &[u8]) -> PathBuf {
    let path = parent.0.join("plex.json");
    let directory = private_directory(&parent.0).unwrap();
    let mut file = open_file_at(&directory, "plex.json", true).unwrap();
    file.write_all(bytes).unwrap();
    path
}
fn declaration(path: &Path, bytes: &[u8]) -> plex::Declaration {
    plex::Declaration::parse(path.to_owned(), &digest(bytes), &["127.0.0.1".into(), "32401".into()]).unwrap()
}
fn snapshot(parent: &Scratch) -> (PathBuf, plex::Snapshot) {
    let path = input(parent, VALID);
    let snapshot = plex::Snapshot::prepare(declaration(&path, VALID), &[]).unwrap();
    (path, snapshot)
}

#[test]
fn plex_parser_requires_complete_hash_pair_and_canonical_explicit_loopback() {
    let head = ["--fixture-state", "/private/fixture", "--fixture-create", "--fixture-plex-profile", "/private/input/plex.json", "--fixture-plex-sha256"];
    let args = |hash: &str, tail: &[&str]| head.iter().copied().chain(std::iter::once(hash)).chain(tail.iter().copied()).map(String::from).collect::<Vec<_>>();
    let hash = digest(VALID);
    let valid = args(&hash, &["127.0.0.1", "32401"]);
    let parsed = parse_args(&valid).unwrap();
    assert!(parsed.4.is_some());
    for tail in [vec![], vec!["localhost", "32401"], vec!["127.0.0.1", "0"], vec!["127.0.0.1", "032401"], vec!["127.0.0.1", "+32401"], vec!["127.0.0.1", "65536"], vec!["127.0.0.1", "32401", "extra"]] {
        assert!(parse_args(&args(&hash, &tail)).is_err(), "{tail:?}");
    }
    for hash in [hash.to_uppercase(), "a".repeat(63), "g".repeat(64)] {
        assert!(parse_args(&args(&hash, &["127.0.0.1", "32401"])).is_err());
    }
    let mut missing = args(&digest(VALID), &["127.0.0.1", "32401"]);
    missing.remove(5);
    assert!(parse_args(&missing).is_err());
    let mut ordered = ["--fixture-state", "/private/fixture", "--fixture-create", "--fixture-ca", "/private/ca.pem", "--fixture-origin", "https://127.0.0.1:9443", "--fixture-local", "/private/local.json"].map(String::from).to_vec();
    ordered.extend(args(&digest(VALID), &["127.0.0.1", "32401"])[3..].iter().cloned());
    let parsed = parse_args(&ordered).unwrap();
    assert!(parsed.2.is_some() && parsed.3.is_some() && parsed.4.is_some());
}

#[test]
fn plex_strict_schema_refuses_every_nonfixture_authority() {
    let original: serde_json::Value = serde_json::from_slice(VALID).unwrap();
    for (field, value) in [
        ("baseUrl", serde_json::json!("http://127.0.0.1:32402")),
        ("baseUrl", serde_json::json!("http://localhost:32401")),
        ("baseUrl", serde_json::json!("http://192.168.1.1:32401")),
        ("baseUrl", serde_json::json!("https://127.0.0.1:32401")),
        ("baseUrl", serde_json::json!("http://127.0.0.1:32401/")),
        ("baseUrl", serde_json::json!("http://user@127.0.0.1:32401")),
        ("baseUrl", serde_json::json!("http://127.0.0.1:32401?token=x")),
        ("token", serde_json::json!("other-token")),
        ("serverId", serde_json::json!("other-machine")),
        ("name", serde_json::json!("Other Plex")),
        ("allowInsecureHttp", serde_json::json!(false)),
        ("allowInsecureHttp", serde_json::json!("true")),
        ("unknown", serde_json::json!(true)),
    ] {
        let parent = Scratch::new();
        let mut changed = original.clone();
        changed[field] = value;
        let bytes = serde_json::to_vec(&changed).unwrap();
        let path = input(&parent, &bytes);
        assert!(plex::Snapshot::prepare(declaration(&path, &bytes), &[]).is_err(), "{field}");
    }
    for field in ["baseUrl", "token", "serverId", "name", "allowInsecureHttp"] {
        let parent = Scratch::new();
        let mut changed = original.clone();
        changed.as_object_mut().unwrap().remove(field);
        let bytes = serde_json::to_vec(&changed).unwrap();
        let path = input(&parent, &bytes);
        assert!(plex::Snapshot::prepare(declaration(&path, &bytes), &[]).is_err());
    }
    for bytes in [b"{broken".to_vec(), [VALID, b" " , b"{}"].concat(), [b"{\"name\":\"Fixture Plex\",", &VALID[1..]].concat(), [VALID, &vec![b' '; 4096]].concat()] {
        let parent = Scratch::new();
        let path = input(&parent, &bytes);
        assert!(plex::Snapshot::prepare(declaration(&path, &bytes), &[]).is_err());
    }
}

#[test]
fn plex_snapshot_binds_actual_consumed_bytes_path_and_declared_digest() {
    let parent = Scratch::new();
    let (path, snapshot) = snapshot(&parent);
    assert_eq!(snapshot.path().unwrap(), path);
    assert!(snapshot.validate(&path, VALID).is_ok());
    assert!(snapshot.validate(&parent.0.join("other.json"), VALID).is_err());
    assert!(snapshot.validate(&path, b"{}").is_err());
    let mut wrong = declaration(&path, VALID);
    wrong.digest = [0; 32];
    assert!(plex::Snapshot::prepare(wrong, &[]).is_err());
}

#[test]
fn plex_descriptor_requires_private_disjoint_canonical_single_link_file() {
    for case in ["public-file", "public-parent", "symlink", "hardlink", "directory", "relative", "overlap", "nested-overlap"] {
        let parent = Scratch::new();
        let mut path = input(&parent, VALID);
        match case {
            "public-file" => std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap(),
            "public-parent" => std::fs::set_permissions(&parent.0, std::fs::Permissions::from_mode(0o755)).unwrap(),
            "symlink" => { let target = parent.0.join("target.json"); std::fs::rename(&path, &target).unwrap(); symlink(target, &path).unwrap(); },
            "hardlink" => std::fs::hard_link(&path, parent.0.join("alias.json")).unwrap(),
            "directory" => { std::fs::remove_file(&path).unwrap(); std::fs::create_dir(&path).unwrap(); },
            "relative" => path = PathBuf::from("plex.json"),
            _ => {}
        }
        let child = parent.0.join("state");
        let excluded: Vec<&Path> = match case { "overlap" => vec![&parent.0], "nested-overlap" => vec![&child], _ => vec![] };
        assert!(plex::Snapshot::prepare(declaration(&path, VALID), &excluded).is_err(), "{case}");
    }
}

#[test]
fn plex_live_revalidation_rejects_missing_replaced_chmod_mutated_and_linked_files() {
    for case in ["missing", "replacement", "chmod-file", "chmod-parent", "mutation", "hardlink", "symlink", "parent-replacement"] {
        let parent = Scratch::new();
        let (path, snapshot) = snapshot(&parent);
        match case {
            "missing" => std::fs::remove_file(&path).unwrap(),
            "replacement" => { std::fs::rename(&path, parent.0.join("old.json")).unwrap(); input(&parent, VALID); },
            "chmod-file" => std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap(),
            "chmod-parent" => std::fs::set_permissions(&parent.0, std::fs::Permissions::from_mode(0o755)).unwrap(),
            "mutation" => std::fs::write(&path, b"{}").unwrap(),
            "hardlink" => std::fs::hard_link(&path, parent.0.join("alias.json")).unwrap(),
            "symlink" => { let old = parent.0.join("old.json"); std::fs::rename(&path, &old).unwrap(); symlink(old, &path).unwrap(); },
            "parent-replacement" => { std::fs::remove_file(&path).unwrap(); std::fs::remove_dir(&parent.0).unwrap(); std::fs::DirBuilder::new().mode(0o700).create(&parent.0).unwrap(); input(&parent, VALID); },
            _ => unreachable!()
        }
        assert!(snapshot.path().is_err(), "{case}");
        assert!(snapshot.validate(&path, VALID).is_err(), "{case}");
    }
}

#[test]
fn plex_context_requires_explicit_declaration_and_live_fixture_authority() {
    let root = Scratch::new();
    let parent = Scratch::new();
    let path = input(&parent, VALID);
    let mut context = root.prepare(true).unwrap();
    assert!(context.plex_profile_path().unwrap().is_none());
    assert!(context.validate_plex_profile(&path, VALID).is_err());
    context.plex = Some(plex::Snapshot::prepare(declaration(&path, VALID), &[&root.0]).unwrap());
    assert_eq!(context.plex_profile_path().unwrap(), Some(path.clone()));
    assert!(context.validate_plex_profile(&path, VALID).is_ok());
    let key = root.0.join("key");
    let mut changed = std::fs::read(&key).unwrap();
    changed[0] ^= 1;
    std::fs::write(key, changed).unwrap();
    assert!(context.plex_profile_path().is_err());
    assert!(context.validate_plex_profile(&path, VALID).is_err());
}

#[test]
fn plex_declaration_does_not_relax_runtime_or_environment_denial() {
    let runtime = Scratch::new();
    let assets = Scratch::new();
    let env = environment(&runtime.0, &assets.0);
    for name in ["plxnative-token", "plxnative-plextv", "ottplay-profile.json"] {
        let directory = private_directory(&runtime.0).unwrap();
        drop(open_file_at(&directory, name, true).unwrap());
        assert!(validate_environment_values(&env).is_err());
        std::fs::remove_file(runtime.0.join(name)).unwrap();
    }
    let mut changed = env;
    changed.push(("OTTPLAY_PLEX_PROFILE".into(), "/private/input/plex.json".into()));
    assert!(validate_environment_values(&changed).is_err());
}
