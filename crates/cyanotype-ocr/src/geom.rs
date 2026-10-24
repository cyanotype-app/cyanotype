//! The little geometry text detection needs without OpenCV: connected components of a bitmap, a convex hull, the smallest
//! rotated rectangle around a set of points, and sampling a rotated rectangle out of an image.

pub type P = (f32, f32);

/// Connected components (8-neighbourhood) of `bits` (row-major, `w` x `h`): the pixels of each.
pub fn components(bits: &[bool], w: usize, h: usize, max_components: usize) -> Vec<Vec<(u32, u32)>> {
    let mut seen = vec![false; bits.len()];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..bits.len() {
        if !bits[start] || seen[start] {
            continue;
        }
        if out.len() >= max_components {
            break;
        }
        let mut comp = Vec::new();
        seen[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            comp.push((x as u32, y as u32));
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if bits[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        out.push(comp);
    }
    out
}

fn cross(o: P, a: P, b: P) -> f32 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// Andrew's monotone chain, counter-clockwise (in a y-up reading), without repeated points.
pub fn convex_hull(points: &[P]) -> Vec<P> {
    let mut p: Vec<P> = points.to_vec();
    p.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let mut hull: Vec<P> = Vec::with_capacity(p.len() * 2);
    for &pt in &p {
        while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], pt) <= 0.0 {
            hull.pop();
        }
        hull.push(pt);
    }
    let lower = hull.len() + 1;
    for &pt in p.iter().rev().skip(1) {
        while hull.len() >= lower && cross(hull[hull.len() - 2], hull[hull.len() - 1], pt) <= 0.0 {
            hull.pop();
        }
        hull.push(pt);
    }
    hull.pop();
    hull
}

/// A rotated rectangle: its centre, the unit vector `u` along its long side (pointing the way text reads, see
/// [`Rect::oriented`]), `v` across it, and the two lengths (`len_u >= len_v`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub center: P,
    pub u: P,
    pub v: P,
    pub len_u: f32,
    pub len_v: f32,
}

impl Rect {
    /// The corners in reading order: top-left, top-right, bottom-right, bottom-left.
    pub fn corners(&self) -> [P; 4] {
        let (hu, hv) = (self.len_u / 2.0, self.len_v / 2.0);
        let at = |a: f32, b: f32| {
            (
                self.center.0 + self.u.0 * a + self.v.0 * b,
                self.center.1 + self.u.1 * a + self.v.1 * b,
            )
        };
        [at(-hu, -hv), at(hu, -hv), at(hu, hv), at(-hu, hv)]
    }

    pub fn area(&self) -> f32 {
        self.len_u * self.len_v
    }

    /// The same rectangle grown by `d` on every side.
    pub fn grown(&self, d: f32) -> Rect {
        Rect {
            len_u: self.len_u + 2.0 * d,
            len_v: self.len_v + 2.0 * d,
            ..*self
        }
    }

    pub fn contains(&self, p: P) -> bool {
        let (dx, dy) = (p.0 - self.center.0, p.1 - self.center.1);
        let a = dx * self.u.0 + dy * self.u.1;
        let b = dx * self.v.0 + dy * self.v.1;
        a.abs() <= self.len_u / 2.0 && b.abs() <= self.len_v / 2.0
    }

    /// Scale coordinates (not lengths' direction): x by `sx`, y by `sy` (both the same for an unstretched image).
    pub fn scaled(&self, s: f32) -> Rect {
        Rect {
            center: (self.center.0 * s, self.center.1 * s),
            len_u: self.len_u * s,
            len_v: self.len_v * s,
            ..*self
        }
    }
}

/// The smallest rectangle around a convex hull: one side of it lies along an edge of the hull (rotating-calipers, tried per edge).
pub fn min_area_rect(hull: &[P]) -> Option<Rect> {
    if hull.len() < 3 {
        return None;
    }
    let mut best: Option<(f32, Rect)> = None;
    for i in 0..hull.len() {
        let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
        let (ex, ey) = (b.0 - a.0, b.1 - a.1);
        let l = (ex * ex + ey * ey).sqrt();
        if l < 1e-6 {
            continue;
        }
        let e = (ex / l, ey / l);
        let n = (-e.1, e.0);
        let (mut min_e, mut max_e, mut min_n, mut max_n) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in hull {
            let (pe, pn) = (p.0 * e.0 + p.1 * e.1, p.0 * n.0 + p.1 * n.1);
            min_e = min_e.min(pe);
            max_e = max_e.max(pe);
            min_n = min_n.min(pn);
            max_n = max_n.max(pn);
        }
        let (le, ln) = (max_e - min_e, max_n - min_n);
        let area = le * ln;
        if best.as_ref().is_none_or(|(a, _)| area < *a) {
            let (ce, cn) = ((min_e + max_e) / 2.0, (min_n + max_n) / 2.0);
            let center = (ce * e.0 + cn * n.0, ce * e.1 + cn * n.1);
            let rect = if le >= ln {
                Rect {
                    center,
                    u: e,
                    v: n,
                    len_u: le,
                    len_v: ln,
                }
            } else {
                Rect {
                    center,
                    u: n,
                    v: (-e.0, -e.1),
                    len_u: ln,
                    len_v: le,
                }
            };
            best = Some((area, rect));
        }
    }
    best.map(|(_, r)| r.oriented())
}

