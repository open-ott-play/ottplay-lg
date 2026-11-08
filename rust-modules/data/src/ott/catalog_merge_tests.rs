//! Merge cost and admission regressions. The legacy oracle below is the exact
//! pre-optimization algorithm; categories retains its old graph checks for this
//! test-only oracle. No wall-clock benchmark or global allocator hook.
use super::*;
use serde_json::json;
use std::collections::BTreeMap;

fn row(id: &str, title: &str) -> CatalogEntry {
    CatalogEntry {
        synopsis: None,
        id: id.into(), title: title.into(), category: String::new(),
        description: String::new(), poster: String::new(),
        media: MediaLocator::VPortal { request: json!({"id": id, "title": title}) },
        duration_ms: None, provider_id: None, provider_id_field: None, live: None,
    }
}
fn flat(entries: Vec<CatalogEntry>) -> Catalog { Catalog { entries, hierarchy: None } }
fn graph(entry: &str) -> CategoryHierarchy {
    CategoryHierarchy {
        nodes: vec![CategoryNode { id: "folder".into(), parent: None, label: "Folder".into(),
            anonymous_fingerprint: None, source_identity: None }],
        memberships: vec![CategoryMembership { entry_id: entry.into(), category_id: "folder".into() }],
    }
}

fn legacy_model_bytes(entry:&CatalogEntry)->usize {
    let media=match &entry.media {
        MediaLocator::Http {url}=>url.len(),MediaLocator::Local {relative_path}=>relative_path.len(),
        MediaLocator::VPortal {request}=>request.to_string().len(),
        MediaLocator::Plex {server_id,rating_key}=>server_id.len()+rating_key.len(),
        MediaLocator::Xtream {stream_id,stream_kind,extension,series_id,..}=>stream_id.len()+stream_kind.len()+extension.len()+series_id.as_ref().map_or(0,String::len),
        MediaLocator::Stalker {cmd,stream_kind,series}=>cmd.len()+stream_kind.len()+series.len(),
        MediaLocator::StalkerApi {channel_id}=>channel_id.len(),
        MediaLocator::ServerLibrary {source_id,item_id,relative_path,root_alias}=>source_id.len()+item_id.len()+relative_path.as_ref().map_or(0,String::len)+root_alias.as_ref().map_or(0,String::len),
    };
    entry.id.len()+entry.title.len()+entry.category.len()+entry.description.len()+entry.poster.len()+media
        +entry.provider_id.as_ref().map_or(0,String::len)+entry.provider_id_field.as_ref().map_or(0,String::len)+entry.live.as_ref().map_or(0,|live|live.tvg_name.len()+live.catchup.as_ref().map_or(0,|catchup|catchup.source.len()))+std::mem::size_of::<CatalogEntry>()
}

fn legacy_merge(target: &mut Catalog, batch: Catalog) -> Result<(), CatalogError> {
    let mut ids: BTreeSet<&str> = target.entries.iter().map(|e| e.id.as_str()).collect();
    ids.extend(batch.entries.iter().map(|e| e.id.as_str()));
    if ids.len() > MAX_CATALOG_ENTRIES { return Err(CatalogError::LimitExceeded); }
    let mut positions: BTreeMap<String, usize> = target.entries.iter().enumerate().map(|(i,e)| (e.id.clone(), i)).collect();
    let mut sizes: BTreeMap<&str, usize> = target.entries.iter().map(|e| (e.id.as_str(), legacy_model_bytes(e))).collect();
    for entry in &batch.entries { sizes.insert(&entry.id, legacy_model_bytes(entry)); }
    let mut hierarchy = target.hierarchy.clone();
    if let Some(incoming) = &batch.hierarchy {
        hierarchy.get_or_insert_with(CategoryHierarchy::default).merge(incoming)?;
    }
    let rows_bytes = sizes.values().try_fold(0usize, |total, size| total.checked_add(*size))
        .ok_or(CatalogError::LimitExceeded)?;
    let hierarchy_bytes = if let Some(hierarchy) = &hierarchy {
        hierarchy.validate(&ids)?;
        hierarchy.byte_len().and_then(|bytes| bytes.checked_add(hierarchy.projection_bytes().ok()?))
            .ok_or(CatalogError::LimitExceeded)?
    } else { 0 };
    if rows_bytes.checked_add(hierarchy_bytes).is_none_or(|bytes| bytes > MAX_CATALOG_MODEL_BYTES) {
        return Err(CatalogError::LimitExceeded);
    }
    for entry in batch.entries {
        if let Some(&i) = positions.get(&entry.id) { target.entries[i] = entry; }
        else { positions.insert(entry.id.clone(), target.entries.len()); target.entries.push(entry); }
    }
    target.hierarchy = hierarchy;
    Ok(())
}

