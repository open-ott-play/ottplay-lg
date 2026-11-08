use super::*;

fn key(value: &str) -> VolumeKey {
    VolumeKey {
        namespace: "filesystem-uuid".into(),
        value: value.into(),
    }
}
fn source() -> RememberedSource {
    RememberedSource {
        source_id: "local-ssd-1".into(),
        volume: Some(key("volume-a")),
        relative_folder: "Videos/Family".into(),
    }
}
fn mount(incarnation: &str, root: &str) -> MountObservation {
    MountObservation {
        volume: Some(key("volume-a")),
        incarnation: incarnation.into(),
        canonical_mount: root.into(),
        relative_folder: "Videos/Family".into(),
        folder: FolderObservation::Readable {
            canonical_path: format!("{root}/Videos/Family"),
        },
    }
}
fn snapshot(generation: u64, mounts: &[MountObservation]) -> InventorySnapshot<'_> {
    InventorySnapshot {
        generation,
        complete: true,
        mounts,
    }
}
fn stamp(generation: u64) -> GrantStamp {
    GrantStamp {
        authority_ref: AuthorityRef(3),
        generation,
    }
}
fn plan(value: Assessment) -> BindingPlan {
    match value {
        Assessment::Revalidate(plan) => plan,
        _ => panic!("expected revalidation"),
    }
}
fn binding(src: &RememberedSource, mounts: &[MountObservation]) -> Binding {
    let snapshot = snapshot(10, mounts);
    let selection = Selection::Explicit {
        incarnation: &mounts[0].incarnation,
    };
    let proposal = plan(assess(src, &snapshot, selection, None).unwrap());
    admit(proposal, &snapshot, stamp(10)).unwrap()
}

#[test]
fn same_volume_reappears_at_new_path_with_same_source_and_fresh_slot_generation() {
    let src = source();
    let old = binding(&src, &[mount("attach-1", "/media/usb-a")]);
    assert!(matches!(
        assess(
            &src,
            &snapshot(11, &[]),
            Selection::Automatic,
            Some(PreviousBinding {
                binding: &old,
                retired: false
            })
        )
        .unwrap(),
        Assessment::Unavailable(Unavailable::Absent)
    ));
    let rows = [mount("attach-2", "/media/usb-b")];
    let fresh = snapshot(12, &rows);
    let proposal = plan(
        assess(
            &src,
            &fresh,
            Selection::Automatic,
            Some(PreviousBinding {
                binding: &old,
                retired: false,
            }),
        )
        .unwrap(),
    );
    let new = admit(proposal, &fresh, stamp(12)).unwrap();
    assert_eq!(new.source_id(), old.source_id());
    assert_eq!(new.canonical_path(), "/media/usb-b/Videos/Family");
    assert!(new.grant().authority_ref == old.grant().authority_ref);
    assert_eq!(new.grant().generation, 12);
}

#[test]
fn still_present_incarnation_keeps_current_grant_without_generation_bump() {
    let src = source();
    let rows = [mount("attach-1", "/media/usb-a")];
    let old = binding(&src, &rows);
    assert!(matches!(
        assess(
            &src,
            &snapshot(10, &rows),
            Selection::Automatic,
            Some(PreviousBinding {
                binding: &old,
                retired: false
            })
        )
        .unwrap(),
        Assessment::Current
    ));
    assert!(matches!(
        assess(
            &src,
            &snapshot(11, &rows),
            Selection::Automatic,
            Some(PreviousBinding {
                binding: &old,
                retired: false
            })
        )
        .unwrap(),
        Assessment::Current
    ));
    assert_eq!(old.grant().generation, 10);
}

#[test]
fn different_volume_cannot_reuse_old_mount_path_even_by_explicit_selection() {
    let src = source();
    let mut replacement = mount("attach-2", "/media/usb-a");
    replacement.volume = Some(key("volume-b"));
    let rows = [replacement];
    let fresh = snapshot(11, &rows);
    assert!(matches!(
        assess(&src, &fresh, Selection::Automatic, None).unwrap(),
        Assessment::Unavailable(Unavailable::Absent)
    ));
    assert!(matches!(
        assess(
            &src,
            &fresh,
            Selection::Explicit {
                incarnation: "attach-2"
            },
            None
        )
        .unwrap(),
        Assessment::Unavailable(Unavailable::DifferentVolume)
    ));
}

#[test]
fn cloned_volume_keys_are_ambiguous_even_if_one_folder_is_unreadable() {
    let mut other = mount("attach-2", "/media/usb-b");
    other.folder = FolderObservation::PermissionDenied;
    let rows = [mount("attach-1", "/media/usb-a"), other];
    let fresh = snapshot(11, &rows);
    for selection in [
        Selection::Automatic,
        Selection::Explicit {
            incarnation: "attach-1",
        },
    ] {
        assert!(matches!(
            assess(&source(), &fresh, selection, None).unwrap(),
            Assessment::Unavailable(Unavailable::Ambiguous)
        ));
    }
}

