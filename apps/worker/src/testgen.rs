//! Synthetic test images, generated row by row so even 200 MP fixtures never exist as a raster in memory.
use jpeg_encoder::{Encoder, ImageBuffer, JpegColorType, SamplingFactor};

/// A photo-like test pattern: smooth gradients, a few large shapes and mild texture, so it compresses like a picture
/// rather than like noise or a flat colour.
pub fn pixel(x: u32, y: u32, w: u32, h: u32) -> [u8; 3] {
    let (fx, fy) = (f64::from(x) / f64::from(w), f64::from(y) / f64::from(h));
    let hash = (x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA6B)).wrapping_mul(0xC2B2_AE35) >> 28;
    let tex = hash as f64 - 8.0;
    let ring = ((fx - 0.5).hypot((fy - 0.5) * f64::from(h) / f64::from(w)) * 14.0).sin() * 30.0;
    let r = 40.0 + 170.0 * fx + ring + tex;
    let g = 30.0 + 180.0 * fy - ring * 0.5 + tex;
    let b = 90.0 + 90.0 * (1.0 - fx) * (0.5 + 0.5 * (fy * 9.0).sin()) + tex;
    [
        r.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        b.clamp(0.0, 255.0) as u8,
    ]
}

struct Synth {
    w: u16,
    h: u16,
    cmyk: bool,
    ycck: bool,
}

