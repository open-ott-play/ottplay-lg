use super::*;
use std::num::NonZeroU64;

fn source(token: u64) -> SourceRef {
    SourceRef::from_opaque(NonZeroU64::new(token).unwrap())
}

fn scene_id(token: u64) -> SceneId {
    SceneId::from_opaque(NonZeroU64::new(token).unwrap())
}

fn media(token: u64) -> MediaRef {
    MediaRef::from_opaque(NonZeroU64::new(token).unwrap())
}

fn loaded_scene(slot: SlotId, src: SourceRef, item: MediaRef) -> (Scene, ResolveTicket) {
    let mut scene = Scene::new(scene_id(1));
    scene.set_layout(Layout::Grid4).unwrap();
    scene.bind_source(slot, src).unwrap();
    let ticket = scene.begin_resolution(slot, src, item).unwrap();
    scene
        .complete_resolution(ticket, ResolveOutcome::Resolved)
        .unwrap();
    (scene, ticket)
}

#[test]
fn layout_geometry_is_not_backend_capacity_and_slots_are_stable() {
    assert_eq!(SLOT_COUNT, 9);
    assert_eq!(SlotId::new(8), Some(SlotId::SLOT_8));
    assert_eq!(SlotId::new(9), None);
    assert_eq!(Layout::Single.visible_capacity(), 1);
    assert_eq!(Layout::PictureInPicture.visible_capacity(), 2);
    assert_eq!(Layout::Grid4.visible_capacity(), 4);
    assert_eq!(Layout::Grid9.visible_capacity(), 9);
    assert_eq!(Layout::perimeter(0).unwrap().visible_capacity(), 1);
    assert_eq!(Layout::perimeter(8).unwrap().visible_capacity(), 9);
    assert_eq!(Layout::perimeter(9), None);

    let mut scene = Scene::new(scene_id(2));
    assert_eq!(scene.admitted_pipeline_slots(), 0);
    scene.set_layout(Layout::Grid9).unwrap();
    scene.bind_source(SlotId::SLOT_8, source(8)).unwrap();
    let ticket = scene
        .begin_resolution(SlotId::SLOT_8, source(8), media(8))
        .unwrap();
    scene
        .complete_resolution(ticket, ResolveOutcome::Resolved)
        .unwrap();
    assert_eq!(scene.admitted_pipeline_slots(), 0);
    scene
        .complete_admission(ticket, AdmissionOutcome::Admitted)
        .unwrap();
    assert_eq!(scene.admitted_pipeline_slots(), 1);

    let perimeter = Layout::perimeter(8).unwrap();
    assert_eq!(
        perimeter.visible_slots().collect::<Vec<_>>(),
        (0..9).map(|i| SlotId::new(i).unwrap()).collect::<Vec<_>>()
    );
    assert_eq!(
        scene.set_layout(Layout::Perimeter { satellites: 9 }),
        Err(Error::InvalidLayout)
    );
}

#[test]
fn old_scene_completion_cannot_attach_to_a_new_scene_with_matching_slot_counters() {
    let src = source(1);
    let item = media(10);
    let mut old_scene = Scene::new(scene_id(101));
    old_scene.bind_source(SlotId::SLOT_0, src).unwrap();
    let old_ticket = old_scene
        .begin_resolution(SlotId::SLOT_0, src, item)
        .unwrap();

    let mut new_scene = Scene::new(scene_id(102));
    new_scene.bind_source(SlotId::SLOT_0, src).unwrap();
    let new_ticket = new_scene
        .begin_resolution(SlotId::SLOT_0, src, item)
        .unwrap();
    assert_eq!(
        (old_ticket.slot(), old_ticket.source()),
        (new_ticket.slot(), new_ticket.source())
    );
    assert_eq!(
        new_scene.complete_resolution(old_ticket, ResolveOutcome::Resolved),
        Err(Error::StaleCompletion)
    );
    new_scene
        .complete_resolution(new_ticket, ResolveOutcome::Resolved)
        .unwrap();
}

#[test]
fn a_b_a_source_rebinding_rejects_old_completion() {
    let mut scene = Scene::new(scene_id(3));
    scene.set_layout(Layout::PictureInPicture).unwrap();
    let a = source(1);
    let b = source(2);
    let item = media(10);
    scene.bind_source(SlotId::SLOT_0, a).unwrap();
    let old = scene.begin_resolution(SlotId::SLOT_0, a, item).unwrap();
    scene.close_media(SlotId::SLOT_0).unwrap();
    scene.bind_source(SlotId::SLOT_0, b).unwrap();
    scene.bind_source(SlotId::SLOT_0, a).unwrap();
    assert_eq!(
        scene.complete_resolution(old, ResolveOutcome::Resolved),
        Err(Error::StaleCompletion)
    );
}

