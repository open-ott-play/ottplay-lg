//! The webOS port: the one table that fills the `tv` interfaces, and the C entry that installs it.
//!
//! Everything outside this module reaches the television through `plx_platform::tv`; this is the only
//! place that names `webos`, `keymanager`, `system` and `player::ffi` together. Its functions keep
//! their own `cfg(any(hostsim, test))` arms, so the simulator installs this same table and behaves
//! as it always did.
use std::os::raw::{c_char, c_int};

#[cfg(all(not(feature = "hostsim"), not(test)))]
const SINK: &dyn plx_platform::tv::sink::VideoSink = &plx_media::player::ffi::StarfishSink;
#[cfg(feature = "hostsim")]
const SINK: &dyn plx_platform::tv::sink::VideoSink = &plx_media::player::ffi_host::HostSink;
#[cfg(all(not(feature = "hostsim"), test))]
const SINK: &dyn plx_platform::tv::sink::VideoSink = &plx_platform::tv::sink::NoSink;

static PORT: plx_platform::tv::Port = plx_platform::tv::Port {
    removable_identities,
    removable_snapshot,
    probe_device: plx_platform::webos::probe,
    start_capability_probe: plx_platform::webos::caps::start_probe,
    repair_sandbox: || Err(plx_platform::tv::sandbox::Failure::Unsupported),
    seal: plx_platform::keymanager::seal,
    open: plx_platform::keymanager::open,
    remove: plx_platform::keymanager::remove,
    seal_named,
    open_named,
    system_locale: plx_platform::webos::system_locale,
    sound_output: plx_platform::webos::sound_output,
    go_home: plx_platform::webos::go_home,
    poll_home: plx_platform::webos::poll_home,
    deliver_toast: plx_platform::webos::toast::deliver,
    bind_window: plx_platform::webos::bind_window,
    grab_surface: crate::system::sys_grab_wayland,
    release_surface: crate::system::sys_release_wayland,
    arm_opaque_region: crate::system::opaque_region_init,
    opaque_route: crate::system::opaque_route,
    clear_opaque_region: crate::system::clear_opaque_region,
    pump_bus: crate::system::ls2_pump,
    frame_probe_request: crate::system::frame_probe_request,
    frame_probe_prepare_begin: crate::system::frame_probe_prepare_begin,
    frame_probe_prepare_end: crate::system::frame_probe_prepare_end,
    frame_probe_waiting: crate::system::frame_probe_waiting,
    frame_probe_acquired: crate::system::frame_probe_acquired,
    frame_probe_fields: crate::system::frame_probe_fields,
    sink: SINK,
};

/// The C shim's entry (`src/main.c`) and the simulator's (`src/bin/sim.rs`): install the port, then
/// hand over to `app::run_application`.
#[no_mangle]
pub extern "C" fn plex_run(pms_host: *const c_char, pms_port: c_int) -> c_int {
    let _ = plx_platform::tv::install(&PORT);
    #[cfg(target_os = "linux")]
    let _ = plx_platform::storage::client::install_activator(plx_platform::webos::activate_storage_helper);
    if let Some(code) = run_explicit_diagnostic() {
        return code;
    }
    crate::app::run_application(pms_host, pms_port)
}

fn run_explicit_diagnostic() -> Option<c_int> {
    #[cfg(all(feature = "devtriggers", target_os = "linux", target_arch = "arm", not(feature = "hostsim")))]
    if crate::app::ui_snapshot::cold_launch() {
        return None;
    }
    run_explicit_storage_probe()
}

#[derive(Debug, PartialEq, Eq)]
enum StorageProbeLaunch {
    Ordinary,
    Rejected,
    Requested(String),
}

/// A cold SAM launch can request one synthetic diagnostic instead of starting the player.
/// This accepts no key name, URI, credentials, or payload to encrypt.
fn parse_storage_probe_launch(raw: &str) -> StorageProbeLaunch {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Request {
        version: u32,
        request_id: String,
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return StorageProbeLaunch::Ordinary;
    };
    let Some(parameters) = value.get("parameters").and_then(serde_json::Value::as_object) else {
        return StorageProbeLaunch::Ordinary;
    };
    let Some(probe) = parameters.get("ottStorageProbe") else {
        return StorageProbeLaunch::Ordinary;
    };
    if value.get("event").and_then(serde_json::Value::as_str) != Some("launch")
        || parameters.len() != 1
    {
        return StorageProbeLaunch::Rejected;
    }
    let Ok(probe) = serde_json::from_value::<Request>(probe.clone()) else {
        return StorageProbeLaunch::Rejected;
    };
    if probe.version != 1 || probe.request_id.len() != 32
        || !probe.request_id.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return StorageProbeLaunch::Rejected;
    }
    StorageProbeLaunch::Requested(probe.request_id)
}

