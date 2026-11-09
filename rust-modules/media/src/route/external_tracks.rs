//! Track choices for one non-Plex playback. No IO or player mutation happens here.
//!
//! A catalog has an opaque identity separate from its media address: reopening the same
//! file starts a new identity. Cloning it for an in-session reload preserves that identity.
//! A menu choice therefore cannot select a track in a later playback, even when its
//! source, labels and ordinals happen to be identical.
use crate::ff::inspect::{InspectedTrack, TrackInventory, TrackKind};
use std::collections::HashSet;
use std::sync::Arc;
pub use crate::player::sidecar::SidecarFormat;

/// Opaque identity for one app-authorized adjacent asset. No path, root, grant or
/// demuxer ordinal crosses this boundary. Rebinding an asset requires a fresh ID.
#[derive(Clone)]
pub struct LocalSubtitleTarget(Arc<()>);
impl LocalSubtitleTarget {
    pub fn new() -> Self { Self(Arc::new(())) }
}
impl Default for LocalSubtitleTarget { fn default() -> Self { Self::new() } }
impl PartialEq for LocalSubtitleTarget {
    fn eq(&self, other: &Self) -> bool { Arc::ptr_eq(&self.0, &other.0) }
}
impl Eq for LocalSubtitleTarget {}
impl std::fmt::Debug for LocalSubtitleTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LocalSubtitleTarget(..)")
    }
}

/// Labels have already passed the application's shared track metadata bounds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSubtitleTrack {
    pub target: LocalSubtitleTarget,
    pub format: SidecarFormat,
    pub title: String,
    pub language: String,
}

#[path = "external_tracks_policy.rs"]
mod policy;
pub use policy::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackUnavailable {
    Codec,
    Transport,
    Decoder,
    MissingTrack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackAvailability {
    Supported,
    Unsupported(TrackUnavailable),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalTrackError {
    InvalidInventory,
    MissingTrack,
    Unsupported(TrackUnavailable),
    StaleSession,
}

#[derive(Debug)]
struct Catalog {
    inventory: TrackInventory,
    audio: Vec<TrackAvailability>,
    subtitles: Vec<TrackAvailability>,
    local_subtitles: Vec<LocalSubtitleTrack>,
    local_complete: bool,
}

#[derive(Clone, Debug)]
pub struct ExternalTracks {
    catalog: Arc<Catalog>,
    selected_audio: Option<i32>,
    selected_subtitle: Option<i32>,
    selected_local: Option<LocalSubtitleTarget>,
}

/// Only a catalog can mint a choice. The retained identity keeps pointer reuse from
/// turning an old menu action into a valid action on a later catalog.
#[derive(Clone, Debug)]
pub struct ExternalTrackChoice {
    catalog: Arc<Catalog>,
    kind: TrackKind,
    ordinal: Option<i32>,
    local: Option<LocalSubtitleTarget>,
}

impl PartialEq for ExternalTrackChoice {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.catalog, &other.catalog)
            && self.kind == other.kind
            && self.ordinal == other.ordinal
            && self.local == other.local
    }
}
impl Eq for ExternalTrackChoice {}

impl ExternalTrackChoice {
    pub fn kind(&self) -> TrackKind {
        self.kind
    }
    /// Container ordinal only. A Local choice has no demuxer ordinal; inspect
    /// local_target before interpreting None as the Off choice.
    pub fn ordinal(&self) -> Option<i32> { self.ordinal }
    pub fn local_target(&self) -> Option<&LocalSubtitleTarget> { self.local.as_ref() }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalTrackPlan {
    Unchanged,
    Audio(InspectedTrack),
    Subtitle(Option<InspectedTrack>),
    LocalSubtitle(LocalSubtitleTrack),
}

impl ExternalTracks {
    /// Availability is supplied by the native adapter's actual codec/transport policy,
    /// never guessed from the presence of a stream in the container.
    pub fn new(
        inventory: TrackInventory,
        audio: Vec<TrackAvailability>,
        subtitles: Vec<TrackAvailability>,
        selected_audio: Option<i32>,
    ) -> Result<Self, ExternalTrackError> {
        if inventory.audio.len() != audio.len() || inventory.subtitles.len() != subtitles.len() {
            return Err(ExternalTrackError::InvalidInventory);
        }
        let mut stream_ids = HashSet::new();
        for (kind, rows) in [
            (TrackKind::Audio, &inventory.audio),
            (TrackKind::Subtitle, &inventory.subtitles),
        ] {
            for (ordinal, track) in rows.iter().enumerate() {
                if track.kind != kind
                    || track.ordinal < 0
                    || track.ordinal as usize != ordinal
                    || track.stream_index < 0
                    || !stream_ids.insert(track.stream_index)
                {
                    return Err(ExternalTrackError::InvalidInventory);
                }
            }
        }
        let catalog = Self {
            catalog: Arc::new(Catalog {
                inventory,
                audio,
                subtitles,
                local_subtitles: Vec::new(),
                local_complete: true,
            }),
            selected_audio,
            selected_subtitle: None,
            selected_local: None,
        };
        if let Some(ordinal) = selected_audio {
            // A transport may play its default stream but not support switching it.
            // Preserve that truthful selection while keeping the row non-actionable.
            if usize::try_from(ordinal)
                .ok()
                .and_then(|i| catalog.catalog.inventory.audio.get(i))
                .is_none()
            {
                return Err(ExternalTrackError::MissingTrack);
            }
        }
        Ok(catalog)
    }

