//! Pointer Back uses the shipping dispatcher and Settings surface in both UI modes.
//! Like settings_nav_structure_tests, these sans-IO tests publish the real queried
//! geometry to the hit map; they do not execute GL paint or the application writer.
use super::test_support::{root_key, scratch_session, seated_scratch_session};
use super::*;
use crate::registry::{self, AppMsg, InnerViews};
use plx_machine::machine::{
    FontScale, InputEvent, InputKind, InputOwner, Source, TimerId, TypographyMeasure, UiMode,
};
use plx_machine::present::Present;
use plx_ui::containers::modal::Style;
use plx_ui::dispatch::{CxParts, Dispatcher, FrameReport, NoTap, Rig, Split};
use plx_ui::fixture::{FixtureMeasure, key, key_up, tick};
use plx_ui::screen::{Activate, Hover, Stop};

struct Mounter;
impl plx_ui::screen::Mounter<InnerHost> for Mounter {
    fn mount(
        &mut self,
        id: InstanceId,
        arg: &SettingsPage,
        _: &ReturnState<u32>,
        cx: &Cx<'_, InnerHost>,
        fx: &mut Effects<'_, InnerHost>,
    ) -> Box<dyn Screen<InnerHost>> {
        let InputOwner::Entry(entry) = cx.owner else {
            panic!("mount without entry")
        };
        match arg {
            SettingsPage::About => mount_page(entry, *arg, cx, fx),
            SettingsPage::ConsentStage(_) => {
                Box::new(RouteSurface::new(entry, id, Family::FirstRunConsent, *arg))
            }
            _ => Box::new(RouteSurface::new(entry, id, Family::Settings, *arg)),
        }
    }
}
struct TestRig {
    mounter: Mounter,
    measure: TypographyMeasure<FixtureMeasure>,
    mode: UiMode,
    stores: plx_data::stores::Stores,
    directory: plx_data::stores::browse::DirectorySnapshot,
    preferences: Vec<registry::PreferenceCmd>,
    privacy_actions: usize,
    roots: usize,
}
impl Rig<InnerHost> for TestRig {
    fn split(&mut self) -> Split<'_, InnerHost> {
        self.stores.capture_browse(&mut self.directory);
        Split {
            mounter: &mut self.mounter,
            views: InnerViews {
                directory: self.directory.view(),
                ui_mode: self.mode,
            },
            measure: &self.measure,
        }
    }
    fn deliver(
        &mut self,
        _: MachineId,
        _: &AppMsg,
        _: &CxParts<u32>,
        _: &mut Effects<'_, InnerHost>,
    ) -> Handled {
        Handled::No
    }
    fn timer(
        &mut self,
        _: MachineId,
        _: TimerId,
        _: &CxParts<u32>,
        _: &mut Effects<'_, InnerHost>,
    ) {
    }
    fn app_fx(
        &mut self,
        _: MachineId,
        effect: AppFx,
        _: &CxParts<u32>,
        _: &mut Effects<'_, InnerHost>,
    ) {
        if matches!(
            &effect,
            AppFx::Consent(_) | AppFx::Loop(registry::LoopReq::DeleteAllLocalData)
        ) {
            self.privacy_actions += 1;
        }
        if matches!(&effect, AppFx::Loop(registry::LoopReq::BackAtRoot)) {
            self.roots += 1;
        }
        if let AppFx::Preferences(command) = effect {
            self.preferences.push(command);
        }
    }
    fn log(&mut self, _: &str) {}
    fn prepare(&mut self, _: &mut Budget, _: &mut Present) {}
    fn ls2_pump(&mut self) {}
    fn opaque_route(&mut self, _: bool) {}
    fn clear_opaque_region(&mut self) {}
    fn now_us(&self) -> u64 {
        0
    }
    fn back_at_root(&mut self) {
        self.roots += 1;
    }
}
struct Harness {
    d: Dispatcher<InnerHost>,
    rig: TestRig,
    id: EntryId,
    ms: u32,
}
impl Harness {
    fn open(mode: UiMode, page: SettingsPage, percent: u16) -> Self {
        let mut measure = TypographyMeasure::new(FixtureMeasure);
        measure.set_font_scale(FontScale::from_percent(percent).unwrap());
        let mut rig = TestRig {
            mounter: Mounter,
            measure,
            mode,
            stores: Default::default(),
            directory: Default::default(),
            preferences: Vec::new(),
            privacy_actions: 0,
            roots: 0,
        };
        let mut d = Dispatcher::new();
        d.request(MachineId::Nav, NavOp::Root(SettingsPage::About));
        d.frame_with(&mut rig, tick(0), vec![], vec![], &mut NoTap, false);
        d.nav.next_style = Style::Opaque { snapshot: true };
        d.request(MachineId::Nav, NavOp::Present(page));
        d.frame_with(&mut rig, tick(16), vec![], vec![], &mut NoTap, false);
        let id = d.nav.modals.top().unwrap().entry.id;
        let mut h = Self { d, rig, id, ms: 16 };
        h.settle();
        h
    }
    fn frame(&mut self, inputs: Vec<InputEvent<u32>>) -> FrameReport {
        self.ms += 16;
        let report = self.d.frame_with(
            &mut self.rig,
            tick(self.ms),
            inputs,
            vec![],
            &mut NoTap,
            false,
        );
        // Match the application's frame tail: retire bodies after their Unmount.
        self.d.prune(&report.unmounted);
        report
    }
    fn settle(&mut self) {
        for _ in 0..60 {
            self.frame(vec![]);
        }
    }
    fn press(&mut self, k: Key) {
        self.frame(vec![key(k, tick(self.ms + 16))]);
        self.frame(vec![key_up(k, tick(self.ms + 16))]);
        self.settle();
    }
    fn path(&self) -> String {
        super::composed_tests::path(&self.d, self.id)
    }
    fn context(&self) -> Cx<'_, InnerHost> {
        Cx {
            views: InnerViews {
                directory: self.rig.directory.view(),
                ui_mode: self.rig.mode,
            },
            tick: tick(self.ms),
            measure: &self.rig.measure,
            press: Default::default(),
            focus: plx_machine::machine::FocusRead {
                current: self.d.focus(),
                ..Default::default()
            },
            owner: InputOwner::Entry(self.id),
        }
    }
    fn placed(&self, elem: u32) -> Option<Placed> {
        self.d
            .nav
            .entry(self.id)?
            .inst
            .as_ref()?
            .screen
            .place(&elem, &self.context(), At::Drawn)
    }
    fn groups(&self) -> Vec<GroupSpec> {
        let mut groups = Vec::new();
        self.d
            .nav
            .entry(self.id)
            .unwrap()
            .inst
            .as_ref()
            .unwrap()
            .screen
            .groups(&self.context(), &mut groups);
        groups
    }
    fn point(&mut self, elem: u32) {
        let placed = self
            .placed(elem)
            .unwrap_or_else(|| panic!("missing visible control {elem}: {}", self.path()));
        let rect = placed.rect.intersect(placed.clip);
        assert!(
            rect.w > 0.0 && rect.h > 0.0,
            "control is clipped: {placed:?}"
        );
        let screen = &self
            .d
            .nav
            .entry(self.id)
            .unwrap()
            .inst
            .as_ref()
            .unwrap()
            .screen;
        let kind = screen
            .press_kind(&elem, &self.context())
            .or_else(|| {
                let group = screen.group_of(&elem, &self.context())?;
                self.groups()
                    .into_iter()
                    .find(|g| g.id == group)
                    .map(|g| g.elem)
            })
            .unwrap();
        self.d.input.hit.fill(vec![Stop {
            key: FocusKey {
                entry: self.id,
                elem,
            },
            rect: placed.rect,
            rest_rect: placed.rest_rect,
            clip: placed.clip,
            hover: Hover::Focus,
            activate: if kind == ElemKind::Control {
                Activate::Press
            } else {
                Activate::Direct
            },
        }]);
        self.d.input.hit.swap();
        self.d.input.hit.dpad_mode = false;
        self.frame(vec![InputEvent {
            at: tick(self.ms + 16),
            source: Source::Script,
            kind: InputKind::Pointer {
                x: rect.cx(),
                y: rect.cy(),
                hit: None,
            },
        }]);
        self.frame(vec![InputEvent {
            at: tick(self.ms + 16),
            source: Source::Script,
            kind: InputKind::Click {
                x: rect.cx(),
                y: rect.cy(),
                hit: None,
            },
        }]);
    }
    fn click(&mut self, elem: u32) {
        self.point(elem);
        self.frame(vec![key_up(Key::Ok, tick(self.ms + 16))]);
        self.settle();
    }
    fn assert_focus(&self, elem: u32) {
        assert_eq!(
            self.d.focus(),
            Some(FocusKey {
                entry: self.id,
                elem
            })
        );
    }
}

