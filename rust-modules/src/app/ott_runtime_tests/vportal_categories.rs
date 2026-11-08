//! Real parser -> category projection -> UI/remote/Saved regressions.
//! Revoked writer completions are modeled; no provider or protected storage IO.
use super::*;

fn nested() -> Value {
    json!({"type":"videoportal","items":[
        {"type":"category","title":"A","items":[
            {"type":"stream","id":"direct","title":"Direct","request":{"id":"direct"}},
            {"type":"category","title":"B","items":[
                {"type":"category","title":"C","items":[
                    {"type":"stream","id":"deep","title":"Deep","request":{"id":"deep"}}
                ]},
                {"type":"category","title":"Empty","items":[]}
            ]}
        ]},
        {"type":"category","title":"Other","items":[
            {"type":"stream","id":"other","title":"Other video","request":{"id":"other"}}
        ]}
    ]})
}

fn parse(value: Value) -> Catalog {
    plx_data::ott::parse_vportal_response(&value.to_string(), &synthetic_profile(), "")
        .expect("synthetic portal response").catalog
}

struct Fixture { app: App, serial: u64 }
impl Fixture {
    fn new(value: Value) -> Self {
        let mut app = app_fixture();
        app.running = true;
        let r = app.ott.as_mut().unwrap();
        r.loaded = true;
        r.provider = ProviderKind::VPortal;
        r.saved.profiles.profiles = vec![synthetic_profile()];
        r.saved.profiles.active_profile = Some("portal-fixture".into());
        r.catalog = parse(value);
        r.catalog_complete = true;
        r.catalog_loading = false;
        r.entropy = Some([1; 16]);
        r.write_permit.revoke();
        r.catalog_done.store(false, Ordering::Release);
        r.metadata_done.store(false, Ordering::Release);
        r.resolver_busy = true;
        r.plex_refresh_pending = true;
        r.continuity.stop(r.generation);
        r.page = "categories";
        Self { app, serial: 0 }
    }
    fn runtime_id(&self) -> String {
        self.app.ott.as_ref().unwrap().remote.runtime().to_owned()
    }
    fn request(&mut self, action: &str, params: Value) -> Option<Value> {
        self.serial += 1;
        let mut r = self.app.ott.take().unwrap();
        let now = r.now_ms();
        let body = json!({"commands":[],"server_time":1000,"requests":[{
            "id":format!("{:032x}", self.serial),"expires_at":1020,
            "action":action,"params":params
        }]}).to_string();
        let mut incoming = r.remote.raw_poll_for_test(&body, now);
        assert_eq!(incoming.len(), 1);
        let inv = incoming.pop().unwrap();
        assert!(r.remote.invocation_is_current(&inv));
        let response = commands::command(&mut r, &mut self.app, &inv);
        if let Some(response) = &response { r.remote.complete(inv, response.clone()); }
        self.app.ott = Some(r);
        response
    }
    fn categories(&mut self) -> Value {
        let response = self.request("categories", json!({"op":"list",
            "runtime":self.runtime_id(),"offset":0,"limit":200})).unwrap();
        assert_eq!(response["status"], "ok");
        response["data"].clone()
    }
}

fn category(r: &Runtime, label: &str) -> String {
    let rows: Vec<_> = r.organization_categories().into_iter()
        .filter(|row| row.title == label).collect();
    assert_eq!(rows.len(), 1, "one structural category for {label}");
    rows[0].id.clone()
}

fn row_id(r: &Runtime, provider_id: &str) -> String {
    r.catalog.entries.iter().find(|row| row.provider_id.as_deref() == Some(provider_id))
        .expect("provider row").id.clone()
}

