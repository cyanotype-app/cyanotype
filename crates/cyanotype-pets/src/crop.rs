//! The picture of one animal that its embedding is made from: the detector's box, made a little bigger and squarer, cut out of the
//! preview (`docs/pets.md`, "Telling individuals apart"). The numbers (padding, squareness) were chosen by the bake-off.

/// How much room is left around the box, as a fraction of the box's own width and height on each side.
pub const PAD: f32 = 0.10;

/// An RGB8 picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Rgb {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Cut the box `bbox` (left, top, width, height, fractions of the picture) out of `rgb` (`w` x `h`), `pad` bigger on every side. With
/// `square` the result is as close to a square as the picture allows (the shorter side grows around the box's centre, and the cut
/// slides to stay inside), so the model sees a dog and not a stretched one. `None` for a box that is nothing once inside the picture.
pub fn crop(rgb: &[u8], w: u32, h: u32, bbox: [f32; 4], pad: f32, square: bool) -> Option<Rgb> {
    if w == 0 || h == 0 || rgb.len() != w as usize * h as usize * 3 {
        return None;
    }
    let (fw, fh) = (w as f32, h as f32);
    let (bw, bh) = (bbox[2] * fw, bbox[3] * fh);
    let (cx, cy) = (bbox[0] * fw + bw / 2.0, bbox[1] * fh + bh / 2.0);
    let (mut cw, mut ch) = (bw * (1.0 + 2.0 * pad), bh * (1.0 + 2.0 * pad));
    if square {
        let side = cw.max(ch);
        (cw, ch) = (side, side);
    }
    cw = cw.min(fw);
    ch = ch.min(fh);
    let x0 = (cx - cw / 2.0).clamp(0.0, fw - cw);
    let y0 = (cy - ch / 2.0).clamp(0.0, fh - ch);
    let (x0, y0) = (x0.round() as u32, y0.round() as u32);
    let (cw, ch) = ((cw.round() as u32).min(w - x0), (ch.round() as u32).min(h - y0));
    if cw < 8 || ch < 8 {
        return None;
    }
    let mut data = Vec::with_capacity(cw as usize * ch as usize * 3);
    for y in y0..y0 + ch {
        let start = (y as usize * w as usize + x0 as usize) * 3;
        data.extend_from_slice(&rgb[start..start + cw as usize * 3]);
    }
    Some(Rgb {
        width: cw,
        height: ch,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture whose pixel (x, y) is (x, y, 0), so a cut shows where it came from.
    fn ramp(w: u32, h: u32) -> Vec<u8> {
        (0..h)
            .flat_map(|y| (0..w).flat_map(move |x| [x as u8, y as u8, 0]))
            .collect()
    }

    fn at(c: &Rgb, x: u32, y: u32) -> (u8, u8) {
        let i = (y as usize * c.width as usize + x as usize) * 3;
        (c.data[i], c.data[i + 1])
    }

    #[test]
    fn the_box_is_cut_out_with_its_padding() {
        let img = ramp(100, 80);
        // 20 x 20 px at (40, 30), no padding.
        let c = crop(&img, 100, 80, [0.4, 0.375, 0.2, 0.25], 0.0, false).unwrap();
        assert_eq!((c.width, c.height), (20, 20));
        assert_eq!(at(&c, 0, 0), (40, 30));
        // With 10 % on every side it is 24 x 24, starting 2 px earlier.
        let c = crop(&img, 100, 80, [0.4, 0.375, 0.2, 0.25], 0.1, false).unwrap();
        assert_eq!((c.width, c.height), (24, 24));
        assert_eq!(at(&c, 0, 0), (38, 28));
    }

    #[test]
    fn a_square_cut_grows_the_short_side_around_the_centre() {
        let img = ramp(100, 80);
        // 40 wide, 10 tall at (30, 35): square of 40 centred on (50, 40).
        let c = crop(&img, 100, 80, [0.3, 0.4375, 0.4, 0.125], 0.0, true).unwrap();
        assert_eq!((c.width, c.height), (40, 40));
        assert_eq!(at(&c, 0, 0), (30, 20));
    }

    #[test]
    fn a_cut_stays_inside_the_picture_and_slides_rather_than_shrinks() {
        let img = ramp(100, 80);
        // A tall box against the left edge: the square slides right instead of leaving the picture.
        let c = crop(&img, 100, 80, [0.0, 0.0, 0.1, 0.5], 0.0, true).unwrap();
        assert_eq!((c.width, c.height), (40, 40));
        assert_eq!(at(&c, 0, 0), (0, 0));
        // A box bigger than the picture is the picture.
        let c = crop(&img, 100, 80, [0.0, 0.0, 1.0, 1.0], 0.5, true).unwrap();
        assert_eq!((c.width, c.height), (100, 80));
    }

    #[test]
    fn nothing_is_cut_from_a_degenerate_box_or_buffer() {
        let img = ramp(100, 80);
        assert!(crop(&img, 100, 80, [0.5, 0.5, 0.0, 0.0], 0.0, false).is_none());
        assert!(crop(&img[..10], 100, 80, [0.0, 0.0, 1.0, 1.0], 0.0, false).is_none());
        assert!(crop(&img, 0, 80, [0.0, 0.0, 1.0, 1.0], 0.0, false).is_none());
    }
}
