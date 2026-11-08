//! Explicit, NONCONFIDENTIAL named storage for synthetic macOS/Linux acceptance only.
//! The parent module excludes this entire adapter from every shipping/TV build.
//! One immutable entrypoint authority is shared with storage workers; no screen or
//! machine consults it. The held lock excludes a second process until process exit.
use super::{Backend, Namespace, Sealed};
use plx_base::{
    b64,
    sha256::{ct_eq, hmac_sha256},
};
use std::{
    ffi::{CString, OsString},
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[path = "fixture_ca.rs"]
mod ca;
#[path = "fixture_local.rs"]
mod local;
#[path = "fixture_plex.rs"]
mod plex;

const PAYLOAD_BYTES: usize = 1024 * 1024;
const MARKER_BYTES: usize = 8192;
const DOMAIN: &[u8] = b"ott-host-fixture-v1\0NONCONFIDENTIAL";
const ASSETS: &[&str] = &[
    "appfont.ttf",
    "appfont-bold.ttf",
    "appfont-cjk.ttf",
    "appfont-emoji.ttf",
    "appfont-arabic.ttf",
    "appfont-arabic-bold.ttf",
    "appfont-hebrew.ttf",
    "appfont-hebrew-bold.ttf",
    "icon.png",
    "largeIcon.png",
    "splash.png",
    "OFL.txt",
    "le-roots.pem",
    "libass-plx.0.dylib",
    #[cfg(target_os = "linux")]
    "libass-plx-host.so.0",
    "libavutil-plx.61.dylib",
    "libavcodec-plx.63.dylib",
    "libavformat-plx.63.dylib",
    "libswscale-plx.10.dylib",
];

struct Context {
    root: PathBuf,
    directory: File,
    records: File,
    _lock: File,
    key: [u8; 32],
    install: String,
    ca: Option<ca::Snapshot>,
    local: Option<local::LocalFixture>,
    plex: Option<plex::Snapshot>,
}
// This is a process-lifetime port authority, installed once before plex_run starts
// workers. Unlike the thread-local unit seam, it also governs asynchronous writes.
static ACTIVE: OnceLock<Context> = OnceLock::new();

pub fn active() -> bool {
    ACTIVE.get().is_some()
}
pub fn records_root() -> Option<PathBuf> {
    ACTIVE.get().map(|c| c.root.join("records"))
}
pub fn check() -> bool {
    ACTIVE.get().is_some_and(|c| c.check().is_ok())
}
/// Optional request-scoped trust for the one explicitly declared loopback origin.
/// An invalid active grant is an error, never permission to retry with weaker TLS.
pub fn control_ca_bundle(url: &str) -> Result<Option<&'static str>, &'static str> {
    ACTIVE.get().map_or(Ok(None), |context| context.ca_bundle(url))
}

pub fn seal(namespace: Namespace, bytes: &[u8]) -> Option<Sealed> {
    ACTIVE.get()?.seal(namespace, bytes)
}
pub fn open(namespace: Namespace, sealed: &Sealed) -> Option<Vec<u8>> {
    ACTIVE.get()?.open(namespace, sealed)
}


/// An explicit synthetic descriptor only; absence never permits an ambient import.
pub fn plex_profile_path() -> Result<Option<PathBuf>, &'static str> {
    ACTIVE.get().map_or(Ok(None), Context::plex_profile_path)
}
/// Validate the actual bytes about to be parsed, as well as the held live authority.
pub fn validate_plex_profile(path: &Path, bytes: &[u8]) -> Result<(), &'static str> {
    ACTIVE.get().ok_or("Plex fixture is not active")?.validate_plex_profile(path, bytes)
}

