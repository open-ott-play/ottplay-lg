//! Remote snapshot regressions for the categories -> channels -> play exchange.
//! Mounted as a child of catalog_remote; uses its real command-envelope fixture.
//! Resolver requests stay queued: no resolver worker, native start or decoder is exercised.
//! Category mutation uses the existing revoked storage writer and a modeled Saved ACK.
use super::*;

fn loading_fixture() -> Fixture {
    let mut f = Fixture::new(4);
    let runtime = f.app.ott.as_mut().unwrap();
    runtime.catalog_loading = true;
    runtime.catalog_complete = false;
    runtime.catalog_done.store(false, Ordering::Release);
    runtime.metadata_done.store(false, Ordering::Release);
    runtime.plex_refresh_pending = true;
    // Browse without automatic first-row playback; explicit play_catalog remains eligible.
    runtime.continuity.stop(runtime.generation);
    assert!(runtime.resolver_busy);
    f
}

fn page(f: &mut Fixture, token: &Value, offset: usize, limit: usize) -> Value {
    let response = f.request("channels", json!({
        "runtime":f.runtime_id(), "catalog":token, "category":"all",
        "offset":offset, "limit":limit
    })).expect("channel page is synchronous");
    assert_eq!(response["status"], "ok", "retained snapshot: {response:?}");
    response["data"].clone()
}

fn original_rows(offset: usize, count: usize) -> Value {
    Value::Array((offset..offset + count).map(|i| json!({
        "id":format!("video-{i}"), "name":format!("Video {i}"), "number":i + 1
    })).collect())
}

fn publish(f: &mut Fixture, change: &str) {
    let runtime = f.app.ott.as_ref().unwrap();
    let generation = runtime.generation;
    let revision = runtime.catalog_revision;
    let count = runtime.catalog.entries.len();
    match change {
        "append" => {
            let mut later = synthetic_entry();
            later.id = format!("later-video-{count}");
            later.title = "Later video".into();
            later.category = "Later group".into();
            runtime.catalog_tx.send(CatalogEvent::Batch(generation,
                Catalog { hierarchy: None, entries: vec![later] }))
                .unwrap_or_else(|_| panic!("catalog receiver"));
        }
        "metadata" => {
            let mut enriched = runtime.catalog.entries[0].clone();
            enriched.title = "Updated video title".into();
            enriched.description = "Updated description".into();
            runtime.catalog_tx.send(CatalogEvent::Batch(generation,
                Catalog { hierarchy: None, entries: vec![enriched] }))
                .unwrap_or_else(|_| panic!("catalog receiver"));
        }
        "finished" => runtime.catalog_tx.send(CatalogEvent::Finished(generation, Ok(())))
            .unwrap_or_else(|_| panic!("catalog receiver")),
        _ => unreachable!(),
    }
    // Merge before the next command. The runtime drains Jobs before catalogue events.
    runtime_tick(&mut f.app);
    let runtime = f.app.ott.as_ref().unwrap();
    assert_eq!(runtime.generation, generation);
    if change != "finished" { assert_ne!(runtime.catalog_revision, revision); }
    assert_eq!(runtime.catalog.entries.len(), count + usize::from(change == "append"));
    assert_eq!(runtime.catalog_complete, change == "finished");
    if change == "metadata" {
        assert_eq!(runtime.catalog.entries[0].title, "Updated video title");
        assert_eq!(runtime.catalog.entries[0].description, "Updated description");
    }
    assert_idle(f);
}

fn assert_idle(f: &Fixture) {
    let runtime = f.app.ott.as_ref().unwrap();
    assert!(runtime.resolver_next.is_none());
    assert!(runtime.pending_play.is_none());
    assert!(runtime.play_target.is_none());
    assert!(!runtime.saving);
    assert!(runtime.playing.is_none());
    assert!(!plx_media::route::has_url(&f.app.player.session));
}

fn queue_exact(f: &mut Fixture, token: &Value, expected: &CatalogEntry) -> Value {
    let expected_profile = f.app.ott.as_ref().unwrap().effective_profile().unwrap().clone();
    let response = f.request("play_catalog", json!({"catalog":token,"id":expected.id}));
    assert!(response.is_none(), "eligible exact target must queue: {response:?}");
    let runtime = f.app.ott.as_ref().unwrap();
    let queued = runtime.resolver_next.as_ref().expect("held resolver request");
    assert_eq!(queued.profile, expected_profile);
    let entry = queued.entry.as_ref().unwrap();
    assert_eq!(entry.id, expected.id);
    assert_eq!(entry.media, expected.media, "a snapshot must not rebind its ID to another locator");
    assert!(runtime.pending_play.is_some());
    assert!(!runtime.play_target.as_ref().unwrap().advance_on_failure);
    assert!(!runtime.saving);
    assert!(runtime.playing.is_none());
    assert!(!plx_media::route::has_url(&f.app.player.session));
    // Inspect the existing prepared ACK; no native start or transmitted ACK is claimed.
    commands::play_response(runtime, runtime.pending_play.as_ref().unwrap())
}

