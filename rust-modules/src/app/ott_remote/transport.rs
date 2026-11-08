//! Bounded native IO adapter; never called from a model/screen step.
use super::protocol::{Config, Request, Response, BODY_LIMIT};
use plx_base::task::{time::Budget, OffFrame};
use plx_platform::storage::{private, CommitReceipt};
use serde_json::Value;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, TryRecvError},
    Arc,
};
use std::time::Duration;

pub(super) enum Outcome {
    LanDiscovery(Result<Vec<plx_net::control_discovery::ControlServer>, plx_net::control_discovery::Error>),
    Loaded(Result<(Config, bool, String), &'static str>),
    Saved(Result<(), &'static str>),
    Http {
        response: Result<Response, plx_net::net::RequestFailure>,
        elapsed_ms: u64,
    },
}
pub(super) struct Task {
    receiver: Receiver<Outcome>,
    canceled: Arc<AtomicBool>,
}
impl Task {
    #[cfg(all(test, feature = "hostsim"))]
    pub(super) fn pending_for_test() -> (Self, mpsc::SyncSender<Outcome>) {
        let (sender, receiver) = mpsc::sync_channel(1);
        (Self { receiver, canceled: Arc::new(AtomicBool::new(false)) }, sender)
    }
    pub fn poll(&self) -> Result<Outcome, TryRecvError> {
        self.receiver.try_recv()
    }
    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::Release);
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn launch(work: impl FnOnce(&OffFrame, &AtomicBool) -> Outcome + Send + 'static) -> Option<Task> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let canceled = Arc::new(AtomicBool::new(false));
    let flag = canceled.clone();
    let launched = plx_base::task::spawn_off_frame("ott-control", move |worker| {
        if flag.load(Ordering::Acquire) {
            return;
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(worker, &flag)));
        if flag.load(Ordering::Acquire) {
            return;
        }
        if let Ok(result) = result {
            let _ = sender.send(result);
        }
        plx_machine::idle::invalidate();
    });
    launched.then_some(Task { receiver, canceled })
}
pub(super) fn discover() -> Option<Task> {
    launch(|worker, canceled| Outcome::LanDiscovery(
        plx_net::control_discovery::discover(worker, || canceled.load(Ordering::Acquire))))
}
pub(super) fn send(request: Request) -> Option<Task> {
    send_bounded(request, BODY_LIMIT)
}
pub(super) fn send_bounded(request: Request, body_limit: usize) -> Option<Task> {
    launch(move |worker, _| {
        let budget = Budget::start(worker, Duration::from_secs(8));
        let mut headers = Vec::new();
        if !request.token.is_empty() {
            headers.push(format!("Authorization: Bearer {}", request.token));
        }
        if request.body.is_some() {
            headers.push("Content-Type: application/json".into());
        }
        let response = http_request(&request, &headers, body_limit);
        #[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
        plx_platform::tv::secure::fixture::observe_local_control(request.method, &request.url,
            request.body.as_deref(), response.as_ref().ok().map(|r| (r.status, r.body.as_slice())));
        let response = response.map(|r| Response {
            status: r.status,
            body: r.body,
        });
        Outcome::Http {
            response,
            elapsed_ms: u64::try_from(budget.spent(worker).as_millis()).unwrap_or(u64::MAX),
        }
    })
}

fn http_request(request: &Request, headers: &[String], body_limit: usize)
    -> Result<plx_net::net::Resp, plx_net::net::RequestFailure>
{
    let timeouts = plx_net::net::Timeouts {
        connect_s: 4, total_s: 8, total_ms: 8000, low_speed_bps: 0, low_speed_s: 0,
    };
    #[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
    if let Some(bundle) = plx_platform::tv::secure::fixture::control_ca_bundle(&request.url)
        .map_err(|_| plx_net::net::RequestFailure::from(plx_net::net::RequestError::Transport))?
    {
        // Only this IO adapter consumes the fixture grant. Redirects remain off,
        // so its trust cannot spill to an endpoint outside the declared origin.
        return plx_net::net::request_ca_bundle_evidence(
            &request.url, headers, request.method,
            request.body.as_deref().map(str::as_bytes), timeouts, false,
            Some(body_limit), bundle,
        );
    }
    plx_net::net::request_result_evidence(
        &request.url, headers, request.method, request.body.as_deref().map(str::as_bytes),
        timeouts, false, Some(body_limit), None, false,
    )
}

/// Only closed labels and numeric transport evidence may reach a status message.
/// Never format a request URL, header, response body, or arbitrary curl text.
pub(super) fn failure_message(failure: &plx_net::net::RequestFailure) -> String {
    use plx_net::net::RequestError;
    let label = if failure.body_limit.is_some() {
        "Response exceeds the size limit"
    } else if failure.cause == RequestError::TimedOut {
        "Request timed out"
    } else {
        match failure.curl_rc {
            Some(5 | 6) => "DNS lookup failed",
            Some(7) => "Connection failed",
            Some(35) => "TLS handshake failed",
            Some(51 | 60) => "TLS certificate verification failed",
            Some(58) => "TLS client certificate failed",
            Some(77) => "TLS trust store unavailable",
            Some(90) => "TLS public key verification failed",
            _ => "Network request failed",
        }
    };
    let mut message = label.to_owned();
    if let Some(status) = failure.status {
        message.push_str(&format!("; HTTP {status}"));
    }
    if let Some(code) = failure.curl_rc {
        message.push_str(&format!("; transport code {code}"));
    }
    if failure.curl_rc == Some(60) {
        if let Some(verify) = failure.verify {
            message.push_str(&format!("; verification code {verify}"));
        }
    }
    message
}

pub(super) fn load(device_id: String, runtime: String, permit: private::WritePermit,
    confirm_existing: bool) -> Option<Task> {
    launch(move |_, _| {
        Outcome::Loaded(load_config(device_id, &permit, confirm_existing).and_then(|(config, persisted)| {
            let runtime = if runtime.is_empty() {
                random_identity("native-")?
            } else {
                runtime
            };
            Ok((config, persisted, runtime))
        }))
    })
}
fn random_identity(prefix: &str) -> Result<String, &'static str> {
    let generation = plx_platform::storage::state::Generation::random()
        .map_err(|_| "Native identity generation failed")?;
    let mut identity = prefix.to_owned();
    use std::fmt::Write;
    for byte in generation.0 {
        let _ = write!(&mut identity, "{byte:02x}");
    }
    Ok(identity)
}
fn confirm_config(config: Config, required: bool,
    confirm: impl FnOnce(&Config) -> Result<(), &'static str>) -> Result<(Config, bool), &'static str> {
    if required {
        // A prior uncertain write may be readable without proven durability.
        // Explicit Retry must re-confirm the same identity before it can pair.
        confirm(&config)?;
    }
    Ok((config, true))
}
fn load_config(device_id: String, permit: &private::WritePermit,
    confirm_existing: bool) -> Result<(Config, bool), &'static str> {
    #[cfg(all(target_os = "linux", target_arch = "arm", not(any(feature = "hostsim", test))))]
    let stored = helper_control::load(permit, plx_platform::storage::control::load_guarded);
    #[cfg(not(all(target_os = "linux", target_arch = "arm", not(any(feature = "hostsim", test)))))]
    let stored = private::load(plx_platform::tv::secure::Namespace::OttControl)
        .map_err(|_| "Saved control configuration is locked or unavailable")
        .and_then(|value| value.map(Config::parse).transpose());
    match stored {
        Ok(Some(config)) => return confirm_config(config, confirm_existing,
            |config| save_config(config, permit)),
        Err(error) => return Err(error),
        Ok(None) => {}
    }
    let suppressed = private::imports_suppressed().map_err(|_| "Managed import policy is unavailable")?;
    let managed = if suppressed { Ok(None) } else {
        plx_platform::storage::read_owned_bytes(&plx_base::paths::in_app_dir("ottplay-control.json"))
    };
    match managed {
        Ok(Some((bytes, true))) if bytes.len() <= 65536 => {
            let value = serde_json::from_slice(&bytes)
                .map_err(|_| "Invalid managed control configuration")?;
            let config = Config::parse(value)?;
            // The explicit, private install file is itself the managed authority.
            // An unavailable keymanager never silently copies its token into another plaintext file.
            let _ = save_config(&config, permit);
            Ok((config, true))
        }
        Ok(None) => bootstrap_public_identity(device_id, permit),
        _ => Err("Managed control configuration is unsafe or unreadable"),
    }
}
// A fresh, tokenless installation needs durable public identity, not a sealed
// credential record. Existing encrypted/managed configurations retain precedence.
fn bootstrap_public_identity(device_id: String, permit: &private::WritePermit)
    -> Result<(Config, bool), &'static str>
{
    use plx_platform::storage::public_preferences::{self, Patch};
    confirm_public_identity(device_id, permit,
        || public_preferences::load()
            .map(|snapshot| snapshot.and_then(|snapshot| snapshot.value.device_id))
            .map_err(|_| "Public control identity is unavailable"),
        || random_identity("dev_"),
        |candidate| public_preferences::patch_guarded(Patch::EnsureDeviceId(candidate), permit)
            .map_err(|_| "Public control identity could not be saved")
            .and_then(|(snapshot, receipt)| snapshot.value.device_id
                .map(|identity| (identity, receipt))
                .ok_or("Saved public control identity is missing")))
}

