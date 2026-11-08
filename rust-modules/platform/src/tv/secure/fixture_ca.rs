//! Process-lifetime trust input for the explicitly enabled HostFixture adapter.
//! Included only by the macOS + hostsim + devtriggers module; never persisted as authority.
use super::{checked_file_at, read_bounded};
use plx_base::sha256::sha256;
use std::{
    ffi::CString,
    fs::{File, OpenOptions, Permissions},
    io::Write,
    os::{fd::AsRawFd, unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt}},
    path::Path,
};

pub(super) const NAME: &str = "control-ca.pem";
const MAX_CA_BYTES: usize = 256 * 1024;

pub(super) fn validate_origin(origin: &str) -> Result<(), &'static str> {
    let port = origin.strip_prefix("https://127.0.0.1:")
        .ok_or("fixture CA origin must be exact HTTPS IPv4 loopback and port")?;
    let number = port.parse::<u16>().ok().filter(|number| *number != 0)
        .ok_or("invalid fixture CA port")?;
    if port != number.to_string() {
        return Err("fixture CA origin must be canonical and contain no path");
    }
    Ok(())
}

fn unlink(directory: &File) -> Result<(), &'static str> {
    let name = CString::new(NAME).expect("literal");
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err("cannot remove fixture CA snapshot");
    }
    Ok(())
}

/// Called only after the existing root lock and authenticated marker are checked.
/// A stopped/aborted process may leave its CA copy. It is public data, not a new grant.
pub(super) fn remove_residual(directory: &File) -> Result<(), &'static str> {
    // Accept an interrupted 0600 copy as well as the finished 0400 snapshot.
    let file = checked_file_at(directory, NAME, false, false)?;
    if !matches!(file.metadata().map_err(|_| "invalid CA snapshot")?.mode() & 0o777, 0o400 | 0o600) {
        return Err("invalid residual CA permissions");
    }
    unlink(directory)
}

pub(super) struct Snapshot {
    origin: String,
    path: String,
    directory: File,
    file: File,
    digest: [u8; 32],
}

impl Snapshot {
    pub(super) fn prepare(root: &Path, directory: &File, input: &Path, origin: &str)
        -> Result<Self, &'static str>
    {
        validate_origin(origin)?;
        if input.starts_with(root) || !input.is_absolute()
            || std::fs::canonicalize(input).map_err(|_| "cannot resolve fixture CA")? != input
        {
            return Err("fixture CA must be outside the state root, canonical and contain no symlinks");
        }
        let source = OpenOptions::new().read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(input).map_err(|_| "cannot open fixture CA")?;
        let meta = source.metadata().map_err(|_| "invalid fixture CA metadata")?;
        if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o022 != 0 || meta.nlink() != 1
        {
            return Err("fixture CA must be owned, regular and unlinked elsewhere");
        }
        let bytes = read_bounded(source, MAX_CA_BYTES)?;
        let pem = std::str::from_utf8(&bytes).map_err(|_| "fixture CA must be PEM")?;
        // Check framing only; libcurl's TLS backend validates the certificates,
        // their chains, hostnames and dates. Never reinterpret a key as a CA.
        if pem.contains('\0') || !pem.trim().starts_with("-----BEGIN CERTIFICATE-----")
            || !pem.trim().ends_with("-----END CERTIFICATE-----")
            || pem.contains("PRIVATE KEY")
        {
            return Err("fixture CA must contain public PEM certificates");
        }
        let path = root.join(NAME).to_str().ok_or("fixture CA path must be UTF-8")?.to_owned();
        let owned_directory = directory.try_clone().map_err(|_| "cannot retain CA directory")?;
        let mut output = checked_file_at(directory, NAME, true, true)?;
        let result = (|| {
            output.write_all(&bytes).and_then(|_| output.sync_all())
                .map_err(|_| "cannot copy fixture CA")?;
            output.set_permissions(Permissions::from_mode(0o400))
                .map_err(|_| "cannot seal fixture CA copy")?;
            let file = checked_file_at(directory, NAME, false, false)?;
            let snapshot = Self {
                origin: origin.to_owned(), path, directory: owned_directory, file,
                digest: sha256(&bytes),
            };
            snapshot.bundle()?;
            Ok(snapshot)
        })();
        if result.is_err() { let _ = unlink(directory); }
        result
    }

    pub(super) fn matches(&self, url: &str) -> bool {
        // Slash boundary excludes another port, credentials and alternate authorities.
        // Remote protocol builds canonical URLs; reject controls/backslashes rather
        // than relying on curl's URL normalization to agree with this comparison.
        !url.bytes().any(|byte| byte.is_ascii_control() || byte == b'\\' || byte == b' ')
            && url.strip_prefix(&self.origin).is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('/'))
    }

    pub(super) fn bundle(&self) -> Result<&str, &'static str> {
        let live = checked_file_at(&self.directory, NAME, false, false)?;
        let held_meta = self.file.metadata().map_err(|_| "fixture CA unavailable")?;
        let live_meta = live.metadata().map_err(|_| "fixture CA unavailable")?;
        if live_meta.mode() & 0o777 != 0o400 || held_meta.dev() != live_meta.dev()
            || held_meta.ino() != live_meta.ino()
            || sha256(&read_bounded(live, MAX_CA_BYTES)?) != self.digest
        {
            return Err("fixture CA snapshot changed");
        }
        Ok(&self.path)
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        // Never remove a substituted file. _exit deliberately skips destructors;
        // the next explicit fixture reuse removes a residual under the root lock.
        if let (Ok(live), Ok(held)) = (
            checked_file_at(&self.directory, NAME, false, false).and_then(|file|
                file.metadata().map_err(|_| "CA unavailable")),
            self.file.metadata(),
        ) {
            if live.dev() == held.dev() && live.ino() == held.ino() {
                let _ = unlink(&self.directory);
            }
        }
    }
}
