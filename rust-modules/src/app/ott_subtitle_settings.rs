//! Typed native subtitle preferences. Storage and publication remain owned by Runtime.
use super::{App, Runtime, Saved};
use plx_appkit::track_menu::{TrackCommit, TrackCommitFailure, TrackCommitOutcome, TrackCommitSource};
use plx_media::player::text_style::TextAppearance;
use plx_media::route::NativeSubtitlePreferences;
use plx_plex::plex::session::{SubtitlePosition, SubtitleSize, SubtitleTone};
use plx_screens::registry::{PlayerReq, TrackCommitRequest};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

const APPEARANCE: &str = "nativeSubtitleAppearance";
const SIZE: &str = "nativeSubtitleSize";
const POSITION: &str = "nativeSubtitlePosition";
const TONE: &str = "nativeSubtitleTone";
pub(super) const REVISION: &str = "nativeSubtitleRevision";

fn revision(saved: &Saved) -> Result<u64, &'static str> {
    match saved.settings.get(REVISION) {
        None => Ok(0),
        Some(value) => value.as_u64().ok_or("Invalid native subtitle revision"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Edit {
    Appearance(TextAppearance),
    Size(SubtitleSize),
    Position(SubtitlePosition),
    Tone(SubtitleTone),
}

impl Edit {
    /// Plex geometry retains its profile write path. Appearance belongs to the installation.
    pub(super) fn from_commit(commit: &TrackCommit, external: bool) -> Option<Self> {
        match commit {
            TrackCommit::TextAppearance(value) => Some(Self::Appearance(*value)),
            TrackCommit::SubtitleSize(value) if external => Some(Self::Size(*value)),
            TrackCommit::SubtitlePosition(value) if external => Some(Self::Position(*value)),
            TrackCommit::SubtitleTone(value) if external => Some(Self::Tone(*value)),
            _ => None,
        }
    }

    fn entry(self) -> (&'static str, Value) {
        match self {
            Self::Appearance(value) => (APPEARANCE, json!(value)),
            Self::Size(value) => (SIZE, json!(value)),
            Self::Position(value) => (POSITION, json!(value)),
            Self::Tone(value) => (TONE, json!(value)),
        }
    }

    /// Construct against the current owner's snapshot immediately before Runtime::save.
    /// This does not publish live values or create a second persistence mechanism.
    pub(super) fn candidate(self, saved: &Saved) -> Result<Saved, &'static str> {
        let previous = revision(saved)?;
        let (key, value) = self.entry();
        if saved.settings.get(key) == Some(&value) {
            return Ok(saved.clone());
        }
        let next = previous.checked_add(1).ok_or("Native subtitle revision exhausted")?;
        let mut candidate = saved.clone();
        candidate.settings.insert(key.into(), value);
        candidate.settings.insert(REVISION.into(), json!(next));
        Ok(candidate)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Stored {
    pub appearance: TextAppearance,
    size: Option<SubtitleSize>,
    position: Option<SubtitlePosition>,
    tone: Option<SubtitleTone>,
}

fn optional<T: DeserializeOwned>(saved: &Saved, key: &str) -> Result<Option<T>, &'static str> {
    saved.settings.get(key)
        .map(|value| serde_json::from_value(value.clone())
            .map_err(|_| "Invalid stored subtitle settings"))
        .transpose()
}

impl Stored {
    pub(super) fn read(saved: &Saved) -> Result<Self, &'static str> {
        revision(saved)?;
        Ok(Self {
            appearance: optional(saved, APPEARANCE)?.unwrap_or_default(),
            size: optional(saved, SIZE)?,
            position: optional(saved, POSITION)?,
            tone: optional(saved, TONE)?,
        })
    }

    /// Missing geometry keys preserve the effective value chosen for this playback.
    pub(super) fn effective(self, fallback: NativeSubtitlePreferences) -> NativeSubtitlePreferences {
        NativeSubtitlePreferences {
            size: self.size.unwrap_or(fallback.size),
            position: self.position.unwrap_or(fallback.position),
            tone: self.tone.unwrap_or(fallback.tone),
        }
    }

    /// The application owner calls this only after a verified load/save, or while
    /// preparing a newly admitted source from the already verified Saved record.
    pub(super) fn apply(self, app: &mut super::App, initialize: bool) {
        let ps = &mut app.player.session;
        plx_media::route::restore_subtitle_text_appearance(ps, self.appearance);
        if let Some(current) = plx_media::route::native_subtitle_preferences(ps) {
            let fallback = if initialize {
                NativeSubtitlePreferences {
                    size: plx_media::route::subtitle_size(),
                    position: plx_media::route::subtitle_position(),
                    tone: plx_media::player::subtitle_tone(),
                }
            } else {
                current
            };
            plx_media::route::restore_native_subtitle_preferences(ps, self.effective(fallback));
        }
        app.bridge.sync_subtitle_preferences(ps);
    }
}

/// The existing Runtime save slot owns this tag until its terminal result.
/// Durable preferences survive a retired menu; its reply and live publication do not.
pub(super) struct PendingSave {
    source: TrackCommitSource,
    request: Option<TrackCommitRequest>,
}

impl PendingSave {
    pub(super) fn can_publish(&self, app: &App) -> bool {
        // Admission needs an active, stable playback. A verified completion may
        // still publish into that same session while it is paused or suspended;
        // changing presentation state must not strand a durable preference.
        self.source.matches(&app.player.session)
    }

    pub(super) fn finish(self, app: &mut App, outcome: TrackCommitOutcome) {
        if let Some(request) = self.request {
            super::bridge::finish_track_commit(&mut app.pages, &request, outcome);
        }
    }
}

fn playback_gate(has_url: bool, state: plx_media::player::PlaybackState) -> Result<(), TrackCommitFailure> {
    use plx_media::player::PlaybackState;
    if !has_url || matches!(state, PlaybackState::Idle | PlaybackState::Error) {
        Err(TrackCommitFailure::Unavailable)
    } else if matches!(state, PlaybackState::Resolving | PlaybackState::Connecting) {
        Err(TrackCommitFailure::Busy)
    } else {
        Ok(())
    }
}

fn current_playback(app: &App) -> Result<(), TrackCommitFailure> {
    if !super::playback_allowed(app) {
        return Err(TrackCommitFailure::Unavailable);
    }
    playback_gate(plx_media::route::has_url(&app.player.session),
        plx_media::player::state(&app.player.session))
}

fn save_refusal(runtime: &Runtime) -> Option<TrackCommitFailure> {
    if runtime.persistence_uncertain || runtime.saved.receipts.iter().any(|receipt|
        receipt.outcome == plx_data::ott::favorites_wire::Outcome::Unknown) {
        Some(TrackCommitFailure::SaveUncertain)
    } else if !runtime.loaded || runtime.saving {
        Some(TrackCommitFailure::Busy)
    } else {
        None
    }
}

fn perform(app: &mut App, commit: TrackCommit, request: Option<TrackCommitRequest>)
    -> Option<TrackCommitOutcome>
{
    let external = plx_media::route::native_subtitle_preferences(&app.player.session).is_some();
    let Some(edit) = Edit::from_commit(&commit, external) else {
        return Some(super::playback::commit_track_outcome(&mut app.player.session, commit));
    };
    if let Err(why) = current_playback(app) {
        return Some(TrackCommitOutcome::Rejected(why));
    }
    let source = TrackCommitSource::capture(&app.player.session);
    let Some(runtime) = app.ott.as_mut() else {
        return Some(TrackCommitOutcome::Rejected(TrackCommitFailure::Unavailable));
    };
    if let Some(why) = save_refusal(runtime) {
        return Some(TrackCommitOutcome::Rejected(why));
    }
    let (key, value) = edit.entry();
    if runtime.saved.settings.get(key) == Some(&value) {
        // The durable value may have landed while another source was active.
        // This new, admitted request can publish it without another write.
        match Stored::read(&runtime.saved) {
            Ok(stored) => stored.apply(app, false),
            Err(_) => return Some(TrackCommitOutcome::Rejected(TrackCommitFailure::Unavailable)),
        }
        return Some(TrackCommitOutcome::Unchanged);
    }
    let candidate = match edit.candidate(&runtime.saved) {
        Ok(candidate) => candidate,
        Err(_) => return Some(TrackCommitOutcome::Rejected(TrackCommitFailure::SaveFailed)),
    };
    if runtime.save(candidate, None, false).is_err() {
        return Some(TrackCommitOutcome::Rejected(TrackCommitFailure::SaveFailed));
    }
    runtime.subtitle_save = Some(PendingSave { source, request });
    None
}

pub(super) fn trusted_commit(app: &mut App, commit: TrackCommit) {
    if let Some(TrackCommitOutcome::Rejected(_)) = perform(app, commit, None) {
        plx_base::eventlog::log("track setting: request was not applied");
    }
}

fn addressed_commit(app: &mut App, request: TrackCommitRequest) {
    use super::bridge::TrackCommitAdmission;
    let outcome = match super::bridge::begin_track_commit(
        &mut app.pages, &app.player.session, &request,
    ) {
        TrackCommitAdmission::Ignore => return,
        TrackCommitAdmission::Reject(why) => Some(TrackCommitOutcome::Rejected(why)),
        TrackCommitAdmission::Ready => match current_playback(app) {
            Err(why) => Some(TrackCommitOutcome::Rejected(why)),
            Ok(()) => perform(app, request.commit.clone(), Some(request.clone())),
        },
    };
    if let Some(outcome) = outcome {
        super::bridge::finish_track_commit(&mut app.pages, &request, outcome);
    }
}

/// Called in the original request order, before the legacy player performer.
pub(super) fn player_request(app: &mut App, request: PlayerReq) -> Option<PlayerReq> {
    match request {
        PlayerReq::CommitTrackAddressed(request) => { addressed_commit(app, request); None }
        PlayerReq::CommitTrack(commit) => { trusted_commit(app, commit); None }
        request @ PlayerReq::OpenOverlay(_) => {
            // A previous source's completed save may intentionally have deferred
            // live publication. Opening a fresh menu reads the canonical record.
            let stored = app.ott.as_ref().filter(|r| r.loaded && !r.persistence_uncertain)
                .and_then(|r| Stored::read(&r.saved).ok());
            if let Some(stored) = stored { stored.apply(app, false); }
            Some(request)
        }
        request => Some(request),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plx_media::player::text_style::{TextBackground, TextColor, TextFont, TextOutline};

    fn geometry() -> NativeSubtitlePreferences {
        NativeSubtitlePreferences {
            size: SubtitleSize::Large,
            position: SubtitlePosition::High,
            tone: SubtitleTone::Silver,
        }
    }

    #[test]
    fn playback_gate_allows_pause_buffering_and_seek_but_not_a_new_or_failed_start() {
        use plx_media::player::PlaybackState::*;
        for state in [Buffering, Seeking, Playing, Paused] {
            assert_eq!(playback_gate(true, state), Ok(()));
            assert_eq!(playback_gate(false, state), Err(TrackCommitFailure::Unavailable));
        }
        for state in [Idle, Error] {
            assert_eq!(playback_gate(true, state), Err(TrackCommitFailure::Unavailable));
        }
        for state in [Resolving, Connecting] {
            assert_eq!(playback_gate(true, state), Err(TrackCommitFailure::Busy));
        }
    }

    #[test]
    fn missing_preferences_preserve_geometry_and_legacy_appearance() {
        let stored = Stored::read(&Saved::default()).unwrap();
        assert_eq!(stored.effective(geometry()), geometry());
        assert_eq!(stored.appearance, TextAppearance::default());
    }

    #[test]
    fn every_geometry_rung_round_trips_independently() {
        for size in SubtitleSize::LADDER {
            for position in SubtitlePosition::LADDER {
                for tone in SubtitleTone::LADDER {
                    let sized = Edit::Size(size).candidate(&Saved::default()).unwrap();
                    let positioned = Edit::Position(position).candidate(&sized).unwrap();
                    let saved = Edit::Tone(tone).candidate(&positioned).unwrap();
                    assert_eq!(Stored::read(&saved).unwrap().effective(geometry()),
                        NativeSubtitlePreferences { size, position, tone });
                }
            }
        }
    }

    #[test]
    fn one_edit_retains_other_settings_and_does_not_publish_into_saved_state() {
        let mut saved = Saved::default();
        saved.category = "Favorites".into();
        saved.favorites_selected = true;
        saved.settings.insert("unrelated".into(), json!({"value": 7}));
        let candidate = Edit::Size(SubtitleSize::Small).candidate(&saved).unwrap();
        assert_eq!(candidate.category, saved.category);
        assert_eq!(candidate.favorites_selected, saved.favorites_selected);
        assert_eq!(candidate.settings["unrelated"], saved.settings["unrelated"]);
        assert!(!saved.settings.contains_key(SIZE));
        assert!(!candidate.settings.contains_key(POSITION));
        assert!(!candidate.settings.contains_key(TONE));
        assert_eq!(Stored::read(&candidate).unwrap().effective(geometry()),
            NativeSubtitlePreferences { size: SubtitleSize::Small, ..geometry() });
    }

    #[test]
    fn native_geometry_and_global_appearance_have_distinct_write_owners() {
        for commit in [TrackCommit::SubtitleSize(SubtitleSize::Huge),
            TrackCommit::SubtitlePosition(SubtitlePosition::Middle),
            TrackCommit::SubtitleTone(SubtitleTone::Grey)] {
            assert!(Edit::from_commit(&commit, true).is_some());
            assert!(Edit::from_commit(&commit, false).is_none());
        }
        let appearance = TextAppearance {
            font: TextFont::InterRegular,
            color: TextColor::Yellow,
            outline: TextOutline::ExtraThick,
            background: TextBackground::Half,
        };
        let edit = Edit::Appearance(appearance);
        let commit = TrackCommit::TextAppearance(appearance);
        assert_eq!(Edit::from_commit(&commit, true), Some(edit));
        assert_eq!(Edit::from_commit(&commit, false), Some(edit));
        assert_eq!(Stored::read(&edit.candidate(&Saved::default()).unwrap()).unwrap().appearance, appearance);
        assert!(Edit::from_commit(&TrackCommit::SubtitleOffset(100), true).is_none());
    }

    #[test]
    fn invalid_stored_values_are_not_silently_replaced_with_defaults() {
        for key in [SIZE, POSITION, TONE, APPEARANCE] {
            for value in [Value::Null, json!(123), json!("unknown"), json!({"unknown": true})] {
                let mut saved = Saved::default();
                saved.settings.insert(key.into(), value);
                assert!(Stored::read(&saved).is_err(), "{key}");
            }
        }
    }
}

#[cfg(test)]
#[path = "ott_subtitle_revision_tests.rs"]
mod revision_tests;
