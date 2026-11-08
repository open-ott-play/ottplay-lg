//! The shared decision widget; only the owning Runtime may authorize root exit.
use super::*;
use plx_ui::decision_prompt::{DecisionPrompt, PromptStep};
use plx_ui::screen::{Enter, FocusTarget};

const GROUP: GroupId = GroupId(0x45584954);
// Use the shared page-alert namespace, below the dispatcher-owned tab strip.
const CANCEL: u32 = crate::registry::ALERT;
const AFFIRM: u32 = CANCEL + 1;

pub(super) struct State {
    pub(super) prompt: DecisionPrompt,
    token: Option<u64>,
    return_focus: Option<FocusKey<u32>>,
}
impl State {
    pub(super) fn new() -> Self {
        Self { prompt: DecisionPrompt::new(GROUP, CANCEL, AFFIRM,
            msg::settings_cancel_c(), msg::ott_exit_confirm_c()), token: None, return_focus: None }
    }
    pub(super) fn write(&self, out: &mut Canon) {
        out.option(self.token.as_ref(), |out, token| { out.u64(*token); });
        out.bool(self.prompt.is_open()).bool(self.prompt.choice()).u32(self.prompt.scroll_target_bits());
        out.option(self.return_focus.as_ref(), |out, key| { out.u32(key.elem); });
    }
}
impl OttScreen {
    fn exit_focus<H: AppLike<Memory = PageMemory>>(&self, focus: FocusTarget<u32>, fx: &mut Effects<'_, H>) {
        fx.push(Fx::Deliver(MachineId::Instance(self.instance),
            Delivery::Screen(ScreenEvent::Enter(Enter::Fresh { focus }))));
    }
    pub(super) fn exit_event<H: AppLike<Memory = PageMemory>>(&mut self, ev: &ScreenEvent<H>,
        cx: &Cx<'_, H>, fx: &mut Effects<'_, H>) -> Option<Handled>
    {
        match ev {
            ScreenEvent::App(AppMsg::OttExitDismiss { token }) => {
                if self.exit.token == Some(*token) {
                    self.exit.token = None;
                    self.exit.prompt.dismiss();
                    let focus = self.exit.return_focus.take().map(FocusTarget::Elem)
                        .unwrap_or(FocusTarget::ContainerGroup(GroupId(0)));
                    self.exit_focus(focus, fx);
                    fx.invalidate(plx_machine::present::Provenance::Input);
                }
                return Some(Handled::Yes);
            }
            ScreenEvent::App(AppMsg::OttExitPrompt { token }) => {
                if self.snapshot.page_id == "providers" && self.editor.is_none()
                    && !self.reading && !self.browse && self.exit.token != Some(*token)
                {
                    if self.exit.token.is_none() { self.exit.return_focus = cx.focus.current; }
                    self.exit.token = Some(*token);
                    self.exit.prompt.open(msg::ott_exit_question_c(), "");
                    self.exit_focus(FocusTarget::ContainerGroup(GROUP), fx);
                    fx.invalidate(plx_machine::present::Provenance::Input);
                }
                return Some(Handled::Yes);
            }
            ScreenEvent::Tick(t) => self.exit.prompt.update(t.dt()),
            ScreenEvent::WillLeave(_) | ScreenEvent::Unmount | ScreenEvent::Cover | ScreenEvent::Suspend
            | ScreenEvent::App(AppMsg::OttEditor(_)) => {
                self.exit.token = None;
                self.exit.return_focus = None;
                self.exit.prompt.dismiss();
            }
            ScreenEvent::App(AppMsg::OttSnapshot(snapshot)) if snapshot.page_id != "providers" => {
                self.exit.token = None;
                self.exit.return_focus = None;
                self.exit.prompt.dismiss();
            }
            _ => {}
        }
        match self.exit.prompt.step(ev, cx) {
            PromptStep::Pass => None,
            PromptStep::Done(handled) => Some(handled),
            PromptStep::Answer(accept) => {
                if let Some(token) = self.exit.token.take() {
                    self.send(OttAction::ExitAnswer { token, accept }, fx);
                }
                let focus = self.exit.return_focus.take().map(FocusTarget::Elem)
                    .unwrap_or(FocusTarget::ContainerGroup(GroupId(0)));
                self.exit_focus(focus, fx);
                fx.invalidate(plx_machine::present::Provenance::Input);
                Some(Handled::Yes)
            }
        }
    }
}
