//! Exercise the actual Runtime-to-fullscreen sink with the existing host fixture.
use super::*;
use plx_data::ott::reminders::{Context, Fx as ReminderFx, PromptToken, Record};

fn configured() -> (App, Runtime) {
    let mut app = app_fixture();
    mount_player(&mut app);
    let mut runtime = app.ott.take().unwrap();
    let profile = ProviderProfile {
        id: "reminder-source".into(),
        name: "Source".into(),
        kind: ProviderKind::M3u,
        source: "https://media.invalid/list.m3u".into(),
        source_alias: "Source".into(),
        enabled: true,
    };
    runtime.saved.profiles.active_profile = Some(profile.id.clone());
    runtime.saved.profiles.profiles = vec![profile];
    runtime.loaded = true;
    runtime.wall_clock_ms = 100_000;
    let mut entry = synthetic_entry();
    entry.id = "channel".into();
    entry.media = MediaLocator::Http {
        url: "https://media.invalid/live.m3u8".into(),
    };
    entry.duration_ms = None;
    runtime.catalog.entries = vec![entry];
    runtime.load_reminders(&mut app).unwrap();
    (app, runtime)
}

fn record(runtime: &Runtime, start: i64) -> Record {
    Record {
        key: plx_data::ott::reminders::Key {
            source_id: runtime.reminder_source_id().unwrap(),
            channel_id: "channel".into(),
            programme_id: format!("programme-{start}"),
            start_s: start,
        },
        end_s: start + 600,
        title: "Programme".into(),
    }
}

fn show_prompt(app: &mut App, runtime: &mut Runtime) -> PromptToken {
    let item = record(runtime, 200);
    let (saved, token) = runtime
        .reminders
        .toggle(&runtime.saved, item, 100_000)
        .unwrap();
    runtime.reminder_save = Some(token);
    assert!(runtime.finish_reminders(app, Ok(&saved)).is_none());
    runtime.saved = saved;
    runtime.wall_clock_ms = 200_000;
    let source = runtime.reminder_source_id().unwrap();
    let effects = runtime
        .reminders
        .poll(
            200_000,
            Context {
                source_id: &source,
                revision: runtime.generation,
                active: true,
            },
            0,
            |_| true,
        )
        .unwrap();
    let token = effects
        .iter()
        .find_map(|effect| match effect {
            ReminderFx::PromptSwitch { token, .. } => Some(*token),
            _ => None,
        })
        .unwrap();
    runtime.reminder_effects(app, effects);
    token
}

#[test]
fn fullscreen_prompt_is_withdrawn_when_a_real_player_overlay_takes_input() {
    let _guard = plx_base::testlock::serial();
    let (mut app, mut runtime) = configured();
    let token = show_prompt(&mut app, &mut runtime);
    assert!(runtime.reminders_active(&app));
    assert!(runtime.reminders.prompt_key(token).is_some());
    bridge::open_player_overlay(
        &app.player.session,
        app.bridge.metadata_view(),
        &mut app.pages,
        plx_screens::player::overlay::OverlayKind::Info,
    );
    settle_pages(&mut app);
    assert!(app.pages.surface_up());
    assert!(!runtime.reminders_active(&app));
    runtime.poll_reminders(&mut app, false);
    assert!(runtime.reminders.prompt_key(token).is_none());
    assert!(runtime.answer_reminder(&mut app, token, true).is_err());
}

#[test]
fn strict_policy_transition_withdraws_an_open_prompt_and_rejects_acceptance() {
    let _guard = plx_base::testlock::serial();
    let (mut app, mut runtime) = configured();
    let token = show_prompt(&mut app, &mut runtime);
    let origin =
        json!({"provider":"m3u","profile":"Source","subprofile":null,"sourceAlias":"Source"});
    runtime.saved.settings.insert(
        "nativeKiosk".into(),
        json!({"version":1,"strict":true,
        "origin":origin.clone(),"target":{"kind":"item","entry":{"title":"Channel",
        "origin":origin,"locator":{"kind":"channel","channelId":"fixture-1"}}}}),
    );
    assert!(!runtime.reminders_active(&app));
    runtime.poll_reminders(&mut app, false);
    assert!(runtime.reminders.prompt_key(token).is_none());
    assert!(runtime.answer_reminder(&mut app, token, true).is_err());
    assert!(runtime.answer_reminder(&mut app, token, false).is_ok());
}

#[test]
fn authoritative_load_without_the_owner_clock_remains_unavailable() {
    let _guard = plx_base::testlock::serial();
    let (mut app, mut runtime) = configured();
    show_prompt(&mut app, &mut runtime);
    runtime.clock_sample = None;
    assert!(runtime.load_reminders(&mut app).is_err());
    assert!(!runtime.reminders_active(&app));
    assert!(!runtime.reminder_contains(&record(&runtime, 200).key));
}

#[test]
fn failed_and_uncertain_writer_results_follow_the_actual_runtime_hook() {
    let _guard = plx_base::testlock::serial();
    let (mut app, mut runtime) = configured();
    let prompt = show_prompt(&mut app, &mut runtime);
    let item = record(&runtime, 300);
    let (_, token) = runtime
        .reminders
        .toggle(&runtime.saved, item.clone(), 200_000)
        .unwrap();
    runtime.reminder_save = Some(token);
    assert!(runtime
        .finish_reminders(
            &mut app,
            Err(super::super::reminders::SaveFailure::Rejected)
        )
        .is_none());
    assert!(!runtime.reminder_contains(&item.key));
    assert!(runtime.reminders.prompt_key(prompt).is_some());
    let (_, token) = runtime
        .reminders
        .toggle(&runtime.saved, item.clone(), 200_000)
        .unwrap();
    runtime.reminder_save = Some(token);
    assert!(runtime
        .finish_reminders(&mut app, Err(super::super::reminders::SaveFailure::Unknown))
        .is_some());
    assert!(!runtime.reminder_contains(&item.key));
    assert!(runtime.reminders.prompt_key(prompt).is_none());
    assert!(runtime
        .reminders
        .toggle(&runtime.saved, item, 200_000)
        .is_err());
}

#[test]
fn worker_retirement_refuses_answer_without_consuming_the_prompt() {
    let _guard = plx_base::testlock::serial();
    let (mut app, mut runtime) = configured();
    let token = show_prompt(&mut app, &mut runtime);
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.suspend_source_work(&mut app);
    runtime.resume_source_work();
    assert!(!runtime.play_work_allowed());
    assert!(!runtime.reminders_active(&app));
    let generation = runtime.play_generation;
    let epoch = runtime.playback_epoch;
    assert_eq!(runtime.answer_reminder(&mut app, token, true),
        Err("Media work is waiting for foreground or worker retirement"));
    assert!(runtime.reminders.prompt_key(token).is_some());
    assert_eq!(runtime.play_generation, generation);
    assert_eq!(runtime.playback_epoch, epoch);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
}
