//! Exercise the shipping input chain, not `activate` or synthetic PressHold delivery.
use super::*;
use plx_machine::machine::{FontScale, Host, InputEvent, InputOwner, NavOp, TimerId, TypographyMeasure, UiMode};
use plx_machine::present::Present;
use plx_ui::dispatch::{CxParts, Dispatcher, NoTap, Rig, Split};
use plx_ui::fixture::{key, key_up, tick, FixtureMeasure};
use plx_ui::screen::{Mounter, ReturnState};

struct TestHost;
impl Host for TestHost {
    type Arg = crate::registry::AppArg;
    type Fx = AppFx;
    type Msg = AppMsg;
    type Elem = u32;
    type Views<'a> = UiMode;
    type Init = crate::registry::AppArg;
    type Memory = PageMemory;
    fn ui_mode(mode: &UiMode) -> UiMode { *mode }
}

struct Mount;
impl Mounter<TestHost> for Mount {
    fn mount(&mut self, instance: InstanceId, _: &crate::registry::AppArg,
        _: &ReturnState<u32, PageMemory>, cx: &Cx<'_, TestHost>,
        _: &mut Effects<'_, TestHost>) -> Box<dyn Screen<TestHost>> {
        let InputOwner::Entry(entry) = cx.owner else { panic!("OTT page owner") };
        Box::new(OttScreen::new(entry, instance))
    }
}

mod exit_confirmation_tests {
    use super::*;
    fn exit_prompt(d: &mut Dispatcher<TestHost>, rig: &mut TestRig, token: u64, ms: u32) {
        let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
        d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
            Delivery::Screen(ScreenEvent::App(AppMsg::OttExitPrompt { token }))));
        for at in (ms..=ms + 800).step_by(16) { frame(d, rig, at, vec![]); }
    }
    fn answer_ok(d: &mut Dispatcher<TestHost>, rig: &mut TestRig, ms: u32, expected: OttAction) -> u32 {
        let before = rig.actions.len();
        frame(d, rig, ms, vec![key(Key::Ok, tick(ms))]);
        frame(d, rig, ms + 64, vec![key_up(Key::Ok, tick(ms + 64))]);
        for elapsed in (64..=2000).step_by(16) {
            let now = ms + elapsed;
            if rig.actions.len() != before {
                assert_eq!(&rig.actions[before..], &[expected]);
                return now;
            }
            frame(d, rig, now + 16, vec![]);
        }
        panic!("exit answer did not commit within bounded dispatcher ticks");
    }

    #[test]
    fn exit_confirmation_defaults_to_cancel_in_both_modes() {
        let _guard = plx_base::testlock::serial();
        for mode in [UiMode::Classic, UiMode::Modern] {
            let snapshot = navigation_view("providers", 1,
                vec![navigation_row("profiles", OttAction::ShowProfiles)], Some("profiles"));
            let (mut d, mut rig) = mount_snapshot_in_mode(snapshot, mode);
            exit_prompt(&mut d, &mut rig, 41, 300);
            let answered = answer_ok(&mut d, &mut rig, 1200, OttAction::ExitAnswer { token: 41, accept: false });
            frame(&mut d, &mut rig, answered + 16,
                vec![key(Key::Back, tick(answered + 16)), key_up(Key::Back, tick(answered + 16))]);
            assert_eq!(rig.actions, vec![OttAction::ExitAnswer { token: 41, accept: false }],
                "fade input must not escape into root Back");
            rig.actions.clear();
            let reopen = answered + 400;
            exit_prompt(&mut d, &mut rig, 42, reopen);
            let right = reopen + 900;
            frame(&mut d, &mut rig, right, vec![key(Key::Right, tick(right)), key_up(Key::Right, tick(right))]);
            let answered = answer_ok(&mut d, &mut rig, right + 64,
                OttAction::ExitAnswer { token: 42, accept: true });
            frame(&mut d, &mut rig, answered + 16,
                vec![key(Key::Back, tick(answered + 16)), key_up(Key::Back, tick(answered + 16))]);
            assert_eq!(rig.actions, vec![OttAction::ExitAnswer { token: 42, accept: true }],
                "affirmation and fade are one-shot");
        }
    }

    #[test]
    fn exit_confirmation_back_cancels_and_old_withdrawal_cannot_close_new_prompt() {
        let _guard = plx_base::testlock::serial();
        let snapshot = navigation_view("providers", 1,
            vec![navigation_row("profiles", OttAction::ShowProfiles)], Some("profiles"));
        let (mut d, mut rig) = mount_snapshot(snapshot);
        exit_prompt(&mut d, &mut rig, 41, 300);
        exit_prompt(&mut d, &mut rig, 42, 1200);
        let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
        d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
            Delivery::Screen(ScreenEvent::App(AppMsg::OttExitDismiss { token: 41 }))));
        frame(&mut d, &mut rig, 2100, vec![]);
        frame(&mut d, &mut rig, 2200, vec![key(Key::Back, tick(2200))]);
        assert_eq!(rig.actions, vec![OttAction::ExitAnswer { token: 42, accept: false }]);
        frame(&mut d, &mut rig, 2216, vec![key(Key::Back, tick(2216))]);
        frame(&mut d, &mut rig, 2232, vec![key_up(Key::Back, tick(2232))]);
        assert_eq!(rig.actions, vec![OttAction::ExitAnswer { token: 42, accept: false }]);
    }
}

struct TestRig {
    mount: Mount,
    mode: UiMode,
    measure: TypographyMeasure<FixtureMeasure>,
    snapshot: Arc<OttView>,
    actions: Vec<OttAction>,
    transition: Option<(OttAction, Arc<OttView>)>,
}
impl Rig<TestHost> for TestRig {
    fn split(&mut self) -> Split<'_, TestHost> {
        Split { mounter: &mut self.mount, views: self.mode, measure: &self.measure }
    }
    fn deliver(&mut self, _: MachineId, _: &AppMsg, _: &CxParts<u32>,
        _: &mut Effects<'_, TestHost>) -> Handled { Handled::No }
    fn timer(&mut self, _: MachineId, _: TimerId, _: &CxParts<u32>, _: &mut Effects<'_, TestHost>) {}
    fn app_fx(&mut self, _: MachineId, effect: AppFx, _: &CxParts<u32>, out: &mut Effects<'_, TestHost>) {
        match effect {
            AppFx::Ott(OttOut::Refresh { reply }) => out.push(Fx::Deliver(
                MachineId::Instance(reply), Delivery::Screen(ScreenEvent::App(
                    AppMsg::OttSnapshot(self.snapshot.clone()))))),
            AppFx::Ott(OttOut::Action { reply, action }) => {
                if self.transition.as_ref().is_some_and(|(expected, _)| *expected == action) {
                    let (_, snapshot) = self.transition.take().unwrap();
                    self.snapshot = snapshot.clone();
                    out.push(Fx::Deliver(MachineId::Instance(reply),
                        Delivery::Screen(ScreenEvent::App(AppMsg::OttSnapshot(snapshot)))));
                }
                self.actions.push(action);
            }
            _ => {}
        }
    }
    fn log(&mut self, _: &str) {}
    fn prepare(&mut self, _: &mut Budget, _: &mut Present) {}
    fn ls2_pump(&mut self) {}
    fn opaque_route(&mut self, _: bool) {}
    fn clear_opaque_region(&mut self) {}
    fn now_us(&self) -> u64 { 0 }
}

fn frame(d: &mut Dispatcher<TestHost>, rig: &mut TestRig, ms: u32, inputs: Vec<InputEvent<u32>>) {
    // No SDL/GL or network: input, focus, press, delivery and the real screen all run.
    let report = d.frame_with(rig, tick(ms), inputs, vec![], &mut NoTap, false);
    d.prune(&report.unmounted);
}

fn boot(favorite: bool) -> (Dispatcher<TestHost>, TestRig) {
    let snapshot = Arc::new(OttView { shelf_context: None, shelves: Vec::new(), status_working: false, pending_play: None, pending_step: None, pending_step_sdl: false, classic_preferences: Default::default(), volume_available: false, category_label: None, order_context: None, preview_context: Some("source-fixture".into()), random_context: None, catalog_progress: None, search: None,
        page_id: if favorite { "favorites" } else { "channels" }.into(),
        revision: 1, title: "Fixture videos".into(), description: String::new(),
        status: String::new(), selected_row: Some("video-one".into()),
        rows: vec![OttRow { current_programme: None, content_title: None, guide: None, guide_available: false, records_available: false, media: None, id: "video-one".into(), title: "Video one".into(),
            detail: String::new(), description: "A complete description.".into(),
            poster: None, action: OttAction::Play("video-one".into()), enabled: true, favorite }],
    });
    boot_snapshot(snapshot)
}

fn boot_snapshot(snapshot: Arc<OttView>) -> (Dispatcher<TestHost>, TestRig) {
    let (d, rig) = mount_snapshot(snapshot);
    let selected = format!("item:{}", rig.snapshot.selected_row.as_ref().unwrap());
    assert_focus(&d, &selected);
    (d, rig)
}

fn mount_snapshot(snapshot: Arc<OttView>) -> (Dispatcher<TestHost>, TestRig) {
    mount_snapshot_in_mode(snapshot, UiMode::Modern)
}

fn mount_snapshot_in_mode(snapshot: Arc<OttView>, mode: UiMode) -> (Dispatcher<TestHost>, TestRig) {
    let mut rig = TestRig { mount: Mount, mode, measure: TypographyMeasure::new(FixtureMeasure), snapshot,
        actions: Vec::new(), transition: None };
    let mut d = Dispatcher::new();
    d.nav.tabs.stack.transition = Box::new(plx_ui::containers::transition::Immediate);
    d.request(MachineId::Nav, NavOp::Root(crate::registry::AppArg::Ott));
    for ms in (0..=192).step_by(16) { frame(&mut d, &mut rig, ms, vec![]); }
    rig.actions.clear();
    (d, rig)
}

fn assert_focus(d: &Dispatcher<TestHost>, id: &str) {
    let page = d.nav.top_page().expect("OTT mounted");
    let screen = page.inst.as_ref().unwrap().screen.as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
    assert_eq!(d.focus(), Some(FocusKey { entry: page.id, elem: screen.keys[id] }),
        "the real snapshot must seat {id}");
}

fn navigation_row(id: &str, action: OttAction) -> OttRow {
    OttRow { current_programme: None, content_title: None, guide: None, guide_available: false, records_available: false, media: None, id: id.into(), title: id.into(), detail: String::new(), description: String::new(),
        poster: None, action, enabled: true, favorite: false }
}

fn navigation_view(page_id: &str, revision: u64, rows: Vec<OttRow>, selected: Option<&str>) -> Arc<OttView> {
    Arc::new(OttView { shelf_context: None, shelves: Vec::new(), status_working: false, pending_play: None, pending_step: None, pending_step_sdl: false, classic_preferences: Default::default(), volume_available: false, category_label: None, order_context: None, preview_context: Some("source-fixture".into()), random_context: None, catalog_progress: None, search: None, page_id: page_id.into(), revision, title: page_id.into(),
        description: String::new(), status: String::new(), rows,
        selected_row: selected.map(str::to_owned) })
}

fn publish_snapshot(d: &mut Dispatcher<TestHost>, rig: &mut TestRig, snapshot: Arc<OttView>, ms: u32) {
    rig.snapshot = snapshot.clone();
    let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
    d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
        Delivery::Screen(ScreenEvent::App(AppMsg::OttSnapshot(snapshot)))));
    frame(d, rig, ms, vec![]);
}

fn press_navigation_ok(d: &mut Dispatcher<TestHost>, rig: &mut TestRig, ms: u32, same_batch: bool) {
    if same_batch {
        frame(d, rig, ms, vec![key(Key::Ok, tick(ms)), key_up(Key::Ok, tick(ms))]);
    } else {
        frame(d, rig, ms, vec![key(Key::Ok, tick(ms))]);
        frame(d, rig, ms + 64, vec![key_up(Key::Ok, tick(ms + 64))]);
    }
    frame(d, rig, ms + 80, vec![]);
}

// Header pills are Control elements, unlike immediate Bare table/editor rows.
// Drive the shipping press through its commit and release spring; callers continue
// after the returned tick so no later input moves the fixture clock backwards.
fn press_header_ok(d: &mut Dispatcher<TestHost>, rig: &mut TestRig, ms: u32) -> u32 {
    press_navigation_ok(d, rig, ms, false);
    assert!(d.input.press.is_active(), "header activation waits for its press cycle");
    for elapsed in (96..=2000).step_by(16) {
        let now = ms + elapsed;
        frame(d, rig, now, vec![]);
        if !d.input.press.is_active() && d.input.arm.is_none() { return now; }
    }
    panic!("header press did not settle within the bounded fixture ticks");
}

fn focus_header(d: &mut Dispatcher<TestHost>, rig: &mut TestRig, id: &str, ms: u32) {
    frame(d, rig, ms, vec![key(Key::Up, tick(ms))]);
    for offset in 1..=3 {
        let page = d.nav.top_page().unwrap();
        let screen = page.inst.as_ref().unwrap().screen.as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
        if d.focus().is_some_and(|focus| focus.elem == screen.keys[id]) { break; }
        let now = ms + offset * 16;
        frame(d, rig, now, vec![key(Key::Left, tick(now))]);
    }
    assert_focus(d, id);
}

fn open_browse(d: &mut Dispatcher<TestHost>, rig: &mut TestRig, ms: u32) -> u32 {
    focus_header(d, rig, "nav:browse", ms);
    let completed = press_header_ok(d, rig, ms + 64);
    assert_focus(d, "nav:catalog");
    completed
}

fn discoverable_live_catalog(favorite: bool) -> Arc<OttView> {
    let mut view = (*navigation_view("channels:m3u-owner", 1,
        ["first", "selected"].into_iter().map(|id| {
            let mut row = navigation_row(id, OttAction::Play(id.into()));
            row.media = Some(OttMedia { kind: OttMediaKind::LiveChannel, duration_ms: None, progress: None, synopsis: None });
            row.description = "Current programme details".into();
            row.favorite = favorite;
            row
        }).collect(), Some("selected"))).clone();
    view.title = msg::ott_channel_list().into();
    view.category_label = Some("Long current category / ".repeat(16));
    view.search = Some(OttSearch { query: String::new(), scope: "Current M3U list".into() });
    Arc::new(view)
}

fn semantic_actions(rig: &TestRig) -> Vec<&OttAction> {
    rig.actions.iter().filter(|action| !matches!(action,
        OttAction::Preview(_) | OttAction::PreviewAt { .. } | OttAction::FocusAt { .. })).collect()
}

#[test]
fn dispatcher_modern_header_search_and_favorite_remain_visible_with_a_long_category() {
    let _guard = plx_base::testlock::serial();
    let _locale = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::Ru);
    for favorite in [false, true] {
        for action in ["nav:search", "nav:favorite"] {
            let (mut d, mut rig) = boot_snapshot(discoverable_live_catalog(favorite));
            // The actual placed/drawn controls must exist, not just action IDs.
            draw_pointer_map(&mut d, &rig, &["nav:browse", "nav:search", "nav:favorite"]);
            let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen.as_any().unwrap()
                .downcast_ref::<OttScreen>().unwrap();
            let header = screen.header(&rig.measure).unwrap();
            assert!(header.page.spec().right.iter().any(|item| item.label == if favorite {
                msg::ott_favorite_remove() } else { msg::ott_favorite_add() }));
            focus_header(&mut d, &mut rig, "nav:browse", 256);
            frame(&mut d, &mut rig, 320, vec![key(Key::Right, tick(320))]);
            assert_focus(&d, "nav:search");
            if action == "nav:favorite" {
                frame(&mut d, &mut rig, 336, vec![key(Key::Right, tick(336))]);
            }
            assert_focus(&d, action);
            press_header_ok(&mut d, &mut rig, 384);
            let expected = if action == "nav:search" { OttAction::BeginSearch }
                else { OttAction::ToggleFavorite("selected".into()) };
            assert_eq!(semantic_actions(&rig), vec![&expected]);
        }
    }
}

