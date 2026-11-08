//! Regression fixtures drive the production paged visitor and shared graph merge.
use super::*;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

struct FixtureRead {
    sections: Value,
    pages: RefCell<VecDeque<(i64, i64, Value)>>,
    queries: RefCell<Vec<(i64, i64)>>,
    section_reads: Cell<usize>,
    current: Cell<bool>,
}
impl FixtureRead {
    fn new(sections: Value, pages: Vec<(i64, i64, Value)>) -> Self {
        Self {
            sections, pages: RefCell::new(pages.into()), queries: RefCell::new(Vec::new()),
            section_reads: Cell::new(0), current: Cell::new(true),
        }
    }
}
impl CatalogRead for FixtureRead {
    fn current(&self) -> bool { self.current.get() }
    fn sections(&self) -> Option<plex::MediaContainer> {
        self.section_reads.set(self.section_reads.get() + 1);
        serde_json::from_value(self.sections.clone()).ok()
    }
    fn items(&self, query: &SectionQuery) -> Option<plex::MediaContainer> {
        assert_eq!(query.size, 128);
        assert!(!query.include_meta);
        self.queries.borrow_mut().push((query.section_key, query.start));
        let (section, start, value) = self.pages.borrow_mut().pop_front().expect("unexpected page");
        assert_eq!((query.section_key, query.start), (section, start));
        serde_json::from_value(value).ok()
    }
}
fn profile() -> ProviderProfile {
    ProviderProfile {
        id: "plex-1".into(), name: "Home".into(), kind: plx_data::ott::ProviderKind::Plex,
        source: "server-1".into(), source_alias: String::new(), enabled: true,
    }
}
fn episode(id: usize, show: &str, season: &str) -> Value {
    json!({"type":"episode", "ratingKey":id.to_string(), "title":format!("Episode {id}"),
        "grandparentRatingKey":show, "grandparentTitle":"Show / literal",
        "parentRatingKey":season, "parentTitle":"Season / literal", "parentIndex":1})
}
fn page(start: usize, total: usize, rows: Vec<Value>) -> Value {
    json!({"offset":start, "totalSize":total, "Metadata":rows})
}
fn collect(read: &FixtureRead) -> (Catalog, Vec<usize>) {
    let mut catalog = Catalog::default();
    let mut sizes = Vec::new();
    visit_with(&profile(), &AtomicBool::new(false), &mut |batch| {
        sizes.push(batch.entries.len());
        catalog.merge(batch).expect("production batch must merge");
        Ok(())
    }, read).unwrap();
    (catalog, sizes)
}

#[test]
fn duplicate_library_names_and_literal_slashes_keep_source_scoped_descendants() {
    let read = FixtureRead::new(json!({"Directory":[
        {"key":"1","type":"show","title":"TV / literal"},
        {"key":"2","type":"show","title":"TV / literal"}
    ]}), vec![
        (1, 0, page(0, 2, vec![episode(1, "show", "season1"), episode(2, "show", "season2")])),
        (2, 0, page(0, 1, vec![episode(3, "show", "season1")])),
    ]);
    let (catalog, _) = collect(&read);
    let nodes = &catalog.hierarchy.as_ref().unwrap().nodes;
    let roots: Vec<_> = nodes.iter().filter(|node| node.parent.is_none()).collect();
    assert_eq!(roots.len(), 2);
    assert_ne!(roots[0].id, roots[1].id);
    assert!(roots.iter().all(|node| node.label == "TV / literal"));
    assert_eq!(catalog.category_indices(&roots[0].id), vec![0, 1]);
    assert_eq!(catalog.category_indices(&roots[1].id), vec![2]);
    let shows: Vec<_> = nodes.iter().filter(|node| node.label == "Show / literal").collect();
    assert_eq!(shows.len(), 2);
    assert_ne!(shows[0].id, shows[1].id);
    assert_eq!(catalog.category_indices(&shows[0].id), vec![0, 1]);
    assert_eq!(nodes.iter().filter(|node| node.label == "Season / literal").count(), 3);
    assert_eq!(nodes.len(), 7); // Slashes do not create extra nodes.
    assert!(nodes.iter().all(|node| node.id.len() <= 256));
    assert_eq!(catalog.resolve_category("TV / literal"), None); // Duplicate label is ambiguous.
}

