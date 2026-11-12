//! The crop an art tile samples (`art_uv`): every picture keeps its own aspect in its tile.

use super::*;
use crate::card_row::RowStyle;

/// The crop never reads which server a picture came from, so any raw id will do.
const SRC: u16 = 0;

/// The cast row's circle, at rest and popped. The shape is what matters; the position is not.
fn cast_circle(scale: f32) -> Rect {
    Rect::new(96.0, 700.0, RowStyle::CAST.w, RowStyle::CAST.h).scaled(scale)
}

fn headshot(key: &str) -> Art<'_> {
    Art::Person { sid: SRC, key, res: (300, 300) }
}

/// Texels per drawn pixel on each axis — equal ⇔ an even scale, i.e. no distortion.
fn texel_scale(uv: [f32; 4], tw: f32, th: f32, r: Rect) -> (f32, f32) {
    (uv[2] * tw / r.w, uv[3] * th / r.h)
}

/// **The reported bug.** A cast headshot is requested at 300×300 with `minSize=1`, which COVERS the
/// box, so a portrait photo decodes at 300×450. The tile used to sample the whole texture into the
/// 190×190 circle — a vertical squash to two-thirds. It must now sample an even-scaled window that
/// keeps the full width and rides high on the photo, at rest and through the focus pop.
#[test]
fn a_portrait_headshot_on_the_cast_row_is_cropped_not_squashed() {
    let (tw, th) = (300.0, 450.0);
    for scale in [1.0, RowStyle::CAST.focus_scale] {
        let r = cast_circle(scale);
        let uv = art_uv(&headshot("/library/metadata/1/thumb"), tw, th, r);
        let (sx, sy) = texel_scale(uv, tw, th, r);
        assert!((sx - sy).abs() < 1e-4, "cast headshot drawn with uneven scale {sx} x {sy} (uv {uv:?})");
        assert_eq!(uv[2], 1.0, "the full width of a portrait survives");
        assert!(
            uv[1] < (1.0 - uv[3]) * 0.5,
            "a headshot's crop must keep more of the top than the bottom: {uv:?}"
        );
    }
}

/// Every surface that draws a person's photo draws it through `Art::Person` — the cast row, the
/// person page's portrait and Search's person results — so all of them get the headshot crop.
#[test]
fn every_person_photo_takes_the_headshot_crop_and_other_art_is_even() {
    assert_eq!(art_crop(&headshot("k")), crate::Crop::Headshot);
    assert_eq!(art_crop(&Art::Poster(None)), crate::Crop::Centre);
    assert_eq!(art_crop(&Art::Still(None)), crate::Crop::Centre);
    assert_eq!(
        art_crop(&Art::Thumb { sid: SRC, key: "k", res: (300, 300) }),
        crate::Crop::Centre
    );
}

/// Art already at its tile's aspect is sampled whole, so posters in poster tiles and 16:9 stills in
/// the episode filmstrip draw exactly as before; only art of ANOTHER aspect changes, and it
/// changes from squashed to cropped — here the poster a landscape tile falls back to.
#[test]
fn art_at_its_tiles_aspect_is_untouched_and_a_poster_in_a_landscape_tile_is_cropped() {
    let poster = Rect::new(0.0, 0.0, 250.0, 375.0);
    assert_eq!(art_uv(&Art::Poster(None), 250.0, 375.0, poster), plx_gfx::gfx::UV_FULL);
    let still = Rect::new(0.0, 0.0, RowStyle::EPISODE.w, RowStyle::EPISODE.h);
    let uv = art_uv(&Art::Still(None), 250.0, 375.0, still);
    let (sx, sy) = texel_scale(uv, 250.0, 375.0, still);
    assert!((sx - sy).abs() < 1e-4, "poster fallback drawn with uneven scale {sx} x {sy}");
    assert!(uv[3] < 1.0 && uv[2] == 1.0, "a tall poster in a wide tile loses top and bottom: {uv:?}");
    // an undecoded texture answers the whole window, never a NaN
    assert_eq!(art_uv(&headshot("k"), 0.0, 0.0, cast_circle(1.0)), plx_gfx::gfx::UV_FULL);
}

