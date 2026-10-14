import type { HashMessage } from "./hash.worker";

/** SHA-256 (lowercase hex) of a file, computed in a Web Worker. `onProgress` gets bytes hashed so far. */
export function hashFile(file: Blob, onProgress?: (done: number) => void): Promise<string> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL("./hash.worker.ts", import.meta.url), { type: "module" });
    const finish = () => worker.terminate();
    worker.onmessage = (e: MessageEvent<HashMessage>) => {
      const m = e.data;
      if (m.type === "progress") onProgress?.(m.done);
      else if (m.type === "done") {
        finish();
        resolve(m.hex);
      } else {
        finish();
        reject(new Error(m.message));
      }
    };
    worker.onerror = () => {
      finish();
      reject(new Error("Could not read the file."));
    };
    worker.postMessage({ file });
  });
}