fn confirm_public_identity(
    device_id: String,
    permit: &private::WritePermit,
    load: impl FnOnce() -> Result<Option<String>, &'static str>,
    generate: impl FnOnce() -> Result<String, &'static str>,
    ensure: impl FnOnce(String) -> Result<(String, CommitReceipt), &'static str>,
) -> Result<(Config, bool), &'static str> {
    if !permit.is_valid() { return Err("Control configuration was retired"); }
    let candidate = match load()? {
        Some(existing) => existing,
        None if super::protocol::valid_device(&device_id) => device_id,
        None => generate()?,
    };
    if !super::protocol::valid_device(&candidate) {
        return Err("Invalid public control identity");
    }
    if !permit.is_valid() { return Err("Control configuration was retired"); }
    // Ensure is a fresh CAS commit/readback even for an existing ID. A plain
    // readable value cannot confirm the durability of an earlier uncertain write.
    let (identity, receipt) = ensure(candidate)?;
    if !permit.is_valid() { return Err("Control configuration was retired"); }
    if !matches!(receipt, CommitReceipt::Durable) {
        return Err("Public control identity durability is unconfirmed");
    }
    if !super::protocol::valid_device(&identity) {
        return Err("Invalid public control identity");
    }
    Ok((Config::disabled(identity), true))
}

