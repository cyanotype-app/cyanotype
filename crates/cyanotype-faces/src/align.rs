//! Face alignment: the similarity transform (rotation, uniform scale, shift; no shear, no mirroring) that carries a face's five
//! landmarks onto the ArcFace template, and the 112 x 112 crop it produces. This is the input recognition models of the
//! ArcFace family (SFace included) were trained on, so a face that is turned, tilted or far away looks the same to them.

/// Side of the aligned crop.
pub const SIZE: usize = 112;

/// Where the five landmarks (right eye, left eye, nose tip, right mouth corner, left mouth corner) sit in the 112 x 112
/// crop: the template of ArcFace and of OpenCV's `FaceRecognizerSF::alignCrop`.
pub const TEMPLATE: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

/// A similarity transform: `x' = a*x - b*y + tx`, `y' = b*x + a*y + ty`. Its scale is `hypot(a, b)`, its rotation `atan2(b, a)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Similarity {
    pub a: f32,
    pub b: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Similarity {
    pub fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        [
            self.a * p[0] - self.b * p[1] + self.tx,
            self.b * p[0] + self.a * p[1] + self.ty,
        ]
    }

    /// The transform that undoes this one, or `None` for a degenerate (zero scale) one.
    pub fn inverse(&self) -> Option<Similarity> {
        let n = self.a * self.a + self.b * self.b;
        if n <= f32::MIN_POSITIVE {
            return None;
        }
        let (ia, ib) = (self.a / n, -self.b / n);
        Some(Similarity {
            a: ia,
            b: ib,
            tx: -(ia * self.tx - ib * self.ty),
            ty: -(ib * self.tx + ia * self.ty),
        })
    }

    pub fn scale(&self) -> f32 {
        self.a.hypot(self.b)
    }
}

/// The least-squares similarity transform taking `src` onto `dst` (Umeyama, 1991, in the plane: treating points as complex
/// numbers, `dst = m * src + t` with `m = a + bi`). `None` when the source points are all the same point.
pub fn estimate(src: &[[f32; 2]], dst: &[[f32; 2]]) -> Option<Similarity> {
    if src.len() != dst.len() || src.len() < 2 {
        return None;
    }
    let n = src.len() as f64;
    let mean = |pts: &[[f32; 2]]| {
        let (sx, sy) = pts
            .iter()
            .fold((0.0f64, 0.0f64), |(x, y), p| (x + f64::from(p[0]), y + f64::from(p[1])));
        (sx / n, sy / n)
    };
    let ((sx, sy), (dx, dy)) = (mean(src), mean(dst));
    let (mut num_a, mut num_b, mut den) = (0.0f64, 0.0f64, 0.0f64);
    for (p, q) in src.iter().zip(dst) {
        let (px, py) = (f64::from(p[0]) - sx, f64::from(p[1]) - sy);
        let (qx, qy) = (f64::from(q[0]) - dx, f64::from(q[1]) - dy);
        num_a += px * qx + py * qy;
        num_b += px * qy - py * qx;
        den += px * px + py * py;
    }
    if den <= 1e-12 {
        return None;
    }
    let (a, b) = (num_a / den, num_b / den);
    Some(Similarity {
        a: a as f32,
        b: b as f32,
        tx: (dx - (a * sx - b * sy)) as f32,
        ty: (dy - (b * sx + a * sy)) as f32,
    })
}

/// The transform from a face's landmarks (in its image's pixels) to the crop.
pub fn to_template(landmarks: &[[f32; 2]; 5]) -> Option<Similarity> {
    estimate(landmarks, &TEMPLATE)
}