#[test]
fn pointer_back_walks_settings_picker_parent_and_root_in_both_modes() {
    let _serial = plx_base::testlock::serial();
    let _session = seated_scratch_session("settings-pointer-back-route");
    for mode in [UiMode::Classic, UiMode::Modern] {
        for percent in [20, 100, 200] {
            let mut h = Harness::open(mode, SettingsPage::Root, percent);
            let back = h
                .placed(registry::BAND)
                .expect("Settings must have a visible pointer Back");
            // Test containment directly: intersect reconstructs the height by subtraction,
            // which need not reproduce the original f32 bits even when no edge is clipped.
            assert!(
                back.rect.w > 0.0 && back.rect.h > 0.0
                    && Rect::FULL.contains(back.rect.x, back.rect.y)
                    && Rect::FULL.contains(back.rect.x + back.rect.w, back.rect.y + back.rect.h),
                "Back must remain fully inside the viewport: {:?}",
                back.rect,
            );
            let playback = root_key(RootId::Playback);
            // Scroll the actual table before clicking a row at large font scale.
            for _ in 0..16 {
                if h.d.focus().is_some_and(|k| k.elem == playback) {
                    break;
                }
                h.press(Key::Down);
            }
            h.assert_focus(playback);
            h.click(playback);
            assert!(h.path().contains("/playback:"), "{}", h.path());
            h.click(0); // The first Playback row, Quality.
            assert!(h.path().contains("picker Quality"), "{}", h.path());
            h.click(registry::BAND);
            assert!(
                h.path().contains("/playback:") && !h.path().contains("picker "),
                "{}",
                h.path()
            );
            h.assert_focus(0);
            h.click(registry::BAND);
            assert!(!h.path().contains("/playback:"), "{}", h.path());
            h.assert_focus(playback);
            h.click(registry::BAND);
            assert!(
                h.d.nav.entry(h.id).is_none(),
                "root Back must close the modal"
            );
            assert_eq!(
                h.rig.roots, 0,
                "must not exit the application beneath Settings"
            );
            assert!(
                h.rig.preferences.is_empty(),
                "navigation must not save a setting"
            );
        }
    }
}