#[test]
fn dispatcher_browse_places_search_and_favorite_next_to_the_current_list() {
    let _guard = plx_base::testlock::serial();
    for favorite in [false, true] {
        for action in ["nav:search", "nav:favorite"] {
            let (mut d, mut rig) = boot_snapshot(discoverable_live_catalog(favorite));
            let completed = open_browse(&mut d, &mut rig, 256);
            frame(&mut d, &mut rig, completed + 16, vec![key(Key::Down, tick(completed + 16))]);
            assert_focus(&d, "nav:search");
            if action == "nav:favorite" {
                frame(&mut d, &mut rig, completed + 32, vec![key(Key::Down, tick(completed + 32))]);
            }
            assert_focus(&d, action);
            press_navigation_ok(&mut d, &mut rig, completed + 48, false);
            let expected = if action == "nav:search" { OttAction::BeginSearch }
                else { OttAction::ToggleFavorite("selected".into()) };
            assert_eq!(semantic_actions(&rig), vec![&expected], "Browse must never start playback");
            if action == "nav:search" {
                // The fixture records BeginSearch without publishing an editor;
                // Back closes the still-open Browse and restores its subject.
                frame(&mut d, &mut rig, completed + 160,
                    vec![key(Key::Back, tick(completed + 160))]);
            }
            assert_focus(&d, "item:selected");
        }
    }
}

#[test]
fn dispatcher_provider_to_profiles_focuses_setup_or_profile_without_bouncing() {
    let _guard = plx_base::testlock::serial();
    for same_batch in [false, true] {
        for configured in [false, true] {
            let providers = navigation_view("providers", 1,
                vec![navigation_row("m3u", OttAction::SelectProvider("m3u".into()))], None);
            let (mut d, mut rig) = mount_snapshot(providers);
            assert_focus(&d, "item:m3u");
            let mut rows = Vec::new();
            if configured {
                rows.push(navigation_row("m3u-home", OttAction::SelectProfile("m3u-home".into())));
            }
            rows.push(navigation_row("profile.add", OttAction::EditSetting("profile.add".into())));
            rig.transition = Some((OttAction::SelectProvider("m3u".into()),
                navigation_view("profiles:m3u", 2, rows, Some("obsolete-playing-channel"))));
            press_navigation_ok(&mut d, &mut rig, 256, same_batch);
            let expected = if configured { "item:m3u-home" } else { "item:profile.add" };
            assert_focus(&d, expected);
            assert_eq!(rig.actions, vec![OttAction::SelectProvider("m3u".into())],
                "one OK gesture must only open Profiles, including its release after the page changes");

            // A status-only update must not move focus back to the common navigation.
            let mut pending = (*rig.snapshot).clone();
            pending.revision += 1;
            pending.status = "Profile operation pending".into();
            publish_snapshot(&mut d, &mut rig, Arc::new(pending), 400);
            assert_focus(&d, expected);
            press_navigation_ok(&mut d, &mut rig, 480, same_batch);
            let action = if configured { OttAction::SelectProfile("m3u-home".into()) }
                else { OttAction::EditSetting("profile.add".into()) };
            assert_eq!(rig.actions, vec![OttAction::SelectProvider("m3u".into()), action]);
        }
    }
}

#[test]
fn dispatcher_empty_profiles_and_failure_refresh_stay_on_the_profiles_navigation() {
    let _guard = plx_base::testlock::serial();
    for same_batch in [false, true] {
        let (mut d, mut rig) = mount_snapshot(navigation_view("profiles:m3u", 1, vec![], None));
        assert_focus(&d, "nav:profiles");
        press_navigation_ok(&mut d, &mut rig, 256, same_batch);
        assert_eq!(rig.actions, vec![OttAction::ShowProfiles]);
        let mut failed = (*rig.snapshot).clone();
        failed.revision += 1;
        failed.status = "Profiles are unavailable".into();
        publish_snapshot(&mut d, &mut rig, Arc::new(failed), 400);
        frame(&mut d, &mut rig, 480, vec![]);
        assert_focus(&d, "nav:profiles");
        assert_eq!(rig.actions, vec![OttAction::ShowProfiles], "status updates never navigate");
    }
}

#[test]
fn dispatcher_profile_open_prefers_valid_enabled_content_and_refresh_preserves_focus() {
    let _guard = plx_base::testlock::serial();
    let rows = vec![navigation_row("one", OttAction::SelectProfile("one".into())),
        navigation_row("two", OttAction::SelectProfile("two".into()))];
    let (mut d, mut rig) = mount_snapshot(navigation_view("profiles:m3u", 1, rows.clone(), Some("two")));
    assert_focus(&d, "item:two");
    publish_snapshot(&mut d, &mut rig, navigation_view("profiles:m3u", 2, rows.clone(), Some("one")), 256);
    assert_focus(&d, "item:two");
    let mut disabled = rows;
    disabled[0].enabled = false;
    publish_snapshot(&mut d, &mut rig, navigation_view("profiles:stalker", 3, disabled, Some("one")), 288);
    assert_focus(&d, "item:two");
    assert!(rig.actions.is_empty());
}

#[test]
fn dispatcher_long_ok_toggles_favorite_once_without_playing() {
    let _guard = plx_base::testlock::serial();
    for favorite in [false, true] {
        let (mut d, mut rig) = boot(favorite);
        frame(&mut d, &mut rig, 256, vec![key(Key::Ok, tick(256))]);
        assert!(!rig.actions.iter().any(|a| matches!(a, OttAction::Play(_))),
            "OK Down must wait for tap vs hold instead of starting playback");
        let released = 256 + plx_ui::press::LONG_MS + 64;
        for ms in (272..released).step_by(16) { frame(&mut d, &mut rig, ms, vec![]); }
        frame(&mut d, &mut rig, released, vec![key_up(Key::Ok, tick(released))]);
        for ms in (released + 16..=released + 320).step_by(16) { frame(&mut d, &mut rig, ms, vec![]); }
        assert_eq!(rig.actions.iter().filter(|a| matches!(a,
            OttAction::ToggleFavorite(id) if id == "video-one")).count(), 1,
            "one held gesture adds or removes exactly one Favorite");
        assert!(!rig.actions.iter().any(|a| matches!(a, OttAction::Play(_))),
            "release after a Favorite hold must not also play");
    }
}

#[test]
fn dispatcher_short_ok_plays_once_without_toggling_favorite() {
    let _guard = plx_base::testlock::serial();
    for same_batch in [false, true] {
        let (mut d, mut rig) = boot(false);
        if same_batch {
            // Remote input delivers Down and Up at the same timestamp in one frame.
            frame(&mut d, &mut rig, 256,
                vec![key(Key::Ok, tick(256)), key_up(Key::Ok, tick(256))]);
        } else {
            frame(&mut d, &mut rig, 256, vec![key(Key::Ok, tick(256))]);
            frame(&mut d, &mut rig, 320, vec![key_up(Key::Ok, tick(320))]);
        }
        for ms in (336..=896).step_by(16) { frame(&mut d, &mut rig, ms, vec![]); }
        assert_eq!(rig.actions.iter().filter(|a| matches!(a,
            OttAction::Play(id) if id == "video-one")).count(), 1);
        assert!(!rig.actions.iter().any(|a| matches!(a, OttAction::ToggleFavorite(_))));
    }
}

#[test]
fn dispatcher_back_closes_description_before_leaving_catalogue() {
    let _guard = plx_base::testlock::serial();
    let (mut d, mut rig) = boot(false);
    let reading = |d: &Dispatcher<TestHost>| {
        d.nav.top_page().unwrap().inst.as_ref().unwrap().screen.as_any().unwrap()
            .downcast_ref::<OttScreen>().unwrap().reading
    };
    frame(&mut d, &mut rig, 256, vec![key(Key::Right, tick(256))]);
    assert!(reading(&d), "the engine's Right edge opens the description");
    frame(&mut d, &mut rig, 272, vec![key(Key::Back, tick(272))]);
    assert!(!reading(&d));
    assert!(!rig.actions.iter().any(|a| matches!(a, OttAction::Back)),
        "Back from the description must keep the catalogue open");
    frame(&mut d, &mut rig, 288, vec![key(Key::Back, tick(288))]);
    assert_eq!(rig.actions.iter().filter(|a| matches!(a, OttAction::Back)).count(), 1);
}

#[test]
fn dispatcher_navigation_and_editor_keep_immediate_ok() {
    let _guard = plx_base::testlock::serial();
    let (mut d, mut rig) = boot(false);
    let completed = open_browse(&mut d, &mut rig, 256);
    for offset in [16, 32, 48, 64] {
        let now = completed + offset;
        frame(&mut d, &mut rig, now, vec![key(Key::Down, tick(now))]);
    }
    assert_focus(&d, "nav:settings");
    frame(&mut d, &mut rig, completed + 80, vec![key(Key::Ok, tick(completed + 80))]);
    assert_eq!(rig.actions.iter().filter(|a| matches!(a, OttAction::ShowSettings)).count(), 1,
        "navigation still activates on Down, without waiting for release");
    let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
    d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
        Delivery::Screen(ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor { text_policy: OttTextPolicy::Predictive, kind: OttEditorKind::Setting,
            session: 7, key: "profile.name".into(), title: "Name".into(), value: "Fixture".into(),
            secret: false, limit: 64,
        }))))));
    frame(&mut d, &mut rig, completed + 96, vec![key_up(Key::Ok, tick(completed + 96))]);
    frame(&mut d, &mut rig, completed + 112, vec![key(Key::Down, tick(completed + 112))]);
    frame(&mut d, &mut rig, completed + 128, vec![key(Key::Ok, tick(completed + 128))]);
    assert_eq!(rig.actions.iter().filter(|a| matches!(a,
        OttAction::SubmitSetting { key, value, .. } if key == "profile.name" && value == "Fixture")).count(), 1,
        "Save still commits immediately on Down");
    assert!(!rig.actions.iter().any(|a| matches!(a, OttAction::Play(_) | OttAction::ToggleFavorite(_))));
}

#[test]
fn dispatcher_description_follows_selected_row_and_async_metadata() {
    let _guard = plx_base::testlock::serial();
    let rows = [("empty-first", ""), ("described", "Complete description."), ("empty-last", "")]
        .into_iter().map(|(id, description)| OttRow { current_programme: None, content_title: None, guide: None,
            guide_available: false, records_available: false, media: None,
            id: id.into(), title: id.into(), detail: String::new(), description: description.into(),
            poster: None, action: OttAction::Play(id.into()), enabled: true, favorite: false,
        }).collect();
    let (mut d, mut rig) = boot_snapshot(Arc::new(OttView { shelf_context: None, shelves: Vec::new(), status_working: false, pending_play: None, pending_step: None, pending_step_sdl: false, classic_preferences: Default::default(), volume_available: false, category_label: None, order_context: None, preview_context: Some("source-fixture".into()), random_context: None, catalog_progress: None, search: None,
        page_id: "channels".into(), revision: 1, title: "Fixture videos".into(),
        description: String::new(), status: String::new(), rows,
        selected_row: Some("empty-first".into()),
    }));
    let assert_selection = |d: &Dispatcher<TestHost>, id: &str, available: bool| {
        let page = d.nav.top_page().unwrap();
        let screen = page.inst.as_ref().unwrap().screen.as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
        assert_eq!(screen.header(&FixtureMeasure).is_some_and(|header|
            header.stops.iter().any(|(key, _)| screen.keys.get("nav:description") == Some(key))), available,
            "Description must reflect the selected row, not another catalogue item");
        assert_eq!(d.focus(), Some(FocusKey { entry: page.id, elem: screen.keys[&format!("item:{id}")] }),
            "adding/removing Description must retain the selected item identity");
    };
    assert_selection(&d, "empty-first", false);
    frame(&mut d, &mut rig, 256, vec![key(Key::Down, tick(256))]);
    assert_selection(&d, "described", true);
    frame(&mut d, &mut rig, 272, vec![key(Key::Down, tick(272))]);
    assert_selection(&d, "empty-last", false);
    // Metadata lands without a focus move; the same selected ID must gain/lose its action.
    for (ms, description, available) in [(288, "Details arrived.", true), (304, "", false)] {
        let snapshot = Arc::make_mut(&mut rig.snapshot);
        snapshot.revision += 1;
        snapshot.rows[2].description = description.into();
        let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
        d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
            Delivery::Screen(ScreenEvent::App(AppMsg::OttSnapshot(rig.snapshot.clone())))));
        frame(&mut d, &mut rig, ms, vec![]);
        assert_selection(&d, "empty-last", available);
    }
    assert!(rig.actions.iter().all(|a| matches!(a, OttAction::Preview(_) | OttAction::PreviewAt { .. } | OttAction::FocusAt { .. })),
        "selection and metadata changes must not play, favorite, or navigate");
}


#[test]
fn dispatcher_editor_save_keeps_draft_until_the_host_confirms_it() {
    let _guard = plx_base::testlock::serial();
    let (mut d, mut rig) = boot(false);
    let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
    d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
        Delivery::Screen(ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor { text_policy: OttTextPolicy::Literal, kind: OttEditorKind::Setting,
            session: 41, key: "profile.source".into(), title: "Source".into(),
            value: "private-source".into(), secret: true, limit: 256,
        }))))));
    frame(&mut d, &mut rig, 256, vec![]);
    frame(&mut d, &mut rig, 272, vec![key(Key::Down, tick(272))]);
    frame(&mut d, &mut rig, 288, vec![key(Key::Ok, tick(288))]);
    frame(&mut d, &mut rig, 304, vec![key_up(Key::Ok, tick(304))]);
    let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen.as_any().unwrap()
        .downcast_ref::<OttScreen>().unwrap();
    assert!(screen.handles_private_text(), "Save cannot discard the draft before validation/durable acknowledgement");
    assert_eq!(screen.editor.as_ref().unwrap().draft.text(), "private-source");
    assert_eq!(rig.actions.iter().filter(|action| matches!(action, OttAction::SubmitSetting { .. })).count(), 1);
}

#[test]
fn dispatcher_editor_cancel_returns_to_add_profile_and_can_reopen_it() {
    let _guard = plx_base::testlock::serial();
    let snapshot = Arc::new(OttView { shelf_context: None, shelves: Vec::new(), status_working: false, pending_play: None, pending_step: None, pending_step_sdl: false, classic_preferences: Default::default(), volume_available: false, category_label: None, order_context: None, preview_context: Some("source-fixture".into()), random_context: None, catalog_progress: None, search: None,
        page_id: "profiles:m3u".into(), revision: 1, title: "Profiles".into(),
        description: String::new(), status: String::new(), selected_row: Some("profile.add".into()),
        rows: vec![OttRow { current_programme: None, content_title: None, guide: None, guide_available: false, records_available: false, media: None, id: "profile.add".into(), title: "Add profile".into(),
            detail: String::new(), description: String::new(), poster: None,
            action: OttAction::EditSetting("profile.add".into()), enabled: true, favorite: false }],
    });
    let (mut d, mut rig) = boot_snapshot(snapshot);
    let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
    d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
        Delivery::Screen(ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor { text_policy: OttTextPolicy::Literal, kind: OttEditorKind::Setting,
            session: 42, key: "profile.source".into(), title: "Source".into(),
            value: "draft".into(), secret: true, limit: 256,
        }))))));
    frame(&mut d, &mut rig, 256, vec![]);
    frame(&mut d, &mut rig, 272, vec![key(Key::Back, tick(272))]);
    let page = d.nav.top_page().unwrap();
    let screen = page.inst.as_ref().unwrap().screen.as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
    assert!(!screen.handles_private_text());
    assert_eq!(d.focus(), Some(FocusKey { entry: page.id, elem: screen.keys["item:profile.add"] }));
    frame(&mut d, &mut rig, 288, vec![key(Key::Ok, tick(288))]);
    frame(&mut d, &mut rig, 304, vec![key_up(Key::Ok, tick(304))]);
    assert_eq!(rig.actions, vec![OttAction::CancelSetting { session: 42 }, OttAction::EditSetting("profile.add".into())]);
}