#[test]
fn merge_costs_only_final_row_winners() {
    let mut catalog = flat(vec![row("old", "superseded old"), row("keep", "retained")]);
    let batch = flat(vec![row("old", "superseded batch"), row("new", "superseded new"),
        row("old", "final old"), row("new", "final new")]);
    let mut observed = Vec::new();
    // This is the same always-compiled cost seam called by public merge.
    // The observer invokes the real model_bytes, without a production test hook.
    catalog.merge_with_cost(batch, |entry| {
        let cost = model_bytes(entry);
        observed.push((entry.id.clone(), entry.title.clone(), cost));
        cost
    }).unwrap();
    assert_eq!(observed, [
        ("keep".to_owned(), "retained".to_owned(), legacy_model_bytes(&row("keep", "retained"))),
        ("new".to_owned(), "final new".to_owned(), legacy_model_bytes(&row("new", "final new"))),
        ("old".to_owned(), "final old".to_owned(), legacy_model_bytes(&row("old", "final old"))),
    ]);
    assert_eq!(catalog.entries, [row("old", "final old"), row("keep", "retained"), row("new", "final new")]);
}

#[test]
fn merge_preserves_last_existing_position_and_first_new_position() {
    let mut catalog = flat(vec![row("same", "first survives"), row("middle", "middle"), row("same", "last old")]);
    catalog.merge(flat(vec![row("same", "first replacement"), row("new", "first new"),
        row("same", "last replacement"), row("new", "last new")])).unwrap();
    assert_eq!(catalog.entries, [row("same", "first survives"), row("middle", "middle"),
        row("same", "last replacement"), row("new", "last new")]);
}

