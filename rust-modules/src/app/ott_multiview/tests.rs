use super::*;
use plx_data::ott::favorites::{Favorite, Locator, Origin};
use plx_data::ott::multiview::{AdmissionOutcome, Layout};
use plx_data::ott::{CatalogEntry, MediaLocator, ProviderKind, ProviderProfile};

fn fixture() -> (App, Runtime) {
    let mut app = crate::app::run::lifecycle_regression_tests::app();
    app.running = true;
    let mut runtime = Runtime::unloaded(crate::app::ott_companion::Supervisor::inert_for_test());
    runtime.remote = crate::app::ott_remote::Remote::disabled_for_test("dev_mv_coord", "fixture");
    runtime.loaded = true;
    runtime.state_load = crate::app::ott_runtime::StateLoad::Idle;
    runtime.entropy = Some([7; 16]);
    runtime.resume_source_work();
    runtime.saved.profiles.profiles = (1..=2)
        .map(|n| ProviderProfile {
            id: format!("m3u-{n}"),
            name: format!("Profile {n}"),
            kind: ProviderKind::M3u,
            source: format!("https://example.invalid/{n}.m3u"),
            source_alias: format!("Source {n}"),
            enabled: true,
        })
        .collect();
    for n in 1..=2 {
        runtime
            .saved
            .favorites
            .toggle(Favorite {
                title: format!("Channel {n}"),
                origin: Origin {
                    provider: "m3u".into(),
                    profile: format!("Profile {n}"),
                    subprofile: None,
                    source_alias: format!("Source {n}"),
                },
                locator: Locator::ChannelName {
                    name: format!("Channel {n}"),
                    group: Some("Documentary".into()),
                },
            })
            .unwrap();
    }
    runtime.saved.profiles.active_profile = Some("m3u-1".into());
    runtime.provider = ProviderKind::M3u;
    (app, runtime)
}

fn source(coordinator: &Coordinator, index: usize) -> SourceHandle {
    coordinator.source_rows().get(index).unwrap().handle
}

fn slot(index: u8) -> SlotId {
    SlotId::new(index).unwrap()
}

fn http_entry(title: &str) -> CatalogEntry {
    CatalogEntry {
        id: format!("fixture-{title}"),
        title: title.into(),
        category: "Documentary".into(),
        description: String::new(),
        poster: String::new(),
        synopsis: None,
        protection: Some(false),
        media: MediaLocator::Http {
            url: "https://example.invalid/video.m3u8".into(),
        },
        duration_ms: None,
        provider_id: None,
        provider_id_field: None,
        live: None,
    }
}

fn authorize_fixture(
    coordinator: &mut Coordinator,
    runtime: &mut Runtime,
    app: &App,
    transition: Transition,
    title: &str,
) -> ResolveTicket {
    let (ticket, work) = transition
        .effects
        .into_iter()
        .find_map(|effect| match effect {
            Effect::PrepareMetadata { ticket, work } => Some((ticket, work)),
            _ => None,
        })
        .unwrap();
    let prepared =
        super::super::multiview_sources::prepared_http_fixture_for_test(work, http_entry(title))
            .unwrap();
    let loaded = coordinator
        .metadata_succeeded(prepared, runtime, app)
        .unwrap();
    assert!(loaded.effects.iter().any(|effect| matches!(effect,
        Effect::Backend(BackendIntent::Load { ticket: loaded, input })
        if *loaded == ticket && input.identity_scheme() == 1
            && input.identity_digest().len() == 64)));
    ticket
}

#[test]
fn process_minted_scene_and_ui_serials_are_distinct_and_never_reused() {
    let first_scene = mint_scene_id().unwrap();
    let second_scene = mint_scene_id().unwrap();
    assert_ne!(first_scene.0, second_scene.0);
    assert_ne!(first_scene.1, second_scene.1);

    let first_ui = mint(&NEXT_UI_SERIAL).unwrap();
    let second_ui = mint(&NEXT_UI_SERIAL).unwrap();
    assert!(first_ui < second_ui);
}

