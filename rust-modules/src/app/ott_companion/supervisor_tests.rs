//! Real host child/process and filesystem tests. These do not establish webOS jail behavior.
use super::super::protocol;
use super::*;
use std::os::unix::fs::symlink;

fn endpoint() -> Endpoint {
    Endpoint::new(8793, "supervisor-fixture-0123456789abcdef".into()).unwrap()
}

fn spawn<'a>(worker: &'a OffFrame, command: &mut Command) -> Process<'a> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("host process fixture must start");
    let stderr = child.stderr.take().unwrap();
    let output = match Output::new(stderr) {
        Ok(output) => output,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("could not read host fixture output: {error}");
        }
    };
    Process {
        worker,
        child,
        output,
        client: Client::new(endpoint()),
    }
}

fn assert_reaped(pid: libc::pid_t) {
    let mut status = 0;
    // A zombie would return its PID here. ECHILD proves Process already waited;
    // checking kill(pid, 0) alone would not distinguish a live child from a zombie.
    let result = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    assert_eq!(result, -1, "child must have been terminated and reaped");
    assert_eq!(
        io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}

#[test]
fn normal_drop_terminates_and_reaps_real_child_promptly() {
    let mut command = Command::new("/bin/sleep");
    command.arg("30");
    let worker = OffFrame::for_test();
    let process = spawn(&worker, &mut command);
    let pid = process.child.id() as libc::pid_t;
    let budget = Budget::start(&worker, Duration::from_secs(10));
    drop(process);
    assert_reaped(pid);
    assert!(
        budget.spent(&worker) < Duration::from_secs(2),
        "cooperative child should not need kill escalation"
    );
}

#[test]
fn drop_escalates_when_child_inherits_ignored_sigterm() {
    let mut command = Command::new("/bin/sh");
    // exec preserves SIG_IGN and avoids leaving a shell's sleep grandchild.
    // The readiness message is emitted only after the ignore disposition exists.
    command.args(["-c", "trap '' TERM; printf 'ottplay-companion: http://127.0.0.1:8793\\n' >&2; exec /bin/sleep 30"]);
    let worker = OffFrame::for_test();
    let mut process = spawn(&worker, &mut command);
    let pid = process.child.id() as libc::pid_t;
    let readiness = Budget::start(&worker, Duration::from_secs(2));
    loop {
        if process.output.drain() == Some(8793) {
            break;
        }
        assert!(
            !readiness.expired(&worker),
            "child never installed the signal fixture"
        );
        assert!(matches!(process.child.try_wait(), Ok(None)));
        readiness.wait(
            &worker,
            Duration::from_millis(10),
            Duration::from_millis(10),
            &|| false,
        );
    }
    let budget = Budget::start(&worker, Duration::from_secs(10));
    drop(process);
    assert_reaped(pid);
    assert!(
        budget.spent(&worker) >= Duration::from_secs(3),
        "fixture must exercise escalation rather than immediate SIGTERM"
    );
    assert!(
        budget.spent(&worker) < Duration::from_secs(6),
        "uncooperative child shutdown must be bounded"
    );
}

#[test]
fn already_exited_child_is_reaped_without_waiting_for_grace_period() {
    let mut command = Command::new("/bin/sleep");
    command.arg("0");
    let worker = OffFrame::for_test();
    let mut process = spawn(&worker, &mut command);
    let pid = process.child.id() as libc::pid_t;
    assert!(process.child.wait().unwrap().success());
    let budget = Budget::start(&worker, Duration::from_secs(10));
    drop(process);
    assert_reaped(pid);
    assert!(budget.spent(&worker) < Duration::from_secs(1));
}

#[test]
fn private_directory_preserves_existing_data_and_has_no_probe_residue() {
    let base = plx_base::testscratch::process_dir("companion-private-preserve");
    let root = base.join("companion");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
    fs::write(root.join("operator-state"), b"keep this").unwrap();
    fs::write(root.join(".write-probe"), b"older crashed probe").unwrap();
    for _ in 0..2 {
        private_directory(&root).unwrap();
    }
    assert_eq!(fs::metadata(&root).unwrap().mode() & 0o777, 0o700);
    assert_eq!(fs::read(root.join("operator-state")).unwrap(), b"keep this");
    assert_eq!(
        fs::read(root.join(".write-probe")).unwrap(),
        b"older crashed probe"
    );
    assert_eq!(
        fs::read_dir(&root).unwrap().count(),
        2,
        "new probes must be removed"
    );
}

#[test]
fn private_directory_rejects_root_and_parent_symlinks_without_touching_target() {
    let base = plx_base::testscratch::process_dir("companion-private-symlink");
    let target = base.join("target");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(target.join("keep"), b"untouched").unwrap();
    let linked_root = base.join("linked-root");
    symlink(&target, &linked_root).unwrap();
    assert_eq!(
        private_directory(&linked_root),
        Err(Error("companion_invalid_directory"))
    );
    assert_eq!(
        private_directory(&linked_root.join("child")),
        Err(Error("companion_invalid_directory"))
    );
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o755);
    assert_eq!(fs::read(target.join("keep")).unwrap(), b"untouched");
    assert_eq!(fs::read_dir(&target).unwrap().count(), 1);
}

