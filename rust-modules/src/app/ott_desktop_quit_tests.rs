//! Physical Q at the real SDL ingress. No direct Quit action or patched classifier.
//! Disk durability and process reopen are a separate host-fixture acceptance step.
use super::*;
use plx_machine::machine::{InputKind, Key, TextEdit, UiMode};

const Q: u32 = b'q' as u32;

struct Paused(bool);
impl Drop for Paused {
    fn drop(&mut self) {
        plx_media::player::TX.commit_paused(self.0);
    }
}

fn playing(mode: UiMode) -> App {
    let mut app = fixture();
    let r = app.ott.as_mut().unwrap();
    r.write_permit.revoke();
    r.catalog_done.store(false, Ordering::Release);
    r.metadata_done.store(false, Ordering::Release);
    r.plex_refresh_pending = true;
    r.continuity.stop(r.generation);
    // Publish through the same accepted-settings path as a durable write.
    // Merely replacing Saved leaves the Bridge in its previous UI mode.
    let saved = r.interface_mode_candidate(mode);
    r.save_navigation = SaveNavigation::Preserve;
    r.save_playback_epoch = r.playback_epoch;
    // Keep the independent periodic checkpoint idle during settings publication.
    r.last_checkpoint_ms = r.now_ms();
    r.tx.send(Job::Saved(Ok(saved), None, false, json!({"status":"ok"})))
        .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
    runtime_tick(&mut app);
    let r = app.ott.as_mut().unwrap();
    assert!(!r.saving, "the accepted settings result must settle");
    assert_eq!(r.interface_mode(), mode);
    // Retain the parent fixture's persistence hold for non-editor Q cases.
    r.saving = true;
    settle_pages(&mut app);
    assert_eq!(app.bridge.ui_mode(), mode);
    app.inputs.clear();
    app
}

fn catalog(mode: UiMode, page: &'static str) -> App {
    let mut app = playing(mode);
    let r = app.ott.as_mut().unwrap();
    r.page = page;
    r.changed();
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    runtime_tick(&mut app);
    settle_pages(&mut app);
    app.inputs.clear();
    assert!(!app.pages.surface_up());
    assert!(!app.pages.has_pending_navigation());
    app
}

fn dispatch_queued(app: &mut App) {
    let input = std::mem::take(&mut app.inputs);
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, input);
}

fn open_editor(app: &mut App, key: &str) -> EditorSession {
    let mut r = app.ott.take().unwrap();
    // Opening a real editor requires idle, verified settings and a PIN nonce.
    // The write permit stays revoked and the resolver/metadata slots stay held.
    r.saving = false;
    r.entropy = Some([1; 16]);
    r.reply = app.pages.top_page();
    handle_action(&mut r, app, OttAction::EditSetting(key.into()))
        .expect("fixture editor must be admitted without a writer");
    let session = r.edit_session.clone().expect("real editor session");
    app.ott = Some(r);
    settle_pages(app);
    assert!(bridge::text_owns_input(&app.pages));
    app.inputs.clear();
    session
}

fn text_ascii(value: u8) -> [u8; 128] {
    let mut bytes = [0; 128];
    bytes[..4].copy_from_slice(&0x303_u32.to_ne_bytes());
    bytes[8..12].copy_from_slice(&1_u32.to_ne_bytes());
    bytes[12] = value; // Stock SDL2's inline text begins at +12 on hostsim.
    bytes
}


fn screen_key(app: &mut App, key: Key) {
    let tick = next_tick(app);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, bridge::script_key(key, tick));
}

fn editor_submission(app: &mut App) -> (u64, String, String) {
    // Inspect the real screen's Save effect before the runtime can start a
    // writer. This observes the private draft, not merely the input queue.
    let _ = app.bridge.take_ott_reqs();
    screen_key(app, Key::Down);
    screen_key(app, Key::Ok);
    let mut submissions: Vec<_> = app.bridge.take_ott_reqs().into_iter().filter_map(|output| {
        match output {
            plx_screens::ott::OttOut::Action {
                action: OttAction::SubmitSetting { session, key, value }, ..
            } => Some((session, key, value)),
            _ => None,
        }
    }).collect();
    assert_eq!(submissions.len(), 1, "one Save must emit one actual draft");
    submissions.remove(0)
}