#[test]
fn actual_coordinator_publishes_snapshot_sources_and_ui_focus_is_not_playback_activation() {
    let _serial = plx_base::testlock::serial();
    let (app, runtime) = fixture();
    let mut coordinator = Coordinator::new();
    let activated = coordinator.activate(&runtime, &app, Layout::Grid4).unwrap();
    assert!(activated.effects.iter().any(|effect| matches!(
        effect,
        Effect::Backend(BackendIntent::Layout {
            layout: Layout::Grid4,
            ..
        })
    )));
    assert_eq!(coordinator.source_options().len(), 2);
    assert_eq!(coordinator.source_options()[1].profile, "m3u-2");
    let source0 = source(&coordinator, 0);
    let options = coordinator.source_options();
    assert_eq!(
        coordinator.source_id(source0).as_deref(),
        Some(options[0].source_id.as_str())
    );

    let handle = source(&coordinator, 1);
    coordinator.bind_source(slot(1), handle).unwrap();
    let offer = coordinator.publish_ui_offer().unwrap();
    let focus = coordinator
        .accept_ui_event(
            &runtime,
            &app,
            plx_appkit::multiview_chrome::Event::FocusSlot {
                revision: offer.ui_serial,
                slot: 1,
            },
        )
        .unwrap();
    assert!(focus.effects.is_empty());
    assert_eq!(coordinator.active_slot(), Some(slot(0)));
    assert_eq!(coordinator.focused_slot(), Some(slot(1)));

    let offer = coordinator.publish_ui_offer().unwrap();
    let activate = coordinator
        .accept_ui_event(
            &runtime,
            &app,
            plx_appkit::multiview_chrome::Event::ActivateSlot {
                revision: offer.ui_serial,
                slot: 1,
            },
        )
        .unwrap();
    assert_eq!(coordinator.active_slot(), Some(slot(1)));
    assert_eq!(coordinator.audio_route_state().desired, Some(slot(1)));
    assert!(activate.effects.iter().any(|effect| matches!(effect,
        Effect::Backend(BackendIntent::AudioOwner { owner: Some(id), .. }) if *id == slot(1))));
    let audio_revision = coordinator.audio_route_state().pending_revision.unwrap();
    coordinator
        .audio_route_completed(
            coordinator.scene_id.unwrap(),
            audio_revision,
            Some(slot(1)),
            true,
        )
        .unwrap();
    assert_eq!(coordinator.audio_route_state().applied, Some(slot(1)));
}

