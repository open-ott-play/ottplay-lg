//! USB selection is an addressed operation on the existing Local inventory and
//! catalogue slot. A candidate list never grants file access or changes profiles.
use super::*;
use super::ui_projection::row;
use super::work_lifecycle::RestoreStartError;
use crate::app::ott_local_session::{AuthorityRef, BoundSource, SourceKind};
use crate::app::ott_local_sources::{FolderDescriptor, LocalSourceKind, RememberedFolders};
use crate::app::ott_usb as usb;
#[path = "ott_usb_browser_runtime.rs"]
mod browser;
#[path = "ott_usb_remote.rs"]
mod remote;
#[path = "ott_usb_delayed_mount.rs"]
mod delayed_mount;
#[path = "ott_usb_initial_restore.rs"]
mod initial_restore;

#[derive(Default)]
pub(super) struct State {
    remote: remote::RemoteState,
    generation: u64,
    navigation: u64,
    playback: u64,
    ticket: Option<plx_media::route::PlayRequestTicket>,
    automatic: bool,
    delayed_mount: Option<delayed_mount::Retry>,
    initial_restore: initial_restore::Phase,
    choices: Vec<usb::Choice>,
    browser_choice: Option<usb::Choice>,
    browser: Option<usb::BrowsePage>,
    browser_page: usize,
    pending: Option<usb::Task>,
    waiting: bool,
    selecting: bool,
    target: Option<Target>,
    interrupted: Option<transient::InterruptedScan>,
    error: Option<&'static str>,
    install: Option<Install>,
}
struct Install {
    target: Target,
    source: crate::app::ott_local_root::LocalSource,
    relocation: Option<usb::VerifiedRelocation>,
    checkpoint: Option<usb_resume::CheckpointRebase>,
}
#[derive(Clone)]
struct Target {
    // A kiosk target owns only the remembered root. Its profile is a display
    // projection and is never inserted into or selected in Saved profiles.
    kiosk: Option<crate::app::ott_kiosk_stored::Stored>,
    previous: Option<ProviderProfile>,
    profile: ProviderProfile,
    folders: RememberedFolders,
    source_id: String,
    authority: AuthorityRef,
    relative_folder: String,
}
impl Target {
    fn descriptor(&self, generation: u64, relative: &str) -> BoundSource {
        BoundSource {
            id: self.source_id.clone(),
            name: self.folders.folders().iter().find(|folder| folder.id == self.source_id)
                .map(|folder| folder.name.clone()).unwrap_or_else(||
                    relative.rsplit('/').next().filter(|name| !name.is_empty())
                        .unwrap_or("USB videos").to_owned()),
            kind: SourceKind::Folder, available: false, canonical_path: None,
            authority_ref: self.authority, generation,
        }
    }
    fn browse_relative_folder(&self, choice: &usb::Choice) -> Result<String, &'static str> {
        let binding = self.previous.as_ref().and_then(|profile| {
            self.folders.folders().iter().find(|folder|
                folder.id == self.source_id && folder.path == profile.source)
        }).and_then(|folder| folder.usb.as_ref());
        match binding {
            Some(binding) => choice.saved_relative_folder(binding),
            None => Ok(self.relative_folder.clone()),
        }
    }
    fn current(&self, runtime: &Runtime) -> bool {
        (self.kiosk.as_ref().map_or_else(|| self.previous.as_ref()
            == runtime
                .saved
                .profiles
                .profiles
                .iter()
                .find(|profile| profile.id == self.profile.id),
            |policy| crate::app::ott_kiosk_stored::read(&runtime.saved.settings)
                .ok().flatten().as_ref() == Some(policy)))
            && RememberedFolders::read(&runtime.saved.settings)
                .is_ok_and(|folders| folders == self.folders)
    }
}

