//! Standard OTT rows for on-device scenes. Row keys are process-local receipts,
//! never URLs or provider identities; captured handles cannot retarget a slot.
use super::*;
use plx_data::ott::multiview::{AudioPolicy, Layout, SlotId};

const KEY: &str = "localmv.open";
const PAGE: &str = "multiview-local";
const PAGE_SIZE: usize = 64;

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    pending: Option<Pending>,
}
struct Pending {
    serial: u64,
    until: u64,
    profile_epoch: u64,
    authority_epoch: u64,
    scene: Option<Fence>,
    return_page: &'static str,
    page: Page,
    title: String,
    choices: Vec<(String, String, Choice)>,
    selected: Option<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Fence {
    scene: String,
    layout: String,
    focus: String,
    audio: String,
    slots: Vec<(SlotId, String, String)>,
}
fn fence(scene: multiview::SceneStatus) -> Fence {
    Fence {
        scene: scene.scene,
        layout: scene.layout_revision,
        focus: scene.focus_revision,
        audio: scene.audio_revision,
        slots: scene
            .slots
            .into_iter()
            .map(|s| (s.slot, s.slot_generation, s.source_revision))
            .collect(),
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Root,
    Slots,
    Sources(SlotId, usize),
    WindowChannels(SlotId, Option<usize>),
    WindowMenu(SlotId),
}

impl Page {
    fn window(self) -> Option<SlotId> {
        match self {
            Self::WindowChannels(slot, _) | Self::WindowMenu(slot) => Some(slot),
            _ => None,
        }
    }
}

impl State {
    fn focus_parent(&mut self, from: Page) {
        let Some(pending) = self.pending.as_mut() else { return; };
        pending.selected = pending.choices.iter().position(|(_, _, choice)| match (from, choice) {
            (Page::Slots, Choice::Page(Page::Slots)) => true,
            (Page::Sources(slot, _), Choice::Page(Page::Sources(target, _))) => slot == *target,
            _ => false,
        });
    }
    fn back_destination(&mut self) -> (Option<Page>, &'static str) {
        let Some(pending) = self.pending.take() else {
            return (None, "menu");
        };
        let parent = match pending.page {
            Page::Root | Page::WindowChannels(..) | Page::WindowMenu(_) => None,
            Page::Slots => Some(Page::Root),
            Page::Sources(..) => Some(Page::Slots),
        };
        (parent, pending.return_page)
    }
}
#[derive(Clone, Copy)]
enum Choice {
    Page(Page),
    Layout(Layout),
    Source(SlotId, multiview_sources::SourceHandle),
    Focus(SlotId),
    Stop(SlotId),
    Pause(SlotId, bool),
    Step(SlotId, i64),
    Info(SlotId),
    KeepPlaying,
    Audio(AudioPolicy),
    Off,
}

pub(super) fn entry(runtime: &Runtime) -> OttRow {
    let mut row = ui_projection::row(
        KEY.into(),
        "Multiview".into(),
        "On-device picture-in-picture and multiple videos".into(),
        OttAction::EditSetting(KEY.into()),
        false,
    );
    row.enabled = local_multiview_enabled(runtime)
        && runtime.multiview_driver.shutdown_confirmed()
        && (runtime.multiview.layout().is_none() || runtime.multiview_local.active())
        && runtime.loaded
        && !runtime.saving
        && (runtime.multiview_local.active() || plx_media::local_multiview::available())
        && input_policy::navigation(runtime).is_ok()
        && matches!(
            runtime.track_selection_kiosk(),
            Ok(crate::app::ott_kiosk_policy::State::Off)
        );
    row
}

fn issue(
    runtime: &mut Runtime,
    app: &App,
    page: Page,
    return_page: &'static str,
) -> Result<(), &'static str> {
    if !entry(runtime).enabled || runtime.edit_session.is_some() || runtime.setup.is_some() {
        return Err("Multiview is unavailable");
    }
    if runtime.multiview_local.active() && !runtime.multiview_local.owns_ui(runtime, app) {
        return Err("Multiview source changed");
    }
    if page.window().is_some_and(|slot| runtime.multiview.active_slot() != Some(slot)) {
        return Err("Active window changed");
    }
    let mut choices = Vec::new();
    let mut selected = None;
    let title = match page {
        Page::Root => {
            for (name, layout) in [
                ("One video", Layout::Single),
                ("Picture in picture", Layout::PictureInPicture),
                ("Four videos", Layout::Grid4),
                ("Nine videos", Layout::Grid9),
                (
                    "Large video with eight surrounding videos",
                    Layout::Perimeter { satellites: 8 },
                ),
            ] {
                choices.push((name.into(), String::new(), Choice::Layout(layout)));
            }
            if runtime.multiview_local.active() {
                choices.push((
                    "Choose video / active slot".into(),
                    String::new(),
                    Choice::Page(Page::Slots),
                ));
                choices.push((
                    "Audio follows active video".into(),
                    String::new(),
                    Choice::Audio(AudioPolicy::FollowsFocus),
                ));
                choices.push((
                    "Mute all videos".into(),
                    String::new(),
                    Choice::Audio(AudioPolicy::Explicit(None)),
                ));
                choices.push((
                    "Close multiview".into(),
                    "Restore previous video".into(),
                    Choice::Off,
                ));
            }
            "Multiview".to_owned()
        }
        Page::Slots => {
            let scene = runtime
                .multiview
                .scene_status(runtime)
                .ok_or("Multiview source changed")?;
            for slot in scene.slots {
                let name = format!("Video {}", slot.slot.index() + 1);
                let detail = slot
                    .source
                    .as_ref()
                    .map(|s| s.label.clone())
                    .unwrap_or_else(|| "Empty".into());
                choices.push((
                    format!("{name}: choose source"),
                    detail,
                    Choice::Page(Page::Sources(slot.slot, 0)),
                ));
                choices.push((
                    format!("{name}: make active"),
                    String::new(),
                    Choice::Focus(slot.slot),
                ));
                choices.push((
                    format!("{name}: use audio"),
                    String::new(),
                    Choice::Audio(AudioPolicy::Explicit(Some(slot.slot))),
                ));
                if slot.status != "empty" || slot.source.is_some() {
                    choices.push((
                        format!("{name}: stop"),
                        "Keep the other videos playing".into(),
                        Choice::Stop(slot.slot),
                    ));
                }
            }
            choices.push((
                "Layouts and audio".into(),
                String::new(),
                Choice::Page(Page::Root),
            ));
            "Multiview videos".to_owned()
        }
        Page::Sources(slot, _) | Page::WindowChannels(slot, _) => {
            let mut driver = std::mem::take(&mut runtime.multiview_local);
            let result = driver.refresh_sources(runtime, app);
            runtime.multiview_local = driver;
            result.map_err(|_| "Multiview sources are unavailable")?;
            let sources: Vec<_> = runtime.multiview.source_options().into_iter()
                .filter(|source| source.support != multiview_sources::Support::Unavailable)
                .collect();
            let window = page.window().and_then(|_| runtime.multiview.scene_status(runtime))
                .and_then(|scene| scene.slots.into_iter().find(|entry| entry.slot == slot));
            let current_index = window.as_ref().and_then(|window| window.source.as_ref())
                .and_then(|current| sources.iter().position(|source| source.source_id == current.source_id));
            let offset = match page {
                Page::Sources(_, offset) | Page::WindowChannels(_, Some(offset)) => offset,
                _ => current_index.map_or(0, |index| index / PAGE_SIZE * PAGE_SIZE),
            };
            let adjacent = |offset| if page.window().is_some() { Page::WindowChannels(slot, Some(offset)) }
                else { Page::Sources(slot, offset) };
            for (index, source) in sources.iter().enumerate().skip(offset).take(PAGE_SIZE) {
                if let Some(handle) = runtime.multiview.source_handle(&source.source_id) {
                    let current = current_index == Some(index);
                    if current { selected = Some(choices.len()); }
                    let keep = current && window.as_ref().is_some_and(|window|
                        matches!(window.status, "playing" | "paused"));
                    choices.push((
                        source.label.clone(),
                        format!("{} · {}", source.provider, source.profile),
                        if keep { Choice::KeepPlaying } else { Choice::Source(slot, handle) },
                    ));
                }
            }
            if offset > 0 {
                choices.push((
                    "Previous sources".into(),
                    String::new(),
                    Choice::Page(adjacent(offset.saturating_sub(PAGE_SIZE))),
                ));
            }
            if offset.saturating_add(PAGE_SIZE) < sources.len() {
                choices.push((
                    "More sources".into(),
                    String::new(),
                    Choice::Page(adjacent(offset + PAGE_SIZE)),
                ));
            }
            if page.window().is_some() {
                "Channel list".into()
            } else {
                choices.push(("Back to videos".into(), String::new(), Choice::Page(Page::Slots)));
                format!("Choose source for video {}", slot.index() + 1)
            }
        }
        Page::WindowMenu(slot) => {
            let scene = runtime.multiview.scene_status(runtime).ok_or("Multiview source changed")?;
            let window = scene.slots.into_iter().find(|entry| entry.slot == slot)
                .ok_or("Window is no longer visible")?;
            choices.push(("Channel list".into(), String::new(), Choice::Page(Page::WindowChannels(slot, None))));
            if let Some(source) = window.source.and_then(|source| runtime.multiview.source_handle(&source.source_id)) {
                if matches!(window.status, "playing" | "paused") {
                    let pause = window.status == "playing";
                    let label = if pause { "Pause" } else { "Play" };
                    choices.push((label.into(), String::new(), Choice::Pause(slot, pause)));
                    choices.push(("Info".into(), String::new(), Choice::Info(slot)));
                }
                choices.extend([
                    ("Previous".into(), String::new(), Choice::Step(slot, -1)),
                    ("Next".into(), String::new(), Choice::Step(slot, 1)),
                    ("Restart".into(), String::new(), Choice::Source(slot, source)),
                    ("Use audio from this window".into(), String::new(), Choice::Audio(AudioPolicy::Explicit(Some(slot)))),
                    ("Stop this window".into(), String::new(), Choice::Stop(slot)),
                ]);
            }
            choices.push(("Close multiview".into(), "Restore previous video".into(), Choice::Off));
            "Menu".into()
        }
    };
    let serial = runtime
        .multiview_menu
        .serial
        .checked_add(1)
        .ok_or("Multiview menu exhausted")?;
    runtime.multiview_menu.serial = serial;
    runtime.multiview_menu.pending = Some(Pending {
        serial,
        until: runtime.now_ms().saturating_add(30_000),
        profile_epoch: runtime.multiview_local_profile_epoch,
        authority_epoch: runtime.multiview_authority_epoch,
        scene: runtime.multiview.scene_status(runtime).map(fence),
        return_page,
        page,
        title,
        choices,
        selected,
    });
    runtime.page = PAGE;
    runtime.status.clear();
    runtime.changed();
    Ok(())
}

pub(super) fn can_open_window(runtime: &Runtime, app: &App) -> bool {
    use plx_machine::machine::InputOwner;
    runtime.page != PAGE
        && entry(runtime).enabled
        && runtime.multiview_local.owns_ui(runtime, app)
        && !app.pages.has_pending_navigation()
        && !app.pages.has_pending_surface_navigation()
        && !app.bridge.is_controlled_replay()
        && !matches!(app.rec, super::super::recorder::Recplay::Replaying(_))
        && app.pages.nav.top_page().is_some_and(|page| {
            matches!(page.arg, AppArg::Player)
                && app.pages.nav.input_owner() == Some(InputOwner::Entry(page.id))
                && page.inst.as_ref().is_some_and(|body| !body.staged)
        })
}

pub(super) fn open_window(
    runtime: &mut Runtime,
    app: &mut App,
    slot: SlotId,
    menu: bool,
) -> Result<(), &'static str> {
    if !can_open_window(runtime, app) {
        return Err("Active window changed");
    }
    let page = if menu { Page::WindowMenu(slot) } else { Page::WindowChannels(slot, None) };
    issue(runtime, app, page, runtime.page)?;
    player_browse::present(runtime, app, None, PAGE);
    Ok(())
}

