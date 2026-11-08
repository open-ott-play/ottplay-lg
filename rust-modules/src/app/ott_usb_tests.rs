use super::*;
use crate::app::ott_local_session::AuthorityRef;
use std::fs;
use std::os::unix::fs::symlink;
use std::sync::atomic::AtomicU64;

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ott-usb-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn mount(&self, id: u64) -> MountCandidate {
        MountCandidate::directory_for_test(self.0.to_str().unwrap(), id).unwrap()
    }
    fn descriptor(&self) -> BoundSource {
        BoundSource {
            id: "usb-root".into(),
            name: "USB videos".into(),
            kind: SourceKind::Folder,
            available: false,
            canonical_path: None,
            authority_ref: AuthorityRef(3),
            generation: 9,
        }
    }
    fn task(&self, relative: &str) -> Task {
        Task::Select {
            choice: Choice {
                mount: self.mount(8),
                readable: true,
            },
            source: self.descriptor(),
            relative_folder: relative.into(),
        }
    }
    fn select(&self, relative: &str) -> LocalSource {
        let Answer::Selected(source) = execute_with(
            &OffFrame::for_test(),
            self.task(relative),
            &AtomicBool::new(false),
            &mut |_, _| Ok(vec![self.mount(8)]),
        )
        .unwrap() else {
            panic!("selected source")
        };
        source
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn discovery_yields_candidates_without_acquiring_roots() {
    let f = Fixture::new();
    let Answer::Choices { choices, observed } = execute_with(
        &OffFrame::for_test(),
        Task::Discover { known: vec![] },
        &AtomicBool::new(false),
        &mut |_, _| Ok(vec![f.mount(8)]),
    )
    .unwrap() else {
        panic!("discovery")
    };
    assert_eq!(choices.len(), 1);
    assert!(choices[0].readable());
    assert!(observed.is_empty());
    assert!(!choices[0].label(0).contains(f.0.to_str().unwrap()));
}

#[test]
fn explicit_selection_pins_exact_deep_unicode_folder_and_only_it() {
    let f = Fixture::new();
    fs::create_dir_all(f.0.join("Фильмы/夏の映画")).unwrap();
    let source = f.select("Фильмы/夏の映画");
    assert_eq!(
        source.descriptor().canonical_path.as_deref(),
        f.0.join("Фильмы/夏の映画").to_str()
    );
    assert!(source.root_handle().is_some());
    assert!(source
        .usb_witness()
        .unwrap()
        .is_observed(&[f.mount(8)], source.descriptor()));
    assert!(!source
        .usb_witness()
        .unwrap()
        .is_observed(&[f.mount(9)], source.descriptor()));
}

#[test]
fn missing_subfolder_never_falls_back_to_volume_root() {
    let f = Fixture::new();
    assert!(matches!(
        execute_with(
            &OffFrame::for_test(),
            f.task("Missing"),
            &AtomicBool::new(false),
            &mut |_, _| Ok(vec![f.mount(8)])
        ),
        Err(Error::MissingFolder)
    ));
}

#[test]
fn escapes_and_symlink_components_cannot_be_selected() {
    let f = Fixture::new();
    let outside = Fixture::new();
    symlink(&outside.0, f.0.join("Link")).unwrap();
    for path in ["../outside", "/absolute", "a//b", "Link", "Link/Movies"] {
        assert!(
            execute_with(
                &OffFrame::for_test(),
                f.task(path),
                &AtomicBool::new(false),
                &mut |_, _| Ok(vec![f.mount(8)])
            )
            .is_err(),
            "{path}"
        );
    }
}

#[test]
fn removed_or_replaced_mount_between_observation_and_open_is_refused() {
    let f = Fixture::new();
    let mut calls = 0;
    let result = execute_with(
        &OffFrame::for_test(),
        f.task(""),
        &AtomicBool::new(false),
        &mut |_, _| {
            calls += 1;
            Ok(vec![f.mount(if calls == 1 { 8 } else { 9 })])
        },
    );
    assert!(matches!(result, Err(Error::Changed)));
    assert_eq!(calls, 2);
}

#[test]
fn same_mount_table_and_path_cannot_hide_replaced_directory_after_open() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("Movies")).unwrap();
    let mut calls = 0;
    let result = execute_with(
        &OffFrame::for_test(),
        f.task("Movies"),
        &AtomicBool::new(false),
        &mut |_, _| {
            calls += 1;
            if calls == 2 {
                fs::rename(f.0.join("Movies"), f.0.join("Old")).unwrap();
                fs::create_dir(f.0.join("Movies")).unwrap();
            }
            Ok(vec![f.mount(8)])
        },
    );
    assert!(matches!(result, Err(Error::Changed)));
}

