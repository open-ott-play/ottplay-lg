//! Private remembered Local descriptors. These are not live access grants or
//! wire source rows: the runtime revalidates each source before minting authority.
//! No provider selection, filesystem access, Favorites mutation or writes here.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const SETTINGS_KEY: &str = "nativeLocalSources";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(super) enum LocalSourceKind {
    #[default]
    #[serde(rename = "folder")]
    Folder,
    #[serde(rename = "file")]
    File,
}

impl LocalSourceKind {
    fn is_folder(&self) -> bool { *self == Self::Folder }
}

impl<'de> Deserialize<'de> for LocalSourceKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match String::deserialize(deserializer)?.as_str() {
            "folder" => Ok(Self::Folder),
            "file" => Ok(Self::File),
            _ => Err(serde::de::Error::custom("Invalid local source kind")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FolderDescriptor {
    pub id: String,
    pub name: String,
    /// Receiver-private canonical POSIX source path, never portable metadata.
    pub path: String,
    /// Missing is the legacy Folder shape. A File never grants its parent folder.
    #[serde(default, skip_serializing_if = "LocalSourceKind::is_folder")]
    pub kind: LocalSourceKind,
    /// Installation-private stable filesystem binding. Never a live access grant.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present_usb")]
    pub usb: Option<UsbBinding>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(super) enum UsbNamespace {
    #[serde(rename = "linux-filesystem-uuid-v1")]
    LinuxFilesystemUuidV1,
}

/// Exact observed logical filesystem identity, not an uncloneable hardware ID.
/// Only the trusted, worker-verified Saved path may create an automatic restore.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UsbBinding {
    pub namespace: UsbNamespace,
    pub filesystem_type: String,
    pub uuid: String,
    /// Relative to the filesystem root, NOT to a possibly bind-mounted subtree.
    pub filesystem_relative_folder: String,
}

impl std::fmt::Debug for UsbBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsbBinding").finish_non_exhaustive()
    }
}

// Missing means legacy; present null is malformed, never an implicit downgrade.
fn present_usb<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<UsbBinding>, D::Error> {
    UsbBinding::deserialize(d).map(Some)
}

fn relative_folder(value: &str) -> bool {
    value.len() <= 4096 && !value.contains('\\') && !value.chars().any(char::is_control)
        && (value.is_empty() || value.split('/').all(|p| !matches!(p, "" | "." | "..")))
}

fn filesystem_root(value: &str) -> Result<&str, &'static str> {
    if value.len() > 4096 { return Err("Invalid filesystem mount root"); }
    let relative = value.strip_prefix('/').ok_or("Invalid filesystem mount root")?;
    if !relative_folder(relative) { return Err("Invalid filesystem mount root"); }
    Ok(relative)
}

impl UsbBinding {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        if self.uuid.is_empty() || self.uuid.len() > 256
            || matches!(self.uuid.as_str(), "." | "..")
            || !self.uuid.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.:".contains(&c))
            || self.filesystem_type.is_empty() || self.filesystem_type.len() > 64
            || matches!(self.filesystem_type.as_str(), "." | "..")
            || !self.filesystem_type.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
            || !relative_folder(&self.filesystem_relative_folder)
        { return Err("Invalid stable USB binding"); }
        Ok(())
    }

    /// Pure conversion after a verified worker observation. Exact identity bytes
    /// are preserved; parsing never case-folds, trims, or URL-decodes identifiers.
    pub(super) fn from_observed(filesystem_type: &str, uuid: &str,
        mount_root: &str, selected_relative_folder: &str) -> Result<Self, &'static str> {
        let root = filesystem_root(mount_root)?;
        if !relative_folder(selected_relative_folder) { return Err("Invalid selected USB folder"); }
        let path = match (root.is_empty(), selected_relative_folder.is_empty()) {
            (true, _) => selected_relative_folder.to_owned(),
            (_, true) => root.to_owned(),
            _ => format!("{root}/{selected_relative_folder}"),
        };
        let binding = Self { namespace: UsbNamespace::LinuxFilesystemUuidV1,
            filesystem_type: filesystem_type.into(), uuid: uuid.into(), filesystem_relative_folder: path };
        binding.validate()?;
        Ok(binding)
    }

    /// Translate only after the caller matched the exact observed identity.
    /// An inaccessible subtree never falls back to the mounted root or basename.
    pub(super) fn relative_folder_for_mount_root<'a>(&'a self, mount_root: &str)
        -> Result<&'a str, &'static str> {
        self.validate()?;
        let root = filesystem_root(mount_root)?;
        let saved = self.filesystem_relative_folder.as_str();
        if root.is_empty() { return Ok(saved); }
        if root == saved { return Ok(""); }
        saved.strip_prefix(root).and_then(|suffix| suffix.strip_prefix('/'))
            .ok_or("Saved USB folder is outside this mount")
    }
}