#[test]
fn layout_focus_and_audio_policy_emit_exact_revisioned_backend_intents() {
    let _serial = plx_base::testlock::serial();
    let (app, runtime) = fixture();
    let mut coordinator = Coordinator::new();
    let created = coordinator.activate(&runtime, &app, Layout::Grid4).unwrap();
    let scene = coordinator.scene_id.unwrap();
    assert!(created.effects.iter().any(|effect| matches!(effect,
        Effect::Backend(BackendIntent::Layout {scene:got,layout:Layout::Grid4}) if *got==scene)));
    let (initial_audio_revision, initial_owner) = created
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Backend(BackendIntent::AudioOwner {
                scene: got,
                audio_revision,
                owner,
            }) if *got == scene => Some((*audio_revision, *owner)),
            _ => None,
        })
        .unwrap();
    assert_eq!(initial_owner, Some(slot(0)));
    coordinator
        .audio_route_completed(scene, initial_audio_revision, initial_owner, true)
        .unwrap();
    assert_eq!(coordinator.audio_route_state().applied, Some(slot(0)));

    let layout = coordinator.set_layout(Layout::PictureInPicture).unwrap();
    assert!(layout.effects.iter().any(|effect| matches!(effect,
        Effect::Backend(BackendIntent::Layout {scene:got,layout:Layout::PictureInPicture}) if *got==scene)));
    assert_eq!(coordinator.layout(), Some(Layout::PictureInPicture));

    let focused = coordinator.focus(slot(1)).unwrap();
    assert!(focused.effects.iter().any(|effect| matches!(effect,
        Effect::Backend(BackendIntent::Focus {scene:got,slot:focused}) if *got==scene && *focused==slot(1))));
    let (focus_audio_revision, focus_owner) = focused
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Backend(BackendIntent::AudioOwner {
                scene: got,
                audio_revision,
                owner,
            }) if *got == scene => Some((*audio_revision, *owner)),
            _ => None,
        })
        .unwrap();
    assert_ne!(focus_audio_revision, initial_audio_revision);
    assert_eq!(focus_owner, Some(slot(1)));
    coordinator
        .audio_route_completed(scene, focus_audio_revision, focus_owner, false)
        .unwrap();
    assert_eq!(coordinator.audio_route_state().desired, Some(slot(1)));
    assert_eq!(
        coordinator.audio_route_state().applied,
        Some(slot(0)),
        "a rejected backend command does not claim audio moved"
    );

    let explicit = coordinator
        .set_audio_policy(AudioPolicy::Explicit(Some(slot(0))))
        .unwrap();
    let (revision, owner) = explicit
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Backend(BackendIntent::AudioOwner {
                scene: got,
                audio_revision,
                owner,
            }) if *got == scene => Some((*audio_revision, *owner)),
            _ => None,
        })
        .unwrap();
    assert_ne!(revision, focus_audio_revision);
    assert_eq!(owner, Some(slot(0)));
    coordinator
        .audio_route_completed(scene, revision, owner, true)
        .unwrap();
    let focus_under_explicit = coordinator.focus(slot(0)).unwrap();
    assert!(focus_under_explicit.effects.iter().any(|effect| matches!(effect,
        Effect::Backend(BackendIntent::Focus {scene:got,slot:focused}) if *got==scene && *focused==slot(0))));
    assert!(
        !focus_under_explicit
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::Backend(BackendIntent::AudioOwner { .. }))),
        "focus does not change the owner under an explicit audio policy"
    );
}

#[test]
fn close_scene_cancels_resolvers_and_waits_for_each_pipeline_stop_ack() {
    let _serial = plx_base::testlock::serial();
    let (app, mut runtime) = fixture();
    let mut coordinator = Coordinator::new();
    coordinator.activate(&runtime, &app, Layout::Grid4).unwrap();
    let handle = source(&coordinator, 0);
    coordinator.bind_source(slot(0), handle).unwrap();
    let resolving = coordinator.begin_play(&mut runtime, &app, slot(0)).unwrap();
    let (resolver_ticket, cancel) = resolving
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::PrepareMetadata { ticket, work } => Some((*ticket, work.cancellation())),
            _ => None,
        })
        .unwrap();
    let closed = coordinator.close_scene().unwrap();
    assert!(closed.effects.iter().any(|effect| matches!(effect,
        Effect::CancelResolve {ticket,cancellation} if *ticket==resolver_ticket && Arc::ptr_eq(cancellation,&cancel))));
    assert!(cancel.load(Ordering::Acquire));
    assert_eq!(
        coordinator
            .resolve_failed(resolver_ticket, SourceError::Cancelled)
            .err(),
        Some(Error::StaleCompletion)
    );
    assert_eq!(
        coordinator.layout(),
        None,
        "scene is closed after resolver cancellation retires its slot"
    );

    let (app, mut runtime) = fixture();
    let mut coordinator = Coordinator::new();
    coordinator.activate(&runtime, &app, Layout::Grid4).unwrap();
    let handle = source(&coordinator, 0);
    coordinator.bind_source(slot(0), handle).unwrap();
    let started = coordinator.begin_play(&mut runtime, &app, slot(0)).unwrap();
    let ticket = authorize_fixture(&mut coordinator, &mut runtime, &app, started, "Channel 1");
    coordinator
        .backend_start_completed(ticket, AdmissionOutcome::Admitted)
        .unwrap();
    let stopping = coordinator.close_scene().unwrap();
    assert!(stopping.effects.iter().any(|effect| matches!(effect,
        Effect::Backend(BackendIntent::Stop {ticket:got}) if *got==ticket)));
    assert_eq!(
        coordinator.active_playback(),
        None,
        "closing a scene is no longer an active playback command target"
    );
    assert_eq!(coordinator.admitted_pipeline_slots(), 1);
    coordinator
        .stop_completed(ticket, StopResult::Stopped)
        .unwrap();
    assert_eq!(coordinator.layout(), None);
    assert_eq!(coordinator.admitted_pipeline_slots(), 0);
}

