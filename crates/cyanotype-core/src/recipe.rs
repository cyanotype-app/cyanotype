//! The edit recipe of a photo: quarter turns, a straighten angle and a crop (`docs/library.md`, "Rotate and crop").
//!
//! A recipe says how to *look at* the original; the file itself is never touched. It travels with the other edits
//! ([`crate::edits::Edits::recipe`], the `edits/` object in the bucket, `assets.recipe` in the database) and the worker
//! renders the thumbnail, the preview and the "edited" JPEG from the original plus the recipe.
//!
//! The steps, in this order, on the picture as it is displayed (the file's EXIF orientation already applied):
//!
//! 1. **rotate** clockwise by 0, 90, 180 or 270 degrees;
//! 2. **straighten** by an angle in degrees (positive turns the picture clockwise), keeping the frame's size: the picture
//!    is zoomed just enough that no empty corner shows ([`Recipe::straighten_zoom`]);
//! 3. **crop** to a rectangle in fractions of the frame that steps 1 and 2 produced (`x`, `y` from the top left).
//!
//! Everything here is pure arithmetic; the pixels are the worker's (`imaging::apply_recipe`).
use serde::{Deserialize, Serialize};

/// The largest straighten angle, in degrees either way.
pub const MAX_STRAIGHTEN: f32 = 45.0;
/// The smallest crop side, as a fraction of the frame (a crop is a choice of a part, not a speck).
pub const MIN_CROP_SIDE: f64 = 0.02;
/// Coordinates are kept to this many decimals, so the same crop is the same JSON.
const DECIMALS: f64 = 10_000.0;

/// A rectangle in fractions of the frame: `x` and `y` are the top left corner.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Crop {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Quarter turns, a straighten angle and a crop. The default (no turn, no angle, no crop) changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Recipe {
    /// Clockwise degrees: 0, 90, 180 or 270.
    #[serde(default, skip_serializing_if = "is_zero_u16")]
    pub rotate: u16,
    /// Degrees, -45 to 45, to a tenth.
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub straighten: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<Crop>,
}

fn is_zero_u16(v: &u16) -> bool {
    *v == 0
}
fn is_zero_f32(v: &f32) -> bool {
    *v == 0.0
}

fn round4(v: f64) -> f64 {
    (v * DECIMALS).round() / DECIMALS
}

impl Recipe {
    /// Nothing to do.
    pub fn is_identity(&self) -> bool {
        self.rotate == 0 && self.straighten == 0.0 && self.crop.is_none()
    }

    /// Check what a client sent and put it in its one canonical form: a rotation that is a multiple of 90 is reduced
    /// to 0..=270, the angle is rounded to a tenth of a degree, a crop is clamped into the frame and rounded, and a crop
    /// of the whole frame is none. Returns `None` for a recipe that changes nothing, so "reset" and "never edited"
    /// are the same state.
    pub fn normalized(self) -> Result<Option<Recipe>, String> {
        if !self.rotate.is_multiple_of(90) {
            return Err("rotate must be a multiple of 90 degrees".into());
        }
        let rotate = self.rotate % 360;
        if !self.straighten.is_finite() || self.straighten.abs() > MAX_STRAIGHTEN {
            return Err(format!(
                "straighten must be within -{MAX_STRAIGHTEN} and {MAX_STRAIGHTEN} degrees"
            ));
        }
        let straighten = (self.straighten * 10.0).round() / 10.0;
        let straighten = if straighten == 0.0 { 0.0 } else { straighten };
        let crop = match self.crop {
            None => None,
            Some(c) => {
                if ![c.x, c.y, c.w, c.h].iter().all(|v| v.is_finite()) {
                    return Err("the crop must be numbers".into());
                }
                if c.x < -0.001 || c.y < -0.001 || c.x + c.w > 1.001 || c.y + c.h > 1.001 {
                    return Err("the crop must lie inside the picture".into());
                }
                if c.w < MIN_CROP_SIDE || c.h < MIN_CROP_SIDE {
                    return Err("the crop is too small".into());
                }
                let x = round4(c.x.clamp(0.0, 1.0));
                let y = round4(c.y.clamp(0.0, 1.0));
                let w = round4(c.w.min(1.0 - x));
                let h = round4(c.h.min(1.0 - y));
                if x == 0.0 && y == 0.0 && w >= 1.0 && h >= 1.0 {
                    None
                } else {
                    Some(Crop { x, y, w, h })
                }
            }
        };
        let r = Recipe {
            rotate,
            straighten,
            crop,
        };
        Ok((!r.is_identity()).then_some(r))
    }

