// Incremental SHA-256 off the main thread. WebCrypto cannot hash a stream, so hash-wasm (WASM) consumes the file
// in chunks; memory stays at one chunk however large the RAW is.
import { createSHA256 } from "hash-wasm";

export interface HashRequest {
  file: Blob;
}
export type HashMessage =
  | { type: "progress"; done: number }
  | { type: "done"; hex: string }
  | { type: "error"; message: string };

const post = (m: HashMessage) => self.postMessage(m);

self.onmessage = async (e: MessageEvent<HashRequest>) => {
  try {
    const hasher = await createSHA256();
    hasher.init();
    const reader = e.data.file.stream().getReader();
    let done = 0;
    let lastReport = 0;
    for (;;) {
      const { value, done: finished } = await reader.read();
      if (finished) break;
      hasher.update(value);
      done += value.byteLength;
      if (done - lastReport >= 4 * 1024 * 1024) {
        lastReport = done;
        post({ type: "progress", done });
      }
    }
    post({ type: "done", hex: hasher.digest("hex") });
  } catch (err) {
    post({ type: "error", message: err instanceof Error ? err.message : "hashing failed" });
  }
};