impl Runtime {
    /// Saved intent routes verification; it never supplies a live USB grant.
    /// Invalid or ambiguous metadata is an error, not an ordinary-path fallback.
    pub(super) fn usb_folder_path(&self, path: &str) -> Result<bool, &'static str> {
        let folders = RememberedFolders::read(&self.saved.settings)?;
        Ok(usb::is_usb_path(path) || folders.folders().iter().any(|folder|
            folder.kind == LocalSourceKind::Folder && folder.path == path && folder.usb.is_some()))
    }
    fn usb_target(&self) -> Result<Target, &'static str> {
        let previous = self
            .profile_focus
            .as_ref()
            .and_then(|id| {
                self.saved
                    .profiles
                    .profiles
                    .iter()
                    .find(|profile| &profile.id == id)
            })
            .or_else(|| self.active())
            .filter(|profile| {
                profile.kind == ProviderKind::Local
                    && self.usb_folder_path(&profile.source).unwrap_or(false)
            })
            .cloned();
        self.usb_target_for(previous)
    }

    fn usb_target_for(&self, previous: Option<ProviderProfile>) -> Result<Target, &'static str> {
        let folders = RememberedFolders::read(&self.saved.settings)?;
        if previous.as_ref().is_some_and(|profile|
            self.usb_path_is_single_file(&profile.source, &folders))
        {
            return Err("A single file cannot be selected as a USB folder");
        }
        let profile = if let Some(profile) = &previous {
            profile.clone()
        } else {
            let index = (1..=self.saved.profiles.profiles.len()
                + self.local_inventory.sources.len()
                + folders.folders().len()
                + 1)
                .find(|index| {
                    let id = format!("local-usb-{index}");
                    !self
                        .saved
                        .profiles
                        .profiles
                        .iter()
                        .any(|profile| profile.id == id)
                        && !self
                            .local_inventory
                            .sources
                            .iter()
                            .any(|source| source.descriptor().id == id)
                        && !folders.folders().iter().any(|source| source.id == id)
                })
                .ok_or("No Local profile slot is available")?;
            ProviderProfile {
                id: format!("local-usb-{index}"),
                name: format!("USB {index}"),
                kind: ProviderKind::Local,
                source: String::new(),
                source_alias: format!("USB {index}"),
                enabled: true,
            }
        };
        let descriptor = previous.as_ref().and_then(|profile| {
            folders
                .folders()
                .iter()
                .find(|folder| folder.path == profile.source)
        });
        // Bound sources retain filesystem-relative intent even if the saved
        // mount path is not under /tmp/usb. Restore verifies its exact identity;
        // manual browsing converts it against the chosen mount's root below.
        let relative_folder = previous.as_ref().map(|profile| {
            descriptor.and_then(|folder| folder.usb.as_ref())
                .map(|binding| binding.filesystem_relative_folder.clone())
                .or_else(|| usb::relative_folder(&profile.source))
                .ok_or("Invalid saved USB folder")
        }).transpose()?.unwrap_or_default();
        let source_id = descriptor
            .map(|folder| folder.id.clone())
            .unwrap_or_else(|| profile.id.clone());
        let authority = self
            .local_inventory
            .sources
            .iter()
            .find(|source| source.descriptor().id == source_id)
            .map(|source| source.descriptor().authority_ref)
            .unwrap_or_else(|| {
                AuthorityRef(
                    (1..=self.local_inventory.sources.len() as u64 + 1)
                        .find(|id| {
                            !self
                                .local_inventory
                                .sources
                                .iter()
                                .any(|source| source.descriptor().authority_ref.0 == *id)
                        })
                        .unwrap(),
                )
            });
        Ok(Target {
            kiosk: None,
            previous,
            profile,
            folders,
            source_id,
            authority,
            relative_folder,
        })
    }

    /// A File descriptor only excludes the folder workflow; it never grants
    /// access. The ordinary Local worker still validates the exact held file.
    /// Include unavailable entries so a lost file cannot become its parent root.
    fn usb_path_is_single_file(&self, path: &str, folders: &RememberedFolders) -> bool {
        folders.folders().iter().any(|source|
            source.kind == LocalSourceKind::File && source.path == path)
            || self.local_inventory.sources.iter().any(|source| {
                let source = source.descriptor();
                source.kind == SourceKind::File
                    && source.canonical_path.as_deref() == Some(path)
            })
    }

    fn usb_begin(
        &mut self,
        task: usb::Task,
        target: Target,
        selecting: bool,
    ) -> Result<(), &'static str> {
        self.usb_begin_mode(task, target, selecting, false)?;
        self.launch_catalog();
        Ok(())
    }

    fn usb_begin_mode(
        &mut self, task: usb::Task, target: Target, selecting: bool, automatic: bool,
    ) -> Result<(), &'static str> {
        if !self.loaded
            || self.persistence_uncertain
            || self.saving
            || self.transient.candidate.is_some()
        {
            return Err("Finish the current source operation before selecting USB");
        }
        if self.local_inventory.usb.remote.busy() { return Err("USB browser is busy"); }
        self.local_inventory.usb.remote.clear_view();
        // Runtime stores ClockSample::steady_from(first_frame) before handling
        // Loaded and admitting the initial catalogue. No frame means no timer.
        let delayed_mount = (automatic && target.kiosk.is_none()
            && matches!(&task, usb::Task::Restore { binding, .. } if binding.validate().is_ok()))
            .then(|| self.clock_sample.map(|sample| delayed_mount::Retry::start(sample.unix_ms())))
            .flatten();
        if !automatic {
            self.cancel_setup();
            self.accepted_navigation(false);
        }
        let interrupted = transient::InterruptedScan {
            profile: self.effective_profile().cloned(),
            owner: self
                .transient
                .accepted
                .as_ref()
                .map(transient::TransientMedia::owner),
            restart: self.catalog_loading && !self.catalog_complete,
        };
        self.cancel.store(true, Ordering::Release);
        let classic_generation = self.generation;
        self.generation = self.generation.wrapping_add(1).max(1);
        classic_source::rebase(self, classic_generation);
        self.continuity.rebase(self.generation);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.catalog_loading = false;
        self.catalog_next = None;
        let remote = std::mem::take(&mut self.local_inventory.usb.remote);
        self.local_inventory.usb = State {
            remote,
            generation: self.generation,
            navigation: self.navigation_epoch,
            playback: self.playback_epoch,
            ticket: Some(plx_media::route::play_request_ticket()),
            automatic,
            delayed_mount,
            pending: Some(task),
            waiting: true,
            selecting,
            target: Some(target),
            interrupted: Some(interrupted),
            ..Default::default()
        };
        if !automatic {
            self.provider = ProviderKind::Local;
            self.page = "profiles";
        }
        self.status = if automatic {
            "Reconnecting saved USB folder…"
        } else if selecting {
            "Checking selected USB folder…"
        } else {
            "Looking for USB drives…"
        }
        .into();
        self.changed();
        Ok(())
    }

    fn usb_remote_ui_picker_active(&self) -> bool {
        let state = &self.local_inventory.usb;
        state.browser_choice.is_some() || (!state.automatic
            && self.provider == ProviderKind::Local && self.page == "profiles"
            && self.usb_context_current()
            && state.target.as_ref().is_some_and(|target| target.current(self))
            && (!state.choices.is_empty() || !state.selecting))
    }

    fn usb_context_current(&self) -> bool {
        let state = &self.local_inventory.usb;
        state.generation == self.generation && state.navigation == self.navigation_epoch
            && (!state.automatic || (state.playback == self.playback_epoch
                && state.ticket.is_some_and(plx_media::route::play_request_is_current)
                && state.target.as_ref().is_some_and(|target| {
                    if target.kiosk.is_some() { target.current(self) }
                    else { self.effective_profile() == target.previous.as_ref() }
                })))
    }

    pub(super) fn usb_action(&mut self, app: &App, key: &str) -> Option<Result<(), &'static str>> {
        if let Some(result) = self.usb_browser_action(app, key) { return Some(result); }
        if key != "local.usb" && !key.starts_with("local.usb.select:") {
            return None;
        }
        Some((|| {
            if !playback_allowed(app) || !self.source_work_allowed() {
                return Err("USB selection is unavailable while playback is suspended");
            }
            if key == "local.usb" {
                let target = self.usb_target()?;
                return self.usb_begin(
                    usb::Task::Discover {
                        known: self.local_inventory.sources.clone(),
                    },
                    target,
                    false,
                );
            }
            let fields: Vec<_> = key
                .trim_start_matches("local.usb.select:")
                .split(':')
                .collect();
            if fields.len() != 2 {
                return Err("Invalid USB selection");
            }
            let generation: u64 = fields[0].parse().map_err(|_| "Invalid USB selection")?;
            let index: usize = fields[1].parse().map_err(|_| "Invalid USB selection")?;
            let state = &self.local_inventory.usb;
            if state.waiting
                || state.generation != generation
                || generation != self.generation
                || state.navigation != self.navigation_epoch
            {
                return Err("USB list changed; refresh and select again");
            }
            let choice = state
                .choices
                .get(index)
                .filter(|choice| choice.readable())
                .ok_or("USB drive is unavailable")?
                .clone();
            let target = state
                .target
                .as_ref()
                .filter(|target| target.current(self))
                .ok_or("Local profile changed; refresh USB drives")?
                .clone();
            let relative = target.browse_relative_folder(&choice)?;
            let source = target.descriptor(self.generation.wrapping_add(1).max(1), &relative);
            let task = usb::Task::Browse(usb::BrowseTask::Open {
                choice: choice.clone(), source, initial_relative: relative,
            });
            let choices = self.local_inventory.usb.choices.clone();
            self.usb_begin(task, target, false)?;
            self.local_inventory.usb.choices = choices;
            self.local_inventory.usb.browser_choice = Some(choice);
            self.page = "usb-folders";
            self.changed();
            Ok(())
        })())
    }

    pub(super) fn take_usb_task(&mut self) -> Option<ott_catalog::Task> {
        if !self.usb_context_current()
            || self.cancel.load(Ordering::Acquire)
        {
            self.retire_usb_work();
            return None;
        }
        self.local_inventory
            .usb
            .pending
            .take()
            .map(ott_catalog::Task::Usb)
    }

    fn retire_usb_work(&mut self) {
        // A current failed cold write remains a terminal selected attempt, not
        // permission to fall through to unrelated all-root validation.
        if self.local_inventory.usb.initial_restore != initial_restore::Phase::Owned
            || !self.initial_usb_context_current()
        {
            let state = &mut self.local_inventory.usb;
            state.initial_restore = if state.initial_restore != initial_restore::Phase::None
                && state.install.is_some()
            {
                // Keep stale ACK suppression even if the old target later matches again.
                initial_restore::Phase::Retired
            } else {
                initial_restore::Phase::None
            };
        }
        self.local_inventory.usb.delayed_mount = None;
        self.usb_retire_browser();
        let same_generation = self.local_inventory.usb.generation == self.generation;
        self.local_inventory.usb.pending = None;
        self.local_inventory.usb.waiting = false;
        self.local_inventory.usb.choices.clear();
        // An already-admitted writer retains its staged record until its
        // addressed completion, but never installs after navigation changes.
        if same_generation {
            self.restore_usb_scan();
        } else {
            self.local_inventory.usb.interrupted = None;
        }
    }

    fn restore_usb_scan(&mut self) {
        if self.local_inventory.usb.automatic {
            // Each failed observation ends its catalogue request. A still-current
            // non-kiosk unavailable result may own a finite delayed retry.
            self.local_inventory.usb.interrupted = None;
            return;
        }
        if let Some(interrupted) = self.local_inventory.usb.interrupted.take() {
            transient::restore_interrupted_scan(self, &interrupted);
            self.local_inventory.usb.generation = self.generation;
        }
    }
    pub(super) fn usb_launch_failed(&mut self) {
        if self.local_inventory.usb.waiting
            && self.local_inventory.usb.generation == self.generation
        {
            self.local_inventory.usb.waiting = false;
            self.local_inventory.usb.delayed_mount = None;
            self.local_inventory.usb.error =
                Some("Could not start USB observation; refresh to retry");
            if !self.usb_browsing() { self.restore_usb_scan(); }
        }
    }

    pub(super) fn usb_result(
        &mut self,
        app: &mut App,
        generation: u64,
        result: Result<usb::Answer, usb::Error>,
    ) {
        let state = &self.local_inventory.usb;
        if !self.source_work_allowed()
            || !self.initial_usb_completion_current()
            || !playback_allowed(app)
            || self.cancel.load(Ordering::Acquire)
            || generation != self.generation
            || state.generation != generation
            || !self.usb_context_current()
            || !state.waiting
            || !state
                .target
                .as_ref()
                .is_some_and(|target| target.current(self))
        {
            if state.generation == generation {
                self.retire_usb_work();
            }
            return;
        }
        self.local_inventory.usb.waiting = false;
        match result {
            Ok(usb::Answer::Browse(page)) if self.usb_browsing() && !self.local_inventory.usb.selecting => {
                self.status = page.status().into();
                let same_folder = self.local_inventory.usb.browser.as_ref()
                    .is_some_and(|old| old.root_generation() == page.root_generation() && old.relative() == page.relative());
                let last_page = page.folders().len().saturating_sub(1) / usb::PAGE_ROWS;
                self.local_inventory.usb.browser_page = if same_folder {
                    self.local_inventory.usb.browser_page.min(last_page)
                } else { 0 };
                self.local_inventory.usb.browser = Some(page);
                self.local_inventory.usb.error = None;
            }
            Ok(usb::Answer::Choices { choices, observed })
                if !self.local_inventory.usb.selecting =>
            {
                transient::observe_local_loss(self, app, &observed);
                self.local_inventory.install(observed, self.generation);
                self.local_inventory.usb.choices = choices;
                self.local_inventory.usb.error = None;
                self.status = if self.local_inventory.usb.choices.is_empty() {
                    "No accessible USB mounts were found"
                } else {
                    "Choose a USB drive; no files have been copied"
                }
                .into();
                self.restore_usb_scan();
            }
            Ok(usb::Answer::Restored { source, relocation })
                if self.local_inventory.usb.selecting && self.local_inventory.usb.automatic =>
            {
                self.local_inventory.usb.delayed_mount = None;
                let target = self.local_inventory.usb.target.clone().expect("checked target");
                match self.usb_install_with_relocation(app, target, source, Some(relocation)) {
                    Ok(()) => self.local_inventory.usb.choices.clear(),
                    Err(error) => {
                        self.status = error.into();
                        self.local_inventory.usb.error = Some(error);
                        self.restore_usb_scan();
                    }
                }
            }
            Ok(usb::Answer::Selected(source))
                if self.local_inventory.usb.selecting && !self.local_inventory.usb.automatic => {
                let target = self
                    .local_inventory
                    .usb
                    .target
                    .clone()
                    .expect("checked target");
                self.usb_retire_browser();
                self.page = "profiles";
                let result = self.usb_install(app, target, source);
                if let Err(error) = result {
                    self.status = error.into();
                    self.local_inventory.usb.error = Some(error);
                    self.restore_usb_scan();
                } else {
                    self.local_inventory.usb.choices.clear();
                }
            }
            Err(error) => {
                self.usb_delayed_mount_failed(error);
                self.status = error.message().into();
                self.local_inventory.usb.error = Some(error.message());
                if !self.usb_browsing() { self.restore_usb_scan(); }
            }
            _ => {
                self.local_inventory.usb.delayed_mount = None;
                self.status = "USB operation changed; refresh to retry".into();
                self.restore_usb_scan();
            }
        }
        self.changed();
    }

    fn usb_install(
        &mut self,
        app: &mut App,
        target: Target,
        source: crate::app::ott_local_root::LocalSource,
    ) -> Result<(), &'static str> {
        self.usb_install_with_relocation(app, target, source, None)
    }

    fn usb_install_with_relocation(
        &mut self, app: &mut App, target: Target,
        source: crate::app::ott_local_root::LocalSource,
        relocation: Option<usb::VerifiedRelocation>,
    ) -> Result<(), &'static str> {
        let kiosk_root = target.kiosk.is_some();
        let mut saved = if relocation.is_none() {
            self.stage_usb_install(target, source)?
        } else {
            self.stage_usb_install_with_relocation(target, source, relocation)?
        };
        if let Err(error) = self.usb_rebase_saved(&mut saved) {
            self.local_inventory.usb.install = None;
            return Err(error);
        }
        let unchanged = serde_json::to_value(&saved).ok().is_some_and(|candidate|
            serde_json::to_value(&self.saved).is_ok_and(|current| candidate == current));
        if unchanged {
            // The ordinary loader activates playback; an automatic rebind keeps
            // the existing browse screen, including settings already open.
            let browse = self.usb_automatic_install().then_some((self.page, self.provider));
            let startup = self.usb_startup_snapshot(app);
            self.invalidate_source(app);
            self.usb_commit_install();
            if !kiosk_root { self.load_catalog(); }
            if let Some(startup) = startup { self.finish_usb_startup(app, startup); }
            if let Some((page, provider)) = browse { (self.page, self.provider) = (page, provider); }
            Ok(())
        } else if let Err(error) = self.save(saved, None, !kiosk_root) {
            self.local_inventory.usb.install = None;
            Err(error)
        } else {
            Ok(())
        }
    }

    fn stage_usb_install(
        &mut self,
        target: Target,
        source: crate::app::ott_local_root::LocalSource,
    ) -> Result<Saved, &'static str> {
        self.stage_usb_install_with_relocation(target, source, None)
    }

    fn stage_usb_install_with_relocation(
        &mut self, mut target: Target, source: crate::app::ott_local_root::LocalSource,
        relocation: Option<usb::VerifiedRelocation>,
    ) -> Result<Saved, &'static str> {
        if source.root_handle().is_none()
            || source.usb_witness().is_none()
            || source.descriptor().id != target.source_id
            || source.descriptor().generation != self.generation
        {
            return Err("USB source changed before installation");
        }
        if let Some(receipt) = &relocation {
            let old = target.folders.folders().iter()
                .find(|folder| folder.id == target.source_id)
                .ok_or("Saved USB source changed")?;
            if !receipt.is_current(&source)
                || old.usb.as_ref() != Some(receipt.stable_binding())
                || old.path.as_str() != receipt.old_root().canonical_path.as_deref().unwrap_or("")
                || receipt.old_root().id != target.source_id
                || receipt.old_root().name != old.name
                || receipt.old_root().authority_ref != target.authority
                || receipt.old_root().kind != SourceKind::Folder
                || receipt.old_root().generation != self.generation
                || (target.kiosk.is_none()
                    && target.previous.as_ref().is_none_or(|profile| profile.source != old.path))
            {
                return Err("USB relocation no longer matches the saved source");
            }
        }
        let path = source
            .descriptor()
            .canonical_path
            .as_ref()
            .ok_or("USB folder is unavailable")?
            .clone();
        let old_fingerprint = target.previous.as_ref().map(profile_fingerprint);
        target.profile.source = path.clone();
        target.profile.enabled = true;
        target
            .profile
            .validate()
            .map_err(|_| "Invalid Local profile")?;
        let folders = target.folders.with_reselected(FolderDescriptor {
            kind: crate::app::ott_local_sources::LocalSourceKind::Folder,
            id: target.source_id.clone(),
            name: source.descriptor().name.clone(),
            path,
            usb: source.usb_witness().and_then(|witness| witness.stable_binding()).cloned(),
        })?;
        let mut saved = self.saved.clone();
        folders.put_into(&mut saved.settings)?;
        if target.kiosk.is_none() {
        if let Some(profile) = saved
            .profiles
            .profiles
            .iter_mut()
            .find(|profile| profile.id == target.profile.id)
        {
            *profile = target.profile.clone();
        } else {
            saved.profiles.profiles.push(target.profile.clone());
        }
        saved
            .profiles
            .validate()
            .map_err(|_| "No Local profile slot is available")?;
        saved.profiles.active_profile = Some(target.profile.id.clone());
        let fingerprint = profile_fingerprint(&target.profile);
        if let Some(resume) = saved
            .resume
            .get_mut(&target.profile.id)
            .filter(|resume| Some(&resume.source_fingerprint) == old_fingerprint.as_ref())
        {
            resume.source_fingerprint = fingerprint.clone();
        }
        }
        if !self
            .local_inventory
            .sources
            .iter()
            .any(|old| old.descriptor().id == source.descriptor().id)
            && self.local_inventory.sources.len() >= 32
        {
            return Err("Too many connected Local folders");
        }
        self.local_inventory.usb.install = Some(Install { target, source, relocation, checkpoint: None });
        Ok(saved)
    }

    /// Retains only configured selection intent while detached. The descriptor
    /// carries no live authority; all reads still require the current RootFD.
    pub(super) fn retained_usb_intent(&self, source: &BoundSource) -> Option<FolderDescriptor> {
        if !self.loaded || self.persistence_uncertain || source.kind != SourceKind::Folder {
            return None;
        }
        RememberedFolders::read(&self.saved.settings).ok()?.folders().iter()
            .find(|folder| folder.kind == LocalSourceKind::Folder
                && folder.id == source.id && folder.name == source.name
                && Some(folder.path.as_str()) == source.canonical_path.as_deref()
                && folder.usb.as_ref().is_some_and(|binding| binding.validate().is_ok()))
            .cloned()
    }

    pub(super) fn usb_startup_snapshot(&self, app: &App) -> Option<work_lifecycle::UsbStartup> {
        let install = self.local_inventory.usb.install.as_ref()?;
        let receipt = install.relocation.as_ref()?;
        self.capture_usb_startup(app, receipt, &install.source)
    }

    pub(super) fn usb_install_pending(&self) -> bool {
        self.local_inventory.usb.install.is_some()
    }

    pub(super) fn usb_automatic_install(&self) -> bool {
        self.local_inventory.usb.automatic
            && self.local_inventory.usb.install.as_ref().is_some_and(|install| install.relocation.is_some())
    }

    /// Runs after normal proof invalidation, and also for the no-write fast path.
    /// Only a current worker-issued receipt may restore a previously held proof.
    pub(super) fn usb_rebase_saved(&mut self, candidate: &mut Saved) -> Result<(), &'static str> {
        let Some(install) = &self.local_inventory.usb.install else { return Ok(()); };
        let Some(receipt) = &install.relocation else { return Ok(()); };
        if !self.usb_context_current() || !install.target.current(self)
            || !receipt.is_current(&install.source)
            || (install.target.kiosk.is_none()
                && candidate.profiles.profiles.iter().find(|profile| profile.id == install.target.profile.id)
                    != Some(&install.target.profile))
        {
            return Err("USB relocation changed before saving");
        }
        let previous = RememberedFolders::read(&self.saved.settings)?;
        let old = receipt.old_root();
        if !previous.folders().iter().any(|folder| folder.id == old.id
            && folder.name == old.name && Some(folder.path.as_str()) == old.canonical_path.as_deref()
            && folder.usb.as_ref() == Some(receipt.stable_binding()))
        {
            return Err("Previous USB source no longer matches its saved identity");
        }
        let folders = RememberedFolders::read(&candidate.settings)?;
        let expected = install.source.descriptor();
        if !folders.folders().iter().any(|folder| folder.id == expected.id
            && folder.name == expected.name
            && Some(folder.path.as_str()) == expected.canonical_path.as_deref()
            && folder.usb.as_ref() == Some(receipt.stable_binding()))
        {
            return Err("Saved USB destination changed before saving");
        }
        // Keep the caller's candidate untouched if any checked transformation fails.
        let mut rebased = candidate.clone();
        kiosk_usb::rebase_local_folder_pin(&self.saved, &mut rebased, &install.source, receipt)?;
        rebased.local_favorite_proofs.restore_verified_relocation(
            &self.saved.local_favorite_proofs, receipt, &install.source,
            &self.saved.profiles, &rebased.profiles,
        ).map_err(|_| "USB Favorite proofs could not be relocated")?;
        let policy = crate::app::ott_kiosk_stored::read(&rebased.settings)
            .map_err(|_| "Invalid stored kiosk policy")?;
        rebased.local_favorite_proofs.prune_with_pins(&rebased.favorites,
            policy.as_ref().map_or(&[], |policy| policy.pins()));
        rebased.local_favorite_proofs.reconcile_profiles(&rebased.profiles.profiles);
        let checkpoint = usb_resume::rebase(&self.saved, &mut rebased, &install.source, receipt)?;
        *candidate = rebased;
        self.local_inventory.usb.install.as_mut().expect("checked staged install").checkpoint = Some(checkpoint);
        Ok(())
    }

    /// Applied after the existing checkpoint merge, only to this explicit
    /// reselection transaction; no pre-save mutation of live bookmarks.
    pub(super) fn usb_preserve_checkpoint(&self, candidate: &mut Saved) {
        let Some(install) = &self.local_inventory.usb.install else {
            return;
        };
        if install.target.kiosk.is_some() { return; }
        let old = install.target.previous.as_ref().map(profile_fingerprint);
        let next = profile_fingerprint(&install.target.profile);
        if candidate
            .profiles
            .profiles
            .iter()
            .find(|profile| profile.id == install.target.profile.id)
            != Some(&install.target.profile)
        {
            return;
        }
        if let Some(resume) = candidate
            .resume
            .get_mut(&install.target.profile.id)
            .filter(|resume| Some(&resume.source_fingerprint) == old.as_ref())
        {
            resume.source_fingerprint = next;
        }
    }

    fn usb_commit_install(&mut self) {
        let Some(install) = self.local_inventory.usb.install.take() else {
            return;
        };
        let old = install.target.previous.as_ref().map(profile_fingerprint);
        if let Some(resume) = self
            .checkpoints
            .get_mut(&install.target.profile.id)
            .filter(|resume| Some(&resume.source_fingerprint) == old.as_ref())
        {
            resume.source_fingerprint = profile_fingerprint(&install.target.profile);
        }
        if let Some(checkpoint) = &install.checkpoint { checkpoint.apply(&mut self.checkpoints); }
        let mut sources = self.local_inventory.sources.clone();
        if let Some(source) = sources
            .iter_mut()
            .find(|source| source.descriptor().id == install.source.descriptor().id)
        {
            *source = install.source;
        } else {
            sources.push(install.source);
        }
        self.local_inventory.install(sources, self.generation);
        self.local_inventory.usb.initial_restore = initial_restore::Phase::None;
        if !self.local_inventory.usb.automatic {
            self.profile_focus = Some(install.target.profile.id);
        }
        self.local_inventory.usb.interrupted = None;
        self.local_inventory.usb.automatic = false;
    }

    /// Check the addressed writer before its ordinary source invalidation
    /// advances Runtime's generation. The resulting boolean is used only by
    /// that synchronous Saved handler, never retained for another event.
    pub(super) fn usb_save_ready(&self, candidate: &Saved) -> bool {
        self.source_work_allowed()
            && self.initial_usb_completion_current()
            && !self.cancel.load(Ordering::Acquire)
            && self.usb_context_current()
            && self
                .local_inventory
                .usb
                .install
                .as_ref()
                .is_some_and(|install| {
                    (install.target.kiosk.is_some() || candidate
                        .profiles
                        .profiles
                        .iter()
                        .find(|profile| profile.id == install.target.profile.id)
                        == Some(&install.target.profile))
                        && (install.target.kiosk.is_none() || candidate.profiles == self.saved.profiles)
                        && RememberedFolders::read(&candidate.settings).is_ok_and(|folders|
                            folders.folders().iter().any(|folder| {
                                let source = install.source.descriptor();
                                folder.kind == LocalSourceKind::Folder
                                    && folder.id == source.id && folder.name == source.name
                                    && Some(folder.path.as_str()) == source.canonical_path.as_deref()
                                    && folder.usb.as_ref() == install.source.usb_witness()
                                        .and_then(|witness| witness.stable_binding())
                            }))
                        && install.source.root_handle().is_some()
                        && install.relocation.as_ref().is_none_or(|receipt| receipt.is_current(&install.source))
                })
    }
    /// A failed current kiosk write is a source failure, not a fresh attempt.
    /// Leave one failure for the existing Startup retry owner to consume. Late
    /// acknowledgements still retire their proposal without reviving old intent.
    pub(super) fn usb_save_failed(&mut self) {
        let retry = self.source_work_allowed()
            && !self.cancel.load(Ordering::Acquire)
            && self.usb_context_current()
            && self.local_inventory.usb.automatic
            && self.local_inventory.usb.install.as_ref().is_some_and(|install|
                install.target.kiosk.is_some() && install.target.current(self));
        self.usb_save_finished(false);
        if retry {
            self.local_inventory.usb.error = Some("Could not save USB folder");
        }
    }

    pub(super) fn usb_save_finished(&mut self, addressed: bool) {
        if self.local_inventory.usb.install.is_none() {
            return;
        }
        if addressed {
            self.usb_commit_install();
        } else {
            self.local_inventory.usb.install = None;
            self.retire_usb_work();
        }
    }

    pub(super) fn usb_prepare_kiosk_folder(
        &mut self, source_id: &str, fingerprint: &str,
    ) -> Result<bool, RestoreStartError> {
        let folders = RememberedFolders::read(&self.saved.settings)
            .map_err(|_| RestoreStartError::Invalid)?;
        let Some(folder) = folders.folders().iter().find(|folder| folder.id == source_id) else {
            return Ok(false);
        };
        if folder.kind != LocalSourceKind::Folder
            || self.usb_path_is_single_file(&folder.path, &folders)
        { return Ok(false); }
        let Some(binding) = folder.usb.clone() else { return Ok(false); };
        let authority = self.local_inventory.sources.iter()
            .find(|source| source.descriptor().id == source_id)
            .map(|source| source.descriptor().authority_ref)
            .unwrap_or_else(|| AuthorityRef((1..=self.local_inventory.sources.len() as u64 + 1)
                .find(|id| !self.local_inventory.sources.iter()
                    .any(|source| source.descriptor().authority_ref.0 == *id)).unwrap()));
        let source = BoundSource { id: folder.id.clone(), name: folder.name.clone(),
            kind: SourceKind::Folder, available: false, canonical_path: Some(folder.path.clone()),
            authority_ref: authority, generation: self.generation.wrapping_add(1).max(1) };
        if selection::local_root_fingerprint(&source).map_err(|_| RestoreStartError::Invalid)? != fingerprint {
            return Err(RestoreStartError::ContextChanged);
        }
        if self.local_inventory.sources.iter().any(|current| current.descriptor().id == source_id
            && current.root_handle().is_some() && current.usb_witness().is_some()) { return Ok(false); }
        let policy = crate::app::ott_kiosk_stored::read(&self.saved.settings)
            .map_err(|_| RestoreStartError::Invalid)?.ok_or(RestoreStartError::ContextChanged)?;
        if !matches!(policy.target(), crate::app::ott_kiosk_stored::TargetRef::LocalFolder {
            source_id: id, source_fingerprint: digest } if id == source_id && digest == fingerprint)
        { return Err(RestoreStartError::ContextChanged); }
        if self.local_inventory.usb.waiting || self.local_inventory.usb.install.is_some() {
            return if self.usb_context_current() && self.local_inventory.usb.target.as_ref()
                .is_some_and(|target| target.source_id == source_id && target.current(self))
                { Ok(true) } else { Err(RestoreStartError::Busy) };
        }
        if self.local_inventory.usb.target.as_ref().is_some_and(|target|
            target.source_id == source_id && target.kiosk.as_ref() == Some(&policy))
            && self.local_inventory.usb.error.take().is_some()
        { return Err(RestoreStartError::Unavailable); }
        let target = Target {
            kiosk: Some(policy), previous: None,
            profile: ProviderProfile { id: format!("transient:{source_id}"), name: "Local videos".into(),
                kind: ProviderKind::Local, source: folder.path.clone(), source_alias: folder.name.clone(), enabled: true },
            source_id: source_id.into(), authority, relative_folder: String::new(), folders,
        };
        self.usb_begin_mode(usb::Task::Restore { binding, source }, target, true, true)
            .map_err(|_| RestoreStartError::Busy)?;
        Ok(true)
    }

    /// Called only by the existing foreground rearm edge, never each frame.
    pub(super) fn usb_rearm_after_foreground(&mut self) {
        let state = &self.local_inventory.usb;
        if !state.automatic || state.install.is_some()
            || !self.loaded || self.persistence_uncertain
            || state.navigation != self.navigation_epoch || state.playback != self.playback_epoch
            || !state.ticket.is_some_and(plx_media::route::play_request_is_current)
            || self.playing.is_some() || self.play_target.is_some() || self.resume_pending.is_some()
            || self.transient.accepted.is_some() || self.transient.candidate.is_some()
            || self.catalog_next.is_some()
        { return; }
        let Some(target) = state.target.as_ref().filter(|target| target.kiosk.is_none()
            && target.current(self) && self.effective_profile() == target.previous.as_ref()) else { return; };
        let profile = target.profile.clone();
        let remote = std::mem::take(&mut self.local_inventory.usb.remote);
        self.local_inventory.usb = State { remote, ..State::default() };
        self.catalog_next = Some(profile);
        self.catalog_loading = true;
        self.catalog_complete = false;
    }

    fn usb_restore_target(&self, profile: &ProviderProfile) -> Result<(Target, usb::Task), &'static str> {
        if self.effective_profile() != Some(profile) || !self.source_work_allowed()
            || self.transient.accepted.is_some() || self.play_target.is_some()
        {
            return Err("Saved USB profile is no longer active");
        }
        let target = self.usb_target_for(Some(profile.clone()))?;
        let folder = target.folders.folders().iter()
            .find(|folder| folder.kind == LocalSourceKind::Folder
                && folder.id == target.source_id && folder.path == profile.source)
            .ok_or("Select the USB drive again in Local profiles before playback")?;
        let binding = folder.usb.clone()
            .ok_or("Select the USB drive again to remember its identity")?;
        let source = BoundSource {
            id: folder.id.clone(), name: folder.name.clone(), kind: SourceKind::Folder,
            available: false, canonical_path: Some(folder.path.clone()),
            authority_ref: target.authority,
            generation: self.generation.wrapping_add(1).max(1),
        };
        Ok((target, usb::Task::Restore { binding, source }))
    }

    pub(super) fn usb_catalog_allowed(&mut self) -> bool {
        let Some(profile) = self.catalog_next.as_ref().filter(|profile| {
            // A malformed remembered collection must reach the existing
            // typed Restore error, never fall through to plain-path work.
            profile.kind == ProviderKind::Local
                && self.usb_folder_path(&profile.source).unwrap_or(true)
        }).cloned() else {
            return true;
        };
        let staged_file = matches!(&self.transient.candidate,
            Some(transient::Candidate::Local(candidate))
                if candidate.owner == crate::app::ott_local_session::OwnerToken(self.generation)
                    && matches!(&candidate.phase,
                        transient::CandidatePhase::Scanning { source, profile: staged, .. }
                            if staged == &profile && source.kind == SourceKind::File
                                && source.canonical_path.as_deref() == Some(profile.source.as_str())));
        if staged_file {
            // First temporary playback is not installed yet. The exact staged
            // catalogue still goes through ordinary Local held-file admission.
            return true;
        }
        if RememberedFolders::read(&self.saved.settings).is_ok_and(|folders|
            self.usb_path_is_single_file(&profile.source, &folders))
        {
            // A file may physically reside on USB, but must follow the exact-file
            // acquisition path, never USB folder selection or automatic rebind.
            return true;
        }
        let current = self.local_profile_selection(&profile)
            .and_then(|selection| self.capture_local_root(&selection))
            .is_some_and(|read| {
                self.local_inventory.sources.iter().any(|source| {
                    source.descriptor().id == read.selection.source_id
                        && source.usb_witness().is_some()
                })
            });
        if current { return true; }
        let queued = self.usb_restore_target(&profile)
            .and_then(|(target, task)| self.usb_begin_mode(task, target, true, true));
        if let Err(error) = queued {
            self.catalog_next = None;
            self.catalog_loading = false;
            self.status = error.into();
            self.changed();
        }
        false
    }

    pub(super) fn usb_rows(&self) -> Vec<OttRow> {
        if self.provider != ProviderKind::Local || self.page != "profiles" {
            return Vec::new();
        }
        let state = &self.local_inventory.usb;
        let title = state
            .target
            .as_ref()
            .map(|target| format!("Select USB for {}", target.profile.name))
            .unwrap_or_else(|| "Browse USB drives".into());
        let mut refresh = row(
            "local.usb".into(),
            title,
            state
                .error
                .unwrap_or("Select a drive if it was not restored automatically")
                .into(),
            OttAction::EditSetting("local.usb".into()),
            false,
        );
        refresh.enabled = self.loaded && !self.saving && !state.waiting && state.install.is_none();
        let mut rows = vec![refresh];
        if state.generation != self.generation || state.navigation != self.navigation_epoch {
            return rows;
        }
        for (index, choice) in state.choices.iter().enumerate() {
            let key = format!("local.usb.select:{}:{index}", state.generation);
            let detail = state
                .target
                .as_ref()
                .map(|target| {
                    if target.relative_folder.is_empty() {
                        format!("Browse drive folders for {}", target.profile.name)
                    } else {
                        format!("Browse {}/{}", target.profile.name, target.relative_folder)
                    }
                })
                .unwrap_or_default();
            let mut item = row(
                key.clone(),
                choice.label(index),
                detail,
                OttAction::EditSetting(key),
                false,
            );
            item.enabled =
                choice.readable() && !state.waiting && !self.saving && state.install.is_none();
            rows.push(item);
        }
        rows
    }
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_usb_runtime_tests.rs"]
mod tests;

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_usb_restore_tests.rs"]
mod restore_tests;

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_usb_restore_install_tests.rs"]
mod restore_install_tests;

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_usb_file_boundary_tests.rs"]
mod file_boundary_tests;
