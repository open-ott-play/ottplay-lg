//! Baseline-compatible ancestor regressions. These call the actual parser and merge.
use super::*;

fn profile() -> ProviderProfile {
    ProviderProfile {
        id: "ancestor-fixture".into(),
        name: "Ancestor fixture".into(),
        kind: ProviderKind::VPortal,
        source: "portal::[key:test]https://portal.invalid/api".into(),
        source_alias: String::new(),
        enabled: true,
    }
}

#[test]
fn vportal_ancestors_inline_exposes_every_real_parent_without_duplicating_media() {
    let page = parse_vportal_response(
        r#"{"type":"videoportal","items":[{"type":"category","title":"A","items":[{"type":"category","title":"B","items":[{"type":"category","title":"C","items":[{"type":"stream","id":1,"title":"One","url":"/one"}]}]}]}]}"#,
        &profile(), "",
    ).unwrap();
    assert_eq!(page.catalog.entries.len(), 1, "ancestors must not manufacture media rows");
    assert_eq!(page.catalog.entries[0].category, "A / B / C");
    assert_eq!(page.catalog.categories(), vec!["A", "A / B", "A / B / C"]);
}

#[test]
fn vportal_ancestors_lazy_page_retains_real_parent_categories() {
    let p = profile();
    let root = PortalRequest::root().parse_response(
        r#"{"type":"videoportal","items":[{"type":"category","title":"A","items":[{"type":"category","title":"B","items":[{"type":"category","title":"C","request":{"folder":"c"}}]}]}]}"#,
        &p,
    ).unwrap();
    assert_eq!(root.pending.len(), 1);
    let leaf = root.pending[0].parse_response(
        r#"{"type":"category","items":[{"type":"stream","id":1,"title":"One","url":"/one"}]}"#,
        &p,
    ).unwrap();
    let mut catalog = root.catalog;
    catalog.merge(leaf.catalog).unwrap();
    assert_eq!(catalog.entries.len(), 1);
    assert_eq!(catalog.categories(), vec!["A", "A / B", "A / B / C"]);
}

#[test]
fn vportal_ancestors_empty_real_folder_remains_selectable() {
    let page = parse_vportal_response(
        r#"{"type":"videoportal","items":[{"type":"category","title":"Empty","items":[]}]}"#,
        &profile(), "",
    ).unwrap();
    assert!(page.catalog.entries.is_empty());
    assert!(page.pending.is_empty());
    assert_eq!(page.catalog.categories(), vec!["Empty"]);
}

#[test]
fn vportal_ancestors_literal_separator_is_not_an_invented_parent() {
    let page = parse_vportal_response(
        r#"{"type":"videoportal","items":[{"type":"category","title":"A / B","items":[{"type":"stream","id":1,"title":"One","url":"/one"}]}]}"#,
        &profile(), "",
    ).unwrap();
    assert_eq!(page.catalog.entries.len(), 1);
    assert_eq!(page.catalog.categories(), vec!["A / B"]);
}

#[test]
fn vportal_ancestors_repeated_page_keeps_one_raw_media_row() {
    let input = r#"{"type":"videoportal","items":[{"type":"category","title":"A","items":[{"type":"category","title":"B","items":[{"type":"category","title":"C","items":[{"type":"stream","id":1,"title":"One","url":"/one"}]}]}]}]}"#;
    let mut catalog = parse_vportal_response(input, &profile(), "").unwrap().catalog;
    let original = catalog.entries[0].clone();
    catalog.merge(parse_vportal_response(input, &profile(), "").unwrap().catalog).unwrap();
    assert_eq!(catalog.entries, vec![original]);
}