/// Synthetic observations exist only inside the explicit, held HostFixture authority.
pub fn local_mounts(off: &plx_base::task::OffFrame, check: &mut dyn FnMut() -> bool)
    -> Option<Result<Vec<crate::tv::removable::MountCandidate>, crate::tv::removable::ReadError>>
{
    let context = ACTIVE.get()?;
    let local = context.local.as_ref()?;
    Some(if context.check().is_err() { Err(crate::tv::removable::ReadError::Unavailable) } else { local.mounts(off, check) })
}
pub fn local_identities(off: &plx_base::task::OffFrame, mounts: &[crate::tv::removable::MountCandidate], check: &mut dyn FnMut() -> bool)
    -> Option<Result<Vec<crate::tv::removable::identity::IdentityObservation>, crate::tv::removable::identity::IdentityError>>
{
    let context = ACTIVE.get()?;
    let local = context.local.as_ref()?;
    Some(if context.check().is_err() { Err(crate::tv::removable::identity::IdentityError::Unavailable) } else { local.identities(off, mounts, check) })
}
/// Returns an SDL lifecycle event only for the explicitly activated local fixture.
/// This frame-thread gate is atomic-only; mount/storage workers check held filesystem authority.
pub fn local_lifecycle(token: &str) -> Option<u32> {
    if !matches!(token, "fixture:background" | "fixture:foreground") { return None; }
    let context = ACTIVE.get()?;
    let local = context.local.as_ref()?;
    local.lifecycle(token)
}
pub fn local_lifecycle_queued(event: u32, accepted: bool) {
    if let Some(local) = ACTIVE.get().and_then(|context| context.local.as_ref()) {
        local.lifecycle_queued(event, accepted);
    }
}
/// Passive observation only: never changes, accepts, or dispatches the HTTP body.
pub fn observe_local_control(method: &str, url: &str, body: Option<&str>, reply: Option<(u16, &[u8])>) {
    let Some(context) = ACTIVE.get() else { return; };
    let Some(local) = context.local.as_ref() else { return; };
    match context.ca_bundle(url) {
        Ok(Some(_)) => local.control(method, url, body, reply),
        _ => local.unavailable(),
    }
}

/// Consume only a leading, complete fixture declaration. Inactive calls have no
/// filesystem side effects. The simulator calls this before HUD tools/path caches.
pub fn activate_from_args(args: &[String]) -> Result<Option<Vec<String>>, &'static str> {
    if !args.iter().any(|a| a.starts_with("--fixture-")) {
        return Ok(None);
    }
    if ACTIVE.get().is_some() {
        return Err("fixture already activated");
    }
    let (root, create, ca, local, plex, rest) = parse_args(args)?;
    let (runtime, assets) = validate_environment()?;
    let directory = private_directory(&root)?;
    if overlap(&root, &runtime) || overlap(&root, &assets) || overlap(&runtime, &assets) {
        return Err("fixture, runtime and assets must be disjoint");
    }
    // The host normally offers legacy candidates as well. Never read one in this
    // mode, even if the host has a /media tree from another installation.
    for path in plx_base::paths::session_migration_candidates() {
        if !path.starts_with(&runtime) && !path.starts_with(&assets) {
            match std::fs::symlink_metadata(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err("external session candidate exists or cannot be excluded"),
            }
        }
    }
    let mut context = Context::prepare(root, directory, plx_base::paths::app_id(), create)?;
    if let Some((path, origin)) = ca {
        context.install_ca(&path, origin)?;
    }
    if let Some(path) = local {
        if context.ca.is_none() { return Err("local fixture requires the explicit control CA and origin"); }
        context.local = Some(local::LocalFixture::read(&path, &[&context.root, &runtime, &assets])?);
    }
    if let Some(declaration) = plex {
        context.plex = Some(plex::Snapshot::prepare(declaration, &[&context.root, &runtime, &assets])?);
        context.check()?;
    }
    ACTIVE
        .set(context)
        .map_err(|_| "fixture already activated")?;
    Ok(Some(rest.to_vec()))
}

