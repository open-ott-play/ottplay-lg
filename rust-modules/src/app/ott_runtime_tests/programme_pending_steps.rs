//! Physical step input while a resolver is held: newest selection, not a queue.
use super::*;
use plx_machine::machine::UiMode;
use plx_screens::registry::{OttPlayerAction as Action, OttPlayerRequest};

fn rocker(app: &mut App, code: u32) {
    native_edge(app, 0, code, Edge::Down);
    native_edge(app, 0, code, Edge::Up);
}

fn admitted_category_queue() -> App {
    admitted_category_queue_for(UiMode::Classic, false)
}
fn admitted_category_queue_for(mode: UiMode, finite: bool) -> App {
    let mut app = category_queue_before_admission(mode, finite);
    rocker(&mut app, 480);
    assert_eq!(queued(&app), Some("a"));
    super::player_remote::admit_latest_channel(&mut app, "a");
    app
}
fn category_queue_before_admission(mode: UiMode, finite: bool) -> App {
    let mut app = live_m3u_remote_app(mode, false);
    let runtime = app.ott.as_mut().unwrap();
    for entry in &mut runtime.catalog.entries {
        entry.category = "Playing".into();
        if finite { entry.duration_ms = Some(600_000); }
    }
    let mut other = runtime.catalog.entries[0].clone();
    other.id = "other".into();
    other.category = "Browse".into();
    other.media = MediaLocator::Http { url: "https://media.invalid/other.m3u8".into() };
    runtime.catalog.entries.push(other);
    runtime.playing.as_mut().unwrap().entry = runtime.catalog.entries[0].clone();
    runtime.saved.category = "Playing".into();
    runtime.catalog_revision += 1;
    app
}

// A synthetic accepted Saved image crosses the real completion guard without
// starting storage IO. The surrounding panel/input ownership is real.
fn accept_category(app: &mut App, category: &str) {
    surface_action(app, OttAction::ShowCategories);
    let mut runtime = app.ott.take().unwrap();
    let candidate = runtime.category_choice_candidate(Some(category)).unwrap();
    runtime.pending_category_choice = Some(category_state::Pending::capture(&runtime, Some(category)));
    runtime.saved = candidate;
    assert!(runtime.finish_category_choice(app, true));
    runtime.publish(app);
    app.ott = Some(runtime);
    idle(app);
}

// Browse contains only "other". Modern allows returning to fullscreen from
// that list while preserving both its category and the separately admitted queue.
fn browse_other_category_then_return(finite: bool) -> App {
    let mut app = admitted_category_queue_for(UiMode::Modern, finite);
    super::player_remote::open_fullscreen_catalog(&mut app);
    accept_category(&mut app, "Browse");
    assert_eq!(commands::selected_ids(app.ott.as_ref().unwrap()), ["other"]);
    let identity = route::playback_identity(&app.player.session);
    key(&mut app, Key::Back);
    assert!(ott_surface(&app).is_none());
    assert_eq!(route::playback_identity(&app.player.session), identity);
    assert_eq!(app.ott.as_ref().unwrap().playing.as_ref().unwrap().selection_id, "a");
    app
}