#[test]
fn dispatcher_editor_pending_cancel_and_stale_ack_keep_the_new_session() {
    let _guard = plx_base::testlock::serial();
    for same_batch in [false, true] {
        let (mut d, mut rig) = mount_snapshot(navigation_view("profiles:m3u", 1,
            vec![navigation_row("profile.add", OttAction::EditSetting("profile.add".into()))], None));
        let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
        let send_editor = |d: &mut Dispatcher<TestHost>, session, value: &str| {
            d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
                Delivery::Screen(ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor { text_policy: OttTextPolicy::Literal, kind: OttEditorKind::Setting,
                    session, key: "profile.source".into(), title: "Source".into(),
                    value: value.into(), secret: true, limit: 256,
                }))))));
        };
        let editor_state = |d: &Dispatcher<TestHost>| {
            let page = d.nav.top_page().unwrap();
            let screen = page.inst.as_ref().unwrap().screen.as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
            screen.editor.as_ref().map(|editor| (editor.spec.session, editor.pending, editor.draft.text().to_owned()))
        };
        send_editor(&mut d, 71, "first-source");
        frame(&mut d, &mut rig, 256, vec![]);
        frame(&mut d, &mut rig, 272, vec![key(Key::Down, tick(272))]);
        press_navigation_ok(&mut d, &mut rig, 288, same_batch);
        press_navigation_ok(&mut d, &mut rig, 400, same_batch);
        assert_eq!(rig.actions.iter().filter(|action| matches!(action, OttAction::SubmitSetting { .. })).count(), 1);
        assert_eq!(editor_state(&d), Some((71, true, "first-source".into())));
        frame(&mut d, &mut rig, 496, vec![key(Key::Down, tick(496))]);
        press_navigation_ok(&mut d, &mut rig, 512, same_batch);
        assert_eq!(editor_state(&d), None);
        assert_focus(&d, "item:profile.add");

        send_editor(&mut d, 72, "second-source");
        frame(&mut d, &mut rig, 624, vec![]);
        frame(&mut d, &mut rig, 640, vec![key(Key::Down, tick(640))]);
        press_navigation_ok(&mut d, &mut rig, 656, same_batch);
        for (at, error) in [(768, None), (784, Some("Old failure".into()))] {
            d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
                Delivery::Screen(ScreenEvent::App(AppMsg::OttEditorResult { session: 71, error }))));
            frame(&mut d, &mut rig, at, vec![]);
            assert_eq!(editor_state(&d), Some((72, true, "second-source".into())));
        }
        d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
            Delivery::Screen(ScreenEvent::App(AppMsg::OttEditorResult { session: 72, error: None }))));
        frame(&mut d, &mut rig, 800, vec![]);
        assert_eq!(editor_state(&d), None);
        assert_focus(&d, "item:profile.add");
    }
}

#[test]
fn dispatcher_search_uses_addressed_editor_and_never_the_settings_protocol() {
    let _guard = plx_base::testlock::serial();
    let mut snapshot = (*navigation_view("channels", 1, vec![OttRow { current_programme: None, content_title: None, guide: None,
        guide_available: false, records_available: false, media: None,
        id: "video-one".into(), title: "Video one".into(), detail: String::new(),
        description: "Description".into(), poster: None, action: OttAction::Play("video-one".into()),
        enabled: true, favorite: false,
    }], Some("video-one"))).clone();
    snapshot.search = Some(OttSearch { query: String::new(), scope: "Home".into() });
    let (mut d, mut rig) = boot_snapshot(Arc::new(snapshot));
    focus_header(&mut d, &mut rig, "nav:search", 256);
    let completed = press_header_ok(&mut d, &mut rig, 320);
    assert_eq!(rig.actions.iter().filter(|a| matches!(a, OttAction::BeginSearch)).count(), 1);
    let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
    d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
        Delivery::Screen(ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor { text_policy: OttTextPolicy::Predictive,
            kind: OttEditorKind::Search, session: 81, key: String::new(),
            title: String::new(), value: "Video".into(), secret: false, limit: 256,
        }))))));
    frame(&mut d, &mut rig, completed + 16, vec![]);
    frame(&mut d, &mut rig, completed + 32, vec![key(Key::Down, tick(completed + 32))]);
    press_navigation_ok(&mut d, &mut rig, completed + 48, false);
    assert_eq!(rig.actions.iter().filter(|a| matches!(a,
        OttAction::SubmitSearch { session: 81, value } if value == "Video")).count(), 1);
    frame(&mut d, &mut rig, completed + 144, vec![key(Key::Back, tick(completed + 144))]);
    assert_eq!(rig.actions.iter().filter(|a| matches!(a, OttAction::CancelSearch { session: 81 })).count(), 1);
    assert_focus(&d, "nav:search");
    assert!(!rig.actions.iter().any(|a| matches!(a,
        OttAction::SubmitSetting { .. } | OttAction::CancelSetting { .. } | OttAction::Play(_) | OttAction::ToggleFavorite(_))));
}

#[test]
fn dispatcher_browse_is_separate_and_back_returns_to_the_same_media_item() {
    let _guard = plx_base::testlock::serial();
    let (mut d, mut rig) = boot(false);
    let completed = open_browse(&mut d, &mut rig, 256);
    let page = d.nav.top_page().unwrap();
    let screen = page.inst.as_ref().unwrap().screen.as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
    assert!(screen.browse);
    assert!(screen.form.index_of(&"item:video-one".to_owned()).is_none());
    frame(&mut d, &mut rig, completed + 16, vec![key(Key::Back, tick(completed + 16))]);
    assert_focus(&d, "item:video-one");
    assert!(rig.actions.iter().all(|action| matches!(action, OttAction::Preview(_) | OttAction::PreviewAt { .. } | OttAction::FocusAt { .. })));
}

#[test]
fn dispatcher_disabled_media_never_opens_another_items_description() {
    let _guard = plx_base::testlock::serial();
    let rows = ["enabled", "disabled"].into_iter().map(|id| OttRow { current_programme: None, content_title: None, guide: None,
        guide_available: false, records_available: false, media: None,
        id: id.into(), title: id.into(), detail: String::new(), description: "Full description".into(),
        poster: None, action: OttAction::Play(id.into()), enabled: id == "enabled", favorite: false,
    }).collect();
    let (mut d, mut rig) = boot_snapshot(navigation_view("channels", 1, rows, Some("enabled")));
    frame(&mut d, &mut rig, 256, vec![key(Key::Down, tick(256))]);
    assert_focus(&d, "item:disabled");
    frame(&mut d, &mut rig, 272, vec![key(Key::Right, tick(272))]);
    let page = d.nav.top_page().unwrap();
    let screen = page.inst.as_ref().unwrap().screen.as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
    assert!(!screen.reading);
    assert_eq!(screen.media, None);
    assert!(!screen.header(&FixtureMeasure).unwrap().stops.iter().any(|(key, _)| screen.keys.get("nav:description") == Some(key)));
    press_navigation_ok(&mut d, &mut rig, 304, false);
    assert!(!rig.actions.iter().any(|action| matches!(action,
        OttAction::Play(_) | OttAction::ToggleFavorite(_))
        || matches!(action, OttAction::Preview(id) | OttAction::PreviewAt { id, .. } | OttAction::FocusAt { id, .. } if id == "disabled")));
}

fn card_snapshot(page: &str, revision: u64, kind: OttMediaKind, ids: &[&str], selected: &str) -> Arc<OttView> {
    let mut view = (*navigation_view(page, revision, Vec::new(), Some(selected))).clone();
    view.rows = ids.iter().map(|id| OttRow { current_programme: None, content_title: None, guide: None,
        guide_available: false, records_available: false, media: Some(OttMedia { kind, duration_ms: None, progress: None, synopsis: None }),
        id: (*id).into(), title: (*id).into(), detail: String::new(), description: "Full description".into(),
        poster: None, action: OttAction::Play((*id).into()), enabled: true, favorite: false,
    }).collect();
    Arc::new(view)
}

#[test]
fn dispatcher_cards_restore_selection_and_append_without_retargeting() {
    let _guard = plx_base::testlock::serial();
    let (mut d, mut rig) = boot_snapshot(card_snapshot("channels:one", 1, OttMediaKind::Movie, &["first", "second"], "second"));
    assert_focus(&d, "item:second");
    publish_snapshot(&mut d, &mut rig, card_snapshot("channels:one", 2, OttMediaKind::Movie, &["zero", "first", "second", "third"], "first"), 256);
    assert_focus(&d, "item:second");
    publish_snapshot(&mut d, &mut rig, card_snapshot("channels:two", 3, OttMediaKind::LiveChannel, &["first", "second", "third"], "third"), 272);
    assert_focus(&d, "item:third");
    assert!(rig.actions.iter().all(|action| matches!(action, OttAction::Preview(_) | OttAction::PreviewAt { .. } | OttAction::FocusAt { .. })));
}

#[test]
fn dispatcher_card_tap_and_hold_each_emit_one_semantic_action() {
    let _guard = plx_base::testlock::serial();
    for hold in [false, true] {
        let (mut d, mut rig) = boot_snapshot(card_snapshot("channels:one", 1, OttMediaKind::Movie, &["first"], "first"));
        frame(&mut d, &mut rig, 256, vec![key(Key::Ok, tick(256))]);
        let release = if hold { 256 + plx_ui::press::LONG_MS + 64 } else { 320 };
        for ms in (272..release).step_by(16) { frame(&mut d, &mut rig, ms, vec![]); }
        frame(&mut d, &mut rig, release, vec![key_up(Key::Ok, tick(release))]);
        for ms in (release + 16..=release + 320).step_by(16) { frame(&mut d, &mut rig, ms, vec![]); }
        let actions: Vec<_> = rig.actions.iter().filter(|action| !matches!(action, OttAction::Preview(_) | OttAction::PreviewAt { .. } | OttAction::FocusAt { .. })).collect();
        let expected = if hold { OttAction::ToggleFavorite("first".into()) } else { OttAction::Play("first".into()) };
        assert_eq!(actions, vec![&expected]);
    }
}

#[test]
fn dispatcher_card_browse_and_back_preserve_the_exact_subject() {
    let _guard = plx_base::testlock::serial();
    let (mut d, mut rig) = boot_snapshot(card_snapshot("channels:one", 1, OttMediaKind::Movie, &["first", "second"], "second"));
    let completed = open_browse(&mut d, &mut rig, 256);
    frame(&mut d, &mut rig, completed + 16, vec![key(Key::Back, tick(completed + 16))]);
    assert_focus(&d, "item:second");
    assert!(rig.actions.iter().all(|action| matches!(action, OttAction::Preview(_) | OttAction::PreviewAt { .. } | OttAction::FocusAt { .. })));
}

#[test]
fn dispatcher_reader_subject_loss_closes_reader_and_recovers_browse_focus() {
    let _guard = plx_base::testlock::serial();
    for removed in [false, true] {
        let (mut d, mut rig) = boot(false);
        frame(&mut d, &mut rig, 256, vec![key(Key::Right, tick(256))]);
        let mut view = (*rig.snapshot).clone();
        view.revision += 1;
        if removed { view.rows.clear(); } else { view.rows[0].enabled = false; }
        publish_snapshot(&mut d, &mut rig, Arc::new(view), 272);
        let page = d.nav.top_page().unwrap();
        let screen = page.inst.as_ref().unwrap().screen.as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
        assert!(!screen.reading);
        assert_eq!(screen.media, None);
        assert_focus(&d, "nav:browse");
        rig.actions.clear();
        frame(&mut d, &mut rig, 288, vec![key(Key::Back, tick(288))]);
        assert_eq!(rig.actions, vec![OttAction::Back]);
    }
}

// Use the actual screen draw to populate the same map that native pointer input reads.
// Recording and frame-clear suppression are the existing no-GL test seams.
fn draw_pointer_map(d: &mut Dispatcher<TestHost>, rig: &TestRig, controls: &[&str]) -> Vec<Rect> {
    draw_pointer_map_with_activation(d, rig, controls, plx_ui::screen::Activate::Press)
}

fn draw_pointer_map_with_activation(d: &mut Dispatcher<TestHost>, rig: &TestRig,
    controls: &[&str], activation: plx_ui::screen::Activate) -> Vec<Rect> {
    let current = d.focus();
    let page = d.nav.tabs.stack.top_mut().unwrap();
    let screen = page.inst.as_mut().unwrap().screen.as_any_mut().unwrap().downcast_mut::<OttScreen>().unwrap();
    let cx = Cx { views: rig.mode, tick: tick(240), measure: &rig.measure,
        press: Default::default(), focus: plx_machine::machine::FocusRead { current, ..Default::default() },
        owner: InputOwner::Entry(page.id) };
    let placed: Vec<_> = controls.iter().map(|id| {
        let elem = screen.keys[*id];
        let placed = Focusable::<TestHost>::place(screen, &elem, &cx, At::Drawn).expect("visible header control");
        (FocusKey { entry: page.id, elem }, placed.rect.intersect(placed.clip))
    }).collect();
    let mut draw = DrawFrame::new(&cx, plx_ui::Painter::recording());
    plx_ui::screen::record_stops_while_recording(|| {
        plx_gfx::gfx::without_frame_clear(|| Screen::<TestHost>::draw(screen, &mut draw));
    });
    let stops = draw.into_stops();
    for (key, rect) in &placed {
        let matching: Vec<_> = stops.iter().filter(|stop| stop.key == *key).collect();
        assert_eq!(matching.len(), 1, "one drawn stop for {key:?}");
        assert_eq!(matching[0].rect.intersect(matching[0].clip), *rect);
        assert_eq!(matching[0].hover, plx_ui::screen::Hover::Focus);
        assert_eq!(matching[0].activate, activation);
    }
    d.input.hit.fill(stops);
    d.input.hit.swap();
    let gaps = plx_ui::hit::pointer_gaps(&mut d.input.hit, page.id, &placed);
    assert!(gaps.is_empty(), "{}%: {}", rig.measure.scale().percent(), gaps.join("\n"));
    placed.into_iter().map(|(_, rect)| rect).collect()
}

fn click_header(d: &mut Dispatcher<TestHost>, rig: &mut TestRig, rect: Rect, ms: u32) {
    // Supply coordinates only; the shipping dispatcher must resolve the real drawn stop.
    // Start in D-pad mode as the real session did. A click must still focus and act.
    d.input.hit.note_dpad();
    frame(d, rig, ms, vec![InputEvent { at: tick(ms), source: plx_machine::machine::Source::Script,
        kind: InputKind::Click { x: rect.cx(), y: rect.cy(), hit: None } }]);
    frame(d, rig, ms + 64, vec![key_up(Key::Ok, tick(ms + 64))]);
    for now in (ms + 80..=ms + 400).step_by(16) { frame(d, rig, now, vec![]); }
}

#[test]
fn dispatcher_browse_pointer_opens_and_closes_at_supported_font_extremes() {
    let _guard = plx_base::testlock::serial();
    let _locale = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    for percent in [20, 100, 200] {
        let (mut d, mut rig) = mount_snapshot(navigation_view("providers", 1,
            vec![navigation_row("m3u", OttAction::SelectProvider("m3u".into()))], None));
        rig.measure.set_font_scale(FontScale::from_percent(percent).unwrap());
        assert_focus(&d, "item:m3u");
        let rect = draw_pointer_map(&mut d, &rig, &["nav:browse"])[0];
        click_header(&mut d, &mut rig, rect, 256);
        assert_focus(&d, "nav:providers");
        let page = d.nav.top_page().unwrap();
        let screen = page.inst.as_ref().unwrap().screen.as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
        assert!(screen.browse, "{percent}%: pointer opens Browse");
        assert!(rig.actions.is_empty(), "opening Browse must not select the provider");

        let rect = draw_pointer_map(&mut d, &rig, &["nav:browse"])[0];
        click_header(&mut d, &mut rig, rect, 704);
        assert_focus(&d, "item:m3u");
        let page = d.nav.top_page().unwrap();
        let screen = page.inst.as_ref().unwrap().screen.as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
        assert!(!screen.browse, "{percent}%: a second click closes Browse");
        assert!(rig.actions.is_empty(), "closing Browse must not activate its remembered row");
    }
}