type ParsedArgs<'a> = (PathBuf, bool, Option<(PathBuf, &'a str)>, Option<PathBuf>, Option<plex::Declaration>, &'a [String]);
fn parse_args(args: &[String]) -> Result<ParsedArgs<'_>, &'static str> {
    if args.len() < 3 || args[0] != "--fixture-state" {
        return Err("invalid fixture arguments");
    }
    let create = match args[2].as_str() {
        "--fixture-create" => true,
        "--fixture-reuse" => false,
        _ => return Err("choose fixture-create or fixture-reuse"),
    };
    let (ca, rest) = if args.get(3).map(String::as_str) == Some("--fixture-ca") {
        if args.get(5).map(String::as_str) != Some("--fixture-origin") || args.len() < 7 {
            return Err("fixture CA and origin must be supplied together");
        }
        ca::validate_origin(&args[6])?;
        (Some((PathBuf::from(&args[4]), args[6].as_str())), &args[7..])
    } else {
        (None, &args[3..])
    };
    let (local, rest) = if rest.first().map(String::as_str) == Some("--fixture-local") {
        (Some(PathBuf::from(rest.get(1).ok_or("local fixture declaration required")?)), &rest[2..])
    } else { (None, rest) };
    let (plex, rest) = if rest.first().map(String::as_str) == Some("--fixture-plex-profile") {
        if rest.get(2).map(String::as_str) != Some("--fixture-plex-sha256") || rest.len() < 4 {
            return Err("Plex fixture profile and SHA256 must be supplied together");
        }
        let remaining = &rest[4..];
        (Some(plex::Declaration::parse(PathBuf::from(&rest[1]), &rest[3], remaining)?), remaining)
    } else { (None, rest) };
    if !rest.is_empty()
        && (rest.len() != 2
            || rest[0] != "127.0.0.1"
            || rest[1].parse::<u16>().ok().filter(|p| *p != 0).is_none())
    {
        return Err("fixture server must be explicit IPv4 loopback and port");
    }
    Ok((PathBuf::from(&args[1]), create, ca, local, plex, rest))
}

fn validate_environment() -> Result<(PathBuf, PathBuf), &'static str> {
    validate_environment_values(&std::env::vars_os().collect::<Vec<_>>())
}
fn validate_environment_values(
    vars: &[(OsString, OsString)],
) -> Result<(PathBuf, PathBuf), &'static str> {
    let value = |name: &str| {
        vars.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    };
    for (name, _) in vars {
        let name = name.to_string_lossy();
        if (name.starts_with("PLXNATIVE_")
            || name.starts_with("OTTPLAY_")
            || name.starts_with("OTTP_")
            || name.starts_with("PLX_"))
            && !matches!(
                name.as_ref(),
                "PLXNATIVE_RUNTIME_DIR" | "PLXNATIVE_APP_DIR" | "PLXNATIVE_WIN" | "PLXNATIVE_SHOT"
            )
        {
            return Err("unexpected fixture environment override");
        }
    }
    let runtime = PathBuf::from(value("PLXNATIVE_RUNTIME_DIR").ok_or("explicit runtime required")?);
    let assets = PathBuf::from(value("PLXNATIVE_APP_DIR").ok_or("explicit assets required")?);
    let _runtime = private_directory(&runtime)?;
    let _assets = private_directory(&assets)?;
    let mut clock_sink = false;
    let mut sim_video = false;
    for entry in std::fs::read_dir(&runtime).map_err(|_| "cannot read runtime")? {
        let entry = entry.map_err(|_| "cannot read runtime")?;
        // Only these explicit empty markers are safe before synthetic host boot.
        // acb_init queries the host sink at boot, so clocksink cannot be armed later.
        // The enclosing HostFixture cfg keeps this out of shipping/device builds.
        let name = match entry.file_name().to_str() {
            Some("plxnative-focus") => "plxnative-focus",
            Some("plxnative-clocksink") => "plxnative-clocksink",
            Some("plxnative-simvideo") => "plxnative-simvideo",
            _ => return Err("runtime must be fresh"),
        };
        let file = open_file_at(&_runtime, name, false)?;
        if file.metadata().map_err(|_| "invalid runtime marker")?.len() != 0 {
            return Err("invalid runtime marker");
        }
        clock_sink |= name == "plxnative-clocksink";
        sim_video |= name == "plxnative-simvideo";
    }
    // The existing picture renderer consumes only AUs accepted by the host clock sink.
    if sim_video && !clock_sink {
        return Err("simvideo requires clocksink");
    }
    if let Some(shot) = value("PLXNATIVE_SHOT") {
        if PathBuf::from(shot) != runtime.join("capture.png") {
            return Err("shot must stay in fresh runtime");
        }
    }
    if let Some(window) = value("PLXNATIVE_WIN") {
        let window = window.to_str().ok_or("invalid fixture window")?;
        let Some((w, h)) = window.split_once('x') else {
            return Err("invalid fixture window");
        };
        if [w, h].iter().any(|n| {
            n.parse::<u32>()
                .ok()
                .filter(|v| (1..=8192).contains(v))
                .is_none()
        }) {
            return Err("invalid fixture window");
        }
    }
    for entry in std::fs::read_dir(&assets).map_err(|_| "cannot read assets")? {
        let entry = entry.map_err(|_| "cannot read assets")?;
        let name = entry.file_name();
        let name = name.to_str().ok_or("invalid asset name")?;
        if !ASSETS.contains(&name) {
            return Err("asset directory contains non-asset state");
        }
        let _file = checked_file_at(&_assets, name, false, false)?;
    }
    Ok((runtime, assets))
}