#[test]
fn categories_token_survives_real_append_metadata_and_finished_before_channels() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for change in ["append", "metadata", "finished"] {
        let mut f = loading_fixture();
        let root = f.categories();
        assert_eq!(root["loading"], true);
        for cycle in 0..if change == "append" { 3 } else { 1 } {
            publish(&mut f, change);
            for action in ["capabilities", "status"] {
                let response = f.request(action, json!({})).unwrap();
                assert_eq!(response["status"], "ok");
            }
            // On the first cycle no channel-page receipt existed before the update.
            // Repeated capability probes must not self-evict the old root.
            let retained = page(&mut f, &root["catalog"], 0, 200);
            assert_eq!(retained["total"], 4, "{change}/{cycle}");
            assert_eq!(retained["channels"], original_rows(0, 4), "{change}/{cycle}");
            assert_eq!(retained["loading"], true, "the old snapshot keeps its captured state");
            assert_idle(&f);
        }
        // Admit only one new family: ordinary bounded eviction of a third family is valid.
        let fresh_root = f.categories();
        let fresh = page(&mut f, &fresh_root["catalog"], 0, 200);
        assert_eq!(fresh["total"], if change == "append" { 7 } else { 4 });
        assert_eq!(fresh["loading"], change != "finished");
        if change == "append" {
            assert_eq!(fresh["channels"][4]["id"], "later-video-4");
        }
        if change == "metadata" {
            assert_eq!(fresh["channels"][0]["name"], "Updated video title");
        }
        assert_eq!(page(&mut f, &root["catalog"], 0, 200)["channels"], original_rows(0, 4));
        assert_idle(&f);
    }
}

#[test]
fn channel_snapshot_keeps_total_order_and_exact_play_while_fresh_root_sees_updates() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for change in ["append", "metadata", "finished"] {
        let mut f = loading_fixture();
        let selected = f.app.ott.as_ref().unwrap().catalog.entries[0].clone();
        let root = f.categories();
        let first = page(&mut f, &root["catalog"], 0, 2);
        assert_eq!(first["channels"], original_rows(0, 2));
        publish(&mut f, change);
        let tail = page(&mut f, &first["catalog"], 2, 200);
        assert_eq!(tail["catalog"], first["catalog"]);
        assert_eq!(tail["total"], 4);
        assert_eq!(tail["channels"], original_rows(2, 2));
        assert_eq!(tail["has_more"], false);
        let end = page(&mut f, &first["catalog"], 4, 200);
        assert_eq!(end["channels"], json!([]));
        let fresh_root = f.categories();
        let fresh = page(&mut f, &fresh_root["catalog"], 0, 200);
        assert_eq!(fresh["total"], if change == "append" { 5 } else { 4 });
        if change == "append" {
            assert_eq!(fresh["channels"][4]["id"], "later-video-4");
            let unseen = f.request("play_catalog", json!({
                "catalog":first["catalog"],"id":"later-video-4"
            })).expect("an unseen ID is rejected synchronously");
            assert_eq!(unseen["status"], "rejected");
            assert_idle(&f);
        }
        // Obtaining a fresh root must not revoke another still-current snapshot.
        let prepared_ack = queue_exact(&mut f, &first["catalog"], &selected);
        assert_eq!(prepared_ack["status"], "ok");
        assert_eq!(prepared_ack["data"]["channel"], first["channels"][0],
            "ACK id/name/number belong to the old snapshot, including after title enrichment");
    }
}

#[test]
fn channel_snapshot_refuses_replaced_or_missing_locator_before_play_is_captured() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for change in ["locator", "missing"] {
        let mut f = loading_fixture();
        let selected = f.app.ott.as_ref().unwrap().catalog.entries[0].clone();
        let root = f.categories();
        let old = page(&mut f, &root["catalog"], 0, 200);
        let runtime = f.app.ott.as_mut().unwrap();
        if change == "locator" {
            let mut replacement = selected.clone();
            replacement.media = MediaLocator::VPortal { request: json!({"id":999}) };
            assert_ne!(replacement.media, selected.media);
            runtime.catalog_tx.send(CatalogEvent::Batch(runtime.generation,
                Catalog { hierarchy: None, entries: vec![replacement] }))
                .unwrap_or_else(|_| panic!("catalog receiver"));
            runtime_tick(&mut f.app);
            assert_ne!(f.app.ott.as_ref().unwrap().catalog.entries[0].media, selected.media);
        } else {
            // Batch has no deletion operation: model eviction just as the existing missing-row test does.
            runtime.catalog.entries.retain(|entry| entry.id != selected.id);
            runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
            assert!(runtime.catalog.entries.iter().all(|entry| entry.id != selected.id));
        }
        let rejected = f.request("play_catalog", json!({
            "catalog":old["catalog"],"id":selected.id
        })).expect("changed exact media must be rejected before a resolver is queued");
        assert_eq!(rejected["status"], "rejected", "{change}");
        assert_idle(&f);
        // Fresh rows remain eligible; refusal is not caused by a disabled source/worker fixture.
        let current = f.app.ott.as_ref().unwrap().catalog.entries[0].clone();
        let fresh_root = f.categories();
        let fresh = page(&mut f, &fresh_root["catalog"], 0, 200);
        queue_exact(&mut f, &fresh["catalog"], &current);
    }
}

