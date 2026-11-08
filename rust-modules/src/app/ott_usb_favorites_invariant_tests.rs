//! Cross-feature Saved invariants, without a protected writer or media decoder.
//! USB observations are injected into the real held-directory worker; transient
//! playback uses the existing tagged native-admission fixture. UUID allocation
//! uses the production generator. Migration persistence is an explicit callback.
use super::*;
use plx_data::ott::favorites::{Favorite, Locator, Origin};
use plx_data::ott::favorites_wire::{DurableStatus, Outcome};

struct ResetPlayback;
impl Drop for ResetPlayback {
    fn drop(&mut self) {
        plx_media::player::SHARED.reset_session();
        plx_media::player::TX.reset();
        plx_media::route::reset_player_control_for_test(&plx_media::route::PlaybackSession::IDLE);
    }
}

fn add_favorite(saved: &mut Saved) {
    saved
        .favorites
        .toggle(Favorite {
            title: "Retained television".into(),
            origin: Origin {
                provider: "m3u".into(),
                profile: "Permanent".into(),
                subprofile: None,
                source_alias: "Permanent".into(),
            },
            locator: Locator::Channel {
                channel_id: "live".into(),
            },
        })
        .unwrap();
}

fn allocate_ids(saved: &mut Saved) {
    assert!(favorite_item_ids::prepare(saved, &OffFrame::for_test()).unwrap());
    let ids = favorite_item_ids::ordered(saved).unwrap();
    assert_eq!(ids.len(), 1);
    assert!(favorite_item_ids::uuid(&ids[0]));
}

fn image(saved: &Saved) -> Value {
    serde_json::to_value(saved).unwrap()
}

#[test]
fn unknown_favorites_receipt_refuses_real_usb_relocation_without_losing_ids() {
    let _serial = plx_base::testlock::serial();
    let directory = Directory::new();
    let mut app = app(); // Capture Runtime's native ticket after App initialization.
    let mut runtime = runtime();
    add_favorite(&mut runtime.saved);
    allocate_ids(&mut runtime.saved);
    let operation_runtime = runtime.remote.runtime().to_owned();
    runtime.saved.receipts.push(DurableStatus {
        operation_id: "00000000-0000-4000-8000-000000000001".into(),
        operation_runtime: Some(operation_runtime),
        transaction_id: Some("00000000-0000-4000-8000-000000000002".into()),
        outcome: Outcome::Unknown,
        state: "unknown".into(),
        receipt: None,
    });
    let before = image(&runtime.saved);
    let ids = favorite_item_ids::ordered(&runtime.saved).unwrap();
    assert!(runtime.settings_write_refusal().is_none());
    assert!(playback_allowed(&app) && runtime.source_work_allowed());
    let (_, source, relocation) = receive(&mut runtime, &directory, |_| {});
    assert_ne!(
        relocation.old_root().canonical_path,
        source.descriptor().canonical_path
    );
    assert!(runtime.local_inventory.usb.waiting);
    let generation = runtime.generation;
    runtime.usb_result(
        &mut app,
        generation,
        Ok(usb::Answer::Restored { source, relocation }),
    );
    assert_eq!(
        runtime.local_inventory.usb.error,
        Some("A Favorites operation needs reconciliation")
    );
    assert_eq!(image(&runtime.saved), before);
    assert_eq!(favorite_item_ids::ordered(&runtime.saved).unwrap(), ids);
    assert_eq!(runtime.saved.receipts[0].outcome, Outcome::Unknown);
    assert!(runtime.local_inventory.sources.is_empty());
    assert!(runtime.local_inventory.usb.install.is_none());
    assert!(!runtime.saving);
    assert!(
        runtime.rx.try_recv().is_err(),
        "no protected writer was dispatched"
    );
}