#[test]
fn binary_path_rejects_links_writable_executables_and_non_executables() {
    let base = plx_base::testscratch::process_dir("companion-binary-permissions");
    let binary = base.join("helper");
    fs::write(&binary, b"fixture, not an ELF acceptance test").unwrap();
    for mode in [0o644, 0o775, 0o757] {
        fs::set_permissions(&binary, fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(
            valid_binary(&binary),
            Err(Error("companion_invalid_binary"))
        );
    }
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(valid_binary(&binary).is_ok());
    let link = base.join("link");
    symlink(&binary, &link).unwrap();
    assert_eq!(valid_binary(&link), Err(Error("companion_invalid_binary")));
}

fn handle() -> Supervisor {
    let (tx, _) = mpsc::sync_channel(REQUEST_CAPACITY);
    let (_, rx) = mpsc::sync_channel(REQUEST_CAPACITY);
    Supervisor {
        shared: Arc::new(Shared {
            desired: Mutex::new(Configuration::default()),
            snapshot: Mutex::new(Snapshot {
                installed: true,
                ready: true,
                torrent: true,
                local_torznab: true,
                plugins: true,
                state: "ready",
                ..Snapshot::default()
            }),
            endpoint: Mutex::new(Some(endpoint())),
            leases: Mutex::new(BTreeMap::from([("lease-1".into(), 1)])),
            live_generation: AtomicU64::new(1),
            lifecycle: AtomicU64::new(1),
            media: AtomicU64::new(1),
            shutdown: AtomicBool::new(false),
        }),
        tx,
        rx,
        pending: BTreeSet::new(),
        next_id: 1,
        observed: Snapshot::default(),
        worker: None,
        data_root: None,
    }
}

fn erasure_root(tag: &str) -> (PathBuf, PathBuf) {
    // /var is a symlink on macOS. The fixture deliberately passes the owned,
    // canonical directory, just as the no-follow eraser requires in production.
    let base = plx_base::testscratch::process_dir(tag)
        .canonicalize()
        .unwrap();
    let root = base.join("ott-companion");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    (base, root)
}

fn finish_erasure(stop: &mut ErasureStop) -> Result<(), Error> {
    let off = OffFrame::for_test();
    let budget = Budget::start(&off, Duration::from_secs(3));
    loop {
        let result = stop.finish(budget.remaining(&off));
        if result.is_ok() || budget.expired(&off) {
            return result;
        }
        budget.wait(&off, POLL_FOR_ERASURE_TEST, POLL_FOR_ERASURE_TEST, &|| {
            false
        });
    }
}
const POLL_FOR_ERASURE_TEST: Duration = Duration::from_millis(5);

#[test]
fn erasure_revokes_producers_and_removes_only_owned_helper_data() {
    let (base, root) = erasure_root("companion-erasure-scope");
    let outside = base.join("original-media");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("film.mp4"), b"original media").unwrap();
    fs::create_dir(root.join("torznab-private")).unwrap();
    fs::write(
        root.join("torznab-private/state.json"),
        b"synthetic credentials",
    )
    .unwrap();
    fs::create_dir(root.join("torrent-cache")).unwrap();
    fs::write(root.join("torrent-cache/lease-fixture.bin"), b"pieces").unwrap();
    symlink(&outside, root.join("external-directory")).unwrap();
    symlink(outside.join("film.mp4"), root.join("external-file")).unwrap();

    let mut owner = handle();
    owner.data_root = Some(root.clone());
    let mut stop = owner.retire_for_erasure();
    assert!(owner.shared.shutdown.load(Ordering::SeqCst));
    assert!(!owner.snapshot().ready);
    assert!(lock(&owner.shared.endpoint).is_none());
    assert!(owner.configure(Configuration::default()).is_err());
    assert!(owner.start().is_err());
    assert!(
        root.exists(),
        "retirement on the frame thread must not run disk cleanup"
    );
    assert_eq!(finish_erasure(&mut stop), Ok(()));
    assert!(!root.exists());
    assert_eq!(
        fs::read(outside.join("film.mp4")).unwrap(),
        b"original media"
    );
    assert_eq!(
        stop.finish(Duration::ZERO),
        Ok(()),
        "completed erasure is idempotent"
    );
}

#[test]
fn erasure_timeout_keeps_root_until_the_old_worker_finishes() {
    let (_base, root) = erasure_root("companion-erasure-timeout");
    fs::write(root.join("old-data"), b"keep until stopped").unwrap();
    let (release, held) = mpsc::channel();
    let (ready, started) = mpsc::channel();
    let late_root = root.clone();
    let mut owner = handle();
    owner.data_root = Some(root.clone());
    owner.worker = plx_base::task::spawn_off_frame_keeping("erasure-held-fixture", move |_| {
        let _ = ready.send(());
        if held.recv_timeout(Duration::from_secs(5)).is_ok() {
            fs::write(late_root.join("last-worker-write"), b"last write").unwrap();
        }
    });
    assert!(owner.worker.is_some());
    started.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut stop = owner.retire_for_erasure();
    let off = OffFrame::for_test();
    let bound = Budget::start(&off, Duration::from_secs(1));
    assert!(stop.finish(Duration::from_millis(10)).is_err());
    assert!(bound.spent(&off) < Duration::from_millis(500));
    assert_eq!(
        fs::read(root.join("old-data")).unwrap(),
        b"keep until stopped"
    );
    release.send(()).unwrap();
    assert_eq!(finish_erasure(&mut stop), Ok(()));
    assert!(
        !root.exists(),
        "the last worker write must precede deletion"
    );
}

