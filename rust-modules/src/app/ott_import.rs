//! Native-only external profile boundary. Never reads credentials from launch arguments.
use super::ott_profile::Profile;
use plx_plex::plex::{Origin, session::{AccountScope, Session, ServerRef, SourceRef, UserRef}};
use std::{io::Read, path::PathBuf, time::Duration};
use plx_base::task::{OffFrame, time::Budget};

#[derive(Default)]
struct ImportAuthority {
    allowed: Vec<[u8; 32]>,
    owned: Vec<([u8; 32], plx_plex::plex::ImportedLease)>,
}
static IMPORT_AUTHORITY: std::sync::Mutex<ImportAuthority> = std::sync::Mutex::new(ImportAuthority { allowed: Vec::new(), owned: Vec::new() });
fn source_key(machine: &str, origin: &Origin, token: &str) -> [u8; 32] {
    plx_base::sha256::sha256(serde_json::json!([machine,origin.base(),token]).to_string().as_bytes())
}
/// Synchronize only after a durable load/write of nativePlexSources. Keep this
/// call before registration so late worker replies cannot revive removed media.
pub(super) fn set_authorized_sources(sources: &[SourceRef]) -> Result<(), &'static str> {
    if sources.len() > 15 { return Err("too many stored Plex sources"); }
    let mut allowed = Vec::new();
    for source in sources {
        let origin = source.origin().ok_or("invalid stored server origin")?;
        if source.machine_id.is_empty() || source.token.is_empty() { return Err("invalid stored Plex source"); }
        allowed.push(source_key(&source.machine_id, &origin, &source.token));
    }
    let mut authority = IMPORT_AUTHORITY.lock().map_err(|_| "import authority unavailable")?;
    authority.owned.retain(|(key, lease)| {
        if allowed.contains(key) { true } else { plx_plex::plex::retire_imported(lease); false }
    });
    authority.allowed = allowed;
    Ok(())
}

fn profile_path() -> Result<Option<PathBuf>, &'static str> {
    profile_path_with(|| {
        #[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
        if plx_platform::tv::secure::fixture::active() {
            return Some(plx_platform::tv::secure::fixture::plex_profile_path());
        }
        None
    }, || {
        std::env::var_os("OTTPLAY_PLEX_PROFILE").map(PathBuf::from)
            .unwrap_or_else(|| plx_base::paths::in_app_dir("ottplay-profile.json"))
    })
}

// An active fixture's absent or rejected declaration is final. In particular,
// neither case may consult an ambient profile path belonging to another install.
fn profile_path_with(
    fixture: impl FnOnce() -> Option<Result<Option<PathBuf>, &'static str>>,
    ordinary: impl FnOnce() -> PathBuf,
) -> Result<Option<PathBuf>, &'static str> {
    match fixture() {
        Some(declared) => declared,
        None => Ok(Some(ordinary())),
    }
}

fn identity(origin: &Origin, expected: &str, remaining: &impl Fn() -> Duration,
    cancelled: &impl Fn() -> bool) -> Result<String, &'static str> {
    let pin = plx_plex::plex::ResolvePin::for_manual_origin(origin);
    let timeout = remaining().saturating_sub(Duration::from_secs(4)).min(Duration::from_secs(3));
    if cancelled() || timeout.is_zero() { return Err("verification cancelled or expired"); }
    let reply = plx_session::auth::get_identity(origin, pin.as_ref(), timeout);
    match reply {
        plx_session::auth::ProbeReply::Answered { status: 200, body, .. } => {
            let observed = plx_session::auth::machine_id_in(&body).ok_or("server returned no identity")?;
            if !expected.is_empty() && observed != expected { return Err("server identity does not match the profile"); }
            Ok(observed)
        }
        _ => Err("server identity could not be verified"),
    }
}