#[test]
fn vportal_ancestors_share_order_and_raw_ordinals_across_ui_and_transport() {
    let _serial = plx_base::testlock::serial();
    let mut f = Fixture::new(nested());
    let r = f.app.ott.as_ref().unwrap();
    let original = r.catalog.entries.clone();
    let direct = row_id(r, "direct");
    let deep = row_id(r, "deep");
    assert_eq!(r.organization_categories().iter().map(|row| row.title.as_str()).collect::<Vec<_>>(),
        vec!["A", "A / B", "A / B / C", "A / B / Empty", "Other"]);
    for (label, ids, ordinals) in [
        ("A", vec![direct, deep.clone()], vec![1, 2]),
        ("A / B", vec![deep.clone()], vec![2]),
        ("A / B / C", vec![deep], vec![2]),
        ("A / B / Empty", Vec::new(), Vec::new()),
    ] {
        let r = f.app.ott.as_mut().unwrap();
        let key = category(r, label);
        r.saved = r.category_choice_candidate(Some(&key)).unwrap();
        assert_eq!(r.effective_category(), key);
        assert_eq!(r.ordered_ids(), ids, "{label}");
        r.page = "categories";
        assert_eq!(r.raw_view().selected_row.as_deref(), Some(key.as_str()));
        r.page = "channels";
        let visible: Vec<_> = r.raw_view().rows.into_iter()
            .filter_map(|row| match row.action { OttAction::Play(id) => Some(id), _ => None }).collect();
        assert_eq!(visible, ids);
        let response = f.request("channels", json!({})).unwrap();
        assert_eq!(response["status"], "ok");
        let numbers: Vec<_> = response["data"]["channels"].as_array().unwrap().iter()
            .map(|row| row["number"].as_u64().unwrap() as usize).collect();
        assert_eq!(numbers, ordinals);
    }
    let r = f.app.ott.as_mut().unwrap();
    r.saved = r.category_choice_candidate(Some("")).unwrap();
    assert_eq!(r.ordered_ids().len(), 3);
    assert_eq!(r.catalog.entries, original, "memberships must not copy or mutate provider rows");
}

#[test]
fn vportal_remote_ancestor_membership_and_empty_selection_use_the_existing_saved_writer() {
    let _serial = plx_base::testlock::serial();
    let mut f = Fixture::new(nested());
    let categories = f.categories();
    let rows = categories["categories"].as_array().unwrap();
    for (name, numbers) in [("A", vec![1, 2]), ("A / B", vec![2]), ("A / B / Empty", vec![])] {
        let category = rows.iter().find(|row| row["name"] == name).unwrap();
        assert_eq!(category["count"], numbers.len());
        let response = f.request("channels", json!({"runtime":f.runtime_id(),
            "catalog":categories["catalog"],"category":category["id"],"offset":0,"limit":200})).unwrap();
        assert_eq!(response["status"], "ok");
        let actual: Vec<_> = response["data"]["channels"].as_array().unwrap().iter()
            .map(|row| row["number"].as_u64().unwrap()).collect();
        assert_eq!(actual, numbers);
    }
    let wire = rows.iter().find(|row| row["name"] == "A / B / Empty").unwrap()["id"].clone();
    let r = f.app.ott.as_ref().unwrap();
    let key = category(r, "A / B / Empty");
    let candidate = r.category_choice_candidate(Some(&key)).unwrap();
    let generation = r.play_generation;
    assert!(f.request("categories", json!({"op":"select","runtime":f.runtime_id(),
        "catalog":categories["catalog"],"category":wire})).is_none());
    let r = f.app.ott.as_mut().unwrap();
    assert!(r.saved.category.is_empty(), "do not publish before the durable acknowledgement");
    let Job::Saved(Err(_), Some(inv), false, response) = r.rx.recv_timeout(Duration::from_secs(1)).unwrap()
        else { panic!("revoked writer result"); };
    let request_id = inv.id.clone();
    r.tx.send(Job::Saved(Ok(candidate), Some(inv), false, response))
        .unwrap_or_else(|_| panic!("fixture receiver"));
    runtime_tick(&mut f.app);
    let r = f.app.ott.as_ref().unwrap();
    let response = r.remote.response_for_id_for_test(&request_id).unwrap();
    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["selected"], wire);
    assert_eq!(r.saved.category, key);
    assert_eq!(r.effective_category(), key);
    assert_eq!(r.page, "channels");
    assert!(r.ordered_ids().is_empty());
    assert_eq!(r.play_generation, generation);
    assert!(r.resolver_next.is_none() && r.play_target.is_none());
}

