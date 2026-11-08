use super::*;

fn inline() -> Target {
    Target::Inline {
        rect: Viewport {
            x: 48,
            y: 144,
            width: 704,
            height: 396,
        },
        canvas: (1920, 1080),
    }
}

#[test]
fn logical_geometry_rejects_empty_negative_overflow_and_outside_canvas() {
    for rect in [
        Viewport {
            x: 0,
            y: 0,
            width: 0,
            height: 1,
        },
        Viewport {
            x: -1,
            y: 0,
            width: 1,
            height: 1,
        },
        Viewport {
            x: i32::MAX,
            y: 0,
            width: 1,
            height: 1,
        },
        Viewport {
            x: 1919,
            y: 0,
            width: 2,
            height: 1,
        },
        Viewport {
            x: 0,
            y: 1080,
            width: 1,
            height: 1,
        },
    ] {
        assert_eq!(
            validate(Target::Inline {
                rect,
                canvas: (1920, 1080)
            }),
            Err(Failure::InvalidGeometry)
        );
    }
    assert_eq!(validate(inline()), Ok(()));
    assert_eq!(validate(Target::Fullscreen), Ok(()));
}

#[test]
fn unsupported_canvas_is_not_reinterpreted_as_native_pixels() {
    assert_eq!(
        exported_destination(Target::Inline {
            rect: Viewport {
                x: 1,
                y: 1,
                width: 20,
                height: 20
            },
            canvas: (3840, 2160),
        }),
        Err(Failure::Unsupported)
    );
}

#[test]
fn empty_presentation_canvas_has_no_geometry() {
    for (width, height) in [(0, 1080), (1920, 0), (-1, 1080), (1920, i32::MIN)] {
        assert_eq!(
            crate::player::video_presentation(width, height),
            Viewport::default()
        );
    }
}

#[test]
#[cfg(not(feature = "hostsim"))]
fn unqualified_backend_refuses_custom_content_geometry() {
    use crate::player::video_geometry::{PictureMode, Zoom};
    // This covers the platform policy without asserting any television behavior.
    for transform in [
        Transform {
            mode: PictureMode::Fill,
            zoom: Zoom::X100,
        },
        Transform {
            mode: PictureMode::Fit,
            zoom: Zoom::X125,
        },
    ] {
        assert_eq!(
            content_destination(Target::Fullscreen, transform),
            Err(Failure::Unsupported)
        );
    }
    assert_eq!(
        content_destination(Target::Fullscreen, Transform::default()),
        exported_destination(Target::Fullscreen)
    );
}

#[cfg(feature = "hostsim")]
mod host {
    use super::*;
    use crate::{
        ff::inspect::MediaInfo,
        player::{ffi_host, SHARED, TX},
        route,
    };

    struct Rig {
        ps: PlaybackSession,
        pa: PlayerAdapter,
    }
    impl Rig {
        fn new() -> Self {
            SHARED.reset_session();
            TX.reset();
            route::reset_player_control_for_test(&PlaybackSession::IDLE);
            ffi_host::force_clocksink_for_test(true);
            let mut ps = PlaybackSession::IDLE;
            let info = MediaInfo {
                tracks: Default::default(),
                resolved_source: None,
                hls_playback: None,
                video_codec: "h264".into(),
                audio_codec: "aac".into(),
                width: 1280,
                height: 720,
                fps: 24.0,
                duration_ns: 60_000_000_000,
                audio_channels: 2,
                audio_sample_rate: 48000,
                audio_bit_depth: 16,
                dovi: Default::default(),
            };
            route::request_external(
                &mut ps,
                &info,
                "https://example.invalid/viewport.mp4",
                "Viewport",
                "Fixture",
                plx_platform::devcaps::assumed_shared(),
            )
            .unwrap();
            let transaction = route::pending_route_start().expect("real prepared route");
            let attempt = route::claim_route_start_attempt(transaction).expect("real Load ticket");
            assert!(route::settle_route_start(
                &mut ps,
                attempt,
                route::RouteStartResult::Started
            ));
            let epoch = SHARED.begin_native_session().expect("native epoch");
            assert!(SHARED.mark_native_load_returned(epoch, None).is_some());
            let mut engine = crate::player::engine::engine_after_reload_for_test();
            engine.native_epoch = epoch;
            engine.route_start = attempt;
            let mut pa = PlayerAdapter::new(unsafe { MainThread::assume() });
            pa.install(engine);
            SHARED.publish_video_raster(1280, 720);
            Self { ps, pa }
        }
        fn source(&mut self) -> Source {
            capture_source(&mut self.pa, &self.ps).expect("current ready source")
        }
    }
    impl Drop for Rig {
        fn drop(&mut self) {
            route::reset_player_control_for_test(&PlaybackSession::IDLE);
            SHARED.reset_session();
            TX.reset();
            ffi_host::reset_place_probe_for_test(false);
            ffi_host::force_clocksink_for_test(false);
        }
    }

