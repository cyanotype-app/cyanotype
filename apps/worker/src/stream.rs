//! Streaming downscale: pixels arrive in blocks (PNG rows, HEIF tiles, a libheif plane) and are box-averaged by an
//! integer factor into a small buffer as they come, so the source image never exists in memory at full size.
//!
//! The factor is `floor(long edge / target)`, so the result is at least the target size (between 1x and 2x of it) and
//! the usual Lanczos resize then takes it the rest of the way. Averaging is per pixel, so it does not matter where a
//! tile boundary falls, and there are no seams. Alpha is averaged premultiplied.
use anyhow::{Context, Result, ensure};
use image::{DynamicImage, RgbImage, RgbaImage};

/// A `w` x `h` block of 8-bit RGB (or RGBA, as the accumulator was created) pixels whose top-left corner is at
/// `(x0, y0)` in the source (negative when the block starts outside it). `stride` is the byte distance between rows.
#[derive(Clone, Copy)]
pub struct Block<'a> {
    pub x0: i64,
    pub y0: i64,
    pub w: u32,
    pub h: u32,
    pub data: &'a [u8],
    pub stride: usize,
    pub premultiplied: bool,
}

pub struct Accum {
    src_w: u32,
    src_h: u32,
    k: u32,
    out_w: u32,
    out_h: u32,
    alpha: bool,
    /// k == 1: the image itself (RGB or RGBA, 8-bit), written straight through.
    direct: Vec<u8>,
    /// k > 1: per output cell, the sum of channel*alpha (colour) and of alpha, or plain channel sums without alpha.
    sums: Vec<u32>,
    /// The direct buffer holds premultiplied pixels (undone at the end).
    premultiplied: bool,
}

/// Box factor that keeps `long / k >= target`.
pub fn factor_for(w: u32, h: u32, target: u32) -> u32 {
    (w.max(h) / target.max(1)).max(1)
}

impl Accum {
    pub fn new(src_w: u32, src_h: u32, target: u32, alpha: bool) -> Result<Self> {
        ensure!(src_w > 0 && src_h > 0, "empty image");
        let k = factor_for(src_w, src_h, target);
        let (out_w, out_h) = (src_w.div_ceil(k), src_h.div_ceil(k));
        let ch = if alpha { 4 } else { 3 };
        let cells = out_w as usize * out_h as usize * ch;
        let (direct, sums) = if k == 1 {
            (vec![0u8; cells], Vec::new())
        } else {
            (Vec::new(), vec![0u32; cells])
        };
        Ok(Self {
            src_w,
            src_h,
            k,
            out_w,
            out_h,
            alpha,
            direct,
            sums,
            premultiplied: false,
        })
    }

    /// Bytes of the accumulation buffer for these parameters (for memory estimates).
    pub fn bytes_for(src_w: u32, src_h: u32, target: u32, alpha: bool) -> u64 {
        let k = factor_for(src_w, src_h, target);
        let cells = u64::from(src_w.div_ceil(k)) * u64::from(src_h.div_ceil(k)) * if alpha { 4 } else { 3 };
        cells * if k == 1 { 1 } else { 4 }
    }

    #[cfg(test)]
    pub fn factor(&self) -> u32 {
        self.k
    }

    /// Add a block of pixels. Parts outside the image are ignored.
    pub fn add_block(&mut self, b: &Block) {
        let Block {
            x0,
            y0,
            w,
            h,
            data,
            stride,
            premultiplied,
        } = *b;
        let ch = if self.alpha { 4 } else { 3 };
        // Clip to the image.
        let cx0 = (-x0).max(0) as u32;
        let cy0 = (-y0).max(0) as u32;
        let cx1 = (i64::from(self.src_w) - x0).clamp(0, i64::from(w)) as u32;
        let cy1 = (i64::from(self.src_h) - y0).clamp(0, i64::from(h)) as u32;
        if cx0 >= cx1 || cy0 >= cy1 {
            return;
        }
        for ty in cy0..cy1 {
            let row_start = ty as usize * stride;
            let row = &data[row_start + cx0 as usize * ch..row_start + cx1 as usize * ch];
            let sy = (y0 + i64::from(ty)) as u32;
            let sx = (x0 + i64::from(cx0)) as u32;
            self.add_row_at(sx, sy, row, premultiplied);
        }
    }

    /// One row of pixels starting at source column `sx`, row `sy`.
    fn add_row_at(&mut self, sx: u32, sy: u32, row: &[u8], premultiplied: bool) {
        let ch = if self.alpha { 4 } else { 3 };
        if self.k == 1 {
            let at = (sy as usize * self.out_w as usize + sx as usize) * ch;
            self.direct[at..at + row.len()].copy_from_slice(row);
            self.premultiplied |= premultiplied;
            return;
        }
        let k = self.k;
        let cy = sy / k;
        let base = cy as usize * self.out_w as usize * ch;
        let mut cx = sx / k;
        let mut in_cell = sx % k;
        if !self.alpha {
            for px in row.as_chunks::<3>().0 {
                let c = &mut self.sums[base + cx as usize * 3..][..3];
                c[0] += u32::from(px[0]);
                c[1] += u32::from(px[1]);
                c[2] += u32::from(px[2]);
                in_cell += 1;
                if in_cell == k {
                    in_cell = 0;
                    cx += 1;
                }
            }
        } else {
            for px in row.as_chunks::<4>().0 {
                let a = u32::from(px[3]);
                // Colour weighted by alpha; premultiplied input already carries the weight (x255 to match).
                let (wr, wg, wb) = if premultiplied {
                    (u32::from(px[0]) * 255, u32::from(px[1]) * 255, u32::from(px[2]) * 255)
                } else {
                    (u32::from(px[0]) * a, u32::from(px[1]) * a, u32::from(px[2]) * a)
                };
                let c = &mut self.sums[base + cx as usize * 4..][..4];
                c[0] += wr;
                c[1] += wg;
                c[2] += wb;
                c[3] += a;
                in_cell += 1;
                if in_cell == k {
                    in_cell = 0;
                    cx += 1;
                }
            }
        }
    }

