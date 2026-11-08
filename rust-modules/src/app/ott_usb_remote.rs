//! Passive addressed projection of the existing USB picker. All filesystem
//! operations still run as USB tasks in the one catalogue worker.
use super::*;
use crate::app::ott_local_root::LocalSource;
#[path = "ott_usb_remote_protocol.rs"]
mod protocol;
use protocol::{Error, Op, Request};
const VIEW_TTL_MS: u64 = 5 * 60 * 1000;
#[derive(Default)]
pub(super) struct RemoteState {
    serial: u64,
    namespace: String,
    view: Option<View>,
    pending: Option<Pending>,
    consumer: Option<Consumer>,
    handles: Vec<Handle>,
}
struct Handle {
    token: String,
    runtime: String,
    source: LocalSource,
    confirmed: usb::ConfirmedFolder,
}
struct View {
    generation: String,
    choices: Vec<usb::Choice>,
    page: Option<usb::BrowsePage>,
    entries: Vec<(String, Entry)>,
    select: Option<String>,
    cursor: Option<(String, usize)>,
    parent: Option<String>,
    expires: u64,
}
#[derive(Clone, Copy)]
enum Entry {
    Root(usize),
    Child(usize),
}
struct Consumer {
    serial: u64,
    ticket: plx_media::route::PlayRequestTicket,
    invocation: Invocation,
    source: LocalSource,
    generation: u64,
    navigation: u64,
    playback: u64,
    cancel: Arc<AtomicBool>,
    queued: bool,
    replied: bool,
}
struct Pending {
    serial: u64,
    ticket: plx_media::route::PlayRequestTicket,
    replied: bool,
    invocation: Invocation,
    request: Request,
    generation: u64,
    navigation: u64,
    playback: u64,
    task: Option<usb::Task>,
    cancel: Arc<AtomicBool>,
    offset: usize,
}
impl RemoteState {
    fn next(&mut self) -> Result<u64, Error> {
        self.serial = self.serial.checked_add(1).ok_or(Error::Limit)?;
        Ok(self.serial)
    }
    fn token(&mut self, entropy: [u8; 16], domain: &str) -> Result<String, Error> {
        let serial = self.next()?;
        let mut bytes = entropy.to_vec();
        bytes.extend_from_slice(self.namespace.as_bytes());
        bytes.extend_from_slice(domain.as_bytes());
        bytes.extend_from_slice(&serial.to_be_bytes());
        Ok(plx_base::sha256::sha256(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect())
    }
    pub(super) fn clear_view(&mut self) {
        if let Some(mut view) = self.view.take() {
            if let Some(page) = view.page.as_mut() {
                page.retire();
            }
        }
    }
    pub(super) fn busy(&self) -> bool {
        self.pending.is_some() || self.consumer.is_some()
    }
}
impl Drop for RemoteState {
    fn drop(&mut self) {
        if let Some(p) = &self.pending { p.cancel.store(true, Ordering::Release); }
        if let Some(c) = &self.consumer { c.cancel.store(true, Ordering::Release); }
        // This is only the picker's private root. Confirmed source grants are
        // independent inventory entries and must not be withdrawn here.
        self.clear_view();
    }
}

fn usb_error(error: usb::Error) -> Error {
    match error {
        usb::Error::Cancelled => Error::Cancelled,
        usb::Error::Changed => Error::Changed,
        usb::Error::TooMany => Error::Limit,
        usb::Error::Invalid => Error::Invalid,
        _ => Error::Unavailable,
    }
}
fn descriptor(id: String, name: String, authority: AuthorityRef, generation: u64) -> BoundSource {
    BoundSource {
        id,
        name,
        kind: SourceKind::Folder,
        available: false,
        canonical_path: None,
        authority_ref: authority,
        generation,
    }
}
impl Runtime {
    pub(in crate::app::ott_runtime) fn usb_remote_capable(&self) -> bool {
        self.loaded && self.entropy.is_some() && !self.persistence_uncertain
    }
    pub(in crate::app::ott_runtime) fn usb_remote_failure(
        &self,
        inv: &Invocation,
        code: &str,
    ) -> Value {
        let error = match code {
            "expired" => Error::Expired,
            "restricted" => Error::Restricted,
            _ => Error::Stale,
        };
        protocol::failure(self.remote.runtime(), &inv.params, error)
    }
    fn usb_remote_authority(&self, p: &Pending) -> Result<(), Error> {
        if !self.remote.invocation_is_current(&p.invocation)
            || p.request.runtime != self.remote.runtime()
            || p.generation != self.generation
            || p.navigation != self.navigation_epoch
            || p.playback != self.playback_epoch
            || !plx_media::route::play_request_is_current(p.ticket)
        {
            return Err(Error::Stale);
        }
        if !self.usb_remote_capable()
            || !self.source_work_allowed()
            || self.usb_remote_ui_picker_active()
        {
            return Err(Error::Restricted);
        }
        if p.cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        if !matches!(p.request.op, Op::Roots) {
            let view = self
                .local_inventory
                .usb
                .remote
                .view
                .as_ref()
                .ok_or(Error::Stale)?;
            if p.request.generation.as_deref() != Some(view.generation.as_str()) {
                return Err(Error::Stale);
            }
            if view.page.as_ref().is_some_and(|page| !page.is_live()) {
                return Err(Error::Changed);
            }
        }
        Ok(())
    }
    fn usb_remote_context(&self, p: &Pending) -> Result<(), Error> {
        self.usb_remote_authority(p)?;
        if self.now_ms() >= p.invocation.deadline_ms || self.local_inventory.usb.remote.view.as_ref()
            .is_some_and(|v| !matches!(p.request.op, Op::Roots) && v.expires <= self.now_ms()) {
            return Err(Error::Expired);
        }
        Ok(())
    }
    fn usb_remote_poll_pending(&mut self) {
        let Some(p) = self.local_inventory.usb.remote.pending.as_ref() else { return; };
        let Some(error) = self.usb_remote_context(p).err() else { return; };
        if error == Error::Expired && self.usb_remote_authority(p).is_ok()
            && matches!(p.request.op, Op::Select(_)) && p.task.is_none() {
            if p.replied { return; }
            let p = self.local_inventory.usb.remote.pending.as_mut().unwrap();
            p.replied = true;
            let invocation = p.invocation.clone();
            self.remote.complete(invocation.clone(), protocol::failure(self.remote.runtime(), &invocation.params, error));
        } else {
            self.usb_remote_fail(error);
        }
    }
    pub(in crate::app::ott_runtime) fn usb_remote_command(
        &mut self,
        inv: &Invocation,
    ) -> Option<Value> {
        match self.usb_remote_prepare(inv) {
            Ok(()) => {
                self.launch_catalog();
                None
            }
            Err(error) => Some(protocol::failure(self.remote.runtime(), &inv.params, error)),
        }
    }
    fn usb_remote_prepare(&mut self, inv: &Invocation) -> Result<(), Error> {
        let request = Request::parse(&inv.params)?;
        if self.now_ms() >= inv.deadline_ms {
            return Err(Error::Expired);
        }
        if request.runtime != self.remote.runtime() || !self.remote.invocation_is_current(inv) {
            return Err(Error::Stale);
        }
        if !self.usb_remote_capable() || !self.source_work_allowed() {
            return Err(Error::Restricted);
        }
        if self.local_inventory.usb.remote.busy()
            || self.usb_remote_ui_picker_active()
            || self.local_inventory.usb.waiting
            || self.local_inventory.usb.install.is_some()
            || self.saving
            || self.transient.candidate.is_some()
            || self.play_target.is_some()
            || self.pending_play.is_some()
            || self.resolver_busy
            || self.catalog_loading
            || self.catalog_next.is_some()
            || !self.catalog_done.load(Ordering::Acquire)
            || !self.metadata_done.load(Ordering::Acquire)
        {
            return Err(Error::Busy);
        }
        let entropy = self.entropy.ok_or(Error::Unsupported)?;
        let now = self.now_ms();
        let authority = AuthorityRef(
            (1..=self.local_inventory.sources.len() as u64 + 1)
                .find(|id| {
                    !self
                        .local_inventory
                        .sources
                        .iter()
                        .any(|s| s.descriptor().authority_ref.0 == *id)
                })
                .ok_or(Error::Limit)?,
        );
        let state = &mut self.local_inventory.usb.remote;
        state.namespace = request.runtime.clone();
        if !matches!(request.op, Op::Roots) {
            let view = state.view.as_ref().ok_or(Error::Stale)?;
            if view.expires <= now {
                state.clear_view();
                return Err(Error::Expired);
            }
            if request.generation.as_deref() != Some(view.generation.as_str()) {
                return Err(Error::Stale);
            }
            if view.page.as_ref().is_some_and(|page| !page.is_live()) {
                state.clear_view();
                return Err(Error::Changed);
            }
        }
        let serial = state.next()?;
        let mut offset = 0;
        let task = match &request.op {
            Op::Roots => {
                state.clear_view();
                usb::Task::Discover { known: Vec::new() }
            }
            Op::Cursor(token) => {
                let view = state.view.as_ref().ok_or(Error::Stale)?;
                offset = view
                    .cursor
                    .as_ref()
                    .filter(|(id, _)| id == token)
                    .ok_or(Error::Stale)?
                    .1;
                match &view.page {
                    Some(page) => usb::Task::Browse(page.inspect()),
                    None => usb::Task::InspectChoices(view.choices.clone()),
                }
            }
            Op::Entry(token) => {
                let view = state.view.as_ref().ok_or(Error::Stale)?;
                if view.parent.as_ref() == Some(token) {
                    usb::Task::Browse(
                        view.page
                            .as_ref()
                            .map(usb::BrowsePage::parent)
                            .ok_or(Error::Stale)?,
                    )
                } else {
                    let entry = view
                        .entries
                        .iter()
                        .find(|(id, _)| id == token)
                        .map(|(_, entry)| *entry)
                        .ok_or(Error::Stale)?;
                    match entry {
                        Entry::Child(index) => usb::Task::Browse(
                            view.page
                                .as_ref()
                                .ok_or(Error::Stale)?
                                .open_child(index)
                                .map_err(usb_error)?,
                        ),
                        Entry::Root(index) => {
                            let choice = view
                                .choices
                                .get(index)
                                .filter(|c| c.readable())
                                .ok_or(Error::Unavailable)?
                                .clone();
                            let id = state.token(entropy, "root")?;
                            usb::Task::Browse(usb::BrowseTask::Open {
                                choice,
                                source: descriptor(id, "USB videos".into(), authority, serial),
                                initial_relative: String::new(),
                            })
                        }
                    }
                }
            }
            Op::More(token) => {
                let view = state.view.as_ref().ok_or(Error::Stale)?;
                if view.select.as_ref() != Some(token) {
                    return Err(Error::Stale);
                }
                usb::Task::Browse(
                    view.page
                        .as_ref()
                        .and_then(usb::BrowsePage::more)
                        .ok_or(Error::Limit)?,
                )
            }
            Op::Select(token) => {
                let view = state.view.as_ref().ok_or(Error::Stale)?;
                if view.select.as_ref() != Some(token) {
                    return Err(Error::Stale);
                }
                let page = view.page.as_ref().ok_or(Error::Stale)?;
                let name = page
                    .relative()
                    .rsplit('/')
                    .next()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("USB videos")
                    .to_owned();
                if !protocol::name(&name) {
                    return Err(Error::Invalid);
                }
                let page = page.clone();
                let id = state.token(entropy, "source")?;
                usb::Task::ConfirmFolder {
                    task: page.confirm(descriptor(id, name, authority, serial)),
                    remembered: crate::app::ott_local_sources::RememberedFolders::read(&self.saved.settings).map_err(|_| Error::Invalid)?,
                    known: self.local_inventory.sources.clone(),
                }
            }
        };
        state.pending = Some(Pending {
            serial,
            ticket: plx_media::route::play_request_ticket(),
            replied: false,
            invocation: inv.clone(),
            request,
            generation: self.generation,
            navigation: self.navigation_epoch,
            playback: self.playback_epoch,
            task: Some(task),
            cancel: Arc::new(AtomicBool::new(false)),
            offset,
        });
        Ok(())
    }
    pub(in crate::app::ott_runtime) fn usb_remote_waiting_result(&self) -> bool {
        self.local_inventory
            .usb
            .remote
            .pending
            .as_ref()
            .is_some_and(|p| p.task.is_none())
            || self
                .local_inventory
                .usb
                .remote
                .consumer
                .as_ref()
                .is_some_and(|c| !c.queued)
    }
    pub(in crate::app::ott_runtime) fn take_usb_remote_task(
        &mut self,
    ) -> Option<ott_catalog::Task> {
        self.usb_consumer_poll();
        if let Some(c) = self
            .local_inventory
            .usb
            .remote
            .consumer
            .as_mut()
            .filter(|c| c.queued)
        {
            c.queued = false;
            return Some(ott_catalog::Task::UsbRemote {
                serial: c.serial,
                task: usb::Task::VerifySource(c.source.clone()),
                cancel: c.cancel.clone(),
            });
        }
        self.usb_remote_poll_pending();
        let p = self.local_inventory.usb.remote.pending.as_mut()?;
        p.task.take().map(|task| ott_catalog::Task::UsbRemote {
            serial: p.serial,
            task,
            cancel: p.cancel.clone(),
        })
    }
    fn usb_remote_fail(&mut self, error: Error) {
        if let Some(p) = self.local_inventory.usb.remote.pending.take() {
            p.cancel.store(true, Ordering::Release);
            if !p.replied {
                let response = protocol::failure(self.remote.runtime(), &p.invocation.params, error);
                self.remote.complete(p.invocation, response);
            }
        }
    }
    pub(in crate::app::ott_runtime) fn usb_remote_poll(&mut self) {
        self.usb_consumer_poll();
        self.usb_remote_poll_pending();
        if self
            .local_inventory
            .usb
            .remote
            .view
            .as_ref()
            .is_some_and(|v| v.expires <= self.now_ms())
            && !self.local_inventory.usb.remote.pending.as_ref()
                .is_some_and(|p| matches!(p.request.op, Op::Select(_)) && p.task.is_none())
        {
            self.local_inventory.usb.remote.clear_view();
        }
    }
    pub(in crate::app::ott_runtime) fn usb_remote_launch_failed(&mut self) -> bool {
        if self.local_inventory.usb.remote.consumer.is_some() {
            self.usb_consumer_fail(crate::app::ott_local_session::LocalError::NativeError);
            return true;
        }
        if self.local_inventory.usb.remote.pending.is_none() {
            return false;
        }
        self.usb_remote_fail(Error::Unavailable);
        true
    }
    pub(in crate::app::ott_runtime) fn usb_remote_result(
        &mut self,
        app: &mut App,
        serial: u64,
        result: Result<usb::Answer, usb::Error>,
    ) {
        if self
            .local_inventory
            .usb
            .remote
            .consumer
            .as_ref()
            .is_some_and(|c| c.serial == serial)
        {
            use crate::app::ott_local_session::LocalError;
            let authority = self
                .local_inventory
                .usb
                .remote
                .consumer
                .as_ref()
                .map(|c| self.usb_consumer_authority(c))
                .unwrap();
            let c = self.local_inventory.usb.remote.consumer.take().unwrap();
            if authority.is_ok() {
                if let Ok(usb::Answer::SourceLost(lost)) = &result {
                    let exact = lost.same_grant(&c.source)
                        && lost.descriptor().id == c.source.descriptor().id
                        && lost.descriptor().generation == c.source.descriptor().generation
                        && self.local_inventory.sources.iter().any(|current| current.same_grant(lost)
                            && current.descriptor().id == lost.descriptor().id
                            && current.descriptor().generation == lost.descriptor().generation);
                    if exact {
                        transient::observe_local_loss(self, app, &[LocalSource::unavailable(lost.descriptor().clone())]);
                    }
                }
            }
            // An expired RPC cannot admit playback, but a completed, current
            // observation still revokes only the exact old grant above.
            if c.replied {
                return;
            }
            let valid = authority.and_then(|()| {
                if self.now_ms() >= c.invocation.deadline_ms {
                    Err(LocalError::RuntimeChanged)
                } else {
                    Ok(())
                }
            });
            let accepted = valid.and_then(|()| match result {
                Ok(usb::Answer::Verified(source))
                    if source.is_live() && source.same_grant(&c.source) =>
                {
                    Ok(source)
                }
                _ => Err(LocalError::SourceUnavailable),
            });
            let response = match accepted {
                Ok(source) if input_policy::remote(self, &c.invocation).is_ok() => {
                    transient::command_verified_handle(self, app, &c.invocation, source)
                }
                Ok(_) => Some(consumer_error(&c.invocation, LocalError::Restricted)),
                Err(error) => Some(consumer_error(&c.invocation, error)),
            };
            if let Some(response) = response {
                self.remote.complete(c.invocation, response);
            }
            return;
        }
        if !self
            .local_inventory
            .usb
            .remote
            .pending
            .as_ref()
            .is_some_and(|p| p.serial == serial)
        {
            return;
        }
        let observation_current = self.local_inventory.usb.remote.pending.as_ref()
            .is_some_and(|p| matches!(p.request.op, Op::Select(_)) && self.usb_remote_authority(p).is_ok());
        if observation_current {
            if let Ok(usb::Answer::SourceLost(lost)) = &result {
                if self.local_inventory.sources.iter().any(|current| current.same_grant(lost)
                    && current.descriptor().id == lost.descriptor().id
                    && current.descriptor().generation == lost.descriptor().generation) {
                    transient::observe_local_loss(self, app, &[LocalSource::unavailable(lost.descriptor().clone())]);
                }
            }
        }
        if let Some(error) = self.local_inventory.usb.remote.pending.as_ref()
            .and_then(|p| self.usb_remote_context(p).err()) {
            self.usb_remote_fail(error);
            return;
        }
        let p = self.local_inventory.usb.remote.pending.take().unwrap();
        if p.replied { return; }
        if input_policy::remote(self, &p.invocation).is_err() {
            self.remote.complete(
                p.invocation.clone(),
                protocol::failure(
                    self.remote.runtime(),
                    &p.invocation.params,
                    Error::Restricted,
                ),
            );
            return;
        }
        let reply = match result {
            Err(error) => Err(usb_error(error)),
            Ok(usb::Answer::ConfirmedFolder(confirmed)) if matches!(p.request.op, Op::Select(_)) => {
                self.usb_remote_selected(&p, confirmed)
            }
            Ok(usb::Answer::Choices { choices, .. })
                if matches!(p.request.op, Op::Roots | Op::Cursor(_)) =>
            {
                self.usb_remote_window(&p, choices, None)
            }
            Ok(usb::Answer::Browse(page)) if !matches!(p.request.op, Op::Select(_)) => {
                self.usb_remote_window(&p, Vec::new(), Some(page))
            }
            _ => Err(Error::Changed),
        };
        if reply.is_err() {
            self.local_inventory.usb.remote.clear_view();
        }
        let response = reply
            .unwrap_or_else(|e| protocol::failure(self.remote.runtime(), &p.invocation.params, e));
        self.remote.complete(p.invocation, response);
    }
    fn usb_remote_window(
        &mut self,
        p: &Pending,
        choices: Vec<usb::Choice>,
        page: Option<usb::BrowsePage>,
    ) -> Result<Value, Error> {
        let entropy = self.entropy.ok_or(Error::Unsupported)?;
        let expires = self.now_ms().saturating_add(VIEW_TTL_MS);
        let state = &mut self.local_inventory.usb.remote;
        let generation = state.token(entropy, "view")?;
        let select = page
            .as_ref()
            .map(|_| state.token(entropy, "select"))
            .transpose()?;
        let parent = page
            .as_ref()
            .filter(|page| !page.is_root())
            .map(|_| state.token(entropy, "parent"))
            .transpose()?;
        let total = page.as_ref().map_or(choices.len(), |p| p.folders().len());
        if p.offset > total {
            return Err(Error::Stale);
        }
        let mut entries = Vec::new();
        let mut wire = Vec::new();
        let mut end = p.offset;
        let partial = page.as_ref().is_some_and(usb::BrowsePage::partial);
        let limit = page.as_ref().is_some_and(usb::BrowsePage::limit_reached);
        let mut response = ok(
            json!({"version":1,"runtime":p.request.runtime,"op":p.request.op.name(),"state":"ready","generation":generation,"kind":if page.is_some(){"directories"}else{"roots"},"entries":[],"parent_id":parent,"select_id":select,"next_cursor":null,"partial":partial,"limit_reached":limit}),
        );
        // Reserve the longest possible cursor before accepting any row. Names
        // are displayed verbatim, not interpreted as selectors or paths.
        response["data"]["next_cursor"] = json!("c".repeat(64));
        for index in p.offset..total {
            if entries.len() == protocol::MAX_ROWS {
                break;
            }
            let name = page.as_ref().map_or_else(
                || choices[index].label(index),
                |p| p.folders()[index].clone(),
            );
            if !protocol::name(&name) {
                return Err(Error::Unavailable);
            }
            let id = state.token(entropy, "entry")?;
            let row =
                json!({"id":id,"name":name,"kind":if page.is_some(){"directory"}else{"root"}});
            wire.push(row);
            response["data"]["entries"] = json!(wire);
            if !protocol::bounded(&response) {
                wire.pop();
                break;
            }
            entries.push((
                id,
                if page.is_some() {
                    Entry::Child(index)
                } else {
                    Entry::Root(index)
                },
            ));
            end = index + 1;
        }
        if end == p.offset && end < total {
            return Err(Error::Limit);
        }
        let cursor = (end < total)
            .then(|| state.token(entropy, "cursor").map(|token| (token, end)))
            .transpose()?;
        response["data"]["entries"] = json!(wire);
        response["data"]["next_cursor"] = json!(cursor.as_ref().map(|c| &c.0));
        if !protocol::bounded(&response) {
            return Err(Error::Limit);
        }
        // Browse children/Inspect share the same private root; do not revoke it
        // when replacing only a window. A fresh roots request retires explicitly.
        state.view = Some(View {
            generation,
            choices,
            page,
            entries,
            select,
            cursor,
            parent,
            expires,
        });
        Ok(response)
    }
    fn usb_remote_selected(&mut self, p: &Pending, confirmed: usb::ConfirmedFolder) -> Result<Value, Error> {
        let source = confirmed.source().clone();
        if !source.is_live() || source.usb_witness().is_none()
            || !crate::app::ott_local_sources::RememberedFolders::read(&self.saved.settings)
                .is_ok_and(|saved| confirmed.remembered_is_current(&saved)) {
            return Err(Error::Changed);
        }
        let existing = self.local_inventory.sources.iter().position(|old| old.descriptor().id == source.descriptor().id);
        if existing.is_none() && self.local_inventory.sources.len() >= 32 { return Err(Error::Limit); }
        if self.local_inventory.sources.iter().enumerate().any(|(index, old)| {
            if Some(index) == existing { old.is_live() && !old.same_grant(&source) }
            else { old.descriptor().authority_ref == source.descriptor().authority_ref }
        }) { return Err(Error::Changed); }
        self.local_inventory.usb.remote.handles.retain(|h| self.local_inventory.sources.iter()
            .any(|s| s.same_grant(&h.source) && s.is_live()));
        let handle_index = self.local_inventory.usb.remote.handles.iter().position(|h| h.source.same_grant(&source));
        let token = if let Some(index) = handle_index { self.local_inventory.usb.remote.handles[index].token.clone() }
        else {
            if self.local_inventory.usb.remote.handles.len() >= 32 { return Err(Error::Limit); }
            self.local_inventory.usb.remote.token(self.entropy.ok_or(Error::Unsupported)?, "handle")?
        };
        let descriptor = source.descriptor();
        let response = ok(json!({"version":1,"runtime":p.request.runtime,"op":"select","state":"ready","generation":p.request.generation,"source":{"id":descriptor.id,"name":descriptor.name,"kind":"folder","handle":token,"stable_identity":source.usb_witness().and_then(|w|w.stable_binding()).is_some()},"persisted":false,"activated":false}));
        if !protocol::bounded(&response) { return Err(Error::Limit); }
        // A successful explicit selection completes only this cold empty
        // inventory. Other remembered folders remain unvalidated intents.
        let settles_initial_inventory = self.local_inventory.usb.initial_restore == initial_restore::Phase::Owned
            && self.initial_usb_context_current();
        let handle = Handle {token, runtime:p.request.runtime.clone(), source:source.clone(), confirmed};
        if let Some(index) = handle_index { self.local_inventory.usb.remote.handles[index] = handle; }
        else { self.local_inventory.usb.remote.handles.push(handle); }
        if let Some(index) = existing { self.local_inventory.sources[index] = source; }
        else { self.local_inventory.sources.push(source); }
        if settles_initial_inventory {
            self.local_inventory.phase = transient::InventoryPhase::Ready(self.generation);
        }
        Ok(response)
    }

