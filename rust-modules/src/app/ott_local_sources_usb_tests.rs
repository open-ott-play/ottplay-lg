use super::*;
use serde_json::json;

fn binding() -> UsbBinding {
    UsbBinding::from_observed("ext4", "A1-b2", "/Media", "Movies").unwrap()
}
fn folder(id: &str) -> FolderDescriptor {
    FolderDescriptor { kind: crate::app::ott_local_sources::LocalSourceKind::Folder, id: id.into(), name: "Movies".into(), path: format!("/media/{id}/Movies"), usb: Some(binding()) }
}
fn read(value: Value) -> Result<RememberedFolders, &'static str> {
    RememberedFolders::read(&BTreeMap::from([(SETTINGS_KEY.into(), value)]))
}

#[test]
fn legacy_roundtrip_does_not_invent_a_binding_or_upgrade() {
    let original = json!({"version":1,"folders":[{"id":"a","name":"Movies","path":"/media/a/Movies"}]});
    let state = read(original.clone()).unwrap();
    assert!(state.folders()[0].usb.is_none());
    assert_eq!(serde_json::to_value(state).unwrap(), original);
}

#[test]
fn binding_requires_v2_and_new_connection_promotes_collection() {
    assert!(read(json!({"version":1,"folders":[folder("a")]})).is_err());
    let state = RememberedFolders::default().with_connected(folder("a")).unwrap();
    assert_eq!(state.version, 2);
    assert_eq!(read(serde_json::to_value(&state).unwrap()).unwrap(), state);
    assert!(read(json!({"version":3,"folders":[]})).is_ok());
    assert!(read(json!({"version":3,"folders":[folder("a")]})).is_ok());
    for version in [0, 4, u32::MAX] {
        assert!(read(json!({"version":version,"folders":[]})).is_err());
    }
}

#[test]
fn binding_schema_rejects_null_unknown_namespace_and_live_authority() {
    let mut row = serde_json::to_value(folder("a")).unwrap();
    row["usb"] = Value::Null;
    assert!(read(json!({"version":2,"folders":[row]})).is_err());
    for (key, value) in [("namespace", json!("path-hash-v1")), ("authority_ref", json!("grant")),
        ("generation", json!(1)), ("device_number", json!(2049)), ("mount_id", json!(12))] {
        let mut row = serde_json::to_value(folder("a")).unwrap();
        row["usb"][key] = value;
        assert!(read(json!({"version":2,"folders":[row]})).is_err(), "{key}");
    }
}

#[test]
fn duplicate_raw_fields_fail_in_all_schema_objects_including_null_optional() {
    for text in [
        r#"{"version":2,"version":2,"folders":[]}"#,
        r#"{"version":2,"folders":[],"folders":[]}"#,
        r#"{"version":2,"folders":[{"id":"a","id":"b","name":"n","path":"/x"}]}"#,
        r#"{"version":2,"folders":[{"id":"a","name":"n","path":"/x","usb":null,"usb":{"namespace":"linux-filesystem-uuid-v1","filesystem_type":"ext4","uuid":"a","filesystem_relative_folder":""}}]}"#,
        r#"{"version":2,"folders":[{"id":"a","name":"n","path":"/x","usb":{"namespace":"linux-filesystem-uuid-v1","filesystem_type":"ext4","uuid":"a","uuid":"b","filesystem_relative_folder":""}}]}"#,
    ] { assert!(serde_json::from_str::<RememberedFolders>(text).is_err(), "{text}"); }
}