#[test]
fn accepted_organization_growth_refreshes_only_the_admitted_category_without_a_raw_catalog_change() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = category_queue_before_admission(UiMode::Classic, false);
    let mut runtime = app.ott.take().unwrap();
    runtime.resolver_hold_for_test = true;
    let profile_id = runtime.effective_profile().unwrap().id.clone();
    let id = |index| format!("{profile_id}:{index:016x}");
    let (a, b, c, other) = (id(1), id(2), id(3), id(4));
    for entry in &mut runtime.catalog.entries {
        entry.id = match entry.id.as_str() {
            "a" => a.clone(), "b" => b.clone(), "c" => c.clone(), "other" => other.clone(),
            _ => unreachable!("fixed synthetic catalogue"),
        };
    }
    runtime.catalog_revision += 1;
    assert!(runtime.organization_projection().is_some(), "use the real canonical-ID organization contract");
    runtime.play_with_policy(&mut app, &a, false).unwrap();
    app.ott = Some(runtime);
    super::player_remote::admit_latest_channel(&mut app, &a);
    super::player_remote::open_fullscreen_catalog(&mut app);
    accept_category(&mut app, "Browse");
    let runtime = app.ott.as_mut().unwrap();
    let identity = route::playback_identity(&app.player.session);
    let raw_revision = runtime.catalog_revision;
    let profile_epoch = runtime.profile_edit_epoch;
    let original = vec![c.clone(), a.clone(), b.clone()];
    assert_eq!(runtime.ordered_ids(), original);
    assert_eq!(commands::selected_ids(runtime), [other.clone()]);
    let source = catalog_organization::Source::new(runtime.effective_profile().unwrap()).unwrap();
    let projection = runtime.organization_projection().unwrap();
    let category = projection.selection_id("Playing").unwrap().to_owned();
    let mut document = catalog_organization::Document::read(&runtime.saved.settings).unwrap();
    document.apply(&source, &runtime.catalog, catalog_organization::Operation::AddMember {
        category: category.clone(), item: other.clone(),
    }).unwrap();
    // Model the committed Saved image, not a draft or a protected writer.
    document.write_into(&mut runtime.saved.settings).unwrap();
    runtime.observe_organization_context();
    assert_eq!(runtime.catalog_revision, raw_revision, "organization does not rewrite provider rows");
    assert_eq!(runtime.profile_edit_epoch, profile_epoch);
    assert_eq!(commands::selected_ids(runtime), [other.clone()], "Browse keeps its own membership");
    assert_eq!(runtime.ordered_ids(), [c.clone(), a.clone(), b.clone(), other.clone()], "compatible growth belongs to the admitted category");
    assert_eq!(runtime.playing.as_ref().unwrap().selection_id, a);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    document.apply(&source, &runtime.catalog, catalog_organization::Operation::MoveMember {
        category, item: c, direction: catalog_organization::Direction::Down,
    }).unwrap();
    document.write_into(&mut runtime.saved.settings).unwrap();
    runtime.observe_organization_context();
    assert!(runtime.ordered_ids().is_empty(), "reordering an old occurrence revokes the receipt");
    assert_eq!(commands::selected_ids(runtime), [other.clone()]);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(!runtime.metadata_done.load(Ordering::Acquire));
    assert_eq!(runtime.resolver_launches_for_test, 0);
    assert_eq!(route::playback_identity(&app.player.session), identity);
}

#[test]
fn browsed_category_cannot_replace_the_admitted_fullscreen_channel_queue() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for (code, expected) in [(480, "b"), (481, "c")] {
        let mut app = browse_other_category_then_return(false);
        let identity = route::playback_identity(&app.player.session);
        rocker(&mut app, code);
        assert_eq!(queued(&app), Some(expected));
        assert_eq!(app.ott.as_ref().unwrap().effective_category(), "Browse");
        assert_eq!(route::playback_identity(&app.player.session), identity);
    }
}

#[test]
fn admitted_queue_survives_progressive_append_and_rapid_steps_keep_the_latest_destination() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = browse_other_category_then_return(false);
    let runtime = app.ott.as_mut().unwrap();
    let mut appended = runtime.catalog.entries[0].clone();
    appended.id = "discovered-later".into();
    runtime.catalog.entries.push(appended);
    runtime.catalog.entries[0].description = "Fresh programme metadata".into();
    runtime.catalog_revision += 1;
    rocker(&mut app, 480);
    assert_eq!(queued(&app), Some("b"));
    let old = app.ott.as_mut().unwrap().resolver_next.take().unwrap();
    rocker(&mut app, 480);
    assert_eq!(queued(&app), Some("discovered-later"), "compatible discovery extends the admitted category, not the browsed category");
    assert!(old.cancel.load(Ordering::Acquire));
    rocker(&mut app, 481);
    assert_eq!(queued(&app), Some("b"));
    assert_eq!(app.ott.as_ref().unwrap().effective_category(), "Browse");
}

#[test]
fn admitted_queue_rejects_replaced_removed_or_revoked_source_without_browse_fallback() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mutation in ["profile", "generation", "removed", "media", "uncertain", "kiosk"] {
        let mut app = browse_other_category_then_return(false);
        let mut runtime = app.ott.take().unwrap();
        match mutation {
            "profile" => {
                runtime.saved.profiles.profiles[0].source = "https://changed.invalid/list.m3u".into();
                runtime.retire_profile_edits();
            }
            "generation" => runtime.generation += 1,
            "removed" => runtime.catalog.entries.retain(|entry| entry.id != "a"),
            "media" => runtime.catalog.entries.iter_mut().find(|entry| entry.id == "b").unwrap().media =
                MediaLocator::Http { url: "https://changed.invalid/b.m3u8".into() },
            "kiosk" => {
                let origin = json!({"provider":"m3u","profile":"Home","subprofile":null,"sourceAlias":"Local"});
                runtime.saved.settings.insert("nativeKiosk".into(), json!({"version":1,"strict":true,"origin":origin,
                    "target":{"kind":"item","entry":{"title":"Fixture","origin":origin,"locator":{"kind":"channel","channelId":"fixture-1"}}}}));
            }
            _ => { runtime.persistence_uncertain = true; },
        }
        assert!(playback_step::perform(&mut runtime, &mut app, 1).is_err(), "{mutation}");
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
        assert_eq!(runtime.effective_category(), "Browse");
        assert_eq!(runtime.playing.as_ref().unwrap().selection_id, "a");
    }
}