    pub(in crate::app::ott_runtime) fn usb_remote_connect_candidate(
        &self, token: &str, verified: &LocalSource,
    ) -> Result<crate::app::ott_local_sources::RememberedFolders, crate::app::ott_local_session::LocalError> {
        use crate::app::ott_local_session::LocalError;
        let live = self.usb_remote_handle(token)?;
        if !live.same_grant(verified) { return Err(LocalError::SourceUnavailable); }
        let record = self.local_inventory.usb.remote.handles.iter().find(|h| h.token == token
            && h.runtime == self.remote.runtime() && h.source.same_grant(&live)).ok_or(LocalError::SourceUnavailable)?;
        let current = crate::app::ott_local_sources::RememberedFolders::read(&self.saved.settings).map_err(|_| LocalError::NativeError)?;
        record.confirmed.connect_candidate(&current).map_err(|_| LocalError::SourceUnavailable)
    }

    pub(in crate::app::ott_runtime) fn usb_remote_binding_folders(
        &self, token: &str, remembered: &crate::app::ott_local_sources::RememberedFolders,
    ) -> Result<Vec<crate::app::ott_local_sources::FolderDescriptor>, crate::app::ott_local_session::LocalError> {
        let source = self.usb_remote_handle(token)?;
        let descriptor = source.descriptor();
        if remembered.folders().iter().any(|row| row.id == descriptor.id
            && Some(row.path.as_str()) != descriptor.canonical_path.as_deref()) {
            // Explicit selection proved a moved mount, but Connect has not yet
            // committed that path. Use only this receipt's exact candidate as
            // temporary validator input; never rewrite Saved for Play/Shuffle.
            let current = crate::app::ott_local_sources::RememberedFolders::read(&self.saved.settings)
                .map_err(|_| crate::app::ott_local_session::LocalError::NativeError)?;
            if &current != remembered { return Err(crate::app::ott_local_session::LocalError::SourceUnavailable); }
            return self.usb_remote_connect_candidate(token, &source).map(|candidate| candidate.folders().to_vec());
        }
        Ok(remembered.folders().to_vec())
    }

