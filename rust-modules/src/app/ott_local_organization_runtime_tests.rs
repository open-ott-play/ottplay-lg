//! Permanent Local organization holds the original read capability through the
//! editor and Saved ACK. Roots and rejected writers reuse the existing fixtures.
use super::*;
use crate::app::ott_runtime::tests::local_integration;
use plx_data::ott::{catalog_from_local_files, LocalFile};

fn local_fixture(permanent: bool) -> (local_integration::TempDir, Box<App>) {
    let (directory, mut app, _) = local_integration::setup();
    app.running = true;
    bridge::show_page(&mut app.pages, AppArg::Ott);
    settle(&mut app);
    let mut runtime = app.ott.take().unwrap();
    runtime.sync_source_lifecycle(&mut app);
    let profile = runtime.effective_profile().unwrap().clone();
    if permanent {
        runtime.transient.accepted = None;
        runtime.saved.profiles.profiles.push(profile.clone());
        runtime.saved.profiles.active_profile = Some(profile.id.clone());
        runtime.provider = ProviderKind::Local;
    }
    runtime.catalog = catalog_from_local_files(&[
        LocalFile { relative_path: "Films/one.mp4".into(), size: 1, duration_ms: None },
        LocalFile { relative_path: "Films/two.mp4".into(), size: 1, duration_ms: None },
        LocalFile { relative_path: "Clips/three.mp4".into(), size: 1, duration_ms: None },
    ], &profile).unwrap();
    runtime.catalog_complete = true;
    runtime.catalog_loading = false;
    runtime.saving = false;
    runtime.entropy = Some([7; 16]);
    runtime.page = "channels";
    runtime.reply = app.pages.top_page();
    runtime.saved.category.clear();
    runtime.saved.favorites_selected = false;
    runtime.selected = Some(runtime.catalog.entries[0].id.clone());
    runtime.plex_refresh_pending = true;
    runtime.write_permit.revoke();
    runtime.sync_saved_context();
    app.ott = Some(runtime);
    (directory, app)
}

fn replace_or_revoke(runtime: &mut Runtime, replacement: bool) {
    if replacement {
        let descriptor = runtime.local_inventory.sources[0].descriptor().clone();
        let source = crate::app::ott_local_root::LocalSource::acquire_authorized(
            &plx_base::task::OffFrame::for_test(), descriptor, &mut || true,
        ).unwrap();
        runtime.local_inventory.install(vec![source], runtime.generation);
    } else {
        runtime.local_inventory.sources[0].withdraw();
    }
}

fn begin_change(app: &mut App, operation: Operation) -> Saved {
    let saved = candidate(app, operation.clone());
    let runtime = app.ott.as_mut().unwrap();
    let session = runtime.organization.session.clone().unwrap();
    runtime.organization_apply(&session, operation).unwrap();
    drop(take_writer(app));
    saved
}

#[test]
fn permanent_local_manage_commits_create_rename_and_logical_membership_only_after_ack() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = local_fixture(true);
    let before = app.ott.as_ref().unwrap().saved.clone();
    let raw = app.ott.as_ref().unwrap().catalog.clone();
    let selected = app.ott.as_ref().unwrap().selected.clone();
    let playback_epoch = app.ott.as_ref().unwrap().playback_epoch;
    assert!(app.ott.as_ref().unwrap().organization_available());
    open(&mut app);
    let (saved, _) = text_save(&mut app, "create", "Virtual", Operation::Create { label: "Virtual".into() });
    assert!(!app.ott.as_ref().unwrap().organization_categories().iter().any(|row| row.title == "Virtual"));
    durable(&mut app, saved);
    let category = select_category(&mut app, "Virtual");
    let (saved, _) = text_save(&mut app, "rename", "My films", Operation::Rename {
        category: category.clone(), label: "My films".into(),
    });
    durable(&mut app, saved);
    let item = raw.entries[2].id.clone();
    let saved = begin_change(&mut app, Operation::AddMember { category: category.clone(), item: item.clone() });
    assert!(app.ott.as_ref().unwrap().organization_projection().unwrap().category(&category).unwrap().members.is_empty());
    durable(&mut app, saved);
    assert_eq!(app.ott.as_ref().unwrap().organization_projection().unwrap().category(&category).unwrap().members, [item.clone()]);
    let saved = begin_change(&mut app, Operation::RemoveMember { category: category.clone(), item });
    durable(&mut app, saved);
    let runtime = app.ott.as_ref().unwrap();
    assert!(runtime.organization_projection().unwrap().category(&category).unwrap().members.is_empty());
    assert_eq!(runtime.saved.profiles, before.profiles);
    assert_eq!(runtime.saved.category, before.category);
    assert_eq!(runtime.saved.favorites, before.favorites);
    assert_eq!(runtime.catalog, raw);
    assert_eq!(runtime.selected, selected);
    assert_eq!(runtime.playback_epoch, playback_epoch);
    assert!(runtime.play_target.is_none());
}

