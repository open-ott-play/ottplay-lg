use super::*;

fn frame(rate_index: u8, crc: bool) -> Vec<u8> {
    let mut data = super::super::adts_header(rate_index, 2, 11).to_vec();
    data.extend_from_slice(&[0; 11]);
    if crc {
        data[1] &= !1;
        data.splice(7..7, [0, 0]);
        let len = data.len();
        data[3] = (data[3] & 0xfc) | ((len >> 11) as u8 & 3);
        data[4] = (len >> 3) as u8;
        data[5] = (data[5] & 0x1f) | ((len as u8 & 7) << 5);
    }
    data
}

fn packet(pts: Option<i64>, dts: Option<i64>, duration: Option<i64>, rate_index: u8)
    -> AudioPacketTiming
{
    AudioPacketTiming::new(pts, dts, duration, (1, 90_000), &frame(rate_index, false))
}

#[test]
fn complete_adts_frames_share_one_checked_duration_ratio() {
    for crc in [false, true] {
        let data = frame(3, crc);
        let ratio = adts_timing::duration(&data).unwrap();
        assert_eq!((ratio.samples, ratio.rate), (1_024, 48_000));
        assert_eq!(super::super::adts_duration_ns(&data), Some(21_333_333));
        let joined = [data.as_slice(), data.as_slice()].concat();
        let ratio = adts_timing::duration(&joined).unwrap();
        assert_eq!((ratio.samples, ratio.rate), (2_048, 48_000));
    }
}

#[test]
fn adts_duration_requires_complete_supported_and_consistent_frames() {
    let good = frame(3, false);
    let mut cases = vec![vec![], good[..6].to_vec(), good[..good.len()-1].to_vec()];
    let mut trailing = good.clone(); trailing.push(0); cases.push(trailing);
    for (index, value) in [(0, 0), (1, 0xf3), (2, 0x0c), (2, 0x7c), (6, 0xff)] {
        let mut changed = good.clone(); changed[index] = value; cases.push(changed);
    }
    let mut short = good.clone(); short[3] &= 0xfc; short[4] = 0; short[5] = 7 << 5;
    cases.push(short);
    let mut crc_short = frame(3, true); crc_short.truncate(8); cases.push(crc_short);
    cases.push([good.as_slice(), frame(4, false).as_slice()].concat());
    let mut channels = good.clone(); channels[3] ^= 0x40;
    cases.push([good.as_slice(), channels.as_slice()].concat());
    for data in cases {
        assert!(adts_timing::duration(&data).and_then(adts_timing::Duration::for_rendition).is_none(),
            "accepted unsupported rendition ADTS: {data:?}");
    }
}

#[test]
fn anchored_adts_recovers_missing_pts_duration_and_only_missing_dts() {
    let raw = [packet(None, None, None, 3), packet(Some(900_001), Some(899_999), None, 3),
        packet(None, None, None, 3)];
    let actual = resolve_audio_timing(&raw).unwrap();
    assert_eq!(actual.iter().map(|s| s.pts).collect::<Vec<_>>(), [898_081, 900_001, 901_921]);
    assert_eq!(actual.iter().map(|s| s.dts).collect::<Vec<_>>(),
        [Some(898_081), Some(899_999), Some(901_921)]);
    assert!(actual.iter().all(|s| s.duration == Some(1_920) && s.time_base == (1, 90_000)));
    assert!(resolve_audio_timing(&[packet(None, Some(900_001), None, 3)]).is_err());
}

#[test]
fn long_44100_adts_run_uses_cumulative_endpoints_without_drift() {
    let mut raw = vec![packet(None, None, None, 4); 1_471];
    raw[0].pts = Some(900_001);
    let actual = resolve_audio_timing(&raw).unwrap();
    // 1470 * 1024 * 90000 / 44100 is exactly 3,072,000 ticks.
    assert_eq!(actual[1_470].pts, 3_972_001);
    assert_eq!(actual[..1_470].iter().map(|s| s.duration.unwrap()).sum::<i64>(), 3_072_000);
    assert!(actual.iter().any(|s| s.duration == Some(2_089)));
    assert!(actual.iter().any(|s| s.duration == Some(2_090)));
    assert!(actual.windows(2).all(|w| w[0].pts + w[0].duration.unwrap() == w[1].pts));
    for (index, sample) in actual.iter().enumerate() {
        let numerator = index as i128 * 1_024 * 90_000;
        let expected = (numerator + 44_100 / 2) / 44_100;
        assert_eq!(i128::from(sample.pts), 900_001 + expected);
    }
}