#[test]
fn dispatcher_search_and_description_pointer_targets_keep_their_actions() {
    let _guard = plx_base::testlock::serial();
    let _locale = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    for percent in [20, 100, 200] {
        for selected in [1, 2] {
            let mut snapshot = (*navigation_view("channels", 1, vec![OttRow { current_programme: None, content_title: None, guide: None, guide_available: false, records_available: false,
                media: None, id: "video-one".into(), title: "Video one".into(), detail: String::new(),
                description: "Full description".into(), poster: None, action: OttAction::Play("video-one".into()),
                enabled: true, favorite: false,
            }], Some("video-one"))).clone();
            snapshot.search = Some(OttSearch { query: String::new(), scope: "Home".into() });
            let (mut d, mut rig) = boot_snapshot(Arc::new(snapshot));
            rig.measure.set_font_scale(FontScale::from_percent(percent).unwrap());
            let rects = draw_pointer_map(&mut d, &rig, &["nav:browse", "nav:search", "nav:description"]);
            click_header(&mut d, &mut rig, rects[selected], 256);
            let actions: Vec<_> = rig.actions.iter().filter(|action| !matches!(action, OttAction::Preview(_) | OttAction::PreviewAt { .. } | OttAction::FocusAt { .. })).collect();
            let page = d.nav.top_page().unwrap();
            let screen = page.inst.as_ref().unwrap().screen.as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
            assert!(!screen.browse);
            if selected == 1 {
                assert_eq!(actions, vec![&OttAction::BeginSearch]);
                assert!(!screen.reading);
            } else {
                assert!(actions.is_empty(), "Description must not play or favorite the media");
                assert!(screen.reading);
                let header_keys: Vec<_> = ["nav:browse", "nav:search", "nav:description"].iter()
                    .map(|id| screen.keys[*id]).collect();
                draw_pointer_map(&mut d, &rig, &[]);
                assert!(d.input.hit.front().iter().all(|stop| !header_keys.contains(&stop.key.elem)),
                    "the reader must not retain hidden header targets");
            }
        }
    }
}

#[test]
fn dispatcher_editor_draw_has_no_hidden_header_pointer_targets() {
    let _guard = plx_base::testlock::serial();
    for percent in [20, 100, 200] {
        let (mut d, mut rig) = boot(false);
        rig.measure.set_font_scale(FontScale::from_percent(percent).unwrap());
        let page = d.nav.top_page().unwrap();
        let instance = page.inst.as_ref().unwrap().id;
        let screen = page.inst.as_ref().unwrap().screen.as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
        let header_keys: Vec<_> = screen.header(&rig.measure).unwrap().stops.iter().map(|(elem, _)| *elem).collect();
        d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
            Delivery::Screen(ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor { text_policy: OttTextPolicy::Literal,
                kind: OttEditorKind::Setting, session: 92, key: "profile.source".into(), title: "Source".into(),
                value: String::new(), secret: false, limit: 256,
            }))))));
        frame(&mut d, &mut rig, 256, vec![]);
        draw_pointer_map(&mut d, &rig, &[]);
        assert!(d.input.hit.front().iter().all(|stop| !header_keys.contains(&stop.key.elem)),
            "{percent}%: the editor must not retain hidden header targets");
        assert!(!d.input.hit.front().is_empty(), "editor controls still receive pointer stops");
    }
}

#[test]
fn dispatcher_provider_profile_rows_match_pointer_and_keyboard_activation() {
    let _guard = plx_base::testlock::serial();
    for pointer in [false, true] {
        for (page, id, action) in [
            ("providers", "m3u", OttAction::SelectProvider("m3u".into())),
            ("profiles:m3u", "home", OttAction::SelectProfile("home".into())),
        ] {
            let (mut d, mut rig) = mount_snapshot(navigation_view(page, 1,
                vec![navigation_row(id, action.clone())], Some(id)));
            if pointer {
                let item = format!("item:{id}");
                let rect = draw_pointer_map_with_activation(&mut d, &rig, &[&item],
                    plx_ui::screen::Activate::Direct)[0];
                click_header(&mut d, &mut rig, rect, 256);
            } else {
                press_navigation_ok(&mut d, &mut rig, 256, false);
            }
            assert_eq!(rig.actions, vec![action], "{page}: one activation, pointer={pointer}");
        }
    }
}

#[test]
fn dispatcher_category_back_footer_and_back_key_emit_the_same_action() {
    let _guard = plx_base::testlock::serial();
    for pointer in [false, true] {
        let (mut d, mut rig) = mount_snapshot(navigation_view("categories", 1,
            vec![navigation_row("all", OttAction::SelectCategory(String::new())),
                navigation_row("favorites", OttAction::ShowFavorites)], Some("all")));
        if pointer {
            let rect = draw_pointer_map_with_activation(&mut d, &rig, &["nav:back"],
                plx_ui::screen::Activate::Direct)[0];
            click_header(&mut d, &mut rig, rect, 256);
        } else {
            frame(&mut d, &mut rig, 256, vec![key(Key::Back, tick(256))]);
        }
        assert_eq!(rig.actions, vec![OttAction::Back], "one Back, pointer={pointer}");
    }
}

fn remote_keyboard_editor() -> (Dispatcher<TestHost>, TestRig, InstanceId) {
    let (mut d, mut rig) = boot(false);
    let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
    d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
        Delivery::Screen(ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor { text_policy: OttTextPolicy::Literal,
            kind: OttEditorKind::Setting, session: 901, key: "remote.address".into(),
            title: "Remote control server".into(), value: String::new(), secret: false, limit: 2048,
        }))))));
    frame(&mut d, &mut rig, 256, vec![]);
    assert_focus(&d, "edit:value");
    rig.actions.clear();
    // Enter the actual Value control; do not set editing or keyboard state by hand.
    press_navigation_ok(&mut d, &mut rig, 272, false);
    assert!(d.input.keyboard);
    assert_eq!(d.input.keyboard_owner, Some(instance));
    assert_focus(&d, "edit:value");
    (d, rig, instance)
}

fn remote_keyboard_draft(d: &Dispatcher<TestHost>) -> (bool, bool, String) {
    let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
        .as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
    let editor = screen.editor.as_ref().expect("addressed editor remains mounted");
    (screen.editing, editor.pending, editor.draft.text().to_owned())
}

fn type_remote_keyboard_address(d: &mut Dispatcher<TestHost>, rig: &mut TestRig) -> &'static str {
    use plx_machine::machine::{InputKind, Source, TextEdit};
    let address = "https://control.invalid/01234567890";
    assert_eq!(address.len(), 35);
    // These are normalized SDL-origin character commits. Multi-character IME
    // prediction and raw SDL decoding have separate regressions.
    let inputs = address.chars().map(|ch| InputEvent {
        at: tick(384), source: Source::Sdl,
        kind: InputKind::Text(TextEdit::Commit(ch.to_string().into())),
    }).collect();
    frame(d, rig, 384, inputs);
    assert_eq!(remote_keyboard_draft(d), (true, false, address.into()));
    address
}

#[test]
fn remote_editor_keyboard_enter_or_down_preserves_draft_until_explicit_save_and_ack() {
    let _guard = plx_base::testlock::serial();
    for dismiss in [Key::Ok, Key::Down] {
        let (mut d, mut rig, instance) = remote_keyboard_editor();
        let address = type_remote_keyboard_address(&mut d, &mut rig);
        frame(&mut d, &mut rig, 400,
            vec![key(dismiss, tick(400)), key_up(dismiss, tick(400))]);
        frame(&mut d, &mut rig, 416, vec![]);
        assert!(!d.input.keyboard, "{dismiss:?} dismisses the keyboard only");
        assert_eq!(d.input.keyboard_owner, None);
        assert_focus(&d, "edit:value");
        assert_eq!(remote_keyboard_draft(&d), (false, false, address.into()));
        assert!(rig.actions.is_empty(), "keyboard dismissal must not save, cancel or navigate");

        frame(&mut d, &mut rig, 496, vec![key(Key::Down, tick(496))]);
        assert_focus(&d, "edit:save");
        press_navigation_ok(&mut d, &mut rig, 512, false);
        assert_eq!(rig.actions, vec![OttAction::SubmitSetting {
            session: 901, key: "remote.address".into(), value: address.into(),
        }]);
        assert_eq!(remote_keyboard_draft(&d), (false, true, address.into()));

        d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
            Delivery::Screen(ScreenEvent::App(AppMsg::OttEditorResult { session: 900, error: None }))));
        frame(&mut d, &mut rig, 624, vec![]);
        assert_eq!(remote_keyboard_draft(&d), (false, true, address.into()),
            "an older result cannot discard the pending draft");
        d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
            Delivery::Screen(ScreenEvent::App(AppMsg::OttEditorResult { session: 901, error: None }))));
        frame(&mut d, &mut rig, 640, vec![]);
        let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
            .as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
        assert!(!screen.handles_private_text(), "only its matching success closes the editor");
        assert_focus(&d, "item:video-one");
        assert_eq!(rig.actions.len(), 1, "release and acknowledgement cannot submit again");
    }
}

#[test]
fn remote_editor_keyboard_back_dismisses_before_a_second_back_cancels() {
    let _guard = plx_base::testlock::serial();
    let (mut d, mut rig, _) = remote_keyboard_editor();
    let address = type_remote_keyboard_address(&mut d, &mut rig);
    frame(&mut d, &mut rig, 400,
        vec![key(Key::Back, tick(400)), key_up(Key::Back, tick(400))]);
    frame(&mut d, &mut rig, 416, vec![]);
    assert!(!d.input.keyboard);
    assert_eq!(d.input.keyboard_owner, None);
    assert_focus(&d, "edit:value");
    assert_eq!(remote_keyboard_draft(&d), (false, false, address.into()));
    assert!(rig.actions.is_empty(), "first Back belongs only to the keyboard");

    frame(&mut d, &mut rig, 432,
        vec![key(Key::Back, tick(432)), key_up(Key::Back, tick(432))]);
    let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
        .as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
    assert!(!screen.handles_private_text());
    assert_focus(&d, "item:video-one");
    assert_eq!(rig.actions, vec![OttAction::CancelSetting { session: 901 }],
        "second Back cancels once without Save or outer-page navigation");
}

mod literal_fields {
    include!("literal_fields_tests.rs");
}

fn browse_focus_view(revision: u64, ids: &[&str], selected: &str) -> Arc<OttView> {
    let rows = ids.iter().map(|id| navigation_row(id, OttAction::Play((*id).into()))).collect();
    let mut view = (*navigation_view("channels", revision, rows, Some(selected))).clone();
    view.preview_context = Some("focus-source".into());
    view.catalog_progress = Some(OttCatalogProgress { loaded: ids.len(), total: None, loading: true, complete: false });
    Arc::new(view)
}

#[test]
fn directional_focus_is_immediate_intent_but_initial_dwell_and_repaint_are_not() {
    let _guard = plx_base::testlock::serial();
    let (mut d, mut rig) = mount_snapshot(browse_focus_view(1, &["first", "second"], "first"));
    for ms in (208..=448).step_by(16) { frame(&mut d, &mut rig, ms, vec![]); }
    assert!(rig.actions.iter().all(|action| !format!("{action:?}").starts_with("FocusAt(")));
    frame(&mut d, &mut rig, 464, vec![key(Key::Down, tick(464))]);
    assert_focus(&d, "item:second");
    assert_eq!(rig.actions.iter().filter(|action| format!("{action:?}") == "FocusAt(\"second\")").count(), 1);
    assert!(!rig.actions.iter().any(|action| matches!(action, OttAction::Play(_))));
    let before = rig.actions.iter().filter(|action| format!("{action:?}").starts_with("FocusAt(")).count();
    publish_snapshot(&mut d, &mut rig, browse_focus_view(2, &["first", "second", "third"], "second"), 512);
    frame(&mut d, &mut rig, 768, vec![]);
    assert_eq!(rig.actions.iter().filter(|action| format!("{action:?}").starts_with("FocusAt(")).count(), before);
}

#[test]
fn pending_private_focus_waits_for_its_row_and_newer_directional_intent_cancels_it() {
    let _guard = plx_base::testlock::serial();
    for user_moves in [false, true] {
        let (mut d, mut rig) = mount_snapshot(browse_focus_view(1, &["first", "other"], "last"));
        assert_focus(&d, "item:first");
        if user_moves {
            frame(&mut d, &mut rig, 256, vec![key(Key::Down, tick(256))]);
            assert_focus(&d, "item:other");
        }
        publish_snapshot(&mut d, &mut rig,
            browse_focus_view(2, &["first", "other", "last"], "last"), 320);
        assert_focus(&d, if user_moves { "item:other" } else { "item:last" });
        assert!(!rig.actions.iter().any(|action| matches!(action, OttAction::Play(_))));
    }
}

#[test]
fn changed_context_on_same_page_retires_old_pending_focus_and_honors_new_selection() {
    let _guard = plx_base::testlock::serial();
    // A same-page source generation change must retire its pending memory.
    // Ordinary profile switches also change the runtime page key.
    for present in [false, true] {
        let (mut d, mut rig) = mount_snapshot(browse_focus_view(1, &["common"], "a-late"));
        let mut b = (*browse_focus_view(2, if present { &["common", "a-late", "b-late"] }
            else { &["common", "a-late"] }, "b-late")).clone();
        b.preview_context = Some("different-source".into());
        publish_snapshot(&mut d, &mut rig, Arc::new(b), 256);
        if present { assert_focus(&d, "item:b-late"); }
        let mut finished = (*browse_focus_view(3, &["common", "a-late", "b-late"], "b-late")).clone();
        finished.preview_context = Some("different-source".into());
        publish_snapshot(&mut d, &mut rig, Arc::new(finished), 320);
        assert_focus(&d, "item:b-late");
        assert!(!rig.actions.iter().any(|action| matches!(action, OttAction::Play(_))));
    }
}

#[test]
fn pointer_hover_records_intent_without_activating_video() {
    let _guard = plx_base::testlock::serial();
    let (mut d, mut rig) = mount_snapshot(browse_focus_view(1, &["first", "second"], "first"));
    let rect = draw_pointer_map_with_activation(&mut d, &rig, &["item:second"],
        plx_ui::screen::Activate::Direct)[0];
    frame(&mut d, &mut rig, 256, vec![InputEvent { at: tick(256),
        source: plx_machine::machine::Source::Script,
        kind: InputKind::Pointer { x: rect.cx(), y: rect.cy(), hit: None } }]);
    assert_focus(&d, "item:second");
    assert!(rig.actions.iter().any(|action| format!("{action:?}") == "FocusAt(\"second\")"));
    assert!(!rig.actions.iter().any(|action| matches!(action, OttAction::Play(_))));
}

#[test]
fn modern_channel_footer_has_real_pointer_stop_and_dispatches_category_action() {
    let _guard = plx_base::testlock::serial();
    let snapshot = navigation_view("channels:all", 7,
        vec![navigation_row("channel-one", OttAction::Play("channel-one".into()))], Some("channel-one"));
    let (mut d, mut rig) = mount_snapshot(snapshot);
    let rect = draw_pointer_map(&mut d, &rig, &["nav:categories"])[0];
    assert!(rect.y > plx_ui::page_header::GRID_TOP, "the contextual actions sit at the bottom of Modern channel list");
    click_header(&mut d, &mut rig, rect, 256);
    assert_eq!(rig.actions, vec![OttAction::ShowCategories], "Magic Remote click follows the normal SDL pointer hit path");
}

#[test]
fn modern_footer_reuses_no_focus_keys_from_the_top_header() {
    let _guard = plx_base::testlock::serial();
    for page_id in ["channels:all", "categories:all"] {
        let mut view = (*navigation_view(page_id, 7,
            vec![navigation_row("channel-one", OttAction::Play("channel-one".into()))], Some("channel-one"))).clone();
        view.rows[0].description = "Programme description".into();
        view.search = Some(OttSearch { query: String::new(), scope: String::new() });
        view.order_context = Some("order-context".into());
        let (d, _) = mount_snapshot(Arc::new(view));
        let page = d.nav.top_page().unwrap();
        let screen = page.inst.as_ref().unwrap().screen.as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
        let header: Vec<_> = screen.header(&FixtureMeasure).unwrap().stops.iter().map(|(key, _)| *key).collect();
        let footer: Vec<_> = screen.modern_footer(&FixtureMeasure).unwrap().stops.iter().map(|(key, _)| *key).collect();
        assert!(!footer.is_empty());
        assert!(footer.iter().all(|key| !header.contains(key)), "{page_id}: a footer key is already owned by the top header");
    }
}