    /// The size of the frame after the quarter turns (the straighten keeps it), for a picture that is `w` x `h` displayed.
    pub fn frame_dims(&self, w: u32, h: u32) -> (u32, u32) {
        if self.rotate % 180 == 90 { (h, w) } else { (w, h) }
    }

    /// The size of the result for a picture that is `w` x `h` as displayed: the frame, cut to the crop.
    pub fn output_dims(&self, w: u32, h: u32) -> (u32, u32) {
        let (fw, fh) = self.frame_dims(w, h);
        match self.crop {
            None => (fw, fh),
            Some(c) => (
                ((f64::from(fw) * c.w).round() as u32).clamp(1, fw),
                ((f64::from(fh) * c.h).round() as u32).clamp(1, fh),
            ),
        }
    }

    /// The pixel rectangle `(x, y, w, h)` of the crop in a frame of `fw` x `fh` (the whole frame without a crop).
    pub fn crop_pixels(&self, fw: u32, fh: u32) -> (u32, u32, u32, u32) {
        match self.crop {
            None => (0, 0, fw, fh),
            Some(c) => {
                let x = ((f64::from(fw) * c.x).round() as u32).min(fw - 1);
                let y = ((f64::from(fh) * c.y).round() as u32).min(fh - 1);
                let w = ((f64::from(fw) * c.w).round() as u32).clamp(1, fw - x);
                let h = ((f64::from(fh) * c.h).round() as u32).clamp(1, fh - y);
                (x, y, w, h)
            }
        }
    }

    /// The zoom that keeps a frame of `fw` x `fh` filled after turning the picture by the straighten angle: 1 for none.
    /// The turned picture must cover every corner of the frame, which is `cos a + sin a * max(fw/fh, fh/fw)`.
    pub fn straighten_zoom(&self, fw: u32, fh: u32) -> f64 {
        if self.straighten == 0.0 || fw == 0 || fh == 0 {
            return 1.0;
        }
        let a = f64::from(self.straighten.abs()).to_radians();
        let (fw, fh) = (f64::from(fw), f64::from(fh));
        a.cos() + a.sin() * (fw / fh).max(fh / fw)
    }