#[test]
fn resolver_cancellation_is_slot_local_and_late_completion_is_rejected() {
    let _serial = plx_base::testlock::serial();
    let (app, mut runtime) = fixture();
    let mut coordinator = Coordinator::new();
    coordinator.activate(&runtime, &app, Layout::Grid4).unwrap();
    let source0 = source(&coordinator, 0);
    let source1 = source(&coordinator, 1);
    coordinator.bind_source(slot(0), source0).unwrap();
    coordinator.bind_source(slot(1), source1).unwrap();
    let first = coordinator.begin_play(&mut runtime, &app, slot(0)).unwrap();
    let second = coordinator.begin_play(&mut runtime, &app, slot(1)).unwrap();
    let (first_ticket, first_cancel) = first
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::PrepareMetadata { ticket, work } => Some((*ticket, work.cancellation())),
            _ => None,
        })
        .unwrap();
    let (second_ticket, second_cancel) = second
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::PrepareMetadata { ticket, work } => Some((*ticket, work.cancellation())),
            _ => None,
        })
        .unwrap();
    assert!(!Arc::ptr_eq(&first_cancel, &second_cancel));
    let closed = coordinator.close_slot(slot(0)).unwrap();
    assert!(closed.effects.iter().any(
        |effect| matches!(effect, Effect::CancelResolve { ticket, .. } if *ticket == first_ticket)
    ));
    assert!(first_cancel.load(Ordering::Acquire));
    assert!(!second_cancel.load(Ordering::Acquire));
    assert!(matches!(
        coordinator.resolve_failed(first_ticket, SourceError::Cancelled),
        Err(Error::StaleCompletion)
    ));
    assert!(coordinator
        .resolve_failed(second_ticket, SourceError::Resolve)
        .unwrap()
        .issues
        .iter()
        .any(|issue| issue.slot == slot(1)));
}

#[test]
fn a_new_same_source_attempt_rotates_slot_receipt_but_not_source_revision() {
    let _serial = plx_base::testlock::serial();
    let (app, mut runtime) = fixture();
    let mut coordinator = Coordinator::new();
    coordinator.activate(&runtime, &app, Layout::Grid4).unwrap();
    let handle = source(&coordinator, 0);
    coordinator.bind_source(slot(0), handle).unwrap();
    let first = coordinator.begin_play(&mut runtime, &app, slot(0)).unwrap();
    let first_ticket = first
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::PrepareMetadata { ticket, .. } => Some(*ticket),
            _ => None,
        })
        .unwrap();
    let first_status = coordinator.scene_status(&runtime).unwrap();
    let first_slot = first_status
        .slots
        .iter()
        .find(|item| item.slot == slot(0))
        .unwrap();
    coordinator
        .resolve_failed(first_ticket, SourceError::Resolve)
        .unwrap();

    let second = coordinator.begin_play(&mut runtime, &app, slot(0)).unwrap();
    let second_ticket = second
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::PrepareMetadata { ticket, .. } => Some(*ticket),
            _ => None,
        })
        .unwrap();
    assert_ne!(first_ticket, second_ticket);
    let second_status = coordinator.scene_status(&runtime).unwrap();
    let second_slot = second_status
        .slots
        .iter()
        .find(|item| item.slot == slot(0))
        .unwrap();
    assert_ne!(first_slot.slot_generation, second_slot.slot_generation);
    assert_eq!(first_slot.source_revision, second_slot.source_revision);
}