#[test]
fn dpad_reaches_pointer_back_and_keyboard_back_keeps_the_same_route() {
    let _serial = plx_base::testlock::serial();
    let _session = scratch_session("settings-pointer-back-keys");
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut h = Harness::open(mode, SettingsPage::Root, 100);
        for _ in 0..24 {
            if h.d.focus().is_some_and(|k| k.elem == registry::BAND) {
                break;
            }
            h.press(Key::Down);
        }
        h.assert_focus(registry::BAND);
        h.press(Key::Ok);
        assert!(h.d.nav.entry(h.id).is_none());
        assert_eq!(h.rig.roots, 0);
        let mut h = Harness::open(mode, SettingsPage::Picker(PickerKind::Quality), 100);
        h.press(Key::Back);
        assert!(h.path().contains("/playback:") && !h.path().contains("picker "));
        h.click(registry::BAND);
        assert!(!h.path().contains("/playback:"));
    }
}

#[test]
fn pointer_back_respects_pending_account_save_then_returns_after_failure() {
    use plx_plex::plex::account::{AudioPreferences, PreferenceError, PreferenceRequest};
    use registry::{AccountPreferenceReply, PreferenceCmd};
    let _serial = plx_base::testlock::serial();
    let _session = scratch_session("settings-pointer-back-pending");
    struct Restore(std::sync::Arc<plx_plex::plex::session::CurrentProfile>);
    impl Drop for Restore {
        fn drop(&mut self) {
            plx_plex::plex::session::publish_profile_for_test(
                self.0.user.clone(),
                self.0.generation,
            );
        }
    }
    let _restore = Restore(plx_plex::plex::session::current_snapshot());
    for (index, mode) in [UiMode::Classic, UiMode::Modern].into_iter().enumerate() {
        let user = plx_plex::plex::session::UserRef {
            id: 7,
            uuid: format!("back-pending-{index}"),
            ..Default::default()
        };
        let generation = 910 + index as u32;
        plx_plex::plex::session::publish_profile_for_test(Some(user.clone()), generation);
        let (request, snapshot) =
            PreferenceRequest::fixture_for_test(user, generation, AudioPreferences::default());
        let mut h = Harness::open(mode, SettingsPage::AudioSubtitles, 100);
        assert!(
            h.placed(registry::BAND).is_some(),
            "loading still has a pointer exit"
        );
        let Some(PreferenceCmd::Load { reply }) = h.rig.preferences.pop() else {
            panic!("expected account load")
        };
        reply
            .send(AccountPreferenceReply {
                request: Some(request.clone()),
                outcome: Ok(snapshot.clone()),
            })
            .unwrap();
        h.settle();
        h.click(1); // Subtitle mode.
        assert!(h.path().contains("picker SubtitleMode"));
        h.press(Key::Down); // Choose another option, not the checked value.
        h.press(Key::Ok);
        let Some(PreferenceCmd::Save { reply, .. }) = h.rig.preferences.pop() else {
            panic!("expected one account save")
        };
        assert!(h.path().contains("busy=true"));
        h.click(registry::BAND);
        assert!(h.path().contains("picker SubtitleMode") && h.path().contains("busy=true"));
        h.press(Key::Back);
        assert!(h.path().contains("picker SubtitleMode") && h.path().contains("busy=true"));
        assert!(
            h.rig.preferences.is_empty(),
            "Back must not submit/retry an account write"
        );
        reply
            .send(AccountPreferenceReply {
                request: Some(request),
                outcome: Err(PreferenceError::Unavailable),
            })
            .unwrap();
        h.settle();
        assert!(!h.path().contains("busy=true"));
        h.click(registry::BAND);
        assert!(h.path().contains("/audio-subtitles:") && !h.path().contains("picker "));
        h.assert_focus(1);
    }
}