    fn usb_consumer_authority(
        &self,
        c: &Consumer,
    ) -> Result<(), crate::app::ott_local_session::LocalError> {
        use crate::app::ott_local_session::LocalError;
        if !self.remote.invocation_is_current(&c.invocation)
            || c.generation != self.generation
            || c.navigation != self.navigation_epoch
            || c.playback != self.playback_epoch
            || !plx_media::route::play_request_is_current(c.ticket)
        {
            return Err(LocalError::RuntimeChanged);
        }
        if !self.usb_remote_capable()
            || !self.source_work_allowed()
            || c.cancel.load(Ordering::Acquire)
        {
            return Err(LocalError::Restricted);
        }
        let token = c.invocation.params["source_handle"]
            .as_str()
            .ok_or(LocalError::InvalidRequest)?;
        let source = self.usb_remote_handle(token)?;
        if !source.same_grant(&c.source) {
            return Err(LocalError::SourceUnavailable);
        }
        Ok(())
    }
    fn usb_consumer_poll(&mut self) {
        use crate::app::ott_local_session::LocalError;
        if let Some(error) = self.local_inventory.usb.remote.consumer.as_ref()
            .and_then(|c| self.usb_consumer_authority(c).err())
        {
            self.usb_consumer_fail(error);
            return;
        }
        let now = self.now_ms();
        let Some(c) = self.local_inventory.usb.remote.consumer.as_ref() else { return; };
        if now < c.invocation.deadline_ms || c.replied { return; }
        if c.queued {
            self.usb_consumer_fail(LocalError::RuntimeChanged);
            return;
        }
        // The bounded worker already owns its read-only verification. Settle
        // the RPC once, retaining only its addressed observation until done.
        // Do not cancel or launch a task here: a queued positive loss remains
        // evidence even when delivery happens after the request's deadline.
        let c = self.local_inventory.usb.remote.consumer.as_mut().unwrap();
        c.replied = true;
        let invocation = c.invocation.clone();
        let response = consumer_error(&invocation, LocalError::RuntimeChanged);
        self.remote.complete(invocation, response);
    }
    fn usb_consumer_fail(&mut self, error: crate::app::ott_local_session::LocalError) {
        if let Some(c) = self.local_inventory.usb.remote.consumer.take() {
            c.cancel.store(true, Ordering::Release);
            if !c.replied {
                let response = consumer_error(&c.invocation, error);
                self.remote.complete(c.invocation, response);
            }
        }
    }
    pub(in crate::app::ott_runtime) fn usb_remote_consume(
        &mut self,
        inv: &Invocation,
        source: LocalSource,
    ) -> Option<Value> {
        match self.usb_remote_queue_consume(inv, source) {
            Ok(()) => {
                self.launch_catalog();
                None
            }
            Err(error) => Some(consumer_error(inv, error)),
        }
    }
    fn usb_remote_queue_consume(
        &mut self,
        inv: &Invocation,
        source: LocalSource,
    ) -> Result<(), crate::app::ott_local_session::LocalError> {
        use crate::app::ott_local_session::LocalError;
        if self.now_ms() >= inv.deadline_ms {
            return Err(LocalError::RuntimeChanged);
        }
        if self.local_inventory.usb.remote.busy()
            || self.usb_remote_ui_picker_active()
            || self.local_inventory.usb.waiting
            || self.local_inventory.usb.install.is_some()
            || self.saving
            || self.transient.candidate.is_some()
            || self.play_target.is_some()
            || self.pending_play.is_some()
            || self.resolver_busy
            || self.catalog_loading
            || self.catalog_next.is_some()
            || !self.catalog_done.load(Ordering::Acquire)
            || !self.metadata_done.load(Ordering::Acquire)
        {
            return Err(LocalError::OperationInProgress);
        }
        let serial = match self.local_inventory.usb.remote.next() {
            Ok(n) => n,
            Err(_) => return Err(LocalError::NativeError),
        };
        self.local_inventory.usb.remote.consumer = Some(Consumer {
            serial,
            ticket: plx_media::route::play_request_ticket(),
            invocation: inv.clone(),
            source,
            generation: self.generation,
            navigation: self.navigation_epoch,
            playback: self.playback_epoch,
            cancel: Arc::new(AtomicBool::new(false)),
            queued: true,
            replied: false,
        });
        Ok(())
    }
    pub(in crate::app::ott_runtime) fn usb_remote_handle(
        &self,
        token: &str,
    ) -> Result<LocalSource, crate::app::ott_local_session::LocalError> {
        use crate::app::ott_local_session::LocalError;
        let record = self
            .local_inventory
            .usb
            .remote
            .handles
            .iter()
            .find(|h| h.token == token && h.runtime == self.remote.runtime())
            .ok_or(LocalError::SourceNotFound)?;
        self.local_inventory
            .sources
            .iter()
            .find(|s| {
                s.is_live()
                    && s.same_grant(&record.source)
                    && s.descriptor().id == record.source.descriptor().id
                    && s.descriptor().generation == record.source.descriptor().generation
                    && s.usb_witness().is_some()
            })
            .cloned()
            .ok_or(LocalError::SourceUnavailable)
    }
}

fn consumer_error(inv: &Invocation, error: crate::app::ott_local_session::LocalError) -> Value {
    match crate::app::ott_local_session::LocalRequest::parse(&inv.params) {
        Ok(request) => json!({"status":"rejected","data":request.error_data(error)}),
        Err(_) => rejected("Invalid Local request"),
    }
}

#[cfg(all(test, feature = "hostsim"))]
impl Runtime {
    pub(in crate::app::ott_runtime) fn usb_browser_prepare_for_test(
        &mut self,
        inv: &Invocation,
    ) -> Result<(), &'static str> {
        self.usb_remote_prepare(inv).map_err(Error::code)
    }
    pub(in crate::app::ott_runtime) fn expire_usb_select_for_test(&mut self) {
        let now = self.now_ms();
        self.local_inventory.usb.remote.pending.as_mut().unwrap().invocation.deadline_ms = now;
    }
    pub(in crate::app::ott_runtime) fn expire_usb_handle_for_test(&mut self) {
        let now = self.now_ms();
        self.local_inventory.usb.remote.consumer.as_mut().unwrap().invocation.deadline_ms = now;
    }
    pub(in crate::app::ott_runtime) fn usb_handle_prepare_for_test(
        &mut self,
        inv: &Invocation,
        source: LocalSource,
    ) -> Result<(), crate::app::ott_local_session::LocalError> {
        self.usb_remote_queue_consume(inv, source)
    }
}

#[cfg(test)] mod serial_tests {
    use super::*;
    #[test] fn tokens_do_not_repeat_across_windows_or_runtime_and_exhaustion_is_closed() {
        let mut state=RemoteState::default();state.namespace="first".into();let a=state.token([7;16],"entry").unwrap();let b=state.token([7;16],"entry").unwrap();assert_ne!(a,b);
        let mut other=RemoteState::default();other.namespace="second".into();assert_ne!(a,other.token([7;16],"entry").unwrap());
        state.serial=u64::MAX;assert_eq!(state.token([7;16],"entry"),Err(Error::Limit));assert_eq!(state.serial,u64::MAX);
    }
}
