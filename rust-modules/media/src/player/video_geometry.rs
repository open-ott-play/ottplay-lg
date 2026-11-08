//! Picture geometry for overlays, independent of the subtitle's authoring resolution.
//! The ACB sourceInfo supplies the decoded raster and pixel aspect ratio; the dev
//! TV's 1440x1080 capture proves that its full-panel window pillarboxes that picture.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Viewport {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Aspect(u32, u32);

impl Aspect {
    pub fn from_raster(width: i32, height: i32) -> Option<Self> {
        (width > 0 && height > 0).then_some(Self(width as u32, height as u32))
    }

    pub fn pack(self) -> u64 {
        (u64::from(self.0) << 32) | u64::from(self.1)
    }

    pub fn unpack(value: u64) -> Option<Self> {
        let aspect = Self((value >> 32) as u32, value as u32);
        (aspect.0 != 0 && aspect.1 != 0).then_some(aspect)
    }

    pub fn fit(self, width: i32, height: i32) -> Viewport {
        let (w, h) = (i64::from(width.max(1)), i64::from(height.max(1)));
        let (num, den) = (i64::from(self.0), i64::from(self.1));
        let (w, h) = if w * den > h * num {
            (((h * num + den / 2) / den).clamp(1, w), h)
        } else {
            (w, ((w * den + num / 2) / num).clamp(1, h))
        };
        Viewport {
            x: (width - w as i32) / 2,
            y: (height - h as i32) / 2,
            width: w as i32,
            height: h as i32,
        }
    }
}

/// Aspect-preserving sizing inside the logical video aperture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PictureMode {
    #[default]
    Fit,
    Fill,
}

/// Finite additional scale; there is no arbitrary or nonuniform stretch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Zoom {
    #[default]
    X100,
    X125,
    X150,
    X175,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Transform {
    pub mode: PictureMode,
    pub zoom: Zoom,
}

impl Transform {
    /// Zero preserves the existing Fit/1x behavior. Unknown codes are not modes.
    pub fn pack(self) -> u8 {
        let mode = match self.mode {
            PictureMode::Fit => 0,
            PictureMode::Fill => 4,
        };
        mode + match self.zoom {
            Zoom::X100 => 0,
            Zoom::X125 => 1,
            Zoom::X150 => 2,
            Zoom::X175 => 3,
        }
    }

    pub fn unpack(code: u8) -> Option<Self> {
        if code > 7 {
            return None;
        }
        Some(Self {
            mode: if code < 4 {
                PictureMode::Fit
            } else {
                PictureMode::Fill
            },
            zoom: match code % 4 {
                0 => Zoom::X100,
                1 => Zoom::X125,
                2 => Zoom::X150,
                _ => Zoom::X175,
            },
        })
    }

    /// Nominal centered destination, not an applied/displayed geometry receipt.
    /// Fit uses the existing integer contain rounding. Fill rounds its expanded
    /// axis outwards, then zoom rounds both dimensions to the nearest pixel.
    /// The host sink fits the supplied display aspect into this destination once
    /// more; its accepted placement owns that actual, potentially smaller picture.
    /// `aspect` already includes any measured pixel aspect ratio; no rotation is inferred.
    pub fn picture(self, aspect: Aspect, width: i32, height: i32) -> Option<Viewport> {
        if width <= 0 || height <= 0 || aspect.0 == 0 || aspect.1 == 0 {
            return None;
        }
        let (w, h) = (width as u128, height as u128);
        let (num, den) = (u128::from(aspect.0), u128::from(aspect.1));
        let (w, h) = match self.mode {
            PictureMode::Fit => {
                let fit = aspect.fit(width, height);
                (fit.width as u128, fit.height as u128)
            }
            PictureMode::Fill if w * den > h * num => (w, (w * den).div_ceil(num)),
            PictureMode::Fill => ((h * num).div_ceil(den), h),
        };
        let quarters = match self.zoom {
            Zoom::X100 => 4,
            Zoom::X125 => 5,
            Zoom::X150 => 6,
            Zoom::X175 => 7,
        };
        let w = i32::try_from((w * quarters + 2) / 4).ok()?;
        let h = i32::try_from((h * quarters + 2) / 4).ok()?;
        if w <= 0 || h <= 0 {
            return None;
        }
        let x = i32::try_from((i64::from(width) - i64::from(w)) / 2).ok()?;
        let y = i32::try_from((i64::from(height) - i64::from(h)) / 2).ok()?;
        x.checked_add(w)?;
        y.checked_add(h)?;
        Some(Viewport {
            x,
            y,
            width: w,
            height: h,
        })
    }
}