/// An inert managed descriptor. Its proof and configuration never enter Saved.
#[derive(Clone)]
pub(super) struct ManagedCandidate {
    pub verified: VerifiedSource,
    pub credentials: plx_session::auth::owner::CredentialPatch,
    path: PathBuf,
    configuration: [u8; 32],
    pub retired: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl ManagedCandidate {
    /// Worker-only re-read: list never reads the configuration or suppression marker.
    pub fn recheck(&self) -> Result<(), &'static str> {
        if self.retired.load(std::sync::atomic::Ordering::Acquire) { return Err("managed import was retired"); }
        if !matches!(plx_platform::storage::private::imports_suppressed(), Ok(false)) {
            return Err("managed import is revoked");
        }
        let bytes = read_configuration(&self.path)?.ok_or("managed configuration was removed")?;
        if plx_base::sha256::sha256(&bytes) != self.configuration {
            return Err("managed configuration changed");
        }
        Ok(())
    }
    /// A network-only refresh verifies exactly the retained source and never
    /// acquires consent, activates a grant, or substitutes a newer configuration.
    pub fn reverify(&self, worker: &OffFrame, allowance: Duration, cancelled: &std::sync::atomic::AtomicBool) -> Result<Self, &'static str> {
        let budget = Budget::start(worker, allowance);
        self.reverify_before(|| budget.remaining(worker), cancelled)
    }
    /// The caller owns the fixed deadline. Configuration rechecks and both
    /// network probes consume that same budget; none can restart it.
    pub fn reverify_before(&self, remaining: impl Fn() -> Duration,
        cancelled: &std::sync::atomic::AtomicBool) -> Result<Self, &'static str> {
        self.reverify_before_with(remaining, cancelled, || self.recheck(),
            |profile, remaining, cancelled| verify_profile_before(profile, remaining, cancelled))
    }
    // The IO boundary is shared by production and deterministic timing tests.
    // Neither callback decides admission or restarts the caller's budget.
    fn reverify_before_with(&self, remaining: impl Fn() -> Duration,
        cancelled: &std::sync::atomic::AtomicBool,
        mut recheck: impl FnMut() -> Result<(), &'static str>,
        verify: impl FnOnce(&Profile, &dyn Fn() -> Duration, &dyn Fn() -> bool) -> Result<VerifiedSource, &'static str>,
    ) -> Result<Self, &'static str> {
        use plx_plex::plex::grant::ImportFreshness;
        let cancelled = || cancelled.load(std::sync::atomic::Ordering::Acquire)
            || self.retired.load(std::sync::atomic::Ordering::Acquire);
        if cancelled() || remaining().is_zero() { return Err("verification cancelled or expired"); }
        if self.verified.proof.freshness() == ImportFreshness::Revoked {
            return Err("managed import is revoked");
        }
        recheck()?;
        if cancelled() || remaining().is_zero() { return Err("verification cancelled or expired"); }
        let source = &self.verified.source;
        let origin = source.origin().ok_or("invalid retained origin")?;
        let profile = Profile { address: origin.base(), token: source.token.clone(),
            server_id: source.machine_id.clone(), name: source.name.clone(),
            allow_insecure_http: !origin.is_tls() };
        let verified = verify(&profile, &remaining, &cancelled)?;
        recheck()?;
        if cancelled() || remaining().is_zero() { return Err("verification cancelled or expired"); }
        if self.verified.proof.freshness() == ImportFreshness::Revoked {
            return Err("managed import was revoked during verification");
        }
        let mut refreshed = self.clone();
        refreshed.verified = verified;
        Ok(refreshed)
    }
}

/// Worker-only acquisition. Invalid/absent imports leave the saved Session intact.
pub(super) fn load(worker: &OffFrame, saved: &Session, cancelled: &std::sync::atomic::AtomicBool) -> Result<Option<ManagedCandidate>, &'static str> {
    if !matches!(plx_platform::storage::private::imports_suppressed(), Ok(false)) {
        return Ok(None);
    }
    let Some(path) = profile_path()? else { return Ok(None); };
    let result = read_and_verify(worker, &path, saved, cancelled)?;
    if let Some(candidate) = &result { candidate.recheck()?; }
    Ok(result)
}

fn read_configuration(path: &std::path::Path) -> Result<Option<Vec<u8>>, &'static str> {
    read_configuration_with(path, |_path, _bytes| {
        #[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
        if plx_platform::tv::secure::fixture::active() {
            return plx_platform::tv::secure::fixture::validate_plex_profile(_path, _bytes);
        }
        Ok(())
    })
}

