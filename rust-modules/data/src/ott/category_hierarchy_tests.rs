use super::*;
use crate::ott::{parse_vportal_response, PortalRequest, ProviderKind, ProviderProfile};
use serde_json::json;

fn profile() -> ProviderProfile {
    ProviderProfile { id: "categories-fixture".into(), name: "Categories".into(), kind: ProviderKind::VPortal,
        source: "portal::[key:test]https://portal.invalid/api".into(), source_alias: String::new(), enabled: true }
}
fn parse(value: serde_json::Value) -> Catalog {
    parse_vportal_response(&value.to_string(), &profile(), "").unwrap().catalog
}
fn leaf(id: &str) -> serde_json::Value { json!({"type":"stream","id":id,"title":id,"url":format!("/{id}")}) }
fn folder(id: &str, title: &str, items: Vec<serde_json::Value>) -> serde_json::Value {
    json!({"type":"category","id":id,"title":title,"items":items})
}
fn root(items: Vec<serde_json::Value>) -> serde_json::Value { json!({"type":"videoportal","items":items}) }

#[test]
fn ancestor_groups_preserve_raw_order_without_replicating_rows() {
    let catalog = parse(root(vec![folder("a", "A", vec![leaf("first"), folder("b", "B", vec![leaf("second"),
        folder("c", "C", vec![leaf("third")])]), leaf("fourth")])]));
    let groups = catalog.category_groups();
    assert_eq!(groups.iter().map(|group| group.label.as_str()).collect::<Vec<_>>(), ["A", "A / B", "A / B / C"]);
    assert_eq!(groups[0].indices, [0, 1, 2, 3]);
    assert_eq!(groups[1].indices, [1, 2]);
    assert_eq!(groups[2].indices, [2]);
    assert_eq!(catalog.category_indices(""), [0, 1, 2, 3]);
    for group in &groups { assert_eq!(catalog.category_indices(&group.id), group.indices); }
    assert_eq!(catalog.category_indices("unknown"), Vec::<usize>::new());
}

#[test]
fn literal_separator_and_structural_path_have_distinct_keys_and_ambiguous_legacy_label() {
    let catalog = parse(root(vec![folder("literal", "A / B", vec![leaf("one")]),
        folder("a", "A", vec![folder("b", "B", vec![leaf("two")])])]));
    let groups = catalog.category_groups();
    let matching: Vec<_> = groups.iter().filter(|group| group.label == "A / B").collect();
    assert_eq!(matching.len(), 2);
    assert_ne!(matching[0].id, matching[1].id);
    assert_eq!(matching[0].indices, [0]);
    assert_eq!(matching[1].indices, [1]);
    assert_eq!(catalog.resolve_category("A / B"), None);
    assert!(catalog.category_indices("A / B").is_empty());
}

#[test]
fn explicit_same_name_siblings_and_id_field_namespaces_remain_distinct() {
    let mut second = folder("one", "Same", vec![leaf("b")]);
    second.as_object_mut().unwrap().remove("id");
    second["fid"] = json!("one");
    let catalog = parse(root(vec![folder("one", "Same", vec![leaf("a")]), second]));
    let groups = catalog.category_groups();
    assert_eq!(groups.len(), 2);
    assert_ne!(groups[0].id, groups[1].id);
    assert_eq!(groups[0].indices, [0]);
    assert_eq!(groups[1].indices, [1]);
    assert_eq!(catalog.resolve_category("Same"), None);
}

#[test]
fn explicit_folder_rename_preserves_key_but_parent_move_does_not() {
    let first = parse(root(vec![folder("parent", "Parent", vec![folder("child", "Old", vec![])])]));
    let renamed = parse(root(vec![folder("parent", "Parent", vec![folder("child", "New", vec![])])]));
    let moved = parse(root(vec![folder("other-parent", "Parent", vec![folder("child", "New", vec![])])]));
    assert_eq!(first.category_choices()[1].id, renamed.category_choices()[1].id);
    assert_ne!(first.category_choices()[1].id, moved.category_choices()[1].id);
    assert_eq!(renamed.category_choices()[1].label, "Parent / New");
}