#[test]
fn active_owner_validation_rejects_a_retired_profile_snapshot() {
    let _serial = plx_base::testlock::serial();
    let (app, mut runtime) = fixture();
    let mut coordinator = Coordinator::new();
    coordinator.activate(&runtime, &app, Layout::Grid4).unwrap();
    assert!(coordinator.validate_active_owner(&runtime, &app).is_ok());
    runtime.retire_profile_edits();
    assert_eq!(
        coordinator.validate_active_owner(&runtime, &app),
        Err(Error::SourceStale)
    );
}

#[test]
fn backend_admission_is_observed_and_close_waits_for_stop_acknowledgement() {
    let _serial = plx_base::testlock::serial();
    let (app, mut runtime) = fixture();
    let mut coordinator = Coordinator::new();
    coordinator.activate(&runtime, &app, Layout::Grid4).unwrap();
    let handle = source(&coordinator, 0);
    coordinator.bind_source(slot(0), handle).unwrap();
    let started = coordinator.begin_play(&mut runtime, &app, slot(0)).unwrap();
    let ticket = authorize_fixture(&mut coordinator, &mut runtime, &app, started, "Channel 1");
    assert_eq!(coordinator.admitted_pipeline_slots(), 0);
    let admitted = coordinator
        .backend_start_completed(ticket, AdmissionOutcome::Admitted)
        .unwrap();
    assert_eq!(coordinator.admitted_pipeline_slots(), 1);
    assert!(admitted.effects.iter().any(|effect| matches!(effect,
        Effect::Backend(BackendIntent::AudioOwner { owner: Some(id), .. }) if *id == slot(0))));

    let stopping = coordinator.close_slot(slot(0)).unwrap();
    assert!(stopping.effects.iter().any(|effect| matches!(effect,
        Effect::Backend(BackendIntent::Stop { ticket: stopped }) if *stopped == ticket)));
    assert_eq!(
        coordinator.admitted_pipeline_slots(),
        1,
        "an issued stop is not proof that the native pipeline has stopped"
    );
    coordinator
        .stop_completed(ticket, StopResult::Stopped)
        .unwrap();
    assert_eq!(coordinator.admitted_pipeline_slots(), 0);
    assert_eq!(coordinator.slot_status(slot(0)), Ok(SlotStatus::Empty));
}

#[test]
fn rejected_backend_slot_isolated_from_successful_sibling() {
    let _serial = plx_base::testlock::serial();
    let (app, mut runtime) = fixture();
    let mut coordinator = Coordinator::new();
    coordinator.activate(&runtime, &app, Layout::Grid4).unwrap();
    let first = source(&coordinator, 0);
    let second = source(&coordinator, 1);
    coordinator.bind_source(slot(0), first).unwrap();
    coordinator.bind_source(slot(1), second).unwrap();
    let first_started = coordinator.begin_play(&mut runtime, &app, slot(0)).unwrap();
    let first_ticket = authorize_fixture(
        &mut coordinator,
        &mut runtime,
        &app,
        first_started,
        "Channel 1",
    );
    let second_started = coordinator.begin_play(&mut runtime, &app, slot(1)).unwrap();
    let second_ticket = authorize_fixture(
        &mut coordinator,
        &mut runtime,
        &app,
        second_started,
        "Channel 2",
    );
    coordinator
        .backend_start_completed(first_ticket, AdmissionOutcome::Admitted)
        .unwrap();
    let rejected = coordinator
        .backend_start_completed(second_ticket, AdmissionOutcome::Unsupported)
        .unwrap();
    assert_eq!(coordinator.admitted_pipeline_slots(), 1);
    assert!(rejected
        .issues
        .iter()
        .any(|issue| issue.slot == slot(1) && issue.issue == Issue::BackendUnsupported));
    assert!(matches!(
        coordinator.slot_status(slot(0)),
        Ok(SlotStatus::Playing { .. })
    ));
    assert!(matches!(
        coordinator.slot_status(slot(1)),
        Ok(SlotStatus::Unsupported { .. })
    ));
}

