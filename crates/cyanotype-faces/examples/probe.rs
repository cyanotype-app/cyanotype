//! Print what the face models make of some pictures, one JSON object per line:
//! `FACES_MODEL_DIR=models/faces cargo run --release -p cyanotype-faces --example probe -- a.jpg b.jpg ...`
//! (`embed` as the first argument adds each face's fingerprint.) A tool for looking at the models, not part of the product.
use anyhow::{Context, Result};
use cyanotype_faces::{Engine, EngineConfig};

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let embed = args.first().is_some_and(|a| a == "embed");
    if embed {
        args.remove(0);
    }
    let engine = Engine::load(&cyanotype_faces::model_dir(), EngineConfig::default())?;
    for path in args {
        let img = image::open(&path).with_context(|| path.clone())?.to_rgb8();
        let (w, h) = img.dimensions();
        let faces = engine.analyse(img.as_raw(), w, h)?;
        let list: Vec<_> = faces
            .iter()
            .map(|f| {
                let d = &f.detection;
                let mut v = serde_json::json!({"score": d.score, "bbox": d.bbox, "landmarks": d.landmarks});
                if embed {
                    v["embedding"] = serde_json::json!(f.embedding);
                }
                v
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({"file": path, "width": w, "height": h, "faces": list})
        );
    }
    Ok(())
}
