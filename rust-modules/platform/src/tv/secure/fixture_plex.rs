//! Explicit synthetic PMS descriptor for the existing managed import path.
//! This is input validation only: it never publishes credentials, a grant or a session.
use super::{checked_file_at, overlap, private_directory, read_bounded, same_directory};
use plx_base::sha256::sha256;
use serde::Deserialize;
use std::{fs::File, os::unix::fs::MetadataExt, path::{Path, PathBuf}};

const PROFILE_BYTES: usize = 4096;

pub(super) struct Declaration {
    pub path: PathBuf,
    pub digest: [u8; 32],
    port: u16,
}
impl Declaration {
    pub(super) fn parse(path: PathBuf, digest: &str, server: &[String]) -> Result<Self, &'static str> {
        if server.len() != 2 || server[0] != "127.0.0.1" {
            return Err("Plex fixture requires its explicit IPv4 loopback server");
        }
        let port = server[1].parse::<u16>().ok().filter(|port| *port != 0)
            .ok_or("invalid Plex fixture server port")?;
        if server[1] != port.to_string() {
            return Err("Plex fixture server port must be canonical");
        }
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err("Plex fixture SHA256 must be 64 lowercase hexadecimal digits");
        }
        let mut decoded = [0u8; 32];
        for (out, pair) in decoded.iter_mut().zip(digest.as_bytes().chunks_exact(2)) {
            let nibble = |byte: u8| if byte.is_ascii_digit() { byte - b'0' } else { byte - b'a' + 10 };
            *out = nibble(pair[0]) * 16 + nibble(pair[1]);
        }
        Ok(Self { path, digest: decoded, port })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Profile {
    base_url: String,
    token: String,
    server_id: String,
    name: String,
    allow_insecure_http: bool,
}
fn validate_bytes(bytes: &[u8], digest: &[u8; 32], port: u16) -> Result<(), &'static str> {
    if bytes.len() > PROFILE_BYTES || sha256(bytes) != *digest {
        return Err("Plex fixture bytes do not match the declaration");
    }
    let profile: Profile = serde_json::from_slice(bytes).map_err(|_| "invalid Plex fixture schema")?;
    if profile.base_url != format!("http://127.0.0.1:{port}")
        || profile.token != "ott-fixture-plex-token" || profile.server_id != "ott-fixture-plex"
        || profile.name != "Fixture Plex" || !profile.allow_insecure_http
    {
        return Err("Plex fixture must use only the declared synthetic server");
    }
    Ok(())
}

pub(super) struct Snapshot {
    declaration: Declaration,
    parent: PathBuf,
    directory: File,
    file: File,
}
impl Snapshot {
    pub(super) fn prepare(declaration: Declaration, excluded: &[&Path]) -> Result<Self, &'static str> {
        let path = &declaration.path;
        if !path.is_absolute() || std::fs::canonicalize(path).map_err(|_| "Plex fixture unavailable")? != *path {
            return Err("Plex fixture path must be canonical without symlinks");
        }
        let parent = path.parent().ok_or("Plex fixture has no parent")?.to_owned();
        if excluded.iter().any(|other| overlap(&parent, other)) {
            return Err("Plex fixture parent must be disjoint from state, runtime and assets");
        }
        let directory = private_directory(&parent)?;
        let name = path.file_name().and_then(|name| name.to_str()).ok_or("invalid Plex fixture filename")?;
        let file = checked_file_at(&directory, name, false, true)?;
        let snapshot = Self { declaration, parent, directory, file };
        snapshot.current()?;
        Ok(snapshot)
    }
    fn same_file(&self, live: &File) -> Result<(), &'static str> {
        let held = self.file.metadata().map_err(|_| "Plex fixture unavailable")?;
        let found = live.metadata().map_err(|_| "Plex fixture unavailable")?;
        if held.dev() != found.dev() || held.ino() != found.ino() {
            return Err("Plex fixture file was replaced");
        }
        Ok(())
    }
    fn current(&self) -> Result<(), &'static str> {
        same_directory(&self.directory, &self.parent)?;
        let name = self.declaration.path.file_name().and_then(|name| name.to_str())
            .ok_or("invalid Plex fixture filename")?;
        let live = checked_file_at(&self.directory, name, false, true)?;
        self.same_file(&live)?;
        let bytes = read_bounded(live, PROFILE_BYTES)?;
        validate_bytes(&bytes, &self.declaration.digest, self.declaration.port)?;
        // Recheck the path identity after reading. The caller's actual bytes are
        // also hash-checked, so a concurrent edit can never become imported authority.
        same_directory(&self.directory, &self.parent)?;
        self.same_file(&checked_file_at(&self.directory, name, false, true)?)
    }
    pub(super) fn path(&self) -> Result<PathBuf, &'static str> {
        self.current()?;
        Ok(self.declaration.path.clone())
    }
    pub(super) fn validate(&self, path: &Path, bytes: &[u8]) -> Result<(), &'static str> {
        if path != self.declaration.path { return Err("Plex fixture path changed"); }
        validate_bytes(bytes, &self.declaration.digest, self.declaration.port)?;
        self.current()
    }
}