#[test]
fn background_pages_keep_one_category_and_exceed_play_queue_limit() {
    let mut pages = Vec::new();
    for start in (0..513).step_by(128) {
        let mut rows: Vec<_> = (start..(start + 128).min(513)).map(|id| episode(id, "show", "season")).collect();
        if start > 0 {
            for row in &mut rows { row["grandparentTitle"] = json!("Later label"); }
        }
        pages.push((1, start as i64, page(start, 513, rows)));
    }
    let read = FixtureRead::new(json!({"Directory":[{"key":"1","type":"show","title":"TV"}]}), pages);
    let (catalog, sizes) = collect(&read);
    assert_eq!(sizes, vec![0, 1, 127, 128, 128, 128, 1]);
    assert_eq!(catalog.entries.len(), 513);
    assert_eq!(*read.queries.borrow(), vec![(1, 0), (1, 128), (1, 256), (1, 384), (1, 512)]);
    assert!(read.pages.borrow().is_empty());
    let graph = catalog.hierarchy.as_ref().unwrap();
    assert_eq!(graph.nodes.len(), 3);
    assert_eq!(graph.memberships.len(), 513);
    assert_eq!(graph.nodes[1].label, "Show / literal"); // Repeated source ID keeps the initial label.
    assert_eq!(catalog.category_indices(&graph.nodes[0].id).len(), 513);
    assert_eq!(catalog.category_indices(&graph.nodes[1].id).len(), 513);
    assert_eq!(catalog.category_indices(&graph.nodes[2].id).len(), 513);
}

#[test]
fn missing_parent_ids_attach_only_to_proven_ancestors() {
    let read = FixtureRead::new(json!({"Directory":[{"key":"1","type":"show","title":"TV"}]}), vec![
        (1, 0, page(0, 4, vec![episode(1, "", ""), episode(2, "show", ""),
            episode(3, "", "season"), episode(4, "show", "season")]))
    ]);
    let (catalog, _) = collect(&read);
    let graph = catalog.hierarchy.as_ref().unwrap();
    assert_eq!(graph.nodes.len(), 3);
    let root = &graph.nodes[0].id;
    let show = &graph.nodes[1].id;
    let season = &graph.nodes[2].id;
    assert_eq!(&graph.memberships[0].category_id, root);
    assert_eq!(&graph.memberships[1].category_id, show);
    assert_eq!(&graph.memberships[2].category_id, root);
    assert_eq!(&graph.memberships[3].category_id, season);
    assert_eq!(catalog.category_indices(show), vec![1, 3]);
    assert_eq!(catalog.category_indices(season), vec![3]);
}

#[test]
fn empty_libraries_remain_visible_and_movies_are_direct_playable_leaves() {
    let read = FixtureRead::new(json!({"Directory":[
        {"key":"1","type":"movie","title":"Empty"},
        {"key":"2","type":"movie","title":"Movies"}
    ]}), vec![
        (1, 0, page(0, 0, vec![])),
        (2, 0, page(0, 1, vec![json!({"type":"movie","ratingKey":"7","title":"Movie"})])),
    ]);
    let (catalog, sizes) = collect(&read);
    assert_eq!(sizes, vec![0, 0, 1]);
    let graph = catalog.hierarchy.as_ref().unwrap();
    assert_eq!(graph.nodes.len(), 2);
    assert!(catalog.category_indices(&graph.nodes[0].id).is_empty());
    assert_eq!(catalog.category_indices(&graph.nodes[1].id), vec![0]);
    assert_eq!(catalog.entries[0].media, MediaLocator::Plex { server_id:"server-1".into(), rating_key:"7".into() });
}

