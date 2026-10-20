//! Try the QR reader on one image: `qrprobe IMAGE`. A development aid for looking at why a code in a real photo is not read
//! (crumpled paper, glare, too small).
fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: qrprobe IMAGE"))?;
    let img = image::open(path)?.to_rgb8();
    let (w, h) = img.dimensions();
    let t = std::time::Instant::now();
    let found = cyanotype_ocr::qr::detect(img.as_raw(), w, h);
    println!("{w}x{h}: {} code(s) in {} ms", found.len(), t.elapsed().as_millis());
    for c in found {
        println!("  {}  {:?}", c.text, c.quad);
    }
    Ok(())
}