#[test]
fn channel_list_entry_center_release_and_repeat_do_not_advance_selected_channel() {
    let _guard = plx_base::testlock::serial();
    let rows = (1..=3).map(|n| navigation_row(&format!("channel-{n}"), OttAction::Play(format!("channel-{n}"))))
        .collect();
    let snapshot = navigation_view("channels:all", 8, rows, Some("channel-2"));
    let (mut d, mut rig) = mount_snapshot(snapshot);
    assert_focus(&d, "item:channel-2");

    // A center press opens this page from fullscreen. Its release may be delivered
    // after the page transition; it must not become a list-navigation event.
    frame(&mut d, &mut rig, 256, vec![key_up(Key::Ok, tick(256))]);
    assert_focus(&d, "item:channel-2");
    assert!(rig.actions.iter().all(|action| !matches!(action, OttAction::FocusAt { id, .. } if id == "channel-3")));

    // Some remotes repeat the held center key across the transition. The repeat
    // may activate the current item, but must never move focus to its neighbour.
    frame(&mut d, &mut rig, 272, vec![key(Key::Ok, tick(272))]);
    frame(&mut d, &mut rig, 336, vec![key_up(Key::Ok, tick(336))]);
    assert_focus(&d, "item:channel-2");
    assert!(rig.actions.iter().all(|action| !matches!(action, OttAction::FocusAt { id, .. } if id == "channel-3")));
}

#[test]
fn focused_channel_selection_survives_play_and_return_without_focus_or_scroll_jump() {
    let _guard = plx_base::testlock::serial();
    let rows = (0..24).map(|n| navigation_row(&format!("channel-{n}"), OttAction::Play(format!("channel-{n}"))))
        .collect();
    let snapshot = navigation_view("channels:all", 20, rows, Some("channel-0"));
    let (mut d, mut rig) = mount_snapshot(snapshot);
    for step in 0..14u32 {
        let at = 256 + step * 96;
        frame(&mut d, &mut rig, at, vec![key(Key::Down, tick(at)), key_up(Key::Down, tick(at + 64))]);
    }
    assert_focus(&d, "item:channel-14");
    // Let the original selection's scroll spring settle before saving its view.
    for ms in (1600..=3200).step_by(16) { frame(&mut d, &mut rig, ms, vec![]); }
    let (before_scroll, memory, instance) = {
        let page = d.nav.top_page().unwrap();
        let screen = page.inst.as_ref().unwrap().screen.as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
        (screen.form.table.scroll_pos(), Screen::<TestHost>::memory_at(screen, d.focus()), page.inst.as_ref().unwrap().id)
    };
    assert!(before_scroll > 0.0, "the selected item has scrolled into view");

    frame(&mut d, &mut rig, 3300, vec![key(Key::Ok, tick(3300))]);
    frame(&mut d, &mut rig, 3364, vec![key_up(Key::Ok, tick(3364))]);
    for ms in (3380..=3900).step_by(16) { frame(&mut d, &mut rig, ms, vec![]); }
    assert_eq!(rig.actions.iter().filter(|action| matches!(action, OttAction::Play(id) if id == "channel-14")).count(), 1);
    d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance), Delivery::Screen(ScreenEvent::RestoreMemory(memory))));
    let returned = navigation_view("channels:all", 21,
        (0..24).map(|n| navigation_row(&format!("channel-{n}"), OttAction::Play(format!("channel-{n}")))).collect(),
        Some("channel-14"));
    publish_snapshot(&mut d, &mut rig, returned, 3920);
    assert_focus(&d, "item:channel-14");
    let page = d.nav.top_page().unwrap();
    let screen = page.inst.as_ref().unwrap().screen.as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
    assert!((screen.form.table.scroll_pos() - before_scroll).abs() < 0.01,
        "return preserves the selected row's scroll position: before={before_scroll} after={}", screen.form.table.scroll_pos());
}

#[test]
fn restored_or_reconciled_focus_is_not_new_persistent_user_intent() {
    let _guard = plx_base::testlock::serial();
    for by in [plx_ui::screen::By::Restore, plx_ui::screen::By::Reconcile] {
        let (mut d, mut rig) = mount_snapshot(browse_focus_view(1, &["first", "second"], "first"));
        let page = d.nav.top_page().unwrap();
        let instance = page.inst.as_ref().unwrap().id;
        let screen = page.inst.as_ref().unwrap().screen.as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
        let to = FocusKey { entry: page.id, elem: screen.keys["item:second"] };
        // Exercise the screen's real provenance boundary without claiming a
        // pointer/keyboard event occurred during restore or reconciliation.
        d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance), Delivery::Screen(
            ScreenEvent::FocusMoved { from: None, to, by })));
        frame(&mut d, &mut rig, 256, vec![]);
        assert!(rig.actions.iter().all(|action| !format!("{action:?}").starts_with("FocusAt(")));
    }
}

fn quality_view(context: &str, revision: u64, variants: &[&str], selected: &str) -> Arc<OttView> {
    Arc::new(OttView { shelf_context: None, shelves: Vec::new(), status_working: false, pending_play: None, pending_step: None, pending_step_sdl: false,
        classic_preferences: Default::default(), volume_available: false,
        page_id: format!("vportal-quality:{context}"), revision,
        title: msg::settings_playback_quality().into(),
        description: String::new(), status: String::new(),
        // Synthetic context-scoped opaque IDs stand in for the producer's hashes.
        // The action still carries the untouched variant token separately.
        selected_row: Some(format!("{context}/{selected}")),
        rows: variants.iter().map(|variant| navigation_row(&format!("{context}/{variant}"),
            OttAction::SelectVPortalQuality { context: context.into(), variant: (*variant).into() })).collect(),
        category_label: None, order_context: None, preview_context: None,
        random_context: None, catalog_progress: None, search: None,
    })
}

#[test]
fn vportal_quality_action_debug_redacts_both_opaque_values() {
    let action = OttAction::SelectVPortalQuality {
        context: "private-context-fixture".into(), variant: "private-variant-fixture".into(),
    };
    assert_eq!(format!("{action:?}"), "SelectVPortalQuality(<context>, <variant>)");
}

#[test]
fn dispatcher_vportal_quality_selects_current_row_and_activates_once_in_both_modes() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for pointer in [false, true] {
            let (mut d, mut rig) = mount_snapshot_in_mode(
                quality_view("context-one", 1, &["auto", "medium", "high"], "medium"), mode);
            assert_focus(&d, "item:context-one/medium");
            let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
                .as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
            assert_eq!(screen.classic_mode, mode == UiMode::Classic);
            assert!(!screen.grid_active(), "quality choices use the existing table");
            assert!(screen.form.index_of(&"nav:refresh".to_owned()).is_none());
            assert!(screen.form.index_of(&"nav:back".to_owned()).is_some());
            assert!(rig.snapshot.rows.iter().all(|row| row.media.is_none() && row.poster.is_none()));
            assert!(rig.actions.is_empty(), "merely opening the selector must not choose a quality");
            if pointer {
                let rect = draw_pointer_map_with_activation(&mut d, &rig, &["item:context-one/high"],
                    plx_ui::screen::Activate::Direct)[0];
                click_header(&mut d, &mut rig, rect, 256);
            } else {
                frame(&mut d, &mut rig, 240, vec![key(Key::Down, tick(240))]);
                assert_focus(&d, "item:context-one/high");
                press_navigation_ok(&mut d, &mut rig, 256, false);
            }
            assert_eq!(rig.actions, vec![OttAction::SelectVPortalQuality {
                context: "context-one".into(), variant: "high".into(),
            }], "one explicit choice, mode={mode:?}, pointer={pointer}");
        }
    }
}

#[test]
fn dispatcher_vportal_quality_back_never_selects_a_variant_in_both_modes() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for pointer in [false, true] {
            let (mut d, mut rig) = mount_snapshot_in_mode(
                quality_view("context-one", 1, &["auto", "high"], "high"), mode);
            if pointer {
                let rect = draw_pointer_map_with_activation(&mut d, &rig, &["nav:back"],
                    plx_ui::screen::Activate::Direct)[0];
                click_header(&mut d, &mut rig, rect, 256);
            } else {
                frame(&mut d, &mut rig, 256, vec![key(Key::Back, tick(256))]);
            }
            assert_eq!(rig.actions, vec![OttAction::Back], "mode={mode:?}, pointer={pointer}");
        }
    }
}

#[test]
fn dispatcher_vportal_quality_reordering_keeps_the_focused_variant_not_its_old_index() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let (mut d, mut rig) = mount_snapshot_in_mode(
            quality_view("context-one", 1, &["auto", "medium", "high"], "medium"), mode);
        let focused = d.focus();
        publish_snapshot(&mut d, &mut rig,
            quality_view("context-one", 2, &["high", "auto", "medium"], "high"), 240);
        assert_focus(&d, "item:context-one/medium");
        assert_eq!(d.focus(), focused, "focus belongs to the opaque row ID, not position");
        assert!(rig.actions.is_empty());
        press_navigation_ok(&mut d, &mut rig, 256, false);
        assert_eq!(rig.actions, vec![OttAction::SelectVPortalQuality {
            context: "context-one".into(), variant: "medium".into(),
        }]);
    }
}

#[test]
fn dispatcher_vportal_quality_retired_disabled_or_replaced_context_cannot_retarget() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for (removed, replaced_context) in [(false, false), (true, false), (true, true)] {
            let (mut d, mut rig) = mount_snapshot_in_mode(
                quality_view("context-one", 1, &["auto", "medium"], "medium"), mode);
            // Capture the actual rendered target. This case tests stale typed activation;
            // keyboard and coordinate-based pointer dispatch are exercised separately above.
            draw_pointer_map_with_activation(&mut d, &rig, &["item:context-one/medium"],
                plx_ui::screen::Activate::Direct);
            let old_key = d.focus().unwrap().elem;
            let mut changed = (*quality_view(if replaced_context { "context-two" } else { "context-one" }, 2,
                if removed && !replaced_context { &["auto", "high"] } else { &["auto", "medium"] }, "auto")).clone();
            if !removed { changed.rows[1].enabled = false; }
            publish_snapshot(&mut d, &mut rig, Arc::new(changed), 240);
            assert!(rig.actions.is_empty());
            let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
            d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
                Delivery::Screen(ScreenEvent::Activate(old_key))));
            frame(&mut d, &mut rig, 256, vec![]);
            assert!(rig.actions.is_empty(), "stale target must not choose its neighbour, removed={removed}, replaced_context={replaced_context}");
        }
    }
}

#[test]
fn dispatcher_vportal_quality_static_selector_does_not_hide_refresh_on_other_pages() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let (d, _) = mount_snapshot_in_mode(navigation_view("profiles:vportal", 1,
            vec![navigation_row("profile", OttAction::SelectProfile("profile".into()))],
            Some("profile")), mode);
        let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
            .as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
        assert!(screen.form.index_of(&"nav:refresh".to_owned()).is_some());
    }
}

fn m3u_media_links_view(context: &str, revision: u64, profiles: &[&str], selected: &str) -> Arc<OttView> {
    let mut rows: Vec<_> = profiles.iter().map(|profile| {
        // The owner supplies opaque context-scoped row IDs; these synthetic IDs
        // deliberately retain the profile separately in the typed action.
        let mut row = navigation_row(&format!("{context}/{profile}"), OttAction::EditM3uMediaLink {
            profile: (*profile).into(), context: context.into(),
        });
        row.title = (*profile).into();
        row.detail = "Set".into();
        row
    }).collect();
    rows.push(navigation_row("m3u-media.back", OttAction::ShowProfiles));
    navigation_view("m3u-media-links", revision, rows, Some(&format!("{context}/{selected}")))
}

#[test]
fn m3u_media_link_action_debug_redacts_profile_and_context() {
    let action = OttAction::EditM3uMediaLink {
        profile: "private-profile-fixture".into(), context: "private-context-fixture".into(),
    };
    assert_eq!(format!("{action:?}"), "EditM3uMediaLink(<private>)");
    assert_eq!(format!("{:?}", OttAction::ShowM3uMediaLinks), "ShowM3uMediaLinks");
}

#[test]
fn dispatcher_m3u_media_links_opens_and_targets_without_selecting_a_profile_in_both_modes() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for pointer in [false, true] {
            let (mut d, mut rig) = mount_snapshot_in_mode(navigation_view("profiles:m3u", 1,
                vec![navigation_row("profile-a", OttAction::SelectProfile("profile-a".into())),
                    navigation_row("m3u-media.open", OttAction::ShowM3uMediaLinks)],
                Some("m3u-media.open")), mode);
            rig.transition = Some((OttAction::ShowM3uMediaLinks,
                m3u_media_links_view("ctx-1", 2, &["profile-b", "profile-c"], "profile-b")));
            press_navigation_ok(&mut d, &mut rig, 256, false);
            assert_eq!(rig.actions, vec![OttAction::ShowM3uMediaLinks]);
            assert_focus(&d, "item:ctx-1/profile-b");
            let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
                .as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
            assert_eq!(screen.classic_mode, mode == UiMode::Classic);
            assert!(!screen.grid_active());
            assert!(screen.form.index_of(&"nav:refresh".into()).is_none());
            assert!(screen.form.index_of(&"nav:back".into()).is_none());
            assert!(screen.form.index_of(&"item:m3u-media.back".into()).is_some());
            assert_eq!(rig.snapshot.rows[0].title, "profile-b");
            assert!(rig.snapshot.rows.iter().all(|row| row.media.is_none() && row.poster.is_none()));
            if pointer {
                let rect = draw_pointer_map_with_activation(&mut d, &rig, &["item:ctx-1/profile-c"],
                    plx_ui::screen::Activate::Direct)[0];
                click_header(&mut d, &mut rig, rect, 400);
            } else {
                frame(&mut d, &mut rig, 384, vec![key(Key::Down, tick(384))]);
                assert_focus(&d, "item:ctx-1/profile-c");
                press_navigation_ok(&mut d, &mut rig, 400, false);
            }
            assert_eq!(rig.actions, vec![OttAction::ShowM3uMediaLinks,
                OttAction::EditM3uMediaLink { profile: "profile-c".into(), context: "ctx-1".into() }],
                "one addressed edit, not SelectProfile; mode={mode:?}, pointer={pointer}");
        }
    }
}

#[test]
fn dispatcher_m3u_media_links_owner_back_and_back_key_do_not_edit_in_both_modes() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for pointer in [false, true] {
            let (mut d, mut rig) = mount_snapshot_in_mode(
                m3u_media_links_view("ctx-1", 1, &["profile-b"], "profile-b"), mode);
            if pointer {
                let rect = draw_pointer_map_with_activation(&mut d, &rig, &["item:m3u-media.back"],
                    plx_ui::screen::Activate::Direct)[0];
                click_header(&mut d, &mut rig, rect, 256);
            } else {
                frame(&mut d, &mut rig, 240, vec![key(Key::Down, tick(240))]);
                assert_focus(&d, "item:m3u-media.back");
                press_navigation_ok(&mut d, &mut rig, 256, false);
            }
            assert_eq!(rig.actions, vec![OttAction::ShowProfiles]);
        }
        let (mut d, mut rig) = mount_snapshot_in_mode(
            m3u_media_links_view("ctx-1", 1, &["profile-b"], "profile-b"), mode);
        frame(&mut d, &mut rig, 256, vec![key(Key::Back, tick(256))]);
        // Hardware Back remains the existing owner-interpreted action.
        assert_eq!(rig.actions, vec![OttAction::Back]);
    }
}

#[test]
fn dispatcher_m3u_media_links_reorder_keeps_focused_profile_identity() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let (mut d, mut rig) = mount_snapshot_in_mode(
            m3u_media_links_view("ctx-1", 1, &["profile-b", "profile-c"], "profile-b"), mode);
        let focused = d.focus();
        publish_snapshot(&mut d, &mut rig,
            m3u_media_links_view("ctx-1", 2, &["profile-c", "profile-b"], "profile-c"), 240);
        assert_focus(&d, "item:ctx-1/profile-b");
        assert_eq!(d.focus(), focused);
        assert!(rig.actions.is_empty());
        press_navigation_ok(&mut d, &mut rig, 256, false);
        assert_eq!(rig.actions, vec![OttAction::EditM3uMediaLink {
            profile: "profile-b".into(), context: "ctx-1".into(),
        }]);
    }
}