#[test]
fn request_alias_role_is_distinct_while_response_wrapper_and_next_page_retain_identity() {
    let root_value = |title| root(vec![json!({"type":"category","title":title,"request":{"folder":"stable","offset":0}})]);
    let first = PortalRequest::root().parse_response(&root_value("Old").to_string(), &profile()).unwrap();
    let renamed = PortalRequest::root().parse_response(&root_value("New").to_string(), &profile()).unwrap();
    assert_ne!(first.pending[0].category_identity(), renamed.pending[0].category_identity());
    let next = first.pending[0].parse_response(&json!({"type":"category","title":"Response wrapper",
        "items":[leaf("one"),{"type":"next","request":{}}]}).to_string(), &profile()).unwrap();
    assert_eq!(first.pending[0].category_identity(), next.pending[0].category_identity());
    assert_eq!(next.catalog.categories(), ["Old"]);
    assert_eq!(next.catalog.entries[0].category, "Old");
    assert_eq!(next.catalog.category_indices("Old"), [0]);
}

#[test]
fn anonymous_duplicate_nodes_require_identical_children() {
    let anonymous = |child| json!({"type":"category","title":"Anonymous","items":[leaf(child)]});
    let identical = parse(root(vec![anonymous("one"), anonymous("one")]));
    assert_eq!(identical.entries.len(), 1);
    assert_eq!(identical.category_choices().len(), 1);
    assert_eq!(identical.category_indices("Anonymous"), [0]);
    let conflict = root(vec![anonymous("one"), anonymous("two")]);
    assert_eq!(parse_vportal_response(&conflict.to_string(), &profile(), ""), Err(CatalogError::InvalidFormat));
}

#[test]
fn old_row_id_cannot_silently_choose_conflicting_media_between_distinct_folders() {
    let mut conflicting = leaf("same");
    conflicting["url"] = json!("/different");
    let value = root(vec![folder("left", "Same", vec![leaf("same")]), folder("right", "Same", vec![conflicting])]);
    assert_eq!(parse_vportal_response(&value.to_string(), &profile(), ""), Err(CatalogError::InvalidFormat));
}

#[test]
fn node_only_then_filtered_row_batches_merge_and_deduplicate_atomically() {
    let source = parse(root(vec![folder("a", "A", vec![folder("b", "B", vec![leaf("one"), leaf("two")])]),
        folder("empty", "Empty", vec![])]));
    let hierarchy = source.hierarchy.as_ref().unwrap();
    let mut assembled = Catalog::default();
    assembled.merge(Catalog { entries: Vec::new(), hierarchy: Some(hierarchy.nodes_only()) }).unwrap();
    assert_eq!(assembled.categories(), ["A", "A / B", "Empty"]);
    for entry in &source.entries {
        let mut part = hierarchy.for_entries([entry.id.as_str()]);
        part.nodes.clear();
        assembled.merge(Catalog { entries: vec![entry.clone()], hierarchy: Some(part) }).unwrap();
    }
    assembled.merge(Catalog { entries: Vec::new(), hierarchy: Some(hierarchy.clone()) }).unwrap();
    assert_eq!(assembled, source);
    assert_eq!(assembled.category_indices("Empty"), Vec::<usize>::new());
}

#[test]
fn invalid_graph_merge_never_publishes_incoming_rows_or_dangling_memberships() {
    let old = parse(root(vec![folder("a", "A", vec![leaf("one")])]));
    for mode in 0..3 {
        let mut candidate = old.clone();
        let mut incoming = parse(root(vec![folder("b", "B", vec![leaf("two")])]));
        let graph = incoming.hierarchy.as_mut().unwrap();
        match mode {
            0 => graph.nodes[0].parent = Some("missing-parent".into()),
            1 => graph.nodes[0].parent = Some(graph.nodes[0].id.clone()),
            _ => graph.memberships[0].entry_id = "missing-row".into(),
        }
        assert!(candidate.merge(incoming).is_err());
        assert_eq!(candidate, old);
    }
}

