//! The existing USB owner supplies a bounded form page; no separate worker or
//! accepted source is created by navigating folders.
use super::*;

impl Runtime {
    pub(in crate::app::ott_runtime) fn usb_browsing(&self) -> bool {
        self.local_inventory.usb.browser_choice.is_some()
    }

    pub(in crate::app::ott_runtime) fn usb_retire_browser(&mut self) {
        if !self.usb_browsing() {
            return;
        }
        let state = &mut self.local_inventory.usb;
        if state.waiting && state.generation == self.generation {
            self.cancel.store(true, Ordering::Release);
        }
        if let Some(mut page) = state.browser.take() {
            page.retire();
        }
        state.browser_choice = None;
        state.pending = None;
        state.waiting = false;
        state.browser_page = 0;
    }

    pub(in crate::app::ott_runtime) fn usb_navigation_exit(&mut self) {
        if !self.usb_browsing() {
            return;
        }
        let current = self.usb_context_current() && self.transient.candidate.is_none();
        let interrupted = self.local_inventory.usb.interrupted.take();
        self.usb_retire_browser();
        // Browser-only navigation must restore a queued old scan even when no
        // worker ever started and therefore no stale reply will arrive. A new
        // source operation or activating writer keeps its own catalogue plan.
        // A Preserve write (for example Favorites view) has no catalogue
        // generation authority and must not strand this interrupted producer.
        if current
            && (!self.saving || matches!(self.save_navigation, SaveNavigation::Preserve))
            && !self.catalog_loading
            && self.catalog_next.is_none()
        {
            if let Some(interrupted) = interrupted.filter(|plan| {
                plan.restart
                    && self.effective_profile() == plan.profile.as_ref()
                    && self
                        .transient
                        .accepted
                        .as_ref()
                        .map(transient::TransientMedia::owner)
                        == plan.owner
            }) {
                transient::restore_interrupted_scan(self, &interrupted);
            }
        }
    }

    fn usb_exit_browser(&mut self) {
        let current = self.usb_context_current();
        if current {
            self.cancel.store(true, Ordering::Release);
        }
        self.usb_retire_browser();
        if self.page == "usb-folders" {
            self.page = "profiles";
        }
        if current {
            self.restore_usb_scan();
        } else {
            self.local_inventory.usb.interrupted = None;
        }
        self.local_inventory.usb.generation = self.generation;
        self.local_inventory.usb.navigation = self.navigation_epoch;
        if current {
            self.status.clear();
        }
        self.changed();
    }

    pub(in crate::app::ott_runtime) fn usb_suspend_browser(&mut self) {
        if self.usb_browsing() {
            self.usb_exit_browser();
        }
    }

