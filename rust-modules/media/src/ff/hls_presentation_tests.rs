use super::*;

#[test]
fn master_preserves_default_and_separate_rendition_identity_without_exposing_urls() {
    let text = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"sound\",NAME=\"English\",LANGUAGE=\"en\",DEFAULT=YES,URI=\"en.m3u8?token=secret\"\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"sound\",NAME=\"Spanish\",LANGUAGE=\"es\",DEFAULT=NO,URI=\"es.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=100000,AUDIO=\"sound\"\nvideo.m3u8\n";
    let master = parse_master("https://example.invalid/path/master.m3u8", text).unwrap();
    assert_eq!(master.renditions.len(), 2);
    assert_eq!(master.default, RenditionId(1));
    assert_ne!(master.renditions[0].id, master.renditions[1].id);
    assert_eq!(master.renditions[0].name, "English");
    assert!(!format!("{:?}", master.renditions[0].id).contains("secret"));
}

#[test]
fn vod_uses_absolute_anchors_not_media_sequence_alignment() {
    let make = |sequence| format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:{sequence}\n#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:00Z\n#EXTINF:1,\na.ts\n#EXTINF:1,\nb.ts\n#EXT-X-ENDLIST\n");
    let a = parse_vod("https://example.invalid/video.m3u8", &make(77)).unwrap();
    let b = parse_vod("https://example.invalid/audio.m3u8", &make(900)).unwrap();
    assert_eq!(a.segments[0].pdt_ns, b.segments[0].pdt_ns);
    assert_ne!(a.segments[0].segment.sequence, b.segments[0].segment.sequence);
    assert_eq!(a.segments[1].pdt_ns - a.segments[0].pdt_ns, 1_000_000_000);
    assert_eq!(a.segments[0].explicit_precision_ns, Some(1_000_000_000));
    assert_eq!(a.segments[1].explicit_precision_ns, None);
}

#[test]
fn missing_or_discontinuous_anchors_fail_closed() {
    let text = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:00Z\n#EXTINF:1,\na.ts\n#EXT-X-ENDLIST\n";
    assert!(parse_vod("https://example.invalid/v.m3u8", &text.replace("#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:00Z\n", "")).is_err());
    assert!(parse_vod("https://example.invalid/v.m3u8", &text.replace("#EXTINF", "#EXT-X-DISCONTINUITY\n#EXTINF")).is_err());
    assert!(parse_vod("https://example.invalid/v.m3u8", &text.replace("#EXT-X-ENDLIST", "")).is_err());
    // A later explicit PDT is retained for packet-clock validation, not
    // rejected against an accumulated and possibly rounded EXTINF estimate.
    let later = parse_vod("https://example.invalid/v.m3u8", &text.replace("#EXT-X-ENDLIST", "#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:04.000Z\n#EXTINF:1,\nb.ts\n#EXT-X-ENDLIST")).unwrap();
    assert_eq!(later.segments[1].explicit_precision_ns, Some(1_000_000));
}


fn rendition_rows(group: &str, count: usize) -> String {
    (0..count).map(|i| format!(
        "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"{group}\",NAME=\"voice-{i}\",LANGUAGE=\"en\",DEFAULT={},URI=\"{i}.m3u8\"\n",
        if i + 1 == count { "YES" } else { "NO" },
    )).collect()
}
fn rendition_master(rows: &str, group: &str) -> String {
    format!("#EXTM3U\n{rows}#EXT-X-STREAM-INF:BANDWIDTH=100000,AUDIO=\"{group}\"\nvideo.m3u8\n")
}

#[test]
fn master_retains_every_selected_rendition_beyond_sixteen() {
    for count in [17, 257] {
        let text = rendition_master(&rendition_rows("sound", count), "sound");
        let master = parse_master("https://example.invalid/master.m3u8", &text)
            .expect("bounded master must retain all selected audio rows");
        assert_eq!(master.renditions.len(), count);
        assert_eq!(master.default, RenditionId(count as u32));
        for (i, row) in master.renditions.iter().enumerate() {
            assert_eq!(row.id, RenditionId(i as u32 + 1));
            assert_eq!(row.name, format!("voice-{i}"));
        }
        let mut playback = HlsPlayback::fixture_for_test();
        std::sync::Arc::get_mut(&mut playback.presentation).unwrap().renditions = master.renditions;
        assert_eq!(playback.rows().count(), count);
        assert!(playback.has_rendition(master.default));
        assert_eq!(playback.rows().last().unwrap().0, master.default);
    }
}