fn select_storage_probe_launch(payloads: &[String]) -> StorageProbeLaunch {
    let mut selected = StorageProbeLaunch::Ordinary;
    for payload in payloads {
        match parse_storage_probe_launch(payload) {
            StorageProbeLaunch::Ordinary => {},
            StorageProbeLaunch::Rejected => return StorageProbeLaunch::Rejected,
            requested @ StorageProbeLaunch::Requested(_) => {
                if selected != StorageProbeLaunch::Ordinary {
                    return StorageProbeLaunch::Rejected;
                }
                selected = requested;
            }
        }
    }
    selected
}

fn run_explicit_storage_probe() -> Option<c_int> {
    // This runs before SDL/normal application startup, so a warm launch cannot enter this path.
    let payloads = plx_platform::tv::launch::take_all();
    let request = match select_storage_probe_launch(&payloads) {
        StorageProbeLaunch::Ordinary => {
            for payload in payloads {
                plx_platform::tv::launch::offer(&payload);
            }
            return None;
        }
        StorageProbeLaunch::Rejected => {
            plx_base::eventlog::log("storage-canary request=rejected rpc=not_sent cleanup=not_started");
            return Some(2);
        }
        StorageProbeLaunch::Requested(request) => request,
    };
    let pid = std::process::id();
    let started = std::time::Instant::now();
    // No activation, normal Hello, retry, or application bootstrap: the helper must already
    // be available. The client authenticates its peer and private descriptor nonce itself.
    let result = plx_platform::storage::client::probe_keymanager();
    // Cold diagnostic exits immediately: publish the closed result synchronously,
    // without starting the normal diagnostics worker or changing probe/exit policy.
    #[cfg(all(feature = "devtriggers", target_os = "linux", target_arch = "arm", not(feature = "hostsim")))]
    if plx_platform::storage::diagnostics::canary_result(
        env!("PLX_VERSION"), &request, started.elapsed().as_millis(), &result,
    ).is_err() {
        plx_base::eventlog::log(&format!(
            "storage-canary request_id={request} pid={pid} shared_export=unavailable"
        ));
    }
    match result {
        Ok(result) => {
            let complete = result.complete();
            match serde_json::to_string(&result) {
                Ok(evidence) => plx_base::eventlog::log(&format!(
                    "storage-canary request_id={request} pid={pid} elapsed_ms={} result={evidence}",
                    started.elapsed().as_millis(),
                )),
                Err(_) => {
                    plx_base::eventlog::log(&format!(
                        "storage-canary request_id={request} pid={pid} evidence=unavailable cleanup=unconfirmed"
                    ));
                    return Some(2);
                }
            }
            Some(if complete { 0 } else { 2 })
        }
        Err(error) => {
            // ClientError is a closed enum; never log the response or rendezvous descriptor.
            plx_base::eventlog::log(&format!(
                "storage-canary request_id={request} pid={pid} elapsed_ms={} transport={error:?} cleanup=unconfirmed",
                started.elapsed().as_millis(),
            ));
            Some(2)
        }
    }
}

#[cfg(test)]
mod storage_probe_launch_tests {
    use super::{parse_storage_probe_launch, select_storage_probe_launch, StorageProbeLaunch};

    const REQUEST: &str = r#"{"event":"launch","parameters":{"ottStorageProbe":{"version":1,"request_id":"0123456789abcdef0123456789abcdef"}}}"#;

    #[test]
    fn only_explicit_cold_synthetic_request_is_accepted() {
        assert_eq!(parse_storage_probe_launch(REQUEST),
            StorageProbeLaunch::Requested("0123456789abcdef0123456789abcdef".into()));
        for raw in ["{}", "not json", r#"{"parameters":{"plex":{"ratingKey":"1"}}}"#] {
            assert_eq!(parse_storage_probe_launch(raw), StorageProbeLaunch::Ordinary);
        }
        for raw in [
            REQUEST.replace("launch", "relaunch"),
            REQUEST.replace("\"version\":1", "\"version\":2"),
            REQUEST.replace("0123456789abcdef0123456789abcdef", "untrusted\\ntext"),
            REQUEST.replace("\"version\":1", "\"key_name\":\"private-input\",\"version\":1"),
            r#"{"event":"launch","parameters":{"ottStorageProbe":true}}"#.into(),
        ] {
            assert_eq!(parse_storage_probe_launch(&raw), StorageProbeLaunch::Rejected);
        }
    }

