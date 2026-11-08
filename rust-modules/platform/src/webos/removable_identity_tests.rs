use super::*;

#[test]
fn null_missing_or_invalid_utf8_property_is_not_an_identity() {
    assert_eq!(
        unsafe { copy_string(std::ptr::null(), UUID_LIMIT) }.err(),
        Some(IdentityError::Unavailable)
    );
    assert_eq!(
        unsafe { copy_string([0xffu8, 0].as_ptr().cast(), UUID_LIMIT) }.err(),
        Some(IdentityError::Invalid)
    );
}

#[test]
fn library_owned_property_is_copied_and_nul_terminated_within_bound() {
    let bytes = std::ffi::CString::new("ABC-123").unwrap();
    let copied = unsafe { copy_string(bytes.as_ptr(), 7) }.unwrap();
    drop(bytes);
    assert_eq!(copied, "ABC-123");
    let bytes = std::ffi::CString::new("ABC-1234").unwrap();
    assert_eq!(
        unsafe { copy_string(bytes.as_ptr(), 7) }.err(),
        Some(IdentityError::Invalid)
    );
}

#[test]
fn sysfs_device_text_requires_exact_bounded_unsigned_pair() {
    assert_eq!(parse_dev(b"8:17\n"), Some((8, 17)));
    assert_eq!(
        parse_dev(b"4294967295:4294967295\n"),
        Some((u32::MAX, u32::MAX))
    );
    for input in [
        b"8:17".as_slice(),
        b"8:-1\n",
        b"8:1\n\n",
        b"8:1 \n",
        b"+8:1\n",
        b":1\n",
        b"8:\n",
        b"8:1:2\n",
        b"4294967296:1\n",
    ] {
        assert_eq!(parse_dev(input), None, "{input:?}");
    }
}

#[test]
fn writable_or_user_owned_runtime_cache_is_not_trusted() {
    assert!(trusted_mode(0, 0o100644));
    assert!(trusted_mode(0, 0o040755));
    assert!(!trusted_mode(1000, 0o100644));
    assert!(!trusted_mode(0, 0o100664));
    assert!(!trusted_mode(0, 0o100646));
    assert!(!trusted_mode(0, 0o041777));
}

#[test]
fn supported_uuid_punctuation_never_needs_alias_path_decoding() {
    for token in ["a-b", "A_B", "a.b", "a:b"] {
        let value = StableIdentity::new(token, "ext4").unwrap();
        assert_eq!(
            Path::new("/dev/disk/by-uuid").join(value.uuid()).parent(),
            Some(Path::new("/dev/disk/by-uuid"))
        );
    }
    for token in ["../a", "%2e", "a\\x20b", "a b"] {
        assert!(StableIdentity::new(token, "ext4").is_err());
    }
}