#[test]
fn successful_id_migration_preserves_dormant_usb_source_selection_and_resume() {
    let _serial = plx_base::testlock::serial();
    let _app = app();
    let mut runtime = runtime();
    let permanent = ProviderProfile {
        id: "permanent".into(),
        name: "Permanent".into(),
        kind: ProviderKind::M3u,
        source: "https://media.invalid/list.m3u".into(),
        source_alias: "Permanent".into(),
        enabled: true,
    };
    runtime.saved.profiles.profiles.push(permanent);
    runtime.saved.profiles.active_profile = Some("permanent".into());
    runtime.provider = ProviderKind::M3u;
    runtime.catalog_next = None;
    runtime.catalog_loading = false;
    add_favorite(&mut runtime.saved);
    assert!(runtime.saved.favorite_items.is_none());
    let before = image(&runtime.saved);
    let mut persisted = None;
    let (migrated, uncertain) = favorite_item_ids::migrate_with(
        runtime.saved.clone(),
        |saved| favorite_item_ids::prepare(saved, &OffFrame::for_test()),
        |saved| {
            persisted = Some(image(saved));
            Ok(())
        },
    );
    assert!(!uncertain);
    let mut after = image(&migrated);
    assert_eq!(persisted.as_ref(), Some(&after));
    assert!(after
        .as_object_mut()
        .unwrap()
        .remove("favorite_items")
        .is_some());
    assert_eq!(
        after, before,
        "migration changes only receiver row metadata"
    );
    assert_eq!(
        migrated.profiles.active_profile.as_deref(),
        Some("permanent")
    );
    assert_eq!(migrated.resume["local-usb-1"].position_ms, 12_345);
    assert_eq!(favorite_item_ids::ordered(&migrated).unwrap().len(), 1);
    assert_eq!(
        image(&runtime.saved),
        before,
        "worker result awaits normal Loaded admission"
    );
    assert!(runtime.catalog_next.is_none() && runtime.play_target.is_none());
    assert!(runtime.transient.accepted.is_none());
    assert!(runtime.local_inventory.sources.is_empty());
}

#[test]
fn temporary_local_playback_checkpoint_and_discard_preserve_global_favorites_ids() {
    let _serial = plx_base::testlock::serial();
    let _reset = ResetPlayback;
    let (_directory, mut app, root) = crate::app::ott_runtime::tests::local_integration::setup();
    {
        let runtime = app.ott.as_mut().unwrap();
        add_favorite(&mut runtime.saved);
        allocate_ids(&mut runtime.saved);
        runtime.saving = false;
    }
    let before = image(&app.ott.as_ref().unwrap().saved);
    let ids = favorite_item_ids::ordered(&app.ott.as_ref().unwrap().saved).unwrap();
    crate::app::ott_runtime::tests::local_integration::install(&mut app, Some(&root), "first");
    plx_media::player::SHARED
        .playpos_ns
        .store(12_500_000_000, Ordering::Release);
    let mut runtime = app.ott.take().unwrap();
    assert!(playing_matches(runtime.playing.as_ref().unwrap(), &app));
    runtime.checkpoint(&app, true);
    let Some(transient::TransientMedia::Local(session)) = runtime.transient.accepted.as_ref()
    else {
        panic!("accepted temporary Local source");
    };
    assert_eq!(session.context.position_ms(), 12_500);
    assert!(runtime.checkpoints.is_empty());
    assert_eq!(image(&runtime.saved), before);
    assert!(transient::set_view(&mut runtime, "", true));
    assert!(transient::set_view(&mut runtime, "", false));
    transient::note_stop(&mut runtime);
    transient::discard(&mut runtime);
    assert!(runtime.transient.accepted.is_none());
    assert_eq!(image(&runtime.saved), before);
    assert_eq!(favorite_item_ids::ordered(&runtime.saved).unwrap(), ids);
    assert_eq!(
        runtime.saved.profiles.active_profile.as_deref(),
        Some("permanent")
    );
    assert!(!runtime.saving && runtime.rx.try_recv().is_err());
    app.ott = Some(runtime);
}