fn clean_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

impl FolderDescriptor {
    fn minimum_version(&self) -> u32 {
        if self.kind == LocalSourceKind::File { 3 }
        else if self.usb.is_some() { 2 }
        else { 1 }
    }

    pub(super) fn validate(&self) -> Result<(), &'static str> {
        if self.id.is_empty()
            || self.id.len() > 256
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
            || !plx_data::ott::valid_local_source_label(&self.name)
            || !clean_text(&self.path, 4096)
            || !self.path.starts_with('/')
            || (self.path != "/"
                && self
                    .path
                    .split('/')
                    .skip(1)
                    .any(|part| matches!(part, "" | "." | "..")))
        {
            return Err("Invalid remembered local folder");
        }
        if self.kind == LocalSourceKind::File && (self.usb.is_some() || self.path == "/") {
            return Err("Invalid remembered local file");
        }
        if let Some(usb) = &self.usb { usb.validate()?; }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) struct RememberedFolders {
    version: u32,
    folders: Vec<FolderDescriptor>,
}

impl<'de> Deserialize<'de> for RememberedFolders {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            version: u32,
            // Preserve duplicate fields until the version-specific typed row
            // parser has validated them, including nested USB binding fields.
            folders: Vec<Box<serde_json::value::RawValue>>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct LegacyFolder {
            id: String,
            name: String,
            path: String,
            #[serde(default, deserialize_with = "present_usb")]
            usb: Option<UsbBinding>,
        }
        let wire = Wire::deserialize(deserializer)?;
        // Preserve the legacy four-field struct for both map and sequence input.
        // Only v3 adds kind; an old fourth field remains USB, never a kind tag.
        let legacy = matches!(wire.version, 1 | 2);
        let folders = wire.folders.into_iter().map(|row| {
            if legacy {
                serde_json::from_str::<LegacyFolder>(row.get()).map(|old| FolderDescriptor {
                    id: old.id, name: old.name, path: old.path,
                    kind: LocalSourceKind::Folder, usb: old.usb,
                })
            } else {
                serde_json::from_str::<FolderDescriptor>(row.get())
            }
        })
            .collect::<Result<Vec<FolderDescriptor>, _>>()
            .map_err(serde::de::Error::custom)?;
        Ok(Self { version: wire.version, folders })
    }
}

impl Default for RememberedFolders {
    fn default() -> Self {
        Self {
            version: 1,
            folders: Vec::new(),
        }
    }
}