// Validate the bytes actually read before either initial verification or a
// managed candidate's recheck can use them. The same boundary serves both.
fn read_configuration_with(path: &std::path::Path,
    validate: impl FnOnce(&std::path::Path, &[u8]) -> Result<(), &'static str>,
) -> Result<Option<Vec<u8>>, &'static str> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("cannot read profile file"),
    };
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err("profile must be a regular file no larger than 1 MiB");
    }
    let file = std::fs::File::open(path).map_err(|_| "cannot open profile file")?;
    let opened = file.metadata().map_err(|_| "cannot inspect profile file")?;
    if !opened.is_file() || opened.len() > 1024 * 1024 { return Err("invalid opened profile file"); }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
            return Err("profile file changed while opening");
        }
    }
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|_| "cannot read profile file")?;
    if bytes.len() > 1024 * 1024 { return Err("profile exceeds 1 MiB"); }
    validate(path, &bytes)?;
    Ok(Some(bytes))
}

fn read_and_verify(worker: &OffFrame, path: &std::path::Path, saved: &Session, cancelled: &std::sync::atomic::AtomicBool) -> Result<Option<ManagedCandidate>, &'static str> {
    let budget = Budget::start(worker, Duration::from_secs(8));
    let Some(bytes) = read_configuration(path)? else { return Ok(None) };
    let configuration = plx_base::sha256::sha256(&bytes);
    let profile = Profile::parse(&bytes)?;
    let verified = verify_profile_before(&profile, || budget.remaining(worker), || cancelled.load(std::sync::atomic::Ordering::Acquire))?;
    let source = &verified.source;
    let origin = source.origin().ok_or("invalid server origin")?;
    let imported = Session {
        client_id: saved.client_id.clone(), language: saved.language,
        server: ServerRef { machine_id: source.machine_id.clone(), name: profile.name.clone(),
            address: source.address.clone(), port: i64::from(origin.port()),
            token: profile.token.clone(), origin_url: origin.base(), tier: source.tier,
            ..Default::default() },
        user: UserRef { account_scope: AccountScope::PmsOnly, title: profile.name, token: profile.token, ..Default::default() },
        sources: vec![source.clone()], ..Default::default()
    };
    Ok(Some(ManagedCandidate { verified,
        credentials: plx_session::auth::owner::CredentialPatch::of(&imported),
        path: path.to_owned(), configuration,
        retired: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)) }))
}

/// Verified credentials remain private and the proof expires on identity/network changes.
#[derive(Clone)]
pub(super) struct VerifiedSource { pub source: SourceRef, pub(super) proof: plx_plex::plex::grant::ImportProof }

/// Replace only the external verifier result for whole-app lifecycle tests.
#[cfg(all(test, feature = "hostsim"))]
pub(super) fn managed_candidate_for_test(saved: &Session, source: SourceRef) -> ManagedCandidate {
    let origin = source.origin().expect("synthetic origin");
    let imported = Session {
        client_id: saved.client_id.clone(),
        server: ServerRef { machine_id: source.machine_id.clone(), name: source.name.clone(),
            address: source.address.clone(), port: i64::from(origin.port()), token: source.token.clone(),
            origin_url: origin.base(), tier: source.tier, ..Default::default() },
        user: UserRef { account_scope: AccountScope::PmsOnly, title: source.name.clone(), token: source.token.clone(), ..Default::default() },
        sources: vec![source.clone()], ..Default::default()
    };
    ManagedCandidate { verified: verified_source_for_test(source),
        credentials: plx_session::auth::owner::CredentialPatch::of(&imported),
        path: PathBuf::from("/nonexistent-managed-fixture.json"), configuration: [0; 32],
        retired: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)) }
}

/// Supply only a synthetic verification result; registry authority remains real.
#[cfg(all(test, feature = "hostsim"))]
pub(super) fn verified_source_for_test(source: SourceRef) -> VerifiedSource {
    let origin = source.origin().expect("synthetic source origin");
    let proof = plx_plex::plex::grant::prove_import(
        plx_plex::plex::grant::scope(), &origin, &source.machine_id,
        &source.machine_id, !origin.is_tls(),
    ).expect("synthetic source proof");
    VerifiedSource { source, proof }
}