#[test]
fn snapshots_still_reject_changed_source_profile_generation_runtime_and_idle_expiry() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    for phase in ["categories-to-channels", "channels-to-play"] {
        for change in ["source", "profile", "generation", "runtime", "ttl"] {
            let mut f = loading_fixture();
            let root = f.categories();
            let token = if phase == "channels-to-play" {
                page(&mut f, &root["catalog"], 0, 200)["catalog"].clone()
            } else { root["catalog"].clone() };
            let runtime = f.app.ott.as_mut().unwrap();
            match change {
                "source" => runtime.saved.profiles.profiles[0].source =
                    "portal::[key:fixture]https://changed.invalid/nas/api".into(),
                "profile" => {
                    let mut other = runtime.saved.profiles.profiles[0].clone();
                    other.id = "other-profile".into();
                    runtime.saved.profiles.active_profile = Some(other.id.clone());
                    runtime.saved.profiles.profiles.push(other);
                }
                "generation" => runtime.generation = runtime.generation.wrapping_add(1),
                "runtime" => runtime.remote = Remote::disabled_for_test("dev_fixture", "new-runtime"),
                "ttl" => runtime.clock_sample = Some(runtime.clock_sample.unwrap()
                    .advance_for_test(Duration::from_millis(120_000))),
                _ => unreachable!(),
            }
            let response = if phase == "categories-to-channels" {
                f.request("channels", json!({"runtime":f.runtime_id(),"catalog":token,
                    "category":"all","offset":0,"limit":200}))
            } else {
                f.request("play_catalog", json!({"catalog":token,"id":"video-0"}))
            }.expect("stale receipt is rejected synchronously");
            assert_eq!(response["status"], "rejected", "{phase}/{change}");
            if phase == "categories-to-channels" {
                assert_eq!(response["data"]["error"], "catalog_changed", "{change}");
            }
            assert_idle(&f);
            let current = f.app.ott.as_ref().unwrap().catalog.entries[0].clone();
            let fresh_root = f.categories();
            let fresh = page(&mut f, &fresh_root["catalog"], 0, 200);
            queue_exact(&mut f, &fresh["catalog"], &current);
        }
    }
}

#[test]
fn retained_category_snapshot_rechecks_current_membership_before_saved_writer() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = category_mutation_fixture();
    let root = f.categories();
    let category_id = |name: &str| root["categories"].as_array().unwrap().iter()
        .find(|row| row["name"] == name).unwrap()["id"].clone();
    let removed = category_id("Series");
    let retained = category_id("Films");
    let before = serde_json::to_value(&f.app.ott.as_ref().unwrap().saved).unwrap();
    let generation = f.app.ott.as_ref().unwrap().play_generation;
    {
        let runtime = f.app.ott.as_mut().unwrap();
        // Catalogue Batch has no deletion operation. Model current projection eviction.
        runtime.catalog.entries.retain(|row| row.category != "Series");
        runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
        assert_eq!(runtime.catalog.entries.len(), 2);
        assert!(runtime.catalog.entries.iter().all(|row| row.category == "Films"));
    }
    let old_page = f.channels(root["catalog"].as_str().unwrap(), "all", 0);
    assert_eq!(old_page["total"], 4,
        "the retained read snapshot is still valid; this is not stale-token rejection");
    let response = f.request("categories", json!({
        "op":"select", "runtime":f.runtime_id(),
        "catalog":root["catalog"], "category":removed
    })).expect("removed current category must reject before the asynchronous writer");
    assert_eq!(response["status"], "rejected");
    {
        let runtime = f.app.ott.as_ref().unwrap();
        assert!(!runtime.saving && runtime.pending_category_choice.is_none(),
            "no Saved write may be admitted for a category absent from the current projection");
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert_eq!(runtime.play_generation, generation);
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    }

    // Same source, policy, clock, writer and old token: a surviving category is eligible.
    let candidate = f.app.ott.as_ref().unwrap()
        .category_choice_candidate(Some("Films")).unwrap();
    let response = f.request("categories", json!({
        "op":"select", "runtime":f.runtime_id(),
        "catalog":root["catalog"], "category":retained
    }));
    assert!(response.is_none(), "valid retained category must await Saved: {response:?}");
    let (inv, response) = {
        let runtime = f.app.ott.as_mut().unwrap();
        assert!(runtime.saving && runtime.pending_category_choice.is_some());
        let Job::Saved(Err(_), Some(inv), false, response) =
            runtime.rx.recv_timeout(Duration::from_secs(1)).unwrap()
            else { panic!("expected the existing writer's revoked-permit failure"); };
        (inv, response)
    };
    assert_eq!(response["data"]["catalog"], root["catalog"]);
    assert_eq!(response["data"]["selected"], retained);
    let outcome = settle_remote_category(&mut f, candidate, inv, response);
    assert_eq!(outcome["status"], "ok");
    assert_eq!(outcome["data"]["selected"], retained);
    let runtime = f.app.ott.as_ref().unwrap();
    assert_eq!(runtime.saved.category, "Films");
    assert!(!runtime.saving && runtime.pending_category_choice.is_none());
    assert_eq!(runtime.play_generation, generation);
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    assert!(!plx_media::route::has_url(&f.app.player.session));
}