pub(super) fn save(config: Config, permit: private::WritePermit) -> Option<Task> {
    launch(move |_, _| Outcome::Saved(save_config(&config, &permit)))
}
fn save_config(config: &Config, permit: &private::WritePermit) -> Result<(), &'static str> {
    #[cfg(all(target_os = "linux", target_arch = "arm", not(any(feature = "hostsim", test))))]
    let receipt = helper_control::save(config, permit, plx_platform::storage::control::save_guarded)?;
    #[cfg(not(all(target_os = "linux", target_arch = "arm", not(any(feature = "hostsim", test)))))]
    let receipt = {
        let value: Value = serde_json::to_value(config).map_err(|_| "Invalid control configuration")?;
        private::save_guarded(plx_platform::tv::secure::Namespace::OttControl, &value, permit)
            .map_err(|_| "Protected control storage is unavailable")?
    };
    match receipt {
        CommitReceipt::Durable => Ok(()),
        CommitReceipt::Uncertain { .. } => {
            Err("Control configuration durability is unconfirmed")
        }
    }
}

// The paired ARM load/save adapter is compiled only for the native target (and
// pure host tests). Hostsim and app tests keep the existing private fixture path;
// test-support is a dependency feature, not an app feature that cfg can inspect.
#[cfg(any(test, all(target_os = "linux", target_arch = "arm", not(feature = "hostsim"))))]
mod helper_control {
    use super::*;
    use plx_platform::storage::{control, wire::SecretString};