#[test]
fn graph_and_derived_membership_share_the_catalog_byte_budget() {
    let sample = parse(root(vec![folder("a", "A", vec![leaf("one")])]));
    let mut graph = CategoryHierarchy::default();
    for index in 0..128 {
        graph.nodes.push(CategoryNode { id: format!("folder-{index}"), parent: (index > 0).then(|| format!("folder-{}", index - 1)), label: "x".into(), anonymous_fingerprint: None, source_identity: None });
    }
    let mut entries = Vec::new();
    for index in 0..33_000 {
        let mut entry = sample.entries[0].clone();
        entry.id = format!("entry-{index}");
        graph.memberships.push(CategoryMembership { entry_id: entry.id.clone(), category_id: "folder-127".into() });
        entries.push(entry);
    }
    let mut target = Catalog::default();
    let oversized = Catalog { entries, hierarchy: Some(graph) };
    assert_eq!(oversized.checked_category_groups(), Err(CatalogError::LimitExceeded));
    assert_eq!(target.merge(oversized), Err(CatalogError::LimitExceeded));
    assert_eq!(target, Catalog::default());
}

#[test]
fn absent_hierarchy_keeps_flat_catalog_and_legacy_json_contract() {
    let mut source = parse(root(vec![folder("a", "A", vec![leaf("one")])]));
    source.hierarchy = None;
    let encoded = serde_json::to_value(&source).unwrap();
    assert!(encoded.get("hierarchy").is_none());
    let decoded: Catalog = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.resolve_category("A"), Some("A".into()));
    assert_eq!(decoded.category_indices("A"), [0]);
    assert_eq!(decoded.category_choices(), [CategoryChoice { id: "A".into(), label: "A".into() }]);
}

#[test]
fn anonymous_conflicts_across_root_pages_are_refused_without_partial_publication() {
    let anonymous = |child| json!({"type":"category","title":"Same","items":[leaf(child)]});
    let mut first = parse(root(vec![anonymous("one")]));
    let prior = first.clone();
    let second = parse(root(vec![anonymous("two")]));
    assert_eq!(first.merge(second), Err(CatalogError::InvalidFormat));
    assert_eq!(first, prior);
}

#[test]
fn identical_requests_in_distinct_named_aliases_do_not_collapse_pending_pages() {
    let value = root(vec![json!({"type":"category","title":"A","request":{"folder":1}}),
        json!({"type":"category","title":"B","request":{"folder":1}})]);
    let parsed = PortalRequest::root().parse_response(&value.to_string(), &profile()).unwrap();
    assert_eq!(parsed.pending.len(), 2);
    assert_ne!(parsed.pending[0].category_identity(), parsed.pending[1].category_identity());
    assert_eq!(parsed.catalog.categories(), ["A", "B"]);
}

#[test]
fn hierarchy_roundtrip_keeps_only_opaque_request_identity_and_public_labels() {
    let value = root(vec![json!({"type":"category","title":"Public folder",
        "request":{"folder":"private-request-sentinel"}})]);
    let parsed = PortalRequest::root().parse_response(&value.to_string(), &profile()).unwrap();
    let encoded = serde_json::to_string(&parsed.catalog).unwrap();
    assert!(!encoded.contains("private-request-sentinel"));
    assert!(!encoded.contains("portal.invalid"));
    let decoded: Catalog = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, parsed.catalog);
    assert_eq!(decoded.categories(), ["Public folder"]);
}

#[test]
fn coalesced_duplicate_graph_records_apply_unique_admission() {
    let mut catalog = parse(root(vec![folder("a", "A", vec![leaf("one")])]));
    let original = catalog.clone();
    let graph = catalog.hierarchy.as_ref().unwrap();
    let duplicate = CategoryHierarchy {
        nodes: vec![graph.nodes[0].clone(); MAX_CATALOG_ENTRIES + 1],
        memberships: vec![graph.memberships[0].clone(); MAX_CATALOG_ENTRIES + 1],
    };
    catalog.merge(Catalog { entries: Vec::new(), hierarchy: Some(duplicate) }).unwrap();
    assert_eq!(catalog, original);
}

fn plex_identity(section: i64, kind: PlexCategoryKind, source_id: &str) -> CategorySourceIdentity {
    CategorySourceIdentity::Plex { section, kind, source_id: source_id.into() }
}

