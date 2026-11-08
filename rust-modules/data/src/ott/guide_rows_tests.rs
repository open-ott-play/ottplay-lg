use super::*;
use crate::ott::{epg, guide, MAX_CATALOG_BYTES, MAX_CATALOG_ENTRIES, MAX_METADATA_BYTES};

fn origin() -> i64 { epg::timestamp("20260101000000 +0000").unwrap() }
fn stamp(second: i64) -> String {
    format!("20260101{:02}{:02}{:02} +0000", second / 3600, second / 60 % 60, second % 60)
}
fn programme(channel: &str, start: i64, end: i64, title: &str) -> String {
    format!("<programme channel='{channel}' start='{}' stop='{}'><title>{title}</title></programme>", stamp(start), stamp(end))
}
fn selector(id: &str) -> Selector {
    Selector { entry_id: "row".into(), provider_id: Some(id.into()), tvg_name: String::new(), title: String::new(), shift_seconds: 0 }
}
fn index(xml: &str) -> Index { Index::parse_with_cancel(xml, &mut || false).unwrap() }

#[test]
fn indexed_selection_matches_existing_selector_at_overlaps_ties_and_exclusive_ends() {
    let xml = format!("<tv>{}{}{}{}{}{}{}</tv>",
        programme("one", 0, 300, "Long"), programme("one", 40, 80, "Short"),
        programme("one", 40, 100, "First tie"), programme("one", 40, 100, "Last tie"),
        programme("one", 40, 100, "Last tie"), programme("one", 160, 200, "Later"),
        programme("one", 300, 400, "After"));
    let source = epg::parse_xmltv(&xml, "one").unwrap();
    let indexed = index(&xml);
    for shift in [-30, 0, 25] {
        let mut entry = selector("one"); entry.shift_seconds = shift;
        for second in (0..450).step_by(5) {
            let now = origin() + second;
            let selected = guide::select(&source, "one", guide::Query::CurrentNext { now }, shift).unwrap();
            let expected_current = selected.iter().find(|p| p.start <= now && now < p.end)
                .map(|p| CurrentProgramme { title: p.title.clone(), start_s: p.start, end_s: p.end });
            let boundary = expected_current.as_ref().map_or(now, |p| p.end_s);
            let expected_next = selected.iter().any(|p| p.start >= boundary);
            let actual = indexed.select(&entry, now).unwrap();
            assert!(actual.matched);
            assert_eq!(actual.current, expected_current, "second={second} shift={shift}");
            assert_eq!(actual.has_next, expected_next);
        }
    }
}

#[test]
fn matching_preserves_exact_id_distinct_id_ambiguity_and_name_fallback_order() {
    let xml = format!("<tv><channel id='one'><display-name>  World   NEWS </display-name></channel><channel id='one'><display-name>World News</display-name></channel><channel id='two'><display-name>Other</display-name></channel>{}{}</tv>", programme("one",0,100,"One"), programme("two",0,100,"Two"));
    let indexed = index(&xml);
    let mut entry = selector("missing"); entry.tvg_name = "world news".into();
    assert_eq!(indexed.select(&entry, origin()).unwrap().current.unwrap().title, "One");
    entry.provider_id = Some("two".into());
    assert_eq!(indexed.select(&entry, origin()).unwrap().current.unwrap().title, "Two");
    let ambiguous = xml.replace("<channel id='two'>", "<channel id='three'><display-name>World News</display-name></channel><channel id='two'>");
    entry.provider_id = None; entry.title = "Other".into();
    assert!(!index(&ambiguous).select(&entry, origin()).unwrap().matched);
    entry.tvg_name = "absent".into();
    assert_eq!(index(&ambiguous).select(&entry, origin()).unwrap().current.unwrap().title, "Two");
}

#[test]
fn programme_only_ids_and_empty_declared_channels_remain_distinguishable_from_misses() {
    let xml = format!("<tv><channel id='empty'/>{}</tv>", programme("implicit",0,100,"Implicit"));
    let indexed = index(&xml);
    assert!(indexed.select(&selector("implicit"), origin()).unwrap().current.is_some());
    let empty = indexed.select(&selector("empty"), origin()).unwrap();
    assert!(empty.matched && empty.current.is_none() && !empty.has_next);
    assert!(!indexed.select(&selector("missing"), origin()).unwrap().matched);
}

#[test]
fn malformed_time_or_oversized_metadata_invalidates_only_its_channel() {
    let xml = format!("<tv>{}<programme channel='bad' start='invalid'/><programme channel='large' start='{}' stop='{}'><desc><![CDATA[{}]]></desc></programme></tv>", programme("good",0,100,"Good"), stamp(0), stamp(100), "x".repeat(MAX_METADATA_BYTES + 1));
    let indexed = index(&xml);
    assert_eq!(indexed.select(&selector("good"), origin()).unwrap().current.unwrap().title, "Good");
    assert_eq!(indexed.select(&selector("bad"), origin()), Err(CatalogError::InvalidFormat));
    assert_eq!(indexed.select(&selector("large"), origin()), Err(CatalogError::LimitExceeded));
    assert!(epg::parse_xmltv(&xml, "good").is_ok());
    assert!(epg::parse_xmltv(&xml, "bad").is_err());
}

#[test]
fn first_nonempty_source_claims_even_when_it_has_only_a_next_programme() {
    let first = index(&format!("<tv>{}</tv>", programme("one",100,200,"Future")));
    let second = index(&format!("<tv>{}</tv>", programme("one",0,200,"Other current")));
    let claimed = [first, second].iter().map(|source| source.select(&selector("one"), origin()).unwrap())
        .find(|selection| selection.current.is_some() || selection.has_next).unwrap();
    assert!(claimed.current.is_none() && claimed.has_next);
    assert_eq!(claimed.next_transition_s, Some(origin() + 100));
}