    fn decode(value: Value) -> Result<control::Config, ()> {
        // This is also the helper coordinator's required legacy decoder. Keep
        // defaults and URL normalization exactly at the existing app boundary.
        let config = Config::parse(value).map_err(|_| ())?;
        let value = control::Config {
            version: config.version, enabled: config.enabled, address: config.address,
            token: SecretString(config.token), device_id: config.device_id,
            allow_insecure_http: config.allow_insecure_http,
        };
        value.validate().map_err(|_| ())?;
        Ok(value)
    }

    pub(super) fn load(permit: &private::WritePermit,
        load: impl FnOnce(&private::WritePermit, control::LegacyDecoder)
            -> Result<Option<control::Snapshot>, control::Error>,
    ) -> Result<Option<Config>, &'static str> {
        if !permit.is_valid() { return Err("Control configuration was retired"); }
        let stored = load(permit, decode)
            .map_err(|_| "Saved control configuration is locked or unavailable")?;
        if !permit.is_valid() { return Err("Control configuration was retired"); }
        stored.map(|snapshot| {
            let value = serde_json::to_value(snapshot.value).map_err(|_| "Invalid control configuration")?;
            Config::parse(value)
        }).transpose()
    }

    pub(super) fn save(config: &Config, permit: &private::WritePermit,
        save: impl FnOnce(&control::Config, &private::WritePermit, control::LegacyDecoder)
            -> Result<CommitReceipt, control::Error>,
    ) -> Result<CommitReceipt, &'static str> {
        if !permit.is_valid() { return Err("Control configuration was retired"); }
        let value = serde_json::to_value(config).map_err(|_| "Invalid control configuration")?;
        let value = decode(value).map_err(|_| "Invalid control configuration")?;
        let receipt = save(&value, permit, decode).map_err(|error| match error {
            control::Error::Uncertain => "Control configuration durability is unconfirmed",
            _ => "Protected control storage is unavailable",
        })?;
        if !permit.is_valid() { return Err("Control configuration was retired"); }
        Ok(receipt)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde_json::json;

        #[test]
        fn app_parser_normalizes_legacy_and_fresh_configs_with_all_fields_preserved() {
            let input = json!({"version":1,"enabled":true,
                "address":" http://127.0.0.1/api/webhook/commands/// ",
                "token":"a".repeat(32),"device_id":"dev_fixture","allowInsecureHttp":true});
            let expected = json!({"version":1,"enabled":true,"address":"http://127.0.0.1:80",
                "token":"a".repeat(32),"device_id":"dev_fixture","allowInsecureHttp":true});
            assert_eq!(serde_json::to_value(decode(input.clone()).unwrap()).unwrap(), expected);
            let unparsed: Config = serde_json::from_value(input.clone()).unwrap();
            let receipt = save(&unparsed, &private::WritePermit::new(), |value, _, legacy_decode| {
                assert_eq!(serde_json::to_value(value).unwrap(), expected);
                assert_eq!(serde_json::to_value(legacy_decode(input).unwrap()).unwrap(), expected);
                Ok(CommitReceipt::Durable)
            }).unwrap();
            assert_eq!(receipt, CommitReceipt::Durable);
            let defaults = decode(json!({"device_id":"dev_fixture"})).unwrap();
            assert!(!defaults.enabled && defaults.address.is_empty() && defaults.token.0.is_empty());
            assert!(!defaults.allow_insecure_http);
        }

        #[test]
        fn helper_load_distinguishes_absence_from_locked_or_unknown_and_preserves_config() {
            let permit = private::WritePermit::new();
            for error in [control::Error::Locked(private::Error::ProtectionUnavailable),
                control::Error::Uncertain,
                control::Error::Storage(private::Error::Store(plx_platform::storage::StoreError::HelperUnavailable))] {
                assert!(matches!(load(&permit, |_, _| Err(error)),
                    Err("Saved control configuration is locked or unavailable")));
            }
            assert!(load(&permit, |_, _| Ok(None)).unwrap().is_none());
            let value = json!({"enabled":true,"address":"https://home.example/webhook/poll",
                "token":"b".repeat(32),"device_id":"dev_existing"});
            let loaded = load(&permit, |_, legacy_decode| Ok(Some(control::Snapshot {
                value: legacy_decode(value.clone()).unwrap(), revision: 4, epoch: 2,
            }))).unwrap().unwrap();
            assert_eq!(serde_json::to_value(loaded).unwrap(),
                serde_json::to_value(Config::parse(value).unwrap()).unwrap());
        }

        #[test]
        fn invalid_or_retired_config_does_not_write_and_late_retirement_does_not_publish() {
            let permit = private::WritePermit::new();
            let mut config = Config::disabled("dev_fixture".into());
            config.enabled = true;
            assert!(save(&config, &permit, |_, _, _| panic!("invalid config must not write")).is_err());
            config.enabled = false;
            permit.revoke();
            assert!(save(&config, &permit, |_, _, _| panic!("retired config must not write")).is_err());
            assert!(load(&permit, |_, _| panic!("retired config must not load")).is_err());
            let permit = private::WritePermit::new();
            assert_eq!(save(&config, &permit, |_, permit, _| {
                permit.revoke();
                Ok(CommitReceipt::Durable)
            }), Err("Control configuration was retired"));
        }
    }
}

