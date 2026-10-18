//! `onnx-split <model.onnx> [min_mb]`: moves the model's large weights (default: tensors of 32 MB or more) into
//! `<model.onnx>.data` and rewrites `<model.onnx>` to reference them, in place. See `crates/cyanotype-clip/src/split.rs`.
//!
//! Built by the Dockerfile with `rustc --edition 2024 -O tools/onnx_split.rs` (no Cargo, no dependencies).
#[path = "../crates/cyanotype-clip/src/split.rs"]
mod split;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: onnx-split <model.onnx> [min_mb]");
        std::process::exit(2);
    };
    let min_mb: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(32);
    let data_name = format!(
        "{}.data",
        std::path::Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("model.onnx")
    );
    let bytes = std::fs::read(&path).unwrap_or_else(|e| fail(&format!("read {path}: {e}")));
    let out = split::split(&bytes, &data_name, min_mb << 20).unwrap_or_else(|e| fail(&format!("{path}: {e}")));
    if out.moved == 0 {
        println!("{path}: nothing to move");
        return;
    }
    let data_path = format!("{path}.data");
    std::fs::write(&data_path, &out.data).unwrap_or_else(|e| fail(&format!("write {data_path}: {e}")));
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, &out.model).unwrap_or_else(|e| fail(&format!("write {tmp}: {e}")));
    std::fs::rename(&tmp, &path).unwrap_or_else(|e| fail(&format!("replace {path}: {e}")));
    println!(
        "{path}: moved {} tensor(s), {} MB, into {data_path}; model is now {} KB",
        out.moved,
        out.data.len() >> 20,
        out.model.len() >> 10
    );
}

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}
