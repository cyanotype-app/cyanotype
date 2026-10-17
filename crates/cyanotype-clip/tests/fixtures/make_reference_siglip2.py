#!/usr/bin/env python3
"""One-off generator for reference_siglip2.json: what the SigLIP 2 reference stack produces for the fixtures, which the Rust
tests compare against. Not run by CI.

  * preprocessing: Pillow exactly as transformers' SiglipImageProcessor (resize to 224x224 with BILINEAR, no crop, rescale
    1/255, mean 0.5 / std 0.5). The tensor is stored subsampled (a 16 x 16 grid per channel).
  * tokenizer: the model's tokenizer.json through the `tokenizers` Python bindings, after lower-casing, truncated to 63
    tokens + <eos> and padded with <pad> to 64 (what transformers' tokenizer does with padding="max_length", max_length=64,
    truncation=True, once the text is lower-cased: the model was trained on lower-case text).
  * embeddings: onnxruntime on the shipped graphs (vision_model_q4.onnx, text_model_int8.onnx) and on the fp32 graphs
    (vision_model.onnx, text_model.onnx) of the same export. The fp32 ones make the test independent of the CPU the
    quantised kernels ran on.

  python3 -m venv /tmp/v && /tmp/v/bin/pip install onnxruntime numpy pillow tokenizers
  /tmp/v/bin/python make_reference_siglip2.py /path/to/onnx-community/siglip2-base-patch16-224-ONNX > reference_siglip2.json
"""
import json, sys
import numpy as np, onnxruntime as ort
from PIL import Image
from tokenizers import Tokenizer

M = sys.argv[1].rstrip("/") + "/"
IMAGES = ["dog", "cat", "city_night", "fireworks"]
TEXTS = ["dog", "a small white fluffy dog", "red car at night", "birthday cake", "A  Cat with BLUE eyes",
         "perro en la playa", "chien sur la plage", "Hund am Strand", "a photo of two people hiking in the mountains at sunrise"]
LONG = "word " * 100  # more than 64 tokens: truncated to 63 + <eos>


def prep(path):
    im = Image.open(path).convert("RGB").resize((224, 224), Image.BILINEAR)
    a = np.asarray(im, dtype=np.float32) / 255.0
    return ((a - 0.5) / 0.5).transpose(2, 0, 1)[None]


def norm(v):
    return (v / np.linalg.norm(v)).reshape(-1)


tok = Tokenizer.from_file(M + "tokenizer.json")


def ids(t):
    i = [x for x in tok.encode(t.lower()).ids if x != 0]
    if len(i) > 64:
        i = i[:63] + [1]
    return i + [0] * (64 - len(i))


def sess(f):
    o = ort.SessionOptions(); o.intra_op_num_threads = 1
    return ort.InferenceSession(M + "onnx/" + f, o, providers=["CPUExecutionProvider"])


vq, vf, ti, tf = sess("vision_model_q4.onnx"), sess("vision_model.onnx"), sess("text_model_int8.onnx"), sess("text_model.onnx")
r = lambda v, n=5: [round(float(x), n) for x in v]
images = {}
for n in IMAGES:
    x = prep(f"images/{n}.jpg")
    grid = x[0][:, 7::14, 7::14][:, :16, :16]
    images[n] = {
        "tensor_sample": r(grid.reshape(-1), 4),
        "embedding": r(norm(vq.run(None, {"pixel_values": x})[1])),
        "embedding_fp32": r(norm(vf.run(None, {"pixel_values": x})[1])),
    }
texts = {}
for t in TEXTS:
    a = np.array([ids(t)], dtype=np.int64)
    texts[t] = {"ids": ids(t), "embedding": r(norm(ti.run(None, {"input_ids": a})[1])), "embedding_fp32": r(norm(tf.run(None, {"input_ids": a})[1]))}
json.dump({"images": images, "texts": texts, "long_text_ids": ids(LONG)}, sys.stdout, separators=(",", ":"))
