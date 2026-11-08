use super::*;
use serde_json::json;

fn row(index: usize, kind: LocalSourceKind) -> FolderDescriptor {
    FolderDescriptor {
        id: format!("source-{index}"),
        name: format!("Source {index}"),
        path: if kind == LocalSourceKind::File {
            format!("/media/videos/video-{index}.mp4")
        } else {
            format!("/media/videos/folder-{index}")
        },
        kind,
        usb: None,
    }
}

fn usb() -> UsbBinding {
    UsbBinding {
        namespace: UsbNamespace::LinuxFilesystemUuidV1,
        filesystem_type: "ext4".into(),
        uuid: "fixture-uuid".into(),
        filesystem_relative_folder: "Videos".into(),
    }
}

fn load(value: Value) -> Result<RememberedFolders, &'static str> {
    RememberedFolders::read(&BTreeMap::from([(SETTINGS_KEY.into(), value)]))
}

#[test]
fn legacy_folder_shapes_and_usb_bytes_roundtrip_unchanged() {
    for version in [1, 2] {
        let mut value = json!({"version":version,"folders":[{
            "id":"legacy","name":"Videos","path":"/media/videos"
        }]});
        for with_usb in [false, true] {
            if with_usb && version == 1 { continue; }
            if with_usb { value["folders"][0]["usb"] = serde_json::to_value(usb()).unwrap(); }
            let parsed = load(value.clone()).unwrap();
            assert_eq!(parsed.folders()[0].kind, LocalSourceKind::Folder);
            assert_eq!(serde_json::to_value(parsed).unwrap(), value);
        }
    }
}

#[test]
fn legacy_sequence_rows_keep_the_usb_field_position_and_reject_kind_strings() {
    let binding = serde_json::to_value(usb()).unwrap();
    let legacy = load(json!({"version":2,"folders":[[
        "legacy", "Videos", "/media/videos", binding.clone()
    ]]})).unwrap();
    assert_eq!(legacy.folders()[0].kind, LocalSourceKind::Folder);
    assert_eq!(serde_json::to_value(legacy).unwrap(), json!({"version":2,"folders":[{
        "id":"legacy", "name":"Videos", "path":"/media/videos", "usb":binding
    }]}));
    for version in [1, 2] {
        assert!(load(json!({"version":version,"folders":[[
            "legacy", "Videos", "/media/videos", "folder"
        ]]})).is_err());
        let plain = load(json!({"version":version,"folders":[[
            "legacy", "Videos", "/media/videos"
        ]]})).unwrap();
        assert_eq!(plain.folders()[0].kind, LocalSourceKind::Folder);
        assert!(plain.folders()[0].usb.is_none());
    }
}

#[test]
fn first_file_promotes_only_the_private_collection_and_preserves_old_rows() {
    for with_usb in [false, true] {
        let mut folder = row(0, LocalSourceKind::Folder);
        if with_usb { folder.usb = Some(usb()); }
        let old = RememberedFolders::default().with_connected(folder.clone()).unwrap();
        let old_row = serde_json::to_value(&old.folders()[0]).unwrap();
        let file = row(1, LocalSourceKind::File);
        let next = old.with_connected(file.clone()).unwrap();
        assert_eq!(next.version, 3);
        assert_eq!(next.folders(), &[folder, file]);
        assert_eq!(serde_json::to_value(&next.folders()[0]).unwrap(), old_row);
        let mut settings = BTreeMap::from([
            ("provider".into(), json!("m3u")),
            ("favorites".into(), json!(["unchanged"])),
            ("kiosk".into(), json!({"strict":true})),
        ]);
        let original = settings.clone();
        next.put_into(&mut settings).unwrap();
        assert_eq!(RememberedFolders::read(&settings).unwrap(), next);
        assert_eq!(settings[SETTINGS_KEY]["folders"][1]["kind"], "file");
        settings.remove(SETTINGS_KEY);
        assert_eq!(settings, original);
        assert_eq!(old.version, if with_usb { 2 } else { 1 });
        assert_eq!(old.folders().len(), 1);
    }
}

