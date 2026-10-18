//! Moves the big weights of an ONNX model into a side file ("external data"), so ONNX Runtime memory-maps them instead
//! of copying them onto the heap.
//!
//! Why: SigLIP 2's text tower is 283 MB, 197 MB of it one embedding table (256,000 tokens x 768) of which a query reads at
//! most 64 rows. Loaded from a single `.onnx` file, ONNX Runtime holds the file's bytes and a copy of every weight
//! (524 MB resident). With the table in an external data file it is mapped, and only the pages a query touches are
//! ever read (about 120 MB resident, measured in `docs/build.md`).
//!
//! The file is protobuf; only the few fields involved are interpreted, everything else is copied byte for byte. Standard
//! library only, because the Docker build compiles this one file with `rustc` (see `tools/onnx_split.rs`) to prepare the
//! model in its own layer, without building the workspace.
//!
//! ONNX fields used (onnx.proto3): `ModelProto.graph = 7`; `GraphProto.initializer = 5`; `TensorProto`: `raw_data = 9`,
//! `external_data = 13` (`StringStringEntryProto` key = 1, value = 2), `data_location = 14` (1 = EXTERNAL).

const MODEL_GRAPH: u64 = 7;
const GRAPH_INITIALIZER: u64 = 5;
const TENSOR_RAW_DATA: u64 = 9;
const TENSOR_EXTERNAL_DATA: u64 = 13;
const TENSOR_DATA_LOCATION: u64 = 14;
/// Offsets in the side file are multiples of this, so each tensor starts on a page and can be mapped on its own.
const ALIGN: usize = 4096;

#[derive(Debug)]
pub struct Split {
    /// The model with the large initializers' bytes replaced by references.
    pub model: Vec<u8>,
    /// The bytes those references point into (`data_file` of [`split`]).
    pub data: Vec<u8>,
    /// How many tensors were moved.
    pub moved: usize,
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn varint(&mut self) -> Result<u64, String> {
        let (mut v, mut shift) = (0u64, 0);
        loop {
            let b = *self.buf.get(self.pos).ok_or("truncated varint")?;
            self.pos += 1;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
            if shift > 63 {
                return Err("varint too long".into());
            }
        }
    }

    /// The next field: its number, its wire type and the bytes of the whole field (tag included), and for a
    /// length-delimited one the payload range.
    fn field(&mut self) -> Result<Option<Field<'a>>, String> {
        if self.pos >= self.buf.len() {
            return Ok(None);
        }
        let start = self.pos;
        let tag = self.varint()?;
        let (number, wire) = (tag >> 3, (tag & 7) as u8);
        let mut payload = None;
        match wire {
            0 => {
                self.varint()?;
            }
            1 => self.skip(8)?,
            5 => self.skip(4)?,
            2 => {
                let len = self.varint()? as usize;
                let from = self.pos;
                self.skip(len)?;
                payload = Some(&self.buf[from..from + len]);
            }
            w => return Err(format!("unsupported wire type {w}")),
        }
        Ok(Some(Field {
            number,
            whole: &self.buf[start..self.pos],
            payload,
        }))
    }

    fn skip(&mut self, n: usize) -> Result<(), String> {
        if self.pos + n > self.buf.len() {
            return Err("truncated field".into());
        }
        self.pos += n;
        Ok(())
    }
}

