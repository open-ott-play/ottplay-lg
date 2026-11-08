use super::*;

fn fixture(rule: &str) -> (ProviderProfile, CatalogEntry) {
    let profile = ProviderProfile { id: "m3u".into(), name: "TV".into(),
        kind: ProviderKind::M3u, source: "https://guide.invalid/list.m3u".into(),
        source_alias: String::new(), enabled: true };
    let playlist = format!("#EXTM3U\n#EXTINF:-1 {rule},News\nhttps://video.invalid/live");
    let mut catalog = super::super::parse_m3u(&playlist, &profile).unwrap();
    (profile, catalog.entries.remove(0))
}

#[test]
fn records_query_covers_declared_or_fallback_retention() {
    let now = 2_000_000_000;
    let (profile, entry) = fixture("catchup=\"shift\" catchup-days=\"7\"");
    assert_eq!(query(&profile, &entry, 1, now).unwrap(),
        Query::Records { start: now - 7 * 86400, end: now });
    let (profile, entry) = fixture("catchup=\"shift\"");
    assert_eq!(query(&profile, &entry, 8760, now).unwrap(),
        Query::Records { start: now - 365 * 86400, end: now });
    assert!(query(&profile, &entry, 8761, now).is_err());
    assert!(query(&profile, &entry, 0, now).is_err());
    assert!(Query::Records { start: now - 365 * 86400 - 1, end: now }.validate().is_err());
    assert!(Query::Records { start: -1, end: now }.validate().is_err());
    assert!(Query::Records { start: now, end: now }.validate().is_err());
}

#[test]
fn records_query_refuses_missing_invalid_and_non_m3u_sources() {
    for rule in ["", "catchup=\"none\"", "catchup=\"shift\" catchup-days=\"0\"",
        "catchup=\"default\" catchup-days=\"7\" catchup-source=\"file:///private/{utc}\""] {
        let (profile, entry) = fixture(rule);
        assert!(query(&profile, &entry, 168, 2_000_000_000).is_err(), "{rule}");
    }
    let (profile, entry) = fixture("catchup=\"shift\" catchup-days=\"7\"");
    let mut changed = profile.clone();
    changed.kind = ProviderKind::Xtream;
    changed.source = r#"{"server":"https://portal.invalid","username":"test","password":"test"}"#.into();
    assert!(query(&changed, &entry, 168, 2_000_000_000).is_err());
    changed = profile.clone();
    changed.enabled = false;
    assert!(query(&changed, &entry, 168, 2_000_000_000).is_err());
    let mut changed = entry.clone();
    changed.duration_ms = Some(1000);
    assert!(query(&profile, &changed, 168, 2_000_000_000).is_err());
    changed = entry;
    changed.media = MediaLocator::Local { relative_path: "movie.ts".into() };
    assert!(query(&profile, &changed, 168, 2_000_000_000).is_err());
}

fn programme(title: &str, start: i64, end: i64, description: &str) -> Programme {
    Programme { channel: "news".into(), start, end, title: title.into(), description: description.into() }
}
fn result(programmes: Vec<Programme>) -> ResultSet {
    ResultSet { context: super::super::guide::Context { generation: 9, profile_id: "p".into(), entry_id: "e".into() },
        query: Query::Records { start: 100, end: 1000 }, coverage: Coverage::RequestedWindow, programmes }
}

#[test]
fn records_projection_latest_exact_title_and_utf16_order() {
    let value = result(vec![programme("News", 200, 300, "old"),
        programme("news", 400, 500, "case distinct"), programme("News", 600, 700, "latest"),
        programme("\u{e000}", 700, 800, "bmp"), programme("\u{10000}", 800, 900, "non-bmp")]);
    assert_eq!(indices(&value).unwrap(), vec![2, 1, 4, 3]);
    assert_eq!(value.programmes[0].description, "old", "projection must not destroy retained rows");
}

#[test]
fn records_projection_filters_before_dedup_and_preserves_ties() {
    let value = result(vec![programme("News", 200, 300, "first"),
        programme("News", 200, 250, "later equal-start source row"),
        programme("News", 900, 1100, "not completed"), programme("Old", 100, 200, "cutoff excluded"),
        programme("End", 800, 1000, "end included")]);
    assert_eq!(indices(&value).unwrap(), vec![4, 1]);
    let selected = super::super::guide::select(&value.programmes, "news", value.query, 0).unwrap();
    assert_eq!(selected[1].description, "later equal-start source row");
    assert_eq!(indices(&result(selected)).unwrap(), vec![2, 1]);
}

#[test]
fn records_shift_and_empty_do_not_borrow_current_or_other_channel() {
    let rows = vec![programme("Shifted", 90, 150, ""),
        Programme { channel: "other".into(), ..programme("Other", 400, 500, "") }];
    let query = Query::Records { start: 100, end: 1000 };
    let shifted = super::super::guide::select(&rows, "news", query, 20).unwrap();
    assert_eq!(shifted.len(), 1);
    assert_eq!(shifted[0].start, 110);
    assert!(super::super::guide::select(&rows, "news", query, 0).unwrap().is_empty());
    assert!(indices(&result(Vec::new())).unwrap().is_empty());
    let mut wrong = result(rows);
    wrong.query = Query::CurrentNext { now: 300 };
    assert!(indices(&wrong).is_err());
    wrong.query = query;
    wrong.coverage = Coverage::ProviderShortGuide;
    assert!(indices(&wrong).is_err());
}