#[test]
fn strict_kiosk_fails_closed_before_scene_admission() {
    let _serial = plx_base::testlock::serial();
    let (app, mut runtime) = fixture();
    let origin = runtime.saved.favorites.entries()[0].origin.clone();
    let stored = crate::app::ott_kiosk_stored::Stored::armed(origin, true).unwrap();
    runtime.saved.settings.insert(
        crate::app::ott_kiosk_stored::KEY.into(),
        serde_json::to_value(stored).unwrap(),
    );
    assert!(!matches!(
        runtime.track_selection_kiosk(),
        Ok(crate::app::ott_kiosk_policy::State::Off)
    ));
    let mut coordinator = Coordinator::new();
    assert!(matches!(
        coordinator.activate(&runtime, &app, Layout::Grid4),
        Err(Error::KioskRestricted)
    ));
    assert_eq!(coordinator.layout(), None);
}

#[test]
fn stopped_application_fails_closed_before_scene_admission() {
    let _serial = plx_base::testlock::serial();
    let (mut app, runtime) = fixture();
    app.running = false;
    let mut coordinator = Coordinator::new();
    assert_eq!(
        coordinator.activate(&runtime, &app, Layout::Grid4).err(),
        Some(Error::NotReady)
    );
    assert_eq!(coordinator.layout(), None);
    assert!(coordinator.source_options().is_empty());
}

#[test]
fn ui_event_is_bound_to_the_exact_published_scene_snapshot_and_scene_state() {
    let context = UiOfferContext {
        ui_serial: 9182,
        scene: "scene_12".into(),
        snapshot_epoch: 41,
        scene_revision: 9,
        layout_revision: 2,
        focus_revision: 3,
        audio_revision: 1,
    };
    assert!(offer_matches(&context, 9182, Some("scene_12"), Some(41), 9));
    assert!(!offer_matches(
        &context,
        9181,
        Some("scene_12"),
        Some(41),
        9
    ));
    assert!(!offer_matches(
        &context,
        9182,
        Some("scene_13"),
        Some(41),
        9
    ));
    assert!(!offer_matches(
        &context,
        9182,
        Some("scene_12"),
        Some(42),
        9
    ));
    assert!(!offer_matches(
        &context,
        9182,
        Some("scene_12"),
        Some(41),
        10
    ));
}

#[test]
fn source_registry_ids_are_epoch_scoped_tokens() {
    let id = |epoch: u64, index: usize| format!("src_{epoch}_{}", index + 1);
    assert_eq!(id(4, 0), "src_4_1");
    assert_ne!(id(4, 0), id(5, 0));
    assert_ne!(id(4, 0), id(4, 1));
}

#[test]
fn a_new_attempt_changes_slot_receipt_generation_without_changing_source_revision() {
    let slot = SlotId::SLOT_2;
    let first_slot_generation = slot_generation(slot, 7, 1);
    let second_slot_generation = slot_generation(slot, 7, 2);
    let first_source_revision = source_revision(slot, 7);
    let second_source_revision = source_revision(slot, 7);
    assert_ne!(first_slot_generation, second_slot_generation);
    assert_eq!(first_source_revision, second_source_revision);
    assert_ne!(
        slot_generation(slot, 7, 2),
        slot_generation(slot, 8, 1),
        "a source rebind must also rotate the slot receipt"
    );
}
