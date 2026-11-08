use super::*;
use plx_machine::machine::{Delivery, InstanceId, Stamped, UiMode};
use plx_screens::registry::{AppMsg, OttClockSample};
use plx_ui::screen::ScreenEvent;

#[derive(Default)]
struct ClockTap(Vec<(InstanceId, OttClockSample)>);
impl plx_ui::dispatch::Tap<bridge::AppHost> for ClockTap {
    fn effect(&mut self, _: u64, stamped: &Stamped<bridge::AppHost>) {
        if let Fx::Deliver(MachineId::Instance(owner), Delivery::Screen(ScreenEvent::App(AppMsg::OttClock(label)))) = &stamped.fx {
            self.0.push((*owner, label.clone()));
        }
    }
}
fn publish(app: &mut App) -> Vec<(InstanceId, OttClockSample)> {
    let mut runtime = app.ott.take().unwrap();
    super::super::classic_clock::publish(&mut runtime, app);
    app.ott = Some(runtime);
    let tick = next_tick(app);
    let mut tap = ClockTap::default();
    app.pages.frame_with(&mut app.bridge, tick, Vec::new(), Vec::new(), &mut tap, false);
    tap.0
}
fn mounted() -> App {
    let mut app = app_fixture();
    app.ott.as_mut().unwrap().saved.settings.insert("interfaceMode".into(), json!(0));
    app.bridge.apply_ui_mode(UiMode::Classic);
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    runtime.classic_clock = Default::default();
    runtime.wall_clock_ms = 1_730_000_001_000;
    assert!(runtime.clock_sample.is_some());
    assert!(runtime.reply.is_some());
    app
}

#[test]
fn clock_publication_is_addressed_and_does_not_rebuild_the_catalogue() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let runtime = app.ott.as_ref().unwrap();
    let before = (runtime.revision, runtime.generation, runtime.catalog_revision,
        runtime.playback_epoch, serde_json::to_value(&runtime.saved).unwrap());
    let owner = runtime.reply.unwrap();
    let first = publish(&mut app);
    assert_eq!(first.len(), 1); assert_eq!(first[0].0, owner); assert!(first[0].1.label.is_some());
    assert_eq!(first[0].1.utc_s, Some(1_730_000_001));
    assert!(publish(&mut app).is_empty());
    app.ott.as_mut().unwrap().wall_clock_ms += 1_000;
    let second = publish(&mut app);
    assert_eq!(second.len(), 1); assert_ne!(second[0].1.label, first[0].1.label);
    assert_eq!(second[0].1.utc_s, Some(1_730_000_002));
    let runtime = app.ott.as_ref().unwrap();
    assert_eq!((runtime.revision, runtime.generation, runtime.catalog_revision,
        runtime.playback_epoch, serde_json::to_value(&runtime.saved).unwrap()), before);
    app.pages.suspend();
    assert!(publish(&mut app).is_empty(), "a covered/suspended owner gets no clock");
}

#[test]
fn missing_utc_sample_clears_once_and_recovers_from_the_real_runtime_clock() {
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    let first = publish(&mut app);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].1.utc_s, Some(1_730_000_001));
    let sample = app.ott.as_mut().unwrap().clock_sample.take();
    assert!(sample.is_some());
    assert_eq!(publish(&mut app), vec![(first[0].0, OttClockSample::default())]);
    assert!(publish(&mut app).is_empty());
    let runtime = app.ott.as_mut().unwrap();
    runtime.clock_sample = sample;
    runtime.wall_clock_ms = 1_730_000_000_000;
    let recovered = publish(&mut app);
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].0, first[0].0);
    assert_eq!(recovered[0].1.utc_s, Some(1_730_000_000));
    assert!(recovered[0].1.label.is_some());
}

#[test]
fn controlled_modes_suppress_live_clock_and_live_return_republishes() {
    use crate::app::{bootstrap::{Initial, Preflight}, recorder::{Recplay, ReplayMode, state_fp}};
    use plx_ui::rec::{Header, Recording, MemSink};
    let _serial = plx_base::testlock::serial();
    let mut app = mounted();
    assert_eq!(publish(&mut app).len(), 1);
    let initial = Initial::synthetic_home(1, 32517, None).unwrap();
    app.rec = Recplay::recording_with_sink(&initial, Box::new(MemSink::default())).unwrap();
    assert!(publish(&mut app).is_empty());
    let recording = Recording { header: Header::new(state_fp(), &initial),
        frames: Vec::new(), metrics: Default::default(), stopped_at: None };
    app.rec = Recplay::controlled(Preflight::Replay { initial: initial.clone(), recording,
        mode: ReplayMode::Targets }, &initial).unwrap();
    assert!(publish(&mut app).is_empty());
    app.rec = Recplay::Off;
    assert_eq!(publish(&mut app).len(), 1, "same-second return to live resets recipient cache");
    app.bridge.apply_ui_mode(UiMode::Modern);
    assert!(publish(&mut app).is_empty());
    app.bridge.apply_ui_mode(UiMode::Classic);
    assert_eq!(publish(&mut app).len(), 1);
}