#[test]
fn erasure_handles_no_worker_and_an_already_finished_worker() {
    let mut absent = Supervisor::inert_for_test().retire_for_erasure();
    assert_eq!(absent.finish(Duration::ZERO), Ok(()));
    let (_base, root) = erasure_root("companion-erasure-finished");
    let mut owner = handle();
    owner.data_root = Some(root.clone());
    owner.worker = plx_base::task::spawn_off_frame_keeping("erasure-finished-fixture", |_| {});
    assert!(owner.worker.is_some());
    let off = OffFrame::for_test();
    let budget = Budget::start(&off, Duration::from_secs(1));
    while !owner.worker.as_ref().unwrap().is_finished() {
        assert!(
            budget.wait(&off, POLL_FOR_ERASURE_TEST, POLL_FOR_ERASURE_TEST, &|| {
                false
            })
        );
    }
    let mut stop = owner.retire_for_erasure();
    assert_eq!(finish_erasure(&mut stop), Ok(()));
    assert!(!root.exists());
}

#[test]
fn erasure_before_runtime_construction_clears_existing_owned_data() {
    let (_base, root) = erasure_root("companion-erasure-no-runtime");
    fs::create_dir(root.join("torznab-private")).unwrap();
    fs::write(root.join("torznab-private/state.json"), b"old local source").unwrap();
    let mut stop = ErasureStop::owned_data_without_worker(root.clone());
    assert!(
        root.exists(),
        "constructing the erasure owner performs no disk IO"
    );
    assert_eq!(finish_erasure(&mut stop), Ok(()));
    assert!(!root.exists());
}

#[test]
fn erasure_refuses_a_linked_root_and_a_nonhelper_root() {
    let (base, root) = erasure_root("companion-erasure-root-link");
    fs::remove_dir(&root).unwrap();
    let outside = base.join("originals");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("film"), b"original").unwrap();
    symlink(&outside, &root).unwrap();
    for path in [&root, &outside] {
        let mut owner = handle();
        owner.data_root = Some(path.clone());
        let mut stop = owner.retire_for_erasure();
        assert_eq!(
            stop.finish(Duration::from_secs(1)),
            Err(Error("companion_erasure_unsafe"))
        );
        assert_eq!(fs::read(outside.join("film")).unwrap(), b"original");
    }
    assert!(fs::symlink_metadata(root).unwrap().file_type().is_symlink());
}

fn session() -> Session {
    let wire = serde_json::json!({
        "id":"lease-1", "state":"ready", "files":[{"index":0,"name":"fixture.mp4","length":4096}],
        "selected":0, "streamPath":"sessions/lease-1/stream", "downloaded":128, "total":4096
    });
    match protocol::decode(
        &endpoint(),
        &Request::Session {
            id: "lease-1".into(),
        },
        200,
        &serde_json::to_vec(&wire).unwrap(),
    )
    .unwrap()
    {
        Reply::Session(session) => session,
        _ => panic!("session fixture returned another reply kind"),
    }
}

#[test]
fn generation_change_hides_capabilities_before_worker_observes_change() {
    let handle = handle();
    assert!(handle.snapshot().ready);
    handle.shared.lifecycle.store(2, Ordering::SeqCst);
    let snapshot = handle.snapshot();
    assert!(!snapshot.ready && !snapshot.torrent && !snapshot.local_torznab && !snapshot.plugins);
    assert!(
        snapshot.installed,
        "installed binary and live engine are separate facts"
    );
    assert!(handle.stream_url(&session()).is_err());
}

#[test]
fn media_cancellation_revokes_previously_issued_session_capability() {
    let mut handle = handle();
    let session = session();
    assert!(handle.stream_url(&session).is_ok());
    handle.cancel_media();
    assert_eq!(
        handle.stream_url(&session),
        Err(Error("companion_cancelled"))
    );
    lock(&handle.shared.leases).insert(session.id.clone(), 1);
    assert_eq!(
        handle.stream_url(&session),
        Err(Error("companion_cancelled")),
        "late previous-epoch lease cannot revive playback"
    );
}

#[test]
fn changed_endpoint_cannot_reuse_previous_session_even_with_matching_id() {
    let handle = handle();
    let session = session();
    *lock(&handle.shared.endpoint) =
        Some(Endpoint::new(8794, "next-fixture-token-0123456789abcdef".into()).unwrap());
    assert_eq!(
        handle.stream_url(&session),
        Err(Error("companion_receipt_invalid"))
    );
}