#[cfg(test)]
mod bootstrap_tests {
    use super::*;
    use super::super::{Flight, Kind, Remote};
    use serde_json::json;

    fn failed_load() -> Remote {
        let mut remote = Remote::from_install("dev_fixture", "runtime-fixture");
        remote.finish(
            Kind::Load,
            Outcome::Loaded(Err("Protected control storage is unavailable")),
            100,
        );
        remote
    }

    #[test]
    fn bootstrap_failure_stays_visible_until_explicit_retry_and_durable_success() {
        let mut remote = failed_load();
        let failure = remote.snapshot();
        assert_eq!(failure["state"], "error");
        assert_eq!(failure["message"], "Protected control storage is unavailable");
        assert_eq!(failure["persisted"], false);
        assert!(!remote.loading);
        assert!(!remote.confirm_loaded_config);
        remote.cancel_pairing();
        assert_eq!(remote.snapshot(), failure, "Cancel cannot hide a bootstrap failure");
        assert!(remote.discover_and_pair("https://control.invalid", 101).is_err());
        assert!(remote.begin_pairing("https://control.invalid", "fixture", 101).is_err());
        assert!(remote.can_retry_load());
        let generation = remote.generation;
        remote.retry_load().unwrap();
        assert_ne!(remote.generation, generation);
        assert!(remote.loading && !remote.persisted);
        assert!(remote.confirm_loaded_config, "explicit Retry requests confirmation");
        assert_eq!(remote.snapshot()["state"], "loading");
        assert!(!remote.can_retry_load());
        assert!(remote.retry_load().is_err(), "only one attempt is admitted");
        assert!(remote.discover_and_pair("https://control.invalid", 102).is_err());

        // Model the existing worker's durable completion, not a private-store write.
        remote.finish(Kind::Load, Outcome::Loaded(Ok((
            Config::disabled("dev_fixture".into()), true, "runtime-fixture".into(),
        ))), 200);
        assert!(remote.persisted && !remote.loading);
        assert!(!remote.confirm_loaded_config);
        assert!(!remote.can_retry_load());
        assert_eq!(remote.snapshot()["state"], "disabled");
        remote.discover_and_pair("https://control.invalid", 201).unwrap();
        assert_eq!(remote.snapshot()["state"], "discovering");
        assert!(remote.discover_address.is_some(), "only discovery admission; no HTTP");
    }

