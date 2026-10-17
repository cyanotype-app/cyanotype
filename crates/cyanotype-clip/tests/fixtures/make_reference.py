#!/usr/bin/env python3
"""One-off generator for reference.json: embeddings from the reference stack (Pillow preprocessing exactly as
transformers' CLIPImageProcessor, tokenizers' Python bindings, onnxruntime) for the shipped int8 models, which the Rust
tests compare against. Not run by CI.

  python3 -m venv /tmp/v && /tmp/v/bin/pip install onnxruntime numpy pillow tokenizers
  /tmp/v/bin/python make_reference.py /path/to/model/dir > reference.json
"""
import json, sys
import numpy as np, onnxruntime as ort
from PIL import Image
from tokenizers import Tokenizer

M = sys.argv[1].rstrip("/") + "/"
IMAGES = ["dog", "cat", "city_night", "fireworks"]
TEXTS = ["dog", "a small white fluffy dog", "red car at night", "birthday cake", "A  Cat with BLUE eyes", "a photo of two people hiking in the mountains at sunrise"]
LONG = "word " * 100  # longer than the 77-token context: must be truncated, ending with the end-of-text token
MEAN = np.array([0.48145466, 0.4578275, 0.40821073], dtype=np.float32)
STD = np.array([0.26862954, 0.26130258, 0.27577711], dtype=np.float32)


def prep(path):
    im = Image.open(path).convert("RGB")
    w, h = im.size
    s = 224
    nw, nh = (s, max(s, int(s * h / w))) if w <= h else (max(s, int(s * w / h)), s)
    im = im.resize((nw, nh), Image.BICUBIC)
    l, t = (nw - s) // 2, (nh - s) // 2
    a = np.asarray(im.crop((l, t, l + s, t + s)), dtype=np.float32) / 255.0
    return ((a - MEAN) / STD).transpose(2, 0, 1)[None]


def norm(v):
    return (v / np.linalg.norm(v)).reshape(-1)


tok = Tokenizer.from_file(M + "tokenizer.json")


def ids(t):
    i = tok.encode(t).ids
    return i[:76] + [49407] if len(i) > 77 else i


vis = ort.InferenceSession(M + "vision_model_int8.onnx", providers=["CPUExecutionProvider"])
txt = ort.InferenceSession(M + "text_model_int8.onnx", providers=["CPUExecutionProvider"])
r = lambda v: [round(float(x), 5) for x in v]
out = {
    "images": {n: r(norm(vis.run(None, {"pixel_values": prep(f"images/{n}.jpg")})[0])) for n in IMAGES},
    "texts": {t: {"ids": ids(t), "embedding": r(norm(txt.run(None, {"input_ids": np.array([ids(t)], dtype=np.int64)})[0]))} for t in TEXTS},
    "long_text_ids": ids(LONG),
}
json.dump(out, sys.stdout, separators=(",", ":"))
