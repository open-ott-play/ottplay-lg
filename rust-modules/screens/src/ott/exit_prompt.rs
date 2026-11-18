//! The shared decision widget; only the owning Runtime may authorize root exit.
use super::*;
use plx_machine::machine::UiMode;
use plx_ui::decision_prompt::{DecisionPrompt, PromptStep};
use plx_ui::screen::{Enter, FocusTarget};

const GROUP: GroupId = GroupId(0x45584954);
// Use the shared page-alert namespace, below the dispatcher-owned tab strip.
const CANCEL: u32 = crate::registry::ALERT;
const AFFIRM: u32 = CANCEL + 1;
const SCRIM: u32 = AFFIRM + 1;

pub(super) struct State {
    pub(super) prompt: DecisionPrompt,
    // Runtime tokens increase throughout this instance's lifetime. A late
    // delivery cannot reopen an answered or withdrawn question on a later visit.
    last_token: Option<u64>,
    return_focus: Option<FocusKey<u32>>,
}
impl State {
    pub(super) fn new() -> Self {
        Self { prompt: DecisionPrompt::new(GROUP, CANCEL, AFFIRM,
            msg::settings_cancel_c(), msg::ott_exit_confirm_c()), last_token: None, return_focus: None }
    }
    pub(super) fn draw<H: AppLike>(&mut self, frame: &mut DrawFrame<'_, '_, H>, entry: EntryId) {
        if !self.prompt.visible() { return; }
        // The Player catalogue is a panel that dismisses on a pointer miss.
        // Its modal scrim covers every list hit until the exit fade finishes.
        frame.stop(plx_ui::Painter::root(), Stop { key: FocusKey { entry, elem: SCRIM },
            rect: Rect::FULL, rest_rect: Rect::FULL, clip: Rect::FULL,
            hover: Hover::Ignore, activate: Activate::Direct });
        self.prompt.draw(frame, entry);
    }
    pub(super) fn write(&self, out: &mut Canon) {
        out.option(self.last_token, |out, token| { out.u64(token); });
        out.bool(self.prompt.is_open()).bool(self.prompt.choice()).u32(self.prompt.scroll_target_bits());
        out.option(self.return_focus.as_ref(), |out, key| { out.u32(key.elem); });
    }
    pub(super) fn relabel(&mut self, classic: bool) {
        let (cancel, affirm) = if classic { (msg::ott_settings_no_c(), msg::ott_settings_yes_c()) }
            else { (msg::settings_cancel_c(), msg::ott_exit_confirm_c()) };
        self.prompt.relabel(msg::ott_exit_question_c(), "", cancel, affirm);
    }
}
fn prompt_page(view: &OttView, mode: UiMode) -> bool {
    !view.startup_pending && view.pending_play.is_none()
        && match mode {
            UiMode::Modern => view.page_id == "providers",
            UiMode::Classic => matches!(view.page_id.split(':').next(), Some("channels" | "favorites"))
                && view.search.as_ref().is_none_or(|search| search.query.is_empty()),
        }
}
impl OttScreen {
    fn exit_focus<H: AppLike<Memory = PageMemory>>(&self, focus: FocusTarget<u32>, fx: &mut Effects<'_, H>) {
        fx.push(Fx::Deliver(MachineId::Instance(self.instance),
            Delivery::Screen(ScreenEvent::Enter(Enter::Fresh { focus }))));
    }
    fn restore_exit_focus<H: AppLike<Memory = PageMemory>>(&mut self, fx: &mut Effects<'_, H>) {
        let focus = self.exit.return_focus.take().map(FocusTarget::Elem)
            .unwrap_or(FocusTarget::ContainerGroup(GroupId(0)));
        self.exit_focus(focus, fx);
        fx.invalidate(plx_machine::present::Provenance::Input);
    }
    pub(super) fn exit_event<H: AppLike<Memory = PageMemory>>(&mut self, ev: &ScreenEvent<H>,
        cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Option<Handled>
    {
        let mode = H::ui_mode(&cx.views);
        if self.exit.prompt.visible() {
            // Ignore older snapshot deliveries just as the form does. New
            // metadata on the same list leaves the question and choice intact.
            let retired = match ev {
                ScreenEvent::WillLeave(_) | ScreenEvent::Unmount | ScreenEvent::Cover | ScreenEvent::Suspend
                | ScreenEvent::App(AppMsg::OttEditor(_)) => true,
                ScreenEvent::App(AppMsg::OttSnapshot(view)) => view.revision >= self.snapshot.revision
                    && (view.page_id != self.snapshot.page_id || !prompt_page(view, mode)),
                _ => false,
            };
            if retired || self.classic_mode != (mode == UiMode::Classic) {
                // The new page owns its own focus and must not inherit the old fade.
                self.exit = State { last_token: self.exit.last_token, ..State::new() };
                fx.invalidate(plx_machine::present::Provenance::Input);
            }
        }
        match ev {
            ScreenEvent::App(AppMsg::OttExitDismiss { token }) => {
                if Some(*token) >= self.exit.last_token {
                    self.exit.last_token = Some(*token);
                    if self.exit.prompt.is_open() {
                        self.exit.prompt.dismiss();
                        self.restore_exit_focus(fx);
                    }
                }
                return Some(Handled::Yes);
            }
            ScreenEvent::App(AppMsg::OttExitPrompt { token, page_id }) => {
                if Some(*token) > self.exit.last_token {
                    if self.classic_visible && self.editor.is_none() && !self.reading && !self.browse
                        && *page_id == self.snapshot.page_id && prompt_page(&self.snapshot, mode)
                    {
                        if !self.exit.prompt.is_open() { self.exit.return_focus = cx.focus.current; }
                        self.exit.prompt.open(msg::ott_exit_question_c(), "");
                        self.exit.relabel(mode == UiMode::Classic);
                        self.exit_focus(FocusTarget::ContainerGroup(GROUP), fx);
                        fx.invalidate(plx_machine::present::Provenance::Input);
                    } else {
                        if self.exit.prompt.is_open() {
                            self.exit.prompt.dismiss();
                            self.restore_exit_focus(fx);
                        }
                        // Local Back ownership may have changed in flight. Release
                        // this exact grant so a later Back can ask again.
                        self.send(OttAction::ExitAnswer { token: *token, accept: false }, fx);
                    }
                    self.exit.last_token = Some(*token);
                }
                return Some(Handled::Yes);
            }
            ScreenEvent::Tick(t) => self.exit.prompt.update(t.dt()),
            // A stale hit map may still carry an underlying catalogue row.
            ScreenEvent::FocusMoved { to, .. } if self.exit.prompt.is_open()
                && !self.exit.prompt.owns(to.elem) => return Some(Handled::Yes),
            ScreenEvent::PressHold(_) if self.exit.prompt.visible() => return Some(Handled::Yes),
            _ => {}
        }
        match self.exit.prompt.step(ev, cx) {
            PromptStep::Pass => None,
            PromptStep::Done(handled) => Some(handled),
            PromptStep::Answer(accept) => {
                if let Some(token) = self.exit.last_token {
                    self.send(OttAction::ExitAnswer { token, accept }, fx);
                }
                self.restore_exit_focus(fx);
                Some(Handled::Yes)
            }
        }
    }
}
