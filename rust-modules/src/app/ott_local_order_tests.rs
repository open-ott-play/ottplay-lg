use super::*;

fn source() -> BoundSource {
    BoundSource {
        id: "root".into(),
        name: "Movies".into(),
        kind: SourceKind::Folder,
        available: true,
        authority_ref: AuthorityRef(7),
        generation: 3,
        canonical_path: Some("/fixture/Movies".into()),
    }
}
fn update(context: &mut LocalContext, ids: &[&str]) -> LocalUpdate {
    let source = source();
    let items: Vec<_> = ids
        .iter()
        .map(|id| LocalItem {
            id: (*id).into(),
            playable: true,
        })
        .collect();
    context
        .update(
            OwnerToken(1),
            &CatalogSnapshot {
                source_id: &source.id,
                authority_ref: source.authority_ref,
                source_generation: source.generation,
                state: ScanState::Scanning,
                items: &items,
            },
        )
        .unwrap()
}
fn ids(items: &[&str]) -> Vec<String> {
    items.iter().map(|id| (*id).into()).collect()
}

#[test]
fn reset_retains_first_order_across_reshuffle_and_append_without_changing_transport() {
    let mut context = LocalContext::new(OwnerToken(1), source(), None);
    update(&mut context, &["a", "b", "c"]);
    context
        .note_intent(OwnerToken(1), LocalIntent::Selected("b"))
        .unwrap();
    context.checkpoint(OwnerToken(1), "b", 4321).unwrap();
    context
        .note_intent(OwnerToken(1), LocalIntent::Pause)
        .unwrap();
    assert!(context
        .reorder(OwnerToken(1), "", &ids(&["a", "b", "c"]), Some(1))
        .unwrap());
    let once = context.order().to_vec();
    context.reorder(OwnerToken(1), "", &once, Some(2)).unwrap();
    assert!(!update(&mut context, &["a", "b", "c", "d"]).continuation_allowed);
    let current = context.order().to_vec();
    context.reorder(OwnerToken(1), "", &current, None).unwrap();
    assert_eq!(context.order(), ids(&["a", "b", "c", "d"]));
    assert_eq!(context.current(), Some("b"));
    assert_eq!(context.position_ms(), 4321);
    assert!(!context.can_reset_order(""));
    assert!(context.paused);
}

#[test]
fn category_reorder_preserves_other_slots_and_stale_owner_is_atomic() {
    let mut context = LocalContext::new(OwnerToken(1), source(), None);
    update(&mut context, &["a", "other", "b", "outside", "c"]);
    let original = context.order().to_vec();
    assert!(context
        .reorder(OwnerToken(2), "Movies", &ids(&["a", "b", "c"]), Some(1))
        .is_err());
    assert_eq!(context.order(), original);
    context
        .reorder(OwnerToken(1), "Movies", &ids(&["a", "b", "c"]), Some(1))
        .unwrap();
    assert_eq!(context.order()[1], "other");
    assert_eq!(context.order()[3], "outside");
    context
        .reorder(OwnerToken(1), "Movies", &ids(&["a", "b", "c"]), None)
        .unwrap();
    assert_eq!(context.order(), original);
}

#[test]
fn remote_shuffle_has_truthful_reset_and_deleted_items_do_not_return() {
    let mut context = LocalContext::new(OwnerToken(1), source(), Some(1));
    update(&mut context, &["c", "a", "b"]);
    assert!(context.can_reset_order(""));
    context
        .note_intent(OwnerToken(1), LocalIntent::Stop)
        .unwrap();
    assert!(!update(&mut context, &["c", "a", "new"]).continuation_allowed);
    let current = context.order().to_vec();
    context.reorder(OwnerToken(1), "", &current, None).unwrap();
    assert_eq!(context.order(), ids(&["c", "a", "new"]));
    assert!(context.stopped);
    assert_eq!(context.seed, None);
}

#[test]
fn twelve_thousand_temporary_items_remain_in_ram_and_reset_exactly() {
    let mut context = LocalContext::new(OwnerToken(1), source(), None);
    let original: Vec<_> = (0..12_000).map(|n| format!("item-{n}")).collect();
    let borrowed: Vec<_> = original.iter().map(String::as_str).collect();
    update(&mut context, &borrowed);
    context
        .reorder(OwnerToken(1), "", &original, Some(7))
        .unwrap();
    let shuffled = context.order().to_vec();
    context.reorder(OwnerToken(1), "", &shuffled, None).unwrap();
    assert_eq!(context.order(), original);
}

#[test]
fn reset_records_original_new_batch_order_even_after_all_old_rows_disappear() {
    for replace_all in [false, true] {
        let mut context = LocalContext::new(OwnerToken(1), source(), Some(1));
        update(&mut context, &["a", "b"]);
        let expected = if replace_all {
            vec!["c", "d", "e"]
        } else {
            vec!["a", "b", "c", "d", "e"]
        };
        update(&mut context, &expected);
        assert!(context.can_reset_order(""));
        let current = context.order().to_vec();
        context.reorder(OwnerToken(1), "", &current, None).unwrap();
        assert_eq!(context.order(), ids(&expected));
    }
}

#[test]
fn overlapping_all_and_category_resets_share_one_canonical_order() {
    for (category_first, reset_all_first) in
        [(false, false), (false, true), (true, false), (true, true)]
    {
        let mut context = LocalContext::new(OwnerToken(1), source(), None);
        let original = ids(&["a", "b", "other", "c"]);
        update(&mut context, &["a", "b", "other", "c"]);
        if category_first {
            context
                .reorder(OwnerToken(1), "Movies", &ids(&["a", "b", "c"]), Some(1))
                .unwrap();
        }
        context
            .reorder(OwnerToken(1), "", &original, Some(2))
            .unwrap();
        if !category_first {
            context
                .reorder(OwnerToken(1), "Movies", &ids(&["a", "b", "c"]), Some(3))
                .unwrap();
        }
        if reset_all_first {
            let all = context.order().to_vec();
            context.reorder(OwnerToken(1), "", &all, None).unwrap();
            assert_eq!(context.order(), original);
            assert!(!context
                .reorder(OwnerToken(1), "Movies", &ids(&["a", "b", "c"]), None)
                .unwrap());
            assert_eq!(context.order(), original);
        } else {
            context
                .reorder(OwnerToken(1), "Movies", &ids(&["a", "b", "c"]), None)
                .unwrap();
            assert_eq!(
                context
                    .order()
                    .iter()
                    .filter(|id| id.as_str() != "other")
                    .cloned()
                    .collect::<Vec<_>>(),
                ids(&["a", "b", "c"])
            );
        }
        let all = context.order().to_vec();
        context.reorder(OwnerToken(1), "", &all, None).unwrap();
        assert_eq!(context.order(), original);
        assert!(!context.can_reset_order("Movies"));
        assert!(!context
            .reorder(OwnerToken(1), "Movies", &ids(&["a", "b", "c"]), None)
            .unwrap());
        assert_eq!(context.order(), original);
    }
}