#[test]
fn unrelated_renditions_do_not_hide_the_selected_group_or_change_its_ids() {
    let rows = rendition_rows("other", 33) + &rendition_rows("sound", 17);
    let text = rendition_master(&rows, "sound");
    let master = parse_master("https://example.invalid/master.m3u8", &text)
        .expect("unrelated audio groups cannot consume a row allowance");
    assert_eq!(master.renditions.len(), 17);
    assert_eq!(master.renditions[0].id, RenditionId(34));
    assert_eq!(master.default, RenditionId(50));
    assert_eq!(master.renditions.last().unwrap().name, "voice-16");
}

#[test]
fn late_media_keeps_default_and_lowest_variant_first_tie_semantics() {
    let text = format!(
        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=200000,AUDIO=\"other\"\nother.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=100000,AUDIO=\"sound\"\nfirst.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=100000,AUDIO=\"other\"\nsecond.m3u8\n{}{}",
        rendition_rows("other", 2), rendition_rows("sound", 17),
    );
    let master = parse_master("https://example.invalid/master.m3u8", &text)
        .expect("MEDIA declarations may follow the selected variant");
    assert_eq!(master.renditions.len(), 17);
    assert_eq!(master.default, RenditionId(19));
    assert_eq!(master.variant_url, "https://example.invalid:443/first.m3u8");
    let no_default = text.replace("DEFAULT=YES", "DEFAULT=NO");
    let master = parse_master("https://example.invalid/master.m3u8", &no_default).unwrap();
    assert_eq!(master.default, RenditionId(3));
}

#[test]
fn long_master_base_is_not_duplicated_into_retained_rendition_urls() {
    let base = format!("https://example.invalid/{}/master.m3u8", "x".repeat(7000));
    let text = rendition_master(&rendition_rows("sound", 8), "sound");
    let master = parse_master(&base, &text).unwrap();
    for (i, row) in master.renditions.iter().enumerate() {
        assert_eq!(row.url, format!("{i}.m3u8"), "retain the validated URI reference, not an expanded base per row");
        assert_eq!(super::external_hls::uri(&base, &row.url).unwrap(),
            format!("https://example.invalid:443/{}/{i}.m3u8", "x".repeat(7000)));
    }
    let retained_text: usize = master.renditions.iter()
        .map(|row| row.name.len() + row.language.len() + row.url.len()).sum();
    assert!(retained_text <= text.len(), "owned rendition text stays within its bounded input text");
}

#[test]
fn master_still_rejects_bad_or_duplicate_rows_in_unselected_groups() {
    let rows = rendition_rows("other", 2) + &rendition_rows("sound", 2);
    let text = rendition_master(&rows, "sound");
    assert!(parse_master("https://example.invalid/master.m3u8", &text).is_ok(), "same NAME across groups remains valid");
    let two_other_defaults = text.replacen("DEFAULT=NO", "DEFAULT=YES", 1);
    assert!(parse_master("https://example.invalid/master.m3u8", &two_other_defaults).is_ok(), "DEFAULT ambiguity belongs only to the selected group");
    let duplicate = text.clone() + &rendition_rows("other", 1);
    assert_eq!(parse_master("https://example.invalid/master.m3u8", &duplicate).err(),
        Some("Duplicate HLS audio rendition identity"));
    let invalid = text.clone() + "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"other\",NAME=\"late\",URI=\"http://example.invalid/plain.m3u8\"\n";
    assert_eq!(parse_master("https://example.invalid/master.m3u8", &invalid).err(),
        Some("HLS resource would leave HTTPS"));
    let missing = text.clone() + "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"other\",NAME=\"late\"\n";
    assert_eq!(parse_master("https://example.invalid/master.m3u8", &missing).err(),
        Some("Embedded audio group is outside this route"));
    let malformed = text + "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"other\",NAME=\"late\",NAME=\"again\",URI=\"late.m3u8\"\n";
    assert_eq!(parse_master("https://example.invalid/master.m3u8", &malformed).err(), Some("Duplicate HLS attribute"));
}

#[test]
fn master_keeps_byte_line_and_selected_default_guards() {
    let text = rendition_master(&rendition_rows("sound", 2), "sound");
    let duplicate_default = text.replace("DEFAULT=NO", "DEFAULT=YES");
    assert_eq!(parse_master("https://example.invalid/master.m3u8", &duplicate_default).err(),
        Some("Ambiguous default HLS audio"));
    let over_body = format!("#EXTM3U\n{}", " ".repeat(1024 * 1024));
    assert_eq!(parse_master("https://example.invalid/master.m3u8", &over_body).err(),
        Some("HLS playlist exceeds size limit"));
    let over_line = format!("#EXTM3U\n#{}\n", "x".repeat(64 * 1024));
    assert_eq!(parse_master("https://example.invalid/master.m3u8", &over_line).err(),
        Some("HLS playlist line exceeds limit"));
    let at_line = text.clone() + "#" + &"x".repeat(64 * 1024 - 1) + "\n";
    assert!(parse_master("https://example.invalid/master.m3u8", &at_line).is_ok());
    let mut at_body = text;
    while at_body.len() + 2 <= 1024 * 1024 { at_body.push_str("#\n"); }
    if at_body.len() < 1024 * 1024 { at_body.push('#'); }
    assert_eq!(at_body.len(), 1024 * 1024);
    assert!(parse_master("https://example.invalid/master.m3u8", &at_body).is_ok());
}