    /// The fraction of the frame's long edge that the crop keeps, for how much of the original a render needs
    /// (a crop of a quarter of the width needs the original at four times the output's size).
    pub fn crop_fraction(&self, fw: u32, fh: u32) -> f64 {
        match self.crop {
            None => 1.0,
            Some(c) => {
                let long = f64::from(fw.max(fh));
                (c.w * f64::from(fw)).max(c.h * f64::from(fh)) / long
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crop(x: f64, y: f64, w: f64, h: f64) -> Option<Crop> {
        Some(Crop { x, y, w, h })
    }

    #[test]
    fn nothing_to_do_is_none() {
        assert_eq!(Recipe::default().normalized().unwrap(), None);
        let r = Recipe {
            rotate: 360,
            straighten: 0.04,
            crop: crop(0.0, 0.0, 1.0, 1.0),
        };
        assert_eq!(
            r.normalized().unwrap(),
            None,
            "a full crop, 360 degrees and 0.04 degrees change nothing"
        );
    }

    #[test]
    fn canonical_form() {
        let r = Recipe {
            rotate: 450,
            straighten: -1.26,
            crop: crop(0.123456, 0.2, 0.5, 0.6),
        };
        let n = r.normalized().unwrap().unwrap();
        assert_eq!(n.rotate, 90);
        assert_eq!(n.straighten, -1.3);
        assert_eq!(n.crop, crop(0.1235, 0.2, 0.5, 0.6));
        // The same recipe again is the same recipe.
        assert_eq!(n.normalized().unwrap().unwrap(), n);
        // A crop that overshoots by rounding noise is clamped, not refused.
        let c = Recipe {
            crop: crop(0.5, 0.5, 0.5004, 0.5),
            ..Recipe::default()
        };
        assert_eq!(c.normalized().unwrap().unwrap().crop, crop(0.5, 0.5, 0.5, 0.5));
    }

    #[test]
    fn refusals() {
        let bad = |r: Recipe| r.normalized().is_err();
        assert!(bad(Recipe {
            rotate: 45,
            ..Recipe::default()
        }));
        assert!(bad(Recipe {
            straighten: 46.0,
            ..Recipe::default()
        }));
        assert!(bad(Recipe {
            straighten: f32::NAN,
            ..Recipe::default()
        }));
        for c in [
            crop(0.5, 0.5, 0.6, 0.2),
            crop(-0.2, 0.0, 0.5, 0.5),
            crop(0.0, 0.0, 0.01, 0.5),
            crop(0.0, 0.0, f64::NAN, 0.5),
            crop(0.0, 0.0, 0.5, f64::INFINITY),
        ] {
            assert!(
                bad(Recipe {
                    crop: c,
                    ..Recipe::default()
                }),
                "{c:?}"
            );
        }
    }

    #[test]
    fn dimensions() {
        let r = Recipe {
            rotate: 90,
            crop: crop(0.0, 0.0, 0.5, 1.0),
            ..Recipe::default()
        };
        assert_eq!(r.frame_dims(4000, 3000), (3000, 4000));
        assert_eq!(r.output_dims(4000, 3000), (1500, 4000));
        assert_eq!(Recipe::default().output_dims(4000, 3000), (4000, 3000));
        let s = Recipe {
            straighten: 3.0,
            ..Recipe::default()
        };
        assert_eq!(s.output_dims(4000, 3000), (4000, 3000), "straightening keeps the frame");
        // The pixel rectangle never leaves the frame, even for a rounding-edge crop.
        let c = Recipe {
            crop: crop(0.9999, 0.9999, 0.0001 + 0.02, 0.02),
            ..Recipe::default()
        };
        let (x, y, w, h) = c.crop_pixels(100, 50);
        assert!(x + w <= 100 && y + h <= 50 && w >= 1 && h >= 1);
    }

    #[test]
    fn straighten_zoom_covers_the_frame() {
        let s = |deg: f32| Recipe {
            straighten: deg,
            ..Recipe::default()
        };
        assert_eq!(s(0.0).straighten_zoom(400, 300), 1.0);
        // A square turned 45 degrees needs sqrt(2).
        assert!((s(45.0).straighten_zoom(100, 100) - 2f64.sqrt()).abs() < 1e-9);
        // The angle's sign does not matter, and a wide frame needs more than a square one.
        assert_eq!(s(-5.0).straighten_zoom(400, 300), s(5.0).straighten_zoom(400, 300));
        assert!(s(5.0).straighten_zoom(400, 100) > s(5.0).straighten_zoom(300, 300));
        // Check the corner condition directly: every frame corner lands inside the picture.
        let (fw, fh) = (400.0f64, 300.0f64);
        let k = s(7.5).straighten_zoom(400, 300);
        let a = 7.5f64.to_radians();
        for (cx, cy) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
            let (u, v) = (cx * fw / 2.0 / k, cy * fh / 2.0 / k);
            let (sx, sy) = (u * a.cos() + v * a.sin(), -u * a.sin() + v * a.cos());
            assert!(sx.abs() <= fw / 2.0 + 1e-9 && sy.abs() <= fh / 2.0 + 1e-9, "{sx} {sy}");
        }
    }

    #[test]
    fn crop_fraction_of_the_long_edge() {
        let r = Recipe {
            crop: crop(0.0, 0.0, 0.25, 0.5),
            ..Recipe::default()
        };
        assert!(
            (r.crop_fraction(4000, 3000) - 0.375).abs() < 1e-9,
            "the crop is 1500 px of a 4000 px long edge"
        );
        assert!((r.crop_fraction(3000, 4000) - 0.5).abs() < 1e-9);
        assert_eq!(Recipe::default().crop_fraction(10, 10), 1.0);
    }

    #[test]
    fn json_leaves_out_what_is_not_set() {
        let r = Recipe {
            rotate: 90,
            ..Recipe::default()
        };
        assert_eq!(serde_json::to_string(&r).unwrap(), r#"{"rotate":90}"#);
        assert_eq!(serde_json::from_str::<Recipe>("{}").unwrap(), Recipe::default());
    }
}
