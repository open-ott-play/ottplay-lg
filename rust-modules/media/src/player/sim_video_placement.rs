//! Logical-canvas placement of the simulator's existing padded decoded frame.
//! Source raster determines texture padding. Optional measured display aspect
//! determines picture shape; neither panel pixels nor invented PAR enter here.
use super::super::video_geometry::{Aspect, Viewport};

pub(super) const WIDTH: i32 = 1920;
pub(super) const HEIGHT: i32 = 1080;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Placement {
    source: (i32, i32),
    destination: Viewport,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Draw {
    pub picture: Viewport,
    pub matte: Viewport,
    pub crop: [f32; 4],
}

impl Draw {
    pub(super) fn is_full_texture(self) -> bool {
        self.picture == Placement::FULL.destination && self.crop == [0.0, 0.0, 1.0, 1.0]
    }
}

impl Placement {
    pub(super) const FULL: Self = Self {
        source: (1920, 1080),
        destination: Viewport {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        },
    };

    pub(super) fn new(source: (i32, i32), destination: Viewport) -> Option<Self> {
        if source.0 <= 0 || source.1 <= 0 || destination.width <= 0 || destination.height <= 0 {
            return None;
        }
        destination.x.checked_add(destination.width)?;
        destination.y.checked_add(destination.height)?;
        Some(Self {
            source,
            destination,
        })
    }

    /// The full picture before the logical canvas clips it. Shared with the
    /// presentation query so authored overlays use exactly the image's fit.
    pub(super) fn picture(self, display_aspect: Option<Aspect>) -> Option<Viewport> {
        let raster = Aspect::from_raster(self.source.0, self.source.1)?;
        let destination = self.destination;
        let fit = display_aspect
            .unwrap_or(raster)
            .fit(destination.width, destination.height);
        Some(Viewport {
            x: destination.x + fit.x,
            y: destination.y + fit.y,
            ..fit
        })
    }

    pub(super) fn draw(self, display_aspect: Option<Aspect>) -> Option<Draw> {
        let raster = Aspect::from_raster(self.source.0, self.source.1)?;
        let texture = raster.fit(WIDTH, HEIGHT);
        let destination = self.destination;
        let full_picture = self.picture(display_aspect)?;
        // The usual full-canvas case uses the original padded texture byte-for-byte.
        // Cropping is needed only for an inline destination or a measured PAR correction.
        if destination == Self::FULL.destination && full_picture == texture {
            return Some(Draw {
                picture: destination,
                matte: destination,
                crop: [0.0, 0.0, 1.0, 1.0],
            });
        }
        let matte = clip(destination)?;
        let Some(picture) = clip(full_picture) else {
            // A partly off-canvas viewport can expose only its letterbox region.
            return Some(Draw {
                picture: Viewport::default(),
                matte,
                crop: [0.0; 4],
            });
        };
        let left = (picture.x - full_picture.x) as f32 / full_picture.width as f32;
        let top = (picture.y - full_picture.y) as f32 / full_picture.height as f32;
        let width = picture.width as f32 / full_picture.width as f32;
        let height = picture.height as f32 / full_picture.height as f32;
        Some(Draw {
            picture,
            matte,
            crop: [
                (texture.x as f32 + left * texture.width as f32) / 1920.0,
                (texture.y as f32 + top * texture.height as f32) / 1080.0,
                width * texture.width as f32 / 1920.0,
                height * texture.height as f32 / 1080.0,
            ],
        })
    }
}

fn clip(rect: Viewport) -> Option<Viewport> {
    let left = rect.x.max(0);
    let top = rect.y.max(0);
    let right = (rect.x + rect.width).min(1920);
    let bottom = (rect.y + rect.height).min(1080);
    (right > left && bottom > top).then_some(Viewport {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect(x: i32, y: i32, width: i32, height: i32) -> Viewport {
        Viewport {
            x,
            y,
            width,
            height,
        }
    }
    fn draw(source: (i32, i32), destination: Viewport) -> Draw {
        Placement::new(source, destination)
            .unwrap()
            .draw(None)
            .unwrap()
    }
    #[test]
    fn default_fullscreen_keeps_full_texture_and_canvas() {
        let value = Placement::FULL.draw(None).unwrap();
        assert_eq!(value.picture, rect(0, 0, 1920, 1080));
        assert_eq!(value.matte, value.picture);
        assert_eq!(value.crop, [0.0, 0.0, 1.0, 1.0]);
    }
    #[test]
    fn preview_moves_same_picture_without_double_letterboxing() {
        let widescreen = draw((1920, 1080), rect(48, 120, 640, 360));
        assert_eq!(widescreen.picture, rect(48, 120, 640, 360));
        let classic = draw((1440, 1080), rect(48, 120, 640, 360));
        assert_eq!(classic.picture, rect(128, 120, 480, 360));
        assert_eq!(classic.matte, rect(48, 120, 640, 360));
        assert_eq!(classic.crop, [0.125, 0.0, 0.75, 1.0]);
        let matched = draw((1440, 1080), rect(48, 120, 640, 480));
        assert_eq!(matched.picture, matched.matte);
        assert_eq!(matched.crop, classic.crop);
    }
    #[test]
    fn measured_anamorphic_aspect_changes_display_not_raster_padding() {
        let placement = Placement::new((720, 576), rect(0, 0, 1920, 1080)).unwrap();
        let plain = placement.draw(None).unwrap();
        assert!(plain.is_full_texture());
        let aspect = super::super::super::video_geometry::source_aspect(
            br#"{"video":{"width":720,"height":576,"pixelAspectRatio":{"width":64,"height":45}}}"#,
        )
        .unwrap();
        let measured = placement.draw(Some(aspect)).unwrap();
        assert_eq!(measured.picture, rect(0, 0, 1920, 1080));
        assert_eq!(measured.crop, [285.0 / 1920.0, 0.0, 1350.0 / 1920.0, 1.0]);
    }
    #[test]
    fn offscreen_destination_clips_uv_without_distorting_the_visible_part() {
        let value = draw((1920, 1080), rect(-192, -108, 2304, 1296));
        assert_eq!(value.picture, rect(0, 0, 1920, 1080));
        assert_eq!(value.matte, value.picture);
        for (actual, expected) in
            value
                .crop
                .into_iter()
                .zip([1.0 / 12.0, 1.0 / 12.0, 5.0 / 6.0, 5.0 / 6.0])
        {
            assert!((actual - expected).abs() < 0.000_001);
        }
        assert!(Placement::new((1920, 1080), rect(2000, 0, 640, 360))
            .unwrap()
            .draw(None)
            .is_none());
    }
    #[test]
    fn odd_raster_rounding_matches_the_rgba_scale_and_pad_filter() {
        for (source, texture) in [
            ((853, 479), rect(0, 1, 1920, 1078)),
            ((1919, 803), rect(0, 138, 1920, 803)),
            ((720, 576), rect(285, 0, 1350, 1080)),
        ] {
            let value = draw(source, rect(20, 30, 640, 360));
            assert_eq!(
                value.crop,
                [
                    texture.x as f32 / 1920.0,
                    texture.y as f32 / 1080.0,
                    texture.width as f32 / 1920.0,
                    texture.height as f32 / 1080.0
                ]
            );
            assert!(draw(source, rect(0, 0, 1920, 1080)).is_full_texture());
        }
    }
    #[test]
    fn partly_visible_letterbox_still_gets_only_its_own_matte() {
        let value = draw((1, 100), rect(-700, 50, 800, 600));
        assert_eq!(value.picture, Viewport::default());
        assert_eq!(value.matte, rect(0, 50, 100, 600));
        assert_eq!(value.crop, [0.0; 4]);
    }
    #[test]
    fn invalid_dimensions_and_overflow_are_refused() {
        for source in [(0, 1080), (-1, 1080), (1920, 0)] {
            assert!(Placement::new(source, rect(0, 0, 640, 360)).is_none());
        }
        for target in [
            rect(0, 0, 0, 360),
            rect(0, 0, 640, -1),
            rect(i32::MAX, 0, 640, 360),
            rect(0, i32::MAX, 640, 360),
        ] {
            assert!(Placement::new((1920, 1080), target).is_none());
        }
    }

    #[test]
    fn picture_keeps_virtual_extent_before_canvas_clipping() {
        let destination = rect(-192, -108, 2304, 1296);
        let placement = Placement::new((1920, 1080), destination).unwrap();
        assert_eq!(placement.picture(None), Some(destination));
        assert_eq!(placement.draw(None).unwrap().picture, rect(0, 0, 1920, 1080));
    }

    #[test]
    fn picture_and_draw_share_measured_anamorphic_fit() {
        let destination = rect(-240, -135, 2400, 1350);
        let placement = Placement::new((720, 576), destination).unwrap();
        let aspect = super::super::super::video_geometry::source_aspect(
            br#"{"video":{"width":720,"height":576,"pixelAspectRatio":{"width":64,"height":45}}}"#,
        ).unwrap();
        assert_eq!(placement.picture(Some(aspect)), Some(destination));
        let visible = placement.draw(Some(aspect)).unwrap();
        assert_eq!(visible.picture, rect(0, 0, 1920, 1080));
        assert_eq!(visible.matte, visible.picture);
        assert_eq!(placement.picture(None), Some(rect(116, -135, 1688, 1350)));
    }
}
