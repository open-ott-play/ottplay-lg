use super::*;
#[test]
fn observed_pid_selection_ignores_same_labels_and_tracks_current_ordinals() {
    let scope=Scope::new("https://example.invalid/master.m3u8","https://example.invalid/video.m3u8",7,2).unwrap();
    let snapshot=Snapshot::fixture_for_test(scope);
    assert!(!snapshot.inventory().is_complete());
    let selected=Selection::new(snapshot.inventory(),TrackKind::Audio,1).unwrap();
    assert_eq!(selected.transport_identity().pid,0x102);
    let mut reordered=snapshot.inventory().clone();
    reordered.audio.swap(0,1);
    for (i,row) in reordered.audio.iter_mut().enumerate() { row.ordinal=i as i32; }
    assert_eq!(selected.resolve(&reordered),Ok(0));
    assert_eq!(selected.resolve_stream_index(&reordered),Ok(3));
    reordered.audio[0].sample_rate=44100;
    assert!(selected.resolve(&reordered).is_err());
}
#[test]
fn observed_probe_nonsequential_or_truncated_bytes_never_finish() {
    let mut probe=TsProbe::new();
    assert!(probe.feed_at(1,&[0x47]).is_err());
    assert!(probe.finish().is_err());
    let mut probe=TsProbe::new();
    probe.feed(&[0x47]).unwrap();
    assert!(probe.finish().is_err());
}

#[test]
fn observed_finite_segment_before_load_returns_stays_available_until_native_retirement() {
    let _serial=plx_base::testlock::serial();
    let _globals=crate::route::HlsTrackRequest::test_globals_guard();
    let playback=crate::ff::hls_presentation::HlsPlayback::fixture_for_test();
    let info=playback.media_info_for_test();
    let mut ps=crate::route::PlaybackSession::IDLE;
    crate::route::reset_player_control_for_test(&ps);
    crate::route::request_external_source(&mut ps,&info,
        crate::ff::ExternalSource::Hls("https://example.invalid/master.m3u8".into()),"Fixture","M3U",
        plx_platform::devcaps::assumed_shared()).unwrap();
    let logical=crate::route::playback_identity(&ps).unwrap();
    let native=super::super::SHARED.begin_native_session().expect("serialized test starts native session");
    struct Restore { observed:Option<State>, native:u32 }
    impl Drop for Restore {
        fn drop(&mut self) {
            super::super::SHARED.retire_native_session(self.native);
            *OBSERVED.lock().unwrap_or_else(|e| e.into_inner())=self.observed.take();
        }
    }
    let _restore=Restore { observed:OBSERVED.lock().unwrap_or_else(|e| e.into_inner()).take(),native };
    let scope=Scope::new("https://example.invalid/master.m3u8","https://example.invalid/video.m3u8",u64::from(native),0)
        .unwrap().with_playback(logical);
    let fixture=Snapshot::fixture_for_test(scope.clone());
    let candidate=Candidate { inventory:fixture.inventory().clone(),selected_audio:fixture.selected_audio(),
        video:TransportIdentity { transport_stream_id:1,program_id:1,pid:0x100,stream_type:0x1b } };
    assert!(!super::super::SHARED.native_load_returned(native));
    commit_observed(scope,0,candidate).unwrap();
    let snapshot=observed_hls_tracks().expect("a fully fed finite segment may finish before Load returns");
    assert!(!snapshot.inventory().is_complete());
    assert_eq!(observed_hls_tracks().unwrap().revision(),snapshot.revision(),"producer EOF does not retire buffered media");
    assert!(super::super::SHARED.mark_native_load_returned(native,None).is_some());
    assert!(observed_hls_tracks().is_some());
    assert!(super::super::SHARED.retire_native_session(native));
    assert!(observed_hls_tracks().is_none());
}

