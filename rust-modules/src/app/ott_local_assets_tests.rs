use super::*;

fn observed<'a>(names: &[&'a str]) -> Vec<ObservedEntry<'a>> {
    names
        .iter()
        .map(|&relative_path| ObservedEntry {
            relative_path,
            kind: ObservedKind::RegularFile,
        })
        .collect()
}

fn input<'a>(media: &'a str, entries: &'a [ObservedEntry<'a>]) -> SelectInput<'a> {
    SelectInput {
        media_relative_path: media,
        directory: DirectoryObservation {
            parent_relative_path: media.rsplit_once('/').map_or("", |(parent, _)| parent),
            entries,
            complete: true,
        },
        remaining_subtitle_capacity: 8,
    }
}

fn paths<'a>(result: &AssetCandidates<'a>) -> Vec<&'a str> {
    result
        .subtitles
        .iter()
        .map(|candidate| candidate.relative_path)
        .collect()
}

fn expect_error(result: Result<AssetCandidates<'_>, SelectionError>, expected: SelectionError) {
    match result {
        Err(error) => assert_eq!(error, expected),
        Ok(_) => panic!("invalid input was accepted"),
    }
}

#[test]
fn exact_stem_accepts_only_the_existing_four_decoders() {
    let rows = observed(&[
        "Movie.srt",
        "Movie.VTT",
        "Movie.ASS",
        "Movie.ssa",
        "Movie.smi",
        "Movie.sami",
        "Movie.sub",
        "Movie.idx",
        "Movie.sup",
        "Movie.txt",
        "Movie.jpg",
        "Movie2.srt",
        "movie.srt",
        "Movie trailer.srt",
    ]);
    let result = select(input("Movie.mkv", &rows)).unwrap();
    assert_eq!(
        paths(&result),
        ["Movie.ASS", "Movie.VTT", "Movie.srt", "Movie.ssa"]
    );
    assert_eq!(
        result
            .subtitles
            .iter()
            .map(|candidate| candidate.format)
            .collect::<Vec<_>>(),
        [
            SubtitleFormat::Ass,
            SubtitleFormat::Vtt,
            SubtitleFormat::Srt,
            SubtitleFormat::Ssa
        ]
    );
    assert!(result
        .subtitles
        .iter()
        .all(|candidate| candidate.language_suffix.is_none()));
    assert!(result.complete);
}

#[test]
fn a_single_language_token_preserves_its_original_spelling() {
    let rows = observed(&[
        "Show.episode.eng.srt",
        "Show.episode.en.SRT",
        "Show.episode.pt-BR.vtt",
        "Show.episode.zh-Hant.ass",
        "Show.episode.EN-gb.ssa",
    ]);
    let result = select(input("Show.episode.MKV", &rows)).unwrap();
    assert_eq!(
        result
            .subtitles
            .iter()
            .map(|candidate| candidate.language_suffix)
            .collect::<Vec<_>>(),
        [
            Some("EN-gb"),
            Some("en"),
            Some("eng"),
            Some("pt-BR"),
            Some("zh-Hant")
        ]
    );
    assert!(result.complete);
}

#[test]
fn forced_sdh_and_fuzzy_suffixes_are_not_claimed_as_language_tracks() {
    let rows = observed(&[
        "Movie.forced.srt",
        "Movie.en.forced.srt",
        "Movie.SDH.srt",
        "Movie.en.SDH.srt",
        "Movie.cc.srt",
        "Movie.en-cc.srt",
        "Movie.en-sdh.srt",
        "Movie.en-forced.srt",
        "Movie.english.srt",
        "Movie.e.srt",
        "Movie.en_.srt",
        "Movie.en-.srt",
        "Movie.en--GB.srt",
        "Movie.en-123456789.srt",
        "Movie.рус.srt",
        "Movie..srt",
        "Movie.en.commentary.srt",
        "Movie en.srt",
    ]);
    let result = select(input("Movie.mkv", &rows)).unwrap();
    assert!(result.subtitles.is_empty());
    assert!(result.complete);
}