#[test]
fn admitted_partial_two_row_queue_discovers_third_channel_before_wrapping() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = live_m3u_remote_app(plx_machine::machine::UiMode::Classic, false);
    let runtime = app.ott.as_mut().unwrap();
    let third = runtime.catalog.entries.pop().unwrap();
    assert_eq!(runtime.catalog.entries.len(), 2);
    runtime.catalog_complete = false;
    rocker(&mut app, 480);
    super::player_remote::admit_latest_channel(&mut app, "a");
    let runtime = app.ott.as_mut().unwrap();
    runtime.catalog.entries.push(third);
    runtime.catalog_revision += 1;
    rocker(&mut app, 480);
    assert_eq!(queued(&app), Some("b"), "newly discovered third channel precedes wrap to first");
}

#[test]
fn explicit_accepted_category_focuses_current_channel_without_stealing_later_browse_focus() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = admitted_category_queue();
    super::player_remote::open_fullscreen_catalog(&mut app);
    browse_focus::record(app.ott.as_mut().unwrap(), "c");
    accept_category(&mut app, "Browse");
    accept_category(&mut app, "Playing");
    assert_eq!(app.ott.as_ref().unwrap().view().selected_row.as_deref(), Some("a"));
    let identity = route::playback_identity(&app.player.session);
    let generation = app.ott.as_ref().unwrap().play_generation;
    key(&mut app, Key::Ok);
    assert!(ott_surface(&app).is_none(), "real focused current-row OK returns to its retained Player");
    assert!(queued(&app).is_none());
    assert_eq!(app.ott.as_ref().unwrap().play_generation, generation);
    assert_eq!(route::playback_identity(&app.player.session), identity);
    super::player_remote::open_fullscreen_catalog(&mut app);
    // Ordinary focus movement remains presentation, independently of playback.
    key(&mut app, Key::Down);
    let runtime = app.ott.take().unwrap();
    runtime.publish(&mut app);
    app.ott = Some(runtime);
    idle(&mut app);
    assert_eq!(app.ott.as_ref().unwrap().view().selected_row.as_deref(), Some("b"));
    assert_eq!(app.ott.as_ref().unwrap().playing.as_ref().unwrap().selection_id, "a");
    assert!(queued(&app).is_none());
    key(&mut app, Key::Ok);
    assert_eq!(queued(&app), Some("b"), "a later ordinary browse move keeps its real form focus");
}

#[test]
fn finite_eof_uses_the_admitted_category_after_browsing_elsewhere_and_denies_revoked_source() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for revoked in [false, true] {
        let mut app = browse_other_category_then_return(true);
        if revoked {
            let runtime = app.ott.as_mut().unwrap();
            runtime.saved.profiles.profiles[0].enabled = false;
            runtime.retire_profile_edits();
            assert!(runtime.ordered_ids().is_empty());
        }
        assert!(!player::is_started(), "inert native resources do not run a decoder pump");
        player::SHARED.ended.store(true, Ordering::Release);
        let tick = next_tick(&mut app);
        let mut fr = run::Frame {
            ctrl: player_hud::slot(&app.player.session, app.bridge.metadata_view()),
            now: tick.ms, dt: 0.016, underlay_moving: false, player: true,
            present: false, deferred: false, rn: "player",
        };
        unsafe { run::playback_tick(&mut app, &mut fr); }
        if revoked { assert!(queued(&app).is_none(), "revocation cannot fall through to browsed B"); }
        else { assert_eq!(queued(&app), Some("b")); }
    }
}

#[test]
fn partial_playback_queue_does_not_borrow_browsed_favorites_completion_or_outlive_stop() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = admitted_category_queue();
    let runtime = app.ott.as_mut().unwrap();
    runtime.catalog_complete = false;
    runtime.saved.favorites_selected = true;
    assert_eq!(runtime.ordered_ids(), ["c", "a", "b"]);
    assert!(!runtime.selection_complete(), "an empty Favorites browser cannot complete partial media discovery");
    runtime.catalog_complete = true;
    assert!(runtime.selection_complete());
    runtime.continuity.stop(runtime.generation);
    runtime.playing = None;
    assert!(runtime.ordered_ids().is_empty(), "stopped playback cannot lend its old queue to browsing");
    assert!(playback_step::continuation_ids(runtime).is_none());
}