#[test]
fn vportal_ancestor_keys_and_unique_legacy_labels_survive_saved_profile_roundtrip() {
    let _serial = plx_base::testlock::serial();
    let mut f = Fixture::new(nested());
    let r = f.app.ott.as_mut().unwrap();
    let key = category(r, "A / B");
    r.saved = r.category_choice_candidate(Some(&key)).unwrap();
    let mut other = synthetic_profile();
    other.id = "second-portal".into(); other.name = "Second".into();
    r.saved.profiles.profiles.push(other);
    let mut candidate = r.saved.clone();
    candidate.profiles.active_profile = Some("second-portal".into());
    category_state::prepare_profile_change(&r.saved, &mut candidate).unwrap();
    r.saved = candidate;
    assert!(r.saved.category.is_empty());
    let mut candidate = r.saved.clone();
    candidate.profiles.active_profile = Some("portal-fixture".into());
    category_state::prepare_profile_change(&r.saved, &mut candidate).unwrap();
    let encoded = serde_json::to_vec(&candidate).unwrap();
    r.saved = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(r.effective_category(), key);
    assert_eq!(r.ordered_ids(), vec![row_id(r, "deep")]);
    r.saved.category = "A / B / C".into();
    let before = r.saved.clone();
    assert_eq!(r.effective_category(), "A / B / C");
    assert_eq!(r.organization_category_id(r.effective_category()), category(r, "A / B / C"));
    assert_eq!(r.ordered_ids(), vec![row_id(r, "deep")]);
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), serde_json::to_value(before).unwrap());
}

#[test]
fn vportal_category_back_preserves_partial_scan_and_source_selection() {
    let _serial = plx_base::testlock::serial();
    let mut f = Fixture::new(nested());
    let mut r = f.app.ott.take().unwrap();
    let key = category(&r, "A / B");
    r.saved = r.category_choice_candidate(Some(&key)).unwrap();
    r.catalog_loading = true; r.catalog_complete = false;
    r.selected = Some(row_id(&r, "deep"));
    let generation = r.generation;
    let play_generation = r.play_generation;
    let selected = r.selected.clone();
    let entries = r.catalog.entries.clone();
    assert!(!r.cancel.load(Ordering::Acquire));
    handle_action(&mut r, &mut f.app, OttAction::Back).unwrap();
    assert_eq!(r.page, "channels");
    assert_eq!(r.generation, generation);
    assert_eq!(r.play_generation, play_generation);
    assert!(r.catalog_loading && !r.catalog_complete);
    assert!(!r.cancel.load(Ordering::Acquire));
    assert_eq!(r.saved.category, key);
    assert_eq!(r.selected, selected);
    assert_eq!(r.catalog.entries, entries);
    assert_eq!(r.ordered_ids(), vec![row_id(&r, "deep")]);
    f.app.ott = Some(r);
}

#[test]
fn vportal_literal_slash_labels_do_not_alias_structural_categories() {
    let _serial = plx_base::testlock::serial();
    let mut f = Fixture::new(json!({"type":"videoportal","items":[
        {"type":"category","title":"A / B","items":[
            {"type":"stream","id":"literal","title":"Literal","request":{"id":"literal"}}
        ]},
        {"type":"category","title":"A","items":[
            {"type":"category","title":"B","items":[
                {"type":"stream","id":"nested","title":"Nested","request":{"id":"nested"}}
            ]}
        ]}
    ]}));
    let r = f.app.ott.as_mut().unwrap();
    let choices: Vec<_> = r.organization_categories().into_iter().filter(|row| row.title == "A / B").collect();
    assert_eq!(choices.len(), 2);
    assert_ne!(choices[0].id, choices[1].id);
    for (choice, provider_id) in choices.iter().zip(["literal", "nested"]) {
        r.saved.category = choice.id.clone();
        assert_eq!(r.ordered_ids(), vec![row_id(r, provider_id)]);
    }
    r.saved.category = "A / B".into();
    assert_eq!(r.effective_category(), "", "ambiguous legacy label cannot pick one structure");
    assert_eq!(r.ordered_ids().len(), 2);
    assert_eq!(r.saved.category, "A / B", "fallback must not rewrite the remembered choice");
}

#[test]
fn vportal_ancestor_browsing_preserves_one_favorite_and_its_exact_leaf_origin() {
    let _serial = plx_base::testlock::serial();
    let mut f = Fixture::new(nested());
    let r = f.app.ott.as_mut().unwrap();
    let deep = r.catalog.entries.iter().find(|row| row.provider_id.as_deref() == Some("deep")).unwrap().clone();
    let favorite = selection::capture(&synthetic_profile(), &deep).unwrap();
    assert_eq!(favorite.origin.subprofile.as_deref(), Some("A / B / C"));
    r.saved.favorites.toggle(favorite.clone()).unwrap();
    let document = r.saved.favorites.clone();
    for label in ["A", "A / B", "A / B / C"] {
        r.saved.category = category(r, label);
        let displayed = r.organization_rows(r.effective_category()).into_iter()
            .find(|(_, row)| row.id == deep.id).unwrap().1;
        assert_eq!(selection::capture(&synthetic_profile(), displayed), Some(favorite.clone()));
        assert_eq!(r.saved.favorites, document);
    }
    r.saved.favorites_selected = true;
    assert_eq!(r.ordered_ids(), vec![format!("favorite:{}", favorite.identity())]);
    r.saved.favorites_selected = false;
    r.saved.category.clear();
    assert_eq!(r.ordered_ids().len(), 3);
}