#[test]
fn dispatcher_m3u_media_links_disabled_removed_or_replaced_context_cannot_retarget() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for (removed, replaced_context) in [(false, false), (true, false), (true, true)] {
            let (mut d, mut rig) = mount_snapshot_in_mode(
                m3u_media_links_view("ctx-1", 1, &["profile-b", "profile-c"], "profile-b"), mode);
            draw_pointer_map_with_activation(&mut d, &rig, &["item:ctx-1/profile-b"],
                plx_ui::screen::Activate::Direct);
            let old_key = d.focus().unwrap().elem;
            let mut changed = (*m3u_media_links_view(if replaced_context { "ctx-2" } else { "ctx-1" }, 2,
                if removed && !replaced_context { &["profile-c"] } else { &["profile-b", "profile-c"] }, "profile-c")).clone();
            if !removed { changed.rows[0].enabled = false; }
            publish_snapshot(&mut d, &mut rig, Arc::new(changed), 240);
            assert!(rig.actions.is_empty());
            // Isolate stale typed-event handling. Real key/click paths are above;
            // this does not claim to reproduce a late native pointer event.
            let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
            d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
                Delivery::Screen(ScreenEvent::Activate(old_key))));
            frame(&mut d, &mut rig, 256, vec![]);
            assert!(rig.actions.is_empty(), "removed={removed}, replaced_context={replaced_context}");
        }
    }
}

#[test]
fn dispatcher_m3u_media_links_static_and_owner_back_guards_are_exact() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for page in ["profiles:m3u", "m3u-media-links-other"] {
            let (d, _) = mount_snapshot_in_mode(navigation_view(page, 1,
                vec![navigation_row("m3u-media.back", OttAction::ShowProfiles)], Some("m3u-media.back")), mode);
            let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
                .as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
            assert!(screen.form.index_of(&"nav:refresh".into()).is_some());
            assert!(screen.form.index_of(&"nav:back".into()).is_some());
        }
        let mut view = (*m3u_media_links_view("ctx-1", 1, &["profile-b"], "profile-b")).clone();
        view.rows.last_mut().unwrap().enabled = false;
        let (d, _) = mount_snapshot_in_mode(Arc::new(view), mode);
        let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
            .as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
        assert!(screen.form.index_of(&"nav:refresh".into()).is_none());
        assert!(screen.form.index_of(&"nav:back".into()).is_some(), "disabled owner Back cannot strand the page");
    }
}

#[test]
fn dispatcher_m3u_media_links_refresh_between_ok_edges_does_not_send_again() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for replace_context in [false, true] {
            let (mut d, mut rig) = mount_snapshot_in_mode(
                m3u_media_links_view("ctx-1", 1, &["profile-b", "profile-c"], "profile-b"), mode);
            frame(&mut d, &mut rig, 240, vec![key(Key::Ok, tick(240))]);
            let expected = vec![OttAction::EditM3uMediaLink {
                profile: "profile-b".into(), context: "ctx-1".into(),
            }];
            // Bare form rows activate on Down. A subsequent refresh cannot undo
            // that admitted action, but its release must not activate a new row.
            assert_eq!(rig.actions, expected);
            publish_snapshot(&mut d, &mut rig,
                m3u_media_links_view(if replace_context { "ctx-2" } else { "ctx-1" }, 2,
                    &["profile-c", "profile-b"], "profile-c"), 256);
            frame(&mut d, &mut rig, 304, vec![key_up(Key::Ok, tick(304))]);
            frame(&mut d, &mut rig, 320, vec![]);
            assert_eq!(rig.actions, expected, "mode={mode:?}, replace_context={replace_context}");
        }
    }
}


fn classic_arrow_view(mode: u8, volume: bool) -> Arc<OttView> {
    let rows = (0..40).map(|i| {
        let id = format!("channel-{i}");
        let mut row = navigation_row(&id, OttAction::Play(id.clone()));
        row.description = format!("Description {i}");
        row.guide_available = true;
        row.media = Some(OttMedia { kind: OttMediaKind::LiveChannel, duration_ms: None, progress: None, synopsis: None });
        row
    }).collect();
    let mut view = (*navigation_view("channels", 1, rows, Some("channel-0"))).clone();
    view.classic_preferences = plx_appkit::classic_settings::Preferences::from_values(25, 0, mode, 15).unwrap();
    view.volume_available = volume;
    Arc::new(view)
}

fn arrow_page_target(d: &Dispatcher<TestHost>, direction: i32) -> u32 {
    let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
        .as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
    screen.remote_page_key(d.focus().unwrap().elem, direction).unwrap()
}

#[test]
fn classic_list_arrow_paging_and_legacy_three_share_measured_focus_without_playing() {
    let _guard = plx_base::testlock::serial();
    for mode in [0, 3] {
        let (mut d, mut rig) = mount_snapshot_in_mode(classic_arrow_view(mode, true), UiMode::Classic);
        let first = d.focus().unwrap();
        let wanted = arrow_page_target(&d, 1);
        assert_ne!(wanted, first.elem);
        frame(&mut d, &mut rig, 256, vec![key(Key::Right, tick(256))]);
        assert_eq!(d.focus().unwrap().elem, wanted);
        frame(&mut d, &mut rig, 272, vec![key_up(Key::Right, tick(272))]);
        frame(&mut d, &mut rig, 304, vec![key(Key::Left, tick(304))]);
        assert_eq!(d.focus(), Some(first));
        assert!(!rig.actions.iter().any(|a| matches!(a, OttAction::Play(_) | OttAction::Back | OttAction::VolumeStep(_))));
    }
}

#[test]
fn classic_list_volume_arrows_require_capability_and_do_not_move_focus() {
    let _guard = plx_base::testlock::serial();
    for available in [false, true] {
        let (mut d, mut rig) = mount_snapshot_in_mode(classic_arrow_view(1, available), UiMode::Classic);
        let first = d.focus();
        let wanted = arrow_page_target(&d, 1);
        frame(&mut d, &mut rig, 256, vec![key(Key::Right, tick(256)), key_up(Key::Right, tick(256))]);
        let volume: Vec<_> = rig.actions.iter().filter(|a| matches!(a, OttAction::VolumeStep(_))).collect();
        if available {
            assert_eq!(volume, vec![&OttAction::VolumeStep(1)]);
            assert_eq!(d.focus(), first);
        } else {
            assert!(volume.is_empty());
            assert_eq!(d.focus().unwrap().elem, wanted);
        }
    }
}

#[test]
fn classic_list_context_arrows_use_selected_live_and_finite_actions() {
    let _guard = plx_base::testlock::serial();
    for finite in [false, true] {
        for direction in [Key::Left, Key::Right] {
            let mut view = (*classic_arrow_view(2, true)).clone();
            if finite { view.rows[0].media.as_mut().unwrap().kind = OttMediaKind::Movie; }
            let (mut d, mut rig) = mount_snapshot_in_mode(Arc::new(view), UiMode::Classic);
            frame(&mut d, &mut rig, 256, vec![key(direction, tick(256))]);
            let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
                .as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
            if finite && direction == Key::Right {
                assert!(screen.reading);
            } else {
                let expected = if direction == Key::Right { OttAction::OpenGuide("channel-0".into()) }
                    else if finite { OttAction::Back } else { OttAction::ShowCategories };
                assert!(rig.actions.contains(&expected));
            }
            assert!(!rig.actions.iter().any(|a| matches!(a, OttAction::Play(_) | OttAction::VolumeStep(_))));
        }
    }
}

#[test]
fn classic_context_categories_still_page_and_header_browse_keep_focus_navigation() {
    let _guard = plx_base::testlock::serial();
    let mut view = (*classic_arrow_view(2, true)).clone();
    view.page_id = "categories".into();
    for row in &mut view.rows { row.action = OttAction::SelectCategory(row.id.clone()); }
    let (mut d, mut rig) = mount_snapshot_in_mode(Arc::new(view), UiMode::Classic);
    let wanted = arrow_page_target(&d, 1);
    frame(&mut d, &mut rig, 256, vec![key(Key::Right, tick(256))]);
    assert_eq!(d.focus().unwrap().elem, wanted);
    assert!(!rig.actions.iter().any(|a| matches!(a, OttAction::Back | OttAction::SelectCategory(_))));
    let (mut d, mut rig) = mount_snapshot_in_mode(classic_arrow_view(1, true), UiMode::Classic);
    // Classic puts these controls below the list, unlike focus_header's Modern route.
    // Enter through the real drawn Browse stop, then exercise keyboard navigation.
    let browse = draw_pointer_map(&mut d, &rig, &["nav:browse"])[0];
    click_header(&mut d, &mut rig, browse, 256);
    assert_focus(&d, "nav:catalog");
    rig.actions.clear();
    frame(&mut d, &mut rig, 672, vec![key(Key::Left, tick(672)), key_up(Key::Left, tick(672))]);
    assert_focus(&d, "nav:catalog");
    assert!(d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
        .as_any().unwrap().downcast_ref::<OttScreen>().unwrap().browse);
    assert!(!rig.actions.iter().any(|a| matches!(a, OttAction::Back | OttAction::VolumeStep(_))));
    frame(&mut d, &mut rig, 704, vec![key(Key::Back, tick(704)), key_up(Key::Back, tick(704))]);
    assert_focus(&d, "item:channel-0");
    assert!(!rig.actions.iter().any(|a| matches!(a, OttAction::Back | OttAction::VolumeStep(_))));

    // Classic row arrows deliberately wrap. Hold OK is the keyboard-only path
    // into Browse; exercise the real press timer rather than calling activate.
    let (mut d, mut rig) = mount_snapshot_in_mode(classic_arrow_view(1, true), UiMode::Classic);
    frame(&mut d, &mut rig, 256, vec![key(Key::Down, tick(256))]);
    frame(&mut d, &mut rig, 272, vec![key_up(Key::Down, tick(272))]);
    assert_focus(&d, "item:channel-1");
    frame(&mut d, &mut rig, 304, vec![key(Key::Ok, tick(304))]);
    let release = 304 + plx_ui::press::LONG_MS + 64;
    for ms in (320..release).step_by(16) { frame(&mut d, &mut rig, ms, vec![]); }
    frame(&mut d, &mut rig, release, vec![key_up(Key::Ok, tick(release))]);
    for ms in (release + 16..=release + 320).step_by(16) { frame(&mut d, &mut rig, ms, vec![]); }
    assert_focus(&d, "nav:catalog");
    frame(&mut d, &mut rig, release + 336,
        vec![key(Key::Left, tick(release + 336)), key_up(Key::Left, tick(release + 336))]);
    assert_focus(&d, "nav:catalog");
    assert!(d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
        .as_any().unwrap().downcast_ref::<OttScreen>().unwrap().browse);
    frame(&mut d, &mut rig, release + 368,
        vec![key(Key::Back, tick(release + 368)), key_up(Key::Back, tick(release + 368))]);
    assert_focus(&d, "item:channel-1");
    assert!(!rig.actions.iter().any(|a| matches!(a,
        OttAction::Back | OttAction::Play(_) | OttAction::ToggleFavorite(_) | OttAction::VolumeStep(_))));
}

#[test]
fn modern_list_ignores_classic_arrow_policy_and_retains_description_edge() {
    let _guard = plx_base::testlock::serial();
    // Untyped rows use the vertical list, where the first Right is an edge.
    let mut view = (*classic_arrow_view(1, true)).clone();
    for row in &mut view.rows { row.media = None; }
    let (mut d, mut rig) = mount_snapshot_in_mode(Arc::new(view), UiMode::Modern);
    assert_focus(&d, "item:channel-0");
    frame(&mut d, &mut rig, 256, vec![key(Key::Right, tick(256))]);
    let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
        .as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
    assert!(screen.reading);
    assert!(!rig.actions.iter().any(|a| matches!(a, OttAction::VolumeStep(_))));

    // Typed live channels use a card grid: a real neighbour takes precedence over
    // the description edge, and Classic's volume policy still must not run.
    let (mut d, mut rig) = mount_snapshot_in_mode(classic_arrow_view(1, true), UiMode::Modern);
    assert_focus(&d, "item:channel-0");
    frame(&mut d, &mut rig, 256, vec![key(Key::Right, tick(256))]);
    assert_focus(&d, "item:channel-1");
    let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
        .as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
    assert!(screen.grid_active());
    assert!(!screen.reading);
    assert!(!rig.actions.iter().any(|a| matches!(a, OttAction::VolumeStep(_))));
}

#[test]
fn hidden_menu_items_keep_physical_shortcuts_and_current_action_guards() {
    use plx_appkit::menu_visibility::{self, Item, Visibility};
    use plx_ui::consts;
    let _guard = plx_base::testlock::serial();
    struct Reset(Visibility);
    impl Drop for Reset { fn drop(&mut self) { menu_visibility::project(self.0); } }
    let _reset = Reset(menu_visibility::projected());
    let mut hidden = Visibility::default();
    hidden.hide(Item::Categories);
    hidden.hide(Item::Description);
    menu_visibility::project(hidden);
    let raw = |code, edge, ms| InputEvent { at: tick(ms),
        source: plx_machine::machine::Source::RemoteFifo,
        kind: InputKind::Key { key: Key::Other, sym: 0, wcode: code, edge, at_edge: false } };
    let reading = |d: &Dispatcher<TestHost>| d.nav.top_page().unwrap().inst.as_ref().unwrap()
        .screen.as_any().unwrap().downcast_ref::<OttScreen>().unwrap().reading;

    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut view = (*classic_arrow_view(2, true)).clone();
        // This fixture tests shortcut dispatch, independently of deferred preview.
        // Keep exact action assertions instead of ignoring extra playback actions.
        view.preview_context = None;
        view.rows.truncate(1);
        view.rows[0].guide_available = false;
        let (mut d, mut rig) = mount_snapshot_in_mode(Arc::new(view.clone()), mode);
        // Inspect the real Browse form, not just the visibility setting.
        let browse = draw_pointer_map(&mut d, &rig, &["nav:browse"])[0];
        click_header(&mut d, &mut rig, browse, 256);
        let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
            .as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
        assert!(screen.browse);
        for id in ["nav:categories", "nav:description"] {
            assert!(screen.keys.get(id).is_none_or(|key|
                screen.form.index_of_key(RowKey(*key)).is_none()), "{mode:?}: {id} stays hidden");
        }
        let browse = draw_pointer_map(&mut d, &rig, &["nav:browse"])[0];
        click_header(&mut d, &mut rig, browse, 704);
        rig.actions.clear();
        frame(&mut d, &mut rig, 1120, vec![raw(consts::WCODE_BLUE, Edge::Down, 1120)]);
        frame(&mut d, &mut rig, 1136, vec![raw(consts::WCODE_BLUE, Edge::Repeat, 1136)]);
        frame(&mut d, &mut rig, 1152, vec![raw(consts::WCODE_BLUE, Edge::Up, 1152)]);
        assert_eq!(rig.actions, vec![OttAction::ShowCategories], "{mode:?}: one Blue press");
        if mode == UiMode::Classic {
            rig.actions.clear();
            frame(&mut d, &mut rig, 1168, vec![key(Key::Left, tick(1168))]);
            frame(&mut d, &mut rig, 1184, vec![key_up(Key::Left, tick(1184))]);
            assert_eq!(rig.actions, vec![OttAction::ShowCategories], "contextual Left stays available");
        }
        rig.actions.clear();
        frame(&mut d, &mut rig, 1200, vec![raw(consts::WCODE_RED, Edge::Down, 1200)]);
        assert!(reading(&d), "{mode:?}: Red falls back to the current description");
        frame(&mut d, &mut rig, 1216, vec![raw(consts::WCODE_RED, Edge::Up, 1216)]);
        frame(&mut d, &mut rig, 1232, vec![raw(consts::WCODE_BLUE, Edge::Down, 1232)]);
        assert!(rig.actions.is_empty(), "an open reader retains input ownership");

        for (enabled, description) in [(true, ""), (false, "Unavailable description")] {
            let mut unavailable = view.clone();
            unavailable.rows[0].enabled = enabled;
            unavailable.rows[0].description = description.into();
            let (mut d, mut rig) = mount_snapshot_in_mode(Arc::new(unavailable), mode);
            frame(&mut d, &mut rig, 256, vec![raw(consts::WCODE_RED, Edge::Down, 256)]);
            assert!(!reading(&d), "{mode:?}: hidden does not bypass enabled/description guards");
            assert!(rig.actions.is_empty());
        }
        // A guide-capable row must still choose Guide before Description.
        view.rows[0].guide_available = true;
        let (mut d, mut rig) = mount_snapshot_in_mode(Arc::new(view), mode);
        frame(&mut d, &mut rig, 256, vec![raw(consts::WCODE_RED, Edge::Down, 256)]);
        assert!(!reading(&d));
        assert_eq!(rig.actions, vec![OttAction::OpenGuide("channel-0".into())]);
    }
}