#[test]
fn deliberate_new_profile_catalogue_keeps_its_own_progressive_first_play_domain() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = admitted_category_queue();
    let runtime = app.ott.as_mut().unwrap();
    let mut profile = runtime.saved.profiles.profiles[0].clone();
    profile.id = "new-profile".into();
    profile.source = "https://new.invalid/list.m3u".into();
    let mut saved = runtime.saved.clone();
    saved.profiles.profiles.push(profile.clone());
    saved.profiles.active_profile = Some(profile.id.clone());
    runtime.observe_profile_edits(&saved);
    runtime.saved = saved;
    // Hold actual catalogue work, then exercise the real domain transition.
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.load_catalog_profile(Some(profile), false);
    assert!(runtime.catalog_next.is_some());
    let mut entry = synthetic_entry();
    entry.id = "first-new".into();
    entry.category.clear();
    runtime.catalog.entries = vec![entry];
    assert_eq!(runtime.ordered_ids(), ["first-new"]);
    assert!(playback_step::continuation_ids(runtime).is_none(), "old receipt is outside the new continuity generation");
    let ids = runtime.ordered_ids();
    let refs = ids.iter().map(String::as_str).collect::<Vec<_>>();
    assert!(matches!(runtime.continuity.catalog_updated(runtime.generation, &refs, false),
        continuity::Decision::Start { entry_id, .. } if entry_id == "first-new"));
}

#[test]
fn late_explicit_category_ack_keeps_the_newest_pending_channel_focus() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = admitted_category_queue();
    rocker(&mut app, 480);
    assert_eq!(queued(&app), Some("b"));
    let mut runtime = app.ott.take().unwrap();
    let candidate = runtime.category_choice_candidate(Some("Playing")).unwrap();
    runtime.pending_category_choice = Some(category_state::Pending::capture(&runtime, Some("Playing")));
    runtime.saved = candidate;
    assert!(runtime.finish_category_choice(&app, true));
    assert_eq!(runtime.raw_view().selected_row.as_deref(), Some("b"));
    assert_eq!(runtime.play_target.as_ref().unwrap().id, "b");
    assert_eq!(runtime.playing.as_ref().unwrap().selection_id, "a");
}

#[test]
fn finite_error_continuation_keeps_exact_queue_and_denied_scope_cannot_later_fall_back_to_browse() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for revoked in [false, true] {
        let mut app = browse_other_category_then_return(true);
        let runtime = app.ott.as_mut().unwrap();
        // Model the ordinary VOD policy; explicit channel stepping itself
        // deliberately refuses advance-on-error and remains unchanged.
        runtime.playing.as_mut().unwrap().advance_on_failure = true;
        if revoked {
            runtime.catalog.entries.iter_mut().find(|entry| entry.id == "b").unwrap().media =
                MediaLocator::Http { url: "https://changed.invalid/b.m3u8".into() };
            runtime.catalog_revision += 1;
        }
        player::swap_state_for_test(player::PlaybackState::Error);
        assert_eq!(player::state(&app.player.session), player::PlaybackState::Error);
        runtime_tick(&mut app);
        assert!(app.ott.as_ref().unwrap().playing.is_none());
        if revoked {
            assert!(queued(&app).is_none());
            app.ott.as_mut().unwrap().catalog_dirty = true;
            runtime_tick(&mut app);
            assert!(queued(&app).is_none(), "a later unscoped discovery tick cannot borrow browsed B");
            assert!(app.ott.as_ref().unwrap().continuity.pending_retry().is_none());
        } else {
            assert_eq!(queued(&app), Some("b"));
            let runtime = app.ott.as_ref().unwrap();
            assert_eq!(playback_step::target(runtime, &app, 1).unwrap(), "c",
                "the queued successor inherits its original Arc after failure scope exits");
            let runtime = app.ott.as_mut().unwrap();
            runtime.cancel_play_request();
            runtime.continuity.stop(runtime.generation);
            assert!(playback_step::continuation_ids(runtime).is_none(), "scope cannot outlive its exact error transition");
        }
    }
}