    pub fn inventory(&self) -> &TrackInventory {
        &self.catalog.inventory
    }
    pub fn selected_audio(&self) -> Option<i32> {
        self.selected_audio
    }
    pub fn selected_subtitle(&self) -> Option<i32> {
        self.selected_subtitle
    }
    pub fn local_subtitles(&self) -> &[LocalSubtitleTrack] { &self.catalog.local_subtitles }
    pub fn selected_local_subtitle(&self) -> Option<&LocalSubtitleTarget> { self.selected_local.as_ref() }
    pub fn is_complete(&self) -> bool { self.inventory().is_complete() && self.catalog.local_complete }

    /// Replace immutable discovery data, retaining exact selections. Limits are
    /// supplied by the application's existing 128-total/64-kind contract.
    /// Returns true if a selected Local asset disappeared and must be retired.
    pub fn replace_local_subtitles(&mut self, rows: Vec<LocalSubtitleTrack>, complete: bool,
        max_total: usize, max_kind: usize,
    ) -> Result<bool, ExternalTrackError> {
        let inventory = &self.catalog.inventory;
        if inventory.audio.len() > max_kind
            || inventory.subtitles.len().saturating_add(rows.len()) > max_kind
            || inventory.audio.len().saturating_add(inventory.subtitles.len()).saturating_add(rows.len()) > max_total
            || rows.iter().any(|row| row.title.trim().is_empty()
                || row.title.chars().any(char::is_control) || row.language.chars().any(char::is_control))
        { return Err(ExternalTrackError::InvalidInventory); }
        let mut ids = HashSet::new();
        if rows.iter().any(|row| !ids.insert(Arc::as_ptr(&row.target.0) as usize)
            || self.local_subtitles().iter().any(|old| old.target == row.target && old.format != row.format)) {
            return Err(ExternalTrackError::InvalidInventory);
        }
        let removed = self.selected_local.as_ref().is_some_and(|selected|
            !rows.iter().any(|row| &row.target == selected));
        if removed { self.selected_local = None; }
        self.catalog = Arc::new(Catalog {
            inventory: inventory.clone(), audio: self.catalog.audio.clone(),
            subtitles: self.catalog.subtitles.clone(), local_subtitles: rows, local_complete: complete,
        });
        Ok(removed)
    }
    /// Refresh one committed TS inventory. A new PMT never turns an ordinal or
    /// language into identity; only the same verified transport and codec shape
    /// can retain a selected track. Segment observations are always partial.
    pub(crate) fn replace_observed_inventory(&mut self, mut inventory: TrackInventory,
        audio: Vec<TrackAvailability>, subtitles: Vec<TrackAvailability>, force_identity: bool, observed_audio: Option<i32>,
    ) -> Result<(bool, bool), ExternalTrackError> {
        inventory.mark_incomplete();
        if !force_identity && self.inventory() == &inventory
            && self.catalog.audio == audio && self.catalog.subtitles == subtitles { return Ok((false, false)); }
        let retain = |kind, ordinal: Option<i32>| {
            let old_rows = if kind == TrackKind::Audio { &self.inventory().audio } else { &self.inventory().subtitles };
            let rows = if kind == TrackKind::Audio { &inventory.audio } else { &inventory.subtitles };
            let old = old_rows.iter().find(|t| Some(t.ordinal) == ordinal)?;
            let transport = self.inventory().transport_identity(old.stream_index)?;
            let mut matches = rows.iter().filter(|track|
                inventory.transport_identity(track.stream_index) == Some(transport)
                && track.codec == old.codec && track.channels == old.channels
                && track.sample_rate == old.sample_rate && track.bit_depth == old.bit_depth);
            let retained = matches.next()?.ordinal;
            matches.next().is_none().then_some(retained)
        };
        let had_audio_identity = self.selected_audio.and_then(|ordinal| self.inventory().audio.iter().find(|track| track.ordinal == ordinal))
            .and_then(|track| self.inventory().transport_identity(track.stream_index)).is_some();
        let selected_audio = retain(TrackKind::Audio, self.selected_audio)
            .or_else(|| (!had_audio_identity).then_some(observed_audio).flatten());
        let selected_subtitle = retain(TrackKind::Subtitle, self.selected_subtitle);
        let removed = self.selected_subtitle.is_some() && selected_subtitle.is_none();
        let mut next = Self::new(inventory, audio, subtitles, selected_audio)?;
        next.selected_subtitle = selected_subtitle;
        *self = next;
        Ok((true, removed))
    }

