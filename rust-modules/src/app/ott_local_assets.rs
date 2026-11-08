//! Pure adjacent-subtitle selection over an already bounded directory observation.
//!
//! No path here grants access or proves current file identity. The caller retains
//! the existing LocalSelection and playback ticket around this result, then opens
//! the selected file through its authorized read-only root and rechecks admission.
//! Directory enumeration, body reads, decoding and publication belong to that caller.
use plx_data::ott::{is_local_video_path, valid_local_relative_path};
use std::collections::HashSet;

pub(super) struct SelectInput<'a> {
    pub media_relative_path: &'a str,
    pub directory: DirectoryObservation<'a>,
    /// Remaining slots in the caller's shared combined track inventory.
    /// This module neither duplicates the track cap nor reserves embedded slots.
    pub remaining_subtitle_capacity: usize,
}

pub(super) struct DirectoryObservation<'a> {
    /// Empty only for the selected root; otherwise exact root-relative spelling.
    pub parent_relative_path: &'a str,
    /// The caller bounds both entry count and aggregate name bytes before calling.
    pub entries: &'a [ObservedEntry<'a>],
    /// True only if the caller reached the end of this directory without omission.
    pub complete: bool,
}

#[derive(Clone, Copy)]
pub(super) struct ObservedEntry<'a> {
    pub relative_path: &'a str,
    pub kind: ObservedKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ObservedKind {
    RegularFile,
    Directory,
    Symlink,
    Other,
    Unreadable,
}

pub(super) struct AssetCandidates<'a> {
    pub subtitles: Vec<SubtitleCandidate<'a>>,
    /// False if the observation was partial or matching files exceeded capacity.
    /// An incomplete empty answer is not proof that no adjacent subtitles exist.
    pub complete: bool,
}

#[derive(Clone, Copy)]
pub(super) struct SubtitleCandidate<'a> {
    pub relative_path: &'a str,
    pub format: SubtitleFormat,
    /// Filename syntax only, not a normalized/verified language or display label.
    /// The shared UI/wire projection must validate its own metadata byte budget.
    pub language_suffix: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SubtitleFormat {
    Srt,
    Vtt,
    Ass,
    Ssa,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SelectionError {
    InvalidMedia,
    InvalidObservation,
}

pub(super) fn select<'a>(input: SelectInput<'a>) -> Result<AssetCandidates<'a>, SelectionError> {
    if !valid_local_relative_path(input.media_relative_path)
        || !is_local_video_path(input.media_relative_path)
    {
        return Err(SelectionError::InvalidMedia);
    }
    let (parent, media_name) = split_parent(input.media_relative_path);
    let Some((stem, _)) = media_name
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
    else {
        return Err(SelectionError::InvalidMedia);
    };
    if input.directory.parent_relative_path != parent {
        return Err(SelectionError::InvalidObservation);
    }

    let mut seen = HashSet::new();
    let mut subtitles: Vec<SubtitleCandidate<'_>> = Vec::new();
    let mut complete = input.directory.complete;
    for entry in input.directory.entries {
        if !valid_local_relative_path(entry.relative_path)
            || split_parent(entry.relative_path).0 != parent
            || !seen.insert(entry.relative_path)
        {
            return Err(SelectionError::InvalidObservation);
        }
        let Some(candidate) = subtitle_candidate(entry.relative_path, stem) else {
            continue;
        };
        if entry.kind != ObservedKind::RegularFile {
            // A matching name whose type could not be read is not evidence of
            // absence. Unrelated unreadable names cannot contribute a candidate.
            if entry.kind == ObservedKind::Unreadable {
                complete = false;
            }
            continue;
        }
        // Retain only the lexically first available slots, independently of read_dir
        // order. Full capacity drops a last entry before insertion: no N+1 buffer.
        let position = subtitles
            .binary_search_by(|held| held.relative_path.cmp(candidate.relative_path))
            .unwrap_or_else(|position| position);
        if subtitles.len() == input.remaining_subtitle_capacity {
            complete = false;
            if position == subtitles.len() {
                continue;
            }
            subtitles.pop();
        }
        subtitles.insert(position, candidate);
    }
    Ok(AssetCandidates {
        subtitles,
        complete,
    })
}

fn split_parent(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

fn subtitle_candidate<'a>(path: &'a str, media_stem: &str) -> Option<SubtitleCandidate<'a>> {
    let (stem, extension) = split_parent(path).1.rsplit_once('.')?;
    let format = if extension.eq_ignore_ascii_case("srt") {
        SubtitleFormat::Srt
    } else if extension.eq_ignore_ascii_case("vtt") {
        SubtitleFormat::Vtt
    } else if extension.eq_ignore_ascii_case("ass") {
        SubtitleFormat::Ass
    } else if extension.eq_ignore_ascii_case("ssa") {
        SubtitleFormat::Ssa
    } else {
        return None;
    };
    let language_suffix = if stem == media_stem {
        None
    } else {
        let suffix = stem.strip_prefix(media_stem)?.strip_prefix('.')?;
        if !language_token(suffix) {
            return None;
        }
        Some(suffix)
    };
    Some(SubtitleCandidate {
        relative_path: path,
        format,
        language_suffix,
    })
}

// Conservative filename syntax, not the Classic display-title parser: that parser
// strips words and folds labels, neither of which establishes local file identity.
// One language token means a 2–3-letter primary and optional hyphen-separated
// ASCII alphanumeric subtags of 1–8 characters. No forced/SDH/CC interpretation.
fn language_token(value: &str) -> bool {
    let mut parts = value.split('-');
    let primary = parts.next().unwrap_or("");
    if !(2..=3).contains(&primary.len()) || !primary.bytes().all(|b| b.is_ascii_alphabetic()) {
        return false;
    }
    let is_marker = |part: &str| {
        ["forced", "sdh", "cc"]
            .iter()
            .any(|marker| part.eq_ignore_ascii_case(marker))
    };
    !is_marker(primary)
        && parts.all(|part| {
            (1..=8).contains(&part.len())
                && part.bytes().all(|b| b.is_ascii_alphanumeric())
                && !is_marker(part)
        })
}

#[cfg(test)]
#[path = "ott_local_assets_tests.rs"]
mod tests;