#[test]
fn resolver_failure_and_discovery_keep_the_original_queue_even_before_first_admission() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for initial in [false, true] {
        for revoked in [false, true] {
            let mut app = if initial { category_queue_before_admission(UiMode::Classic, true) }
                else { admitted_category_queue_for(UiMode::Classic, true) };
            app.ott.as_mut().unwrap().resolver_hold_for_test = true;
            assert_eq!(app.ott.as_ref().unwrap().resolver_launches_for_test, 0);
            if !initial { super::player_remote::open_fullscreen_catalog(&mut app); }
            let mut runtime = app.ott.take().unwrap();
            runtime.catalog_complete = false;
            runtime.continuity = continuity::Continuity::new(runtime.generation, None);
            if initial {
                runtime.playing = None;
                runtime.play_with_policy(&mut app, "a", true).unwrap();
            } else {
                runtime.continuity.user_selected(runtime.generation, "a");
                runtime.playing.as_mut().unwrap().advance_on_failure = true;
            }
            app.ott = Some(runtime);
            // A pending initial resolver has no offered catalogue surface, and
            // this partial idle catalogue has no remote category-read receipt.
            // Exercise browsing only through the real admitted-player door.
            if !initial { accept_category(&mut app, "Browse"); }
            let stale = if initial {
                let work = app.ott.as_mut().unwrap().resolver_next.take().unwrap();
                let stale = (work.generation, work.profile.clone());
                app.ott.as_ref().unwrap().tx.send(Job::Play(work.generation, work.profile,
                    Err("initial resolver refused"))).unwrap();
                stale
            } else {
                player::swap_state_for_test(player::PlaybackState::Error);
                (0, app.ott.as_ref().unwrap().effective_profile().unwrap().clone())
            };
            runtime_tick(&mut app);
            assert_eq!(app.ott.as_ref().unwrap().resolver_launches_for_test, 0);
            assert_eq!(queued(&app), Some("b"), "initial={initial}: failure must advance within Playing");
            let work = app.ott.as_mut().unwrap().resolver_next.take().unwrap();
            let latest = work.generation;
            if initial {
                app.ott.as_ref().unwrap().tx.send(Job::Play(stale.0, stale.1,
                    Err("late superseded failure"))).unwrap();
                runtime_tick(&mut app);
                assert_eq!(app.ott.as_ref().unwrap().resolver_launches_for_test, 0);
                assert_eq!(app.ott.as_ref().unwrap().play_generation, latest);
                assert_eq!(queued(&app), Some("b"));
            }
            app.ott.as_ref().unwrap().tx.send(Job::Play(work.generation, work.profile,
                Err("successor resolver refused"))).unwrap();
            runtime_tick(&mut app);
            assert_eq!(app.ott.as_ref().unwrap().resolver_launches_for_test, 0);
            let runtime = app.ott.as_mut().unwrap();
            assert!(runtime.play_target.is_none());
            let due = runtime.continuity.pending_retry().expect("second refusal owns the normal cooldown").not_before_ms;
            assert_eq!(runtime.ordered_ids(), ["c", "a", "b"]);
            if revoked { runtime.profile_edit_epoch += 1; }
            let ids = runtime.ordered_ids();
            let refs: Vec<_> = ids.iter().map(String::as_str).collect();
            let decision = runtime.retry_continuity(due, &refs, runtime.selection_complete());
            runtime.apply_continuity(decision);
            assert!(runtime.play_target.is_none(), "partial queue waits for discovery at its end");
            let mut discovered = runtime.catalog.entries[0].clone();
            discovered.id = "discovered-after-errors".into();
            runtime.catalog.entries.push(discovered);
            runtime.catalog_revision += 1;
            runtime.catalog_dirty = true;
            runtime_tick(&mut app);
            assert_eq!(app.ott.as_ref().unwrap().resolver_launches_for_test, 0);
            assert_eq!(queued(&app), if revoked { None } else { Some("discovered-after-errors") },
                "initial={initial}: discovery may extend the original queue, never borrow Browse");
            assert_eq!(app.ott.as_ref().unwrap().effective_category(), if initial { "Playing" } else { "Browse" });
            if revoked {
                app.ott.as_mut().unwrap().catalog_dirty = true;
                runtime_tick(&mut app);
                assert_eq!(app.ott.as_ref().unwrap().resolver_launches_for_test, 0);
                assert!(queued(&app).is_none(), "denied source cannot restart on a later unscoped tick");
            }
        }
    }
}

