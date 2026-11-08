use super::*;
use crate::app::ott_local_session::AuthorityRef;
use std::fs;
use std::sync::atomic::AtomicU64;
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "usb-browser-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        let p = fs::canonicalize(p).unwrap();
        fs::create_dir(p.join("drive")).unwrap();
        Self(p)
    }
    fn root(&self) -> PathBuf {
        self.0.join("drive")
    }
    fn mount(&self) -> MountCandidate {
        let file = File::open(self.root()).unwrap();
        let device = identity::directory_device(&OffFrame::for_test(), &file).unwrap();
        MountCandidate::directory_device_for_test(self.root().to_str().unwrap(), 8, device).unwrap()
    }
    fn descriptor(&self, generation: u64) -> BoundSource {
        BoundSource {
            id: "chosen".into(),
            name: "Movies".into(),
            kind: SourceKind::Folder,
            available: false,
            canonical_path: None,
            authority_ref: AuthorityRef(1),
            generation,
        }
    }
    fn run(&self, task: BrowseTask) -> Result<Answer, Error> {
        super::super::execute_with_identity(
            &OffFrame::for_test(),
            Task::Browse(task),
            &AtomicBool::new(false),
            &mut |_, _| Ok(vec![self.mount()]),
            &mut |_, _, _| Err(IdentityError::Unsupported),
        )
    }
    fn open(&self, relative: &str) -> Result<BrowsePage, Error> {
        let task = BrowseTask::Open {
            choice: Choice {
                mount: self.mount(),
                readable: true,
            },
            source: self.descriptor(7),
            initial_relative: relative.into(),
        };
        match self.run(task)? {
            Answer::Browse(page) => Ok(page),
            _ => panic!("browse result"),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        plx_base::testscratch::remove_tree(&self.0);
    }
}
fn page(answer: Answer) -> BrowsePage {
    match answer {
        Answer::Browse(p) => p,
        _ => panic!("page"),
    }
}

#[test]
fn enter_parent_root_and_confirm_use_only_the_chosen_directory() {
    let f = Fixture::new();
    fs::create_dir_all(f.root().join("Movies/Season")).unwrap();
    fs::write(f.root().join("ignored.mp4"), b"synthetic").unwrap();
    let root = f.open("").unwrap();
    assert_eq!(root.folders(), ["Movies"]);
    let movies = page(f.run(root.open_child(0).unwrap()).unwrap());
    assert_eq!(movies.relative(), "Movies");
    let season = page(f.run(movies.open_child(0).unwrap()).unwrap());
    assert_eq!(season.relative(), "Movies/Season");
    assert_eq!(page(f.run(season.parent()).unwrap()).relative(), "Movies");
    assert!(page(f.run(season.root()).unwrap()).is_root());
    let Answer::Selected(selected) = f.run(season.confirm(f.descriptor(8))).unwrap() else {
        panic!("explicit selection")
    };
    assert_eq!(
        selected.descriptor().canonical_path.as_deref(),
        f.root().join("Movies/Season").to_str()
    );
    assert_eq!(selected.descriptor().generation, 8);
    assert!(selected.is_live());
    assert!(
        !selected.same_grant(&season.location.root),
        "browser root is not the accepted folder grant"
    );
}

#[test]
fn missing_retained_folder_never_silently_opens_or_selects_root() {
    let f = Fixture::new();
    assert!(f.open("missing").is_err());
    assert!(
        f.open("").unwrap().is_root(),
        "root requires a separate explicit request"
    );
}

#[test]
fn symlinks_and_parent_escape_are_not_folder_grants() {
    let f = Fixture::new();
    let other = Fixture::new();
    std::os::unix::fs::symlink(other.root(), f.root().join("linked")).unwrap();
    fs::create_dir(f.root().join("inside")).unwrap();
    let root = f.open("").unwrap();
    assert_eq!(root.folders(), ["inside"]);
    for relative in ["../drive", "linked", "/", "inside/../inside"] {
        assert!(f
            .run(BrowseTask::Read {
                location: root.location.clone(),
                relative: relative.into(),
                entries: FIRST_ENTRIES
            })
            .is_err());
    }
}

#[test]
fn confirmation_rejects_replaced_displayed_folder_and_replaced_mount_root() {
    let f = Fixture::new();
    fs::create_dir(f.root().join("Movies")).unwrap();
    let movies = f.open("Movies").unwrap();
    fs::rename(f.root().join("Movies"), f.root().join("old-Movies")).unwrap();
    fs::create_dir(f.root().join("Movies")).unwrap();
    assert!(matches!(
        f.run(movies.confirm(f.descriptor(8))),
        Err(Error::Changed)
    ));
    let root = f.open("").unwrap();
    fs::rename(f.root(), f.0.join("old-drive")).unwrap();
    fs::create_dir(f.root()).unwrap();
    assert!(matches!(
        f.run(root.confirm(f.descriptor(9))),
        Err(Error::Changed)
    ));
    assert!(
        root.is_live(),
        "worker reports loss without revoking another holder"
    );
}

