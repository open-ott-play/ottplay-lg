use super::*;
use crate::app::ott_local_sources::{RememberedFolders, UsbBinding};
use plx_data::ott::favorites::Origin;

fn configured() -> Saved {
    let mut saved = Saved::default();
    let folder = FolderDescriptor {
        id: "root".into(),
        name: "Movies".into(),
        path: "/private/fixture/Movies".into(),
        kind: LocalSourceKind::Folder,
        usb: Some(UsbBinding::from_observed("ext4", "UUID-1", "/", "Movies").unwrap()),
    };
    RememberedFolders::default()
        .with_connected(folder)
        .unwrap()
        .put_into(&mut saved.settings)
        .unwrap();
    let policy = stored::Stored::local_folder(
        Origin {
            provider: "local".into(),
            profile: "Local videos".into(),
            subprofile: None,
            source_alias: "Movies".into(),
        },
        true,
        "root".into(),
        "a".repeat(64),
    )
    .unwrap();
    saved
        .settings
        .insert(stored::KEY.into(), policy.encode().unwrap());
    saved
}

#[test]
fn source_scope_is_exact_and_strictness_is_not_source_identity() {
    let saved = configured();
    let scope = stored_scope(&saved).unwrap();
    let mut changed = saved.clone();
    let policy = stored::read(&changed.settings)
        .unwrap()
        .unwrap()
        .with_strict(false);
    changed
        .settings
        .insert(stored::KEY.into(), policy.encode().unwrap());
    assert!(stored_scope(&changed).as_ref() == Some(&scope));
    for field in ["uuid", "filesystem_type", "filesystem_relative_folder"] {
        let mut changed = saved.clone();
        changed.settings.get_mut("nativeLocalSources").unwrap()["folders"][0]["usb"][field] =
            json!("different");
        assert!(stored_scope(&changed).as_ref() != Some(&scope));
    }
    for field in ["id", "name", "path"] {
        let mut changed = saved.clone();
        changed.settings.get_mut("nativeLocalSources").unwrap()["folders"][0][field] =
            json!(if field == "path" {
                "/private/other"
            } else {
                "different"
            });
        assert!(stored_scope(&changed).as_ref() != Some(&scope));
    }
}

#[test]
fn new_designation_keeps_its_reservation_over_old_checkpoint_and_off_cannot_resurrect() {
    let mut candidate = configured();
    let expected = stored_scope(&candidate).unwrap().fingerprint;
    candidate.resume.insert(
        KEY.into(),
        SavedResume {
            entry_id: "new-first".into(),
            position_ms: 0,
            source_fingerprint: expected.clone(),
        },
    );
    let mut checkpoints = BTreeMap::new();
    checkpoints.insert(
        KEY.into(),
        SavedResume {
            entry_id: "old-file".into(),
            position_ms: 999,
            source_fingerprint: "old-root".into(),
        },
    );
    merge(&mut candidate, &checkpoints);
    sanitize(&mut candidate);
    assert_eq!(candidate.resume[KEY].entry_id, "new-first");
    checkpoints.get_mut(KEY).unwrap().source_fingerprint = expected;
    candidate.settings.remove(stored::KEY);
    merge(&mut candidate, &checkpoints);
    sanitize(&mut candidate);
    assert!(!candidate.resume.contains_key(KEY));
    merge(&mut candidate, &checkpoints);
    sanitize(&mut candidate);
    assert!(!candidate.resume.contains_key(KEY));
}

#[test]
fn admitted_first_target_gets_saved_offset_once_and_user_intent_cancels_late_restore() {
    let point = continuity::ResumePoint {
        entry_id: "saved".into(),
        position_ms: 12345,
    };
    let mut state = continuity::Continuity::new(7, Some(point.clone()));
    state.initial_admitted(7, "saved");
    assert_eq!(
        state.catalog_updated(7, &["saved"], false),
        continuity::Decision::Start {
            entry_id: "saved".into(),
            position_ms: 12345,
            reason: continuity::Reason::Restore
        }
    );
    assert_eq!(
        state.catalog_updated(7, &["saved"], false),
        continuity::Decision::Idle
    );
    for explicit in 0..3 {
        let mut state = continuity::Continuity::new(7, Some(point.clone()));
        state.initial_admitted(7, "fallback");
        match explicit {
            0 => state.pause(7),
            1 => state.user_selected(7, "fallback"),
            _ => state.stop(7),
        }
        assert_eq!(
            state.catalog_updated(7, &["fallback", "saved"], true),
            continuity::Decision::Idle
        );
    }
}

#[test]
fn only_terminal_scan_completion_retires_an_absent_saved_target() {
    let mut state = continuity::Continuity::new(
        8,
        Some(continuity::ResumePoint {
            entry_id: "missing".into(),
            position_ms: 50,
        }),
    );
    state.initial_admitted(8, "fallback");
    assert_eq!(
        state.catalog_updated(8, &["fallback"], false),
        continuity::Decision::Idle
    );
    assert!(state.pending_restore().is_some());
    assert_eq!(
        state.catalog_updated(8, &["fallback"], true),
        continuity::Decision::Idle
    );
    assert!(state.pending_restore().is_none());
}

#[test]
fn retiring_restore_does_not_reenable_paused_or_stopped_continuity() {
    for stopped in [false, true] {
        let mut state = continuity::Continuity::new(
            7,
            Some(continuity::ResumePoint {
                entry_id: "saved".into(),
                position_ms: 1,
            }),
        );
        state.initial_admitted(7, "fallback");
        if stopped {
            state.stop(7);
        } else {
            state.pause(7);
        }
        state.cancel_restore(7, Some("fallback"));
        assert_eq!(
            state.catalog_updated(7, &["fallback", "saved"], true),
            continuity::Decision::Idle
        );
        assert!(!state.accepts_end(7, "fallback"));
    }
}

#[test]
fn unrelated_usb_receipt_cannot_rebase_the_kiosk_checkpoint() {
    let mut current = configured();
    let scope = stored_scope(&current).unwrap();
    current.resume.insert(
        KEY.into(),
        SavedResume {
            entry_id: "file".into(),
            position_ms: 123,
            source_fingerprint: scope.fingerprint,
        },
    );
    let mut candidate = current.clone();
    candidate.settings.get_mut("nativeLocalSources").unwrap()["folders"][0]["path"] =
        json!("/private/new/Movies");
    let before = candidate.resume.clone();
    assert!(relocation(&current, &mut candidate, "other-source").is_none());
    assert!(candidate.resume == before);
}