#[test]
fn retained_category_id_cannot_be_reinterpreted_as_another_folders_legacy_label() {
    let _lock = plx_base::testlock::serial();
    let _reset = ResetMedia;
    let mut f = category_mutation_fixture();
    {
        let runtime = f.app.ott.as_mut().unwrap();
        runtime.catalog.entries.truncate(2);
        for row in &mut runtime.catalog.entries { row.category.clear(); }
        runtime.catalog.hierarchy = Some(plx_data::ott::CategoryHierarchy {
            nodes: [("old-folder", "Original folder"), ("new-folder", "Other folder")]
                .into_iter().map(|(id, label)| plx_data::ott::CategoryNode {
                    id: id.into(), label: label.into(), parent: None,
                    anonymous_fingerprint: None, source_identity: None,
                }).collect(),
            memberships: runtime.catalog.entries.iter().zip(["old-folder", "new-folder"])
                .map(|(entry, id)| plx_data::ott::CategoryMembership {
                    entry_id: entry.id.clone(), category_id: id.into(),
                }).collect(),
        });
        runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
        assert_eq!(runtime.organization_category_id("old-folder"), "old-folder");
    }
    let root = f.categories();
    let old_wire = root["categories"].as_array().unwrap().iter()
        .find(|row| row["name"] == "Original folder").unwrap()["id"].clone();
    let before = serde_json::to_value(&f.app.ott.as_ref().unwrap().saved).unwrap();
    {
        let runtime = f.app.ott.as_mut().unwrap();
        let graph = runtime.catalog.hierarchy.as_mut().unwrap();
        graph.nodes.retain(|node| node.id != "old-folder");
        graph.memberships.retain(|membership| membership.category_id != "old-folder");
        graph.nodes[0].label = "old-folder".into();
        runtime.catalog_revision = runtime.catalog_revision.wrapping_add(1);
        assert_eq!(runtime.organization_category_id("old-folder"), "new-folder",
            "the deleted canonical ID now resolves through the other folder's legacy label");
        assert!(runtime.organization_categories().iter().any(|row| row.id == "new-folder"));
    }
    assert_eq!(f.channels(root["catalog"].as_str().unwrap(), "all", 0)["total"], 2,
        "the retained token is still readable");
    let response = f.request("categories", json!({
        "op":"select", "runtime":f.runtime_id(),
        "catalog":root["catalog"], "category":old_wire
    })).expect("a different canonical category must not enter the Saved writer");
    assert_eq!(response["status"], "rejected");
    {
        let runtime = f.app.ott.as_ref().unwrap();
        assert!(!runtime.saving && runtime.pending_category_choice.is_none());
        assert_eq!(serde_json::to_value(&runtime.saved).unwrap(), before);
        assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
    }
    // The replacement category is valid when explicitly chosen from a fresh snapshot.
    // This uses the same existing revoked-permit writer and modeled durable ACK.
    // The shared staging helper's modeled candidate uses its display-name argument;
    // hierarchy selection persists the canonical ID instead.
    let candidate = f.app.ott.as_ref().unwrap()
        .category_choice_candidate(Some("new-folder")).unwrap();
    let (_, inv, response, _) = stage_remote_category(&mut f, "old-folder");
    assert_eq!(settle_remote_category(&mut f, candidate, inv, response)["status"], "ok");
    let runtime = f.app.ott.as_ref().unwrap();
    assert_eq!(runtime.saved.category, "new-folder");
    assert!(!runtime.saving && runtime.pending_category_choice.is_none());
    assert!(runtime.play_target.is_none() && runtime.resolver_next.is_none());
}
