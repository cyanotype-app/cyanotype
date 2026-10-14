import { describe, expect, it } from "vitest";
import { ACCEPT, contentTypeOf, filterFiles, isUploadable } from "./files";

describe("isUploadable", () => {
  it("accepts photo and RAW extensions case-insensitively", () => {
    for (const n of ["a.jpg", "A.JPEG", "b.png", "c.webp", "d.HEIC", "e.heif", "f.avif", "g.DNG", "h.cr3", "i.nef", "j.arw", "k.x3f", "l.crw"]) {
      expect(isUploadable(n), n).toBe(true);
    }
  });
  it("rejects other types and extensionless names", () => {
    for (const n of ["notes.txt", "movie.mov", "README", "photo", "x.jpg.part", ".jpg"]) expect(isUploadable(n), n).toBe(false);
  });
  it("skips hidden and system files, including inside hidden folders", () => {
    expect(isUploadable(".hidden.jpg")).toBe(false);
    expect(isUploadable("._IMG_1.jpg")).toBe(false);
    expect(isUploadable("Thumbs.db")).toBe(false);
    expect(isUploadable("a.jpg", ".thumbnails/a.jpg")).toBe(false);
    expect(isUploadable("a.jpg", "__MACOSX/Trip/a.jpg")).toBe(false);
    expect(isUploadable("a.jpg", "Trip/day 1/a.jpg")).toBe(true);
  });
});

describe("filterFiles", () => {
  it("splits accepted from skipped and honours webkitRelativePath", () => {
    const hidden = new File(["x"], "a.jpg");
    Object.defineProperty(hidden, "webkitRelativePath", { value: "Trip/.cache/a.jpg" });
    const ok = new File(["x"], "b.arw");
    const { accepted, skipped } = filterFiles([ok, hidden, new File(["x"], "c.txt")]);
    expect(accepted).toEqual([ok]);
    expect(skipped).toBe(2);
  });
});

describe("misc", () => {
  it("sends octet-stream when the browser has no type", () => {
    expect(contentTypeOf(new File(["x"], "a.cr2"))).toBe("application/octet-stream");
    expect(contentTypeOf(new File(["x"], "a.jpg", { type: "image/jpeg" }))).toBe("image/jpeg");
  });
  it("builds an accept list from the extensions", () => {
    expect(ACCEPT.split(",")).toContain(".cr3");
    expect(ACCEPT.split(",")).toHaveLength(28);
  });
});