#[test]
fn cancellation_after_first_row_stops_remainder_and_background_pages() {
    let read = FixtureRead::new(json!({"Directory":[{"key":"1","type":"show","title":"TV"}]}), vec![
        (1, 0, page(0, 256, (0..128).map(|id| episode(id, "show", "season")).collect())),
        (1, 128, page(128, 256, (128..256).map(|id| episode(id, "show", "season")).collect())),
    ]);
    let cancel = AtomicBool::new(false);
    let mut catalog = Catalog::default();
    visit_with(&profile(), &cancel, &mut |batch| {
        if !batch.entries.is_empty() { cancel.store(true, Ordering::Release); }
        catalog.merge(batch).unwrap();
        Ok(())
    }, &read).unwrap();
    assert_eq!(catalog.entries.len(), 1);
    assert_eq!(*read.queries.borrow(), vec![(1, 0)]);
    assert_eq!(read.pages.borrow().len(), 1);
    visit_with(&profile(), &cancel, &mut |_| panic!("cancelled visit emitted"), &read).unwrap();
    assert_eq!(read.section_reads.get(), 1);
}

#[test]
fn retired_managed_read_cannot_emit_the_rest_of_a_page() {
    let read = FixtureRead::new(json!({"Directory":[{"key":"1","type":"show","title":"TV"}]}), vec![
        (1, 0, page(0, 2, vec![episode(1, "show", "season"), episode(2, "show", "season")]))
    ]);
    let mut rows = 0;
    let result = visit_with(&profile(), &AtomicBool::new(false), &mut |batch| {
        rows += batch.entries.len();
        if rows > 0 { read.current.set(false); }
        Ok(())
    }, &read);
    assert_eq!(result, Err("Plex source is no longer available"));
    assert_eq!(rows, 1);
    assert_eq!(*read.queries.borrow(), vec![(1, 0)]);
}

#[test]
fn category_identity_ignores_labels_and_is_scoped_to_the_server() {
    let ids_for = |profile: &ProviderProfile, title: &str| {
        let mut row = episode(1, "show", "season");
        row["grandparentTitle"] = json!(title);
        row["parentTitle"] = json!(title);
        let read = FixtureRead::new(json!({"Directory":[{"key":"1","type":"show","title":title}]}),
            vec![(1, 0, page(0, 1, vec![row]))]);
        let mut catalog = Catalog::default();
        visit_with(profile, &AtomicBool::new(false), &mut |batch| {
            catalog.merge(batch).unwrap();
            Ok(())
        }, &read).unwrap();
        catalog.hierarchy.unwrap().nodes.into_iter().map(|node| node.id).collect::<Vec<_>>()
    };
    let profile = profile();
    let ids = ids_for(&profile, "First / title");
    assert_eq!(ids, ids_for(&profile, "Renamed / title"));
    let mut other_server = profile.clone();
    other_server.source = "server-2".into();
    let other = ids_for(&other_server, "First / title");
    assert!(ids.iter().all(|id| !other.contains(id)));
}


#[test]
fn classic_read_tracks_its_captured_registry_server_and_rejects_replacement() {
    let _serial = plx_base::testlock::serial();
    struct Reset;
    impl Drop for Reset { fn drop(&mut self) { plex::reset_servers_for_test(); } }
    let _reset = Reset;
    let first = plex::register_pinned_with_client_id("classic-first",
        &plex::Origin::parse("https://classic-1.invalid:32400").unwrap(),
        "first-token", None, "fixture", Default::default());
    let read = ClassicRead::capture(plex::client_for(first).unwrap()).unwrap();
    assert!(read.matches("classic-first"));
    assert!(!read.matches("classic-second"));
    let other = plex::register_pinned_with_client_id("classic-second",
        &plex::Origin::parse("https://classic-2.invalid:32400").unwrap(),
        "other-token", None, "fixture", Default::default());
    assert!(plex::set_current(other));
    assert!(read.current()); // The singleton's selection does not redefine the captured server.
    let mut wrong_profile = profile();
    wrong_profile.source = "classic-second".into();
    assert_eq!(visit_classic(&wrong_profile, &read, &AtomicBool::new(false),
        &mut |_| panic!("wrong server emitted a batch")), Err("Plex source does not match"));
    let replacement = plex::register_pinned_with_client_id("classic-first",
        &plex::Origin::parse("https://classic-3.invalid:32400").unwrap(),
        "first-token", None, "fixture", Default::default());
    assert_eq!(first, replacement);
    assert!(!read.current()); // Same machine/slot, different actual Client instance.
    assert!(ClassicRead::capture(plex::client_for(first).unwrap()).is_ok());
}