fn search_editing(app: &App) -> bool {
    app.pages.top_screen().expect("mounted Search").as_any().unwrap()
        .downcast_ref::<plx_screens::search::SearchScreen>().expect("real Search screen")
        .is_editing()
}

#[test]
fn mac_physical_q_quits_ordinary_routes_in_both_modes_without_back_or_teardown() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused(plx_media::player::transport::paused());
    let _reset = Reset;
    for mode in [UiMode::Classic, UiMode::Modern] {
        for hud in [false, true] {
            let mut app = playing(mode);
            if hud { reveal_hud(&mut app); }
            let owner = app.pages.top_page();
            let identity = plx_media::route::playback_identity(&app.player.session);
            let position = plx_media::player::playpos_ns();
            let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
            ingest(&mut app, physical(Q, 0, true, false));
            assert!(!app.running, "fresh physical Q must request ordinary loop exit");
            assert_eq!(app.pages.top_page(), owner, "Q is not Back navigation");
            assert_eq!(plx_media::route::playback_identity(&app.player.session), identity);
            assert_eq!(plx_media::player::playpos_ns(), position, "shutdown still owns checkpoint-before-teardown");
            assert!(app.inputs.is_empty(), "Q must not leave a second Back queued");
            assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), before);
        }
        for page in ["providers", "channels"] {
            let mut app = catalog(mode, page);
            let owner = app.pages.top_page();
            ingest(&mut app, physical(Q, 0, true, false));
            assert!(!app.running, "ordinary catalogue routes must support Q");
            assert_eq!(app.pages.top_page(), owner);
            assert!(app.inputs.is_empty());
        }
    }
    let mut app = playing(UiMode::Modern);
    app.ott = None;
    bridge::show_page(&mut app.pages, AppArg::Home);
    settle_pages(&mut app);
    app.inputs.clear();
    assert!(!app.pages.has_pending_navigation());
    ingest(&mut app, physical(Q, 0, true, false));
    assert!(!app.running, "native root without an OTT runtime must support Q too");
}

#[test]
fn mac_physical_q_does_not_promote_modified_synthetic_repeat_or_release_to_exit() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused(plx_media::player::transport::paused());
    let _reset = Reset;
    for modifier in [0x0001, 0x0002, 0x0040, 0x0080, 0x0100, 0x0200, 0x0400, 0x0800] {
        let mut app = playing(UiMode::Modern);
        ingest(&mut app, physical(Q, modifier, true, false));
        assert!(app.running, "modified Q does not gain a new quit binding");
    }
    for (bytes, repeat) in [(crate::app::events::encode_key(Q, 0, true), false),
        (physical(Q, 0, true, true), true), (physical(Q, 0, false, false), false)] {
        let mut app = playing(UiMode::Modern);
        ingest(&mut app, bytes);
        assert!(app.running, "synthetic Q, repeat and release are not fresh physical quit gestures");
        if repeat {
            assert!(app.inputs.is_empty(), "an unmatched repeat must not become a fresh Back either");
            assert!(!app.desktop_quit_held, "a repeat alone does not own a future release");
        }
    }
    assert_eq!(plx_ui::consts::classify(Q, 0), plx_ui::consts::Key::Back,
        "TV/remote Q remains the shared Back spelling");
}


#[test]
fn mac_physical_q_ignores_only_caps_and_num_lock_for_plain_quit() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused(plx_media::player::transport::paused());
    let _reset = Reset;
    for mode in [UiMode::Classic, UiMode::Modern] {
        for locks in [0x1000, 0x2000, 0x3000] {
            let mut app = playing(mode);
            ingest(&mut app, physical(Q, locks, true, false));
            assert!(!app.running, "lock state must not disable the plain Q gesture");
            assert!(app.inputs.is_empty());
        }
    }
}