#[test]
fn unknown_identity_requires_explicit_selection_again_after_reappearance() {
    let mut src = source();
    src.volume = None;
    let mut observation = mount("attach-1", "/media/usb-a");
    observation.volume = None;
    let rows = [observation.clone()];
    assert!(matches!(
        assess(&src, &snapshot(10, &rows), Selection::Automatic, None).unwrap(),
        Assessment::Unavailable(Unavailable::UnknownIdentity)
    ));
    let old = binding(&src, &rows);
    assert!(matches!(
        assess(
            &src,
            &snapshot(10, &rows),
            Selection::Automatic,
            Some(PreviousBinding {
                binding: &old,
                retired: false
            })
        )
        .unwrap(),
        Assessment::Current
    ));
    observation.incarnation = "attach-2".into();
    let rows = [observation];
    assert!(matches!(
        assess(
            &src,
            &snapshot(12, &rows),
            Selection::Automatic,
            Some(PreviousBinding {
                binding: &old,
                retired: false
            })
        )
        .unwrap(),
        Assessment::Unavailable(Unavailable::UnknownIdentity)
    ));
    let fresh = snapshot(12, &rows);
    let proposal = plan(
        assess(
            &src,
            &fresh,
            Selection::Explicit {
                incarnation: "attach-2",
            },
            Some(PreviousBinding {
                binding: &old,
                retired: false,
            }),
        )
        .unwrap(),
    );
    assert!(admit(proposal, &fresh, stamp(12)).is_ok());
}

#[test]
fn missing_and_denied_are_unavailable_and_never_overridden_by_explicit_selection() {
    for (folder, expected) in [
        (FolderObservation::Missing, Unavailable::FolderMissing),
        (
            FolderObservation::PermissionDenied,
            Unavailable::PermissionDenied,
        ),
    ] {
        let mut observation = mount("attach-1", "/media/usb-a");
        observation.folder = folder;
        let rows = [observation];
        let result = assess(
            &source(),
            &snapshot(10, &rows),
            Selection::Explicit {
                incarnation: "attach-1",
            },
            None,
        )
        .unwrap();
        assert!(matches!(result, Assessment::Unavailable(reason) if reason == expected));
    }
}

#[test]
fn incomplete_inventory_cannot_prove_unique_volume_or_continue_a_grant() {
    let src = source();
    let rows = [mount("attach-1", "/media/usb-a")];
    let old = binding(&src, &rows);
    let incomplete = InventorySnapshot {
        generation: 11,
        complete: false,
        mounts: &rows,
    };
    assert!(matches!(
        assess(
            &src,
            &incomplete,
            Selection::Automatic,
            Some(PreviousBinding {
                binding: &old,
                retired: false
            })
        )
        .unwrap(),
        Assessment::Unavailable(Unavailable::IncompleteInventory)
    ));
}

#[test]
fn changed_incarnation_requires_new_generation_even_when_path_and_uuid_match() {
    let src = source();
    let old = binding(&src, &[mount("attach-1", "/media/usb-a")]);
    let rows = [mount("attach-2", "/media/usb-a")];
    assert!(matches!(
        assess(
            &src,
            &snapshot(10, &rows),
            Selection::Automatic,
            Some(PreviousBinding {
                binding: &old,
                retired: false
            })
        ),
        Err(PolicyError::StaleEvidence)
    ));
    assert!(matches!(
        assess(
            &src,
            &snapshot(9, &rows),
            Selection::Automatic,
            Some(PreviousBinding {
                binding: &old,
                retired: false
            })
        ),
        Err(PolicyError::StaleEvidence)
    ));
}

#[test]
fn native_admission_rejects_changed_or_wrong_generation_revalidation() {
    let src = source();
    let rows = [mount("attach-1", "/media/usb-a")];
    let initial = snapshot(10, &rows);
    let prepare = || plan(assess(&src, &initial, Selection::Automatic, None).unwrap());
    assert!(matches!(
        admit(prepare(), &snapshot(11, &rows), stamp(11)),
        Err(PolicyError::StaleEvidence)
    ));
    let changed = [mount("attach-2", "/media/usb-a")];
    assert!(matches!(
        admit(prepare(), &snapshot(10, &changed), stamp(10)),
        Err(PolicyError::ChangedEvidence)
    ));
    assert!(matches!(
        admit(prepare(), &initial, stamp(9)),
        Err(PolicyError::InvalidGrant)
    ));
    assert!(matches!(
        admit(
            prepare(),
            &initial,
            GrantStamp {
                authority_ref: AuthorityRef(0),
                generation: 10
            }
        ),
        Err(PolicyError::InvalidGrant)
    ));
}