impl RememberedFolders {
    pub fn read(settings: &BTreeMap<String, Value>) -> Result<Self, &'static str> {
        let value = match settings.get(SETTINGS_KEY) {
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|_| "Invalid remembered local folders")?,
            None => Self::default(),
        };
        value.validate()?;
        Ok(value)
    }

    /// Historical accessor name; v3 callers must dispatch each source by kind.
    pub fn folders(&self) -> &[FolderDescriptor] {
        &self.folders
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if !matches!(self.version, 1 | 2 | 3) || self.folders.len() > 32
            || self.folders.iter().any(|folder| folder.minimum_version() > self.version) {
            return Err("Invalid remembered local folder collection");
        }
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        let mut usb_folders = BTreeSet::new();
        for folder in &self.folders {
            folder.validate()?;
            if folder.usb.as_ref().is_some_and(|usb| !usb_folders.insert(usb)) {
                return Err("Duplicate remembered USB folder identity");
            }
            if !ids.insert(&folder.id) || !paths.insert(&folder.path) {
                return Err("Duplicate remembered local folder identity");
            }
        }
        Ok(())
    }

    /// Called only after the explicit connect worker has canonicalized and
    /// admitted the requested source. The returned candidate is not persisted.
    /// Equal names remain separate: the selection layer must reject ambiguity.
    pub fn with_connected(&self, folder: FolderDescriptor) -> Result<Self, &'static str> {
        self.validate()?;
        folder.validate()?;
        if let Some(old) = self.folders.iter().find(|old| old.id == folder.id) {
            return if *old == folder {
                Ok(self.clone())
            } else {
                Err("A local source identity cannot be silently rebound")
            };
        }
        let mut next = self.clone();
        next.version = next.version.max(folder.minimum_version());
        next.folders.push(folder);
        next.validate()?;
        Ok(next)
    }

    /// Explicit device reselection may move a preserved source ID to its new
    /// canonical path without changing kind. Only the worker-confirmed path
    /// calls this method; a returned descriptor is never a live access grant.
    pub(super) fn with_reselected(&self, folder: FolderDescriptor) -> Result<Self, &'static str> {
        self.validate()?;
        folder.validate()?;
        let mut next = self.clone();
        next.version = next.version.max(folder.minimum_version());
        if let Some(old) = next.folders.iter_mut().find(|old| old.id == folder.id) {
            if old.kind != folder.kind {
                return Err("A local source identity cannot change kind");
            }
            if old.usb.is_some() && folder.usb.is_none() {
                return Err("A stable USB binding cannot be erased by a legacy selection");
            }
            *old = folder;
        } else { next.folders.push(folder); }
        next.validate()?;
        Ok(next)
    }

    /// Alters only a candidate settings map. The existing verified Saved write
    /// owns commit/uncertainty; this does not change profiles or live authority.
    pub fn put_into(&self, settings: &mut BTreeMap<String, Value>) -> Result<(), &'static str> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(|_| "Invalid remembered local folders")?;
        settings.insert(SETTINGS_KEY.into(), value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn folder(n: usize) -> FolderDescriptor {
        FolderDescriptor {
            kind: LocalSourceKind::Folder,
            usb: None,
            id: format!("root-{n}"),
            name: "Movies".into(),
            path: format!("/media/disk-{n}/Movies"),
        }
    }

    #[test]
    fn absent_collection_is_empty_and_roundtrip_touches_only_its_private_key() {
        let mut settings = BTreeMap::from([
            ("selectedProvider".into(), json!("m3u")),
            ("slot:m3u-1".into(), json!(1)),
            ("offlineProfile:1".into(), json!({"name":"kept"})),
        ]);
        let original = settings.clone();
        let empty = RememberedFolders::read(&settings).unwrap();
        assert!(empty.folders().is_empty());
        let connected = empty.with_connected(folder(0)).unwrap();
        connected.put_into(&mut settings).unwrap();
        assert_eq!(RememberedFolders::read(&settings).unwrap(), connected);
        settings.remove(SETTINGS_KEY);
        assert_eq!(settings, original);
    }

    #[test]
    fn thirty_two_connections_are_independent_and_overflow_does_not_mutate() {
        let mut roots = RememberedFolders::default();
        for n in 0..32 {
            roots = roots.with_connected(folder(n)).unwrap();
        }
        let before = roots.clone();
        assert!(roots.with_connected(folder(32)).is_err());
        assert_eq!(roots, before);
        assert_eq!(roots.folders().len(), 32);
    }

    #[test]
    fn duplicate_names_are_preserved_but_duplicate_id_or_path_is_rejected() {
        let one = RememberedFolders::default()
            .with_connected(folder(0))
            .unwrap();
        assert_eq!(one.with_connected(folder(0)).unwrap(), one);
        let two = one.with_connected(folder(1)).unwrap();
        assert_eq!(two.folders()[0].name, two.folders()[1].name);
        let mut rebound = folder(0);
        rebound.path = "/elsewhere/Movies".into();
        assert!(one.with_connected(rebound).is_err());
        let mut duplicate = folder(1);
        duplicate.path = folder(0).path;
        assert!(one.with_connected(duplicate).is_err());
    }

    #[test]
    fn stored_descriptor_never_accepts_live_authority_or_availability_fields() {
        for key in ["available", "authority_ref", "generation", "selected"] {
            let mut row = serde_json::to_value(folder(0)).unwrap();
            row[key] = json!(true);
            let settings =
                BTreeMap::from([(SETTINGS_KEY.into(), json!({"version":1,"folders":[row]}))]);
            assert!(RememberedFolders::read(&settings).is_err(), "{key}");
        }
    }

    #[test]
    fn malformed_paths_and_source_names_cannot_enter_a_candidate() {
        for path in [
            "relative/Movies",
            "~/Movies",
            "https://example.test/Movies",
            "C:/Movies",
            "/media/../Movies",
            "/media/./Movies",
            "/media//Movies",
            "/media/Movies/",
            "/media/Movies\u{0}",
        ] {
            let mut row = folder(0);
            row.path = path.into();
            assert!(
                RememberedFolders::default().with_connected(row).is_err(),
                "{path:?}"
            );
        }
        for name in ["", "a\n", "a\0", "x\u{85}y"] {
            let mut row = folder(0);
            row.name = name.into();
            assert!(
                RememberedFolders::default().with_connected(row).is_err(),
                "{name:?}"
            );
        }
        // Display labels are not paths or legacy name selectors. The shared
        // validator preserves these bytes; the private path and id stay intact.
        for name in [" ", ".", "..", "a/b", "a\\b"] {
            let mut row = folder(0);
            row.name = name.into();
            let expected = row.clone();
            let candidate = RememberedFolders::default().with_connected(row).unwrap();
            assert_eq!(candidate.folders(), &[expected]);
            let mut settings = BTreeMap::new();
            candidate.put_into(&mut settings).unwrap();
            assert_eq!(RememberedFolders::read(&settings).unwrap(), candidate);
        }
        let mut oversized = folder(0);
        oversized.name = "x".repeat(513);
        assert!(RememberedFolders::default().with_connected(oversized).is_err());
    }

    #[test]
    fn canonical_private_posix_paths_do_not_acquire_portable_path_restrictions() {
        for path in ["/", "/media/Фильмы", "/media/old\\disk/Movies"] {
            let mut row = folder(0);
            row.path = path.into();
            assert!(RememberedFolders::default().with_connected(row).is_ok());
        }
    }

    #[test]
    fn unknown_version_duplicate_rows_and_unknown_collection_fields_are_rejected() {
        for value in [
            json!({"version":4,"folders":[]}),
            json!({"version":1,"folders":[folder(0),folder(0)]}),
            json!({"version":1,"folders":[],"active":0}),
            Value::Null,
        ] {
            let settings = BTreeMap::from([(SETTINGS_KEY.into(), value)]);
            assert!(RememberedFolders::read(&settings).is_err());
        }
    }
    #[test]
    fn explicit_reselection_preserves_identity_and_other_folders() {
        let original = RememberedFolders::default().with_connected(folder(0)).unwrap().with_connected(folder(1)).unwrap();
        let mut next = folder(0); next.path = "/tmp/usb/new/partition/Movies".into();
        let result = original.with_reselected(next.clone()).unwrap();
        assert_eq!(result.folders()[0], next);
        assert_eq!(result.folders()[1], original.folders()[1]);
        assert_eq!(original.folders()[0], folder(0));
    }
    #[test]
    fn explicit_reselection_cannot_merge_two_roots_by_path() {
        let original = RememberedFolders::default().with_connected(folder(0)).unwrap().with_connected(folder(1)).unwrap();
        let mut next = folder(0); next.path = folder(1).path;
        assert!(original.with_reselected(next).is_err());
        assert_eq!(original.folders().len(), 2);
    }

}

#[cfg(test)]
#[path = "ott_local_sources_usb_tests.rs"]
mod usb_tests;

#[cfg(test)]
#[path = "ott_local_sources_file_tests.rs"]
mod file_tests;


#[cfg(test)]
mod display_label_tests {
    use super::*;
    #[test]
    fn remembered_display_labels_do_not_relax_paths_or_identities() {
        let mut row=FolderDescriptor{id:"root".into(),name:"Movies".into(),path:"/media/Movies".into(),kind:LocalSourceKind::Folder,usb:None};
        for label in [".",".."," ","back\\slash","親 / Movies"] {row.name=label.into();assert!(row.validate().is_ok());}
        for label in [String::new(),"x\ny".into(),"я".repeat(257)] {row.name=label;assert!(row.validate().is_err());}
        row.name="..".into();
        for path in ["../Movies","/media/../Movies","/media//Movies","/media/./Movies"] {row.path=path.into();assert!(row.validate().is_err());}
    }
}