#[test]
fn rapid_pending_rocker_advances_latest_target_cancels_old_and_ignores_late_result() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [plx_machine::machine::UiMode::Classic, plx_machine::machine::UiMode::Modern] {
        let mut app = live_m3u_remote_app(mode, false);
        browse_focus::record(app.ott.as_mut().unwrap(), "c");
        let installed = route::playback_identity(&app.player.session);
        rocker(&mut app, 480);
        assert_eq!(queued(&app), Some("a"));
        let old = app.ott.as_mut().unwrap().resolver_next.take().unwrap();
        let cancelled = old.cancel.clone();
        rocker(&mut app, 480);
        assert_eq!(queued(&app), Some("b"));
        assert_eq!(app.ott.as_ref().unwrap().raw_view().selected_row.as_deref(), Some("b"));
        assert!(cancelled.load(Ordering::Acquire));
        assert_eq!(app.ott.as_ref().unwrap().resolver_next.as_ref().unwrap().entry.as_ref().unwrap().id, "b");
        let current_generation = app.ott.as_ref().unwrap().play_generation;
        app.ott.as_ref().unwrap().tx.send(Job::Play(old.generation, old.profile,
            Err("superseded inspection result"))).unwrap();
        runtime_tick(&mut app);
        assert_eq!(queued(&app), Some("b"));
        assert_eq!(app.ott.as_ref().unwrap().play_generation, current_generation);
        assert_eq!(app.ott.as_ref().unwrap().raw_view().selected_row.as_deref(), Some("b"),
            "a superseded completion cannot restore the old browse highlight");
        assert_eq!(route::playback_identity(&app.player.session), installed);
        assert!(app.ott.as_ref().unwrap().resolver_busy, "one old worker owns the busy slot until it retires");
    }
}

#[test]
fn same_frame_rocker_burst_coalesces_and_reverse_cancels_pending_outgoing_return() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for reverse in [false, true] {
        let mut app = live_m3u_remote_app(plx_machine::machine::UiMode::Classic, false);
        browse_focus::record(app.ott.as_mut().unwrap(), "c");
        let installed = route::playback_identity(&app.player.session);
        let before = app.ott.as_ref().unwrap().play_generation;
        let down = crate::app::events::encode_key(0, 480, true);
        let release = crate::app::events::encode_key(0, 480, false);
        let second = crate::app::events::encode_key(0, if reverse { 481 } else { 480 }, true);
        let fr = dispatch_batch(&mut app, &[down, release, second]);
        finish_input(&mut app, &fr);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.play_generation.wrapping_sub(before) >= 2);
        if reverse {
            assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none(),
                "returning to the installed outgoing channel cancels the incoming request");
            assert_eq!(runtime.selected.as_deref(), Some("c"));
            assert_eq!(runtime.raw_view().selected_row.as_deref(), Some("c"));
        } else {
            assert_eq!(queued(&app), Some("b"), "two fresh presses in one dispatch batch are both admitted");
            assert_eq!(runtime.raw_view().selected_row.as_deref(), Some("b"));
            assert_eq!(runtime.resolver_next.as_ref().unwrap().entry.as_ref().unwrap().id, "b");
        }
        assert_eq!(route::playback_identity(&app.player.session), installed);
    }
}

#[test]
fn pending_step_offer_rejects_transport_menus_and_stale_source_or_later_frame_lineage() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = live_m3u_remote_app(plx_machine::machine::UiMode::Classic, false);
    rocker(&mut app, 480);
    crate::app::ott_runtime::publish_remote_player_offer(&mut app);
    let source = app.bridge.ott_player_offer().unwrap().source.clone();
    assert!(source.pending());
    let request = OttPlayerRequest { owner: app.pages.top_page().unwrap(), source, action: Action::Next };
    let generation = app.ott.as_ref().unwrap().play_generation;
    for action in [Action::OpenCatalog, Action::OpenMenu, Action::ShowInfo,
        Action::ShowDescription, Action::OpenGuide, Action::SeekForward, Action::SeekBackward, Action::TogglePause] {
        assert!(crate::app::ott_runtime::remote_player_action(&mut app,
            OttPlayerRequest { action, ..request.clone() }).is_err());
    }
    assert_eq!(app.ott.as_ref().unwrap().play_generation, generation);
    crate::app::ott_runtime::remote_player_action(&mut app, request.clone()).unwrap();
    assert_eq!(queued(&app), Some("b"));
    // Refreshing the input publication retires the old same-frame chain too.
    crate::app::ott_runtime::publish_remote_player_offer(&mut app);
    assert!(crate::app::ott_runtime::remote_player_action(&mut app, request).is_err());
    assert_eq!(queued(&app), Some("b"));
}

