use super::*;
fn source(mode: CatchupMode, template: &str) -> CatchupSource {
    CatchupSource {
        mode,
        source: template.into(),
        retention_seconds: 86400,
        correction_seconds: 0,
    }
}
#[test]
fn exact_template_interval_and_existing_query_are_preserved() {
    let interval = ArchiveInterval {
        start: 1000,
        end: 1600,
    };
    let target = plan(
        &source(
            CatchupMode::Append,
            "?start=${start}&end={utcend}&duration={duration}",
        ),
        "https://video.invalid/live?token=fixture",
        interval,
        2000,
    )
    .unwrap();
    assert_eq!(
        target,
        ArchiveTarget {
            url: "https://video.invalid:443/live?token=fixture&start=1000&end=1600&duration=600"
                .into(),
            interval
        }
    );
}
#[test]
fn expired_future_empty_unknown_and_overflow_are_refused() {
    let mut s = source(
        CatchupMode::Template,
        "https://video.invalid/{utc}/{duration}",
    );
    for i in [
        ArchiveInterval { start: 0, end: 1 },
        ArchiveInterval {
            start: 99999,
            end: 100001,
        },
        ArchiveInterval {
            start: 99999,
            end: 99999,
        },
    ] {
        assert!(plan(&s, "https://video.invalid/live", i, 100000).is_err());
    }
    s.source = "https://video.invalid/{unknown}".into();
    assert!(plan(
        &s,
        "https://video.invalid/live",
        ArchiveInterval {
            start: 99998,
            end: 99999
        },
        100000
    )
    .is_err());
    s.source = "https://video.invalid/{utc}".into();
    s.correction_seconds = i64::MAX;
    assert!(plan(
        &s,
        "https://video.invalid/live",
        ArchiveInterval {
            start: 99998,
            end: 99999
        },
        100000
    )
    .is_err());
}
#[test]
fn flussonic_preserves_query_without_changing_requested_duration() {
    let target = plan(
        &source(CatchupMode::Flussonic, ""),
        "https://video.invalid/live/mpegts?x=1&x=2",
        ArchiveInterval {
            start: 1000,
            end: 1100,
        },
        2000,
    )
    .unwrap();
    assert!(
        matches!(target, ArchiveTarget { url, .. } if url == "https://video.invalid:443/live/archive-1000-100.ts?x=1&x=2")
    );
}
#[test]
fn flussonic_recent_window_boundary_uses_rolling_endpoint_only_below_ten_minutes() {
    for (age, recent) in [(599, true), (600, false), (601, false)] {
        let start = 10000 - age;
        let interval = ArchiveInterval {
            start,
            end: start + 100,
        };
        for (name, rolling, archived) in [
            (
                "mpegts",
                format!("timeshift_abs-{start}.ts"),
                format!("archive-{start}-100.ts"),
            ),
            (
                "index.m3u8",
                format!("timeshift_abs-{start}.m3u8"),
                format!("archive-{start}-100.m3u8"),
            ),
            (
                "video.m3u8",
                format!("video-timeshift_abs-{start}.m3u8"),
                format!("video-{start}-100.m3u8"),
            ),
        ] {
            let target = plan(
                &source(CatchupMode::Flussonic, ""),
                &format!("https://video.invalid/live/{name}?x=1&x=2"),
                interval,
                10000,
            )
            .unwrap();
            let file = if recent { rolling } else { archived };
            assert_eq!(
                target,
                ArchiveTarget {
                    url: format!("https://video.invalid:443/live/{file}?x=1&x=2"),
                    interval
                }
            );
        }
    }
}

#[test]
fn shift_uses_existing_query_and_original_requested_interval() {
    let interval = ArchiveInterval {
        start: 1000,
        end: 1100,
    };
    let target = plan(
        &source(CatchupMode::Shift, ""),
        "https://video.invalid/live?token=fixture",
        interval,
        2000,
    )
    .unwrap();
    assert_eq!(
        target.url,
        "https://video.invalid:443/live?token=fixture&utc=1000&lutc=2000"
    );
    assert_eq!(target.interval, interval);
}

#[test]
fn unsafe_or_incomplete_sources_and_negative_clock_are_rejected() {
    let interval = ArchiveInterval {
        start: 1000,
        end: 1100,
    };
    for (mode, value) in [
        (CatchupMode::Template, "file:///private/{utc}"),
        (CatchupMode::Append, "https://other.invalid/{utc}"),
        (CatchupMode::Template, "https://video.invalid/{unknown}"),
        (CatchupMode::Template, "https://video.invalid/\n{utc}"),
    ] {
        assert!(plan(
            &source(mode, value),
            "https://video.invalid/live",
            interval,
            2000
        )
        .is_err());
    }
    assert!(plan(
        &source(CatchupMode::Shift, ""),
        "https://video.invalid/live",
        interval,
        -1
    )
    .is_err());
}

#[test]
fn diagnostics_never_print_archive_credentials() {
    let s = source(
        CatchupMode::Template,
        "https://video.invalid/{utc}?token=private-value",
    );
    assert!(!format!("{s:?}").contains("private-value"));
    let target = plan(
        &s,
        "https://video.invalid/live",
        ArchiveInterval {
            start: 1000,
            end: 1100,
        },
        2000,
    )
    .unwrap();
    assert!(!format!("{target:?}").contains("private-value"));
}
