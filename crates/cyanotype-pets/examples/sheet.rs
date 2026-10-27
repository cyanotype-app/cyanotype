//! Contact sheets for labelling the evaluation set by eye: `sheet OUT.jpg COLS TILE a.jpg b.jpg ...` writes the pictures in a grid,
//! each tile marked with the digits of its file name (`012.jpg` is marked 12), so a label can be read off the sheet. A tool for
//! looking at the set, not part of the product.
use image::{Rgb, RgbImage, imageops::FilterType};

const FONT: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

fn draw_number(img: &mut RgbImage, x0: u32, y0: u32, digits: &str, scale: u32) {
    let w = digits.len() as u32 * 4 * scale + scale;
    for y in 0..7 * scale {
        for x in 0..w {
            if x0 + x < img.width() && y0 + y < img.height() {
                img.put_pixel(x0 + x, y0 + y, Rgb([255, 255, 0]));
            }
        }
    }
    for (i, c) in digits.chars().enumerate() {
        let Some(d) = c.to_digit(10) else { continue };
        for (row, bits) in FONT[d as usize].iter().enumerate() {
            for col in 0..3 {
                if bits >> (2 - col) & 1 == 1 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let x = x0 + scale + (i as u32 * 4 + col) * scale + sx;
                            let y = y0 + scale + row as u32 * scale + sy;
                            if x < img.width() && y < img.height() {
                                img.put_pixel(x, y, Rgb([0, 0, 0]));
                            }
                        }
                    }
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (out, cols, tile) = (
        &args[0],
        args[1].parse::<u32>().unwrap(),
        args[2].parse::<u32>().unwrap(),
    );
    let files = &args[3..];
    let rows = (files.len() as u32).div_ceil(cols);
    let mut sheet = RgbImage::from_pixel(cols * tile, rows * tile, Rgb([40, 40, 40]));
    for (i, f) in files.iter().enumerate() {
        let (c, r) = (i as u32 % cols, i as u32 / cols);
        let Ok(img) = image::open(f) else { continue };
        let img = img.resize(tile - 4, tile - 4, FilterType::Triangle).to_rgb8();
        let (ox, oy) = (
            c * tile + 2 + (tile - 4 - img.width()) / 2,
            r * tile + 2 + (tile - 4 - img.height()) / 2,
        );
        image::imageops::replace(&mut sheet, &img, i64::from(ox), i64::from(oy));
        let stem = std::path::Path::new(f)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let digits: String = stem.chars().filter(char::is_ascii_digit).collect();
        let n: u64 = digits.parse().unwrap_or(0);
        draw_number(&mut sheet, c * tile + 2, r * tile + 2, &n.to_string(), 3);
    }
    sheet.save(out).unwrap();
}