fn overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

fn private_directory(path: &Path) -> Result<File, &'static str> {
    if !path.is_absolute() || std::fs::canonicalize(path).map_err(|_| "invalid directory")? != path
    {
        return Err("directory must be canonical and contain no symlinks");
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| "cannot open private directory")?;
    let meta = file.metadata().map_err(|_| "invalid directory metadata")?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o777 != 0o700 {
        return Err("directory must be owned by this user with mode 0700");
    }
    same_directory(&file, path)?;
    Ok(file)
}
fn same_directory(file: &File, path: &Path) -> Result<(), &'static str> {
    if std::fs::canonicalize(path).map_err(|_| "directory unavailable")? != path {
        return Err("directory path changed");
    }
    let held = file.metadata().map_err(|_| "directory unavailable")?;
    let live = std::fs::symlink_metadata(path).map_err(|_| "directory unavailable")?;
    if !live.is_dir()
        || held.dev() != live.dev()
        || held.ino() != live.ino()
        || live.uid() != unsafe { libc::geteuid() }
        || live.mode() & 0o777 != 0o700
    {
        return Err("directory replaced or permissions changed");
    }
    Ok(())
}
fn open_file_at(directory: &File, name: &str, create: bool) -> Result<File, &'static str> {
    checked_file_at(directory, name, create, true)
}
fn checked_file_at(
    directory: &File,
    name: &str,
    create: bool,
    private: bool,
) -> Result<File, &'static str> {
    let name = CString::new(name).map_err(|_| "invalid fixture filename")?;
    let flags = libc::O_NOFOLLOW
        | libc::O_CLOEXEC
        | libc::O_NONBLOCK
        | if create {
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL
        } else {
            libc::O_RDONLY
        };
    let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags, 0o600) };
    if fd < 0 {
        return Err("cannot open private fixture file");
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let meta = file.metadata().map_err(|_| "invalid fixture metadata")?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || (private && meta.mode() & 0o777 != 0o600)
        || meta.mode() & 0o022 != 0
        || meta.nlink() != 1
    {
        return Err("fixture file must be private, regular and unlinked elsewhere");
    }
    Ok(file)
}
fn read_bounded(mut file: File, limit: usize) -> Result<Vec<u8>, &'static str> {
    if file
        .metadata()
        .map_err(|_| "invalid fixture metadata")?
        .len()
        > limit as u64
    {
        return Err("oversized fixture file");
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read fixture")?;
    if bytes.len() > limit {
        return Err("oversized fixture file");
    }
    Ok(bytes)
}
fn frame(parts: &[&[u8]]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for part in parts {
        bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
        bytes.extend_from_slice(part);
    }
    bytes
}