#[test]
fn vportal_folder_only_batch_updates_categories_and_preserves_remote_snapshots_without_playing() {
    let _serial = plx_base::testlock::serial();
    let mut f = Fixture::new(nested());
    let old = f.categories();
    let r = f.app.ott.as_mut().unwrap();
    let revision = r.catalog_revision;
    let generation = r.play_generation;
    let old_rows = r.catalog.entries.clone();
    let empty = parse(json!({"type":"videoportal","items":[
        {"type":"category","title":"Later empty","items":[]}
    ]}));
    assert!(empty.entries.is_empty());
    r.catalog_tx.send(CatalogEvent::Batch(r.generation, empty))
        .unwrap_or_else(|_| panic!("fixture receiver"));
    runtime_tick(&mut f.app);
    let r = f.app.ott.as_ref().unwrap();
    assert!(r.catalog_revision > revision);
    assert!(r.organization_categories().iter().any(|row| row.title == "Later empty"));
    assert_eq!(r.catalog.entries, old_rows);
    assert_eq!(r.play_generation, generation);
    assert!(r.play_target.is_none() && r.resolver_next.is_none());
    let retained = f.request("categories", json!({"op":"list","runtime":f.runtime_id(),
        "catalog":old["catalog"],"offset":0,"limit":200})).unwrap();
    assert_eq!(retained["status"], "ok");
    assert_eq!(retained["data"], old, "old token retains its exact category snapshot");
    let channels = f.request("channels", json!({"runtime":f.runtime_id(),
        "catalog":old["catalog"],"category":"all","offset":0,"limit":200})).unwrap();
    assert_eq!(channels["status"], "ok");
    assert_eq!(channels["data"]["total"], old_rows.len());
    let ids: Vec<_> = channels["data"]["channels"].as_array().unwrap().iter()
        .map(|row| row["id"].as_str().unwrap()).collect();
    assert_eq!(ids, old_rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>());
    let fresh = f.categories();
    assert_eq!(fresh["total"], old["total"].as_u64().unwrap() + 1);
    let added = fresh["categories"].as_array().unwrap().iter()
        .find(|row| row["name"] == "Later empty").expect("fresh root includes the discovered folder");
    assert_eq!(added["count"], 0);
    let r = f.app.ott.as_ref().unwrap();
    assert_eq!(r.play_generation, generation);
    assert!(r.play_target.is_none() && r.resolver_next.is_none());
}

#[test]
fn vportal_stale_folder_batch_and_old_category_action_cannot_change_replacement_source() {
    let _serial = plx_base::testlock::serial();
    let mut f = Fixture::new(nested());
    let mut r = f.app.ott.take().unwrap();
    let action = r.raw_view().rows.into_iter().find(|row| row.title == "A").unwrap().action;
    let old_generation = r.generation;
    r.generation = r.generation.wrapping_add(1);
    r.saved.profiles.profiles[0].source = "portal::[key:replacement]https://replacement.invalid/api".into();
    let before = serde_json::to_value(&r.saved).unwrap();
    assert!(handle_action(&mut r, &mut f.app, action).is_err());
    assert!(!r.saving);
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
    let empty = parse(json!({"type":"videoportal","items":[
        {"type":"category","title":"Stale empty","items":[]}
    ]}));
    let revision = r.catalog_revision;
    r.catalog_tx.send(CatalogEvent::Batch(old_generation, empty))
        .unwrap_or_else(|_| panic!("fixture receiver"));
    f.app.ott = Some(r);
    runtime_tick(&mut f.app);
    let r = f.app.ott.as_ref().unwrap();
    assert_eq!(r.catalog_revision, revision);
    assert!(!r.organization_categories().iter().any(|row| row.title == "Stale empty"));
    assert_eq!(serde_json::to_value(&r.saved).unwrap(), before);
}
