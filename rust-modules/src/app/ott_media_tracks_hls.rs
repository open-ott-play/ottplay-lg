//! Complete declared HLS audio/subtitle projection and the addressed remote owner.
//! No segment inventory, preparation IO, or decoder observation is performed here.
use super::*;
use plx_appkit::track_menu::{TrackCommitFailure, TrackCommitOutcome};

pub(super) fn complete(catalog: &route::HlsTrackCatalog) -> bool {
    let audio = catalog.rows().len();
    let subtitles = catalog.subtitle_rows().len();
    let count = audio.saturating_add(subtitles);
    count != 0 && audio <= wire::MAX_TRACKS_PER_KIND && subtitles <= wire::MAX_TRACKS_PER_KIND
        && count <= wire::MAX_TRACKS
}

pub(super) fn inventory(
    catalog: &route::HlsTrackCatalog,
    mutable: bool,
    token: &str,
) -> wire::Inventory<NativeChoice> {
    if !complete(catalog) { return wire::Inventory::Incomplete; }
    let mut tracks: Vec<_> = catalog.rows().into_iter().enumerate().map(|(index, row)| wire::Track {
        id: format!("{token}_h{index}"),
        kind: wire::Kind::Audio,
        label: row.name,
        language: Some(row.language),
        // The declared group does not prove the codec of an unprepared lane.
        codec: None,
        choice: mutable.then(|| catalog.choice(row.id).ok()).flatten().map(NativeChoice::Hls),
    }).collect();
    let subtitles = catalog.subtitle_rows();
    tracks.extend(subtitles.iter().enumerate().map(|(index, row)| wire::Track {
        id: format!("{token}_hs{index}"),
        kind: wire::Kind::Subtitle,
        label: row.name.clone(),
        language: Some(row.language.clone()),
        // A declared subtitle URI is validated only by its preparation worker.
        codec: None,
        choice: mutable.then(|| catalog.subtitle_choice(Some(row.id)).ok()).flatten().map(NativeChoice::Hls),
    }));
    let subtitle_off = (mutable && !subtitles.is_empty())
        .then(|| catalog.subtitle_choice(None).ok()).flatten().map(NativeChoice::Hls);
    wire::Inventory::Complete { tracks, subtitle_off }
}

pub(super) fn apply_error(error: route::ExternalTrackApplyError) -> wire::Error {
    match error {
        route::ExternalTrackApplyError::Busy => wire::Error::Busy,
        route::ExternalTrackApplyError::Inactive => wire::Error::NoPlayback,
        route::ExternalTrackApplyError::InvalidChoice(_) => wire::Error::NotSelectable,
    }
}

pub(in crate::app::ott_runtime) struct Remote {
    invocation: Invocation,
    request: wire::Request,
    selection: wire::Selection<Owner, NativeChoice>,
    owner: Owner,
    policy: PolicyReader,
}
impl Remote {
    pub(super) fn new(invocation: Invocation, request: wire::Request,
        selection: wire::Selection<Owner, NativeChoice>, owner: Owner, policy: PolicyReader) -> Self
    {
        Self { invocation, request, selection, owner, policy }
    }

    pub(in crate::app::ott_runtime) fn current(&self, runtime: &Runtime, app: &App,
        request: &route::HlsTrackRequest, now_ms: Option<u64>) -> Result<(), wire::Error>
    {
        if !runtime.remote.invocation_is_current(&self.invocation)
            || runtime.remote.runtime() != self.owner.runtime
        { return Err(wire::Error::RuntimeMismatch); }
        let now = now_ms.ok_or(wire::Error::Unavailable)?;
        if now >= self.invocation.deadline_ms || now >= self.selection.deadline_ms()
        { return Err(wire::Error::Unavailable); }
        if !runtime.loaded || runtime.persistence_uncertain { return Err(wire::Error::Unavailable); }
        if !super::super::super::hls_tracks::source_available(runtime) { return Err(wire::Error::Unavailable); }
        let policy = (self.policy)(runtime).map_err(|_| wire::Error::Restricted)?;
        if policy != self.owner.kiosk { return Err(wire::Error::Restricted); }
        mutation_allowed(runtime, app, policy)?;
        if route::playback_identity(&app.player.session).as_ref() != Some(&self.owner.playback)
        { return Err(wire::Error::PlaybackChanged); }
        // begin_hls_track_request owns a NEW intent revision. Comparing the old
        // passive Owner would reject this request itself. The route checks the
        // post-begin revision, worker ticket and immutable presentation identity.
        if !route::hls_track_request_is_current(&app.player.session, request)
        { return Err(wire::Error::CatalogChanged); }
        Ok(())
    }

    pub(in crate::app::ott_runtime) fn reject(self, runtime: &mut Runtime, error: wire::Error) {
        let response = self.request.error_response(error).into_envelope();
        // The existing Remote owner suppresses replies to a revoked generation.
        runtime.remote.complete(self.invocation, response);
    }

    pub(in crate::app::ott_runtime) fn finish(self, runtime: &mut Runtime, outcome: TrackCommitOutcome) {
        let response = match outcome {
            TrackCommitOutcome::Admitted | TrackCommitOutcome::Unchanged => self.selection.requested_response(),
            // HLS selection never performs a persistent-setting write.
            TrackCommitOutcome::Saved => self.request.error_response(wire::Error::Unavailable),
            TrackCommitOutcome::Rejected(why) => self.request.error_response(match why {
                TrackCommitFailure::Busy => wire::Error::Busy,
                TrackCommitFailure::StalePlayback => wire::Error::CatalogChanged,
                _ => wire::Error::Unavailable,
            }),
        };
        runtime.remote.complete(self.invocation, response.into_envelope());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hls_inventory_preserves_audio_ids_and_keeps_read_only_subtitles_inert() {
        let _serial = plx_base::testlock::serial();
        let _globals = route::HlsTrackRequest::test_globals_guard();
        let playback = plx_media::ff::hls_presentation::HlsPlayback::fixture_with_subtitles_for_test();
        let mut ps = route::PlaybackSession::IDLE;
        route::reset_player_control_for_test(&ps);
        route::request_external_source(&mut ps, &playback.media_info_for_test(),
            plx_media::ff::ExternalSource::Hls("https://example.invalid/master.m3u8".into()),
            "Fixture", "M3U", plx_platform::devcaps::assumed_shared()).unwrap();
        route::reset_player_control_for_test(&ps);
        let catalog = route::hls_track_catalog(&ps).unwrap();
        for mutable in [false, true] {
            let wire::Inventory::Complete { tracks, subtitle_off } = inventory(&catalog, mutable, "fixture")
                else { panic!("complete declared catalog") };
            assert_eq!(tracks.len(), 4);
            assert_eq!(tracks[0].id, "fixture_h0");
            assert_eq!(tracks[1].id, "fixture_h1");
            assert_eq!(tracks[2].id, "fixture_hs0");
            assert_eq!(tracks[3].id, "fixture_hs1");
            assert!(tracks[..2].iter().all(|row| row.kind == wire::Kind::Audio));
            assert!(tracks[2..].iter().all(|row| row.kind == wire::Kind::Subtitle));
            assert!(tracks.iter().all(|row| row.codec.is_none() && row.choice.is_some() == mutable));
            assert_eq!(subtitle_off.is_some(), mutable);
            if mutable {
                let Some(NativeChoice::Hls(off)) = subtitle_off else { panic!("owned HLS Off") };
                assert_eq!(off.target(), route::HlsTrackTarget::Subtitle(None));
            }
        }
    }
}