/// Parse only the sourceInfo video object, never a similarly named nested field.
/// Observed on the TV as `video:{width,height,pixelAspectRatio:{width,height}}`.
/// This runs only on metadata callbacks, not on the frame/position callback path.
pub(super) fn source_aspect(bytes: &[u8]) -> Option<Aspect> {
    let root: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let video = root.get("video")?;
    let positive = |v: &serde_json::Value| v.as_u64().filter(|&n| n > 0 && n <= 65_536);
    let w = positive(video.get("width")?)?;
    let h = positive(video.get("height")?)?;
    let (pw, ph) = match video.get("pixelAspectRatio") {
        Some(par) => (positive(par.get("width")?)?, positive(par.get("height")?)?),
        None => (1, 1),
    };
    let (num, den) = (w * pw, h * ph);
    let (mut a, mut b) = (num, den);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    Some(Aspect(
        u32::try_from(num / a).ok()?,
        u32::try_from(den / a).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_fit_covers_pillarbox_letterbox_and_anamorphic_sources() {
        let fit = |json: &[u8]| source_aspect(json).unwrap().fit(1920, 1080);
        assert_eq!(fit(br#"{"video":{"width":1440,"height":1080,"pixelAspectRatio":{"width":1,"height":1}}}"#),
            Viewport { x: 240, y: 0, width: 1440, height: 1080 });
        assert_eq!(
            fit(br#"{"video":{"width":1920,"height":800}}"#),
            Viewport {
                x: 0,
                y: 140,
                width: 1920,
                height: 800
            }
        );
        assert_eq!(fit(br#"{"video":{"width":720,"height":576,"pixelAspectRatio":{"width":64,"height":45}}}"#),
            Viewport { x: 0, y: 0, width: 1920, height: 1080 });
        assert_eq!(
            Aspect::from_raster(3840, 2160).unwrap().fit(1920, 1080),
            Viewport {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080
            }
        );
    }

    #[test]
    fn missing_or_invalid_geometry_does_not_publish_an_aspect() {
        for value in [
            br#"{}"#.as_slice(),
            br#"{"video":{"width":0,"height":1080}}"#,
            br#"{"video":{"width":1920,"height":1080,"pixelAspectRatio":{"width":0,"height":1}}}"#,
            br#"{"video":{"width":-1,"height":1080}}"#,
            br#"{"video":{"width":4294967295,"height":1080}}"#,
        ] {
            assert_eq!(source_aspect(value), None);
        }
        assert_eq!(Aspect::unpack(0), None);
        let aspect = source_aspect(br#"{"video":{"width":1440,"height":1080}}"#).unwrap();
        assert_eq!(Aspect::unpack(aspect.pack()), Some(aspect));
    }

    #[test]
    fn transform_codes_are_closed_and_zero_is_the_existing_default() {
        assert_eq!(Transform::default(), Transform { mode: PictureMode::Fit, zoom: Zoom::X100 });
        assert_eq!(Transform::default().pack(), 0);
        for (mode, base) in [(PictureMode::Fit, 0), (PictureMode::Fill, 4)] {
            for (zoom, offset) in [(Zoom::X100, 0), (Zoom::X125, 1), (Zoom::X150, 2), (Zoom::X175, 3)] {
                let transform = Transform { mode, zoom };
                assert_eq!(transform.pack(), base + offset);
                assert_eq!(Transform::unpack(base + offset), Some(transform));
            }
        }
        for code in 8..=u8::MAX { assert_eq!(Transform::unpack(code), None); }
    }

    #[test]
    fn default_transform_keeps_existing_contain_geometry() {
        for (w, h) in [(1440, 1080), (1920, 800), (853, 479), (720, 576)] {
            let aspect = Aspect::from_raster(w, h).unwrap();
            assert_eq!(Transform::default().picture(aspect, 1920, 1080), Some(aspect.fit(1920, 1080)));
        }
    }

    #[test]
    fn fill_covers_and_centers_without_stretching_or_inventing_sar() {
        let fill = Transform { mode: PictureMode::Fill, zoom: Zoom::X100 };
        assert_eq!(fill.picture(Aspect::from_raster(4, 3).unwrap(), 1920, 1080),
            Some(Viewport { x: 0, y: -180, width: 1920, height: 1440 }));
        assert_eq!(fill.picture(Aspect::from_raster(12, 5).unwrap(), 1920, 1080),
            Some(Viewport { x: -336, y: 0, width: 2592, height: 1080 }));
        let anamorphic = source_aspect(br#"{"video":{"width":720,"height":576,"pixelAspectRatio":{"width":64,"height":45}}}"#).unwrap();
        assert_eq!(fill.picture(anamorphic, 1920, 1080),
            Some(Viewport { x: 0, y: 0, width: 1920, height: 1080 }));
    }

    #[test]
    fn bounded_zoom_scales_the_selected_picture_and_keeps_its_center() {
        let aspect = Aspect::from_raster(12, 5).unwrap();
        for (zoom, expected) in [
            (Zoom::X100, Viewport { x: 0, y: 140, width: 1920, height: 800 }),
            (Zoom::X125, Viewport { x: -240, y: 40, width: 2400, height: 1000 }),
            (Zoom::X150, Viewport { x: -480, y: -60, width: 2880, height: 1200 }),
            (Zoom::X175, Viewport { x: -720, y: -160, width: 3360, height: 1400 }),
        ] {
            assert_eq!(Transform { mode: PictureMode::Fit, zoom }.picture(aspect, 1920, 1080), Some(expected));
        }
        assert_eq!(Transform { mode: PictureMode::Fill, zoom: Zoom::X125 }
            .picture(Aspect::from_raster(4, 3).unwrap(), 1920, 1080),
            Some(Viewport { x: -240, y: -360, width: 2400, height: 1800 }));
    }

    #[test]
    fn transform_refuses_invalid_canvas_aspect_and_unrepresentable_extent() {
        let aspect = Aspect::from_raster(16, 9).unwrap();
        for (w, h) in [(0, 1080), (1920, 0), (-1, 1080), (1920, -1)] {
            assert_eq!(Transform::default().picture(aspect, w, h), None);
        }
        for aspect in [Aspect(0, 1), Aspect(1, 0), Aspect(0, 0)] {
            assert_eq!(Transform::default().picture(aspect, 1920, 1080), None);
        }
        let fill = Transform { mode: PictureMode::Fill, zoom: Zoom::X100 };
        assert_eq!(fill.picture(Aspect(u32::MAX, 1), 1920, 1080), None);
        assert_eq!(fill.picture(Aspect(1, u32::MAX), 1920, 1080), None);
        assert_eq!(Transform { mode: PictureMode::Fill, zoom: Zoom::X175 }
            .picture(Aspect(1, 1), i32::MAX, i32::MAX), None);
        assert_eq!(fill.picture(Aspect(u32::MAX, u32::MAX), i32::MAX, i32::MAX),
            Some(Viewport { x: 0, y: 0, width: i32::MAX, height: i32::MAX }));
    }

    #[test]
    fn nominal_destination_is_not_a_second_fit_or_display_receipt() {
        let aspect = Aspect::from_raster(100, 1).unwrap();
        let nominal = Transform { mode: PictureMode::Fit, zoom: Zoom::X125 }
            .picture(aspect, 149, 100).unwrap();
        assert_eq!(nominal, Viewport { x: -18, y: 49, width: 186, height: 1 });
        assert_eq!(aspect.fit(nominal.width, nominal.height),
            Viewport { x: 43, y: 0, width: 100, height: 1 });
        assert_eq!(Transform { mode: PictureMode::Fill, zoom: Zoom::X100 }
            .picture(Aspect::from_raster(16, 9).unwrap(), 101, 101),
            Some(Viewport { x: -39, y: 0, width: 180, height: 101 }));
    }
}