#[test]
fn next_transition_includes_an_overlapping_takeover_and_current_expiry() {
    let indexed = index(&format!("<tv>{}{}</tv>", programme("one",0,300,"Long"), programme("one",100,150,"Short")));
    let entry = selector("one");
    assert_eq!(indexed.select(&entry, origin()+50).unwrap().next_transition_s, Some(origin()+100));
    assert_eq!(indexed.select(&entry, origin()+100).unwrap().next_transition_s, Some(origin()+150));
    assert_eq!(indexed.select(&entry, origin()+150).unwrap().current.unwrap().title, "Long");
    assert_eq!(indexed.select(&entry, origin()+300).unwrap().next_transition_s, None);
}

#[test]
fn all_channel_times_are_checked_before_shifted_lookup_including_inactive_programmes() {
    let xml = format!("<tv><programme channel='one' start='19700101000000 +0000' stop='19700101000001 +0000'/>{}</tv>", programme("one",0,100,"Current"));
    let indexed = index(&xml);
    let mut entry = selector("one"); entry.shift_seconds = -1;
    assert_eq!(indexed.select(&entry, origin()), Err(CatalogError::InvalidFormat));
    entry.shift_seconds = i64::MAX;
    assert_eq!(indexed.select(&entry, origin()), Err(CatalogError::InvalidFormat));
    entry.shift_seconds = 0;
    assert_eq!(indexed.select(&entry, -1), Err(CatalogError::InvalidFormat));
    assert!(indexed.select(&entry, origin()).unwrap().current.is_some());
}

#[test]
fn shared_channel_selectors_keep_independent_shifts_and_catalogue_ids() {
    let indexed = index(&format!("<tv>{}{}</tv>", programme("one",0,100,"Earlier"), programme("one",100,200,"Later")));
    let mut snapshot = Snapshot { now_s: origin()+125, next_transition_s: None, programmes: Default::default() };
    for n in 0..1000 {
        let mut entry = selector("one"); entry.entry_id = format!("row-{n}"); entry.shift_seconds = if n%2 == 0 {0} else {100};
        let selected = indexed.select(&entry, snapshot.now_s).unwrap();
        snapshot.programmes.insert(entry.entry_id, selected.current.unwrap());
    }
    assert_eq!(snapshot.programmes.len(),1000);
    assert_eq!(snapshot.programmes["row-0"].title,"Later");
    assert_eq!(snapshot.programmes["row-1"].title,"Earlier");
}

#[test]
fn whole_document_structure_and_existing_caps_remain_fatal() {
    for xml in ["text", "<tv>", "<!DOCTYPE tv><tv/>", "<tv><tv/></tv>", "<tv><programme channel='one'><title>&bad;</title></programme></tv>"] {
        assert!(matches!(Index::parse_with_cancel(xml,&mut ||false),Err(BuildError::Invalid(_))));
    }
    assert!(matches!(Index::parse_with_cancel(&" ".repeat(MAX_CATALOG_BYTES+1),&mut ||false),Err(BuildError::Invalid(CatalogError::LimitExceeded))));
    let xml = format!("<tv>{}</tv>", "<programme channel='one'/>".repeat(MAX_CATALOG_ENTRIES+1));
    assert!(matches!(Index::parse_with_cancel(&xml,&mut ||false),Err(BuildError::Invalid(CatalogError::LimitExceeded))));
}

#[test]
fn cancellation_is_distinct_and_does_not_return_a_partial_index() {
    assert!(matches!(Index::parse_with_cancel("<tv/>",&mut ||true),Err(BuildError::Cancelled)));
    let xml = format!("<tv>{}{}</tv>", programme("one",0,100,"First"), programme("two",0,100,"Second"));
    let mut polls=0;
    assert!(matches!(Index::parse_with_cancel(&xml,&mut ||{polls+=1;polls>=4}),Err(BuildError::Cancelled)));
    assert!(polls>=4);
    assert_eq!(index(&xml).select(&selector("two"),origin()).unwrap().current.unwrap().title,"Second");
}

#[test]
fn compact_rows_preserve_decoded_titles_without_retaining_full_descriptions() {
    let xml = format!("<tv><programme channel='one' start='{}' stop='{}'><title> A &amp; <![CDATA[B < C]]> </title><desc>{}</desc></programme></tv>",stamp(0),stamp(100),"description ".repeat(1000));
    let indexed=index(&xml);
    let actual=indexed.select(&selector("one"),origin()).unwrap().current.unwrap();
    assert_eq!(actual.title,"A & B < C");
    let full=epg::parse_xmltv(&xml,"one").unwrap();
    assert_eq!(full[0].title,actual.title);
    assert!(full[0].description.len()>10000);
    assert_eq!(indexed.schedules[indexed.ids["one"]].rows.len(),1);
}

#[test]
fn retained_index_budget_is_bounded_and_failed_charge_does_not_wrap() {
    let mut retained=RetainedBytes::default();
    retained.add(MAX_CATALOG_MODEL_BYTES).unwrap();
    assert_eq!(retained.add(1),Err(BuildError::Invalid(CatalogError::LimitExceeded)));
    assert_eq!(retained.0,MAX_CATALOG_MODEL_BYTES);
    assert_eq!(retained.add(usize::MAX),Err(BuildError::Invalid(CatalogError::LimitExceeded)));
    assert_eq!(retained.0,MAX_CATALOG_MODEL_BYTES);
}