#[test]
fn leading_44100_adts_run_backfills_from_real_anchor_without_drift() {
    let mut raw = vec![packet(None, None, None, 4); 148];
    raw[147].pts = Some(900_001);
    let actual = resolve_audio_timing(&raw).unwrap();
    assert_eq!(actual[0].pts, 592_801);
    assert_eq!(actual[147].pts, 900_001);
    assert!(actual.windows(2).all(|w| w[0].pts + w[0].duration.unwrap() == w[1].pts));
}

#[test]
fn supplied_quantized_duration_is_preserved_without_driving_adts_phase() {
    let mut raw = vec![packet(None, None, Some(2_089), 4); 148];
    raw[0].pts = Some(900_001);
    raw[1].duration = Some(2_090);
    let actual = resolve_audio_timing(&raw).unwrap();
    assert_eq!(actual[147].pts, 1_207_201);
    assert_eq!(actual[0].duration, Some(2_089));
    assert_eq!(actual[1].duration, Some(2_090));
    raw[1].duration = Some(2_091);
    assert!(resolve_audio_timing(&raw).is_err());
}

#[test]
fn explicit_anchor_gaps_and_resets_are_not_fitted_to_adts_phase() {
    for anchor in [901_000, 950_000] {
        let raw = [packet(Some(900_001), None, None, 4), packet(None, None, None, 4),
            packet(Some(anchor), Some(anchor - 7), None, 4), packet(None, None, None, 4)];
        let actual = resolve_audio_timing(&raw).unwrap();
        assert_eq!(actual[1].pts, 902_091);
        assert_eq!(actual[1].duration, Some(2_090));
        assert_eq!(actual[2].pts, anchor);
        assert_eq!(actual[2].dts, Some(anchor - 7));
        assert_eq!(actual[3].pts, anchor + 2_090);
    }
}

#[test]
fn adts_recovery_rejects_format_changes_and_timestamp_overflow() {
    assert!(resolve_audio_timing(&[packet(Some(900_001), None, None, 3),
        packet(None, None, None, 4)]).is_err());
    assert!(resolve_audio_timing(&[packet(Some(i64::MAX), None, None, 3)]).is_err());
    assert!(resolve_audio_timing(&[packet(None, None, None, 3),
        packet(Some(i64::MIN + 1), None, None, 3)]).is_err());
    let unsupported = AudioPacketTiming::new(None, None, None, (1, 90_000), &[0xff; 20]);
    assert!(resolve_audio_timing(&[packet(Some(900_001), None, None, 3), unsupported,
        packet(None, None, None, 3)]).is_err());
}

#[test]
fn ordinary_adts_duration_preserves_all_profiles_and_multiple_raw_blocks() {
    // Structural header fixtures exercise transport duration only, not decoding.
    for profile in 0..=3 {
        for blocks in 1..=4 {
            let mut data = frame(3, false);
            data[2] = (data[2] & 0x3f) | (profile << 6);
            data[6] = (data[6] & 0xfc) | (blocks - 1);
            let ratio = adts_timing::duration(&data).unwrap();
            assert_eq!(ratio.samples, i64::from(blocks) * 1_024);
            assert_eq!(super::super::adts_duration_ns(&data),
                Some(i64::from(blocks) * 1_024 * 1_000_000_000 / 48_000));
            assert_eq!(ratio.for_rendition().is_some(), profile == 1 && blocks == 1);
            // The ordinary estimate has always used only the first seven
            // header bytes; stricter framing must not change that path.
            let expected = super::super::adts_duration_ns(&data);
            assert_eq!(super::super::adts_duration_ns(&data[..7]), expected);
            let mut trailing = data.clone(); trailing.push(0);
            assert_eq!(super::super::adts_duration_ns(&trailing), expected);
            assert_eq!(super::super::adts_duration_ns(&[data.as_slice(), data.as_slice()].concat()), expected);
            let mut bad_length = data.clone();
            bad_length[3] &= 0xfc; bad_length[4] = 0; bad_length[5] &= 0x1f;
            assert_eq!(super::super::adts_duration_ns(&bad_length), expected);
        }
    }
}