fn discovery_view(revision: u64, choice: &str, address: &str) -> Arc<OttView> {
    let id = format!("remote.server:0:{choice}");
    let mut row = navigation_row(&id, OttAction::EditSetting(format!("remote.choice:{choice}")));
    row.title = address.into();
    navigation_view("settings", revision, vec![row], Some(&id))
}

#[test]
fn discovery_choice_cannot_activate_before_its_first_presented_frame() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Modern, UiMode::Classic] {
        let (mut d, mut rig) = mount_snapshot_in_mode(discovery_view(1, "1:0", "https://one.invalid"), mode);
        assert_focus(&d, "item:remote.server:0:1:0");
        assert!(d.input.hit.front().is_empty());
        press_navigation_ok(&mut d, &mut rig, 256, true);
        assert!(rig.actions.is_empty(), "a seated but never presented server is not consent");
        draw_pointer_map_with_activation(&mut d, &rig, &["item:remote.server:0:1:0"], plx_ui::screen::Activate::Direct);
        press_navigation_ok(&mut d, &mut rig, 384, true);
        assert_eq!(rig.actions, vec![OttAction::EditSetting("remote.choice:1:0".into())]);
        rig.actions.clear();
        let mut health_update = (*discovery_view(2, "1:0", "https://one.invalid")).clone();
        health_update.status = "Remote control connected".into();
        publish_snapshot(&mut d, &mut rig, Arc::new(health_update), 496);
        press_navigation_ok(&mut d, &mut rig, 512, true);
        assert_eq!(rig.actions, vec![OttAction::EditSetting("remote.choice:1:0".into())],
            "a status-only revision keeps the already-presented choice available");
    }
}

#[test]
fn discovery_replacement_refuses_queued_and_reseated_keyboard_activation_until_draw() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Modern, UiMode::Classic] {
        let (mut d, mut rig) = mount_snapshot_in_mode(discovery_view(1, "1:0", "https://one.invalid"), mode);
        draw_pointer_map_with_activation(&mut d, &rig, &["item:remote.server:0:1:0"], plx_ui::screen::Activate::Direct);
        let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
        let old_key = d.focus().unwrap();
        let next = discovery_view(2, "2:0", "https://two.invalid");
        rig.snapshot = next.clone();
        // A carried delivery can follow an already-queued snapshot inside the same drain.
        d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
            Delivery::Screen(ScreenEvent::App(AppMsg::OttSnapshot(next)))));
        for input in [key(Key::Ok, tick(256)), key_up(Key::Ok, tick(256))] {
            d.emit(MachineId::Input, Fx::Deliver(MachineId::Instance(instance),
                Delivery::Screen(ScreenEvent::Input(input))));
        }
        frame(&mut d, &mut rig, 256, vec![]);
        assert!(rig.actions.is_empty(), "queued OK must not consume the replacement binding");
        assert_focus(&d, "item:remote.server:0:2:0");
        assert_ne!(d.focus(), Some(old_key));
        assert!(d.input.hit.front().iter().any(|stop| stop.key == old_key));
        // Reconcile has already seated the NEW row; key retirement alone cannot protect this OK.
        press_navigation_ok(&mut d, &mut rig, 288, false);
        assert!(rig.actions.is_empty(), "new fallback focus still has not been presented");
        draw_pointer_map_with_activation(&mut d, &rig, &["item:remote.server:0:2:0"], plx_ui::screen::Activate::Direct);
        press_navigation_ok(&mut d, &mut rig, 400, true);
        assert_eq!(rig.actions, vec![OttAction::EditSetting("remote.choice:2:0".into())]);
    }
}

#[test]
fn discovery_replacement_refuses_old_pointer_hit_and_accepts_new_drawn_row() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Modern, UiMode::Classic] {
        let (mut d, mut rig) = mount_snapshot_in_mode(discovery_view(1, "1:0", "https://one.invalid"), mode);
        let old_rect = draw_pointer_map_with_activation(&mut d, &rig,
            &["item:remote.server:0:1:0"], plx_ui::screen::Activate::Direct)[0];
        publish_snapshot(&mut d, &mut rig, discovery_view(2, "2:0", "https://two.invalid"), 256);
        click_header(&mut d, &mut rig, old_rect, 288);
        assert!(rig.actions.is_empty(), "the old painted coordinates cannot select the unseen replacement");
        let new_rect = draw_pointer_map_with_activation(&mut d, &rig,
            &["item:remote.server:0:2:0"], plx_ui::screen::Activate::Direct)[0];
        click_header(&mut d, &mut rig, new_rect, 720);
        assert_eq!(rig.actions, vec![OttAction::EditSetting("remote.choice:2:0".into())]);
    }
}


// Runtime supplies the ordered choices; these tests exercise their installation,
// focus and activation through the shipping screen dispatcher, without discovery IO.
fn inline_remote_choices_view(revision: u64, choices: bool) -> Arc<OttView> {
    let setting = |id: &str| navigation_row(id, OttAction::EditSetting(id.into()));
    let mut rows = vec![setting("remote.address"), setting("remote.find")];
    if choices {
        for (index, token) in ["choice-a", "choice-b"].into_iter().enumerate() {
            rows.push(navigation_row(&format!("remote.server:{index}:{token}"),
                OttAction::EditSetting(format!("remote.choice:{token}"))));
        }
    }
    rows.extend([setting("uiFontScale"), setting("interfaceMode")]);
    Arc::new(OttView {
        page_id: "settings".into(), revision, title: "Settings".into(), rows,
        // A retained fallback must not displace a surviving user-selected row.
        selected_row: Some("remote.address".into()),
        status: if choices { "Select a command server" } else { "" }.into(),
        ..OttView::default()
    })
}

#[test]
fn dispatcher_inline_server_choices_keep_find_focus_and_down_selects_first_choice() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let (mut d, mut rig) = mount_snapshot_in_mode(inline_remote_choices_view(1, false), mode);
        let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen.as_any().unwrap()
            .downcast_ref::<OttScreen>().unwrap();
        assert_eq!(screen.classic_mode, mode == UiMode::Classic);
        assert_focus(&d, "item:remote.address");
        frame(&mut d, &mut rig, 224, vec![key(Key::Down, tick(224))]);
        frame(&mut d, &mut rig, 240, vec![key_up(Key::Down, tick(240))]);
        assert_focus(&d, "item:remote.find");
        press_navigation_ok(&mut d, &mut rig, 256, false);
        assert_eq!(rig.actions, vec![OttAction::EditSetting("remote.find".into())]);
        let focused = d.focus();

        publish_snapshot(&mut d, &mut rig, inline_remote_choices_view(2, true), 400);
        assert_eq!(d.focus(), focused, "an async result must retain the Find row");
        assert_focus(&d, "item:remote.find");
        assert_eq!(rig.actions, vec![OttAction::EditSetting("remote.find".into())],
            "installing choices must not activate a server");
        frame(&mut d, &mut rig, 432, vec![key(Key::Down, tick(432))]);
        frame(&mut d, &mut rig, 448, vec![key_up(Key::Down, tick(448))]);
        assert_focus(&d, "item:remote.server:0:choice-a");
        draw_pointer_map_with_activation(&mut d, &rig,
            &["item:remote.server:0:choice-a"], plx_ui::screen::Activate::Direct);
        press_navigation_ok(&mut d, &mut rig, 480, false);
        assert_eq!(rig.actions, vec![OttAction::EditSetting("remote.find".into()),
            OttAction::EditSetting("remote.choice:choice-a".into())],
            "the first following row must dispatch its own opaque choice");
    }
}

#[test]
fn dispatcher_inline_server_choices_preserve_focus_moved_to_another_setting() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        let (mut d, mut rig) = mount_snapshot_in_mode(inline_remote_choices_view(1, false), mode);
        let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen.as_any().unwrap()
            .downcast_ref::<OttScreen>().unwrap();
        assert_eq!(screen.classic_mode, mode == UiMode::Classic);
        frame(&mut d, &mut rig, 224, vec![key(Key::Down, tick(224))]);
        frame(&mut d, &mut rig, 240, vec![key_up(Key::Down, tick(240))]);
        assert_focus(&d, "item:remote.find");
        press_navigation_ok(&mut d, &mut rig, 256, false);
        frame(&mut d, &mut rig, 352, vec![key(Key::Down, tick(352))]);
        frame(&mut d, &mut rig, 368, vec![key_up(Key::Down, tick(368))]);
        assert_focus(&d, "item:uiFontScale");
        let focused = d.focus();

        publish_snapshot(&mut d, &mut rig, inline_remote_choices_view(2, true), 400);
        assert_eq!(d.focus(), focused, "inserted choices must not steal or retarget focus");
        assert_focus(&d, "item:uiFontScale");
        assert_eq!(rig.actions, vec![OttAction::EditSetting("remote.find".into())],
            "the async result must not activate the new row at the old index");
        press_navigation_ok(&mut d, &mut rig, 448, false);
        assert_eq!(rig.actions, vec![OttAction::EditSetting("remote.find".into()),
            OttAction::EditSetting("uiFontScale".into())]);
    }
}


#[test]
fn dispatcher_left_at_list_boundary_never_becomes_parent_back_in_either_mode() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for (page, id, action) in [
            ("providers", "m3u", OttAction::SelectProvider("m3u".into())),
            ("profiles:m3u", "home", OttAction::SelectProfile("home".into())),
            ("settings", "interfaceMode", OttAction::EditSetting("interfaceMode".into())),
            ("channels", "one", OttAction::Play("one".into())),
            ("favorites", "one", OttAction::Play("one".into())),
            ("categories", "all", OttAction::SelectCategory("all".into())),
        ] {
            let (mut d, mut rig) = mount_snapshot_in_mode(
                navigation_view(page, 1, vec![navigation_row(id, action)], Some(id)), mode);
            assert_focus(&d, &format!("item:{id}"));
            let owner = d.nav.top_page().unwrap().id;
            let focus = d.focus();
            // Real dispatcher input reaches the row's horizontal boundary. No
            // hand-crafted at_edge event or direct screen activation is used.
            frame(&mut d, &mut rig, 256,
                vec![key(Key::Left, tick(256)), key_up(Key::Left, tick(256))]);
            assert!(!rig.actions.contains(&OttAction::Back), "{mode:?} {page}: Left escaped as Back");
            assert_eq!(d.nav.top_page().unwrap().id, owner);
            assert_eq!(d.focus(), focus, "{mode:?} {page}: boundary changed focus");
            // Keep the actual Back path live; the fix must not disable navigation.
            rig.actions.clear();
            frame(&mut d, &mut rig, 288,
                vec![key(Key::Back, tick(288)), key_up(Key::Back, tick(288))]);
            assert_eq!(rig.actions, vec![OttAction::Back], "{mode:?} {page}");
        }
    }
}


#[test]
fn dispatcher_blue_opens_advertised_categories_from_providers_once() {
    use plx_ui::consts;
    let _guard = plx_base::testlock::serial();
    let remote = |code, edge, ms| InputEvent { at: tick(ms),
        source: plx_machine::machine::Source::RemoteFifo,
        kind: InputKind::Key { key: Key::Other, sym: 0, wcode: code, edge, at_edge: false } };
    for mode in [UiMode::Classic, UiMode::Modern] {
        let (mut d, mut rig) = mount_snapshot_in_mode(navigation_view("providers", 1,
            vec![navigation_row("m3u", OttAction::SelectProvider("m3u".into()))], Some("m3u")), mode);
        if mode == UiMode::Classic {
            draw_pointer_map(&mut d, &rig, &["nav:categories"]);
        }
        frame(&mut d, &mut rig, 256, vec![remote(consts::WCODE_BLUE, Edge::Down, 256)]);
        frame(&mut d, &mut rig, 272, vec![remote(consts::WCODE_BLUE, Edge::Repeat, 272)]);
        frame(&mut d, &mut rig, 288, vec![remote(consts::WCODE_BLUE, Edge::Up, 288)]);
        assert_eq!(rig.actions, vec![OttAction::ShowCategories], "{mode:?}: physical Blue follows footer");
        rig.actions.clear();
        // A channel-page key is still limited to catalogue screens.
        let focus = d.focus();
        frame(&mut d, &mut rig, 320, vec![remote(consts::WCODE_CH_DOWN_KEY, Edge::Down, 320)]);
        assert_eq!(d.focus(), focus);
        assert!(rig.actions.is_empty());
        let browse = draw_pointer_map(&mut d, &rig, &["nav:browse"])[0];
        click_header(&mut d, &mut rig, browse, 352);
        frame(&mut d, &mut rig, 768, vec![remote(consts::WCODE_BLUE, Edge::Down, 768)]);
        assert!(rig.actions.is_empty(), "Browse retains input ownership");
    }
}


#[test]
fn classic_settings_replaced_draft_reenters_live_footer_through_focus_engine() {
    let _guard = plx_base::testlock::serial();
    let group = |nonce: u64, field: &str, title: &str| {
        let field = format!("settings.draft:{nonce}:pick:{field}");
        let save = format!("settings.draft:{nonce}:save");
        let back = format!("settings.draft:{nonce}:back");
        let rows = [(&field, title), (&save, "Save Settings"), (&back, "Cancel")]
            .into_iter().map(|(id, title)| {
                let mut row = navigation_row(id, OttAction::EditSetting((*id).clone()));
                row.title = title.into();
                row
            }).collect();
        navigation_view("settings", nonce, rows, Some(&field))
    };
    let (mut d, mut rig) = mount_snapshot_in_mode(group(42, "pageSize", "Page size"), UiMode::Classic);
    assert_focus(&d, "item:settings.draft:42:pick:pageSize");
    let entry = d.nav.top_page().unwrap().id;
    let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;

    // Actually leave Save through the shared engine, so the footer remembers it.
    focus_header(&mut d, &mut rig, "settings-footer:settings.draft:42:back", 256);
    frame(&mut d, &mut rig, 320, vec![key(Key::Right, tick(320))]);
    assert_focus(&d, "settings-footer:settings.draft:42:save");
    let old_save = d.focus().unwrap();
    frame(&mut d, &mut rig, 352, vec![key(Key::Down, tick(352))]);
    assert_focus(&d, "item:settings.draft:42:pick:pageSize");
    assert!(rig.actions.is_empty(), "focus travel must not activate the old draft");

    // The same mounted screen receives a different draft. Reconciliation moves
    // the body cursor, but must not be mistaken for fresh footer memory.
    publish_snapshot(&mut d, &mut rig, group(43, "language", "Language"), 400);
    assert_eq!(d.nav.top_page().unwrap().id, entry);
    assert_eq!(d.nav.top_page().unwrap().inst.as_ref().unwrap().id, instance);
    assert_focus(&d, "item:settings.draft:43:pick:language");
    assert!(d.input.engine.remembered_for(entry).contains(&(navigation::GROUP, old_save.elem)),
        "the regression needs the retired Save remembered in the live footer group");
    let (back, save) = {
        let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen.as_any()
            .unwrap().downcast_ref::<OttScreen>().unwrap();
        let header = screen.header(&rig.measure).unwrap();
        assert!(!header.stops.iter().any(|(key, _)| *key == old_save.elem));
        (screen.keys["settings-footer:settings.draft:43:back"],
            screen.keys["settings-footer:settings.draft:43:save"])
    };
    frame(&mut d, &mut rig, 432, vec![key(Key::Up, tick(432))]);
    assert!(d.focus().is_some_and(|focus| focus.entry == entry && [back, save].contains(&focus.elem)),
        "Up must enter the live footer, not retire into the Language row or reuse the old Save");
    assert!(rig.actions.is_empty());

    // Use real OK press/commit delivery for both live footer actions. This rig
    // records intents without applying a runtime response that would close it.
    if d.focus().unwrap().elem != back {
        frame(&mut d, &mut rig, 448, vec![key(Key::Left, tick(448))]);
    }
    assert_focus(&d, "settings-footer:settings.draft:43:back");
    let completed = press_header_ok(&mut d, &mut rig, 480);
    assert_eq!(rig.actions, vec![OttAction::EditSetting("settings.draft:43:back".into())]);
    frame(&mut d, &mut rig, completed + 32, vec![key(Key::Right, tick(completed + 32))]);
    assert_focus(&d, "settings-footer:settings.draft:43:save");
    press_header_ok(&mut d, &mut rig, completed + 64);
    assert_eq!(rig.actions, vec![OttAction::EditSetting("settings.draft:43:back".into()),
        OttAction::EditSetting("settings.draft:43:save".into())]);
}