fn dismiss_window_panel(app: &mut App) {
    if let Some(entry) = bridge::ott_catalog_surface(&app.pages) {
        app.pages.request(MachineId::Nav, plx_machine::machine::NavOp::Dismiss(entry));
    }
}

pub(super) fn view(runtime: &Runtime) -> Option<OttView> {
    if runtime.page != PAGE {
        return None;
    }
    let pending = runtime.multiview_menu.pending.as_ref()?;
    let enabled = current(runtime, pending);
    let mut rows: Vec<_> = pending
        .choices
        .iter()
        .enumerate()
        .map(|(index, (label, detail, _))| {
            let token = format!("localmv.select:{}:{index}", pending.serial);
            let mut row = ui_projection::row(
                token.clone(),
                label.clone(),
                detail.clone(),
                OttAction::EditSetting(token),
                false,
            );
            row.enabled = enabled;
            row
        })
        .collect();
    rows.push(ui_projection::row(
        "localmv.back".into(),
        "Back".into(),
        String::new(),
        OttAction::Back,
        false,
    ));
    Some(OttView {
        page_id: format!("{PAGE}:{}", pending.serial),
        revision: runtime.revision,
        title: pending.title.clone(),
        status: if enabled {
            runtime.status.clone()
        } else {
            "Selection changed; reopen Multiview".into()
        },
        rows,
        selected_row: pending.selected.filter(|index| *index < pending.choices.len())
            .map(|index| format!("localmv.select:{}:{index}", pending.serial)),
        ..OttView::default()
    })
}

