//! Optional Home app handoff through the shared OTT Settings catalogue.
//! The UI only admits and polls; one bounded platform request runs off-thread.
use super::*;
use plx_platform::tv::home_integration::{
    self as tv_home_integration, Launch as HomeIntegrationLaunch,
};

const KEY: &str = "home.integration.open";
static LAUNCH_PENDING: AtomicBool = AtomicBool::new(false);

struct Permit;
impl Permit {
    fn claim() -> Option<Self> {
        LAUNCH_PENDING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self)
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        LAUNCH_PENDING.store(false, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Address {
    owner: InstanceId,
    generation: u64,
    epoch: u64,
}
struct Flight {
    address: Address,
    cancelled: Arc<AtomicBool>,
    result: Receiver<HomeIntegrationLaunch>,
}
#[derive(Default)]
pub(super) struct State {
    flight: Option<Flight>,
    epoch: u64,
    exhausted: bool,
}
impl State {
    fn prepare(
        &mut self,
        address: Address,
    ) -> Result<(SyncSender<HomeIntegrationLaunch>, Arc<AtomicBool>), &'static str> {
        if self.flight.is_some() {
            return Err(plx_platform::i18n::msg::ott_home_network_busy());
        }
        if self.exhausted || address.epoch != self.epoch {
            return Err(plx_platform::i18n::msg::ott_home_network_unavailable());
        }
        let (send, result) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        self.flight = Some(Flight {
            address,
            cancelled: cancelled.clone(),
            result,
        });
        Ok((send, cancelled))
    }
    fn retire(&mut self) {
        let Some(flight) = &self.flight else {
            return;
        };
        if !flight.cancelled.swap(true, Ordering::AcqRel) {
            match self.epoch.checked_add(1) {
                Some(next) => self.epoch = next,
                None => self.exhausted = true,
            }
        }
        // Keep the receiver until the original worker finishes: retirement is
        // not permission to overlap a second platform launch or replay an effect.
    }
    fn poll(&mut self, current: Option<Address>) -> Option<HomeIntegrationLaunch> {
        if self
            .flight
            .as_ref()
            .is_some_and(|flight| Some(flight.address) != current)
        {
            self.retire();
        }
        let result = match self.flight.as_ref()?.result.try_recv() {
            Ok(value) => value,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => HomeIntegrationLaunch::Unavailable,
        };
        let flight = self.flight.take().expect("observed flight");
        (!flight.cancelled.load(Ordering::Acquire) && Some(flight.address) == current)
            .then_some(result)
    }
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(flight) = &self.flight {
            flight.cancelled.store(true, Ordering::Release);
        }
    }
}
fn address(runtime: &Runtime, app: &App) -> Option<Address> {
    let owner = runtime.reply?;
    (runtime.loaded
        && !runtime.saving
        && runtime.page == "settings"
        && app.running
        && playback_allowed(app)
        && !app.pages.nav.suspended
        && !app.pages.has_pending_navigation()
        && current_ui_owner(app) == Some(owner)
        && input_policy::replace_source(runtime).is_ok())
    .then_some(Address {
        owner,
        generation: runtime.generation,
        epoch: runtime.home_integration.epoch,
    })
}
pub(super) fn entry(runtime: &Runtime) -> OttRow {
    let mut item = ui_projection::row(
        KEY.into(),
        plx_platform::i18n::msg::ott_home_network_title().into(),
        plx_platform::i18n::msg::ott_home_network_detail().into(),
        OttAction::EditSetting(KEY.into()),
        false,
    );
    item.enabled = runtime.loaded
        && !runtime.saving
        && runtime.home_integration.flight.is_none()
        && !runtime.home_integration.exhausted
        && input_policy::replace_source(runtime).is_ok();
    item
}
/// Back cancels immediately, even if an early handler consumes or refuses it.
/// This does not admit navigation, replay effects or cancel the Home action.
pub(super) fn cancel_on_back(runtime: &mut Runtime, action: &OttAction) {
    if matches!(action, OttAction::Back) {
        runtime.home_integration.retire();
    }
}
/// Called only after the common kiosk and parental gates accepted the action.
pub(super) fn before_action(runtime: &mut Runtime, action: &OttAction) {
    if !matches!(
        action,
        OttAction::FocusAt { .. } | OttAction::Refresh | OttAction::VolumeStep(_)
    ) && !matches!(action, OttAction::EditSetting(key) if key == KEY)
    {
        runtime.home_integration.retire();
    }
}
pub(super) fn action(
    runtime: &mut Runtime,
    app: &App,
    key: &str,
) -> Option<Result<(), &'static str>> {
    if key != KEY {
        return None;
    }
    Some((|| {
        let address =
            address(runtime, app).ok_or(plx_platform::i18n::msg::ott_home_network_unavailable())?;
        let permit = Permit::claim().ok_or(plx_platform::i18n::msg::ott_home_network_busy())?;
        let (send, cancelled) = runtime.home_integration.prepare(address)?;
        if !plx_base::task::spawn_small("OTT Home launch", move || {
            let _permit = permit;
            let result = if cancelled.load(Ordering::Acquire) {
                HomeIntegrationLaunch::Unavailable
            } else {
                // Fixed destination; no user supplied identifier, path, URL or credentials.
                // Once handed to LS2, an accepted request cannot be recalled.
                tv_home_integration::open(&cancelled)
            };
            let _ = send.try_send(result);
        }) {
            runtime.home_integration.flight = None;
            return Err(plx_platform::i18n::msg::ott_home_network_unavailable());
        }
        runtime.status = plx_platform::i18n::msg::ott_home_network_opening().into();
        runtime.changed();
        Ok(())
    })())
}
/// Retire at the actual background/Standby edge, even without an intervening tick.
pub(super) fn suspend(runtime: &mut Runtime) {
    runtime.home_integration.retire();
}