/// Bounded network validation only. It does not modify active grants, accounts or the registry.
pub(super) fn verify_profile(worker: &OffFrame, profile: &Profile) -> Result<VerifiedSource, &'static str> {
    verify_profile_bounded(worker, profile, Duration::from_secs(8), || false)
}
fn verify_profile_bounded(worker: &OffFrame, profile: &Profile, allowance: Duration,
    cancelled: impl Fn() -> bool) -> Result<VerifiedSource, &'static str> {
    let budget = Budget::start(worker, allowance);
    verify_profile_before(profile, || budget.remaining(worker), cancelled)
}
fn verify_profile_before(profile: &Profile, remaining: impl Fn() -> Duration,
    cancelled: impl Fn() -> bool) -> Result<VerifiedSource, &'static str> {
    // The existing sections probe has a four-second bound. Reserve it before
    // admitting identity, and check again after local authorization checks,
    // immediately before sending the credential. Never restart the caller's clock.
    if cancelled() || remaining() <= Duration::from_secs(4) { return Err("verification cancelled or expired"); }
    let origin = Origin::parse(&profile.address).ok_or("invalid server origin")?;
    let scope = plx_plex::plex::grant::scope();
    let machine_id = identity(&origin, &profile.server_id, &remaining, &cancelled)?;
    if cancelled() || remaining() < Duration::from_secs(4) { return Err("verification cancelled or expired"); }
    check_existing_authorization(&machine_id, &origin, &profile.token)?;
    let proof = plx_plex::plex::grant::prove_import(scope, &origin, &profile.server_id, &machine_id, profile.allow_insecure_http)
        .ok_or("server verification is no longer current")?;
    if cancelled() || remaining() < Duration::from_secs(4) { return Err("verification cancelled or expired"); }
    let permitted = matches!(plx_plex::http::request_import_probe(&proof, &profile.token), Ok(reply) if reply.ok());
    if cancelled() || remaining().is_zero() { return Err("verification cancelled or expired"); }
    if !permitted { return Err("Plex server did not authorize library access"); }
    let address = plx_plex::plex::ResolvePin::for_manual_origin(&origin)
        .map_or_else(|| origin.host().to_owned(), |pin| pin.addr().to_string());
    let tier = Some(plx_plex::plex::probe::configured_tier(&address));
    if cancelled() || remaining().is_zero() { return Err("verification cancelled or expired"); }
    Ok(VerifiedSource { source: SourceRef { machine_id, name: profile.name.clone(),
        address, port: i64::from(origin.port()), token: profile.token.clone(),
        origin_url: origin.base(), tier, ..Default::default() }, proof })
}

/// Add a durably stored verified source without retokening an account-owned slot.
pub(super) fn register_verified(verified: &VerifiedSource) -> Result<plx_plex::plex::ServerId, &'static str> {
    if !verified.proof.current() { return Err("server verification is no longer current"); }
    let source = &verified.source;
    let origin = source.origin().ok_or("invalid server origin")?;
    let key = source_key(&source.machine_id, &origin, &source.token);
    let mut authority = IMPORT_AUTHORITY.lock().map_err(|_| "import authority unavailable")?;
    if !authority.allowed.contains(&key) { return Err("Plex source was removed or replaced"); }
    let prior = authority.owned.iter().find(|(stored,_)| stored == &key).map(|(_,lease)|lease);
    // An account-owned matching source may be borrowed, never claimed. Its
    // blank/different token remains the account owner's responsibility.
    if prior.is_none() {
        if let Some(id) = plx_plex::plex::id_of_machine(&source.machine_id) {
            let client = plx_plex::plex::client_for(id).ok_or("existing Plex authorization is unavailable")?;
            if client.origin() == &origin && client.uses_token(&source.token) {
                return plx_plex::plex::borrow_verified_import(&verified.proof, &source.token)
                    .ok_or("existing Plex authorization changed");
            }
            return Err("This server already uses a different native authorization");
        }
    }
    let lease = plx_plex::plex::register_imported(&verified.proof, &source.token,
        &plx_plex::plex::session::peek().client_id,
        plx_plex::plex::ConnectionFacts::new(source.tier, plx_plex::plex::IpVersion::of_host(&source.address)), prior)
        .ok_or("Plex source ownership or verification changed")?;
    let id = lease.server_id();
    authority.owned.retain(|(stored,_)| stored != &key);
    authority.owned.push((key,lease));
    plx_plex::plex::describe_server_name(id, &source.name);
    Ok(id)
}