#[test]
fn mac_physical_q_and_shift_q_reach_the_actual_editor_draft_exactly_once() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused(plx_media::player::transport::paused());
    let _reset = Reset;
    for mode in [UiMode::Classic, UiMode::Modern] {
        for (modifiers, letter, expected) in [(0, b'q', "q"), (0x0001, b'Q', "Q")] {
            let mut app = catalog(mode, "profiles");
            let session = open_editor(&mut app, "profile.add");
            let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
            ingest(&mut app, physical(Q, modifiers, true, false));
            assert!(app.running && app.inputs.is_empty(), "Q must not become editor Back");
            assert!(app.desktop_quit_held);
            ingest(&mut app, text_ascii(letter));
            assert_eq!(app.inputs.iter().filter(|event| matches!(&event.kind,
                InputKind::Text(TextEdit::Commit(text)) if text.as_ref() == expected)).count(), 1);
            ingest(&mut app, physical(Q, modifiers, false, false));
            assert!(!app.desktop_quit_held);
            dispatch_queued(&mut app);
            assert!(app.running && bridge::text_owns_input(&app.pages));
            let (id, key, value) = editor_submission(&mut app);
            assert_eq!(id, session.id);
            assert_eq!(key, session.key);
            assert_eq!(value, expected, "real Save must contain exactly the SDL text insertion");
            assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), before,
                "intercepting the screen effect must not publish or persist the draft");
        }
    }
}

#[test]
fn mac_physical_q_and_shift_q_keep_real_search_open_and_insert_once() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused(plx_media::player::transport::paused());
    let _reset = Reset;
    for mode in [UiMode::Classic, UiMode::Modern] {
        for (modifiers, letter, expected) in [(0, b'q', "q"), (0x0001, b'Q', "Q")] {
            let mut app = playing(mode);
            bridge::show_page(&mut app.pages, AppArg::Search);
            settle_pages(&mut app);
            app.inputs.clear();
            assert!(bridge::search_owns_input(&app.pages));
            assert_eq!(app.bridge.views().search.query(), "");
            screen_key(&mut app, Key::Ok);
            assert!(search_editing(&app), "Search field must actually own the keyboard");
            let owner = app.pages.top_page();
            ingest(&mut app, physical(Q, modifiers, true, false));
            assert!(app.running && app.inputs.is_empty());
            ingest(&mut app, text_ascii(letter));
            ingest(&mut app, physical(Q, modifiers, false, false));
            dispatch_queued(&mut app);
            settle_pages(&mut app);
            assert!(app.running && search_editing(&app));
            assert_eq!(app.pages.top_page(), owner, "Q must not leave Search");
            assert_eq!(app.bridge.views().search.query(), expected,
                "actual Search publication must contain one insertion");
            assert!(app.bridge.take_search_reqs().is_empty(), "Q must not request Search Back");
            assert!(!app.desktop_quit_held);
        }
    }
}

#[test]
fn mac_physical_q_preserves_editor_text_and_retires_its_release_after_owner_loss() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused(plx_media::player::transport::paused());
    let _reset = Reset;
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut app = catalog(mode, "profiles");
        let session = open_editor(&mut app, "profile.add");
        let saved = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        ingest(&mut app, physical(Q, 0, true, false));
        assert!(app.running);
        assert!(app.inputs.is_empty(), "the Q key must not be queued as editor Back");
        ingest(&mut app, text_ascii(b'q'));
        assert!(app.inputs.iter().any(|event| matches!(&event.kind,
            InputKind::Text(TextEdit::Commit(text)) if text.as_ref() == "q")),
            "the separate SDL text event must still reach the real editor");
        dispatch_queued(&mut app);
        assert!(bridge::text_owns_input(&app.pages));
        assert!(app.ott.as_ref().unwrap().edit_session.as_ref() == Some(&session));
        assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), saved);
        assert!(app.desktop_quit_held);
        ingest(&mut app, crate::app::events::encode_key(Q, 0, false));
        assert!(app.desktop_quit_held, "a marked remote release cannot retire a physical press");
        // The remote edge belongs to the old routing path; do not dispatch it
        // into this editor while testing the separate physical ownership latch.
        app.inputs.clear();
        // Cancel through the real screen: the emitted action retires runtime
        // authority only after the screen has closed its private draft.
        screen_key(&mut app, Key::Back);
        assert!(!bridge::text_owns_input(&app.pages));
        assert!(!app.pages.input.keyboard);
        assert!(app.pages.input.keyboard_owner.is_none());
        // Restore the inherited hold so this ownership test cannot schedule
        // an unrelated checkpoint or browse-focus write. Cancel remains allowed.
        app.ott.as_mut().unwrap().saving = true;
        runtime_tick(&mut app);
        assert!(app.ott.as_ref().unwrap().edit_session.is_none());
        assert!(app.desktop_quit_held, "screen cancellation must not retire the physical Q latch");
        settle_pages(&mut app);
        app.inputs.clear();
        // Modifier and guard may change before release. No release may land on
        // the replacement owner; a subsequent genuinely fresh gesture must work.
        app.ott_standby = true;
        ingest(&mut app, physical(Q, 0x0400, false, false));
        app.ott_standby = false;
        assert!(app.running && app.inputs.is_empty());
        assert!(!app.desktop_quit_held, "physical release retires across the standby guard");
        ingest(&mut app, physical(Q, 0, true, false));
        assert!(!app.running, "old editor ownership must not swallow a later fresh Q");
    }
}

