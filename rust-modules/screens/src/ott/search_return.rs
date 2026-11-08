//! A Classic search chain returns to its opening list through ordinary Ott memory.
//! This bookmark never owns current focus; the FocusEngine and Form still do.
use super::*;

pub(super) struct Origin {
    memory: Memory,
    context: Option<String>,
}
impl Origin {
    fn current(&self, view: &OttView) -> bool {
        self.memory.page_id == view.page_id && self.context == view.preview_context
    }
    fn write(&self, out: &mut Canon) {
        self.memory.write(out);
        out.option(self.context.as_ref(), |out, context| { out.str(context); });
    }
}

#[derive(Default)]
pub(super) struct State {
    origin: Option<Origin>,
    pub(super) draft: Option<Origin>,
    pub(super) from_actions: Option<Origin>,
    pub(super) pending: Option<u64>,
}
impl State {
    pub(super) fn write(&self, out: &mut Canon) {
        out.option(self.origin.as_ref(), |out, origin| origin.write(out));
        out.option(self.draft.as_ref(), |out, origin| origin.write(out));
        out.option(self.from_actions.as_ref(), |out, origin| origin.write(out));
        out.option(self.pending, |out, revision| { out.u64(revision); });
    }
}

impl OttScreen {
    fn search_list(&self) -> bool {
        self.classic_mode && self.snapshot.search.is_some()
            && matches!(self.snapshot.page_id.split(':').next(), Some("channels" | "favorites"))
    }

    fn capture_search_origin(&self) -> Origin {
        let mut memory = Memory::capture(self, None);
        // Return to the body item, never an old action key still held by the
        // FocusEngine before the newly published list has been reconciled.
        memory.focus = memory.selected.as_ref().map(|id| format!("item:{id}"));
        Origin { memory, context: self.snapshot.preview_context.clone() }
    }

    pub(super) fn prepare_search_return(&mut self, spec: &OttEditor) {
        self.search_return.draft = None;
        let from_actions = self.search_return.from_actions.take();
        if spec.kind != OttEditorKind::Search || !self.search_list()
            || self.search_return.origin.is_some() { return; }
        self.search_return.draft = Some(from_actions.filter(|origin| origin.current(&self.snapshot))
            .unwrap_or_else(|| self.capture_search_origin()));
    }

    pub(super) fn commit_search_return(&mut self) {
        if !self.search_list() || self.snapshot.search.as_ref().is_none_or(|search| search.query.is_empty()) {
            self.search_return = State::default();
        } else if self.search_return.origin.is_none() {
            self.search_return.origin = self.search_return.draft.take()
                .filter(|origin| origin.current(&self.snapshot));
        }
    }

    pub(super) fn accept_search_return(&mut self, snapshot: &OttView) {
        if snapshot.revision < self.snapshot.revision { return; }
        // Actions is an inner menu of this list. It publishes no search/source
        // fields, so retain the origin across it and validate the full owner on
        // return; every other non-list page retires it.
        if self.classic_mode && snapshot.page_id == "actions"
            && (self.search_list() || (self.snapshot.page_id == "actions"
                && (self.search_return.origin.is_some() || self.search_return.from_actions.is_some()))) {
            if self.search_list() && self.search_return.origin.is_none() {
                self.search_return.from_actions = Some(self.capture_search_origin());
            }
            self.search_return.draft = None;
            self.search_return.pending = None;
            return;
        }
        let same_list = self.classic_mode && snapshot.search.is_some()
            && matches!(snapshot.page_id.split(':').next(), Some("channels" | "favorites"))
            && ((self.snapshot.page_id == snapshot.page_id
                && self.snapshot.preview_context == snapshot.preview_context)
                || (self.snapshot.page_id == "actions" && self.search_return.origin.as_ref()
                    .or(self.search_return.from_actions.as_ref()).is_some_and(|origin| origin.current(snapshot))));
        if !same_list {
            self.search_return = State::default();
            return;
        }
        let empty = snapshot.search.as_ref().is_some_and(|search| search.query.is_empty());
        if empty && self.search_return.pending.is_some_and(|revision| snapshot.revision > revision) {
            self.search_return.pending = None;
            if let Some(mut origin) = self.search_return.origin.take().filter(|origin| origin.current(snapshot)) {
                // A removed/disabled origin follows the normal first available
                // row fallback, rather than waiting on or inventing an old item.
                if !snapshot.rows.iter().any(|row| row.enabled
                    && origin.memory.selected.as_ref() == Some(&row.id)) {
                    origin.memory.selected = snapshot.rows.iter().find(|row| row.enabled
                        && matches!(row.action, OttAction::Play(_))).map(|row| row.id.clone());
                    origin.memory.focus = origin.memory.selected.as_ref().map(|id| format!("item:{id}"));
                }
                self.restore_memory = Some(origin.memory);
            }
        }
        // Direct ClearSearch and empty submit retain their existing focus policy.
        if empty { self.search_return.origin = None; }
    }

    pub(super) fn back_from_search<H: AppLike>(&mut self, fx: &mut Effects<'_, H>) -> bool {
        if !self.search_list() || self.snapshot.search.as_ref().is_none_or(|search| search.query.is_empty()) {
            if self.snapshot.page_id == "actions" { self.search_return.from_actions = None; }
            return false;
        }
        self.search_return.pending = Some(self.snapshot.revision);
        // The application retains all source, policy and editor gates. Only an
        // accepted new empty-query snapshot can activate the saved memory.
        // A new Back press may retry a refusal; held Repeat/Up never enter here.
        self.send(OttAction::ClearSearch, fx);
        true
    }
}