#[test]
fn raw_duplicate_row_fields_stay_rejected_in_every_supported_version() {
    for version in [1, 2, 3] {
        for row in [
            r#"{"id":"a","id":"b","name":"n","path":"/x"}"#,
            r#"{"id":"a","\u0069d":"b","name":"n","path":"/x"}"#,
            r#"{"id":"a","name":"n","name":"other","path":"/x"}"#,
            r#"{"id":"a","name":"n","path":"/x","path":"/y"}"#,
            r#"{"id":"a","name":"n","path":"/x","usb":null,"usb":{"namespace":"linux-filesystem-uuid-v1","filesystem_type":"ext4","uuid":"a","filesystem_relative_folder":""}}"#,
            r#"{"id":"a","name":"n","path":"/x","usb":{"namespace":"linux-filesystem-uuid-v1","filesystem_type":"ext4","uuid":"a","uuid":"b","filesystem_relative_folder":""}}"#,
        ] {
            let text = format!(r#"{{"folders":[{row}],"version":{version}}}"#);
            assert!(serde_json::from_str::<RememberedFolders>(&text).is_err(), "{text}");
        }
    }
    let duplicate_kind = r#"{"folders":[{"id":"a","name":"n","path":"/x.mp4","kind":"folder","kind":"file"}],"version":3}"#;
    assert!(serde_json::from_str::<RememberedFolders>(duplicate_kind).is_err());
}

#[test]
fn raw_rows_preserve_late_version_selection_and_legacy_sequence_positions() {
    for version in [1, 2, 3] {
        let text = format!(r#"{{"folders":[{{"id":"a","name":"n","path":"/x"}}],"version":{version}}}"#);
        let parsed: RememberedFolders = serde_json::from_str(&text).unwrap();
        parsed.validate().unwrap();
        assert_eq!(parsed.folders()[0].kind, LocalSourceKind::Folder);
        assert_eq!(read(serde_json::from_str(&text).unwrap()).unwrap(), parsed);
    }
    let legacy = json!({"version":2,"folders":[["a","n","/x",binding()]]});
    let parsed: RememberedFolders = serde_json::from_str(&legacy.to_string()).unwrap();
    parsed.validate().unwrap();
    assert_eq!(parsed.folders()[0].usb.as_ref(), Some(&binding()));
    assert_eq!(read(legacy.clone()).unwrap(), parsed);
    let mut wrong_position = legacy;
    wrong_position["version"] = json!(3);
    assert!(read(wrong_position).is_err(), "v3 fourth sequence field is kind, not USB");
    let file = r#"{"folders":[["f","file","/x.mp4","file"]],"version":3}"#;
    let parsed: RememberedFolders = serde_json::from_str(file).unwrap();
    parsed.validate().unwrap();
    assert_eq!(parsed.folders()[0].kind, LocalSourceKind::File);
    assert!(parsed.folders()[0].usb.is_none());
    assert_eq!(read(serde_json::from_str(file).unwrap()).unwrap(), parsed);
}

#[test]
fn same_filesystem_folder_cannot_gain_a_second_source_identity() {
    let state = RememberedFolders::default().with_connected(folder("a")).unwrap();
    let before = state.clone();
    assert!(state.with_connected(folder("b")).is_err());
    assert_eq!(state, before);
    let mut other = folder("b");
    other.usb.as_mut().unwrap().filesystem_relative_folder = "Media/Other".into();
    assert_eq!(state.with_connected(other).unwrap().folders.len(), 2);
}

#[test]
fn legacy_connect_and_reselect_cannot_erase_binding() {
    let original = folder("a");
    let state = RememberedFolders::default().with_connected(original.clone()).unwrap();
    let mut legacy = original.clone(); legacy.usb = None;
    assert!(state.with_connected(legacy.clone()).is_err());
    assert!(state.with_reselected(legacy).is_err());
    assert_eq!(state.folders(), &[original]);
}

#[test]
fn explicit_reselection_changes_exact_source_and_preserves_other_rows() {
    let mut other = folder("b"); other.usb.as_mut().unwrap().uuid = "other".into();
    let state = RememberedFolders::default().with_connected(folder("a")).unwrap().with_connected(other.clone()).unwrap();
    let mut next = folder("a"); next.path = "/media/new/Movies".into(); next.usb.as_mut().unwrap().uuid = "new".into();
    assert!(state.with_connected(next.clone()).is_err());
    let changed = state.with_reselected(next.clone()).unwrap();
    assert_eq!(changed.folders(), &[next, other]);
    assert_eq!(changed.version, 2);
    assert_eq!(state.folders()[0], folder("a"));
}

#[test]
fn mount_root_translation_uses_component_boundary_and_preserves_subfolder() {
    let saved = binding();
    assert_eq!(saved.filesystem_relative_folder, "Media/Movies");
    assert_eq!(saved.relative_folder_for_mount_root("/").unwrap(), "Media/Movies");
    assert_eq!(saved.relative_folder_for_mount_root("/Media").unwrap(), "Movies");
    assert_eq!(saved.relative_folder_for_mount_root("/Media/Movies").unwrap(), "");
    for root in ["/Med", "/Media2", "/Other", "/Media/Movies/Child"] {
        assert!(saved.relative_folder_for_mount_root(root).is_err(), "{root}");
    }
}

#[test]
fn exact_root_and_unicode_folder_roundtrip_without_decoding() {
    let root = UsbBinding::from_observed("exfat", "12AB-34CD", "/", "").unwrap();
    assert_eq!(root.relative_folder_for_mount_root("/").unwrap(), "");
    assert!(root.relative_folder_for_mount_root("/Movies").is_err());
    let item = UsbBinding::from_observed("exfat", "12AB-34CD", "/Медиа", "Кино/%2e%2e").unwrap();
    assert_eq!(item.relative_folder_for_mount_root("/Медиа").unwrap(), "Кино/%2e%2e");
    assert_eq!(item.uuid, "12AB-34CD");
}

#[test]
fn invalid_relative_and_mount_paths_never_fall_back_to_root() {
    for path in ["/Movies", "a//b", "a/", ".", "..", "a/../b", "a/./b", "a\\b", "a\0b", "a\nb"] {
        assert!(UsbBinding::from_observed("ext4", "a", "/", path).is_err(), "{path:?}");
    }
    for root in ["", "relative", "//", "/a/", "/a//b", "/..", "/a/./b", "/a\\b"] {
        assert!(UsbBinding::from_observed("ext4", "a", root, "").is_err(), "{root:?}");
        assert!(binding().relative_folder_for_mount_root(root).is_err());
    }
}

#[test]
fn identity_and_path_limits_are_exact_and_no_normalization_is_performed() {
    let uuid = "A".repeat(256); let fs = "x".repeat(64); let path = "x".repeat(4096);
    assert!(UsbBinding::from_observed(&fs, &uuid, "/", &path).is_ok());
    assert!(UsbBinding::from_observed(&fs, &(uuid.clone()+"A"), "/", "").is_err());
    assert!(UsbBinding::from_observed(&(fs.clone()+"x"), "a", "/", "").is_err());
    assert!(UsbBinding::from_observed("ext4", "a", "/x", &path).is_err());
    assert!(UsbBinding::from_observed("ext4", "a", &format!("/{path}"), "").is_err());
    for invalid in ["", " a", "a ", "a/b", "a\\b", "a%2fb", "a\nb", "é"] {
        assert!(UsbBinding::from_observed("ext4", invalid, "/", "").is_err(), "{invalid:?}");
    }
    assert_ne!(UsbBinding::from_observed("ext4", "A", "/", "").unwrap(), UsbBinding::from_observed("ext4", "a", "/", "").unwrap());
}

#[test]
fn folder_capacity_and_private_debug_remain_bounded() {
    let mut state = RememberedFolders::default();
    for index in 0..32 {
        let mut next = folder(&format!("id{index}")); next.usb.as_mut().unwrap().uuid = format!("uuid{index}");
        state = state.with_connected(next).unwrap();
    }
    assert!(state.with_connected(folder("overflow")).is_err());
    let value = binding(); let debug = format!("{value:?}");
    assert!(!debug.contains(&value.uuid) && !debug.contains(&value.filesystem_relative_folder));
}

#[test]
fn dot_identity_tokens_match_platform_rejection() {
    for value in [".", ".."] {
        assert!(UsbBinding::from_observed("ext4", value, "/", "").is_err());
        assert!(UsbBinding::from_observed(value, "uuid", "/", "").is_err());
    }
    assert!(UsbBinding::from_observed("ext4", "a.b", "/", "").is_ok());
}
