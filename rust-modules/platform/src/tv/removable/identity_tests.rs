use super::*;

fn mounts(rows: &[(u64, u32, &str, &str)]) -> Vec<MountCandidate> {
    let text: String = rows
        .iter()
        .map(|(id, minor, root, fs)| {
            format!("{id} 1 8:{minor} {root} /tmp/usb/disk{id}/part rw - {fs} /dev/unused rw\n")
        })
        .collect();
    super::super::candidates(text.as_bytes()).unwrap()
}
fn id(uuid: &str) -> StableIdentity {
    StableIdentity::new(uuid, "ext4").unwrap()
}

#[test]
fn exact_uuid_and_filesystem_are_preserved_without_normalization() {
    let value = StableIdentity::new("AaB0-9._:x", "ext4").unwrap();
    assert_eq!(value.uuid(), "AaB0-9._:x");
    assert_eq!(value.filesystem(), "ext4");
    assert!(value != StableIdentity::new("aab0-9._:x", "ext4").unwrap());
}

#[test]
fn invalid_and_oversized_identity_fields_are_rejected_whole() {
    for value in [
        "", ".", "..", "a/b", "a\\b", "a b", " a", "a\n", "é", "%41", "a\0b",
    ] {
        assert!(StableIdentity::new(value, "ext4").is_err(), "{value:?}");
    }
    assert!(StableIdentity::new(&"x".repeat(UUID_LIMIT), "ext4").is_ok());
    assert!(StableIdentity::new(&"x".repeat(UUID_LIMIT + 1), "ext4").is_err());
    assert!(StableIdentity::new("x", &"a".repeat(FILESYSTEM_LIMIT)).is_ok());
    assert!(StableIdentity::new("x", &"a".repeat(FILESYSTEM_LIMIT + 1)).is_err());
    for fs in ["", " ", "ext/4", "ext:4", ".", ".."] {
        assert!(StableIdentity::new("x", fs).is_err());
    }
}

#[test]
fn returned_identity_is_bound_to_exact_current_mount_including_bind_root() {
    let input = mounts(&[(7, 1, "/Media", "ext4")]);
    let output = resolve_with(&input, &mut || true, &mut |_| Ok(id("abc"))).unwrap();
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].mount(), &input[0]);
    assert_eq!(output[0].mount().root(), "/Media");
    assert_eq!(output[0].identity().uuid(), "abc");
}

#[test]
fn duplicate_uuid_across_distinct_devices_is_ambiguous_in_either_order() {
    for input in [
        mounts(&[(7, 1, "/", "ext4"), (8, 2, "/", "ext4")]),
        mounts(&[(8, 2, "/", "ext4"), (7, 1, "/", "ext4")]),
    ] {
        assert_eq!(
            resolve_with(&input, &mut || true, &mut |_| Ok(id("same"))).err(),
            Some(IdentityError::Ambiguous)
        );
    }
}

#[test]
fn same_device_aliases_share_one_read_and_keep_distinct_exposed_roots() {
    let input = mounts(&[(7, 1, "/", "ext4"), (8, 1, "/Media", "ext4")]);
    let mut calls = 0;
    let output = resolve_with(&input, &mut || true, &mut |_| {
        calls += 1;
        Ok(id("abc"))
    })
    .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(output.len(), 2);
    assert_eq!(output[1].mount().root(), "/Media");
}

#[test]
fn unknown_device_never_publishes_successful_prefix() {
    let input = mounts(&[(7, 1, "/", "ext4"), (8, 2, "/", "ext4")]);
    let mut calls = 0;
    assert_eq!(
        resolve_with(&input, &mut || true, &mut |_| {
            calls += 1;
            if calls == 1 {
                Ok(id("abc"))
            } else {
                Err(IdentityError::Unavailable)
            }
        })
        .err(),
        Some(IdentityError::Unavailable)
    );
    assert_eq!(calls, 2);
}

#[test]
fn filesystem_mismatch_is_not_guessed_from_driver_aliases() {
    for fs in ["xfs", "ntfs3", "fuseblk"] {
        let input = mounts(&[(7, 1, "/", fs)]);
        assert_eq!(
            resolve_with(&input, &mut || true, &mut |_| Ok(id("abc"))).err(),
            Some(IdentityError::Changed)
        );
    }
}