#[test]
fn mac_physical_q_preserves_modal_pending_navigation_and_queued_input_ownership() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused(plx_media::player::transport::paused());
    let _reset = Reset;
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut app = playing(mode);
        bridge::open_settings(&mut app.pages);
        settle_pages(&mut app);
        assert!(app.pages.surface_up(), "actual Settings modal owns input");
        ingest(&mut app, physical(Q, 0, true, false));
        assert!(app.running);
        assert!(!app.inputs.is_empty(), "the existing modal Back route must retain Q");
        dispatch_queued(&mut app);
        bridge::dismiss_surfaces_now(&mut app.pages);
        settle_pages(&mut app);
        app.inputs.clear();
        ingest(&mut app, physical(Q, 0, true, true));
        assert!(app.running, "a repeat after modal dismissal must never become Quit");
        app.inputs.clear();
        ingest(&mut app, physical(Q, 0, false, false));
        assert!(app.running);

        let mut app = playing(mode);
        bridge::show_page(&mut app.pages, AppArg::Ott);
        assert!(app.pages.has_pending_navigation(), "fixture must retain a pending page change");
        ingest(&mut app, physical(Q, 0, true, false));
        assert!(app.running);

        let mut app = playing(mode);
        ingest(&mut app, physical(plx_ui::consts::SDLK_RETURN, 0, true, false));
        assert!(!app.inputs.is_empty(), "earlier real key is still queued");
        let queued = app.inputs.len();
        ingest(&mut app, physical(Q, 0, true, false));
        assert!(app.running, "Q cannot overtake an earlier input in the same SDL batch");
        assert_eq!(app.inputs.len(), queued, "queued-input refusal consumes Q, not the older event");
    }
}


#[test]
fn mac_physical_q_does_not_overtake_an_active_dispatcher_press() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused(plx_media::player::transport::paused());
    let _reset = Reset;
    for mode in [UiMode::Classic, UiMode::Modern] {
        // Provider rows activate on Down. A Play card owns the press until release.
        let mut app = catalog(mode, "channels");
        {
            let focus = app.pages.focus().expect("channel card must own focus");
            let parts = app.pages.parts(Tick { ms: app.loop_t, dt_us: 16_000 });
            let split = plx_ui::dispatch::Rig::<bridge::AppHost>::split(&mut app.bridge);
            let cx = parts.cx::<bridge::AppHost>(split.views, split.measure);
            let screen = app.pages.top_screen().expect("mounted channel catalogue");
            assert_eq!(screen.press_kind(&focus.elem, &cx), Some(plx_ui::screen::ElemKind::Card),
                "the actual focused row must use the shared press lifecycle");
        }
        ingest(&mut app, physical(plx_ui::consts::SDLK_RETURN, 0, true, false));
        dispatch_queued(&mut app);
        assert!(app.pages.input.arm.is_some() || app.pages.input.press.is_active(),
            "real OK-down must leave an owned press awaiting release");
        assert!(app.inputs.is_empty());
        assert!(!app.pages.surface_up() && !app.pages.has_pending_navigation());
        ingest(&mut app, physical(Q, 0, true, false));
        assert!(app.running, "Q must preserve the active press owner's routing");
        assert!(!app.desktop_quit_held, "a passed-through Back does not own physical release");
    }
}