#[test]
fn pointer_back_is_not_an_escape_behind_a_confirmation_or_first_run_consent() {
    let _serial = plx_base::testlock::serial();
    let _session = seated_scratch_session("settings-pointer-back-guards");
    struct Restore(plx_media::route::DirectPlayMode);
    impl Drop for Restore {
        fn drop(&mut self) {
            plx_media::route::restore_direct_play_mode(self.0);
        }
    }
    let _restore = Restore(plx_media::route::direct_play_mode());
    plx_media::route::restore_direct_play_mode(plx_media::route::DirectPlayMode::Auto);
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut h = Harness::open(mode, SettingsPage::Picker(PickerKind::DirectPlay), 100);
        h.click(1); // Forced, which requires its own confirmation.
        assert!(h.path().contains("confirm=true"));
        assert!(
            h.placed(registry::BAND).is_none(),
            "do not expose Back behind the foreground question"
        );
        assert_eq!(
            h.groups().iter().map(|g| g.id).collect::<Vec<_>>(),
            vec![ALERT_GROUP]
        );
        h.click(registry::ALERT); // The question's actual Cancel, not the family Back.
        assert!(!h.path().contains("confirm=true"));
        assert!(h.rig.preferences.is_empty());
        let before_back = h.path();
        let focus_before_back = h.d.focus();
        h.click(registry::BAND);
        assert!(
            h.d.nav.entry(h.id).is_some(),
            "confirmation Back must retain parent Playback"
        );
        assert!(
            h.path().contains("/playback:") && !h.path().contains("picker "),
            "mode={mode:?}, before Back={before_back}, focus before Back={focus_before_back:?}, after Back={}, focus after Back={:?}",
            h.path(),
            h.d.focus(),
        );
        let reporting = plx_telemetry::telemetry::sender::configured();
        let mut h = Harness::open(mode, SettingsPage::ConsentStage(0), 100);
        let before = h.path();
        assert!(
            before.contains(if reporting { "consent mode=1 " } else { "consent mode=0 " }),
            "the first-run target must reflect the build's reporting capability: {before}"
        );
        let band_lengths = h.groups().iter()
            .filter(|group| group.id == registry::BAND_GROUP)
            .map(|group| group.len)
            .collect::<Vec<_>>();
        if reporting {
            assert_eq!(band_lengths, vec![2], "first-run consent exposes only its two-answer band");
            assert!(h.placed(registry::BAND).is_some());
            assert!(h.placed(registry::BAND + 1).is_some());
        } else {
            assert!(band_lengths.is_empty(), "the non-reporting projection has no Back or answer band");
            assert!(h.placed(registry::BAND).is_none());
            assert!(h.placed(registry::BAND + 1).is_none());
        }
        h.press(Key::Back);
        if reporting {
            assert!(h.d.nav.entry(h.id).is_some(), "first-run consent keeps its surface");
            assert_eq!(h.path(), before, "first-run consent owns Back");
            assert_eq!(h.rig.roots, 1, "the actual first question requests the platform root press");
        } else {
            // A stale first-run target projects to ordinary Privacy & data when no
            // reporter exists. Its normal Back closes it without answering a question.
            assert!(h.d.nav.entry(h.id).is_none(), "the non-reporting projection must close");
            assert_eq!(h.rig.roots, 0, "closing the projection must not exit the application");
        }
        assert_eq!(h.rig.privacy_actions, 0);
    }
}