    #[test]
    fn bootstrap_retry_preserves_pending_durable_and_revoked_owners() {
        let mut remote = Remote::from_install("dev_fixture", "runtime-fixture");
        assert!(remote.retry_load().is_err(), "initial load already owns admission");
        let mut remote = failed_load();
        remote.pending_save = Some(Config::disabled("dev_fixture".into()));
        let generation = remote.generation;
        assert!(remote.retry_load().is_err());
        assert!(remote.pending_save.is_some());
        assert_eq!(remote.generation, generation);
        let mut remote = Remote::disabled_for_test("dev_fixture", "runtime-fixture");
        remote.fail(100, "Fixture connection failure");
        assert!(remote.retry_load().is_err(), "network errors cannot reload durable identity");
        let mut remote = failed_load();
        remote.retire_for_erasure();
        assert!(remote.retry_load().is_err(), "erasure cannot regain a write permit");
        assert!(!remote.loading);
    }

    #[test]
    fn bootstrap_worker_refusal_does_not_repeat_on_later_ticks() {
        let mut remote = Remote::from_install("dev_fixture", "runtime-fixture");
        remote.launch(Kind::Load, None, 100);
        assert!(!remote.loading, "refused load is a completed failed attempt");
        assert_eq!(remote.snapshot()["state"], "error");
        assert_eq!(remote.snapshot()["message"], "Remote control worker unavailable");
        assert!(remote.can_retry_load());
        for now in [1000, 30_000, 60_000] {
            assert!(remote.tick(now, json!({})).is_empty());
            assert!(remote.flight.is_none(), "no automatic bootstrap worker retry");
        }
        remote.retry_load().unwrap();
        assert!(remote.loading && !remote.can_retry_load());
    }

    #[test]
    fn bootstrap_live_task_stays_single_flight_until_its_real_channel_completion() {
        let mut remote = Remote::from_install("dev_fixture", "runtime-fixture");
        let (sender, receiver) = mpsc::sync_channel(1);
        remote.flight = Some(Flight {
            kind: Kind::Load,
            generation: remote.generation,
            task: Task { receiver, canceled: Arc::new(AtomicBool::new(false)) },
        });
        let generation = remote.generation;
        assert!(remote.retry_load().is_err());
        assert!(remote.tick(100, json!({})).is_empty());
        assert!(remote.flight.is_some());
        assert_eq!(remote.generation, generation);
        assert!(sender.send(Outcome::Loaded(Err("Control configuration durability is unconfirmed"))).is_ok());
        assert!(remote.tick(101, json!({})).is_empty());
        assert!(remote.flight.is_none());
        assert!(!remote.loading && !remote.persisted);
        assert_eq!(remote.snapshot()["message"], "Control configuration durability is unconfirmed");
        assert!(remote.can_retry_load());
        remote.retry_load().unwrap();
        assert!(remote.loading);
    }

    #[test]
    fn bootstrap_disconnected_task_is_not_silently_relaunched() {
        let mut remote = Remote::from_install("dev_fixture", "runtime-fixture");
        let (sender, receiver) = mpsc::sync_channel(1);
        remote.flight = Some(Flight {
            kind: Kind::Load,
            generation: remote.generation,
            task: Task { receiver, canceled: Arc::new(AtomicBool::new(false)) },
        });
        drop(sender);
        assert!(remote.tick(100, json!({})).is_empty());
        assert!(!remote.loading, "disconnected load must release the load request");
        assert!(remote.flight.is_none());
        assert_eq!(remote.snapshot()["message"], "Remote control request failed");
        assert!(remote.can_retry_load());
        assert!(remote.tick(60_000, json!({})).is_empty());
        assert!(remote.flight.is_none(), "later ticks do not retry private IO");
    }