/// Channel row and footer slots must show a whole emblem or wordmark, unlike
/// poster cards. Exercise the same decoded sizes for HTTP, Plex and local art;
/// the fit must preserve aspect, stay centred and leave no sampled edge behind.
#[test]
fn channel_identity_art_fits_both_row_and_footer_without_cropping() {
    let key = plx_machine::machine::PosterKey(701);
    let arts = [Art::Http { url: "https://fixture.invalid/channel.png", res: (96, 64) },
        Art::Thumb { sid: SRC, key: "/channel", res: (240, 240) },
        Art::Local { key: &key }];
    for slot in [Rect::new(81.0, 61.0, 48.0, 32.0), Rect::new(32.0, 900.0, 120.0, 120.0)] {
        for (w, h) in [(1920.0, 240.0), (240.0, 1920.0), (512.0, 512.0), (96.0, 64.0)] {
            let frame = contained_art_frame(slot, ArtImage { tex: 1, w, h, shown: 1.0 });
            assert!(frame.x >= slot.x && frame.y >= slot.y);
            assert!(frame.x + frame.w <= slot.x + slot.w + 1e-4);
            assert!(frame.y + frame.h <= slot.y + slot.h + 1e-4);
            assert!((frame.x + frame.w * 0.5 - (slot.x + slot.w * 0.5)).abs() < 1e-4);
            assert!((frame.y + frame.h * 0.5 - (slot.y + slot.h * 0.5)).abs() < 1e-4);
            assert!((frame.w / w - frame.h / h).abs() < 1e-5);
            for art in &arts {
                let uv = art_uv(art, w, h, frame);
                for (actual, expected) in uv.into_iter().zip(plx_gfx::gfx::UV_FULL) {
                    assert!((actual - expected).abs() < 1e-5,
                        "logo loses an edge: source={w}×{h}, slot={slot:?}, uv={uv:?}");
                }
            }
        }
    }
}

#[test]
fn unresolved_or_invalid_identity_art_keeps_its_reserved_slot() {
    let slot = Rect::new(10.0, 20.0, 48.0, 32.0);
    for (w, h) in [(0.0, 0.0), (10.0, 0.0), (-1.0, 20.0), (f32::NAN, 20.0), (20.0, f32::INFINITY)] {
        assert_eq!(contained_art_frame(slot, ArtImage { tex: 0, w, h, shown: 1.0 }), slot);
    }
}

/// Draw through the actual cache and card renderer, rather than only asserting
/// fit maths: a regression at either call boundary must not silently re-crop.
#[test]
fn resident_channel_logos_reach_renderer_whole_and_inside_the_slot() {
    use plx_machine::machine::{PosterKey, PresentHandle};
    use crate::tex::{Source, Warm, Uploader, Tex, Decoded, PosterReady};
    struct Logos;
    impl Source for Logos {
        fn probe_local(&self, key: PosterKey) -> Option<PosterKey> { Some(key) }
        fn probe_http(&self, _: &str, _: i32, _: i32) -> Option<PosterKey> { Some(PosterKey(1)) }
        fn probe(&self, _: u16, _: &str, _: i32, _: i32, _: bool) -> Option<PosterKey> { Some(PosterKey(2)) }
        fn warm(&self, _: u16, _: &str, _: i32, _: i32, _: bool) -> Warm { Warm::Known }
        fn logo(&self, _: u16, _: &str) -> Option<PosterKey> { None }
        fn logo_warm(&self, _: u16, _: &str) -> Warm { Warm::Known }
        fn unresident(&self, _: PosterKey, _: bool) {}
        fn idle(&self) -> bool { true }
    }
    struct Upload;
    impl Uploader for Upload {
        fn upload(&mut self, d: &Decoded) -> Tex { Tex { id: u32::from(d.w), w: d.w, h: d.h } }
        fn warm(&mut self, _: Tex) {}
        fn free(&mut self, _: Tex) {}
    }
    let _guard = plx_base::testlock::serial();
    crate::tex::install(&Logos);
    crate::tex::reset_for_test(1 << 20);
    for (key, w, h) in [(1, 160, 20), (2, 20, 160), (3, 80, 80)] {
        crate::tex::accept(PosterReady { key: PosterKey(key), result: Ok(Decoded {
            w, h, rgba: vec![255; usize::from(w) * usize::from(h) * 4].into_boxed_slice(),
        }) });
    }
    let mut budget = crate::frame::Budget::new();
    budget.begin_frame(0);
    let mut present = plx_machine::present::Present::new();
    assert_eq!(crate::tex::prepare(&mut budget, &mut Upload, &mut PresentHandle::of(&mut present), || 0), 3);
    let key = PosterKey(3);
    for slot in [Rect::new(90.0, 90.0, 48.0, 32.0), Rect::new(32.0, 900.0, 120.0, 120.0)] {
        let arts = [Art::Http { url: "https://fixture.invalid/logo", res: (96, 64) },
            Art::Thumb { sid: SRC, key: "/logo", res: (240, 240) }, Art::Local { key: &key }];
        for art in arts {
            let quads = crate::draw_census::capture_card_quads(|| card_contained(Painter::root(), slot, art, 3.0));
            assert_eq!(quads.len(), 1);
            let (tex, uv, rect) = quads[0];
            assert_ne!(tex, 0);
            assert!(uv.into_iter().zip(plx_gfx::gfx::UV_FULL).all(|(a, b)| (a-b).abs() < 1e-5));
            assert!(rect.x >= slot.x && rect.y >= slot.y);
            assert!(rect.x + rect.w <= slot.x + slot.w + 1e-4);
            assert!(rect.y + rect.h <= slot.y + slot.h + 1e-4);
        }
    }
    crate::tex::reset_for_test(1 << 20);
}