#[test]
fn category_source_identity_is_closed_bounded_and_domain_separated() {
    use PlexCategoryKind::{Section, Show, Season};
    let identity = plex_identity(7, Show, "show-42");
    assert_eq!(identity.validate(), Ok(()));
    assert_eq!(identity.fingerprint(), "4c82d64191f6fe7223c6471f853fb35967a57acc7f2d2bc5d97a8cb5985efb37");
    assert_eq!(serde_json::from_value::<CategorySourceIdentity>(serde_json::to_value(&identity).unwrap()).unwrap(), identity);
    for other in [plex_identity(8, Show, "show-42"), plex_identity(7, Season, "show-42"),
        plex_identity(7, Show, "show-43")] {
        assert_ne!(identity.fingerprint(), other.fingerprint());
    }
    assert_eq!(plex_identity(7, Section, "7").validate(), Ok(()));
    assert_eq!(plex_identity(7, Show, &"x".repeat(256)).validate(), Ok(()));
    for invalid in [plex_identity(0, Section, "0"), plex_identity(-1, Section, "-1"),
        plex_identity(7, Section, "07"), plex_identity(7, Section, "8"),
        plex_identity(7, Show, ""), plex_identity(7, Show, &"x".repeat(257)),
        plex_identity(7, Season, "../42"), plex_identity(7, Show, "42?token=private")] {
        assert_eq!(invalid.validate(), Err(CatalogError::InvalidFormat));
    }
    for invalid in [
        json!({"provider":"other","section":7,"kind":"show","source_id":"42"}),
        json!({"provider":"plex","section":7,"kind":"movie","source_id":"42"}),
        json!({"provider":"plex","section":7,"kind":"show","source_id":"42","token":"private"}),
    ] {
        assert!(serde_json::from_value::<CategorySourceIdentity>(invalid).is_err());
    }
}

#[test]
fn optional_source_identity_keeps_legacy_nodes_and_survives_catalog_carriers() {
    let old = json!({"id":"old","parent":null,"label":"Legacy"});
    let node: CategoryNode = serde_json::from_value(old.clone()).unwrap();
    assert!(node.source_identity.is_none());
    assert_eq!(serde_json::to_value(node).unwrap(), old);

    let mut catalog = parse(root(vec![folder("a", "A", vec![leaf("one")])]));
    let identity = plex_identity(7, PlexCategoryKind::Section, "7");
    catalog.hierarchy.as_mut().unwrap().nodes[0].source_identity = Some(identity.clone());
    let graph = catalog.hierarchy.as_ref().unwrap();
    let mut restored = Catalog::default();
    restored.merge(Catalog { entries: Vec::new(), hierarchy: Some(graph.nodes_only()) }).unwrap();
    restored.merge(Catalog { entries: catalog.entries.clone(), hierarchy: Some(graph.for_entries(
        catalog.entries.iter().map(|entry| entry.id.as_str()))) }).unwrap();
    let encoded = serde_json::to_string(&restored).unwrap();
    assert_eq!(serde_json::from_str::<Catalog>(&encoded).unwrap(), catalog);
    assert_eq!(restored.hierarchy.unwrap().nodes[0].source_identity, Some(identity));
}

#[test]
fn invalid_or_ambiguous_provenance_never_partially_merges() {
    let mut old = parse(root(vec![folder("a", "A", vec![leaf("one")])]));
    old.hierarchy.as_mut().unwrap().nodes[0].source_identity = Some(plex_identity(7, PlexCategoryKind::Section, "7"));
    for mode in 0..3 {
        let mut target = old.clone();
        let mut incoming = parse(root(vec![folder("b", "B", vec![leaf("two")])]));
        let node = &mut incoming.hierarchy.as_mut().unwrap().nodes[0];
        node.source_identity = Some(plex_identity(7, PlexCategoryKind::Section, if mode == 0 { "invalid" } else { "7" }));
        if mode == 2 {
            node.id = old.hierarchy.as_ref().unwrap().nodes[0].id.clone();
            node.source_identity = Some(plex_identity(8, PlexCategoryKind::Section, "8"));
        }
        assert_eq!(target.merge(incoming), Err(CatalogError::InvalidFormat));
        assert_eq!(target, old);
    }
}