pub(super) fn tick(runtime: &mut Runtime, app: &App) {
    let current = address(runtime, app);
    let had_flight = runtime.home_integration.flight.is_some();
    if let Some(result) = runtime.home_integration.poll(current) {
        runtime.status = match result {
            HomeIntegrationLaunch::Requested => {
                plx_platform::i18n::msg::ott_home_network_requested()
            }
            HomeIntegrationLaunch::Rejected => plx_platform::i18n::msg::ott_home_network_rejected(),
            HomeIntegrationLaunch::Unavailable => {
                plx_platform::i18n::msg::ott_home_network_unavailable()
            }
        }
        .into();
    }
    // Retired replies cannot publish status, but draining their flight enables
    // the Home row again and must invalidate the retained Settings snapshot.
    if had_flight && runtime.home_integration.flight.is_none() {
        runtime.changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owner(id: u32, epoch: u64) -> Address {
        Address {
            owner: InstanceId(id),
            generation: 7,
            epoch,
        }
    }
    #[test]
    fn one_flight_survives_retirement_until_worker_completes_without_stale_publish() {
        let mut state = State::default();
        let first = owner(1, 0);
        let (send, cancelled) = state.prepare(first).unwrap();
        assert!(state.prepare(first).is_err());
        assert_eq!(state.poll(None), None);
        assert!(cancelled.load(Ordering::Acquire));
        assert!(state.flight.is_some());
        assert!(state.prepare(owner(2, state.epoch)).is_err());
        send.try_send(HomeIntegrationLaunch::Requested).unwrap();
        assert_eq!(state.poll(Some(owner(2, state.epoch))), None);
        assert!(state.flight.is_none());
    }
    #[test]
    fn generation_owner_and_navigation_epoch_each_fence_results() {
        for current in [
            None,
            Some(owner(2, 0)),
            Some(Address {
                generation: 8,
                ..owner(1, 0)
            }),
            Some(owner(1, 1)),
        ] {
            let mut state = State::default();
            let (send, _) = state.prepare(owner(1, 0)).unwrap();
            send.try_send(HomeIntegrationLaunch::Requested).unwrap();
            assert_eq!(state.poll(current), None);
        }
    }
    #[test]
    fn suspend_then_resume_same_owner_without_tick_keeps_old_result_retired() {
        let mut state = State::default();
        let original = owner(1, 0);
        let (send, cancelled) = state.prepare(original).unwrap();
        state.retire();
        send.try_send(HomeIntegrationLaunch::Requested).unwrap();
        assert!(cancelled.load(Ordering::Acquire));
        assert_eq!(state.poll(Some(original)), None);
        assert!(state.flight.is_none());
    }
    #[test]
    fn current_request_is_only_an_ack_and_failure_is_not_retried() {
        for outcome in [
            HomeIntegrationLaunch::Requested,
            HomeIntegrationLaunch::Rejected,
            HomeIntegrationLaunch::Unavailable,
        ] {
            let mut state = State::default();
            let current = owner(1, 0);
            let (send, _) = state.prepare(current).unwrap();
            send.try_send(outcome).unwrap();
            assert_eq!(state.poll(Some(current)), Some(outcome));
            assert_eq!(state.poll(Some(current)), None);
            assert!(state.flight.is_none());
        }
    }
    #[test]
    fn settings_row_uses_common_kiosk_gate_and_exposes_no_configuration() {
        let mut runtime =
            Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        assert!(!entry(&runtime).enabled);
        runtime.loaded = true;
        assert!(entry(&runtime).enabled);
        let row = entry(&runtime);
        assert_eq!(row.id, KEY);
        assert!(matches!(row.action, OttAction::EditSetting(ref key) if key == KEY));
        runtime
            .saved
            .settings
            .insert("nativeKiosk".into(), json!("malformed policy"));
        assert!(
            !entry(&runtime).enabled,
            "uncertain kiosk policy fails closed"
        );
        runtime.saved.settings.remove("nativeKiosk");
        let favorite = plx_data::ott::favorites::Favorite {
            title: "Fixture".into(),
            origin: plx_data::ott::favorites::Origin {
                provider: "local".into(),
                profile: "Fixture".into(),
                subprofile: None,
                source_alias: "Fixture".into(),
            },
            locator: plx_data::ott::favorites::Locator::Local {
                root_alias: "Fixture".into(),
                relative_path: "clip.mp4".into(),
            },
        };
        let pin = crate::app::ott_kiosk_stored::Stored::item(favorite, true).unwrap();
        runtime
            .saved
            .settings
            .insert("nativeKiosk".into(), pin.encode().unwrap());
        assert!(
            !entry(&runtime).enabled,
            "strict item pin cannot launch another application"
        );
    }
    #[test]
    fn dropping_state_cancels_queued_worker_without_an_effect_retry() {
        let mut state = State::default();
        let (_, cancelled) = state.prepare(owner(1, 0)).unwrap();
        drop(state);
        assert!(cancelled.load(Ordering::Acquire));
    }
    #[test]
    fn back_cancellation_is_sticky_and_does_not_cancel_the_home_action() {
        let mut runtime =
            Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        let original = owner(1, 0);
        let (send, cancelled) = runtime.home_integration.prepare(original).unwrap();
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        cancel_on_back(&mut runtime, &OttAction::EditSetting(KEY.into()));
        assert!(!cancelled.load(Ordering::Acquire));
        cancel_on_back(&mut runtime, &OttAction::Back);
        cancel_on_back(&mut runtime, &OttAction::Back);
        assert!(cancelled.load(Ordering::Acquire));
        assert_eq!(runtime.home_integration.epoch, 1);
        assert!(runtime.home_integration.flight.is_some());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        send.try_send(HomeIntegrationLaunch::Requested).unwrap();
        assert_eq!(runtime.home_integration.poll(Some(original)), None);
    }
    #[cfg(feature = "hostsim")]
    #[test]
    fn actual_player_settings_early_back_cancels_before_the_next_tick() {
        use plx_machine::machine::{MachineId, NavOp, Tick};
        use plx_screens::registry::AppArg;
        use plx_ui::containers::modal::Style;
        let _serial = plx_base::testlock::serial();
        let mut app = crate::app::run::lifecycle_regression_tests::app();
        fn settle(app: &mut App) {
            for _ in 0..24 {
                app.loop_t = app.loop_t.wrapping_add(16);
                let tick = Tick {
                    ms: app.loop_t,
                    dt_us: 16_000,
                };
                bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
            }
        }
        bridge::show_page(&mut app.pages, AppArg::Player);
        settle(&mut app);
        app.pages.nav.next_style = Style::PlayerPanel {
            survives_failure: true,
        };
        app.pages
            .request(MachineId::Nav, NavOp::Present(AppArg::Ott));
        settle(&mut app);
        assert!(bridge::ott_catalog_surface(&app.pages).is_some());
        let mut runtime =
            Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        runtime.remote = Remote::disabled_for_test("dev_home_fixture", "fixture");
        runtime.loaded = true;
        runtime.page = "settings";
        runtime.reply = current_ui_owner(&app);
        let original = Address {
            owner: runtime.reply.expect("actual Settings surface owner"),
            generation: runtime.generation,
            epoch: runtime.home_integration.epoch,
        };
        let (send, cancelled) = runtime.home_integration.prepare(original).unwrap();
        assert!(!cancelled.load(Ordering::Acquire));
        // This enters the real outer handler and its early Player-browse Back.
        // No OTT tick or platform worker is run before inspecting cancellation.
        handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();
        assert_eq!(runtime.page, "providers");
        assert!(cancelled.load(Ordering::Acquire));
        assert!(runtime.home_integration.flight.is_some());
        send.try_send(HomeIntegrationLaunch::Requested).unwrap();
        assert_eq!(runtime.home_integration.poll(Some(original)), None);
    }
    #[cfg(feature = "hostsim")]
    #[test]
    fn actual_boot_resume_early_back_cancels_home_before_return_and_drops_late_ack() {
        use plx_machine::machine::{MachineId, NavOp, Tick};
        use plx_screens::registry::AppArg;
        use plx_ui::containers::modal::Style;
        let _serial = plx_base::testlock::serial();
        let mut app = crate::app::run::lifecycle_regression_tests::app();
        fn settle(app: &mut App) {
            for _ in 0..24 {
                app.loop_t = app.loop_t.wrapping_add(16);
                let tick = Tick {
                    ms: app.loop_t,
                    dt_us: 16_000,
                };
                bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
            }
        }
        // A running .26 session starts on Player. Mount the actual Settings
        // panel over it instead of asking the root route to become a catalogue.
        bridge::show_page(&mut app.pages, AppArg::Player);
        settle(&mut app);
        app.pages.nav.next_style = Style::PlayerPanel {
            survives_failure: true,
        };
        app.pages
            .request(MachineId::Nav, NavOp::Present(AppArg::Ott));
        settle(&mut app);
        app.running = true;
        assert!(bridge::ott_catalog_surface(&app.pages).is_some());
        assert!(!app.pages.has_pending_navigation());
        let mut runtime =
            Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        runtime.remote = Remote::disabled_for_test("dev_home_boot_fixture", "fixture");
        runtime.loaded = true;
        runtime.page = "settings";
        runtime.reply = current_ui_owner(&app);
        let original = address(&runtime, &app).expect("current mounted Settings owner");
        boot_resume::arm(&mut runtime);
        assert!(runtime.view().startup_pending);
        let play_generation = runtime.play_generation;
        // Keep the existing worker boundary pending without spawning a thread or
        // making a platform request. Only its eventual acknowledgement is injected.
        let (send, cancelled) = runtime.home_integration.prepare(original).unwrap();
        assert!(!cancelled.load(Ordering::Acquire));

        handle_action(&mut runtime, &mut app, OttAction::Back).unwrap();

        // The .26 boot handler consumed Back, rather than the later Settings or
        // Player handlers. Home must already be cancelled before any OTT tick.
        assert!(!runtime.view().startup_pending);
        assert_eq!(runtime.page, "settings");
        assert_ne!(runtime.play_generation, play_generation);
        assert!(cancelled.load(Ordering::Acquire));
        assert_eq!(runtime.home_integration.epoch, original.epoch + 1);
        assert!(runtime.home_integration.flight.is_some());
        let revision = runtime.revision;
        let status = runtime.status.clone();
        assert!(address(&runtime, &app).is_some());
        assert!(!app.pages.has_pending_navigation());
        assert!(!LAUNCH_PENDING.load(Ordering::Acquire));

        send.try_send(HomeIntegrationLaunch::Requested).unwrap();
        tick(&mut runtime, &app);
        assert!(runtime.home_integration.flight.is_none());
        assert_eq!(
            runtime.revision,
            revision.wrapping_add(1),
            "draining the cancelled flight must publish row availability"
        );
        assert_eq!(
            runtime.status, status,
            "late ack must not replace boot Back status"
        );
        assert_eq!(runtime.page, "settings");
        assert!(!runtime.view().startup_pending);
        assert!(
            !app.pages.has_pending_navigation(),
            "late ack cannot navigate"
        );
        assert!(
            !LAUNCH_PENDING.load(Ordering::Acquire),
            "late ack cannot retry launch"
        );
    }

    #[cfg(feature = "hostsim")]
    fn mounted_settings() -> (App, Runtime) {
        use plx_machine::machine::{MachineId, NavOp, Tick};
        use plx_screens::registry::AppArg;
        use plx_ui::containers::modal::Style;
        fn settle(app: &mut App) {
            for _ in 0..24 {
                app.loop_t = app.loop_t.wrapping_add(16);
                let tick = Tick {
                    ms: app.loop_t,
                    dt_us: 16_000,
                };
                bridge::frame(&mut app.pages, &mut app.bridge, tick, Vec::new());
            }
        }
        let mut app = crate::app::run::lifecycle_regression_tests::app();
        bridge::show_page(&mut app.pages, AppArg::Player);
        settle(&mut app);
        app.pages.nav.next_style = Style::PlayerPanel {
            survives_failure: true,
        };
        app.pages
            .request(MachineId::Nav, NavOp::Present(AppArg::Ott));
        settle(&mut app);
        app.running = true;
        assert!(bridge::ott_catalog_surface(&app.pages).is_some());
        assert!(!app.pages.has_pending_navigation());
        let mut runtime =
            Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
        runtime.remote = Remote::disabled_for_test("dev_home_group_fixture", "fixture");
        runtime.loaded = true;
        runtime.page = "settings";
        runtime.reply = current_ui_owner(&app);
        assert!(address(&runtime, &app).is_some());
        (app, runtime)
    }

    #[cfg(feature = "hostsim")]
    #[test]
    fn actual_settings_group_navigation_retires_only_after_acceptance() {
        let _serial = plx_base::testlock::serial();
        for (group, accepted) in [("remote", true), ("not-a-settings-group", false)] {
            let (mut app, mut runtime) = mounted_settings();
            let original = address(&runtime, &app).unwrap();
            let (send, cancelled) = runtime.home_integration.prepare(original).unwrap();
            // A focus-only intent must leave the pending launch alone.
            before_action(
                &mut runtime,
                &OttAction::FocusAt {
                    context: String::new(),
                    id: "settings.open:remote".into(),
                },
            );
            assert!(!cancelled.load(Ordering::Acquire));
            let result = handle_action(
                &mut runtime,
                &mut app,
                OttAction::EditSetting(format!("settings.open:{group}")),
            );
            assert_eq!(result.is_ok(), accepted);
            assert_eq!(cancelled.load(Ordering::Acquire), accepted);
            assert_eq!(
                runtime.home_integration.epoch,
                original.epoch + if accepted { 1 } else { 0 }
            );
            assert!(runtime.home_integration.flight.is_some());
            assert_eq!(runtime.page, "settings");
            assert!(
                address(&runtime, &app).is_some(),
                "the same Settings owner remains mounted"
            );
            send.try_send(HomeIntegrationLaunch::Requested).unwrap();
            let current = address(&runtime, &app);
            assert_eq!(
                runtime.home_integration.poll(current),
                if accepted {
                    None
                } else {
                    Some(HomeIntegrationLaunch::Requested)
                }
            );
        }
    }

    #[cfg(feature = "hostsim")]
    #[test]
    fn actual_cancelled_completion_republishes_enabled_home_row_without_stale_status() {
        let _serial = plx_base::testlock::serial();
        for reply in [true, false] {
            let (app, mut runtime) = mounted_settings();
            let original = address(&runtime, &app).unwrap();
            let (send, _) = runtime.home_integration.prepare(original).unwrap();
            runtime.home_integration.retire();
            runtime.status = "Retained Settings status".into();
            let revision = runtime.revision;
            let pending_view = runtime.view();
            assert_eq!(pending_view.revision, revision);
            assert!(
                !pending_view
                    .rows
                    .iter()
                    .find(|row| row.id == KEY)
                    .unwrap()
                    .enabled
            );
            tick(&mut runtime, &app);
            assert_eq!(
                runtime.revision, revision,
                "pending work must not repaint every tick"
            );
            if reply {
                send.try_send(HomeIntegrationLaunch::Requested).unwrap();
            }
            drop(send); // Also cover worker disconnect without an acknowledgement.
            tick(&mut runtime, &app);
            assert!(runtime.home_integration.flight.is_none());
            assert_eq!(runtime.revision, revision.wrapping_add(1));
            assert_eq!(runtime.status, "Retained Settings status");
            let completed_view = runtime.view();
            assert_eq!(completed_view.revision, revision.wrapping_add(1));
            assert!(
                completed_view
                    .rows
                    .iter()
                    .find(|row| row.id == KEY)
                    .unwrap()
                    .enabled
            );
            tick(&mut runtime, &app);
            assert_eq!(
                runtime.revision,
                revision.wrapping_add(1),
                "completion publishes once"
            );
            assert!(!LAUNCH_PENDING.load(Ordering::Acquire));
        }
    }
}