#[test]
fn dispatcher_classic_search_back_restores_origin_in_the_actual_focus_engine() {
    let _guard = plx_base::testlock::serial();
    let make = |revision, query: &str, ids: &[&str]| {
        let mut view = (*navigation_view("channels:category-a", revision,
            ids.iter().map(|id| navigation_row(id, OttAction::Play((*id).into()))).collect(),
            Some("origin"))).clone();
        view.search = Some(OttSearch { query: query.into(), scope: "Fixture category".into() });
        Arc::new(view)
    };
    let (mut d, mut rig) = mount_snapshot_in_mode(make(1, "", &["first", "origin", "last"]), UiMode::Classic);
    assert_focus(&d, "item:origin");
    let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
    d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
        Delivery::Screen(ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor {
            text_policy: OttTextPolicy::Predictive, kind: OttEditorKind::Search,
            session: 81, key: String::new(), title: "Fixture category".into(),
            value: "needle".into(), secret: false, limit: 256,
        }))))));
    frame(&mut d, &mut rig, 256, vec![]);
    frame(&mut d, &mut rig, 272, vec![key(Key::Down, tick(272)), key_up(Key::Down, tick(272))]);
    press_navigation_ok(&mut d, &mut rig, 288, false);
    assert_eq!(rig.actions.iter().filter(|action| matches!(action,
        OttAction::SubmitSearch { session: 81, .. })).count(), 1);
    publish_snapshot(&mut d, &mut rig, make(2, "needle", &["first", "last"]), 400);
    d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
        Delivery::Screen(ScreenEvent::App(AppMsg::OttEditorResult { session: 81, error: None }))));
    frame(&mut d, &mut rig, 416, vec![]);
    assert_focus(&d, "item:first");
    let before = rig.actions.len();
    rig.transition = Some((OttAction::ClearSearch, make(3, "", &["last", "first", "origin"])));
    frame(&mut d, &mut rig, 448, vec![key(Key::Back, tick(448)), key_up(Key::Back, tick(448))]);
    frame(&mut d, &mut rig, 464, vec![]);
    assert_eq!(&rig.actions[before..], &[OttAction::ClearSearch]);
    assert_focus(&d, "item:origin");
    assert_eq!(d.nav.top_page().unwrap().inst.as_ref().unwrap().id, instance);
}

fn search_return_page_view(revision: u64, query: &str, selected: &str) -> Arc<OttView> {
    let mut view = (*classic_arrow_view(0, false)).clone();
    view.revision = revision;
    view.search = Some(OttSearch { query: query.into(), scope: "Fixture category".into() });
    view.selected_row = Some(selected.into());
    if !query.is_empty() { view.rows.retain(|row| row.id != "channel-0"); }
    Arc::new(view)
}

fn submit_search_return_page(d: &mut Dispatcher<TestHost>, rig: &mut TestRig, at: u32) -> u32 {
    let instance = d.nav.top_page().unwrap().inst.as_ref().unwrap().id;
    d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
        Delivery::Screen(ScreenEvent::App(AppMsg::OttEditor(Arc::new(OttEditor {
            text_policy: OttTextPolicy::Predictive, kind: OttEditorKind::Search,
            session: 81, key: String::new(), title: "Fixture category".into(),
            value: "needle".into(), secret: false, limit: 256,
        }))))));
    frame(d, rig, at, vec![]);
    frame(d, rig, at + 16, vec![key(Key::Down, tick(at + 16)), key_up(Key::Down, tick(at + 16))]);
    press_navigation_ok(d, rig, at + 32, false);
    assert_eq!(rig.actions.iter().filter(|action| matches!(action,
        OttAction::SubmitSearch { session: 81, .. })).count(), 1);
    let revision = rig.snapshot.revision + 1;
    publish_snapshot(d, rig, search_return_page_view(revision, "needle", "channel-1"), at + 128);
    d.emit(MachineId::Nav, Fx::Deliver(MachineId::Instance(instance),
        Delivery::Screen(ScreenEvent::App(AppMsg::OttEditorResult { session: 81, error: None }))));
    frame(d, rig, at + 144, vec![]);
    at + 160
}

fn search_return_page_input(channel_button: bool, direction: i32, edge: Edge, at: u32) -> InputEvent<u32> {
    if channel_button {
        InputEvent { at: tick(at), source: plx_machine::machine::Source::RemoteFifo,
            kind: InputKind::Key { key: Key::Other, sym: 0,
                wcode: if direction > 0 { plx_ui::consts::WCODE_CH_DOWN_KEY }
                    else { plx_ui::consts::WCODE_CH_UP_KEY }, edge, at_edge: false } }
    } else {
        let key_code = if direction > 0 { Key::Right } else { Key::Left };
        if edge == Edge::Up { key_up(key_code, tick(at)) } else { key(key_code, tick(at)) }
    }
}

#[test]
fn dispatcher_classic_search_paging_cancels_only_a_real_pending_return() {
    let _guard = plx_base::testlock::serial();
    for channel_button in [false, true] {
        for direction in [-1, 1] {
            let (mut d, mut rig) = mount_snapshot_in_mode(
                search_return_page_view(1, "", "channel-0"), UiMode::Classic);
            let origin = d.focus().unwrap();
            let at = submit_search_return_page(&mut d, &mut rig, 256);
            assert_focus(&d, "item:channel-1");
            let before_page = d.focus().unwrap();
            let requested = {
                let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
                    .as_any().unwrap().downcast_ref::<OttScreen>().unwrap();
                screen.remote_page_key(before_page.elem, direction)
            };
            if direction > 0 { assert!(requested.is_some_and(|key| key != before_page.elem)); }
            else { assert!(requested.is_none_or(|key| key == before_page.elem), "first row has no previous page"); }
            frame(&mut d, &mut rig, at, vec![key(Key::Back, tick(at)), key_up(Key::Back, tick(at))]);
            assert!(matches!(rig.actions.last(), Some(OttAction::ClearSearch)));
            frame(&mut d, &mut rig, at + 16, vec![
                search_return_page_input(channel_button, direction, Edge::Down, at + 16),
                search_return_page_input(channel_button, direction, Edge::Up, at + 16),
            ]);
            let after_page = d.focus().unwrap();
            if direction > 0 { assert_eq!(after_page.elem, requested.unwrap()); }
            else { assert_eq!(after_page, before_page); }
            let revision = rig.snapshot.revision + 1;
            publish_snapshot(&mut d, &mut rig, search_return_page_view(revision, "", "channel-0"), at + 48);
            assert_eq!(d.focus(), Some(if direction > 0 { after_page } else { origin }),
                "a moved page wins; a no-op page keeps the pending search return");
            assert!(!rig.actions.iter().any(|action| matches!(action, OttAction::Play(_) | OttAction::VolumeStep(_))));
        }
    }
}

#[test]
fn dispatcher_classic_paging_retires_a_pre_actions_search_bookmark() {
    let _guard = plx_base::testlock::serial();
    for channel_button in [false, true] {
        let (mut d, mut rig) = mount_snapshot_in_mode(
            search_return_page_view(1, "", "channel-0"), UiMode::Classic);
        let origin = d.focus().unwrap();
        publish_snapshot(&mut d, &mut rig, navigation_view("actions", 2,
            vec![navigation_row("order.search", OttAction::BeginSearch)], Some("order.search")), 256);
        publish_snapshot(&mut d, &mut rig, search_return_page_view(3, "", "channel-1"), 272);
        assert_focus(&d, "item:channel-1");
        let requested = arrow_page_target(&d, 1);
        frame(&mut d, &mut rig, 288, vec![
            search_return_page_input(channel_button, 1, Edge::Down, 288),
            search_return_page_input(channel_button, 1, Edge::Up, 288),
        ]);
        let paged = d.focus().unwrap();
        assert_eq!(paged.elem, requested);
        assert_ne!(paged, origin);
        // The editor delivery is deliberately delayed until after actual paging.
        // It must capture the new row, not the earlier pre-Actions bookmark.
        let at = submit_search_return_page(&mut d, &mut rig, 320);
        frame(&mut d, &mut rig, at, vec![key(Key::Back, tick(at)), key_up(Key::Back, tick(at))]);
        assert!(matches!(rig.actions.last(), Some(OttAction::ClearSearch)));
        let revision = rig.snapshot.revision + 1;
        publish_snapshot(&mut d, &mut rig, search_return_page_view(revision, "", "channel-0"), at + 32);
        assert_eq!(d.focus(), Some(paged), "paging retires the old Actions bookmark before the editor opens");
    }
}

#[path = "shelf_tests.rs"]
mod shelf_tests;


#[test]
fn classic_play_pause_open_category_selection_once_without_transport_or_browse() {
    let _guard = plx_base::testlock::serial();
    for mode in [UiMode::Classic, UiMode::Modern] {
        for key_code in [plx_ui::consts::WCODE_PLAY, plx_ui::consts::WCODE_PAUSE, plx_ui::consts::WCODE_PLAYPAUSE] {
            let (mut d, mut rig) = mount_snapshot_in_mode(classic_arrow_view(0, false), mode);
            let remote = |edge, ms| InputEvent { at: tick(ms),
                source: plx_machine::machine::Source::RemoteFifo,
                kind: InputKind::Key { key: Key::Other, sym: 0, wcode: key_code, edge, at_edge: false } };
            frame(&mut d, &mut rig, 256, vec![remote(Edge::Down, 256)]);
            frame(&mut d, &mut rig, 272, vec![remote(Edge::Up, 272)]);
            if mode == UiMode::Classic {
                assert_eq!(rig.actions, vec![OttAction::ShowCategories]);
                assert!(!d.nav.top_page().unwrap().inst.as_ref().unwrap().screen
                    .as_any().unwrap().downcast_ref::<OttScreen>().unwrap().browse);
            } else {
                assert!(!rig.actions.contains(&OttAction::ShowCategories), "Modern keeps its established transport/focus policy");
            }
        }
    }
}


#[test]
fn pending_play_cover_blocks_hidden_list_and_back_restores_exact_selection() {
    let _guard = plx_base::testlock::serial();
    for (mode, source_title, display_label) in [UiMode::Classic, UiMode::Modern].into_iter().flat_map(|mode| [
        (mode, Some("123 Channel A"), "18  123 Channel A"),
        (mode, None, "007 Channel"),
        (mode, Some(""), "18  "),
    ]) {
        let mut view = (*classic_arrow_view(0, false)).clone();
        view.selected_row = Some("channel-17".into());
        view.rows[17].title = display_label.into();
        view.rows[17].content_title = source_title.map(str::to_owned);
        let (mut d, mut rig) = mount_snapshot_in_mode(Arc::new(view.clone()), mode);
        let selected = d.focus();
        view.revision += 1;
        view.pending_play = Some("channel-17".into());
        view.status = "Opening video…".into();
        publish_snapshot(&mut d, &mut rig, Arc::new(view.clone()), 256);
        rig.actions.clear();
        for (n, code) in [Key::Down, Key::Up, Key::Ok].into_iter().enumerate() {
            let ms = 300 + n as u32 * 32;
            frame(&mut d, &mut rig, ms, vec![key(code, tick(ms)), key_up(code, tick(ms))]);
        }
        let remote = |edge, ms| InputEvent { at: tick(ms),
            source: plx_machine::machine::Source::RemoteFifo,
            kind: InputKind::Key { key: Key::Other, sym: 0,
                wcode: plx_ui::consts::WCODE_PLAYPAUSE, edge, at_edge: false } };
        frame(&mut d, &mut rig, 420, vec![remote(Edge::Down, 420), remote(Edge::Up, 420)]);
        frame(&mut d, &mut rig, 2200, vec![]);
        assert!(rig.actions.is_empty(), "covered list must not Play, preview or navigate");
        assert_eq!(d.focus(), selected);
        // The actual draw must not publish pointer targets from the covered list.
        let current = d.focus();
        let page = d.nav.tabs.stack.top_mut().unwrap();
        let screen = page.inst.as_mut().unwrap().screen.as_any_mut().unwrap().downcast_mut::<OttScreen>().unwrap();
        let cx = Cx { views: rig.mode, tick: tick(2200), measure: &rig.measure,
            press: Default::default(), focus: plx_machine::machine::FocusRead { current, ..Default::default() },
            owner: InputOwner::Entry(page.id) };
        let mut draw = DrawFrame::new(&cx, plx_ui::Painter::root());
        let (runs, waiting) = plx_ui::placeholder::capture_declared(||
            plx_ui::draw_census::capture_runs(||
                plx_ui::screen::record_stops_while_recording(|| {
                    plx_gfx::gfx::without_frame_clear(|| Screen::<TestHost>::draw(screen, &mut draw));
                })));
        assert_eq!(waiting.of(plx_ui::placeholder::Reason::WorkingReadout), 1,
            "the actual pending cover draws its loading spinner in {mode:?}");
        assert!(runs.iter().any(|run| run.text == "Opening video…"), "missing loading caption in {mode:?}");
        let expected_title = source_title.unwrap_or(display_label);
        assert!(expected_title.is_empty() || runs.iter().any(|run| run.text == expected_title),
            "missing original channel title {expected_title:?} in {mode:?}");
        assert!(!runs.iter().any(|run| run.text.starts_with("18  ") || run.text == "18"),
            "the catalogue ordinal must not enter the loading readout in {mode:?}");
        assert!(draw.into_stops().is_empty(), "pending cover exposes no hidden list controls");
        frame(&mut d, &mut rig, 2216, vec![key(Key::Back, tick(2216)), key_up(Key::Back, tick(2216))]);
        assert_eq!(rig.actions, vec![OttAction::Back]);
        view.revision += 1;
        view.pending_play = None;
        view.status.clear();
        publish_snapshot(&mut d, &mut rig, Arc::new(view), 2232);
        assert_eq!(d.focus(), selected, "cancel/failure must reopen the exact original row");
    }
}


#[test]
fn dispatcher_classic_footer_search_and_favorite_use_existing_context_actions() {
    let _guard = plx_base::testlock::serial();
    let _locale = plx_platform::i18n::language_on_this_thread_for_test(plx_platform::i18n::Preference::En);
    for favorite in [false, true] {
        for action in ["nav:search", "nav:favorite"] {
            let (mut d, mut rig) = mount_snapshot_in_mode(discoverable_live_catalog(favorite), UiMode::Classic);
            let rects = draw_pointer_map(&mut d, &rig, &[action]);
            let screen = d.nav.top_page().unwrap().inst.as_ref().unwrap().screen.as_any().unwrap()
                .downcast_ref::<OttScreen>().unwrap();
            assert!(!screen.browse, "common actions must be reachable without Browse");
            assert!(screen.header(&rig.measure).unwrap().stops.iter().any(|(key, _)| *key == screen.keys[action]));
            click_header(&mut d, &mut rig, rects[0], 256);
            let expected = if action == "nav:search" { OttAction::BeginSearch }
                else { OttAction::ToggleFavorite("selected".into()) };
            assert_eq!(semantic_actions(&rig), vec![&expected]);
            assert!(!rig.actions.iter().any(|action| matches!(action, OttAction::Play(_) | OttAction::Random(_))));
        }
    }
}