impl ImageBuffer for Synth {
    fn get_jpeg_color_type(&self) -> JpegColorType {
        match (self.cmyk, self.ycck) {
            (_, true) => JpegColorType::Ycck,
            (true, false) => JpegColorType::Cmyk,
            _ => JpegColorType::Ycbcr,
        }
    }
    fn width(&self) -> u16 {
        self.w
    }
    fn height(&self) -> u16 {
        self.h
    }
    fn fill_buffers(&self, y: u16, buffers: &mut [Vec<u8>; 4]) {
        for x in 0..self.w {
            let [r, g, b] = pixel(u32::from(x), u32::from(y), u32::from(self.w), u32::from(self.h));
            if self.cmyk && !self.ycck {
                // Adobe-style inverted CMYK, as Photoshop writes it: stored value = 255 - ink.
                buffers[0].push(r);
                buffers[1].push(g);
                buffers[2].push(b);
                buffers[3].push(255);
            } else {
                let (r, g, b) = if self.ycck {
                    // Photoshop's inverted CMY (255 - ink) is what the YCC transform gets: ink = 255 - r here.
                    (f32::from(255 - r), f32::from(255 - g), f32::from(255 - b))
                } else {
                    (f32::from(r), f32::from(g), f32::from(b))
                };
                if self.ycck {
                    // Adobe YCCK: the YCbCr transform of the (inverted) CMY, plus the inverted K.
                    buffers[3].push(255);
                }
                let yy = 0.299 * r + 0.587 * g + 0.114 * b;
                let cb = 128.0 - 0.168_736 * r - 0.331_264 * g + 0.5 * b;
                let cr = 128.0 + 0.5 * r - 0.418_688 * g - 0.081_312 * b;
                buffers[0].push(yy.round().clamp(0.0, 255.0) as u8);
                buffers[1].push(cb.round().clamp(0.0, 255.0) as u8);
                buffers[2].push(cr.round().clamp(0.0, 255.0) as u8);
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct JpegSpec {
    pub w: u16,
    pub h: u16,
    pub progressive: bool,
    pub quality: u8,
    /// 4:2:0 chroma subsampling (the camera default) instead of 4:4:4.
    pub subsample: bool,
    pub restart_interval: u16,
    pub cmyk: bool,
    /// With `cmyk`: store as Adobe YCCK instead of plain CMYK.
    pub ycck: bool,
}

impl JpegSpec {
    pub fn new(w: u16, h: u16) -> Self {
        Self {
            w,
            h,
            progressive: false,
            quality: 85,
            subsample: true,
            restart_interval: 0,
            cmyk: false,
            ycck: false,
        }
    }
}

pub fn jpeg(spec: JpegSpec) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = Encoder::new(&mut out, spec.quality);
    enc.set_progressive(spec.progressive);
    enc.set_restart_interval(spec.restart_interval);
    enc.set_sampling_factor(if spec.subsample {
        SamplingFactor::F_2_2
    } else {
        SamplingFactor::F_1_1
    });
    enc.encode_image(Synth {
        w: spec.w,
        h: spec.h,
        cmyk: spec.cmyk,
        ycck: spec.ycck,
    })
    .expect("encode synthetic jpeg");
    out
}

/// Peak signal-to-noise ratio between two same-sized RGB images, in dB (infinite when identical).
pub fn psnr(a: &image::RgbImage, b: &image::RgbImage) -> f64 {
    assert_eq!(
        (a.width(), a.height()),
        (b.width(), b.height()),
        "psnr needs equal sizes"
    );
    let sq: f64 = a
        .as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(&x, &y)| (f64::from(x) - f64::from(y)).powi(2))
        .sum();
    if sq == 0.0 {
        return f64::INFINITY;
    }
    let mse = sq / a.as_raw().len() as f64;
    10.0 * (255.0f64 * 255.0 / mse).log10()
}

// ---- HEIF grid images ---------------------------------------------------------------------------------------------
//
// `heif-enc` only writes single-image HEICs, and the statically linked libheif has no encoder, so grid fixtures are
// assembled here: each tile is a one-tile HEIC from `heif-enc` whose HEVC stream and `hvcC` are lifted out and put into
// a `grid` item, the way iPhones store large photos.

fn box_at(b: &[u8], pos: usize) -> Option<(usize, [u8; 4], usize)> {
    let size = u32::from_be_bytes(b.get(pos..pos + 4)?.try_into().ok()?) as usize;
    let ty: [u8; 4] = b.get(pos + 4..pos + 8)?.try_into().ok()?;
    let (hdr, end) = match size {
        0 => (8, b.len()),
        1 => (
            16,
            u64::from_be_bytes(b.get(pos + 8..pos + 16)?.try_into().ok()?) as usize + pos,
        ),
        n => (8, pos + n),
    };
    (end <= b.len() && end >= pos + hdr).then_some((pos + hdr, ty, end))
}

fn find_box(b: &[u8], start: usize, end: usize, want: &[u8; 4]) -> Option<(usize, usize, usize)> {
    let mut pos = start;
    while pos + 8 <= end {
        let (body, ty, bend) = box_at(&b[..end], pos)?;
        if &ty == want {
            return Some((pos, body, bend));
        }
        pos = bend;
    }
    None
}

fn be(b: &[u8], n: usize) -> usize {
    b[..n].iter().fold(0usize, |a, &x| a << 8 | usize::from(x))
}

/// The `hvcC` box (whole, header included) and the HEVC stream of the primary item of a single-image HEIC.
pub fn heic_tile_parts(heic: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let (_, meta_body, meta_end) = find_box(heic, 0, heic.len(), b"meta").expect("meta");
    let meta_body = meta_body + 4; // full box header
    let (_, pitm_body, _) = find_box(heic, meta_body, meta_end, b"pitm").expect("pitm");
    let pitm_ver = heic[pitm_body];
    let item = if pitm_ver == 0 {
        be(&heic[pitm_body + 4..], 2)
    } else {
        be(&heic[pitm_body + 4..], 4)
    };

    let (iprp, iprp_body, iprp_end) = find_box(heic, meta_body, meta_end, b"iprp").expect("iprp");
    let _ = iprp;
    let (hvcc_at, _, hvcc_end) = find_box(heic, iprp_body, iprp_end, b"ipco")
        .and_then(|(_, body, end)| find_box(heic, body, end, b"hvcC"))
        .expect("hvcC");
    let hvcc = heic[hvcc_at..hvcc_end].to_vec();

    let (_, iloc, _) = find_box(heic, meta_body, meta_end, b"iloc").expect("iloc");
    let ver = heic[iloc];
    let mut p = iloc + 4;
    let (off_sz, len_sz) = (usize::from(heic[p] >> 4), usize::from(heic[p] & 15));
    let (base_sz, idx_sz) = (
        usize::from(heic[p + 1] >> 4),
        if ver > 0 { usize::from(heic[p + 1] & 15) } else { 0 },
    );
    p += 2;
    let count = if ver < 2 {
        let c = be(&heic[p..], 2);
        p += 2;
        c
    } else {
        let c = be(&heic[p..], 4);
        p += 4;
        c
    };
    for _ in 0..count {
        let id = if ver < 2 {
            let v = be(&heic[p..], 2);
            p += 2;
            v
        } else {
            let v = be(&heic[p..], 4);
            p += 4;
            v
        };
        if ver > 0 {
            p += 2;
        } // construction method
        p += 2; // data reference index
        let base = be(&heic[p..], base_sz);
        p += base_sz;
        let extents = be(&heic[p..], 2);
        p += 2;
        let mut data = Vec::new();
        for _ in 0..extents {
            p += idx_sz;
            let (o, l) = (be(&heic[p..], off_sz), be(&heic[p + off_sz..], len_sz));
            p += off_sz + len_sz;
            if id == item {
                data.extend_from_slice(&heic[base + o..base + o + l]);
            }
        }
        if id == item {
            return (hvcc, data);
        }
    }
    panic!("primary item not in iloc");
}

fn mkbox(ty: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend(ty);
    v.extend(body);
    v
}

fn fullbox(ty: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![version];
    b.extend(&flags.to_be_bytes()[1..]);
    b.extend(body);
    mkbox(ty, &b)
}

/// Transformations stored with a grid image, in the order they apply.
#[derive(Clone, Copy, Default)]
pub struct GridTransform {
    /// `irot` angle in 90 degree steps counter-clockwise (1..=3), or 0 for none.
    pub irot: u8,
    /// `imir` axis (0: top-bottom flip, 1: left-right), if mirrored.
    pub imir: Option<u8>,
}

/// Shape of a grid: `cols` x `rows` square tiles of `tile` pixels, cropped to `out_w` x `out_h`.
#[derive(Clone, Copy)]
pub struct GridSpec {
    pub cols: u32,
    pub rows: u32,
    pub tile_w: u32,
    pub tile_h: u32,
    pub out_w: u32,
    pub out_h: u32,
}

impl GridSpec {
    pub fn new(cols: u32, rows: u32, tile: u32, out_w: u32, out_h: u32) -> Self {
        Self {
            cols,
            rows,
            tile_w: tile,
            tile_h: tile,
            out_w,
            out_h,
        }
    }
}

/// A HEIC whose primary image is a `cols` x `rows` grid of `tile_w` x `tile_h` tiles cropped to `out_w` x `out_h`.
/// `tiles` holds `(hvcC box, HEVC stream)` per tile in row-major order, or one entry used for every tile.
pub fn heic_grid(tiles: &[(Vec<u8>, Vec<u8>)], spec: GridSpec, tf: GridTransform) -> Vec<u8> {
    let GridSpec {
        cols,
        rows,
        tile_w,
        tile_h,
        out_w,
        out_h,
    } = spec;
    let n = (cols * rows) as usize;
    assert!(tiles.len() == 1 || tiles.len() == n);
    let tile = |i: usize| &tiles[if tiles.len() == 1 { 0 } else { i }];
    let grid_id = n as u16 + 1;

    let mut desc = vec![0u8, 1, (rows - 1) as u8, (cols - 1) as u8]; // version 0, flags: 32-bit sizes
    desc.extend(out_w.to_be_bytes());
    desc.extend(out_h.to_be_bytes());

    let build = |data_start: usize| -> Vec<u8> {
        // Item data layout in mdat: tiles first, then the grid descriptor.
        let mut ext = Vec::new();
        let mut off = data_start;
        for i in 0..n {
            ext.push((off, tile(i).1.len()));
            off += tile(i).1.len();
        }
        ext.push((off, desc.len()));

        let mut iloc = vec![0x44, 0x00];
        iloc.extend((n as u16 + 1).to_be_bytes());
        for (i, (o, l)) in ext.iter().enumerate() {
            iloc.extend((i as u16 + 1).to_be_bytes());
            iloc.extend([0, 0]); // construction method 0
            iloc.extend([0, 0]); // data reference index
            iloc.extend(1u16.to_be_bytes());
            iloc.extend((*o as u32).to_be_bytes());
            iloc.extend((*l as u32).to_be_bytes());
        }

        let mut iinf = (n as u16 + 1).to_be_bytes().to_vec();
        for i in 1..=n as u16 {
            let mut infe = i.to_be_bytes().to_vec();
            infe.extend([0, 0]);
            infe.extend(b"hvc1");
            infe.push(0);
            iinf.extend(fullbox(b"infe", 2, 1, &infe)); // flags 1: hidden
        }
        let mut infe = grid_id.to_be_bytes().to_vec();
        infe.extend([0, 0]);
        infe.extend(b"grid");
        infe.push(0);
        iinf.extend(fullbox(b"infe", 2, 0, &infe));

        let mut dimg = grid_id.to_be_bytes().to_vec();
        dimg.extend((n as u16).to_be_bytes());
        for i in 1..=n as u16 {
            dimg.extend(i.to_be_bytes());
        }

        let ispe = |w: u32, h: u32| {
            let mut b = w.to_be_bytes().to_vec();
            b.extend(h.to_be_bytes());
            fullbox(b"ispe", 0, 0, &b)
        };
        let mut ipco = tile(0).0.clone(); // 1: hvcC
        ipco.extend(ispe(tile_w, tile_h)); // 2
        ipco.extend(ispe(out_w, out_h)); // 3
        let mut grid_assoc = vec![3u8];
        if tf.irot > 0 {
            ipco.extend(mkbox(b"irot", &[tf.irot & 3]));
            grid_assoc.push(0x80 | (ipco_count(&ipco)));
        }
        if let Some(axis) = tf.imir {
            ipco.extend(mkbox(b"imir", &[axis & 1]));
            grid_assoc.push(0x80 | (ipco_count(&ipco)));
        }
        let mut ipma = (n as u32 + 1).to_be_bytes().to_vec();
        for i in 1..=n as u16 {
            ipma.extend(i.to_be_bytes());
            ipma.extend([2, 0x81, 2]);
        }
        ipma.extend(grid_id.to_be_bytes());
        ipma.push(grid_assoc.len() as u8);
        ipma.extend(&grid_assoc);

        let mut hdlr = vec![0u8; 4];
        hdlr.extend(b"pict");
        hdlr.extend([0u8; 12]);
        hdlr.push(0);

        let mut meta = fullbox(b"hdlr", 0, 0, &hdlr);
        meta.extend(fullbox(b"pitm", 0, 0, &grid_id.to_be_bytes()));
        meta.extend(fullbox(b"iloc", 1, 0, &iloc));
        meta.extend(fullbox(b"iinf", 0, 0, &iinf));
        meta.extend(mkbox(b"iref", &fullbox_body_dimg(&dimg)));
        let mut iprp = mkbox(b"ipco", &ipco);
        iprp.extend(fullbox(b"ipma", 0, 0, &ipma));
        meta.extend(mkbox(b"iprp", &iprp));
        fullbox(b"meta", 0, 0, &meta)
    };

    let ftyp = mkbox(b"ftyp", &[&b"heic"[..], &[0, 0, 0, 0], b"mif1", b"heic"].concat());
    let meta_len = build(0).len();
    let data_start = ftyp.len() + meta_len + 8;
    let meta = build(data_start);
    assert_eq!(meta.len(), meta_len);

    let mut out = ftyp;
    out.extend(meta);
    let mut mdat = Vec::new();
    for i in 0..n {
        mdat.extend(&tile(i).1);
    }
    mdat.extend(&desc);
    out.extend(mkbox(b"mdat", &mdat));
    out
}

/// Number of property boxes in an `ipco` body (1-based index of the last one).
fn ipco_count(ipco: &[u8]) -> u8 {
    let (mut pos, mut n) = (0, 0u8);
    while pos + 8 <= ipco.len() {
        pos += u32::from_be_bytes(ipco[pos..pos + 4].try_into().unwrap()) as usize;
        n += 1;
    }
    n
}

/// `iref` v0 body holding one `dimg` reference box.
fn fullbox_body_dimg(dimg_body: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; 4]; // version 0, flags 0
    b.extend(mkbox(b"dimg", dimg_body));
    // `mkbox(b"iref", ..)` above wraps this; the full-box header bytes belong to iref itself.
    b
}

/// An iPhone-like grid HEIC of the given size: 512x512 tiles (one real tile repeated, so the file is small), optionally
/// with a quarter-turn `irot`.
pub fn grid_heic_of_size(w: u32, h: u32, rotated: bool) -> Vec<u8> {
    let tile = heic_tile_parts(include_bytes!("../tests/fixtures/tile512.heic"));
    let tf = GridTransform {
        irot: u8::from(rotated),
        imir: None,
    };
    heic_grid(&[tile], GridSpec::new(w.div_ceil(512), h.div_ceil(512), 512, w, h), tf)
}