    pub fn local_choice(&self, target: &LocalSubtitleTarget) -> Result<ExternalTrackChoice, ExternalTrackError> {
        if !self.local_subtitles().iter().any(|row| row.target == *target) {
            return Err(ExternalTrackError::MissingTrack);
        }
        Ok(ExternalTrackChoice { catalog: self.catalog.clone(), kind: TrackKind::Subtitle,
            ordinal: None, local: Some(target.clone()) })
    }
    pub fn same_session(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.catalog, &other.catalog)
    }

    pub fn availability(&self, kind: TrackKind, ordinal: i32) -> TrackAvailability {
        let rows = match kind {
            TrackKind::Audio => &self.catalog.audio,
            TrackKind::Subtitle => &self.catalog.subtitles,
        };
        usize::try_from(ordinal)
            .ok()
            .and_then(|i| rows.get(i))
            .copied()
            .unwrap_or(TrackAvailability::Unsupported(
                TrackUnavailable::MissingTrack,
            ))
    }

    fn check(&self, kind: TrackKind, ordinal: i32) -> Result<(), ExternalTrackError> {
        match self.availability(kind, ordinal) {
            TrackAvailability::Supported => Ok(()),
            TrackAvailability::Unsupported(TrackUnavailable::MissingTrack) => {
                Err(ExternalTrackError::MissingTrack)
            }
            TrackAvailability::Unsupported(reason) => Err(ExternalTrackError::Unsupported(reason)),
        }
    }

    pub fn choice(
        &self,
        kind: TrackKind,
        ordinal: Option<i32>,
    ) -> Result<ExternalTrackChoice, ExternalTrackError> {
        match ordinal {
            Some(ordinal) => self.check(kind, ordinal)?,
            None if kind == TrackKind::Subtitle => {}
            None => return Err(ExternalTrackError::MissingTrack),
        }
        Ok(ExternalTrackChoice {
            catalog: self.catalog.clone(),
            kind,
            ordinal,
            local: None,
        })
    }

    /// A successful plan grants no decoder success. The caller first admits the plan
    /// through the existing playback owner, then calls `commit` on that same catalog.
    pub fn plan(
        &self,
        choice: &ExternalTrackChoice,
    ) -> Result<ExternalTrackPlan, ExternalTrackError> {
        if !Arc::ptr_eq(&self.catalog, &choice.catalog) {
            return Err(ExternalTrackError::StaleSession);
        }
        if let Some(target) = &choice.local {
            let row = self.local_subtitles().iter().find(|row| row.target == *target)
                .ok_or(ExternalTrackError::MissingTrack)?;
            // Explicit reselect is also a fresh read ticket, so a previous failure
            // or held completion cannot strand/revive this pick.
            return Ok(ExternalTrackPlan::LocalSubtitle(row.clone()));
        }
        if let Some(ordinal) = choice.ordinal {
            self.check(choice.kind, ordinal)?;
        }
        match choice.kind {
            TrackKind::Audio => {
                let ordinal = choice.ordinal.ok_or(ExternalTrackError::MissingTrack)?;
                if self.selected_audio == Some(ordinal) {
                    return Ok(ExternalTrackPlan::Unchanged);
                }
                Ok(ExternalTrackPlan::Audio(
                    self.catalog.inventory.audio[ordinal as usize].clone(),
                ))
            }
            TrackKind::Subtitle => {
                if self.selected_local.is_none() && self.selected_subtitle == choice.ordinal {
                    return Ok(ExternalTrackPlan::Unchanged);
                }
                Ok(ExternalTrackPlan::Subtitle(choice.ordinal.map(|i| {
                    self.catalog.inventory.subtitles[i as usize].clone()
                })))
            }
        }
    }