#[test]
fn plex_provenance_survives_temporary_profile_recreation_without_changing_catalog_ids() {
    let collect_for = |profile: &ProviderProfile, title: &str| {
        let mut item = episode(42, "show", "season");
        item["grandparentTitle"] = json!(title);
        item["parentTitle"] = json!(title);
        let read = FixtureRead::new(json!({"Directory":[{"key":"7","type":"show","title":title}]}),
            vec![(7, 0, page(0, 1, vec![item]))]);
        let mut catalog = Catalog::default();
        visit_with(profile, &AtomicBool::new(false), &mut |batch| {
            catalog.merge(batch).unwrap();
            Ok(())
        }, &read).unwrap();
        catalog
    };
    let first = profile();
    let mut next = first.clone();
    next.id = "classic-next-runtime-1".into();
    let before = collect_for(&first, "Old label");
    let after = collect_for(&next, "Renamed label");
    let left = &before.hierarchy.as_ref().unwrap().nodes;
    let right = &after.hierarchy.as_ref().unwrap().nodes;
    assert_eq!(left.len(), 3);
    for (index, (kind, source_id)) in [(PlexCategoryKind::Section, "7"),
        (PlexCategoryKind::Show, "show"), (PlexCategoryKind::Season, "season")].into_iter().enumerate() {
        let expected = CategorySourceIdentity::Plex { section: 7, kind, source_id: source_id.into() };
        assert_eq!(left[index].source_identity.as_ref(), Some(&expected));
        assert_eq!(right[index].source_identity.as_ref(), Some(&expected));
        assert_eq!(left[index].source_identity.as_ref().unwrap().fingerprint(),
            right[index].source_identity.as_ref().unwrap().fingerprint());
        assert_ne!(left[index].id, right[index].id);
        let kind_name = match kind { PlexCategoryKind::Section => "section", PlexCategoryKind::Show => "show", PlexCategoryKind::Season => "season" };
        let old_scope = serde_json::to_vec(&(first.id.as_str(), first.source.as_str(), 7_i64, kind_name, source_id)).unwrap();
        assert_eq!(left[index].id, format!("plex:{kind_name}:{}", plx_data::ott::favorites_wire::hash(&old_scope)));
    }
    assert_eq!(before.entries[0].id, "plex-1:42");
    assert_eq!(after.entries[0].id, "classic-next-runtime-1:42");
    assert_eq!(before.entries[0].provider_id.as_deref(), Some("42"));
    assert_eq!(before.entries[0].media, after.entries[0].media);
    assert_eq!(before.entries[0].media, MediaLocator::Plex { server_id: first.source, rating_key: "42".into() });
}

#[test]
fn plex_node_rejects_invalid_provenance_before_mutating_projection() {
    let mut projection = Projection::default();
    let mut catalog = Catalog::default();
    for (section, kind, source_id) in [(0, "section", "0"), (7, "section", "07"),
        (7, "unknown", "42"), (7, "show", "../42")] {
        assert!(projection.node(&profile(), section, (kind, source_id), None, "Title".into(), &mut catalog).is_err());
        assert!(projection.parents.is_empty());
        assert_eq!(catalog, Catalog::default());
    }
}