    #[test]
    fn resize_and_fullscreen_keep_native_owner_position_pause_and_play_count() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::new();
        let source = rig.source();
        SHARED.playpos_ns.store(17_000_000_000, Relaxed);
        TX.commit_paused(true);
        let plays = ffi_host::play_calls_for_test();
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &source, inline()),
            Ok(Submission::Submitted)
        );
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &source, inline()),
            Ok(Submission::Unchanged)
        );
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &source, Target::Fullscreen),
            Ok(Submission::Submitted)
        );
        assert_eq!(rig.source(), source);
        assert_eq!(SHARED.playpos_ns.load(Relaxed), 17_000_000_000);
        assert!(crate::player::transport::paused());
        assert_eq!(ffi_host::play_calls_for_test(), plays);
    }

    #[test]
    fn source_raster_correction_keeps_inline_destination() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::new();
        let source = rig.source();
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &source, inline()),
            Ok(Submission::Submitted)
        );
        SHARED.publish_video_raster(1920, 1080);
        let (engine, mt) = rig.pa.split();
        let engine = engine.unwrap();
        assert_eq!(refresh_exported(mt, engine), Ok(Submission::Submitted));
        assert_eq!(engine.viewport.target, inline());
        assert_eq!(
            engine.viewport.submitted.unwrap().destination,
            Viewport {
                x: 48,
                y: 144,
                width: 704,
                height: 396
            }
        );
        assert_eq!(refresh_exported(mt, engine), Ok(Submission::Unchanged));
    }

    #[test]
    fn stale_native_generation_cannot_reset_new_owner() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::new();
        let old = rig.source();
        assert!(SHARED.retire_native_session(old.native_epoch));
        let epoch = SHARED.begin_native_session().unwrap();
        assert!(SHARED.mark_native_load_returned(epoch, None).is_some());
        rig.pa.engine().unwrap().native_epoch = epoch;
        let new = rig.source();
        assert_ne!(old, new);
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &new, inline()),
            Ok(Submission::Submitted)
        );
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &old, Target::Fullscreen),
            Err(Failure::StalePlayback)
        );
        assert_eq!(rig.pa.engine().unwrap().viewport.target, inline());
    }

    #[test]
    fn host_receives_outer_aperture_for_single_fit_and_matte() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::new();
        let source = rig.source();
        SHARED.publish_video_raster(1920, 800);
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &source, inline()),
            Ok(Submission::Submitted)
        );
        assert_eq!(
            rig.pa
                .engine()
                .unwrap()
                .viewport
                .submitted
                .unwrap()
                .destination,
            Viewport {
                x: 48,
                y: 144,
                width: 704,
                height: 396
            }
        );
    }

    #[test]
    fn pending_replacement_keeps_old_window_restore_but_blocks_new_inline() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::new();
        let source = rig.source();
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &source, inline()),
            Ok(Submission::Submitted)
        );
        let replacement = route::begin_route_start().expect("recoverable preparation");
        assert_eq!(
            route::route_start_status(source.attempt),
            RouteStartStatus::Stale
        );
        assert!(source_is_current(&mut rig.pa, &rig.ps, &source));
        assert_eq!(capture_source(&mut rig.pa, &rig.ps), None);
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &source, inline()),
            Err(Failure::NotReady)
        );
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &source, Target::Fullscreen),
            Ok(Submission::Submitted)
        );
        assert!(route::reject_route_start_preparation(replacement));
        assert_eq!(rig.source(), source);
        assert_eq!(rig.pa.engine().unwrap().viewport.target, Target::Fullscreen);
    }

    #[test]
    fn current_but_not_ready_owner_keeps_restore_obligation() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::new();
        let source = rig.source();
        rig.pa.engine().unwrap().stage = Stage::Loading;
        assert!(source_is_current(&mut rig.pa, &rig.ps, &source));
        assert_eq!(capture_source(&mut rig.pa, &rig.ps), None);
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &source, Target::Fullscreen),
            Err(Failure::NotReady)
        );
        rig.pa.engine().unwrap().stage = Stage::Streaming;
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &source, Target::Fullscreen),
            Ok(Submission::Submitted)
        );
    }

    #[test]
    fn wrong_logical_source_and_retired_epoch_are_rejected() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::new();
        let source = rig.source();
        assert_eq!(
            apply(&mut rig.pa, &PlaybackSession::IDLE, &source, inline()),
            Err(Failure::StalePlayback)
        );
        assert!(SHARED.retire_native_session(source.native_epoch));
        assert!(!source_is_current(&mut rig.pa, &rig.ps, &source));
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &source, inline()),
            Err(Failure::StalePlayback)
        );
    }

    #[test]
    fn invalid_geometry_preserves_last_accepted_target() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::new();
        let source = rig.source();
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &source, inline()),
            Ok(Submission::Submitted)
        );
        let bad = Target::Inline {
            rect: Viewport {
                x: 1900,
                y: 0,
                width: 704,
                height: 396,
            },
            canvas: (1920, 1080),
        };
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &source, bad),
            Err(Failure::InvalidGeometry)
        );
        assert_eq!(rig.pa.engine().unwrap().viewport.target, inline());
    }

    // These are native placement/ownership tests. The existing simulator seam seeds
    // a synthetic RGBA frame; no decoder, media fetch or physical display is exercised.
    fn fill(zoom: crate::player::video_geometry::Zoom) -> Transform {
        Transform {
            mode: crate::player::video_geometry::PictureMode::Fill,
            zoom,
        }
    }

    #[test]
    fn content_needs_a_picture_and_changes_neither_playback_nor_owner() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::new();
        let source = rig.source();
        {
            let mut absent = crate::player::viewport::picture_for_test();
            absent.clear();
            ffi_host::reset_place_probe_for_test(false);
            assert_eq!(
                apply_content(
                    &mut rig.pa,
                    &rig.ps,
                    &source,
                    fill(crate::player::video_geometry::Zoom::X125)
                ),
                Err(Failure::NotReady)
            );
            assert!(ffi_host::place_calls_for_test().is_empty());
            assert_eq!(
                content_state(&mut rig.pa, &rig.ps, &source),
                Err(Failure::NotReady)
            );
        }
        let _picture = crate::player::viewport::picture_for_test();
        ffi_host::reset_place_probe_for_test(false);
        SHARED.playpos_ns.store(17_000_000_000, Relaxed);
        TX.commit_paused(true);
        let plays = ffi_host::play_calls_for_test();
        assert_eq!(
            content_state(&mut rig.pa, &rig.ps, &source),
            Ok(ContentState {
                capability: ContentCapability::FitFillZoom,
                transform: Transform::default(),
            })
        );
        let chosen = fill(crate::player::video_geometry::Zoom::X125);
        assert_eq!(
            apply_content(&mut rig.pa, &rig.ps, &source, chosen),
            Ok(Submission::Submitted)
        );
        assert_eq!(
            content_state(&mut rig.pa, &rig.ps, &source)
                .unwrap()
                .transform,
            chosen
        );
        assert_eq!(rig.source(), source);
        assert_eq!(SHARED.playpos_ns.load(Relaxed), 17_000_000_000);
        assert!(crate::player::transport::paused());
        assert_eq!(ffi_host::play_calls_for_test(), plays);
        assert_eq!(ffi_host::place_calls_for_test().len(), 1);
    }

    #[test]
    fn content_rejects_stale_unready_and_replaced_sources_without_dispatch() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::new();
        let _picture = crate::player::viewport::picture_for_test();
        let source = rig.source();
        let chosen = fill(crate::player::video_geometry::Zoom::X150);
        ffi_host::reset_place_probe_for_test(false);
        assert_eq!(
            apply_content(&mut rig.pa, &PlaybackSession::IDLE, &source, chosen),
            Err(Failure::StalePlayback)
        );
        rig.pa.engine().unwrap().stage = Stage::Loading;
        assert_eq!(
            content_state(&mut rig.pa, &rig.ps, &source),
            Err(Failure::NotReady)
        );
        assert_eq!(
            apply_content(&mut rig.pa, &rig.ps, &source, chosen),
            Err(Failure::NotReady)
        );
        rig.pa.engine().unwrap().stage = Stage::Streaming;
        let replacement = route::begin_route_start().unwrap();
        assert!(source_is_current(&mut rig.pa, &rig.ps, &source));
        assert_eq!(
            content_state(&mut rig.pa, &rig.ps, &source),
            Err(Failure::NotReady)
        );
        assert_eq!(
            apply_content(&mut rig.pa, &rig.ps, &source, chosen),
            Err(Failure::NotReady)
        );
        assert!(ffi_host::place_calls_for_test().is_empty());
        // The established restoration path keeps its intentionally weaker policy.
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &source, Target::Fullscreen),
            Ok(Submission::Submitted)
        );
        assert!(route::reject_route_start_preparation(replacement));
        assert!(SHARED.retire_native_session(source.native_epoch));
        ffi_host::reset_place_probe_for_test(false);
        assert_eq!(
            apply_content(&mut rig.pa, &rig.ps, &source, chosen),
            Err(Failure::StalePlayback)
        );
        assert!(ffi_host::place_calls_for_test().is_empty());
    }

    #[test]
    fn refused_content_preserves_intent_and_actual_presentation() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::new();
        let _picture = crate::player::viewport::picture_for_test();
        let source = rig.source();
        ffi_host::reset_place_probe_for_test(false);
        let accepted = fill(crate::player::video_geometry::Zoom::X125);
        assert_eq!(
            apply_content(&mut rig.pa, &rig.ps, &source, accepted),
            Ok(Submission::Submitted)
        );
        let picture = crate::player::video_presentation(1920, 1080);
        assert_eq!(
            crate::player::video_presentation(960, 540),
            Viewport {
                x: -120,
                y: -67,
                width: 1200,
                height: 674,
            }
        );
        assert_eq!(
            crate::player::video_presentation(i32::MAX, i32::MAX),
            crate::player::video_viewport(i32::MAX, i32::MAX)
        );
        ffi_host::reset_place_probe_for_test(true);
        assert_eq!(
            apply_content(&mut rig.pa, &rig.ps, &source, Transform::default()),
            Err(Failure::NativeRejected)
        );
        assert_eq!(
            content_state(&mut rig.pa, &rig.ps, &source)
                .unwrap()
                .transform,
            accepted
        );
        assert_eq!(crate::player::video_presentation(1920, 1080), picture);
        assert_eq!(ffi_host::place_calls_for_test().len(), 1);
        ffi_host::reset_place_probe_for_test(false);
    }

    #[test]
    fn inline_uses_fit_and_restores_retained_fullscreen_content() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::new();
        let _picture = crate::player::viewport::picture_for_test();
        let source = rig.source();
        ffi_host::reset_place_probe_for_test(false);
        let chosen = fill(crate::player::video_geometry::Zoom::X175);
        assert_eq!(
            apply_content(&mut rig.pa, &rig.ps, &source, chosen),
            Ok(Submission::Submitted)
        );
        let fullscreen = crate::player::video_presentation(1920, 1080);
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &source, inline()),
            Ok(Submission::Submitted)
        );
        assert_eq!(
            crate::player::video_presentation(1920, 1080),
            Viewport {
                x: 48,
                y: 144,
                width: 704,
                height: 396
            }
        );
        ffi_host::reset_place_probe_for_test(false);
        assert_eq!(
            content_state(&mut rig.pa, &rig.ps, &source),
            Err(Failure::Unsupported)
        );
        assert_eq!(
            apply_content(&mut rig.pa, &rig.ps, &source, Transform::default()),
            Err(Failure::Unsupported)
        );
        assert!(ffi_host::place_calls_for_test().is_empty());
        assert_eq!(
            apply(&mut rig.pa, &rig.ps, &source, Target::Fullscreen),
            Ok(Submission::Submitted)
        );
        assert_eq!(crate::player::video_presentation(1920, 1080), fullscreen);
        assert_eq!(
            content_state(&mut rig.pa, &rig.ps, &source)
                .unwrap()
                .transform,
            chosen
        );
    }

    #[test]
    fn identical_pixels_still_commit_fill_for_later_aspect_changes() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::new();
        let _picture = crate::player::viewport::picture_for_test();
        let source = rig.source();
        ffi_host::reset_place_probe_for_test(false);
        assert_eq!(
            apply_content(&mut rig.pa, &rig.ps, &source, Transform::default()),
            Ok(Submission::Submitted)
        );
        assert_eq!(
            ffi_host::place_calls_for_test()[0].0,
            [1280, 720, 0, 0, 1920, 1080]
        );
        let chosen = fill(crate::player::video_geometry::Zoom::X100);
        assert_eq!(
            apply_content(&mut rig.pa, &rig.ps, &source, chosen),
            Ok(Submission::Unchanged)
        );
        assert_eq!(
            content_state(&mut rig.pa, &rig.ps, &source)
                .unwrap()
                .transform,
            chosen
        );
        assert_eq!(ffi_host::place_calls_for_test().len(), 1);
        SHARED.video_aspect.store(
            crate::player::video_geometry::Aspect::from_raster(4, 3)
                .unwrap()
                .pack(),
            Relaxed,
        );
        let (engine, mt) = rig.pa.split();
        assert_eq!(
            refresh_exported(mt, engine.unwrap()),
            Ok(Submission::Submitted)
        );
        assert_eq!(
            crate::player::video_presentation(1920, 1080),
            Viewport {
                x: 0,
                y: -180,
                width: 1920,
                height: 1440
            }
        );
    }

    #[test]
    fn failed_aspect_refresh_reports_actual_accepted_window_mapping() {
        let _serial = plx_base::testlock::serial();
        let mut rig = Rig::new();
        let _picture = crate::player::viewport::picture_for_test();
        let source = rig.source();
        ffi_host::reset_place_probe_for_test(false);
        assert_eq!(
            apply_content(
                &mut rig.pa,
                &rig.ps,
                &source,
                fill(crate::player::video_geometry::Zoom::X100)
            ),
            Ok(Submission::Submitted)
        );
        SHARED.video_aspect.store(
            crate::player::video_geometry::Aspect::from_raster(1, 1)
                .unwrap()
                .pack(),
            Relaxed,
        );
        ffi_host::reset_place_probe_for_test(true);
        let (engine, mt) = rig.pa.split();
        let engine = engine.unwrap();
        assert_eq!(refresh_exported(mt, engine), Err(Failure::NativeRejected));
        assert_eq!(refresh_exported(mt, engine), Err(Failure::NativeRejected));
        assert_eq!(ffi_host::place_calls_for_test().len(), 1);
        // The accepted window is still the original full aperture. The renderer's
        // current PAR fits within that window; a requested Fill square was refused.
        assert_eq!(
            crate::player::video_presentation(1920, 1080),
            Viewport {
                x: 420,
                y: 0,
                width: 1080,
                height: 1080
            }
        );
        assert_eq!(
            crate::player::video_presentation(960, 540),
            Viewport {
                x: 210,
                y: 0,
                width: 540,
                height: 540
            }
        );
        ffi_host::reset_place_probe_for_test(false);
    }

    #[test]
    fn new_engine_and_window_restore_default_content() {
        let _serial = plx_base::testlock::serial();
        let _picture = crate::player::viewport::picture_for_test();
        let old = {
            let mut rig = Rig::new();
            let source = rig.source();
            ffi_host::reset_place_probe_for_test(false);
            assert_eq!(
                apply_content(
                    &mut rig.pa,
                    &rig.ps,
                    &source,
                    fill(crate::player::video_geometry::Zoom::X150)
                ),
                Ok(Submission::Submitted)
            );
            source
        };
        // A new existing Rig constructs a new inert Engine through the real test
        // constructor. Window creation uses the native placement reset; this is
        // not a full decoder reload or a proof of app-level intent restoration.
        let mut rig = Rig::new();
        crate::player::sim_video::create_window();
        let source = rig.source();
        assert_ne!(source, old);
        assert_eq!(
            apply_content(&mut rig.pa, &rig.ps, &old, Transform::default()),
            Err(Failure::StalePlayback)
        );
        assert_eq!(
            content_state(&mut rig.pa, &rig.ps, &source)
                .unwrap()
                .transform,
            Transform::default()
        );
        assert_eq!(
            crate::player::video_presentation(1920, 1080),
            crate::player::video_viewport(1920, 1080)
        );
        assert_eq!(
            apply_content(&mut rig.pa, &rig.ps, &source, Transform::default()),
            Ok(Submission::Submitted)
        );
    }
}