#[test]
fn merge_matches_legacy_for_bounded_duplicate_and_hierarchy_cases() {
    let mut valid = flat(vec![row("old", "old")]); valid.hierarchy = Some(graph("old"));
    let mut foreign = valid.clone(); foreign.hierarchy.as_mut().unwrap().memberships[0].entry_id = "missing".into();
    let mut cycle = valid.clone(); cycle.hierarchy.as_mut().unwrap().nodes[0].parent = Some("folder".into());
    let mut conflict = flat(vec![row("old", "changed")]); conflict.hierarchy = Some(graph("old"));
    conflict.hierarchy.as_mut().unwrap().nodes[0].label = "Conflict".into();
    let mut duplicate_member = valid.clone();
    duplicate_member.hierarchy.as_mut().unwrap().memberships.push(graph("old").memberships[0].clone());
    let bases = [Catalog::default(), flat(vec![row("old", "one"), row("old", "two")]),
        valid, foreign, cycle, duplicate_member];
    let batches = [Catalog::default(), flat(vec![row("old", "update"), row("new", "a"), row("new", "b")]), conflict];
    for base in bases {
        for batch in &batches {
            let mut actual = base.clone(); let mut expected = base.clone();
            let wanted = legacy_merge(&mut expected, batch.clone());
            assert_eq!(actual.merge(batch.clone()), wanted);
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn merge_final_winner_budget_and_overflow_are_atomic() {
    let mut catalog = flat(vec![row("same", "too large")]);
    catalog.merge_with_cost(flat(vec![row("same", "also too large"), row("same", "fits")]), |entry| {
        if entry.title == "fits" { MAX_CATALOG_MODEL_BYTES } else { usize::MAX }
    }).unwrap();
    assert_eq!(catalog.entries, [row("same", "fits")]);
    let before = catalog.clone();
    assert_eq!(catalog.merge_with_cost(flat(vec![row("new", "extra")]), |entry| {
        if entry.id == "same" { MAX_CATALOG_MODEL_BYTES } else { 1 }
    }), Err(CatalogError::LimitExceeded));
    assert_eq!(catalog, before);
    assert_eq!(catalog.merge_with_cost(flat(vec![row("new", "overflow")]), |_| usize::MAX),
        Err(CatalogError::LimitExceeded));
    assert_eq!(catalog, before);
}

#[test]
fn merge_unique_entry_gate_precedes_cost_and_does_not_mutate() {
    let mut catalog = flat(vec![row("old", "original")]);
    let before = catalog.clone();
    let batch = flat((0..MAX_CATALOG_ENTRIES).map(|i| row(&format!("new-{i}"), "new")).collect());
    assert_eq!(catalog.merge_with_cost(batch, |_| panic!("over-count candidate must not be costed")),
        Err(CatalogError::LimitExceeded));
    assert_eq!(catalog, before);
}

#[test]
fn empty_merge_still_revalidates_publicly_mutated_hierarchy() {
    let mut catalog = flat(vec![row("old", "original")]);
    catalog.hierarchy = Some(graph("old"));
    catalog.merge(Catalog::default()).unwrap();
    catalog.hierarchy.as_mut().unwrap().memberships[0].entry_id = "not-present".into();
    let invalid = catalog.clone();
    assert_eq!(catalog.merge(Catalog::default()), Err(CatalogError::InvalidFormat));
    assert_eq!(catalog, invalid);
}

#[test]
fn json_length_matches_compact_serialization_without_a_payload_buffer() {
    // The counting sink owns only one word, regardless of the request size.
    assert_eq!(std::mem::size_of::<JsonLength>(), std::mem::size_of::<usize>());
    let values = [json!(null), json!(true), json!(-123), json!(u64::MAX), json!(1.25e100),
        json!("quotes\" slash\\ newline\n tab\t control\u{0001} café 🦀"),
        json!({"private-token-key": ["x".repeat(64 * 1024), {"nested": [null, false, 42]}]})];
    for value in values {
        let expected = value.to_string().len();
        assert_eq!(json_model_bytes(&value), expected);
        let mut entry = row("id", "title");
        entry.media = MediaLocator::VPortal { request: value };
        assert_eq!(model_bytes(&entry), std::mem::size_of::<CatalogEntry>() + 2 + 5 + expected);
    }
}

#[test]
fn json_length_overflow_saturates_instead_of_wrapping_under_budget() {
    use std::io::Write;
    let mut length = JsonLength(usize::MAX - 1);
    assert_eq!(length.write(b"abc").unwrap(), 3);
    assert_eq!(length.0, usize::MAX);
    assert_eq!(length.write(b"more").unwrap(), 4);
    assert_eq!(length.0, usize::MAX);
}


#[test]
fn twenty_thousand_rows_preserve_updates_atomicity_and_exact_serialization_work() {
    const HALF: usize = 10_000;
    fn id(index: usize) -> String { format!("large-{index:05}") }
    fn merge_observed(catalog: &mut Catalog, batch: Catalog, expected: usize) {
        let mut observed = Vec::new();
        catalog.merge_with_cost(batch, |entry| {
            let cost = model_bytes(entry);
            observed.push((entry.id.clone(), entry.title.clone(), cost));
            cost
        }).unwrap();
        assert_eq!(catalog.entries.len(), expected);
        assert_eq!(observed.len(), expected, "one real sizing visit per final winner");
        assert_eq!(observed.iter().map(|(id, _, _)| id).collect::<BTreeSet<_>>().len(), expected);
        assert!(!observed.iter().any(|(_, title, _)| title.starts_with("discarded")));
        let mut legacy_costs: Vec<_> = catalog.entries.iter().map(|entry|
            (entry.id.clone(), entry.title.clone(), legacy_model_bytes(entry))).collect();
        legacy_costs.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        // Exact cost parity includes compact JSON bytes, without claiming an
        // observed count of internal serializer writes or a wall-clock bound.
        assert_eq!(observed, legacy_costs);
    }
    let mut catalog = Catalog::default();
    merge_observed(&mut catalog,
        flat((0..HALF).map(|index| row(&id(index), "initial")).collect()), HALF);
    // This is a per-merge linear-work bound, not a claim of subquadratic
    // cumulative progressive publication: all retained winners are visited.
    merge_observed(&mut catalog,
        flat((HALF..2 * HALF).map(|index| row(&id(index), "initial")).collect()), 2 * HALF);
    for (index, entry) in catalog.entries.iter().enumerate() {
        assert_eq!(entry, &row(&id(index), "initial"));
    }
    merge_observed(&mut catalog, flat(vec![
        row(&id(0), "discarded replacement"),
        row(&id(2 * HALF), "discarded append"),
        row(&id(HALF), "final middle"),
        row(&id(0), "final first"),
        row(&id(2 * HALF - 1), "final last"),
        row(&id(2 * HALF), "final appended"),
    ]), 2 * HALF + 1);
    for (index, entry) in catalog.entries.iter().enumerate() {
        let title = if index == 0 { "final first" }
            else if index == HALF { "final middle" }
            else if index == 2 * HALF - 1 { "final last" }
            else if index == 2 * HALF { "final appended" }
            else { "initial" };
        assert_eq!(entry, &row(&id(index), title));
    }
    let before = catalog.clone();
    let mut invalid = flat(vec![row("rejected", "must not append")]);
    let mut hierarchy = graph("rejected");
    hierarchy.nodes[0].parent = Some("missing-parent".into());
    invalid.hierarchy = Some(hierarchy);
    assert_eq!(catalog.merge(invalid), Err(CatalogError::InvalidFormat));
    assert_eq!(catalog, before, "graph refusal cannot change any of the 20k retained rows");
}