struct Field<'a> {
    number: u64,
    whole: &'a [u8],
    payload: Option<&'a [u8]>,
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_bytes_field(out: &mut Vec<u8>, number: u64, bytes: &[u8]) {
    put_varint(out, number << 3 | 2);
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn entry(key: &str, value: &str) -> Vec<u8> {
    let mut e = Vec::new();
    put_bytes_field(&mut e, 1, key.as_bytes());
    put_bytes_field(&mut e, 2, value.as_bytes());
    e
}

/// Rewrites `tensor` (a `TensorProto`) to point into the side file, or returns `None` if it does not carry at least
/// `min_bytes` of `raw_data`.
fn externalize(
    tensor: &[u8],
    data_file: &str,
    min_bytes: usize,
    data: &mut Vec<u8>,
) -> Result<Option<Vec<u8>>, String> {
    let mut r = Reader { buf: tensor, pos: 0 };
    let mut kept = Vec::new();
    let mut raw = None;
    while let Some(f) = r.field()? {
        match (f.number, f.payload) {
            (TENSOR_RAW_DATA, Some(p)) => raw = Some(p),
            // Already external: leave the tensor alone.
            (TENSOR_EXTERNAL_DATA | TENSOR_DATA_LOCATION, _) => return Ok(None),
            _ => kept.extend_from_slice(f.whole),
        }
    }
    let Some(raw) = raw.filter(|r| r.len() >= min_bytes) else {
        return Ok(None);
    };
    while !data.len().is_multiple_of(ALIGN) {
        data.push(0);
    }
    let offset = data.len();
    data.extend_from_slice(raw);
    for (k, v) in [
        ("location", data_file.to_string()),
        ("offset", offset.to_string()),
        ("length", raw.len().to_string()),
    ] {
        put_bytes_field(&mut kept, TENSOR_EXTERNAL_DATA, &entry(k, &v));
    }
    put_varint(&mut kept, TENSOR_DATA_LOCATION << 3);
    put_varint(&mut kept, 1);
    Ok(Some(kept))
}

/// Move every initializer with at least `min_bytes` of raw data into the side file named `data_file` (a plain file name:
/// ONNX Runtime looks it up next to the model).
pub fn split(model: &[u8], data_file: &str, min_bytes: usize) -> Result<Split, String> {
    let mut out = Vec::with_capacity(model.len() / 2);
    let mut data = Vec::new();
    let mut moved = 0;
    let mut top = Reader { buf: model, pos: 0 };
    while let Some(f) = top.field()? {
        let (MODEL_GRAPH, Some(graph)) = (f.number, f.payload) else {
            out.extend_from_slice(f.whole);
            continue;
        };
        let mut new_graph = Vec::with_capacity(graph.len() / 2);
        let mut g = Reader { buf: graph, pos: 0 };
        while let Some(gf) = g.field()? {
            match (gf.number, gf.payload) {
                (GRAPH_INITIALIZER, Some(t)) => match externalize(t, data_file, min_bytes, &mut data)? {
                    Some(rewritten) => {
                        moved += 1;
                        put_bytes_field(&mut new_graph, GRAPH_INITIALIZER, &rewritten);
                    }
                    None => new_graph.extend_from_slice(gf.whole),
                },
                _ => new_graph.extend_from_slice(gf.whole),
            }
        }
        put_bytes_field(&mut out, MODEL_GRAPH, &new_graph);
    }
    Ok(Split {
        model: out,
        data,
        moved,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tensor(name: &str, raw: &[u8]) -> Vec<u8> {
        let mut t = Vec::new();
        // dims = [raw.len()] (field 1, varint), data_type = 3 (field 2), name (8), raw_data (9)
        put_varint(&mut t, 1 << 3);
        put_varint(&mut t, raw.len() as u64);
        put_varint(&mut t, 2 << 3);
        put_varint(&mut t, 3);
        put_bytes_field(&mut t, 8, name.as_bytes());
        put_bytes_field(&mut t, TENSOR_RAW_DATA, raw);
        t
    }

    fn model(tensors: &[Vec<u8>]) -> Vec<u8> {
        let mut graph = Vec::new();
        put_bytes_field(&mut graph, 2, b"g"); // GraphProto.name
        for t in tensors {
            put_bytes_field(&mut graph, GRAPH_INITIALIZER, t);
        }
        let mut m = Vec::new();
        put_varint(&mut m, 1 << 3); // ir_version
        put_varint(&mut m, 7);
        put_bytes_field(&mut m, MODEL_GRAPH, &graph);
        put_bytes_field(&mut m, 14, b"trailing metadata field"); // after the graph: must survive
        m
    }

    #[test]
    fn large_tensors_move_to_the_side_file_and_small_ones_stay() {
        let big = vec![7u8; 10_000];
        let small = vec![1u8; 100];
        let m = model(&[tensor("small", &small), tensor("big", &big), tensor("big2", &big)]);
        let s = split(&m, "m.onnx.data", 1000).unwrap();
        assert_eq!(s.moved, 2);
        assert!(
            s.model.len() < m.len() - 2 * big.len() + 400,
            "{} -> {}",
            m.len(),
            s.model.len()
        );
        // Both big tensors are in the side file, page-aligned, in order.
        assert_eq!(s.data.len(), 2 * 12288 - (12288 - 10_000));
        assert!(s.data[..10_000].iter().all(|b| *b == 7));
        assert!(s.data[10_000..12288].iter().all(|b| *b == 0));
        assert!(s.data[12288..].iter().all(|b| *b == 7));
        // The small tensor's bytes and the trailing field are untouched.
        let has = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).any(|w| w == needle);
        assert!(has(&s.model, &small));
        assert!(has(&s.model, b"trailing metadata field"));
        assert!(!has(&s.model, &big[..64]));
        assert!(has(&s.model, b"m.onnx.data"));
        assert!(has(&s.model, b"location") && has(&s.model, b"offset") && has(&s.model, b"length"));
        // Splitting the result again changes nothing (nothing left to move).
        let again = split(&s.model, "m.onnx.data", 1000).unwrap();
        assert_eq!((again.moved, again.model), (0, s.model));
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        assert!(split(&[0x3a, 0xff], "x", 1).is_err());
        assert!(split(&[0x0a], "x", 1).is_err());
        assert!(split(&[], "x", 1).unwrap().model.is_empty());
    }
}