#[test]
fn local_grant_revocation_or_same_descriptor_replacement_rejects_an_open_editor() {
    let _serial = plx_base::testlock::serial();
    for replacement in [false, true] {
        let (_directory, mut app) = local_fixture(true);
        open(&mut app);
        scoped(&mut app, "create");
        let mut runtime = app.ott.take().unwrap();
        let editor = runtime.edit_session.clone().unwrap();
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        replace_or_revoke(&mut runtime, replacement);
        assert!(runtime.organization_view().is_none());
        assert!(handle_action(&mut runtime, &mut app, OttAction::SubmitSetting {
            session: editor.id, key: editor.key, value: "Stale editor".into(),
        }).is_err());
        assert!(!runtime.saving && runtime.organization.pending.is_none());
        assert!(runtime.rx.try_recv().is_err());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
        app.ott = Some(runtime);
    }
}

#[test]
fn local_grant_replacement_does_not_reuse_a_captured_candidate_context() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = local_fixture(true);
    open(&mut app);
    let runtime = app.ott.as_mut().unwrap();
    let old = runtime.organization.session.clone().unwrap();
    let generation = runtime.generation;
    let revision = runtime.catalog_revision;
    replace_or_revoke(runtime, true);
    assert_eq!(runtime.generation, generation);
    assert_eq!(runtime.catalog_revision, revision);
    assert!(runtime.organization_candidate(&old, Operation::Create { label: "Old grant".into() }).is_err());
    assert!(runtime.organization_apply(&old, Operation::Create { label: "Old grant".into() }).is_err());
    assert!(!runtime.saving && runtime.rx.try_recv().is_err());
    runtime.organization_open().unwrap();
    assert!(runtime.organization_view().is_some(), "a newly captured live grant can open a new editor");
}

#[test]
fn local_pending_view_and_saved_ack_cannot_revive_a_revoked_or_replaced_grant() {
    let _serial = plx_base::testlock::serial();
    for replacement in [false, true] {
        let (_directory, mut app) = local_fixture(true);
        open(&mut app);
        let saved = begin_change(&mut app, Operation::Create { label: "Admitted".into() });
        let runtime = app.ott.as_mut().unwrap();
        assert!(runtime.organization_view().unwrap().pending);
        let selected = runtime.selected.clone();
        let raw = runtime.catalog.clone();
        replace_or_revoke(runtime, replacement);
        assert!(runtime.organization_view().is_none(), "a pending writer does not extend the held grant");
        durable(&mut app, saved);
        let runtime = app.ott.as_ref().unwrap();
        assert!(runtime.organization.session.is_none(), "a durable ACK cannot revive stale UI authority");
        assert!(runtime.organization_view().is_none());
        assert_eq!(runtime.selected, selected);
        assert_eq!(runtime.catalog, raw);
    }
}

#[test]
fn local_manage_refuses_transient_ambiguous_unavailable_and_partial_sources() {
    let _serial = plx_base::testlock::serial();
    let (_directory, mut app) = local_fixture(false);
    assert!(app.ott.as_ref().unwrap().transient.accepted.is_some());
    assert!(!app.ott.as_ref().unwrap().organization_available());
    assert!(app.ott.as_mut().unwrap().organization_open().is_err());
    for case in 0..6 {
        let (_directory, mut app) = local_fixture(true);
        let runtime = app.ott.as_mut().unwrap();
        match case {
            0 => { let profile = runtime.active().unwrap().clone(); runtime.saved.profiles.profiles.push(profile); }
            1 => runtime.local_inventory.sources[0].withdraw(),
            2 => runtime.catalog_complete = false,
            3 => runtime.catalog_loading = true,
            4 => runtime.saved.favorites_selected = true,
            _ => { let id = runtime.active().unwrap().id.clone(); runtime.saved.profiles.profiles.iter_mut().find(|p| p.id == id).unwrap().enabled = false; }
        }
        let saved = serde_json::to_value(&runtime.saved).unwrap();
        assert!(!runtime.organization_available(), "case {case}");
        assert!(runtime.organization_open().is_err(), "case {case}");
        assert!(!runtime.saving && runtime.rx.try_recv().is_err());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), saved);
    }
}