#[test]
fn sibling_resolution_and_completion_are_independent() {
    let mut scene = Scene::new(scene_id(4));
    scene.set_layout(Layout::Grid4).unwrap();
    let a = source(1);
    let b = source(2);
    scene.bind_source(SlotId::SLOT_0, a).unwrap();
    scene.bind_source(SlotId::SLOT_1, b).unwrap();
    let first = scene
        .begin_resolution(SlotId::SLOT_0, a, media(10))
        .unwrap();
    let second = scene
        .begin_resolution(SlotId::SLOT_1, b, media(20))
        .unwrap();
    let replacement = scene
        .begin_resolution(SlotId::SLOT_0, a, media(11))
        .unwrap();

    assert_eq!(
        scene.complete_resolution(first, ResolveOutcome::Resolved),
        Err(Error::StaleCompletion)
    );
    scene
        .complete_resolution(second, ResolveOutcome::Resolved)
        .unwrap();
    scene
        .complete_resolution(replacement, ResolveOutcome::Resolved)
        .unwrap();
    assert_eq!(
        scene.slot_status(SlotId::SLOT_1),
        Ok(SlotStatus::Resolved { media: media(20) })
    );
    assert_eq!(
        scene.slot_status(SlotId::SLOT_0),
        Ok(SlotStatus::Resolved { media: media(11) })
    );
}

#[test]
fn focus_and_source_revisions_are_separate() {
    let mut scene = Scene::new(scene_id(5));
    scene.set_layout(Layout::Grid4).unwrap();
    let before = scene.revisions();
    assert_eq!(before.layout, 1);
    scene.focus(SlotId::SLOT_2).unwrap();
    let after_focus = scene.revisions();
    assert_eq!(after_focus.focus, before.focus + 1);
    assert_eq!(after_focus.audio, before.audio + 1);
    assert_eq!(after_focus.layout, before.layout);
    assert_eq!(after_focus.source, before.source);
    assert_eq!(after_focus.scene, before.scene + 1);

    scene.bind_source(SlotId::SLOT_2, source(1)).unwrap();
    let after_source = scene.revisions();
    assert_eq!(after_source.source, after_focus.source + 1);
    assert_eq!(after_source.focus, after_focus.focus);
    assert_eq!(after_source.layout, after_focus.layout);
    assert_eq!(after_source.audio, after_focus.audio);
    assert_eq!(after_source.scene, after_focus.scene + 1);

    scene.set_layout(Layout::PictureInPicture).unwrap();
    let after_layout = scene.revisions();
    assert_eq!(after_layout.layout, after_source.layout + 1);
    // PiP hides slot 2: focus and follows-focus audio must move to visible slot 0.
    assert_eq!(after_layout.focus, after_source.focus + 1);
    assert_eq!(after_layout.audio, after_source.audio + 1);
    assert_eq!(scene.active_slot(), SlotId::SLOT_0);
    assert_eq!(after_layout.source, after_source.source);
}

#[test]
fn explicit_audio_owner_has_its_own_revision_and_does_not_follow_focus() {
    let mut scene = Scene::new(scene_id(10));
    scene.set_layout(Layout::Grid4).unwrap();
    scene
        .set_audio_policy(AudioPolicy::Explicit(Some(SlotId::SLOT_1)))
        .unwrap();
    let before_focus = scene.revisions();
    scene.focus(SlotId::SLOT_2).unwrap();
    let after_focus = scene.revisions();
    assert_eq!(after_focus.audio, before_focus.audio);
    assert_eq!(scene.audio_owner(), Some(SlotId::SLOT_1));
    scene.set_audio_policy(AudioPolicy::Explicit(None)).unwrap();
    assert_eq!(scene.audio_owner(), None);
    assert_eq!(scene.revisions().audio, after_focus.audio + 1);
}

#[test]
fn only_one_audio_owner_exists_and_focus_policy_has_no_fallback() {
    let (mut scene, ticket) = loaded_scene(SlotId::SLOT_1, source(1), media(10));
    scene
        .complete_admission(ticket, AdmissionOutcome::Admitted)
        .unwrap();

    assert_eq!(scene.audio_owner(), Some(SlotId::SLOT_0));
    scene.focus(SlotId::SLOT_1).unwrap();
    assert_eq!(scene.audio_owner(), Some(SlotId::SLOT_1));
    scene.focus(SlotId::SLOT_2).unwrap();
    assert_eq!(scene.audio_owner(), Some(SlotId::SLOT_2));
    assert_eq!(scene.admitted_pipeline_slots(), 1);

    scene
        .set_audio_policy(AudioPolicy::Explicit(Some(SlotId::SLOT_1)))
        .unwrap();
    scene.focus(SlotId::SLOT_3).unwrap();
    assert_eq!(scene.audio_owner(), Some(SlotId::SLOT_1));
    scene.set_audio_policy(AudioPolicy::Explicit(None)).unwrap();
    assert_eq!(scene.audio_owner(), None);
}

