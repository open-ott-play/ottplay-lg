use super::*;

#[test]
fn logical_preflight_includes_plain_metadata_at_the_exact_existing_limit() {
    for namespace in [Namespace::OttControl, Namespace::OttState] {
        let overhead = encode_plain(namespace, &Value::String(String::new()))
            .unwrap()
            .len();
        let value = Value::String("x".repeat(LOGICAL_BYTES - overhead));
        assert_eq!(
            encode_plain(namespace, &value).unwrap().len(),
            LOGICAL_BYTES
        );
        assert_eq!(validate_value(namespace, &value), Ok(()));
        let too_large = Value::String("x".repeat(LOGICAL_BYTES - overhead + 1));
        assert_eq!(validate_value(namespace, &too_large), Err(Error::TooLarge));
    }
}

#[test]
fn logical_preflight_counts_json_escaping_and_utf8_not_display_characters() {
    let namespace = Namespace::OttState;
    let overhead = encode_plain(namespace, &Value::String(String::new()))
        .unwrap()
        .len();
    let escaped = Value::String("\n".repeat((LOGICAL_BYTES - overhead) / 2 + 1));
    assert_eq!(validate_value(namespace, &escaped), Err(Error::TooLarge));
    let unicode = Value::String("界".repeat((LOGICAL_BYTES - overhead) / 3 + 1));
    assert_eq!(validate_value(namespace, &unicode), Err(Error::TooLarge));
    let value = serde_json::json!({"unchanged": [null, "界", "\\\n\""], "settings": {}});
    let before = value.clone();
    assert_eq!(validate_value(namespace, &value), Ok(()));
    assert_eq!(value, before);
    let decoded: Plain = serde_json::from_slice(&encode_plain(namespace, &value).unwrap()).unwrap();
    assert_eq!(decoded.value, before);
    assert_eq!(decoded.namespace, label(namespace));
    assert_eq!(decoded.install, plx_base::paths::app_id());
}

#[test]
fn actual_host_save_rejects_the_same_oversize_value_before_sealing_or_root_creation() {
    let path = plx_base::testscratch::process_dir("ott-value-preflight").join("must-not-exist");
    assert!(!path.exists());
    let value = Value::String("x".repeat(LOGICAL_BYTES));
    assert_eq!(
        validate_value(Namespace::OttState, &value),
        Err(Error::TooLarge)
    );
    assert_eq!(
        save_guarded_at(
            &path,
            Namespace::OttState,
            &value,
            &WritePermit::new(),
            |_, _| panic!("oversize must not reach the sealer")
        ),
        Err(Error::TooLarge)
    );
    assert!(!path.exists());
}