#[test]
fn large_master_still_validates_late_unselected_descriptors() {
    let rows = rendition_rows("sound", 2) + &rendition_rows("other", 33);
    let text = rendition_master(&rows, "sound");
    assert!(parse_master("https://example.invalid/master.m3u8", &text).is_ok(), "large valid unselected group is allowed");
    for (row, expected) in [
        ("TYPE=AUDIO,GROUP-ID=\"other\",NAME=\"voice-32\",URI=\"dup.m3u8\"", "Duplicate HLS audio rendition identity"),
        ("TYPE=AUDIO,GROUP-ID=\"other\",NAME=\"late\",URI=\"http://example.invalid/plain.m3u8\"", "HLS resource would leave HTTPS"),
        ("TYPE=AUDIO,GROUP-ID=\"other\",NAME=\"late\"", "Embedded audio group is outside this route"),
        ("TYPE=AUDIO,GROUP-ID=\"other\",URI=\"late.m3u8\"", "Missing HLS audio name"),
        ("TYPE=AUDIO,NAME=\"late\",URI=\"late.m3u8\"", "Missing HLS audio group"),
        ("TYPE=AUDIO,GROUP-ID=\"other\",NAME=\"late\",NAME=\"again\",URI=\"late.m3u8\"", "Duplicate HLS attribute"),
    ] {
        let bad = format!("{text}#EXT-X-MEDIA:{row}\n");
        assert_eq!(parse_master("https://example.invalid/master.m3u8", &bad).err(), Some(expected));
    }
}


#[test]
fn subtitle_associations_do_not_change_the_selected_audio_presentation() {
    let base = "https://example.invalid/master.m3u8";
    let plain = rendition_master(&rendition_rows("sound", 2), "sound");
    let expected = parse_master(base, &plain).unwrap();
    let declaration = "#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"English\",LANGUAGE=\"en\",DEFAULT=YES,AUTOSELECT=YES,FORCED=YES,URI=\"unselected-captions.m3u8\"\n";
    for association in [
        plain.replace("AUDIO=\"sound\"", "AUDIO=\"sound\",SUBTITLES=\"subs\""),
        plain.clone() + "#EXT-X-STREAM-INF:BANDWIDTH=200000,AUDIO=\"sound\",SUBTITLES=\"subs\"\nother.m3u8\n",
    ] {
        let text = association.replacen("#EXTM3U\n", &(String::from("#EXTM3U\n") + declaration), 1);
        let got = parse_master(base, &text).expect("unselected subtitle group must not block audio/video");
        assert_eq!(got.variant_url, expected.variant_url);
        assert_eq!(got.default, expected.default);
        let rows = |master: &Master| master.renditions.iter().map(|row|
            (row.id, row.name.clone(), row.language.clone(), row.url.clone(), row.default))
            .collect::<Vec<_>>();
        assert_eq!(rows(&got), rows(&expected), "subtitles must not add rows or renumber audio");
        assert!(got.renditions.iter().all(|row| !row.url.contains("captions")));
    }
}

#[test]
fn subtitle_presence_does_not_bypass_video_audio_or_attribute_guards() {
    let base = "https://example.invalid/master.m3u8";
    let text = rendition_master(&rendition_rows("sound", 2), "sound")
        .replace("AUDIO=\"sound\"", "AUDIO=\"sound\",SUBTITLES=\"subs\"");
    assert_eq!(parse_master(base, &text.replace("SUBTITLES=\"subs\"", "SUBTITLES=\"subs\",VIDEO=\"video\"")).err(),
        Some("Additional HLS rendition groups are outside this route"));
    assert_eq!(parse_master(base, &text.replace("AUDIO=\"sound\",", "")).err(),
        Some("HLS variant has no audio group"));
    assert_eq!(parse_master(base, &text.replace("AUDIO=\"sound\"", "AUDIO=\"missing\"")).err(),
        Some("HLS audio group has no external rendition"));
    let malformed = text + "#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"one\",NAME=\"two\",URI=\"captions.m3u8\"\n";
    assert_eq!(parse_master(base, &malformed).err(), Some("Duplicate HLS attribute"));
}