impl Context {
    fn prepare(
        root: PathBuf,
        directory: File,
        install: &str,
        create: bool,
    ) -> Result<Self, &'static str> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&root).map_err(|_| "cannot read fixture root")? {
            let name = entry.map_err(|_| "cannot read fixture root")?.file_name();
            names.push(name.into_string().map_err(|_| "invalid fixture filename")?);
        }
        if create && !names.is_empty() {
            return Err("fresh fixture root must be empty");
        }
        if !create
            && (names
                .iter()
                .any(|n| !["lock", "key", "marker", "records", ca::NAME].contains(&n.as_str()))
                || ["lock", "key", "marker"]
                    .iter()
                    .any(|n| !names.iter().any(|v| v == *n)))
        {
            return Err("not a complete fixture root");
        }
        let lock = open_file_at(&directory, "lock", create)?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("fixture already in use");
        }
        let mut key = [0u8; 32];
        if create {
            File::open("/dev/urandom")
                .and_then(|mut f| f.read_exact(&mut key))
                .map_err(|_| "entropy unavailable")?;
            let mut file = open_file_at(&directory, "key", true)?;
            file.write_all(&key)
                .and_then(|_| file.sync_all())
                .map_err(|_| "cannot persist fixture key")?;
        } else {
            let bytes = read_bounded(open_file_at(&directory, "key", false)?, 32)?;
            if bytes.len() != 32 {
                return Err("invalid fixture key");
            }
            key.copy_from_slice(&bytes);
        }
        if create {
            let name = CString::new("records").expect("literal");
            if unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                return Err("cannot create fixture records");
            }
        }
        let records = private_directory(&root.join("records"))?;
        let context = Self {
            root,
            directory,
            records,
            _lock: lock,
            key,
            install: install.to_owned(),
            ca: None,
            local: None,
            plex: None,
        };
        let marker = context.marker()?;
        if create {
            let mut file = open_file_at(&context.directory, "marker", true)?;
            file.write_all(&marker)
                .and_then(|_| file.sync_all())
                .map_err(|_| "cannot persist fixture marker")?;
            context
                .directory
                .sync_all()
                .map_err(|_| "cannot sync fixture root")?;
            // The caller created the empty root; persist its directory entry too.
            File::open(context.root.parent().ok_or("invalid root")?)
                .map_err(|_| "cannot open fixture parent")?
                .sync_all()
                .map_err(|_| "cannot sync fixture parent")?;
        } else if !ct_eq(
            &read_bounded(
                open_file_at(&context.directory, "marker", false)?,
                MARKER_BYTES,
            )?,
            &marker,
        ) {
            return Err("fixture marker does not match root, installation or key");
        }
        context.check()?;
        // exit_now uses _exit: a previous process can leave its public CA copy.
        // Only the locked, authenticated owner may remove it; it never restores trust.
        if names.iter().any(|name| name == ca::NAME) {
            ca::remove_residual(&context.directory)?;
        }
        Ok(context)
    }
    fn plex_profile_path(&self) -> Result<Option<PathBuf>, &'static str> {
        self.check()?;
        let path = self.plex.as_ref().map(plex::Snapshot::path).transpose()?;
        self.check()?;
        Ok(path)
    }
    fn validate_plex_profile(&self, path: &Path, bytes: &[u8]) -> Result<(), &'static str> {
        self.check()?;
        self.plex.as_ref().ok_or("Plex fixture was not declared")?.validate(path, bytes)?;
        self.check()
    }
    fn install_ca(&mut self, path: &Path, origin: &str) -> Result<(), &'static str> {
        self.check()?;
        if self.ca.is_some() {
            return Err("fixture CA already installed");
        }
        self.ca = Some(ca::Snapshot::prepare(&self.root, &self.directory, path, origin)?);
        Ok(())
    }
    fn ca_bundle(&self, url: &str) -> Result<Option<&str>, &'static str> {
        let Some(ca) = self.ca.as_ref().filter(|ca| ca.matches(url)) else {
            return Ok(None);
        };
        self.check()?;
        ca.bundle().map(Some)
    }
    fn marker(&self) -> Result<Vec<u8>, &'static str> {
        let root = self.root.to_str().ok_or("fixture root must be UTF-8")?;
        let mut bytes = frame(&[DOMAIN, b"marker", root.as_bytes(), self.install.as_bytes()]);
        bytes.extend_from_slice(&hmac_sha256(&self.key, &bytes));
        if bytes.len() > MARKER_BYTES {
            return Err("fixture identity too long");
        }
        Ok(bytes)
    }
    fn check(&self) -> Result<(), &'static str> {
        same_directory(&self.directory, &self.root)?;
        same_directory(&self.records, &self.root.join("records"))?;
        let key = read_bounded(open_file_at(&self.directory, "key", false)?, 32)?;
        let marker = read_bounded(
            open_file_at(&self.directory, "marker", false)?,
            MARKER_BYTES,
        )?;
        if !ct_eq(&key, &self.key) || !ct_eq(&marker, &self.marker()?) {
            return Err("fixture authority changed");
        }
        let held = self
            ._lock
            .metadata()
            .map_err(|_| "fixture lock unavailable")?;
        let live = open_file_at(&self.directory, "lock", false)?
            .metadata()
            .map_err(|_| "fixture lock unavailable")?;
        if held.dev() != live.dev() || held.ino() != live.ino() {
            return Err("fixture lock replaced");
        }
        Ok(())
    }
    fn tag(&self, namespace: Namespace, bytes: &[u8]) -> Option<[u8; 32]> {
        let root = self.root.to_str()?;
        Some(hmac_sha256(
            &self.key,
            &frame(&[
                DOMAIN,
                b"envelope",
                b"1",
                b"host_fixture",
                root.as_bytes(),
                self.install.as_bytes(),
                namespace.key_name().as_bytes(),
                bytes,
            ]),
        ))
    }
    fn seal(&self, namespace: Namespace, bytes: &[u8]) -> Option<Sealed> {
        if bytes.len() > PAYLOAD_BYTES || self.check().is_err() {
            return None;
        }
        let sealed = Sealed {
            backend: Backend::HostFixture,
            key: namespace.key_name(),
            iv: b64::encode(&self.tag(namespace, bytes)?),
            data: b64::encode(bytes),
        };
        self.check().ok()?;
        Some(sealed)
    }
    fn open(&self, namespace: Namespace, sealed: &Sealed) -> Option<Vec<u8>> {
        if sealed.backend != Backend::HostFixture
            || sealed.key != namespace.key_name()
            || sealed.data.len() > PAYLOAD_BYTES.div_ceil(3) * 4
            || sealed.iv.len() != 44
            || self.check().is_err()
        {
            return None;
        }
        let bytes = b64::decode(&sealed.data)?;
        let tag = b64::decode(&sealed.iv)?;
        if b64::encode(&bytes) != sealed.data || b64::encode(&tag) != sealed.iv {
            return None;
        }
        // Authenticate opaque bytes before storage::private parses the inner JSON.
        if bytes.len() > PAYLOAD_BYTES || !ct_eq(&tag, &self.tag(namespace, &bytes)?) {
            return None;
        }
        self.check().ok()?;
        Some(bytes)
    }
}

#[cfg(test)]
#[path = "fixture_tests.rs"]
mod tests;