fn initial_ott_pending() -> App {
    let mut app = live_m3u_remote_app(plx_machine::machine::UiMode::Classic, false);
    player::stop_bufferfeed(&mut app.player.session, &mut app.adapters.player);
    route::cancel_play(&mut app.player.session);
    app.player.session = route::PlaybackSession::IDLE;
    route::reset_player_control_for_test(&app.player.session);
    app.ott.as_mut().unwrap().playing = None;
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle_pages(&mut app);
    let mut runtime = app.ott.take().unwrap();
    runtime.reply = current_ui_owner(&app);
    runtime.page = "channels";
    runtime.catalog_dirty = false;
    runtime.resolver_busy = true;
    runtime.continuity.stop(runtime.generation);
    apply_ui_action(&mut runtime, &mut app, OttAction::Play("a".into()), true, true, false);
    assert!(runtime.play_target.is_some(), "initial Play refused: {}", runtime.status);
    assert_eq!(runtime.view().pending_play.as_deref(), Some("a"),
        "mode={:?} reply={} owner={} page={}", runtime.interface_mode(), runtime.reply.is_some(), current_ui_owner(&app).is_some(), runtime.page);
    assert!(runtime.view().pending_step.is_some());
    runtime.publish(&mut app);
    app.ott = Some(runtime);
    idle(&mut app);
    assert!(matches!(app.route(), AppArg::Ott));
    assert!(route::playback_identity(&app.player.session).is_none());
    app
}

#[test]
fn initial_ott_loading_cover_handles_physical_rocker_but_not_hidden_list_and_back_cancels() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = initial_ott_pending();
    let old = app.ott.as_ref().unwrap().play_cancel.clone();
    rocker(&mut app, 480);
    assert_eq!(queued(&app), Some("b"));
    assert!(old.load(Ordering::Acquire));
    assert!(matches!(app.route(), AppArg::Ott), "stepping does not admit a native Player");
    let generation = app.ott.as_ref().unwrap().play_generation;
    native_edge(&mut app, consts::SDLK_DOWN, 81, Edge::Down);
    native_edge(&mut app, consts::SDLK_RETURN, 40, Edge::Down);
    assert_eq!(app.ott.as_ref().unwrap().play_generation, generation, "hidden rows cannot be activated");
    native_edge(&mut app, consts::SDLK_ESCAPE, 41, Edge::Down);
    assert!(app.ott.as_ref().unwrap().play_target.is_none());
    assert!(app.ott.as_ref().unwrap().resolver_next.is_none());
    assert!(app.ott.as_ref().unwrap().view().pending_play.is_none());
}


#[test]
fn three_same_frame_channel_presses_leave_only_the_latest_destination() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = live_m3u_remote_app(plx_machine::machine::UiMode::Classic, false);
    let runtime = app.ott.as_mut().unwrap();
    let mut last = runtime.catalog.entries[0].clone();
    last.id = "d".into(); last.title = "Fourth".into();
    last.media = MediaLocator::Http { url: "https://media.invalid/d.m3u8".into() };
    runtime.catalog.entries.push(last);
    runtime.catalog_revision += 1;
    let down = crate::app::events::encode_key(0, 480, true);
    let release = crate::app::events::encode_key(0, 480, false);
    let fr = dispatch_batch(&mut app, &[down, release, down, release, down]);
    finish_input(&mut app, &fr);
    assert_eq!(queued(&app), Some("d"));
    assert_eq!(app.ott.as_ref().unwrap().resolver_next.as_ref().unwrap().entry.as_ref().unwrap().id, "d");
}

#[test]
fn replay_namespace_cannot_spend_initial_pending_step_offer() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    let mut app = initial_ott_pending();
    let before = app.ott.as_ref().unwrap().play_generation;
    let tick = next_tick(&mut app);
    run::publish_playback_frame(&mut app, false);
    bridge::frame(&mut app.pages, &mut app.bridge, tick, vec![plx_machine::machine::InputEvent {
        kind: plx_machine::machine::InputKind::Key { key: plx_machine::machine::Key::Other,
            sym: 0, wcode: 480, edge: Edge::Down, at_edge: false },
        at: tick, source: plx_machine::machine::Source::Replay,
    }]);
    let fr = run::Frame { ctrl: player_hud::slot(&app.player.session, app.bridge.metadata_view()),
        now: tick.ms, dt: 0.016, underlay_moving: false, player: false,
        present: false, deferred: false, rn: "ott" };
    finish_input(&mut app, &fr);
    assert_eq!(app.ott.as_ref().unwrap().play_generation, before);
    assert_eq!(queued(&app), Some("a"));
}