fn current(runtime: &Runtime, pending: &Pending) -> bool {
    runtime.page == PAGE
        && runtime.now_ms() < pending.until
        && runtime.multiview_local_profile_epoch == pending.profile_epoch
        && runtime.multiview_authority_epoch == pending.authority_epoch
        && if let Some(slot) = pending.page.window() {
            runtime.multiview.active_slot() == Some(slot)
                && runtime.multiview.scene_status(runtime).map(fence)
                    .zip(pending.scene.as_ref())
                    .is_some_and(|(current, offered)| same_window(&current, offered, slot))
        } else {
            runtime.multiview.scene_status(runtime).map(fence) == pending.scene
        }
}

fn same_window(current: &Fence, offered: &Fence, slot: SlotId) -> bool {
    current.scene == offered.scene
        && current.layout == offered.layout
        && offered.slots.iter().find(|entry| entry.0 == slot)
            .is_some_and(|entry| current.slots.contains(entry))
}

pub(super) fn action(runtime: &mut Runtime, app: &mut App, action: &OttAction) -> bool {
    if runtime.page == PAGE && matches!(action, OttAction::Back) {
        let from = runtime.multiview_menu.pending.as_ref().map(|pending| pending.page);
        let (parent, return_page) = runtime.multiview_menu.back_destination();
        runtime.page = return_page;
        if from.and_then(Page::window).is_some() {
            dismiss_window_panel(app);
            runtime.changed();
            return true;
        }
        if let Some(parent) = parent {
            // Reissue the parent through the ordinary ownership/policy gate.
            // A retired source can still leave the menu without reviving it.
            if issue(runtime, app, parent, return_page).is_ok() {
                if let Some(from) = from { runtime.multiview_menu.focus_parent(from); }
                return true;
            }
        }
        if return_page == "settings" { runtime.settings_focus = Some(KEY); }
        runtime.changed();
        return true;
    }
    let OttAction::EditSetting(key) = action else {
        return false;
    };
    if key != KEY && !key.starts_with("localmv.select:") {
        return false;
    }
    let result = if key == KEY {
        let return_page = runtime
            .multiview_menu
            .pending
            .as_ref()
            .filter(|_| runtime.page == PAGE)
            .map_or(runtime.page, |p| p.return_page);
        issue(runtime, app, Page::Root, return_page)
    } else {
        select(runtime, app, key)
    };
    if let Err(error) = result {
        runtime.status = error.into();
        runtime.changed();
    }
    true
}