#[test]
fn mac_physical_q_is_not_disabled_merely_by_an_inactive_parental_pin() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused(plx_media::player::transport::paused());
    let _reset = Reset;
    for mode in [UiMode::Classic, UiMode::Modern] {
        let mut app = playing(mode);
        app.ott.as_mut().unwrap().saved.settings.insert("parentPin".into(), json!(
            "pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
        assert!(app.ott.as_ref().unwrap().edit_session.is_none());
        assert!(!bridge::text_owns_input(&app.pages));
        ingest(&mut app, physical(Q, 0, true, false));
        assert!(!app.running, "a configured PIN is not an active editor or exit lock");
    }
}

#[test]
fn mac_physical_q_cannot_bypass_a_pending_non_strict_kiosk_write() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused(plx_media::player::transport::paused());
    let _reset = Reset;
    let mut app = playing(UiMode::Modern);
    let mut r = app.ott.take().unwrap();
    let pin = crate::app::ott_kiosk_stored::Stored::local_folder(
        plx_data::ott::favorites::Origin { provider: "local".into(), profile: "Fixture".into(),
            source_alias: "Fixture".into(), subprofile: None },
        false, "fixture-root".into(), "a".repeat(64)).unwrap().encode().unwrap();
    r.saved.settings.insert("nativeKiosk".into(), pin);
    r.saving = false;
    r.entropy = Some([4; 16]);
    // Admit the real operation, but the fixture's revoked permit refuses its
    // worker before any protected-state or filesystem I/O can begin.
    r.write_permit.revoke();
    let body = json!({"commands":[], "server_time":1000, "requests":[{
        "id":"0123456789abcdef0123456789abcdef", "action":"kiosk",
        "params":{"mode":"off"}, "expires_at":1020
    }]}).to_string();
    let mut invocations = r.remote.raw_poll_for_test(&body, 100);
    assert_eq!(invocations.len(), 1);
    let mut invocation = invocations.remove(0);
    invocation.deadline_ms = r.now_ms().saturating_add(10_000);
    assert!(commands::command(&mut r, &mut app, &invocation).is_none());
    assert!(matches!(r.rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        Job::Saved(Err(_), Some(_), false, _)));
    assert!(commands::kiosk_pending(&r));
    assert!(!crate::app::ott_runtime::input_policy::strict(&r));
    let before = serde_json::to_value(&r.saved).unwrap();
    app.ott = Some(r);
    assert!(crate::app::ott_runtime::kiosk_user_keys_blocked(&app));
    tap(&mut app, Q, 0);
    assert!(app.running && app.inputs.is_empty());
    let r = app.ott.as_ref().unwrap();
    assert!(commands::kiosk_pending(r), "a physical key cannot settle the owned write");
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
}

#[test]
fn mac_physical_q_respects_parental_editor_and_real_strict_kiosk_policy() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused(plx_media::player::transport::paused());
    let _reset = Reset;
    let mut app = catalog(UiMode::Modern, "settings");
    app.ott.as_mut().unwrap().saved.settings.insert("parentPin".into(), json!(
        "pbkdf2-sha256$600000$000102030405060708090a0b0c0d0e0f$ac310cc8527c6c3a4e20d2a2246ebd77d59766dae1c5fe843d65ecf6a7e20c65"));
    let session = open_editor(&mut app, "parental.unlock");
    ingest(&mut app, physical(Q, 0, true, false));
    assert!(app.running && app.inputs.is_empty());
    assert!(app.ott.as_ref().unwrap().edit_session.as_ref() == Some(&session));

    for malformed in [false, true] {
        let mut app = playing(UiMode::Modern);
        let value = if malformed { json!(null) } else {
            crate::app::ott_kiosk_stored::Stored::local_folder(
                plx_data::ott::favorites::Origin { provider: "local".into(), profile: "Fixture".into(),
                    source_alias: "Fixture".into(), subprofile: None },
                true, "fixture-root".into(), "a".repeat(64)).unwrap().encode().unwrap()
        };
        app.ott.as_mut().unwrap().saved.settings.insert("nativeKiosk".into(), value);
        assert!(crate::app::ott_runtime::kiosk_user_keys_blocked(&app));
        let before = serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap();
        tap(&mut app, Q, 0);
        assert!(app.running && app.inputs.is_empty());
        assert_eq!(serde_json::to_value(&app.ott.as_ref().unwrap().saved).unwrap(), before);
    }
}

