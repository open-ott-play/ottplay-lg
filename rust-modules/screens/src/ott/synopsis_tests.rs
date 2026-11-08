use super::*;
use plx_data::ott::SynopsisSpan;

fn row(full: &str, start: usize) -> OttRow {
    OttRow {
        current_programme: None,
        content_title: Some("Displayed title".into()),
        guide: None,
        guide_available: false,
        records_available: false,
        media: Some(OttMedia {
            kind: OttMediaKind::Video,
            duration_ms: None,
            progress: None,
            synopsis: SynopsisSpan::new(full, start).and_then(|span| span.validated_for(full)),
        }),
        id: "movie".into(),
        title: "Displayed title".into(),
        detail: String::new(),
        description: full.into(),
        poster: None,
        action: OttAction::Play("movie".into()),
        enabled: true,
        favorite: false,
    }
}

fn painted(row: &OttRow, classic_mode: bool) -> Vec<String> {
    let measure = &plx_ui::fixture::FixtureMeasure;
    plx_ui::draw_census::capture_runs(|| {
        let p = plx_ui::Painter::recording();
        if classic_mode {
            classic::paint_left(
                p,
                plx_ui::classic::Layout::new(Rect::FULL, measure, false),
                row,
                "",
                false,
                None,
                Default::default(),
                measure,
            );
        } else {
            metadata::paint(
                p,
                measure,
                row,
                "",
                false,
                false,
                false,
                RouteLayout::screen(),
                None,
                None,
            );
        }
    })
    .into_iter()
    .filter(|run| run.ink[3] > 0.0)
    .map(|run| run.text)
    .collect()
}

#[test]
fn both_metadata_previews_begin_with_source_synopsis_and_full_reader_retains_facts_and_tail() {
    let _serial = plx_base::testlock::serial();
    let prefix = "GENERATED_YEAR\nGENERATED_DURATION\n";
    let synopsis = format!(
        "SYNOPSIS_START. {}FULL_DESCRIPTION_TAIL",
        "Полный текст. ".repeat(500)
    );
    let full = format!("{prefix}{synopsis}");
    let row = row(&full, prefix.len());
    assert_eq!(metadata::preview_description(&row, None), synopsis);
    for classic in [false, true] {
        let text = painted(&row, classic);
        assert!(
            text.iter().any(|run| run.contains("SYNOPSIS_START")),
            "mode {classic}: synopsis starts in the visible preview"
        );
        assert!(!text
            .iter()
            .any(|run| run.contains("GENERATED_YEAR") || run.contains("GENERATED_DURATION")));
        let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
        screen.install(Arc::new(OttView {
            rows: vec![row.clone()],
            page_id: "channels".into(),
            selected_row: Some(row.id.clone()),
            ..Default::default()
        }));
        assert!(screen.has_description());
        assert_eq!(
            screen.description(&screen.snapshot.rows[0]),
            full,
            "the reader receives the complete canonical document, not the fitted preview"
        );
        assert!(screen
            .description(&screen.snapshot.rows[0])
            .ends_with("FULL_DESCRIPTION_TAIL"));
    }
}

#[test]
fn facts_only_keep_full_description_available_and_unannotated_prose_is_never_trimmed() {
    let _serial = plx_base::testlock::serial();
    let full = "2026\n90 min";
    let facts = row(full, full.len());
    assert_eq!(metadata::preview_description(&facts, None), "");
    assert_eq!(metadata::description(&facts, None), full);
    let mut screen = OttScreen::new(EntryId(1), InstanceId(1));
    screen.install(Arc::new(OttView {
        rows: vec![facts.clone()],
        page_id: "channels".into(),
        ..Default::default()
    }));
    assert!(
        screen.has_description(),
        "Description remains available even when the source has no synopsis"
    );
    assert!(painted(&facts, false)
        .iter()
        .any(|run| run.contains(plx_platform::i18n::msg::ott_description_hint())));
    let mut unannotated = row("Displayed title\n2026\n90 min\nThis is provider prose.", 0);
    unannotated.media.as_mut().unwrap().synopsis = None;
    assert_eq!(
        metadata::preview_description(&unannotated, None),
        unannotated.description
    );
    let source = SynopsisSpan::new(&unannotated.description, "Displayed title\n".len()).unwrap();
    unannotated.media.as_mut().unwrap().synopsis = Some(source);
    assert_eq!(
        metadata::preview_description(&unannotated, None),
        unannotated.description,
        "an unvalidated deserialized annotation cannot suppress prose"
    );
}

#[test]
fn guide_rollover_ignores_catalogue_synopsis_annotation() {
    let mut row = row("2026\nCatalogue synopsis", 5);
    row.guide = Some(OttNowNext {
        now_s: 10,
        current: Some(OttGuideProgramme {
            title: "Current".into(),
            description: "Current synopsis".into(),
            start_s: 0,
            end_s: 20,
        }),
        next: Some(OttGuideProgramme {
            title: "Next".into(),
            description: "Next synopsis".into(),
            start_s: 20,
            end_s: 40,
        }),
    });
    for (now, expected) in [(10, "Current synopsis"), (20, "Next synopsis"), (40, "")] {
        assert_eq!(metadata::preview_description(&row, Some(now)), expected);
        assert_eq!(metadata::description(&row, Some(now)), expected);
    }
}