#[test]
fn strict_kiosk_allows_only_the_exact_pinned_slot_source_and_item() {
    let mut scene = Scene::new(scene_id(6));
    scene.set_layout(Layout::Grid4).unwrap();
    let pinned_source = source(1);
    let pinned_media = media(10);
    let pin = StrictKioskPin::new(SlotId::SLOT_0, pinned_source, pinned_media);
    scene.enable_strict_kiosk(pin).unwrap();
    scene.bind_source(SlotId::SLOT_0, pinned_source).unwrap();
    assert_eq!(
        scene.bind_source(SlotId::SLOT_1, source(2)),
        Err(Error::KioskRestricted)
    );
    assert_eq!(
        scene.begin_resolution(SlotId::SLOT_0, pinned_source, media(11)),
        Err(Error::KioskRestricted)
    );
    let ticket = scene
        .begin_resolution(SlotId::SLOT_0, pinned_source, pinned_media)
        .unwrap();
    assert_eq!(
        scene.close_media(SlotId::SLOT_0),
        Err(Error::KioskRestricted)
    );
    scene
        .complete_resolution(ticket, ResolveOutcome::Resolved)
        .unwrap();

    // A real backend failure may retire its own pipeline without disabling the kiosk pin.
    scene.report_pipeline_stopped(ticket).unwrap();
    assert_eq!(scene.strict_kiosk(), Some(pin));
    assert_eq!(scene.slot_source(SlotId::SLOT_0), Ok(Some(pinned_source)));
}

#[test]
fn stopped_strict_kiosk_slot_remains_visible_across_layout_changes() {
    let mut scene = Scene::new(scene_id(9));
    scene.set_layout(Layout::Grid4).unwrap();
    let src = source(4);
    let item = media(40);
    let pin = StrictKioskPin::new(SlotId::SLOT_3, src, item);
    scene.enable_strict_kiosk(pin).unwrap();
    scene.bind_source(SlotId::SLOT_3, src).unwrap();
    let ticket = scene.begin_resolution(SlotId::SLOT_3, src, item).unwrap();
    scene
        .complete_resolution(ticket, ResolveOutcome::Resolved)
        .unwrap();
    scene.report_pipeline_stopped(ticket).unwrap();
    assert_eq!(scene.slot_status(SlotId::SLOT_3), Ok(SlotStatus::Empty));
    assert_eq!(
        scene.set_layout(Layout::Single),
        Err(Error::WouldHideKioskSlot)
    );
    assert_eq!(scene.layout(), Layout::Grid4);
}

#[test]
fn strict_kiosk_refuses_to_adopt_a_conflicting_existing_scene() {
    let (mut scene, _) = loaded_scene(SlotId::SLOT_1, source(2), media(20));
    assert_eq!(
        scene.enable_strict_kiosk(StrictKioskPin::new(SlotId::SLOT_0, source(1), media(10))),
        Err(Error::KioskConflict)
    );
}

#[test]
fn layout_cannot_hide_active_slots_or_explicit_audio_owner() {
    let (mut scene, ticket) = loaded_scene(SlotId::SLOT_3, source(3), media(30));
    scene
        .complete_admission(ticket, AdmissionOutcome::Admitted)
        .unwrap();
    assert_eq!(
        scene.set_layout(Layout::Single),
        Err(Error::WouldHideActiveSlot)
    );

    scene.report_pipeline_stopped(ticket).unwrap();
    scene
        .set_audio_policy(AudioPolicy::Explicit(Some(SlotId::SLOT_3)))
        .unwrap();
    assert_eq!(
        scene.set_layout(Layout::Single),
        Err(Error::WouldHideAudioOwner)
    );
}

#[test]
fn backend_unsupported_is_reported_per_slot_without_claiming_scene_support() {
    let mut scene = Scene::new(scene_id(7));
    scene.set_layout(Layout::Grid4).unwrap();
    scene.bind_source(SlotId::SLOT_0, source(1)).unwrap();
    scene.bind_source(SlotId::SLOT_1, source(2)).unwrap();
    let first = scene
        .begin_resolution(SlotId::SLOT_0, source(1), media(10))
        .unwrap();
    let second = scene
        .begin_resolution(SlotId::SLOT_1, source(2), media(20))
        .unwrap();
    scene
        .complete_resolution(first, ResolveOutcome::Resolved)
        .unwrap();
    scene
        .complete_resolution(second, ResolveOutcome::Resolved)
        .unwrap();
    scene
        .complete_admission(first, AdmissionOutcome::Admitted)
        .unwrap();
    scene
        .complete_admission(second, AdmissionOutcome::Unsupported)
        .unwrap();
    assert_eq!(
        scene.slot_status(SlotId::SLOT_0),
        Ok(SlotStatus::Playing { media: media(10) })
    );
    assert_eq!(
        scene.slot_status(SlotId::SLOT_1),
        Ok(SlotStatus::Unsupported { media: media(20) })
    );
    assert_eq!(scene.admitted_pipeline_slots(), 1);
}

#[test]
fn opaque_refs_and_tickets_do_not_expose_source_values_in_debug() {
    let src = source(987654321);
    let item = media(123456789);
    let mut scene = Scene::new(scene_id(8));
    scene.bind_source(SlotId::SLOT_0, src).unwrap();
    let ticket = scene.begin_resolution(SlotId::SLOT_0, src, item).unwrap();
    let debug = format!("{src:?} {item:?} {ticket:?}");
    assert!(!debug.contains("987654321"));
    assert!(!debug.contains("123456789"));
}