#[test]
fn mac_physical_q_standby_gesture_only_wakes_and_next_fresh_gesture_quits() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused(plx_media::player::transport::paused());
    let _reset = Reset;
    let mut app = playing(UiMode::Modern);
    app.ott_standby = true;
    ingest(&mut app, physical(Q, 0, true, false));
    assert!(app.running && !app.ott_standby && app.ott_wake_release);
    ingest(&mut app, physical(Q, 0, false, false));
    assert!(app.running && !app.ott_wake_release);
    settle_pages(&mut app);
    app.inputs.clear();
    ingest(&mut app, physical(Q, 0, true, false));
    assert!(!app.running);
}

#[test]
fn mac_physical_q_preserves_controlled_bootstrap_input_fence() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused(plx_media::player::transport::paused());
    let _reset = Reset;
    let mut app = playing(UiMode::Modern);
    let initial = crate::app::bootstrap::Initial::synthetic_home(17, 32517, None).unwrap();
    app.rec = crate::app::recorder::Recplay::recording_with_sink(
        &initial, Box::new(plx_ui::rec::MemSink::default())).unwrap();
    app.boot_initial = Some(initial);
    ingest(&mut app, physical(Q, 0, true, false));
    assert!(app.running, "ordinary Q remapping must not bypass controlled input ownership");
}

#[test]
fn mac_physical_q_retains_nonzero_checkpoint_projection_for_reopen() {
    let _serial = plx_base::testlock::serial();
    let _paused = Paused(plx_media::player::transport::paused());
    let _reset = Reset;
    let mut app = playing(UiMode::Modern);
    plx_media::player::SHARED.playpos_ns.store(23_700_000_000, Relaxed);
    let before = app.ott.as_ref().unwrap().saved.clone();
    ingest(&mut app, physical(Q, 0, true, false));
    assert!(!app.running);
    let mut r = app.ott.take().unwrap();
    // This is the production shutdown's pre-teardown projection, not a disk
    // durability claim. The existing busy writer is deliberately left inert.
    r.checkpoint(&app, false);
    let checkpoint = r.checkpoints.get("desktop").expect("current profile checkpoint").clone();
    assert_eq!(checkpoint.entry_id, "first");
    assert_eq!(checkpoint.position_ms, 23_700);
    let mut projected = r.saved.clone();
    crate::app::ott_runtime::kiosk_resume::merge(&mut projected, &r.checkpoints);
    assert_eq!(projected.profiles, before.profiles);
    assert_eq!(projected.category, before.category);
    assert_eq!(projected.favorites, before.favorites);
    drop(r);
    drop(app);
    let mut reopened = app_fixture();
    let r = reopened.ott.as_mut().unwrap();
    r.catalog_done.store(false, Ordering::Release);
    r.metadata_done.store(false, Ordering::Release);
    r.resolver_busy = true;
    r.plex_refresh_pending = true;
    r.write_permit.revoke();
    r.tx.send(Job::Loaded(Ok((projected, [1; 16]))))
        .unwrap_or_else(|_| panic!("reopen receiver disappeared"));
    runtime_tick(&mut reopened);
    let r = reopened.ott.as_ref().unwrap();
    assert!(r.saved.resume.get("desktop") == Some(&checkpoint));
    assert_eq!(r.saved.profiles.active_profile.as_deref(), Some("desktop"));
}
