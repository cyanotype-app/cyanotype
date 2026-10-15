import { describe, expect, it } from "vitest";
import { fileSize, pairLabel } from "./pairText";

const raw = { id: "r", original_name: "IMG_1.CR3", content_type: "image/x-canon-cr3", size_bytes: 25_000_000, kind: "raw" as const };

describe("pairText", () => {
  it("formats sizes", () => {
    expect(fileSize(512)).toBe("512 B");
    expect(fileSize(2048)).toBe("2.0 KB");
    expect(fileSize(3_500_000)).toBe("3.3 MB");
    expect(fileSize(25_000_000)).toBe("24 MB");
  });

  it("names the pair after the visible file", () => {
    expect(pairLabel({ content_type: "image/jpeg", original_name: "a.jpg" }, [raw])).toBe("RAW + JPEG");
    expect(pairLabel({ content_type: "application/octet-stream", original_name: "a.HEIC" }, [raw])).toBe("RAW + HEIC");
    expect(pairLabel({ content_type: "image/avif", original_name: "a.avif" }, [raw])).toBe("RAW + AVIF");
    expect(pairLabel({ content_type: "image/jpeg", original_name: "a.jpg" }, [])).toBe("");
  });
});