#[test]
fn exact_folder_containment_rejects_sibling_prefix_and_canonical_escape() {
    for bad in [
        "/media/usb-a/Videos/Family2",
        "/other/Family",
        "/media/usb-a/Videos/Family/../Other",
    ] {
        let mut row = mount("attach-1", "/media/usb-a");
        row.folder = FolderObservation::Readable {
            canonical_path: bad.into(),
        };
        assert!(matches!(
            assess(&source(), &snapshot(10, &[row]), Selection::Automatic, None),
            Err(PolicyError::InvalidEvidence)
        ));
    }
    for bad in [
        "../Family",
        "/Family",
        "Videos//Family",
        "Videos/./Family",
        "Videos\\Family",
    ] {
        let mut src = source();
        src.relative_folder = bad.into();
        assert!(matches!(
            assess(&src, &snapshot(10, &[]), Selection::Automatic, None),
            Err(PolicyError::InvalidEvidence)
        ));
    }
}

#[test]
fn nonascii_folder_and_volume_root_are_exact_without_path_normalization() {
    let mut src = source();
    src.relative_folder = "Домашнее/Семья".into();
    let mut row = mount("attach-1", "/media/usb-a");
    row.relative_folder.clone_from(&src.relative_folder);
    row.folder = FolderObservation::Readable {
        canonical_path: "/media/usb-a/Домашнее/Семья".into(),
    };
    assert_eq!(
        binding(&src, &[row]).canonical_path(),
        "/media/usb-a/Домашнее/Семья"
    );
    src.relative_folder.clear();
    let mut row = mount("attach-1", "/media/usb-a");
    row.relative_folder.clear();
    row.folder = FolderObservation::Readable {
        canonical_path: "/media/usb-a".into(),
    };
    assert_eq!(binding(&src, &[row]).canonical_path(), "/media/usb-a");
}

#[test]
fn snapshot_bounds_and_duplicate_incarnation_fail_closed() {
    let row = mount("attach-1", "/media/usb-a");
    assert!(matches!(
        assess(
            &source(),
            &snapshot(10, &[row.clone(), row.clone()]),
            Selection::Automatic,
            None
        ),
        Err(PolicyError::InvalidEvidence)
    ));
    let rows = vec![row; OBSERVATION_LIMIT + 1];
    assert!(matches!(
        assess(&source(), &snapshot(10, &rows), Selection::Automatic, None),
        Err(PolicyError::InvalidEvidence)
    ));
}

#[test]
fn revoked_grant_never_revives_even_when_the_observation_looks_unchanged() {
    let src = source();
    let rows = [mount("attach-1", "/media/usb-a")];
    let old = binding(&src, &rows);
    let retired = Some(PreviousBinding {
        binding: &old,
        retired: true,
    });
    assert!(matches!(
        assess(&src, &snapshot(10, &rows), Selection::Automatic, retired),
        Err(PolicyError::StaleEvidence)
    ));
    let fresh = snapshot(11, &rows);
    let proposal = plan(assess(&src, &fresh, Selection::Automatic, retired).unwrap());
    let new = admit(proposal, &fresh, stamp(11)).unwrap();
    assert_eq!(new.grant().generation, 11);
    let mut unknown = src;
    unknown.volume = None;
    let old = binding(&unknown, &rows);
    assert!(matches!(
        assess(
            &unknown,
            &fresh,
            Selection::Automatic,
            Some(PreviousBinding {
                binding: &old,
                retired: true
            })
        )
        .unwrap(),
        Assessment::Unavailable(Unavailable::UnknownIdentity)
    ));
}

#[test]
fn reattachment_cannot_change_the_existing_inventory_slot() {
    let src = source();
    let old = binding(&src, &[mount("attach-1", "/media/usb-a")]);
    let rows = [mount("attach-2", "/media/usb-b")];
    let fresh = snapshot(11, &rows);
    let proposal = plan(
        assess(
            &src,
            &fresh,
            Selection::Automatic,
            Some(PreviousBinding {
                binding: &old,
                retired: true,
            }),
        )
        .unwrap(),
    );
    assert!(matches!(
        admit(
            proposal,
            &fresh,
            GrantStamp {
                authority_ref: AuthorityRef(4),
                generation: 11
            }
        ),
        Err(PolicyError::InvalidGrant)
    ));
}

#[test]
fn final_admission_rechecks_completeness_and_clone_ambiguity() {
    let src = source();
    let rows = [mount("attach-1", "/media/usb-a")];
    let initial = snapshot(10, &rows);
    let prepare = || plan(assess(&src, &initial, Selection::Automatic, None).unwrap());
    let incomplete = InventorySnapshot {
        generation: 10,
        complete: false,
        mounts: &rows,
    };
    assert!(matches!(
        admit(prepare(), &incomplete, stamp(10)),
        Err(PolicyError::ChangedEvidence)
    ));
    let duplicates = [rows[0].clone(), mount("attach-2", "/media/usb-b")];
    assert!(matches!(
        admit(prepare(), &snapshot(10, &duplicates), stamp(10)),
        Err(PolicyError::ChangedEvidence)
    ));
}
