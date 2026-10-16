//! Tile access to grid HEIC/AVIF images through libheif's C API (`heif_tiling.h`, libheif >= 1.20).
//!
//! `libheif-rs` 3.0 wraps neither `heif_image_handle_get_image_tiling` nor `heif_image_handle_decode_image_tile`, and
//! keeps its raw handle pointers private, so this opens the file a second time through `libheif-sys` (same statically
//! linked libheif, same security limits). That is cheap: opening parses boxes, it does not decode pixels.
//!
//! Everything unsafe in the HEIF path lives here. Each wrapper owns its libheif object and frees it on drop.
use std::{
    ffi::{CStr, c_int, c_void},
    marker::PhantomData,
    ptr::{self, NonNull},
};

use anyhow::{Result, bail};
use libheif_sys as lh;

/// Mirrors `struct heif_image_tiling` (version 1) from `heif_tiling.h`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct RawTiling {
    version: c_int,
    num_columns: u32,
    num_rows: u32,
    tile_width: u32,
    tile_height: u32,
    image_width: u32,
    image_height: u32,
    top_offset: u32,
    left_offset: u32,
    number_of_extra_dimensions: u8,
    extra_dimension_size: [u32; 8],
}

unsafe extern "C" {
    fn heif_image_handle_get_image_tiling(
        handle: *const lh::heif_image_handle,
        process_image_transformations: c_int,
        out: *mut RawTiling,
    ) -> lh::heif_error;

    fn heif_image_handle_decode_image_tile(
        handle: *const lh::heif_image_handle,
        out_img: *mut *mut lh::heif_image,
        colorspace: lh::heif_colorspace,
        chroma: lh::heif_chroma,
        options: *const lh::heif_decoding_options,
        tile_x: u32,
        tile_y: u32,
    ) -> lh::heif_error;
}

fn check(err: lh::heif_error, what: &str) -> Result<()> {
    if err.code == lh::heif_error_code_heif_error_Ok {
        return Ok(());
    }
    // SAFETY: libheif's error messages are static NUL-terminated strings.
    let msg = if err.message.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(err.message) }.to_string_lossy().into_owned()
    };
    bail!("{what}: {msg} (code {}, subcode {})", err.code, err.subcode)
}

/// The tile layout of the primary image, in display coordinates (irot/imir/clap already applied).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tiling {
    pub columns: u32,
    pub rows: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    /// Display size of the whole image.
    pub width: u32,
    pub height: u32,
    /// How far the first tile column / row sticks out past the image's left / top edge, in pixels (0 for an upright
    /// grid; non-zero when a rotation or mirror moved the grid's overhang from the right/bottom to the left/top).
    ///
    /// libheif's `heif_image_tiling` calls this `left_offset`/`top_offset`, but what it stores is the number of
    /// *visible* pixels in that first tile (`image_size % tile_size`, rotated like the image), so the shift is the rest
    /// of the tile. The grid check in the tests (tile-by-tile against libheif's own whole-image decode, with irot and
    /// imir) is what pins this down.
    pub left_shift: u32,
    pub top_shift: u32,
}

impl Tiling {
    pub fn tile_count(&self) -> u64 {
        u64::from(self.columns) * u64::from(self.rows)
    }
}

/// The primary image of a file, opened for tile access.
pub struct TileReader<'a> {
    ctx: NonNull<lh::heif_context>,
    handle: NonNull<lh::heif_image_handle>,
    opts: NonNull<lh::heif_decoding_options>,
    /// The context reads the file in place; keep the borrow alive.
    _bytes: PhantomData<&'a [u8]>,
}

impl<'a> TileReader<'a> {
    pub fn open(bytes: &'a [u8], max_pixels: u64, max_total_memory: u64) -> Result<Self> {
        // SAFETY: plain libheif C API use; every pointer is checked and owned by the returned value (or freed on the
        // error paths below).
        unsafe {
            let ctx =
                NonNull::new(lh::heif_context_alloc()).ok_or_else(|| anyhow::anyhow!("libheif: out of memory"))?;
            let free_ctx = |e: anyhow::Error| {
                lh::heif_context_free(ctx.as_ptr());
                e
            };

            let limits = lh::heif_context_get_security_limits(ctx.as_ptr());
            if limits.is_null() {
                return Err(free_ctx(anyhow::anyhow!("libheif: no security limits")));
            }
            let mut l = *limits;
            l.max_image_size_pixels = max_pixels;
            l.max_total_memory = max_total_memory;
            if let Err(e) = check(
                lh::heif_context_set_security_limits(ctx.as_ptr(), &l),
                "set libheif limits",
            ) {
                return Err(free_ctx(e));
            }
            lh::heif_context_set_max_decoding_threads(ctx.as_ptr(), 1);
            if let Err(e) = check(
                lh::heif_context_read_from_memory_without_copy(
                    ctx.as_ptr(),
                    bytes.as_ptr().cast::<c_void>(),
                    bytes.len(),
                    ptr::null(),
                ),
                "read HEIF",
            ) {
                return Err(free_ctx(e));
            }
            let mut handle: *mut lh::heif_image_handle = ptr::null_mut();
            if let Err(e) = check(
                lh::heif_context_get_primary_image_handle(ctx.as_ptr(), &mut handle),
                "primary image",
            ) {
                return Err(free_ctx(e));
            }
            let Some(handle) = NonNull::new(handle) else {
                return Err(free_ctx(anyhow::anyhow!("no primary image")));
            };
            let opts = lh::heif_decoding_options_alloc();
            let Some(opts) = NonNull::new(opts) else {
                lh::heif_image_handle_release(handle.as_ptr());
                return Err(free_ctx(anyhow::anyhow!("libheif: out of memory")));
            };
            (*opts.as_ptr()).ignore_transformations = 0; // tile positions and pixels in display orientation
            (*opts.as_ptr()).convert_hdr_to_8bit = 1;
            Ok(Self {
                ctx,
                handle,
                opts,
                _bytes: PhantomData,
            })
        }
    }