#[test]
fn kind_and_usb_never_silently_downgrade_or_expand_a_file() {
    let base = serde_json::to_value(row(0, LocalSourceKind::File)).unwrap();
    for version in [1, 2] {
        assert!(load(json!({"version":version,"folders":[base.clone()]})).is_err());
        let mut explicit_folder = serde_json::to_value(row(0, LocalSourceKind::Folder)).unwrap();
        explicit_folder["kind"] = json!("folder");
        assert!(load(json!({"version":version,"folders":[explicit_folder]})).is_err(),
            "legacy documents must retain their previous unknown-field rejection");
    }
    for kind in [Value::Null, json!("FILE"), json!("unknown"), json!(0),
        json!({"file":null}), json!({"folder":null})] {
        let mut invalid = base.clone();
        invalid["kind"] = kind;
        assert!(load(json!({"version":3,"folders":[invalid]})).is_err());
    }
    for binding in [Value::Null, serde_json::to_value(usb()).unwrap()] {
        let mut invalid = base.clone();
        invalid["usb"] = binding;
        assert!(load(json!({"version":3,"folders":[invalid]})).is_err());
    }
    let mut tagged_folder = serde_json::to_value(row(0, LocalSourceKind::Folder)).unwrap();
    tagged_folder["kind"] = json!({"folder":null});
    tagged_folder["usb"] = serde_json::to_value(usb()).unwrap();
    assert!(load(json!({"version":3,"folders":[tagged_folder]})).is_err());
    let mut root_file = row(0, LocalSourceKind::File);
    root_file.path = "/".into();
    assert!(RememberedFolders::default().with_connected(root_file).is_err());
    let mut usb_file = row(0, LocalSourceKind::File);
    usb_file.usb = Some(usb());
    assert!(RememberedFolders::default().with_connected(usb_file).is_err());
}

#[test]
fn mixed_sources_share_capacity_id_path_and_usb_uniqueness_rules() {
    let mut sources = RememberedFolders::default();
    for index in 0..32 {
        sources = sources.with_connected(row(index, if index % 2 == 0 {
            LocalSourceKind::Folder
        } else { LocalSourceKind::File })).unwrap();
    }
    let before = sources.clone();
    assert!(sources.with_connected(row(32, LocalSourceKind::File)).is_err());
    assert_eq!(sources, before);

    let original = row(0, LocalSourceKind::File);
    let one = RememberedFolders::default().with_connected(original.clone()).unwrap();
    assert_eq!(one.with_connected(original.clone()).unwrap(), one);
    let mut same_path = row(1, LocalSourceKind::Folder);
    same_path.path = original.path.clone();
    assert!(one.with_connected(same_path).is_err());
    let mut same_id = row(0, LocalSourceKind::Folder);
    same_id.path = "/media/another-folder".into();
    assert!(one.with_connected(same_id).is_err());

    let mut first_usb = row(2, LocalSourceKind::Folder);
    first_usb.usb = Some(usb());
    let mut second_usb = row(3, LocalSourceKind::Folder);
    second_usb.usb = Some(usb());
    let mixed = one.with_connected(first_usb).unwrap();
    assert!(mixed.with_connected(second_usb).is_err());
}

#[test]
fn reselection_cannot_change_kind_erase_usb_or_downgrade_version_three() {
    let file = row(0, LocalSourceKind::File);
    let mut folder = row(1, LocalSourceKind::Folder);
    folder.usb = Some(usb());
    let sources = RememberedFolders::default().with_connected(file.clone()).unwrap()
        .with_connected(folder.clone()).unwrap();
    assert_eq!(sources.version, 3);
    for original in [&file, &folder] {
        let mut changed = original.clone();
        changed.kind = if original.kind == LocalSourceKind::File {
            LocalSourceKind::Folder
        } else { LocalSourceKind::File };
        changed.usb = None;
        assert!(sources.with_reselected(changed).is_err());
    }
    let mut stripped = folder.clone();
    stripped.usb = None;
    assert!(sources.with_reselected(stripped).is_err());
    let mut moved = file.clone();
    moved.path = "/media/selected-again.mp4".into();
    let next = sources.with_reselected(moved.clone()).unwrap();
    assert_eq!(next.version, 3);
    assert_eq!(next.folders(), &[moved, folder.clone()]);
    let reselected_folder = next.with_reselected(folder).unwrap();
    assert_eq!(reselected_folder.version, 3);
    assert_eq!(sources.folders()[0], file);
}

#[test]
fn version_three_descriptor_cannot_serialize_live_authority() {
    for field in ["available", "authority_ref", "generation", "selected", "fd", "parent"] {
        let mut invalid = serde_json::to_value(row(0, LocalSourceKind::File)).unwrap();
        invalid[field] = json!(1);
        assert!(load(json!({"version":3,"folders":[invalid]})).is_err(), "{field}");
    }
    for version in [0, 4] {
        assert!(load(json!({"version":version,"folders":[]})).is_err());
    }
}