    #[test]
    fn bootstrap_retry_reconfirms_visible_config_before_publishing_persisted() {
        let mut remote = failed_load();
        let visible = Config::disabled("dev_existing".into());
        let expected = serde_json::to_value(&visible).unwrap();
        remote.retry_load().unwrap();
        assert!(remote.confirm_loaded_config);
        let mut confirmations = 0;
        let result = confirm_config(visible.clone(), remote.confirm_loaded_config, |config| {
            confirmations += 1;
            assert_eq!(serde_json::to_value(config).unwrap(), expected,
                "confirmation must keep the existing installation identity");
            Err("Control configuration durability is unconfirmed")
        });
        assert_eq!(confirmations, 1);
        remote.finish(Kind::Load, Outcome::Loaded(result.map(|(config, persisted)|
            (config, persisted, "runtime-fixture".into()))), 200);
        assert!(!remote.persisted && !remote.loading);
        assert_eq!(remote.snapshot()["message"], "Control configuration durability is unconfirmed");
        assert!(remote.discover_and_pair("https://control.invalid", 201).is_err());
        assert!(remote.can_retry_load());

        remote.retry_load().unwrap();
        let result = confirm_config(visible.clone(), remote.confirm_loaded_config, |config| {
            confirmations += 1;
            assert_eq!(serde_json::to_value(config).unwrap(), expected);
            Ok(())
        });
        assert_eq!(confirmations, 2);
        remote.finish(Kind::Load, Outcome::Loaded(result.map(|(config, persisted)|
            (config, persisted, "runtime-fixture".into()))), 300);
        assert!(remote.persisted && !remote.loading);
        assert_eq!(remote.device_id(), "dev_existing");
        remote.discover_and_pair("https://control.invalid", 301).unwrap();
    }

    #[test]
    fn bootstrap_initial_restore_does_not_rewrite() {
        let (config, persisted) = confirm_config(Config::disabled("dev_existing".into()), false, |_| {
            panic!("ordinary initial read must not add a protected write")
        }).unwrap();
        assert!(persisted);
        assert_eq!(config.device_id, "dev_existing");
    }

    #[test]
    fn bootstrap_new_identity_propagates_failure_instead_of_unpersisted_success() {
        let mut confirmations = 0;
        let result = confirm_config(Config::disabled("dev_new".into()), true, |config| {
            confirmations += 1;
            assert_eq!(config.device_id, "dev_new");
            Err("Protected control storage is unavailable")
        });
        assert_eq!(confirmations, 1);
        assert!(matches!(result, Err("Protected control storage is unavailable")),
            "failed bootstrap must not become Ok(config, false)");
    }

    #[test]
    fn public_bootstrap_creates_only_a_durable_tokenless_identity() {
        let permit = private::WritePermit::new();
        let (config, persisted) = confirm_public_identity(String::new(), &permit,
            || Ok(None), || Ok("dev_new".into()), |candidate| {
                assert_eq!(candidate, "dev_new");
                Ok((candidate, CommitReceipt::Durable))
            }).unwrap();
        assert!(persisted);
        assert_eq!(config.device_id, "dev_new");
        assert!(!config.enabled);
        assert!(config.address.is_empty() && config.token.is_empty());
        let mut remote = Remote::from_install("", "runtime-fixture");
        remote.finish(Kind::Load, Outcome::Loaded(Ok((config, persisted, "runtime-fixture".into()))), 1);
        remote.discover_and_pair("https://control.invalid", 2).unwrap();
        assert!(remote.discover_address.is_some());
        assert!(!remote.config.enabled && remote.config.token.is_empty());
    }

    #[test]
    fn public_bootstrap_restart_reconfirms_existing_id_without_entropy() {
        let permit = private::WritePermit::new();
        let (config, persisted) = confirm_public_identity(String::new(), &permit,
            || Ok(Some("dev_existing".into())),
            || panic!("restart must not require new identity entropy"), |candidate| {
                assert_eq!(candidate, "dev_existing");
                Ok((candidate, CommitReceipt::Durable))
            }).unwrap();
        assert!(persisted);
        assert_eq!(config.device_id, "dev_existing");
        assert!(!config.enabled && config.token.is_empty());
    }