    /// The tile layout in display coordinates. `Err` when libheif cannot describe one.
    pub fn tiling(&self) -> Result<Tiling> {
        let mut t = RawTiling {
            version: 1,
            ..Default::default()
        };
        // SAFETY: valid handle; `t` matches the C struct.
        check(
            unsafe { heif_image_handle_get_image_tiling(self.handle.as_ptr(), 1, &mut t) },
            "image tiling",
        )?;
        if t.number_of_extra_dimensions != 0 {
            bail!("volumetric image");
        }
        if t.left_offset >= t.tile_width.max(1) || t.top_offset >= t.tile_height.max(1) {
            bail!("tile offsets outside the first tile");
        }
        let shift = |visible: u32, tile: u32| if visible == 0 { 0 } else { tile - visible };
        Ok(Tiling {
            columns: t.num_columns,
            rows: t.num_rows,
            tile_width: t.tile_width,
            tile_height: t.tile_height,
            width: t.image_width,
            height: t.image_height,
            left_shift: shift(t.left_offset, t.tile_width),
            top_shift: shift(t.top_offset, t.tile_height),
        })
    }

    /// Decode one tile (display coordinates, tile indices) to interleaved 8-bit RGB or RGBA.
    pub fn decode_tile(&self, x: u32, y: u32, alpha: bool) -> Result<Tile> {
        let chroma = if alpha {
            lh::heif_chroma_heif_chroma_interleaved_RGBA
        } else {
            lh::heif_chroma_heif_chroma_interleaved_RGB
        };
        let mut img: *mut lh::heif_image = ptr::null_mut();
        // SAFETY: valid handle/options; `img` is owned by the returned Tile.
        check(
            unsafe {
                heif_image_handle_decode_image_tile(
                    self.handle.as_ptr(),
                    &mut img,
                    lh::heif_colorspace_heif_colorspace_RGB,
                    chroma,
                    self.opts.as_ptr(),
                    x,
                    y,
                )
            },
            "decode tile",
        )?;
        let Some(img) = NonNull::new(img) else {
            bail!("libheif returned no tile");
        };
        Ok(Tile { img, alpha })
    }
}

impl Drop for TileReader<'_> {
    fn drop(&mut self) {
        // SAFETY: each pointer was acquired in `open` and is released exactly once, handle before context.
        unsafe {
            lh::heif_decoding_options_free(self.opts.as_ptr());
            lh::heif_image_handle_release(self.handle.as_ptr());
            lh::heif_context_free(self.ctx.as_ptr());
        }
    }
}

/// One decoded tile.
pub struct Tile {
    img: NonNull<lh::heif_image>,
    alpha: bool,
}

/// A borrowed view of a tile's pixels.
pub struct TilePixels<'a> {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub data: &'a [u8],
    pub premultiplied: bool,
}

impl Tile {
    pub fn pixels(&self) -> Result<TilePixels<'_>> {
        let channels = if self.alpha { 4usize } else { 3 };
        let ch = lh::heif_channel_heif_channel_interleaved;
        // SAFETY: valid image; the slice borrows from `self`, which owns the image until dropped.
        unsafe {
            let w = lh::heif_image_get_width(self.img.as_ptr(), ch).max(0) as usize;
            let h = lh::heif_image_get_height(self.img.as_ptr(), ch).max(0) as usize;
            if lh::heif_image_get_bits_per_pixel_range(self.img.as_ptr(), ch) != 8 {
                bail!("unexpected tile pixel format");
            }
            let mut stride = 0usize;
            let p = lh::heif_image_get_plane_readonly2(self.img.as_ptr(), ch, &mut stride);
            if p.is_null() || w == 0 || h == 0 || stride < w * channels {
                bail!("decoder returned no pixel plane for a tile");
            }
            let len = stride * (h - 1) + w * channels;
            Ok(TilePixels {
                width: w as u32,
                height: h as u32,
                stride,
                data: std::slice::from_raw_parts(p, len),
                premultiplied: lh::heif_image_is_premultiplied_alpha(self.img.as_ptr()) != 0,
            })
        }
    }
}

impl Drop for Tile {
    fn drop(&mut self) {
        // SAFETY: owned image, released once.
        unsafe { lh::heif_image_release(self.img.as_ptr()) }
    }
}