#[test]
fn guarded_subtitle_publication_is_drained_by_off_and_rejected_after_epoch_retirement() {
    let _serial = plx_base::testlock::serial();
    let _globals = crate::route::HlsTrackRequest::test_globals_guard();
    let playback = crate::ff::hls_presentation::HlsPlayback::fixture_for_test();
    let mut ps = crate::route::PlaybackSession::IDLE;
    crate::route::reset_player_control_for_test(&ps);
    crate::route::request_external_source(&mut ps, &playback.media_info_for_test(),
        crate::ff::ExternalSource::Hls("https://example.invalid/master.m3u8".into()), "Fixture", "M3U",
        plx_platform::devcaps::assumed_shared()).unwrap();
    let native = super::super::SHARED.begin_native_session().unwrap();
    let logical = crate::route::playback_identity(&ps).unwrap();
    let scope = Scope::new("https://example.invalid/master.m3u8", "https://example.invalid/video.m3u8",
        u64::from(native), 0).unwrap().with_playback(logical);
    let row = InspectedTrack { kind: TrackKind::Subtitle, stream_index: 2, ordinal: 0,
        codec: "webvtt".into(), language: "eng".into(), title: "English".into(), channels: 0,
        sample_rate: 0, bit_depth: 0, decoder_available: false,
        disposition_known: false, default: false, forced: false };
    let inventory = TrackInventory::from_partial(Vec::new(), vec![row]).with_transport_for_test(&[
        (2, TransportIdentity { transport_stream_id: 1, program_id: 1, pid: 0x120, stream_type: 0x06 }),
    ]).unwrap();
    let previous = OBSERVED.lock().unwrap().take();
    let previous_subtitle = SUBTITLE.lock().unwrap().clone();
    let snapshot = State::default().commit(scope.clone(), 0, inventory).unwrap();
    let mut state = State::default();
    state.current = Some(snapshot.clone());
    *OBSERVED.lock().unwrap() = Some(state);
    let selection = Selection::new(snapshot.inventory(), TrackKind::Subtitle, 0).unwrap();
    set_subtitle_selection(&snapshot, selection.clone()).unwrap();
    let request = subtitle_request(scope.playback().unwrap(), "https://example.invalid/master.m3u8").unwrap();
    crate::player::request_subtitle(0);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let producer = std::thread::spawn(move || {
        super::super::SHARED.with_active_native_load(native, || with_current_subtitle_request(&request, || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            super::super::hls_embedded_subtitles::text_batch_for_test().publish(
                "fixture:guarded", crate::player::text_style::TextStyle::default(), &mut None, 0);
        })).flatten().unwrap();
    });
    started_rx.recv().unwrap();
    let (off_tx, off_rx) = std::sync::mpsc::channel();
    let off = std::thread::spawn(move || {
        clear_subtitle_selection();
        crate::player::ass_source::retire_selection();
        off_tx.send(()).unwrap();
    });
    assert!(off_rx.recv_timeout(std::time::Duration::from_millis(20)).is_err(),
        "Off cannot pass an in-flight selection-guarded publication");
    release_tx.send(()).unwrap();
    producer.join().unwrap(); off.join().unwrap();
    assert!(crate::player::ass_source::selected(0, crate::player::text_style::TextStyle::default()).is_none(),
        "Off wins after the previously admitted batch finishes");
    set_subtitle_selection(&snapshot, selection).unwrap();
    let request = subtitle_request(scope.playback().unwrap(), "https://example.invalid/master.m3u8").unwrap();
    assert!(super::super::SHARED.retire_native_session(native));
    let mut ran = false;
    assert!(super::super::SHARED.with_active_native_load(native, ||
        with_current_subtitle_request(&request, || ran = true)).is_none());
    assert!(!ran, "a retired Load cannot reach the caption store");
    *OBSERVED.lock().unwrap() = previous;
    *SUBTITLE.lock().unwrap() = previous_subtitle;
    crate::player::request_subtitle(-1);
    crate::player::ass_source::retire_selection();
}