#[test]
fn thirty_two_pending_channel_steps_keep_filtered_browse_focus_on_latest_intent() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [plx_machine::machine::UiMode::Classic, plx_machine::machine::UiMode::Modern] {
        let mut app = live_m3u_remote_app(mode, false);
        let runtime = app.ott.as_mut().unwrap();
        let mut first = runtime.catalog.entries[0].clone();
        first.id = "c".into();
        first.category = "Selected".into();
        runtime.catalog.entries = vec![first.clone()];
        for index in 1..=32 {
            let mut row = first.clone();
            row.id = format!("channel-{index:02}");
            row.title = format!("Channel {}", index + 1);
            row.media = MediaLocator::Http { url: format!("https://media.invalid/{}.m3u8", row.id) };
            runtime.catalog.entries.push(row.clone());
            row.id = format!("hidden-{index:02}");
            row.category = "Other".into();
            runtime.catalog.entries.push(row);
        }
        runtime.saved.category = "Selected".into();
        runtime.catalog_revision += 1;
        browse_focus::record(runtime, "c");
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        let installed = route::playback_identity(&app.player.session);
        for index in 1..=32 {
            rocker(&mut app, 480);
            let wanted = format!("channel-{index:02}");
            let runtime = app.ott.as_ref().unwrap();
            assert_eq!(queued(&app), Some(wanted.as_str()), "{mode:?}, step {index}");
            assert_eq!(runtime.raw_view().selected_row.as_deref(), Some(wanted.as_str()));
            assert_eq!(runtime.effective_category(), "Selected");
        }
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved,
            "rocker focus stays in RAM until an existing persistence boundary");
        assert_eq!(route::playback_identity(&app.player.session), installed);
    }
}

#[test]
fn refused_channel_step_keeps_existing_browse_focus() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [plx_machine::machine::UiMode::Classic, plx_machine::machine::UiMode::Modern] {
        let mut app = live_m3u_remote_app(mode, false);
        let mut runtime = app.ott.take().unwrap();
        browse_focus::record(&mut runtime, "b");
        runtime.catalog.entries.retain(|entry| entry.id != "c");
        assert!(playback_step::perform(&mut runtime, &mut app, 1).is_err(),
            "an installed source outside this list cannot admit a destination");
        assert_eq!(runtime.raw_view().selected_row.as_deref(), Some("b"));
        assert!(runtime.play_target.is_none());
    }
}

#[test]
fn favorite_channel_step_records_favorite_identity_without_changing_source_selection() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [plx_machine::machine::UiMode::Classic, plx_machine::machine::UiMode::Modern] {
        let mut app = live_m3u_remote_app(mode, false);
        let runtime = app.ott.as_mut().unwrap();
        let profile = runtime.effective_profile().unwrap().clone();
        // The inherited synthetic entries all have provider_id "1". For M3U
        // that is the portable tvg-id, so they would toggle one Favorite three
        // times rather than create three independently selectable channels.
        for entry in &mut runtime.catalog.entries {
            entry.provider_id = Some(entry.id.clone());
        }
        runtime.playing.as_mut().unwrap().entry = runtime.catalog.entries[0].clone();
        let favorites: Vec<_> = runtime.catalog.entries.iter()
            .map(|entry| selection::capture(&profile, entry).unwrap()).collect();
        let current = format!("favorite:{}", favorites[0].identity());
        let wanted = format!("favorite:{}", favorites[1].identity());
        assert_ne!(current, wanted, "the fixture needs distinct portable channel identities");
        for favorite in favorites { runtime.saved.favorites.toggle(favorite).unwrap(); }
        assert_eq!(runtime.saved.favorites.entries().len(), 3);
        runtime.saved.favorites_selected = true;
        runtime.page = "favorites";
        runtime.playing.as_mut().unwrap().selection_id = current.clone();
        browse_focus::record(runtime, &current);
        let active = runtime.saved.profiles.active_profile.clone();
        rocker(&mut app, 480);
        let runtime = app.ott.as_ref().unwrap();
        assert_eq!(queued(&app), Some(wanted.as_str()));
        assert_eq!(runtime.raw_view().selected_row.as_deref(), Some(wanted.as_str()));
        assert!(runtime.saved.favorites_selected);
        assert_eq!(runtime.saved.profiles.active_profile, active);
    }
}

#[test]
fn failed_channel_resolver_retains_admitted_browse_destination() {
    let _lock = plx_base::testlock::serial();
    let _globals = Globals(player::swap_state_for_test(player::PlaybackState::Idle));
    for mode in [plx_machine::machine::UiMode::Classic, plx_machine::machine::UiMode::Modern] {
        let mut app = live_m3u_remote_app(mode, false);
        browse_focus::record(app.ott.as_mut().unwrap(), "c");
        rocker(&mut app, 480);
        let failed = app.ott.as_mut().unwrap().resolver_next.take().unwrap();
        app.ott.as_ref().unwrap().tx.send(Job::Play(failed.generation, failed.profile,
            Err("fixture channel resolution refused"))).unwrap();
        runtime_tick(&mut app);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.play_target.is_none());
        assert_eq!(runtime.raw_view().selected_row.as_deref(), Some("a"));
        assert_eq!(runtime.playing.as_ref().unwrap().selection_id, "c",
            "failed navigation is not installed playback");
    }
}