#[test]
fn late_cancel_never_returns_a_root_grant() {
    let f = Fixture::new();
    let cancel = AtomicBool::new(false);
    let mut calls = 0;
    let result = execute_with(
        &OffFrame::for_test(),
        f.task(""),
        &cancel,
        &mut |_, cancel| {
            calls += 1;
            if calls == 2 {
                cancel.store(true, Ordering::Release);
            }
            Ok(vec![f.mount(8)])
        },
    );
    assert!(matches!(result, Err(Error::Cancelled)));
}

#[test]
fn removed_volume_is_only_an_observation_until_current_inventory_consumes_it() {
    let f = Fixture::new();
    let source = f.select("");
    let held = source.root_handle().unwrap().clone();
    let Answer::Choices { observed, .. } = execute_with(
        &OffFrame::for_test(),
        Task::Discover {
            known: vec![source],
        },
        &AtomicBool::new(false),
        &mut |_, _| Ok(vec![]),
    )
    .unwrap() else {
        panic!("discovery")
    };
    assert!(observed[0].root_handle().is_none());
    assert!(held.is_live(), "worker must not revoke shared grant");
    drop(observed);
    assert!(
        held.is_live(),
        "discarding stale result must not revoke shared grant"
    );
}

#[test]
fn observation_failure_is_not_a_successful_empty_inventory() {
    let f = Fixture::new();
    let source = f.select("");
    let held = source.root_handle().unwrap().clone();
    let result = execute_with(
        &OffFrame::for_test(),
        Task::Discover {
            known: vec![source],
        },
        &AtomicBool::new(false),
        &mut |_, _| Err(Error::Unavailable),
    );
    assert!(matches!(result, Err(Error::Unavailable)));
    assert!(held.is_live());
}

#[test]
fn same_live_witness_survives_refresh_without_changing_its_root_arc() {
    let f = Fixture::new();
    let source = f.select("");
    let held = source.root_handle().unwrap().clone();
    let Answer::Choices { observed, .. } = execute_with(
        &OffFrame::for_test(),
        Task::Discover {
            known: vec![source],
        },
        &AtomicBool::new(false),
        &mut |_, _| Ok(vec![f.mount(8)]),
    )
    .unwrap() else {
        panic!("discovery")
    };
    assert!(std::sync::Arc::ptr_eq(
        &held,
        observed[0].root_handle().unwrap()
    ));
}

#[test]
fn saved_path_layout_keeps_exact_relative_folder_without_becoming_authority() {
    assert_eq!(
        relative_folder("/tmp/usb/sda/sda1/Фильмы/夏"),
        Some("Фильмы/夏".into())
    );
    assert_eq!(relative_folder("/tmp/usb/sdz/sdz4"), Some(String::new()));
    for path in [
        "/tmp/usb",
        "/tmp/usb/a",
        "/tmp/usb/a/b/../x",
        "/tmp/usb/a/b//x",
    ] {
        assert_eq!(relative_folder(path), None);
    }
}

#[test]
fn overmounted_path_is_ambiguous_even_when_one_mount_id_still_matches() {
    let f = Fixture::new();
    let result = execute_with(
        &OffFrame::for_test(),
        f.task(""),
        &AtomicBool::new(false),
        &mut |_, _| Ok(vec![f.mount(8), f.mount(9)]),
    );
    assert!(matches!(result, Err(Error::Changed)));
}


#[test]
fn portable_saved_folder_maps_against_exact_selected_mount_root() {
    let mut rows = removable::candidates(
        b"7 1 8:1 /Movies /tmp/usb/disk/part rw - ext4 /dev/example rw\n").unwrap();
    let choice = Choice { mount: rows.remove(0), readable: true };
    let binding = UsbBinding::from_observed("ext4", "fixture-uuid", "/", "Movies/夏").unwrap();
    assert_eq!(choice.saved_relative_folder(&binding).unwrap(), "夏");
    let outside = UsbBinding::from_observed("ext4", "fixture-uuid", "/", "Other/夏").unwrap();
    assert!(choice.saved_relative_folder(&outside).is_err(), "never substitute the mount root");
}