    #[test]
    fn public_bootstrap_uses_the_committed_concurrent_identity_winner() {
        let permit = private::WritePermit::new();
        let (config, persisted) = confirm_public_identity("dev_candidate".into(), &permit,
            || Ok(None), || panic!("valid candidate needs no entropy"), |candidate| {
                assert_eq!(candidate, "dev_candidate");
                Ok(("dev_winner".into(), CommitReceipt::Durable))
            }).unwrap();
        assert!(persisted);
        assert_eq!(config.device_id, "dev_winner");
    }

    #[test]
    fn public_bootstrap_failed_read_or_invalid_identity_never_falls_back() {
        let permit = private::WritePermit::new();
        let result = confirm_public_identity("dev_candidate".into(), &permit,
            || Err("Public control identity is unavailable"),
            || panic!("failed read cannot generate a replacement"),
            |_| panic!("failed read cannot authorize a write"));
        assert!(matches!(result, Err("Public control identity is unavailable")));
        let result = confirm_public_identity("dev_candidate".into(), &permit,
            || Ok(Some("invalid identity".into())),
            || panic!("invalid record cannot generate a replacement"),
            |_| panic!("invalid record cannot authorize a write"));
        assert!(matches!(result, Err("Invalid public control identity")));
        let result = confirm_public_identity("dev_candidate".into(), &permit,
            || Ok(None), || panic!("valid candidate needs no entropy"),
            |_| Ok((String::new(), CommitReceipt::Durable)));
        assert!(matches!(result, Err("Invalid public control identity")));
    }

    #[test]
    fn public_bootstrap_failed_or_uncertain_commit_cannot_publish_persisted() {
        let permit = private::WritePermit::new();
        for result in [Err("Public control identity could not be saved"),
            Ok(("dev_existing".into(), CommitReceipt::Uncertain {
                stage: plx_platform::storage::CommitStage::ParentSync, errno: 5,
            }))] {
            let outcome = confirm_public_identity("dev_candidate".into(), &permit,
                || Ok(Some("dev_existing".into())),
                || panic!("existing identity must be retained"), |_| result);
            assert!(outcome.is_err());
            let mut remote = Remote::from_install("", "runtime-fixture");
            remote.finish(Kind::Load, Outcome::Loaded(outcome.map(|(config, persisted)|
                (config, persisted, "runtime-fixture".into()))), 1);
            assert!(!remote.persisted && !remote.config.enabled);
            assert!(remote.discover_and_pair("https://control.invalid", 2).is_err());
        }
    }

    #[test]
    fn public_bootstrap_revocation_before_or_during_commit_cannot_publish() {
        let permit = private::WritePermit::new();
        permit.revoke();
        let outcome = confirm_public_identity("dev_candidate".into(), &permit,
            || panic!("retired owner cannot read"), || panic!("retired owner cannot generate"),
            |_| panic!("retired owner cannot write"));
        assert!(matches!(outcome, Err("Control configuration was retired")));
        let permit = private::WritePermit::new();
        let outcome = confirm_public_identity("dev_candidate".into(), &permit,
            || Ok(None), || panic!("valid candidate needs no entropy"), |candidate| {
                permit.revoke();
                Ok((candidate, CommitReceipt::Durable))
            });
        assert!(matches!(outcome, Err("Control configuration was retired")),
            "the commit may be durable, but a retired owner cannot publish its identity");
    }

    #[test]
    fn public_bootstrap_late_load_after_cancel_cannot_replace_current_identity() {
        let mut remote = Remote::disabled_for_test("dev_current", "runtime-fixture");
        let (sender, receiver) = mpsc::sync_channel(1);
        remote.flight = Some(Flight { kind: Kind::Load, generation: remote.generation,
            task: Task { receiver, canceled: Arc::new(AtomicBool::new(false)) } });
        remote.cancel_pairing();
        sender.send(Outcome::Loaded(Ok((Config::disabled("dev_retired".into()), true,
            "runtime-retired".into())))).unwrap();
        assert!(remote.tick(1, json!({})).is_empty());
        assert!(remote.flight.is_none());
        assert_eq!(remote.device_id(), "dev_current");
        assert_eq!(remote.runtime(), "runtime-fixture");
        assert!(!remote.config.enabled && remote.config.token.is_empty());
    }
}