    #[test]
    fn duplicate_requests_never_repeat_the_probe() {
        assert_eq!(select_storage_probe_launch(&[REQUEST.into(), REQUEST.into()]),
            StorageProbeLaunch::Rejected);
        assert_eq!(select_storage_probe_launch(&["{}".into()]), StorageProbeLaunch::Ordinary);
    }

    #[test]
    fn sam_metadata_is_allowed_but_mixed_player_actions_are_rejected() {
        let mut value: serde_json::Value = serde_json::from_str(REQUEST).unwrap();
        value["appId"] = serde_json::json!("com.example.installed.app");
        value["interfaceVersion"] = serde_json::json!(2);
        value["reason"] = serde_json::json!("external request");
        assert!(matches!(parse_storage_probe_launch(&value.to_string()),
            StorageProbeLaunch::Requested(_)));
        value["parameters"]["plex"] = serde_json::json!({"ratingKey":"1"});
        assert_eq!(parse_storage_probe_launch(&value.to_string()), StorageProbeLaunch::Rejected);
    }
}

/// `src/main.c` hands over SAM's launch payload (`argv[1]`) before `plex_run`. Kept raw in
/// `tv::launch`; `app::deeplink` reads it once the app has a session to act in.
///
/// # Safety
/// `json` is NULL or a NUL-terminated string that outlives the call.
#[no_mangle]
pub unsafe extern "C" fn plx_launch_args(json: *const c_char) {
    if json.is_null() {
        return;
    }
    let raw = unsafe { std::ffi::CStr::from_ptr(json) }.to_string_lossy();
    #[cfg(feature = "devtriggers")]
    crate::dev::capture_cold_launch_diagnostics(&raw);
    plx_platform::tv::launch::offer(&raw);
}

// Only the explicit development simulator can substitute named OTT protection.
// Inactive calls and every TV build retain the existing keymanager path.
fn seal_named(namespace: plx_platform::tv::secure::Namespace, plain: &[u8])
    -> Result<plx_platform::tv::secure::Sealed, plx_platform::storage::wire::KeymanagerFailure>
{
    #[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
    if plx_platform::tv::secure::fixture::active() {
        return plx_platform::tv::secure::fixture::seal(namespace, plain)
            .ok_or_else(plx_platform::tv::secure::protection_unavailable);
    }
    plx_platform::keymanager::seal_named_detailed(namespace, plain)
}
fn open_named(namespace: plx_platform::tv::secure::Namespace, sealed: &plx_platform::tv::secure::Sealed)
    -> Option<Vec<u8>>
{
    #[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
    if plx_platform::tv::secure::fixture::active() {
        return plx_platform::tv::secure::fixture::open(namespace, sealed);
    }
    plx_platform::keymanager::open_named(namespace, sealed)
}

#[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
pub fn activate_fixture(args: &[String]) -> Result<Option<Vec<String>>, &'static str> {
    plx_platform::tv::secure::fixture::activate_from_args(args)
}

fn removable_snapshot(off: &plx_base::task::OffFrame, check: &mut dyn FnMut() -> bool)
    -> Result<Vec<plx_platform::tv::removable::MountCandidate>, plx_platform::tv::removable::ReadError>
{
    #[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
    if let Some(result) = plx_platform::tv::secure::fixture::local_mounts(off, check) { return result; }
    plx_platform::webos::removable::observe(off, check)
}
fn removable_identities(off: &plx_base::task::OffFrame, mounts: &[plx_platform::tv::removable::MountCandidate], check: &mut dyn FnMut() -> bool)
    -> Result<Vec<plx_platform::tv::removable::identity::IdentityObservation>, plx_platform::tv::removable::identity::IdentityError>
{
    #[cfg(all(feature = "hostsim", feature = "devtriggers", any(target_os = "macos", target_os = "linux")))]
    if let Some(result) = plx_platform::tv::secure::fixture::local_identities(off, mounts, check) { return result; }
    plx_platform::webos::removable_identity::identity_snapshot(off, mounts, check)
}