fn select(runtime: &mut Runtime, app: &mut App, key: &str) -> Result<(), &'static str> {
    let pending = runtime
        .multiview_menu
        .pending
        .as_ref()
        .ok_or("Multiview selection expired")?;
    let (serial, index) = key
        .strip_prefix("localmv.select:")
        .and_then(|s| s.split_once(':'))
        .and_then(|(s, i)| Some((s.parse::<u64>().ok()?, i.parse::<usize>().ok()?)))
        .ok_or("Multiview selection expired")?;
    if serial != pending.serial || !current(runtime, pending) || !entry(runtime).enabled {
        return Err("Multiview selection changed");
    }
    let choice = pending
        .choices
        .get(index)
        .ok_or("Multiview selection changed")?
        .2;
    let return_page = pending.return_page;
    let window = pending.page.window();
    if window.is_some() && bridge::ott_catalog_surface(&app.pages).is_none() {
        return Err("Window menu is no longer open");
    }
    // One-shot issuance: even a backend refusal cannot be replayed against a new slot.
    if runtime.multiview_effects.len() > 56 {
        return Err("Multiview control is busy");
    }
    runtime.multiview_menu.pending = None;
    runtime.page = return_page;
    if let Choice::Page(page) = choice {
        return issue(runtime, app, page, return_page);
    }
    if window.is_some() {
        dismiss_window_panel(app);
    }
    let mut driver = std::mem::take(&mut runtime.multiview_local);
    let result = match choice {
        Choice::Layout(layout) => driver.set_layout(runtime, app, layout),
        Choice::Source(slot, handle) => driver.play_source(runtime, app, slot, handle),
        Choice::Off => driver.close_restore_previous(runtime, app),
        Choice::Focus(slot) | Choice::Audio(AudioPolicy::Explicit(Some(slot))) => {
            if !driver.owns_ui(runtime, app) {
                Err(multiview::Error::NotReady)
            } else {
                let result = if matches!(choice, Choice::Focus(_)) {
                    runtime.multiview.focus(slot)
                } else {
                    runtime
                        .multiview
                        .set_audio_policy(AudioPolicy::Explicit(Some(slot)))
                };
                result.map(|t| runtime.multiview_effects.extend(t.effects))
            }
        }
        Choice::Stop(slot) => {
            if !driver.owns_ui(runtime, app) {
                Err(multiview::Error::NotReady)
            } else {
                driver.stop_slot(runtime, slot)
            }
        }
        Choice::Pause(slot, paused) => {
            if runtime.multiview.active_slot() != Some(slot) {
                Err(multiview::Error::NotReady)
            } else {
                driver.set_active_paused(runtime, app, paused).map_err(|_| multiview::Error::NotReady)
            }
        }
        Choice::Step(slot, offset) => {
            if runtime.multiview.active_slot() != Some(slot) {
                Err(multiview::Error::NotReady)
            } else {
                driver.step_active(runtime, app, offset)
            }
        }
        Choice::Info(slot) => {
            if let Some(player) = bridge::player_mut(&mut app.pages) {
                player.reveal_multiview_footer(slot.index() as u8, clock::now());
                Ok(())
            } else {
                Err(multiview::Error::NotReady)
            }
        }
        Choice::KeepPlaying => Ok(()),
        Choice::Audio(policy) => {
            if !driver.owns_ui(runtime, app) {
                Err(multiview::Error::NotReady)
            } else {
                runtime
                    .multiview
                    .set_audio_policy(policy)
                    .map(|t| runtime.multiview_effects.extend(t.effects))
            }
        }
        Choice::Page(_) => unreachable!(),
    };
    runtime.multiview_local = driver;
    runtime.page = return_page;
    result.map_err(|_| "Multiview request could not be accepted")?;
    runtime.changed();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "hostsim")]
    #[test]
    fn window_panel_returns_to_the_same_grid_and_selects_only_its_bound_slot() {
        use plx_appkit::multiview_chrome::Event;
        use plx_machine::machine::Tick;
        let _serial = plx_base::testlock::serial();
        let mut app = crate::app::run::lifecycle_regression_tests::app();
        app.running = true;
        bridge::show_page(&mut app.pages, AppArg::Player);
        let settle = |app: &mut App| {
            for _ in 0..24 {
                app.loop_t = app.loop_t.wrapping_add(25);
                let tick = Tick { ms: app.loop_t, dt_us: 25_000 };
                bridge::frame(&mut app.pages, &mut app.bridge, tick, vec![]);
            }
        };
        settle(&mut app);
        let player = app.pages.top_page().unwrap();
        let mut runtime = Box::new(Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test()));
        runtime.clock_sample = Some(plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap());
        runtime.remote = Remote::disabled_for_test("dev_window_menu", "fixture");
        runtime.loaded = true;
        runtime.state_load = StateLoad::Idle;
        runtime.entropy = Some([9; 16]);
        runtime.resume_source_work();
        runtime.page = "channels";
        runtime.provider = ProviderKind::M3u;
        let profile = ProviderProfile {
            id: "window-fixture".into(), name: "Window fixture".into(),
            kind: ProviderKind::M3u, source: "https://fixture.invalid/fixture.m3u".into(),
            source_alias: "Synthetic".into(), enabled: true,
        };
        runtime.saved.profiles.active_profile = Some(profile.id.clone());
        runtime.saved.profiles.profiles.push(profile);
        for (id, title) in [("existing", "Existing picture"), ("choice", "Replacement choice")] {
            runtime.catalog.entries.push(CatalogEntry {
                id: id.into(), title: title.into(), category: String::new(),
                description: String::new(), poster: String::new(), synopsis: None,
                protection: Some(false), media: MediaLocator::Http {
                    url: format!("https://fixture.invalid/{id}.m3u8"),
                }, duration_ms: None, provider_id: None, provider_id_field: None, live: None,
            });
        }
        // The normal coordinator admits the grid. The host UI fixture does
        // not load FFmpeg or execute decoder effects.
        let mut coordinator = std::mem::take(&mut runtime.multiview);
        let layout = coordinator.activate_local(&runtime, &app, Layout::Grid4).unwrap();
        runtime.multiview = coordinator;
        runtime.multiview_effects.extend(layout.effects);
        runtime.multiview_local = multiview_local::Driver::active_layout_for_test(Layout::Grid4);
        let sibling = runtime.multiview.source_rows()[0].handle;
        runtime.multiview.bind_source(SlotId::SLOT_0, sibling).unwrap();
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let initial = runtime.multiview.scene_status(&runtime).unwrap();
        for accept in [false, true] {
            super::super::multiview_ui::publish(&mut runtime, &mut app);
            let revision = app.bridge.multiview_offer().unwrap().state().revision;
            app.ott = Some(runtime);
            super::super::multiview_ui::player_action(&mut app, Event::OpenWindowChannels { revision, slot: 1 });
            runtime = app.ott.take().unwrap();
            assert_eq!(runtime.multiview.active_slot(), Some(SlotId::SLOT_1));
            settle(&mut app);
            assert!(bridge::ott_catalog_surface(&app.pages).is_some());
            let list = view(&runtime).unwrap();
            let choice = list.rows.iter().find(|row| row.title == "Replacement choice").unwrap().action.clone();
            assert_eq!(list.title, "Channel list");
            assert!(action(&mut runtime, &mut app, if accept { &choice } else { &OttAction::Back }));
            settle(&mut app);
            assert_eq!(app.pages.top_page(), Some(player));
            assert!(bridge::ott_catalog_surface(&app.pages).is_none());
            let now = runtime.multiview.scene_status(&runtime).unwrap();
            assert_eq!(now.slots[0], initial.slots[0]);
            assert_eq!(now.slots[1].source.as_ref().map(|source| source.label.as_str()),
                accept.then_some("Replacement choice"));
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
            assert!(runtime.multiview_menu.pending.is_none());
        }
    }

    fn pending(page: Page) -> Pending {
        Pending {
            serial: 7,
            until: u64::MAX,
            profile_epoch: 0,
            authority_epoch: 0,
            scene: None,
            return_page: "settings",
            page,
            title: "Fixture".into(),
            choices: vec![("Stop".into(), String::new(), Choice::Stop(SlotId::SLOT_0))],
            selected: None,
        }
    }

    #[test]
    fn back_pops_one_level_and_retires_the_old_selection_receipt() {
        for (page, parent) in [
            (Page::Root, None),
            (Page::Slots, Some(Page::Root)),
            (Page::Sources(SlotId::SLOT_0, 0), Some(Page::Slots)),
            (Page::Sources(SlotId::new(8).unwrap(), PAGE_SIZE), Some(Page::Slots)),
            (Page::WindowChannels(SlotId::SLOT_1, Some(PAGE_SIZE)), None),
            (Page::WindowMenu(SlotId::SLOT_1), None),
        ] {
            let mut state = State { serial: 7, pending: Some(pending(page)) };
            assert_eq!(state.back_destination(), (parent, "settings"));
            assert!(state.pending.is_none());
            assert_eq!(state.serial, 7, "Back must not reuse old action serials");
            assert_eq!(state.back_destination(), (None, "menu"));
        }
    }

    #[cfg(feature = "hostsim")]
    #[test]
    fn parent_focus_returns_to_the_same_window_or_video_controls() {
        let _serial = plx_base::testlock::serial();
        let app = crate::app::run::lifecycle_regression_tests::app();
        let ninth = SlotId::new(8).unwrap();
        for (from, expected) in [(Page::Sources(ninth, PAGE_SIZE), 1), (Page::Slots, 2)] {
            let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
            runtime.clock_sample = Some(plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap());
            runtime.page = PAGE;
            let mut offered = pending(Page::Root);
            offered.profile_epoch = runtime.multiview_local_profile_epoch;
            offered.authority_epoch = runtime.multiview_authority_epoch;
            offered.choices = vec![
                ("First".into(), String::new(), Choice::Page(Page::Sources(SlotId::SLOT_0, 0))),
                ("Ninth".into(), String::new(), Choice::Page(Page::Sources(ninth, 0))),
                ("Videos".into(), String::new(), Choice::Page(Page::Slots)),
            ];
            runtime.multiview_menu.pending = Some(offered);
            runtime.multiview_menu.focus_parent(from);
            let projected = view(&runtime).unwrap();
            assert_eq!(projected.selected_row.as_deref(), Some(projected.rows[expected].id.as_str()));
            assert!(runtime.multiview_effects.is_empty());
        }
    }

    #[cfg(feature = "hostsim")]
    #[test]
    fn stale_stop_rows_cannot_queue_transport_or_change_the_ordinary_source() {
        let _serial = plx_base::testlock::serial();
        let mut app = crate::app::run::lifecycle_regression_tests::app();
        for stale in ["expired", "profile", "authority", "scene", "serial"] {
            let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
            runtime.clock_sample = Some(plx_base::task::time::ClockSample::capture_main(app.adapters.player.mt()).unwrap());
            runtime.page = PAGE;
            let mut offered = pending(Page::Slots);
            offered.profile_epoch = runtime.multiview_local_profile_epoch;
            offered.authority_epoch = runtime.multiview_authority_epoch;
            match stale {
                "expired" => offered.until = 0,
                "profile" => offered.profile_epoch = runtime.multiview_local_profile_epoch.wrapping_add(1),
                "authority" => offered.authority_epoch = runtime.multiview_authority_epoch.wrapping_add(1),
                "scene" => offered.scene = Some(fence(scene())),
                _ => {},
            }
            runtime.multiview_menu.pending = Some(offered);
            let saved = serde_json::to_value(&runtime.saved).unwrap();
            let key = if stale == "serial" { "localmv.select:6:0" } else { "localmv.select:7:0" };
            assert!(select(&mut runtime, &mut app, key).is_err(), "{stale}");
            assert!(runtime.multiview_effects.is_empty(), "{stale}");
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved, "{stale}");
            assert_eq!(runtime.page, PAGE, "{stale}");
            assert!(runtime.multiview_menu.pending.is_some(), "{stale}");
        }
    }

    #[cfg(feature = "hostsim")]
    #[test]
    fn back_leaves_a_retired_parent_without_starting_or_restoring_video() {
        let _serial = plx_base::testlock::serial();
        let mut app = crate::app::run::lifecycle_regression_tests::app();
        for page in [Page::Slots, Page::Sources(SlotId::SLOT_0, PAGE_SIZE)] {
            let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
            runtime.page = PAGE;
            runtime.multiview_menu = State { serial: 7, pending: Some(pending(page)) };
            let saved = serde_json::to_value(&runtime.saved).unwrap();
            assert!(action(&mut runtime, &mut app, &OttAction::Back));
            assert_eq!(runtime.page, "settings");
            assert!(runtime.multiview_menu.pending.is_none());
            assert_eq!(runtime.multiview_menu.serial, 7);
            assert!(runtime.multiview_effects.is_empty());
            assert!(runtime.multiview.layout().is_none());
            assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        }
    }

    fn scene() -> multiview::SceneStatus {
        multiview::SceneStatus {
            runtime: "runtime".into(),
            scene: "scene".into(),
            layout_revision: "layout1".into(),
            focus_revision: "focus1".into(),
            audio_revision: "audio1".into(),
            admitted_pipeline_slots: 0,
            slots: vec![multiview::SlotView {
                slot: SlotId::new(0).unwrap(),
                slot_generation: "generation1".into(),
                source_revision: "source1".into(),
                source: None,
                status: "starting",
                error: None,
            }],
        }
    }
    #[test]
    fn source_picker_receipt_rejects_replaced_slot_and_changed_layout() {
        let original = fence(scene());
        let mut replaced = scene();
        replaced.slots[0].slot_generation = "generation2".into();
        assert_ne!(fence(replaced), original);
        let mut rebound = scene();
        rebound.slots[0].source_revision = "source2".into();
        assert_ne!(fence(rebound), original);
        let mut relayout = scene();
        relayout.layout_revision = "layout2".into();
        assert_ne!(fence(relayout), original);
    }
    #[test]
    fn window_picker_survives_sibling_updates_and_requires_its_own_generation() {
        let mut offered_scene = scene();
        let mut sibling = offered_scene.slots[0].clone();
        sibling.slot = SlotId::SLOT_1;
        offered_scene.slots.push(sibling);
        let offered = fence(offered_scene.clone());
        offered_scene.slots[1].slot_generation = "sibling-replaced".into();
        offered_scene.audio_revision = "audio-settled".into();
        assert!(same_window(&fence(offered_scene.clone()), &offered, SlotId::SLOT_0));
        offered_scene.slots[0].source_revision = "target-replaced".into();
        assert!(!same_window(&fence(offered_scene.clone()), &offered, SlotId::SLOT_0));
        offered_scene.slots.remove(0);
        assert!(!same_window(&fence(offered_scene), &offered, SlotId::SLOT_0));
    }
    #[test]
    fn asynchronous_playing_status_does_not_expire_source_picker() {
        let original = fence(scene());
        let mut playing = scene();
        playing.slots[0].status = "playing";
        playing.admitted_pipeline_slots = 1;
        assert_eq!(fence(playing), original);
    }
    #[test]
    fn audio_or_focus_changes_invalidate_target_bound_actions() {
        let original = fence(scene());
        let mut moved = scene();
        moved.focus_revision = "focus2".into();
        assert_ne!(fence(moved), original);
        let mut changed_audio = scene();
        changed_audio.audio_revision = "audio2".into();
        assert_ne!(fence(changed_audio), original);
    }
}
