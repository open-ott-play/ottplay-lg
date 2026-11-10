//! Real OTT card screen through the shared Tier 2 card-conformance driver.
use super::*;
use plx_machine::machine::{FocusRead, Host, InputOwner, PressRead, Tick};
use plx_ui::cards::conformance::{CardHarness, Landing, Nb};
use plx_ui::fixture::FixtureMeasure;
use plx_ui::screen::By;

struct TestHost;
impl Host for TestHost {
    type Arg = crate::registry::AppArg;
    type Fx = AppFx;
    type Msg = AppMsg;
    type Elem = u32;
    type Views<'a> = ();
    type Init = crate::registry::AppArg;
    type Memory = PageMemory;
}
const ENTRY: EntryId = EntryId(9);
struct Harness { screen: OttScreen, focus: Option<FocusKey<u32>>, press: f32, ms: u32 }
pub(crate) fn mount(n: usize) -> Box<dyn CardHarness> { Box::new(Harness::new(n)) }
fn item(id: String) -> OttRow {
    OttRow { current_programme: None, content_title: None, guide: None, guide_available: false, records_available: false, media: Some(OttMedia { kind: OttMediaKind::Movie, duration_ms: None, progress: None, synopsis: None }),
        title: id.clone(), action: OttAction::Play(id.clone()), id,
        detail: String::new(), description: String::new(), poster: None, enabled: true, favorite: false }
}
impl Harness {
    fn new(n: usize) -> Self {
        let mut h = Self { screen: OttScreen::new(ENTRY, InstanceId(9)), focus: None, press: 1.0, ms: 0 };
        h.step(ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(OttView { shelf_context: None, shelves: Vec::new(), status_working: false, pending_play: None, classic_preferences: Default::default(), volume_available: false, category_label: None,
            preview_context: Some("source-fixture".into()), random_context: None, order_context: None,
            page_id: "channels:fixture".into(), revision: 1, title: msg::ott_channel_list().into(),
            description: String::new(), status: String::new(), selected_row: None,
            rows: (0..n).map(|i| item(format!("movie-{i}"))).collect(), search: None, catalog_progress: None,
        }))));
        h.step(ScreenEvent::Tick(Tick { ms: 0, dt_us: 16_667 }));
        h
    }
    fn cx(&self) -> Cx<'_, TestHost> {
        Cx { views: (), tick: Tick { ms: self.ms, dt_us: 16_667 }, measure: &FixtureMeasure,
            press: PressRead { scale: self.press, owner: self.focus, ..Default::default() },
            focus: FocusRead { current: self.focus, ..Default::default() }, owner: InputOwner::Entry(ENTRY) }
    }
    fn step(&mut self, ev: ScreenEvent<TestHost>) -> bool {
        let mut present = plx_machine::present::Present::new();
        let mut out = Vec::new();
        let (_, moving) = plx_machine::idle::scoped_motion(|| {
            let cx = Cx { views: (), tick: Tick { ms: self.ms, dt_us: 16_667 }, measure: &FixtureMeasure,
                press: PressRead { scale: self.press, owner: self.focus, ..Default::default() },
                focus: FocusRead { current: self.focus, ..Default::default() }, owner: InputOwner::Entry(ENTRY) };
            let mut fx = Effects::new(&mut out, MachineId::Instance(InstanceId(9)), &mut present);
            Machine::<TestHost>::step(&mut self.screen, &ev, &cx, &mut fx);
        });
        moving || present.page_moving()
    }
    fn key(&self, elem: u32) -> FocusKey<u32> { FocusKey { entry: ENTRY, elem } }
    fn publish(&mut self, view: OttView) { self.step(ScreenEvent::App(AppMsg::OttSnapshot(Arc::new(view)))); }
}
impl CardHarness for Harness {
    fn cards(&self) -> Vec<u32> { self.screen.catalog_keys.clone() }
    fn focused(&self) -> Option<u32> { self.focus.map(|key| key.elem) }
    fn focus(&mut self, elem: u32, by: By) {
        let from = self.focus;
        self.focus = Some(self.key(elem));
        self.step(ScreenEvent::FocusMoved { from, to: self.key(elem), by });
    }
    fn tick(&mut self, frames: u32) -> bool {
        let mut moving = false;
        for _ in 0..frames { self.ms += 16; moving |= self.step(ScreenEvent::Tick(Tick { ms: self.ms, dt_us: 16_667 })); }
        moving
    }
    fn cover(&mut self) { self.step(ScreenEvent::Cover); }
    fn uncover(&mut self) { self.step(ScreenEvent::Uncover); }
    fn set_press(&mut self, scale: f32) { self.press = scale; }
    fn neighbour(&self, elem: u32, dir: Dir) -> Nb {
        match Focusable::<TestHost>::neighbour(&self.screen, self.key(elem), dir, &self.cx()) {
            Step::Move(key) => Nb::To(key.elem), Step::Edge => Nb::Edge,
        }
    }
    fn place(&self, elem: u32, at: At) -> Option<Placed> {
        Focusable::<TestHost>::place(&self.screen, &elem, &self.cx(), at)
    }
    fn drawn_rect(&self, elem: u32, press: f32) -> Option<Rect> {
        let cx = Cx { press: PressRead { scale: press, owner: self.focus, ..Default::default() }, ..self.cx() };
        let mut frame = DrawFrame::new(&cx, plx_ui::Painter::root());
        let view = self.screen.grid_view();
        view.stack.view(&view.page).record_stops(&mut frame);
        frame.stops().iter().find(|stop| stop.key.elem == elem).map(|stop| stop.rect)
    }
    fn scale(&self, elem: u32) -> Option<f32> {
        let view = self.screen.grid_view();
        view.stack.view(&view.page).scale_of(&self.cx(), &elem)
    }
    fn focus_scale(&self) -> f32 { plx_ui::cards::GRID_STYLE.focus_scale }
    fn canon(&self) -> u64 { LogicalState::hash(&self.screen) }
    fn identity(&self, elem: u32) -> String {
        self.screen.media_keys.get(&elem).map(|&i| self.screen.snapshot.rows[i].id.clone()).unwrap_or_default()
    }
    fn scroll(&self) -> Option<f32> { Some(self.screen.card_stack.scroll()) }
    fn columns(&self) -> Option<usize> { Some(media::columns(false)) }
    fn landing(&mut self, landing: Landing) -> Result<(), &'static str> {
        let want = self.focus.ok_or("nothing focused")?;
        let index = self.screen.media_keys.get(&want.elem).copied().ok_or("not a card")?;
        let mut view = (*self.screen.snapshot).clone();
        view.revision += 1;
        match landing {
            Landing::Reorder => view.rows.swap(0, 2),
            Landing::InsertAbove => view.rows.insert(0, item("inserted".into())),
            Landing::RemoveFocused => { view.rows.remove(index); },
        }
        self.publish(view);
        let now = Focusable::<TestHost>::reconcile(&self.screen, want, &self.cx());
        if now != want { self.focus(now.elem, By::Reconcile); }
        Ok(())
    }
    fn shift(&mut self, by: usize) -> Result<(), &'static str> {
        let mut view = (*self.screen.snapshot).clone();
        view.revision += 1;
        view.rows.splice(0..0, (0..by).map(|i| item(format!("shifted-{i}"))));
        self.publish(view);
        Ok(())
    }
    fn memory_roundtrip(&mut self) -> Result<Box<dyn CardHarness>, &'static str> {
        let memory = Screen::<TestHost>::memory_at(&self.screen, self.focus);
        let mut fresh = Harness { screen: OttScreen::new(ENTRY, InstanceId(9)), focus: None, press: 1.0, ms: 0 };
        fresh.step(ScreenEvent::RestoreMemory(memory));
        fresh.publish((*self.screen.snapshot).clone());
        let want = self.focus.ok_or("nothing focused")?;
        let now = Focusable::<TestHost>::reconcile(&fresh.screen, want, &fresh.cx());
        fresh.focus(now.elem, By::Restore);
        Ok(Box::new(fresh))
    }
}

#[test]
fn shared_card_contracts() {
    // run_all owns the serial guard for every mount and case.
    use plx_ui::cards::conformance::{run_all, Outcome, Mount};
    for (screen, case, result) in run_all(&[("ott", mount as Mount)]) {
        assert_eq!(result, Outcome::Pass, "{screen}: {case}: {result:?}");
    }
}