#[test]
fn an_old_back_press_cannot_pop_the_new_inner_page() {
    let _serial = plx_base::testlock::serial();
    let _session = seated_scratch_session("settings-pointer-back-stale-press");
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut h = Harness::open(mode, SettingsPage::Root, 100);
        h.point(registry::BAND);
        let armed = h.d.input.arm.expect("the pointer armed a Back control");
        let instance = h.d.nav.instance_of(h.id).unwrap();
        // Another admitted command changes the inner page before the press completes.
        h.d.emit(
            MachineId::Input,
            Fx::Deliver(
                MachineId::Instance(instance),
                Delivery::Screen(ScreenEvent::Activate(root_key(RootId::Playback))),
            ),
        );
        h.frame(vec![]);
        h.settle();
        assert!(h.path().contains("/playback:"));
        let p = h.placed(registry::BAND).expect("new page has its own Back");
        h.d.input.hit.fill(vec![Stop {
            key: armed.key,
            rect: p.rect,
            rest_rect: p.rest_rect,
            clip: p.clip,
            hover: Hover::Focus,
            activate: Activate::Press,
        }]);
        h.d.input.hit.swap();
        h.d.input.hit.dpad_mode = false;
        h.frame(vec![InputEvent {
            at: tick(h.ms + 16),
            source: Source::Script,
            kind: InputKind::Pointer {
                x: p.rect.cx(),
                y: p.rect.cy(),
                hit: None,
            },
        }]);
        h.assert_focus(registry::BAND);
        // The same outer entry/key is valid again. It must not admit the old inner-page press.
        h.d.emit(
            MachineId::Input,
            Fx::Deliver(
                MachineId::Instance(instance),
                Delivery::Press {
                    id: armed.id,
                    key: armed.key,
                    held: false,
                },
            ),
        );
        let report = h.frame(vec![]);
        assert_eq!(
            report.dropped_deliveries, 0,
            "exercise the surface guard, not an earlier typed-key rejection"
        );
        h.settle();
        assert!(
            h.path().contains("/playback:"),
            "stale Back popped the new page"
        );
        assert!(h.rig.preferences.is_empty());
        h.click(registry::BAND);
        assert!(!h.path().contains("/playback:"), "a fresh Back still works");
    }
}