#[test]
fn category_source_identity_heap_bytes_are_part_of_the_shared_model_budget() {
    let catalog = parse(root(vec![folder("a", "A", vec![])]));
    let mut graph = catalog.hierarchy.unwrap();
    let original = graph.byte_len().unwrap();
    graph.nodes[0].source_identity = Some(plex_identity(7, PlexCategoryKind::Show, &"x".repeat(256)));
    assert_eq!(graph.byte_len().unwrap(), original + 256);
}


#[test]
fn checked_projection_matches_empty_merge_without_changing_raw_order_or_aliases() {
    let mut catalog = parse(root(vec![folder("a", "A", vec![leaf("first"),
        folder("b", "B", vec![leaf("second")])]), folder("literal", "A / B", vec![])]));
    let graph = catalog.hierarchy.as_mut().unwrap();
    graph.memberships.push(CategoryMembership {
        entry_id: catalog.entries[1].id.clone(), category_id: graph.nodes[0].id.clone(),
    });
    let before = catalog.clone();
    let mut legacy = catalog.clone();
    legacy.merge(Catalog::default()).unwrap();
    let groups = catalog.checked_category_groups().unwrap();
    assert_eq!(groups, legacy.category_groups());
    assert_eq!(groups[0].indices, [0, 1], "ancestor membership is not duplicated");
    assert_eq!(groups[1].indices, [1]);
    assert!(groups[2].indices.is_empty());
    assert_eq!(groups[1].label, groups[2].label);
    assert_ne!(groups[1].id, groups[2].id);
    assert_eq!(catalog, before);
}

#[test]
fn checked_projection_revalidates_public_graph_instead_of_trusting_prior_success() {
    let source = parse(root(vec![folder("a", "A", vec![leaf("one")]), folder("b", "B", vec![])]));
    for mode in 0..7 {
        let mut catalog = source.clone();
        assert!(catalog.checked_category_groups().is_ok());
        let graph = catalog.hierarchy.as_mut().unwrap();
        match mode {
            0 => graph.nodes[0].parent = Some("missing".into()),
            1 => graph.nodes[0].parent = Some(graph.nodes[0].id.clone()),
            2 => graph.nodes.push(graph.nodes[0].clone()),
            3 => graph.memberships[0].entry_id = "missing".into(),
            4 => graph.memberships.push(graph.memberships[0].clone()),
            5 => graph.memberships[0].category_id = "missing".into(),
            _ => {
                let identity = plex_identity(7, PlexCategoryKind::Section, "7");
                graph.nodes[0].source_identity = Some(identity.clone());
                graph.nodes[1].source_identity = Some(identity);
            }
        }
        let before = catalog.clone();
        let mut legacy = catalog.clone();
        assert_eq!(legacy.merge(Catalog::default()), Err(CatalogError::InvalidFormat));
        assert_eq!(catalog.checked_category_groups(), Err(CatalogError::InvalidFormat));
        assert_eq!(catalog, before);
    }
}

#[test]
fn checked_projection_counts_last_duplicate_row_but_preserves_raw_indices() {
    let mut catalog = parse(root(vec![folder("a", "A", vec![leaf("one")])]));
    let last = catalog.entries[0].clone();
    // merge(empty) budgets the last row of each ID; projection still indexes
    // the unmodified raw vector. Raw organization separately rejects duplicates.
    catalog.entries[0].description = "x".repeat(MAX_CATALOG_MODEL_BYTES);
    catalog.entries.push(last);
    assert_eq!(catalog.checked_category_groups().unwrap()[0].indices, [0, 1]);
    catalog.entries.swap(0, 1);
    assert_eq!(catalog.checked_category_groups(), Err(CatalogError::LimitExceeded));
}

#[test]
fn checked_projection_keeps_empty_hierarchy_and_flat_catalog_contracts() {
    let empty = parse(root(vec![folder("empty", "Empty", vec![])]));
    assert_eq!(empty.checked_category_groups().unwrap(), empty.category_groups());
    let mut flat = parse(root(vec![folder("a", "A", vec![leaf("one"), leaf("two")])]));
    flat.hierarchy = None;
    assert_eq!(flat.checked_category_groups().unwrap(), flat.category_groups());
}