/// Resample an RGB8 image into a 112 x 112 RGB8 crop. `to_crop` maps source pixels to crop pixels; each crop pixel is read
/// at its inverse image with bilinear interpolation (pixel centres at integer coordinates, like OpenCV's `warpAffine`);
/// what falls outside the source is black.
pub fn warp(rgb: &[u8], w: u32, h: u32, to_crop: &Similarity) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || rgb.len() != w as usize * h as usize * 3 {
        return None;
    }
    let inv = to_crop.inverse()?;
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; SIZE * SIZE * 3];
    let px = |x: isize, y: isize, c: usize| -> f32 {
        if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
            0.0
        } else {
            f32::from(rgb[(y as usize * w + x as usize) * 3 + c])
        }
    };
    for oy in 0..SIZE {
        for ox in 0..SIZE {
            let [sx, sy] = inv.apply([ox as f32, oy as f32]);
            let (x0, y0) = (sx.floor(), sy.floor());
            let (fx, fy) = (sx - x0, sy - y0);
            let (x0, y0) = (x0 as isize, y0 as isize);
            for c in 0..3 {
                let top = px(x0, y0, c) * (1.0 - fx) + px(x0 + 1, y0, c) * fx;
                let bottom = px(x0, y0 + 1, c) * (1.0 - fx) + px(x0 + 1, y0 + 1, c) * fx;
                out[(oy * SIZE + ox) * 3 + c] = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Some(out)
}

/// A face's aligned 112 x 112 RGB8 crop from its five landmarks.
pub fn align(rgb: &[u8], w: u32, h: u32, landmarks: &[[f32; 2]; 5]) -> Option<Vec<u8>> {
    warp(rgb, w, h, &to_template(landmarks)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 2], b: [f32; 2], tol: f32) -> bool {
        (a[0] - b[0]).abs() <= tol && (a[1] - b[1]).abs() <= tol
    }

    #[test]
    fn the_template_maps_to_itself_by_the_identity() {
        let s = estimate(&TEMPLATE, &TEMPLATE).unwrap();
        assert!((s.a - 1.0).abs() < 1e-5 && s.b.abs() < 1e-5 && s.tx.abs() < 1e-3 && s.ty.abs() < 1e-3);
    }

    #[test]
    fn a_known_scale_rotation_and_shift_is_recovered() {
        // Landmarks = template turned by 20 degrees, enlarged 2.5 times, moved to (400, 300).
        let (angle, scale) = (20.0f32.to_radians(), 2.5f32);
        let (a, b) = (scale * angle.cos(), scale * angle.sin());
        let truth = Similarity {
            a,
            b,
            tx: 400.0,
            ty: 300.0,
        };
        let landmarks: Vec<[f32; 2]> = TEMPLATE.iter().map(|p| truth.apply(*p)).collect();
        let found = estimate(&landmarks, &TEMPLATE).unwrap();
        assert!((found.scale() - 1.0 / scale).abs() < 1e-4);
        assert!((found.b.atan2(found.a) + angle).abs() < 1e-4);
        for (l, t) in landmarks.iter().zip(&TEMPLATE) {
            assert!(close(found.apply(*l), *t, 1e-2));
        }
        // And the inverse takes the crop back.
        let inv = found.inverse().unwrap();
        for (l, t) in landmarks.iter().zip(&TEMPLATE) {
            assert!(close(inv.apply(*t), *l, 1e-2));
        }
    }

    #[test]
    fn noisy_landmarks_are_fitted_in_the_least_squares_sense() {
        let landmarks: Vec<[f32; 2]> = TEMPLATE
            .iter()
            .enumerate()
            .map(|(i, p)| {
                [
                    p[0] * 2.0 + 10.0 + if i % 2 == 0 { 0.5 } else { -0.5 },
                    p[1] * 2.0 + 20.0,
                ]
            })
            .collect();
        let s = estimate(&landmarks, &TEMPLATE).unwrap();
        assert!((s.scale() - 0.5).abs() < 0.01);
        for (l, t) in landmarks.iter().zip(&TEMPLATE) {
            assert!(close(s.apply(*l), *t, 1.0));
        }
    }

    #[test]
    fn degenerate_input_has_no_transform() {
        assert!(estimate(&[[1.0, 1.0]; 5], &TEMPLATE).is_none());
        assert!(estimate(&TEMPLATE[..1], &TEMPLATE[..1]).is_none());
        assert!(estimate(&TEMPLATE[..3], &TEMPLATE).is_none());
        assert!(to_template(&[[5.0, 5.0]; 5]).is_none());
    }

    #[test]
    fn warping_with_the_identity_copies_the_image_and_blanks_the_outside() {
        let (w, h) = (SIZE as u32 - 12, SIZE as u32);
        let rgb: Vec<u8> = (0..w * h * 3).map(|i| (i % 251) as u8).collect();
        let id = Similarity {
            a: 1.0,
            b: 0.0,
            tx: 0.0,
            ty: 0.0,
        };
        let out = warp(&rgb, w, h, &id).unwrap();
        assert_eq!(&out[..30], &rgb[..30]);
        let row = 50 * SIZE * 3;
        assert_eq!(&out[row..row + 3], &rgb[50 * w as usize * 3..50 * w as usize * 3 + 3]);
        // The last 12 columns are outside the source: black.
        assert_eq!(&out[row + (SIZE - 1) * 3..row + SIZE * 3], &[0, 0, 0]);
    }

    #[test]
    fn warping_puts_the_landmarks_where_the_template_wants_them() {
        // A 400x300 image that is black with bright squares on five landmarks, tilted and enlarged.
        let (w, h) = (400u32, 300u32);
        let truth = Similarity {
            a: 2.0 * 12.0f32.to_radians().cos(),
            b: 2.0 * 12.0f32.to_radians().sin(),
            tx: 80.0,
            ty: 10.0,
        };
        let lm: [[f32; 2]; 5] = std::array::from_fn(|i| truth.apply(TEMPLATE[i]));
        let mut rgb = vec![0u8; (w * h * 3) as usize];
        for p in lm {
            for dy in -6i32..=6 {
                for dx in -6i32..=6 {
                    let (x, y) = (p[0] as i32 + dx, p[1] as i32 + dy);
                    let o = (y as usize * w as usize + x as usize) * 3;
                    rgb[o..o + 3].copy_from_slice(&[255, 255, 255]);
                }
            }
        }
        let crop = align(&rgb, w, h, &lm).unwrap();
        assert_eq!(crop.len(), SIZE * SIZE * 3);
        for t in TEMPLATE {
            let o = (t[1].round() as usize * SIZE + t[0].round() as usize) * 3;
            assert!(crop[o] > 200, "template point {t:?} is {}", crop[o]);
        }
        // Away from the landmarks it is still black.
        assert_eq!(crop[(10 * SIZE + 10) * 3], 0);
    }

    #[test]
    fn warp_refuses_a_buffer_of_the_wrong_size() {
        let id = Similarity {
            a: 1.0,
            b: 0.0,
            tx: 0.0,
            ty: 0.0,
        };
        assert!(warp(&[0; 5], 4, 4, &id).is_none());
        assert!(warp(&[], 0, 4, &id).is_none());
    }
}