#[test]
fn deep_unicode_paths_match_exactly_without_case_or_normalization_merging() {
    let rows = observed(&[
        "家族/Фильмы/épisode.srt",
        "家族/Фильмы/e\u{301}pisode.srt",
        "家族/Фильмы/Épisode.srt",
        "家族/Фильмы/épisode.en.srt",
    ]);
    let result = select(input("家族/Фильмы/épisode.mkv", &rows)).unwrap();
    assert_eq!(
        paths(&result),
        ["家族/Фильмы/épisode.en.srt", "家族/Фильмы/épisode.srt"]
    );
    let decomposed = select(input("家族/Фильмы/e\u{301}pisode.mkv", &rows)).unwrap();
    assert_eq!(paths(&decomposed), ["家族/Фильмы/e\u{301}pisode.srt"]);
}

#[test]
fn only_the_final_media_extension_is_removed_and_dotfiles_keep_their_stem() {
    let rows = observed(&[".film.part.en.srt", ".film.srt", "film.part.en.srt"]);
    let result = select(input(".film.part.mkv", &rows)).unwrap();
    assert_eq!(paths(&result), [".film.part.en.srt"]);
    expect_error(select(input(".mkv", &rows)), SelectionError::InvalidMedia);
}

#[test]
fn invalid_media_paths_use_the_shared_path_and_media_validators() {
    for media in [
        "",
        "/Movie.mkv",
        "../Movie.mkv",
        "a/../Movie.mkv",
        "a//Movie.mkv",
        "./Movie.mkv",
        "C:/Movie.mkv",
        "a\\Movie.mkv",
        "Movie\0.mkv",
        "Movie\n.mkv",
        "Movie",
        "Movie.jpg",
        "https://example.invalid/Movie.mkv",
    ] {
        expect_error(select(input(media, &[])), SelectionError::InvalidMedia);
    }
    let oversized = format!("{}.mkv", "a".repeat(8192));
    expect_error(select(input(&oversized, &[])), SelectionError::InvalidMedia);
}

#[test]
fn parent_snapshot_and_every_entry_must_belong_to_the_same_immediate_directory() {
    let rows = observed(&["Folder/Movie.srt"]);
    let mut request = input("Folder/Movie.mkv", &rows);
    request.directory.parent_relative_path = "Other";
    expect_error(select(request), SelectionError::InvalidObservation);
    for path in [
        "Movie.srt",
        "Other/Movie.srt",
        "Folder/nested/Movie.srt",
        "Folder",
    ] {
        let rows = observed(&[path]);
        expect_error(
            select(input("Folder/Movie.mkv", &rows)),
            SelectionError::InvalidObservation,
        );
    }
}

#[test]
fn invalid_observations_are_not_silently_ignored_even_after_capacity_is_full() {
    for path in [
        "Folder/../outside.srt",
        "Folder//Movie.srt",
        "Folder/./Movie.srt",
        "Folder/Movie\0.srt",
        "Folder/Movie\n.srt",
        "/Folder/Movie.srt",
        "Folder/Movie\\en.srt",
    ] {
        let rows = observed(&["Folder/Movie.srt", path]);
        let mut request = input("Folder/Movie.mkv", &rows);
        request.remaining_subtitle_capacity = 1;
        expect_error(select(request), SelectionError::InvalidObservation);
    }
}

#[test]
fn duplicate_exact_observations_are_invalid_even_when_they_do_not_match() {
    let rows = observed(&["Movie.srt", "unrelated.txt", "unrelated.txt"]);
    expect_error(
        select(input("Movie.mkv", &rows)),
        SelectionError::InvalidObservation,
    );
    let rows = [
        ObservedEntry {
            relative_path: "Movie.srt",
            kind: ObservedKind::RegularFile,
        },
        ObservedEntry {
            relative_path: "Movie.srt",
            kind: ObservedKind::Symlink,
        },
    ];
    expect_error(
        select(input("Movie.mkv", &rows)),
        SelectionError::InvalidObservation,
    );
}

#[test]
fn nonregular_matching_entries_never_become_read_candidates() {
    for kind in [
        ObservedKind::Directory,
        ObservedKind::Symlink,
        ObservedKind::Other,
        ObservedKind::Unreadable,
    ] {
        let rows = [ObservedEntry {
            relative_path: "Movie.srt",
            kind,
        }];
        let result = select(input("Movie.mkv", &rows)).unwrap();
        assert!(result.subtitles.is_empty());
    }
}