pub(super) fn check_existing_authorization(machine: &str, origin: &Origin, token: &str) -> Result<(), &'static str> {
    let key = source_key(machine, origin, token);
    let authority = IMPORT_AUTHORITY.lock().map_err(|_| "import authority unavailable")?;
    if let Some((_,lease)) = authority.owned.iter().find(|(stored,_)| stored == &key) {
        if authority.allowed.contains(&key) && plx_plex::plex::imported_lease_matches(lease, machine, origin, token) { return Ok(()); }
    }
    if let Some(id) = plx_plex::plex::id_of_machine(machine) {
        let client = plx_plex::plex::client_for(id).ok_or("existing Plex authorization is unavailable")?;
        if client.origin() != origin || !client.uses_token(token) {
            return Err("This server already uses a different native authorization; change it through native Plex sign-in");
        }
    }
    Ok(())
}

/// Re-prove stored server descriptors on the worker; persistence itself grants no network access.
pub(super) fn restore_verified_sources(
    worker: &OffFrame,
    sources: Vec<SourceRef>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Vec<Result<VerifiedSource, &'static str>> {
    restore_sources_with(sources,
        || cancel.load(std::sync::atomic::Ordering::Acquire), |profile| verify_profile(worker, profile))
}

fn restore_sources_with<T>(
    sources: Vec<SourceRef>,
    cancelled: impl Fn() -> bool,
    mut verify: impl FnMut(&Profile) -> Result<T, &'static str>,
) -> Vec<Result<T, &'static str>> {
    if sources.len() > 15 { return vec![Err("too many stored Plex sources")]; }
    let mut results = Vec::new();
    for source in sources {
        if cancelled() { break; }
        let result = (|| {
            let origin = source.origin().ok_or("invalid stored server origin")?;
            if let Some(pin) = plx_plex::plex::ResolvePin::for_manual_origin(&origin) {
                // Old manual records stored the certificate host as address.
                // New records store the encoded literal; conflicting persisted
                // address/port evidence must not silently choose another route.
                if source.port != i64::from(origin.port())
                    || (source.address != origin.host()
                        && plx_plex::plex::ResolvePin::for_origin(&origin, &source.address).as_ref() != Some(&pin))
                {
                    return Err("stored Plex address does not match its origin");
                }
            }
            let profile = Profile::parse(serde_json::json!({"address":origin.base(),"token":source.token,
                "serverId":source.machine_id,"name":source.name,"allowInsecureHttp":!origin.is_tls()}).to_string().as_bytes())?;
            verify(&profile)
        })();
        // An already admitted network request has its existing finite budget.
        // Cancellation discards its reply and prevents the next source request.
        if cancelled() { break; }
        results.push(result);
    }
    results
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    #[test]
    fn replacing_authorized_source_retires_only_its_lease_and_rejects_old_result() {
        let _guard = plx_base::testlock::serial();
        plx_plex::plex::grant::reset_for_test();
        plx_plex::plex::reset_servers_for_test();
        *IMPORT_AUTHORITY.lock().unwrap() = ImportAuthority::default();
        let origin = Origin::http("127.0.0.1", 32400);
        let proof = plx_plex::plex::grant::prove_import(plx_plex::plex::grant::scope(), &origin, "source", "source", true).unwrap();
        let source = SourceRef { machine_id:"source".into(),origin_url:origin.base(),address:origin.host().into(),port:i64::from(origin.port()),token:"source-token".into(),..Default::default() };
        set_authorized_sources(std::slice::from_ref(&source)).unwrap();
        let lease = plx_plex::plex::register_imported(&proof, &source.token, "test-client", plx_plex::plex::ConnectionFacts::default(), None).unwrap();
        let id = lease.server_id();
        IMPORT_AUTHORITY.lock().unwrap().owned.push((source_key(&source.machine_id,&origin,&source.token),lease));
        let old = VerifiedSource {source:source.clone(),proof};
        let mut replacement = source;
        replacement.token = "new-token".into();
        set_authorized_sources(&[replacement]).unwrap();
        assert!(!plx_plex::plex::client_for(id).unwrap().uses_token("source-token"));
        assert_eq!(register_verified(&old).err(),Some("Plex source was removed or replaced"));
        set_authorized_sources(&[]).unwrap();
        *IMPORT_AUTHORITY.lock().unwrap() = ImportAuthority::default();
        plx_plex::plex::grant::reset_for_test();
        plx_plex::plex::reset_servers_for_test();
    }

    fn exercise(machine: &'static str, library_status: u16) -> Result<Option<Session>, &'static str> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let requests = if machine == "expected" { 2 } else { 1 };
            for index in 0..requests {
                let (mut stream, _) = plx_base::testnet::accept_within(&listener, Duration::from_secs(6)).expect("request did not arrive");
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut bytes = Vec::new();
                let mut part = [0u8; 1024];
                while !bytes.windows(4).any(|s| s == b"\r\n\r\n") {
                    let n = stream.read(&mut part).unwrap();
                    assert!(n > 0 && bytes.len() < 16384);
                    bytes.extend_from_slice(&part[..n]);
                }
                let request = String::from_utf8(bytes).unwrap().to_ascii_lowercase();
                let (status, body) = if index == 0 {
                    assert!(request.starts_with("get /identity "));
                    assert!(!request.contains("x-plex-token"), "identity probe carried a credential");
                    (200, format!("{{\"MediaContainer\":{{\"machineIdentifier\":\"{machine}\"}}}}"))
                } else {
                    assert!(request.starts_with("get /library/sections "));
                    assert!(request.contains("x-plex-token: server-only"));
                    (library_status, "{\"MediaContainer\":{\"Directory\":[]}}".to_owned())
                };
                write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let directory = std::env::temp_dir().join(format!("ott-native-import-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("profile.json");
        std::fs::write(&path, serde_json::to_vec(&serde_json::json!({
            "baseUrl": format!("http://127.0.0.1:{port}"), "token": "server-only",
            "serverId": "expected", "allowInsecureHttp": true
        })).unwrap()).unwrap();
        let saved: Session = serde_json::from_value(serde_json::json!({
            "client_id": "local-client", "account_token": "account-only", "future_setting": "preserved"
        })).unwrap();
        let result = read_and_verify(&OffFrame::for_test(), &path, &saved, &std::sync::atomic::AtomicBool::new(false)).map(|candidate| candidate.map(|candidate| {
            assert!(candidate.verified.proof.current());
            assert!(!plx_plex::plex::grant::allowed_for(
                plx_plex::plex::CredentialPolicy::HttpsOnly, "expected", &candidate.verified.source.origin().unwrap()));
            candidate.credentials.merge_into(&saved)
        }));
        server.join().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        result
    }

    #[test]
    fn verified_import_preserves_settings_and_scopes_the_server_token() {
        let _guard = plx_base::testlock::serial();
        plx_plex::plex::grant::reset_for_test();
        let imported = exercise("expected", 200).unwrap().unwrap();
        assert_eq!(imported.client_id, "local-client");
        assert!(imported.account_token.is_empty());
        assert_eq!(imported.server.token, "server-only");
        assert_eq!(serde_json::to_value(&imported).unwrap()["future_setting"], "preserved");
        plx_plex::plex::grant::reset_for_test();
    }

    #[test]
    fn identity_mismatch_and_unauthorized_library_do_not_import() {
        let _guard = plx_base::testlock::serial();
        plx_plex::plex::grant::reset_for_test();
        assert_eq!(exercise("different", 200).err(), Some("server identity does not match the profile"));
        assert_eq!(exercise("expected", 401).err(), Some("Plex server did not authorize library access"));
        plx_plex::plex::grant::reset_for_test();
    }

    #[test]
    fn absent_profile_keeps_the_normal_session_and_qr_flow() {
        let directory = std::env::temp_dir().join(format!("ott-native-absent-profile-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        assert!(read_and_verify(&OffFrame::for_test(), &directory.join("missing.json"), &Session::default(), &std::sync::atomic::AtomicBool::new(false)).unwrap().is_none());
    }
}

#[cfg(test)]
mod restore_cancellation_tests {
    use super::*;
    use std::cell::Cell;
    fn source() -> SourceRef {
        SourceRef { machine_id: "fixture".into(), token: "fixture".into(),
            origin_url: "https://media.invalid:443".into(), name: "Fixture".into(),
            address: "media.invalid".into(), port: 443, ..Default::default() }
    }
    #[test]
    fn cancelled_restore_never_dispatches_verification() {
        let result: Vec<Result<(), &'static str>> = restore_sources_with(
            vec![source()], || true, |_| panic!("verification after cancellation"));
        assert!(result.is_empty());
    }
    #[test]
    fn cancellation_after_one_verification_discards_it_and_skips_remaining_sources() {
        let cancelled = Cell::new(false);
        let calls = Cell::new(0);
        let result = restore_sources_with(vec![source(), source()], || cancelled.get(), |_| {
            calls.set(calls.get() + 1);
            cancelled.set(true);
            Ok(())
        });
        assert_eq!(calls.get(), 1);
        assert!(result.is_empty());
    }
    #[test]
    fn active_restore_keeps_real_verifier_failure_without_starting_another_worker() {
        let calls = Cell::new(0);
        let result: Vec<Result<(), &'static str>> = restore_sources_with(
            vec![source()], || false, |_| {
                calls.set(calls.get() + 1);
                Err("fixture verification failed")
            });
        assert_eq!(calls.get(), 1);
        assert_eq!(result, vec![Err("fixture verification failed")]);
    }
}

#[cfg(test)]
#[path = "ott_import_resolve_tests.rs"]
mod manual_resolve_tests;

#[cfg(test)]
#[path = "ott_import_fixture_tests.rs"]
mod fixture_boundary_tests;

#[cfg(all(test, feature = "hostsim"))]
mod fixed_deadline_tests {
    use super::*;

    #[test]
    fn expired_managed_reverification_refuses_before_configuration_io() {
        let _serial = plx_base::testlock::serial();
        plx_plex::plex::grant::reset_for_test();
        struct Reset;
        impl Drop for Reset { fn drop(&mut self) { plx_plex::plex::grant::reset_for_test(); } }
        let _reset = Reset;
        let source = SourceRef { machine_id: "fixed-deadline".into(), name: "Fixture".into(),
            origin_url: "http://127.0.0.1:32400".into(), address: "127.0.0.1".into(),
            port: 32400, token: "fixture-token".into(), ..Default::default() };
        let candidate = managed_candidate_for_test(&Session::default(), source);
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        assert_eq!(candidate.reverify_before(|| Duration::ZERO, &cancelled).err(),
            Some("verification cancelled or expired"));
        // The synthetic candidate's nonexistent configuration was not consulted.
        // No identity or credential probe can start once the carried budget ends.
    }

    #[test]
    fn positive_budget_is_consumed_by_recheck_before_verification() {
        use std::cell::Cell;
        let _serial = plx_base::testlock::serial();
        plx_plex::plex::grant::reset_for_test();
        struct Reset;
        impl Drop for Reset { fn drop(&mut self) { plx_plex::plex::grant::reset_for_test(); } }
        let _reset = Reset;
        let worker = OffFrame::for_test();
        let initial = plx_base::task::time::ClockSample::capture(&worker).unwrap();
        let source = SourceRef { machine_id: "fixed-positive-deadline".into(), name: "Fixture".into(),
            origin_url: "http://127.0.0.1:32400".into(), address: "127.0.0.1".into(),
            port: 32400, token: "fixture-token".into(), ..Default::default() };
        let candidate = managed_candidate_for_test(&Session::default(), source);
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        for expire in [false, true] {
            let deadline = initial.deadline_at_unix_ms(initial.unix_ms() + 8_000);
            let observed = Cell::new(initial);
            let rechecks = Cell::new(0u32);
            let verifications = Cell::new(0u32);
            let result = candidate.reverify_before_with(
                || deadline.remaining_at_sample(observed.get()), &cancelled,
                || {
                    let count = rechecks.get() + 1;
                    rechecks.set(count);
                    let spent = if expire { 8_000 } else { 2_000 * u64::from(count) };
                    observed.set(initial.advance_for_test(Duration::from_millis(spent)));
                    Ok(())
                },
                |_, remaining, _| {
                    verifications.set(verifications.get() + 1);
                    assert_eq!(remaining(), Duration::from_secs(6), "recheck time was renewed");
                    Ok(candidate.verified.clone())
                },
            );
            if expire {
                assert_eq!(result.err(), Some("verification cancelled or expired"));
                assert_eq!(rechecks.get(), 1);
                assert_eq!(verifications.get(), 0, "expired recheck must not dispatch the verifier");
            } else {
                assert!(result.is_ok());
                assert_eq!(rechecks.get(), 2);
                assert_eq!(verifications.get(), 1);
                assert_eq!(deadline.remaining_at_sample(observed.get()), Duration::from_secs(4));
            }
        }
    }

}