#[test]
fn same_device_conflicting_filesystem_is_rejected() {
    let input = mounts(&[(7, 1, "/", "ext4"), (8, 1, "/Media", "xfs")]);
    assert_eq!(
        resolve_with(&input, &mut || true, &mut |_| Ok(id("abc"))).err(),
        Some(IdentityError::Changed)
    );
}

#[test]
fn repeated_mount_id_and_hidden_device_fail_before_read() {
    let one = mounts(&[(7, 1, "/", "ext4")]);
    assert_eq!(
        resolve_with(
            &[one[0].clone(), one[0].clone()],
            &mut || true,
            &mut |_| panic!("no read")
        )
        .err(),
        Some(IdentityError::Invalid)
    );
    let zero =
        super::super::candidates(b"7 1 0:45 / /tmp/usb/a/b rw - fuseblk /dev/no rw\n").unwrap();
    assert_eq!(
        resolve_with(&zero, &mut || true, &mut |_| panic!(
            "no guessed backing device"
        ))
        .err(),
        Some(IdentityError::Unsupported)
    );
}

#[test]
fn overmounted_same_path_is_ambiguous_before_device_read() {
    let input = super::super::candidates(b"7 1 8:1 / /tmp/usb/a/b rw - ext4 /dev/no rw\n8 1 8:2 / /tmp/usb/a/b rw - ext4 /dev/no rw\n").unwrap();
    assert_eq!(
        resolve_with(&input, &mut || true, &mut |_| panic!("no read")).err(),
        Some(IdentityError::Ambiguous)
    );
}

#[test]
fn same_mount_point_with_a_new_uuid_never_reuses_prior_identity() {
    let input = mounts(&[(7, 1, "/", "ext4")]);
    let first = resolve_with(&input, &mut || true, &mut |_| Ok(id("first"))).unwrap();
    let second = resolve_with(&input, &mut || true, &mut |_| Ok(id("second"))).unwrap();
    assert!(first[0].identity() != second[0].identity());
}

#[test]
fn cancellation_after_property_read_rejects_the_whole_reply() {
    let input = mounts(&[(7, 1, "/", "ext4")]);
    let cancelled = std::cell::Cell::new(false);
    assert_eq!(
        resolve_with(&input, &mut || !cancelled.get(), &mut |_| {
            cancelled.set(true);
            Ok(id("abc"))
        })
        .err(),
        Some(IdentityError::Cancelled)
    );
}

#[test]
fn cancellation_and_mount_limit_are_checked_before_work() {
    let input = mounts(&[(7, 1, "/", "ext4")]);
    assert_eq!(
        resolve_with(&input, &mut || false, &mut |_| panic!("no work")).err(),
        Some(IdentityError::Cancelled)
    );
    let oversized = vec![input[0].clone(); super::super::MOUNT_LIMIT + 1];
    assert_eq!(
        resolve_with(&oversized, &mut || true, &mut |_| panic!("bounded")).err(),
        Some(IdentityError::Invalid)
    );
}

#[test]
fn unavailable_backend_is_not_translated_to_empty_list() {
    let input = mounts(&[(7, 1, "/", "ext4")]);
    for error in [
        IdentityError::Unavailable,
        IdentityError::Unsupported,
        IdentityError::Changed,
    ] {
        assert_eq!(
            resolve_with(&input, &mut || true, &mut |_| Err(error)).err(),
            Some(error)
        );
    }
}

#[test]
fn held_directory_device_is_observed_without_udev_or_path_reopening() {
    let off = OffFrame::for_test();
    let directory = std::fs::File::open(env!("CARGO_MANIFEST_DIR")).unwrap();
    let device = directory_device(&off, &directory).unwrap();
    let mount =
        MountCandidate::directory_device_for_test(env!("CARGO_MANIFEST_DIR"), 17, device).unwrap();
    assert_eq!(mount.device(), device);
    assert_eq!(mount.mount_id(), 17);
}

#[test]
fn regular_file_cannot_be_presented_as_a_held_root() {
    let off = OffFrame::for_test();
    let file = std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    assert_eq!(directory_device(&off, &file), Err(IdentityError::Invalid));
}