    fn usb_queue_browser(
        &mut self,
        task: usb::BrowseTask,
        selecting: bool,
    ) -> Result<(), &'static str> {
        if self.saving
            || self.persistence_uncertain
            || !self.source_work_allowed()
            || !self
                .local_inventory
                .usb
                .target
                .as_ref()
                .is_some_and(|target| target.current(self))
        {
            return Err("USB list changed; refresh and select again");
        }
        self.cancel.store(true, Ordering::Release);
        let classic_generation = self.generation;
        self.generation = self.generation.wrapping_add(1).max(1);
        classic_source::rebase(self, classic_generation);
        self.continuity.rebase(self.generation);
        self.cancel = Arc::new(AtomicBool::new(false));
        let state = &mut self.local_inventory.usb;
        state.generation = self.generation;
        state.navigation = self.navigation_epoch;
        state.pending = Some(usb::Task::Browse(task));
        state.waiting = true;
        state.selecting = selecting;
        state.error = None;
        self.status = if selecting {
            "Checking selected USB folder…"
        } else {
            "Reading folders…"
        }
        .into();
        self.changed();
        self.launch_catalog();
        Ok(())
    }

    pub(in crate::app::ott_runtime) fn usb_browser_back(
        &mut self,
        app: &App,
    ) -> Result<(), &'static str> {
        if !self.usb_browsing() {
            return Ok(());
        }
        if !self.usb_context_current() {
            self.usb_exit_browser();
            return Ok(());
        }
        if self.local_inventory.usb.waiting {
            // Cancel only the pending navigation first. Its real worker still
            // owns catalog_done; retaining the old page is not a new read.
            self.cancel.store(true, Ordering::Release);
            let classic_generation = self.generation;
            self.generation = self.generation.wrapping_add(1).max(1);
            classic_source::rebase(self, classic_generation);
            self.continuity.rebase(self.generation);
            self.cancel = Arc::new(AtomicBool::new(false));
            let state = &mut self.local_inventory.usb;
            state.generation = self.generation;
            state.pending = None;
            state.waiting = false;
            state.selecting = false;
            state.error = None;
            if state.browser.is_none() {
                self.usb_exit_browser();
            } else {
                self.status = "Folder loading cancelled".into();
                self.changed();
            }
            return Ok(());
        }
        if !playback_allowed(app) || !self.source_work_allowed() {
            self.usb_exit_browser();
            return Ok(());
        }
        if let Some(page) = self
            .local_inventory
            .usb
            .browser
            .as_ref()
            .filter(|page| !page.is_root())
        {
            return self.usb_queue_browser(page.parent(), false);
        }
        self.usb_exit_browser();
        Ok(())
    }

    pub(super) fn usb_browser_action(
        &mut self,
        app: &App,
        key: &str,
    ) -> Option<Result<(), &'static str>> {
        let fields = key.strip_prefix("local.usb.browse:")?;
        Some((|| {
            let mut parts = fields.split(':');
            let generation: u64 = parts
                .next()
                .ok_or("Invalid USB selection")?
                .parse()
                .map_err(|_| "Invalid USB selection")?;
            let page_index: usize = parts
                .next()
                .ok_or("Invalid USB selection")?
                .parse()
                .map_err(|_| "Invalid USB selection")?;
            let action = parts.next().ok_or("Invalid USB selection")?;
            let argument = parts.next();
            if parts.next().is_some() || !self.usb_browsing() {
                return Err("Invalid USB selection");
            }
            let state = &self.local_inventory.usb;
            if state.generation != generation || page_index != state.browser_page {
                return Err("USB list changed; refresh and select again");
            }
            if action == "cancel" {
                self.usb_exit_browser();
                return Ok(());
            }
            if self.generation != generation || state.navigation != self.navigation_epoch {
                return Err("USB list changed; refresh and select again");
            }
            if action == "back" {
                return self.usb_browser_back(app);
            }
            if !playback_allowed(app) || !self.source_work_allowed() || state.waiting || self.saving
            {
                return Err("USB selection is unavailable while playback is suspended");
            }
            let page = state.browser.as_ref();
            if action == "next" || action == "previous" {
                let count = page.ok_or("USB folder is unavailable")?.folders().len();
                let next = if action == "next" {
                    page_index.checked_add(1)
                } else {
                    page_index.checked_sub(1)
                }
                .filter(|index| index.saturating_mul(usb::PAGE_ROWS) < count)
                .ok_or("USB folder page is unavailable")?;
                self.local_inventory.usb.browser_page = next;
                self.changed();
                return Ok(());
            }
            let task = match (action, page) {
                ("enter", Some(page)) => {
                    let index: usize = argument
                        .ok_or("Invalid USB selection")?
                        .parse()
                        .map_err(|_| "Invalid USB selection")?;
                    if index / usb::PAGE_ROWS != page_index {
                        return Err("USB folder page changed");
                    }
                    page.open_child(index).map_err(usb::Error::message)?
                }
                ("parent", Some(page)) => page.parent(),
                ("root", Some(page)) => page.root(),
                ("refresh", Some(page)) => page.refresh(),
                ("more", Some(page)) => page.more().ok_or("Folder list limit reached")?,
                ("use", Some(page)) => {
                    let target = state.target.as_ref().ok_or("USB selection changed")?;
                    page.confirm(
                        target.descriptor(self.generation.wrapping_add(1).max(1), page.relative()),
                    )
                }
                ("root", None) => {
                    let target = state.target.as_ref().ok_or("USB selection changed")?;
                    usb::BrowseTask::Open {
                        choice: state
                            .browser_choice
                            .as_ref()
                            .ok_or("USB drive is unavailable")?
                            .clone(),
                        source: target.descriptor(self.generation.wrapping_add(1).max(1), ""),
                        initial_relative: String::new(),
                    }
                }
                _ => return Err("USB folder is unavailable"),
            };
            self.usb_queue_browser(task, action == "use")
        })())
    }

    pub(in crate::app::ott_runtime) fn usb_browser_view(&self) -> Option<OttView> {
        if self.page != "usb-folders" || !self.usb_browsing() {
            return None;
        }
        let state = &self.local_inventory.usb;
        let prefix = format!(
            "local.usb.browse:{}:{}:",
            state.generation, state.browser_page
        );
        let enabled = self.usb_context_current()
            && !state.waiting
            && !self.saving
            && self.source_work_allowed();
        let mut rows = Vec::new();
        let mut push = |id: String, title: String, detail: String, action: String, active: bool| {
            let mut item = row(
                id,
                title,
                detail,
                OttAction::EditSetting(format!("{prefix}{action}")),
                false,
            );
            item.enabled = active;
            rows.push(item);
        };
        push(
            "usb.cancel".into(),
            "Cancel".into(),
            "Return without changing the selected source".into(),
            "cancel".into(),
            true,
        );
        let page_id;
        let description: String;
        if let Some(page) = &state.browser {
            description = if page.is_root() {
                "Drive root".into()
            } else {
                page.relative().into()
            };
            page_id = format!("usb-folders:{}:{}", page.root_generation(), page.relative());
            push(
                "usb.use".into(),
                "Use this folder".into(),
                description.clone(),
                "use".into(),
                enabled && page.is_live(),
            );
            if !page.is_root() {
                push(
                    "usb.parent".into(),
                    "Parent folder".into(),
                    String::new(),
                    "back".into(),
                    true,
                );
                push(
                    "usb.root".into(),
                    "Drive root".into(),
                    String::new(),
                    "root".into(),
                    enabled,
                );
            }
            push(
                "usb.refresh".into(),
                "Refresh".into(),
                String::new(),
                "refresh".into(),
                enabled,
            );
            let start = state.browser_page.saturating_mul(usb::PAGE_ROWS);
            for (index, name) in page
                .folders()
                .iter()
                .enumerate()
                .skip(start)
                .take(usb::PAGE_ROWS)
            {
                // Identity stays stable when a larger sorted snapshot inserts
                // earlier names. The action independently carries the new epoch.
                push(
                    format!("usb.folder:{}/{}", page.relative(), name),
                    name.clone(),
                    String::new(),
                    format!("enter:{index}"),
                    enabled,
                );
            }
            if state.browser_page > 0 {
                push(
                    "usb.previous".into(),
                    "Previous page".into(),
                    String::new(),
                    "previous".into(),
                    enabled,
                );
            }
            if start.saturating_add(usb::PAGE_ROWS) < page.folders().len() {
                push(
                    "usb.next".into(),
                    "Next page".into(),
                    String::new(),
                    "next".into(),
                    enabled,
                );
            }
            if page.can_more() {
                push(
                    "usb.more".into(),
                    "Load more".into(),
                    "Look for more folders in this directory".into(),
                    "more".into(),
                    enabled,
                );
            }
        } else {
            page_id = "usb-folders:opening".into();
            description = "Choose a folder on this USB drive".into();
            if !state.waiting {
                push(
                    "usb.root".into(),
                    "Browse drive root".into(),
                    String::new(),
                    "root".into(),
                    enabled,
                );
            }
        }
        Some(OttView {
            page_id,
            revision: self.revision,
            title: "USB folders".into(),
            description,
            status: if !self.usb_context_current() {
                "USB selection changed; close this picker".into()
            } else if state.waiting {
                if state.selecting {
                    "Checking selected USB folder…"
                } else {
                    "Reading folders…"
                }
                .into()
            } else {
                state
                    .error
                    .unwrap_or_else(|| {
                        state
                            .browser
                            .as_ref()
                            .map_or("USB folder is unavailable", usb::BrowsePage::status)
                    })
                    .into()
            },
            rows,
            ..OttView::default()
        })
    }
}

#[cfg(all(test, feature = "hostsim"))]
#[path = "ott_usb_browser_runtime_tests.rs"]
mod tests;