    pub fn commit(
        &mut self,
        choice: &ExternalTrackChoice,
    ) -> Result<ExternalTrackPlan, ExternalTrackError> {
        let plan = self.plan(choice)?;
        match choice.kind {
            TrackKind::Audio => self.selected_audio = choice.ordinal,
            TrackKind::Subtitle => {
                self.selected_subtitle = choice.ordinal;
                self.selected_local = choice.local.clone();
            },
        }
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(kind: TrackKind, stream_index: i32, ordinal: i32) -> InspectedTrack {
        InspectedTrack {
            kind,
            stream_index,
            ordinal,
            codec: if kind == TrackKind::Audio {
                "aac"
            } else {
                "subrip"
            }
            .into(),
            language: "eng".into(),
            title: format!("Track {stream_index}"),
            channels: 2,
            sample_rate: 48000,
            bit_depth: 16,
            default: false,
            forced: false,
            disposition_known: false,
            decoder_available: true,
        }
    }
    fn inventory() -> TrackInventory {
        TrackInventory::from_partial(
            vec![track(TrackKind::Audio, 1, 0), track(TrackKind::Audio, 3, 1)],
            vec![
                track(TrackKind::Subtitle, 2, 0),
                track(TrackKind::Subtitle, 4, 1),
            ],
        )
    }
    #[test]
    fn partial_inventory_stays_available_to_manual_selection_without_becoming_complete() {
        let tracks = catalog();
        assert!(!tracks.inventory().is_complete());
        let mut copy = tracks.clone();
        let choice = copy.choice(TrackKind::Audio, Some(1)).unwrap();
        assert!(matches!(copy.commit(&choice), Ok(ExternalTrackPlan::Audio(_))));
        assert!(!copy.inventory().is_complete());
        assert!(copy.same_session(&tracks));
    }

    fn catalog() -> ExternalTracks {
        ExternalTracks::new(
            inventory(),
            vec![TrackAvailability::Supported; 2],
            vec![TrackAvailability::Supported; 2],
            Some(0),
        )
        .unwrap()
    }

    #[test]
    fn another_playback_of_the_same_inventory_rejects_the_old_choice() {
        let first = catalog();
        let choice = first.choice(TrackKind::Audio, Some(1)).unwrap();
        let mut next = catalog();
        assert_eq!(next.commit(&choice), Err(ExternalTrackError::StaleSession));
        assert_eq!(next.selected_audio(), Some(0));
    }

    #[test]
    fn same_session_reload_preserves_choice_identity_and_container_ordinal() {
        let first = catalog();
        let choice = first.choice(TrackKind::Audio, Some(1)).unwrap();
        let mut reload = first.clone();
        assert!(reload.same_session(&first));
        match reload.commit(&choice).unwrap() {
            ExternalTrackPlan::Audio(track) => {
                assert_eq!((track.stream_index, track.ordinal), (3, 1))
            }
            other => panic!("unexpected plan {other:?}"),
        }
        assert_eq!(reload.selected_audio(), Some(1));
        assert_eq!(reload.plan(&choice), Ok(ExternalTrackPlan::Unchanged));
    }

    #[test]
    fn subtitles_select_and_turn_off_without_changing_audio() {
        let mut active = catalog();
        let choice = active.choice(TrackKind::Subtitle, Some(1)).unwrap();
        assert!(matches!(
            active.commit(&choice),
            Ok(ExternalTrackPlan::Subtitle(Some(_)))
        ));
        assert_eq!(active.selected_subtitle(), Some(1));
        let off = active.choice(TrackKind::Subtitle, None).unwrap();
        assert_eq!(active.commit(&off), Ok(ExternalTrackPlan::Subtitle(None)));
        assert_eq!(active.selected_subtitle(), None);
        assert_eq!(active.selected_audio(), Some(0));
    }

    #[test]
    fn unsupported_or_missing_rows_cannot_change_selection() {
        let active = ExternalTracks::new(
            inventory(),
            vec![
                TrackAvailability::Supported,
                TrackAvailability::Unsupported(TrackUnavailable::Codec),
            ],
            vec![TrackAvailability::Supported; 2],
            Some(0),
        )
        .unwrap();
        assert_eq!(
            active.choice(TrackKind::Audio, Some(1)).unwrap_err(),
            ExternalTrackError::Unsupported(TrackUnavailable::Codec)
        );
        assert_eq!(
            active.choice(TrackKind::Audio, Some(-1)).unwrap_err(),
            ExternalTrackError::MissingTrack
        );
        assert_eq!(
            active.choice(TrackKind::Subtitle, Some(2)).unwrap_err(),
            ExternalTrackError::MissingTrack
        );
        assert_eq!(
            active.choice(TrackKind::Audio, None).unwrap_err(),
            ExternalTrackError::MissingTrack
        );
        assert_eq!(active.selected_audio(), Some(0));
    }

    #[test]
    fn playing_default_is_visible_when_transport_cannot_switch_tracks() {
        let unavailable = TrackAvailability::Unsupported(TrackUnavailable::Transport);
        let active = ExternalTracks::new(
            inventory(),
            vec![unavailable; 2],
            vec![unavailable; 2],
            Some(0),
        )
        .unwrap();
        assert_eq!(active.selected_audio(), Some(0));
        assert_eq!(active.inventory().audio.len(), 2);
        assert_eq!(
            active.choice(TrackKind::Audio, Some(0)).unwrap_err(),
            ExternalTrackError::Unsupported(TrackUnavailable::Transport)
        );
        assert!(active.choice(TrackKind::Subtitle, None).is_ok());
        assert_eq!(
            ExternalTracks::new(
                inventory(),
                vec![unavailable; 2],
                vec![unavailable; 2],
                Some(2),
            )
            .unwrap_err(),
            ExternalTrackError::MissingTrack
        );
    }

    #[test]
    fn invalid_inventory_never_becomes_a_session() {
        for case in 0..4 {
            let mut tracks = inventory();
            match case {
                0 => tracks.subtitles[0].stream_index = tracks.audio[0].stream_index,
                1 => tracks.audio[1].ordinal = 9,
                2 => tracks.audio[1].kind = TrackKind::Subtitle,
                _ => tracks.audio[1].stream_index = -1,
            }
            assert_eq!(
                ExternalTracks::new(
                    tracks,
                    vec![TrackAvailability::Supported; 2],
                    vec![TrackAvailability::Supported; 2],
                    Some(0)
                )
                .unwrap_err(),
                ExternalTrackError::InvalidInventory
            );
        }
        assert!(ExternalTracks::new(
            inventory(),
            Vec::new(),
            vec![TrackAvailability::Supported; 2],
            Some(0)
        )
        .is_err());
    }
    fn local_row(title: &str) -> LocalSubtitleTrack {
        LocalSubtitleTrack { target: LocalSubtitleTarget::new(), format: SidecarFormat::Srt,
            title: title.into(), language: "en".into() }
    }
    #[test]
    fn local_catalog_replacement_preserves_embedded_selection_and_invalidates_old_choices() {
        let mut active = catalog();
        let embedded = active.choice(TrackKind::Subtitle, Some(1)).unwrap();
        active.commit(&embedded).unwrap();
        let old = active.clone();
        let row = local_row("Movie.en.srt");
        assert!(!active.replace_local_subtitles(vec![row.clone()], false, 128, 64).unwrap());
        assert!(!active.is_complete());
        assert_eq!(active.selected_subtitle(), Some(1));
        assert_eq!(active.selected_audio(), Some(0));
        assert!(!active.same_session(&old));
        assert_eq!(active.plan(&embedded), Err(ExternalTrackError::StaleSession));
        let local = active.local_choice(&row.target).unwrap();
        assert_eq!(local.ordinal(), None);
        assert_eq!(local.local_target(), Some(&row.target));
        active.commit(&local).unwrap();
        assert_eq!(active.selected_subtitle(), None);
        assert_eq!(active.selected_local_subtitle(), Some(&row.target));
        assert!(!active.replace_local_subtitles(vec![row.clone()], true, 128, 64).unwrap());
        assert_eq!(active.selected_local_subtitle(), Some(&row.target));
        assert_eq!(active.plan(&local), Err(ExternalTrackError::StaleSession));
        assert!(active.replace_local_subtitles(Vec::new(), true, 128, 64).unwrap());
        assert_eq!(active.selected_local_subtitle(), None);
        assert_eq!(active.local_choice(&row.target), Err(ExternalTrackError::MissingTrack));
    }
    #[test]
    fn local_catalog_limits_duplicates_and_reused_identity_format_are_atomic() {
        let mut active = catalog();
        let before = active.clone();
        let row = local_row("Movie.srt");
        for (rows,total,kind) in [(vec![row.clone()],4,64),(vec![row.clone()],128,2),
            (vec![row.clone(),row.clone()],128,64)] {
            assert_eq!(active.replace_local_subtitles(rows,true,total,kind), Err(ExternalTrackError::InvalidInventory));
            assert!(active.same_session(&before));
        }
        active.replace_local_subtitles(vec![row.clone()],true,5,3).unwrap();
        let before = active.clone();
        let mut mutated = row;
        mutated.format = SidecarFormat::Ass;
        assert_eq!(active.replace_local_subtitles(vec![mutated],true,128,64), Err(ExternalTrackError::InvalidInventory));
        assert!(active.same_session(&before));
    }
    #[test]
    fn local_targets_are_opaque_and_equal_labels_do_not_alias() {
        let one = local_row("same.srt");
        let two = local_row("same.srt");
        assert_ne!(one.target,two.target);
        assert_eq!(format!("{:?}",one.target),"LocalSubtitleTarget(..)");
        let mut active = catalog();
        active.replace_local_subtitles(vec![one.clone(),two.clone()],true,128,64).unwrap();
        active.commit(&active.local_choice(&one.target).unwrap()).unwrap();
        let off = active.choice(TrackKind::Subtitle,None).unwrap();
        assert_eq!(off.local_target(),None);
        active.commit(&off).unwrap();
        assert_eq!(active.selected_local_subtitle(),None);
    }

}

#[cfg(test)]
mod observed_tests {
    use super::*;
    use crate::ff::hls_observed::{Scope, Snapshot};
    #[test]
    fn observed_pmt_reordering_retains_pid_and_partial_status_without_language_deduplication() {
        let scope = Scope::new("https://example.invalid/master.m3u8", "https://example.invalid/video.m3u8", 1, 0).unwrap();
        let snapshot = Snapshot::fixture_for_test(scope);
        let inventory = snapshot.inventory().clone();
        let availability = vec![TrackAvailability::Supported; inventory.audio.len()];
        let subtitles = vec![TrackAvailability::Unsupported(TrackUnavailable::Transport); inventory.subtitles.len()];
        let mut tracks = ExternalTracks::new(inventory.clone(), availability.clone(), subtitles.clone(), Some(0)).unwrap();
        let old = tracks.choice(TrackKind::Audio, Some(1)).unwrap();
        assert_eq!(inventory.audio[0].language, inventory.audio[1].language);
        let mut audio = inventory.audio.clone();
        audio.swap(0, 1);
        for (i, track) in audio.iter_mut().enumerate() { track.ordinal = i as i32; }
        let bindings: Vec<_> = inventory.audio.iter().chain(&inventory.subtitles)
            .map(|track| (track.stream_index, inventory.transport_identity(track.stream_index).unwrap())).collect();
        let reordered = TrackInventory::from_partial(audio, inventory.subtitles.clone()).with_transport_for_test(&bindings).unwrap();
        assert_eq!(tracks.replace_observed_inventory(reordered.clone(), availability.clone(), subtitles.clone(), false, Some(0)), Ok((true, false)));
        assert_eq!(tracks.selected_audio(), Some(1), "the same PID moved to a different dense ordinal");
        assert_eq!(tracks.inventory().audio.len(), 2, "equal languages and codecs cannot collapse distinct PIDs");
        assert!(!tracks.is_complete());
        assert_eq!(tracks.plan(&old), Err(ExternalTrackError::StaleSession));
        assert_eq!(tracks.replace_observed_inventory(reordered, availability, subtitles, false, Some(1)), Ok((false, false)),
            "ordinary segment publication does not churn menu identity");
    }
}