#[test]
fn explicit_load_more_reaches_a_shallow_directory_with_over_ten_thousand_files() {
    let f = Fixture::new();
    for i in 0..10_001 {
        fs::write(f.root().join(format!("file-{i}.mp4")), b"").unwrap();
    }
    fs::create_dir(f.root().join("Wanted folder")).unwrap();
    let mut current = f.open("").unwrap();
    assert!(!current.reached_end && current.can_more());
    let mut passes = 1;
    while let Some(more) = current.more() {
        let next = page(f.run(more).unwrap());
        assert!(next.allowance > current.allowance);
        assert!(next.root_generation() == current.root_generation());
        current = next;
        passes += 1;
    }
    assert!(current.reached_end && current.complete);
    assert_eq!(current.folders(), ["Wanted folder"]);
    assert_eq!(passes, 3);
}

#[test]
fn cancellation_and_revocation_reject_page_without_mutating_its_prior_snapshot() {
    let f = Fixture::new();
    fs::create_dir(f.root().join("Movies")).unwrap();
    let mut current = f.open("").unwrap();
    let cancel = AtomicBool::new(false);
    let result = super::super::execute_with_identity(
        &OffFrame::for_test(),
        Task::Browse(current.refresh()),
        &cancel,
        &mut |_, _| {
            cancel.store(true, Ordering::Release);
            Ok(vec![f.mount()])
        },
        &mut |_, _, _| Err(IdentityError::Unsupported),
    );
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!(current.folders(), ["Movies"]);
    assert!(current.is_live());
    let late = current.refresh();
    current.retire();
    assert!(f.run(late).is_err());
}

#[test]
fn folder_memory_limit_is_visible_and_does_not_claim_complete() {
    let f = Fixture::new();
    for i in 0..FOLDER_LIMIT + 1 {
        fs::create_dir(f.root().join(format!("folder-{i}"))).unwrap();
    }
    let first = f.open("").unwrap();
    let full = page(f.run(first.more().unwrap()).unwrap());
    assert_eq!(full.folders().len(), FOLDER_LIMIT);
    assert!(full.memory_limited && !full.complete && !full.can_more());
    assert!(full.status().contains("limit reached"));
}

#[test]
fn confirmation_rejects_a_different_identity_even_if_outer_observations_return_to_original() {
    let f = Fixture::new();
    fs::create_dir(f.root().join("Movies")).unwrap();
    let mount = f.mount();
    let identity = |uuid| {
        IdentityObservation::for_test(
            mount.clone(),
            identity::StableIdentity::new(uuid, mount.filesystem()).unwrap(),
        )
    };
    let a = identity("fixture-volume-A");
    let b = identity("fixture-volume-B");
    let opened = super::super::execute_with_identity(
        &OffFrame::for_test(),
        Task::Browse(BrowseTask::Open {
            choice: Choice {
                mount: mount.clone(),
                readable: true,
            },
            source: f.descriptor(7),
            initial_relative: "Movies".into(),
        }),
        &AtomicBool::new(false),
        &mut |_, _| Ok(vec![mount.clone()]),
        &mut |_, _, _| Ok(vec![a.clone()]),
    )
    .unwrap();
    let page = page(opened);
    // A stable child confirmation succeeds; comparing root-relative and child
    // bindings directly would incorrectly reject this control.
    assert!(matches!(
        super::super::execute_with_identity(
            &OffFrame::for_test(),
            Task::Browse(page.confirm(f.descriptor(8))),
            &AtomicBool::new(false),
            &mut |_, _| Ok(vec![mount.clone()]),
            &mut |_, _, _| Ok(vec![a.clone()]),
        ),
        Ok(Answer::Selected(_))
    ));
    let mut calls = 0;
    let result = super::super::execute_with_identity(
        &OffFrame::for_test(),
        Task::Browse(page.confirm(f.descriptor(9))),
        &AtomicBool::new(false),
        &mut |_, _| Ok(vec![mount.clone()]),
        &mut |_, _, _| {
            calls += 1;
            // Original held-root check, then Select's before/after pair. A
            // weaker final-only check sees A again and would accept B's proof.
            Ok(vec![if calls == 2 || calls == 3 {
                b.clone()
            } else {
                a.clone()
            }])
        },
    );
    assert!(matches!(result, Err(Error::Changed)));
    assert_eq!(
        calls, 3,
        "reject the inconsistent selected proof before final observation"
    );
}