impl Rect {
    /// Point `u` the way a line is read and make `v` the next line's direction: a mostly horizontal rectangle reads left to
    /// right, a mostly vertical one top to bottom (what PP-OCR assumes of tall boxes), and `v` points down (a right-handed
    /// frame in image coordinates).
    pub fn oriented(mut self) -> Rect {
        let flip = if self.u.0.abs() >= self.u.1.abs() {
            self.u.0 < 0.0
        } else {
            self.u.1 < 0.0
        };
        if flip {
            self.u = (-self.u.0, -self.u.1);
        }
        self.v = (-self.u.1, self.u.0);
        self
    }
}

/// Bilinear sample of an RGB8 image at (x, y) (pixel centres at +0.5), clamped at the border. BGR order out, as PP-OCR's
/// models were trained on OpenCV images.
pub fn sample_bgr(rgb: &[u8], w: usize, h: usize, x: f32, y: f32) -> [f32; 3] {
    let (fx, fy) = (
        (x - 0.5).clamp(0.0, (w - 1) as f32),
        (y - 0.5).clamp(0.0, (h - 1) as f32),
    );
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let px = |xx: usize, yy: usize, c: usize| f32::from(rgb[(yy * w + xx) * 3 + c]);
    let mut out = [0f32; 3];
    for (o, c) in out.iter_mut().zip([2usize, 1, 0]) {
        let top = px(x0, y0, c) * (1.0 - tx) + px(x1, y0, c) * tx;
        let bot = px(x0, y1, c) * (1.0 - tx) + px(x1, y1, c) * tx;
        *o = top * (1.0 - ty) + bot * ty;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_split_touching_diagonals_but_not_gaps() {
        // x . . x x
        // . x . . .
        // . . . x .
        let bits = [
            true, false, false, true, true, false, true, false, false, false, false, false, false, true, false,
        ];
        let c = components(&bits, 5, 3, 100);
        assert_eq!(c.len(), 3, "{c:?}");
        let sizes: Vec<usize> = c.iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![2, 2, 1]);
        assert_eq!(components(&bits, 5, 3, 2).len(), 2, "capped");
    }

    #[test]
    fn hull_of_a_square_with_inside_points() {
        let pts = [
            (0.0, 0.0),
            (4.0, 0.0),
            (4.0, 4.0),
            (0.0, 4.0),
            (2.0, 2.0),
            (1.0, 3.0),
            (2.0, 0.0),
        ];
        let h = convex_hull(&pts);
        assert_eq!(h.len(), 4);
        assert!(convex_hull(&[(1.0, 1.0)]).len() == 1);
    }

    #[test]
    fn min_rect_of_an_axis_aligned_box_and_of_a_tilted_one() {
        let hull = convex_hull(&[(0.0, 0.0), (10.0, 0.0), (10.0, 2.0), (0.0, 2.0)]);
        let r = min_area_rect(&hull).unwrap();
        assert!((r.len_u - 10.0).abs() < 1e-3 && (r.len_v - 2.0).abs() < 1e-3, "{r:?}");
        assert!((r.u.0 - 1.0).abs() < 1e-3, "reads left to right: {r:?}");
        // The same box turned 30 degrees.
        let (s, c) = (30f32.to_radians().sin(), 30f32.to_radians().cos());
        let rot = |p: P| (p.0 * c - p.1 * s, p.0 * s + p.1 * c);
        let pts: Vec<P> = [(0.0, 0.0), (10.0, 0.0), (10.0, 2.0), (0.0, 2.0)]
            .into_iter()
            .map(rot)
            .collect();
        let r = min_area_rect(&convex_hull(&pts)).unwrap();
        assert!((r.len_u - 10.0).abs() < 1e-2 && (r.len_v - 2.0).abs() < 1e-2, "{r:?}");
        assert!((r.u.0 - c).abs() < 1e-3 && (r.u.1 - s).abs() < 1e-3, "{r:?}");
    }

    #[test]
    fn a_tall_box_reads_top_to_bottom_and_corners_follow_reading_order() {
        let hull = convex_hull(&[(0.0, 0.0), (2.0, 0.0), (2.0, 10.0), (0.0, 10.0)]);
        let r = min_area_rect(&hull).unwrap();
        assert!(r.u.1 > 0.99, "{r:?}");
        let c = r.corners();
        // First corner is where the line starts (top), the second is its far end.
        assert!(c[0].1 < c[1].1);
        assert!(r.contains((1.0, 5.0)) && !r.contains((5.0, 5.0)));
    }

    #[test]
    fn growing_and_scaling() {
        let r = Rect {
            center: (10.0, 10.0),
            u: (1.0, 0.0),
            v: (0.0, 1.0),
            len_u: 8.0,
            len_v: 2.0,
        };
        let g = r.grown(1.0);
        assert_eq!((g.len_u, g.len_v), (10.0, 4.0));
        let s = r.scaled(2.0);
        assert_eq!((s.center, s.len_u, s.len_v), ((20.0, 20.0), 16.0, 4.0));
    }

    #[test]
    fn sampling_is_bilinear_bgr() {
        // 2x1 image: left red, right blue.
        let rgb = [255, 0, 0, 0, 0, 255];
        let mid = sample_bgr(&rgb, 2, 1, 1.0, 0.5);
        assert!(
            (mid[0] - 127.5).abs() < 1e-3 && (mid[2] - 127.5).abs() < 1e-3,
            "{mid:?}"
        );
        let left = sample_bgr(&rgb, 2, 1, 0.0, 0.5);
        assert_eq!(left, [0.0, 0.0, 255.0], "BGR: red is the third channel");
    }
}
