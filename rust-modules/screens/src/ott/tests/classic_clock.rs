use super::*;
use plx_machine::machine::{UiMode, Tick};

struct ClockHost;
impl Host for ClockHost {
    type Arg = crate::registry::AppArg;
    type Fx = AppFx;
    type Msg = AppMsg;
    type Elem = u32;
    type Views<'a> = UiMode;
    type Init = crate::registry::AppArg;
    type Memory = PageMemory;
    fn ui_mode(mode: &UiMode) -> UiMode { *mode }
}

fn clock(screen: &mut OttScreen, mode: UiMode, value: Option<&str>) -> bool {
    clock_sample(screen, mode, crate::registry::OttClockSample { utc_s: None, label: value.map(str::to_owned) })
}

fn clock_sample(screen: &mut OttScreen, mode: UiMode, sample: crate::registry::OttClockSample) -> bool {
    let cx = Cx::<ClockHost> { views: mode, tick: Tick { ms: 99_000, dt_us: 0 },
        measure: &plx_ui::fixture::FixtureMeasure, press: Default::default(),
        focus: Default::default(), owner: plx_machine::machine::InputOwner::Entry(EntryId(1)) };
    let mut output = Vec::new();
    let mut present = plx_machine::present::Present::new();
    present.take(0); // Consume the initial frame debt before observing this delta.
    let mut fx = Effects::new(&mut output, MachineId::Instance(InstanceId(1)), &mut present);
    assert_eq!(screen.step(&ScreenEvent::App(AppMsg::OttClock(sample)), &cx, &mut fx), Handled::Yes);
    assert!(output.is_empty(), "clock emits no input, viewport or application request");
    present.changed()
}

fn canon(screen: &OttScreen) -> u64 {
    let mut out = Canon::new(); screen.write(&mut out); out.finish()
}

#[test]
fn clock_delta_preserves_catalogue_focus_guide_and_viewport_canon() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(7, &["one", "two"]));
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    screen.classic_visible = true;
    let key = screen.keys["item:two"];
    deliver(&mut screen, ScreenEvent::FocusMoved { from: None,
        to: FocusKey { entry: EntryId(1), elem: key }, by: plx_ui::screen::By::Dir });
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    let before = canon(&screen);
    let snapshot = screen.snapshot.clone();
    let layout = screen.classic_layout.get();
    assert!(clock(&mut screen, UiMode::Classic, Some("16:12:20")));
    assert_eq!(screen.classic_clock.as_deref(), Some("16:12:20"));
    assert_eq!(canon(&screen), before);
    assert!(Arc::ptr_eq(&screen.snapshot, &snapshot));
    assert_eq!(screen.classic_layout.get(), layout);
    assert_eq!(screen.form.selected_id().map(String::as_str), Some("item:two"));
    assert!(!clock(&mut screen, UiMode::Classic, Some("16:12:20")));
    assert!(clock(&mut screen, UiMode::Classic, Some("16:12:21")));
    assert!(clock(&mut screen, UiMode::Classic, None));
    assert_eq!(canon(&screen), before);
}

#[test]
fn hidden_and_modern_clock_observations_do_not_request_present() {
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(1, &["one"]));
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    let before = canon(&screen);
    assert!(!clock(&mut screen, UiMode::Classic, Some("16:12:20")));
    assert_eq!(canon(&screen), before);
    screen.classic_visible = true;
    let before = canon(&screen);
    assert!(!clock(&mut screen, UiMode::Modern, Some("16:12:21")));
    assert_eq!(canon(&screen), before);
}


#[test]
fn addressed_utc_updates_table_without_rebuilding_and_survives_a_form_refresh() {
    use crate::registry::OttClockSample;
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(snapshot(7, &["one", "two"]));
    screen.refresh_presentation(UiMode::Classic, &plx_ui::fixture::FixtureMeasure);
    screen.classic_visible = true;
    let initial = screen.snapshot.clone();
    let selected = screen.form.selected_id().cloned();
    let layout = screen.classic_layout.get();
    let sample = |utc_s| OttClockSample { utc_s: Some(utc_s), label: Some("16:12:20".into()) };
    assert!(clock_sample(&mut screen, UiMode::Classic, sample(1_730_000_001)));
    assert_eq!(screen.form.table.classic_clock(), Some(1_730_000_001));
    assert!(clock_sample(&mut screen, UiMode::Classic, sample(1_730_000_002)),
        "a UTC delta is visible even when its formatted label is unchanged");
    assert_eq!(screen.form.table.classic_clock(), Some(1_730_000_002));
    assert!(Arc::ptr_eq(&initial, &screen.snapshot));
    assert_eq!(screen.form.selected_id(), selected.as_ref());
    assert_eq!(screen.classic_layout.get(), layout);
    assert!(!clock_sample(&mut screen, UiMode::Classic, sample(1_730_000_002)));
    screen.install(snapshot(8, &["one", "two", "three"]));
    assert_eq!(screen.form.table.classic_clock(), Some(1_730_000_002),
        "a new catalogue/form retains the last addressed UTC until its next sample");
    assert!(clock_sample(&mut screen, UiMode::Classic, OttClockSample::default()));
    assert_eq!(screen.form.table.classic_clock(), None);
}