    /// Add a full-width row `sy` (pixels `0..src_w`).
    pub fn add_row(&mut self, sy: u32, row: &[u8], premultiplied: bool) {
        self.add_row_at(0, sy, row, premultiplied);
    }

    pub fn finish(mut self) -> Result<DynamicImage> {
        let (w, h) = (self.out_w, self.out_h);
        if self.k == 1 {
            let mut buf = std::mem::take(&mut self.direct);
            if self.alpha {
                if self.premultiplied {
                    unpremultiply(&mut buf);
                }
                return Ok(DynamicImage::ImageRgba8(
                    RgbaImage::from_raw(w, h, buf).context("rgba buffer")?,
                ));
            }
            return Ok(DynamicImage::ImageRgb8(
                RgbImage::from_raw(w, h, buf).context("rgb buffer")?,
            ));
        }
        let k = self.k;
        let ch = if self.alpha { 4 } else { 3 };
        let mut out = Vec::with_capacity(w as usize * h as usize * ch);
        for cy in 0..h {
            let rows = k.min(self.src_h - cy * k);
            for cx in 0..w {
                let n = rows * k.min(self.src_w - cx * k);
                let c = &self.sums[(cy as usize * w as usize + cx as usize) * ch..][..ch];
                if self.alpha {
                    let sa = c[3];
                    for &v in &c[..3] {
                        out.push((v + sa / 2).checked_div(sa).unwrap_or(0).min(255) as u8);
                    }
                    out.push(((sa + n / 2) / n) as u8);
                } else {
                    for &v in c {
                        out.push(((v + n / 2) / n) as u8);
                    }
                }
            }
        }
        self.sums = Vec::new();
        if self.alpha {
            Ok(DynamicImage::ImageRgba8(
                RgbaImage::from_raw(w, h, out).context("rgba buffer")?,
            ))
        } else {
            Ok(DynamicImage::ImageRgb8(
                RgbImage::from_raw(w, h, out).context("rgb buffer")?,
            ))
        }
    }
}

pub fn unpremultiply(rgba: &mut [u8]) {
    for p in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        if a != 0 && a != 255 {
            for c in &mut p[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factor_keeps_target_size() {
        assert_eq!(factor_for(8064, 6048, 1440), 5);
        assert_eq!(factor_for(2000, 1000, 1440), 1);
        assert_eq!(factor_for(2879, 10, 1440), 1);
        assert_eq!(factor_for(2880, 10, 1440), 2);
    }

    #[test]
    fn block_order_and_tile_boundaries_do_not_matter() {
        let (w, h) = (101u32, 67u32);
        let px = |x: u32, y: u32| [(x * 2) as u8, (y * 3) as u8, (x + y) as u8];
        let mut whole = vec![0u8; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                whole[((y * w + x) * 3) as usize..][..3].copy_from_slice(&px(x, y));
            }
        }
        let mut a = Accum::new(w, h, 20, false).unwrap();
        assert_eq!(a.factor(), 5);
        a.add_block(&Block {
            x0: 0,
            y0: 0,
            w,
            h,
            data: &whole,
            stride: (w * 3) as usize,
            premultiplied: false,
        });
        let a = a.finish().unwrap();

        // The same image in 16x16 tiles, delivered bottom-up with overhanging edge tiles.
        let mut b = Accum::new(w, h, 20, false).unwrap();
        for ty in (0..h.div_ceil(16)).rev() {
            for tx in 0..w.div_ceil(16) {
                let mut tile = vec![0u8; 16 * 16 * 3];
                for y in 0..16 {
                    for x in 0..16 {
                        tile[((y * 16 + x) * 3) as usize..][..3].copy_from_slice(&px(tx * 16 + x, ty * 16 + y));
                    }
                }
                b.add_block(&Block {
                    x0: i64::from(tx * 16),
                    y0: i64::from(ty * 16),
                    w: 16,
                    h: 16,
                    data: &tile,
                    stride: 48,
                    premultiplied: false,
                });
            }
        }
        assert_eq!(a.to_rgb8().as_raw(), b.finish().unwrap().to_rgb8().as_raw());
    }

    #[test]
    fn alpha_is_averaged_premultiplied() {
        // One opaque red and one fully transparent green pixel average to opaque-ish red, not to brown.
        let row = [255, 0, 0, 255, 0, 255, 0, 0];
        let mut a = Accum::new(2, 1, 1, true).unwrap();
        assert_eq!(a.factor(), 2);
        a.add_row(0, &row, false);
        let img = a.finish().unwrap().to_rgba8();
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 128]);
    }

    #[test]
    fn premultiplied_input_gives_the_same_result() {
        let straight = [200, 100, 50, 128, 10, 20, 30, 255];
        let pre = [100, 50, 25, 128, 10, 20, 30, 255];
        let mut a = Accum::new(2, 1, 1, true).unwrap();
        a.add_row(0, &straight, false);
        let mut b = Accum::new(2, 1, 1, true).unwrap();
        b.add_row(0, &pre, true);
        let (a, b) = (a.finish().unwrap().to_rgba8(), b.finish().unwrap().to_rgba8());
        for (x, y) in a.get_pixel(0, 0).0.iter().zip(b.get_pixel(0, 0).0) {
            assert!(
                (i32::from(*x) - i32::from(y)).abs() <= 1,
                "{:?} vs {:?}",
                a.get_pixel(0, 0),
                b.get_pixel(0, 0)
            );
        }
    }
}