#[test]
fn a_matching_unreadable_entry_cannot_establish_complete_absence() {
    let matching = [ObservedEntry {
        relative_path: "Movie.srt",
        kind: ObservedKind::Unreadable,
    }];
    let result = select(input("Movie.mkv", &matching)).unwrap();
    assert!(result.subtitles.is_empty());
    assert!(!result.complete);
    for name in ["Other.srt", "Movie.jpg", "Movie.smi", "Movie.en.forced.srt"] {
        let unrelated = [ObservedEntry {
            relative_path: name,
            kind: ObservedKind::Unreadable,
        }];
        let result = select(input("Movie.mkv", &unrelated)).unwrap();
        assert!(result.subtitles.is_empty());
        assert!(result.complete);
    }
}

#[test]
fn capacity_keeps_a_deterministic_prefix_and_reports_overflow() {
    let names = [
        "Movie.fr.srt",
        "Movie.en.srt",
        "Movie.de.srt",
        "Movie.it.srt",
    ];
    for order in [
        names,
        [names[3], names[2], names[1], names[0]],
        [names[1], names[3], names[0], names[2]],
    ] {
        let rows = observed(&order);
        let mut request = input("Movie.mkv", &rows);
        request.remaining_subtitle_capacity = 2;
        let result = select(request).unwrap();
        assert_eq!(paths(&result), ["Movie.de.srt", "Movie.en.srt"]);
        assert!(!result.complete);
    }
    let rows = observed(&["Movie.en.srt", "Movie.de.srt", "Other.fr.srt"]);
    let mut request = input("Movie.mkv", &rows);
    request.remaining_subtitle_capacity = 2;
    let result = select(request).unwrap();
    assert_eq!(paths(&result), ["Movie.de.srt", "Movie.en.srt"]);
    assert!(result.complete, "unrelated files consume no subtitle slot");
}

#[test]
fn zero_capacity_distinguishes_no_matches_from_unrepresented_matches() {
    for (names, complete) in [
        (&["Movie.srt"][..], false),
        (&["Other.srt"][..], true),
        (&[][..], true),
    ] {
        let rows = observed(names);
        let mut request = input("Movie.mkv", &rows);
        request.remaining_subtitle_capacity = 0;
        let result = select(request).unwrap();
        assert!(result.subtitles.is_empty());
        assert_eq!(result.complete, complete);
    }
}

#[test]
fn input_partialness_is_preserved_without_inventing_a_complete_empty_answer() {
    for names in [&[][..], &["Other.srt"][..], &["Movie.srt"][..]] {
        let rows = observed(names);
        let mut request = input("Movie.mkv", &rows);
        request.directory.complete = false;
        assert!(!select(request).unwrap().complete);
    }
}

#[test]
fn an_oversized_caller_capacity_does_not_preallocate_that_capacity() {
    let rows = observed(&["Movie.srt"]);
    let mut request = input("Movie.mkv", &rows);
    request.remaining_subtitle_capacity = usize::MAX;
    let result = select(request).unwrap();
    assert_eq!(paths(&result), ["Movie.srt"]);
    assert!(result.complete);
}

#[test]
fn percent_sequences_are_literal_path_characters_and_never_decoded() {
    let rows = observed(&["%2e%2e/Movie.srt", "%2e%2e/Movie.en.srt"]);
    let request = input("%2e%2e/Movie.mkv", &rows);
    let result = select(request).unwrap();
    assert_eq!(paths(&result), ["%2e%2e/Movie.en.srt", "%2e%2e/Movie.srt"]);
}

#[test]
fn long_syntactic_language_tokens_remain_private_and_are_not_silently_truncated() {
    let suffix = format!("en{}", "-abcdefgh".repeat(5));
    let filename = format!("Movie.{suffix}.srt");
    let rows = observed(&[&filename]);
    let result = select(input("Movie.mkv", &rows)).unwrap();
    assert_eq!(result.subtitles[0].relative_path, filename);
    assert_eq!(result.subtitles[0].language_suffix, Some(suffix.as_str()));
    assert!(
        suffix.len() > 32,
        "the shared wire projection must separately reject this metadata"
    );
}
