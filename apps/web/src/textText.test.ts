import { describe, expect, it } from "vitest";
import type { PhotoText } from "./api";
import { boxStyle, copyText, lineBoxes, linkHost, qrBoxes, qrKind, safeLink, textStatus } from "./textText";

const text = (over: Partial<PhotoText> = {}): PhotoText => ({ status: "read", text: "", lines: [], qr: [], ...over });

describe("QR payloads", () => {
  it("only a plain web address is a link", () => {
    expect(safeLink("https://example.org/a?b=c")).toBe("https://example.org/a?b=c");
    expect(safeLink("  http://Example.org  ")).toBe("http://example.org/");
    for (const no of ["javascript:alert(1)", "data:text/html,x", "WIFI:S:n;T:WPA;P:p;;", "tel:+1", "ftp://x", "https://a b", "https://x\n.evil", "", "just text", "//example.org"]) {
      expect(safeLink(no), no).toBeNull();
    }
    expect(safeLink(`https://example.org/${"a".repeat(3000)}`)).toBeNull();
  });

  it("names what a code holds", () => {
    expect(qrKind("https://example.org")).toBe("Web address");
    expect(qrKind("WIFI:S:home;T:WPA;P:secret;;")).toBe("Wi-Fi network");
    expect(qrKind("mailto:a@example.org")).toBe("Email address");
    expect(qrKind("TEL:+123")).toBe("Phone number");
    expect(qrKind("BEGIN:VCARD\nVERSION:3.0")).toBe("Contact");
    expect(qrKind("javascript:alert(1)")).toBe("Text");
    expect(linkHost("https://example.org:8080/x")).toBe("example.org:8080");
  });
});

describe("boxes", () => {
  it("is the bounding box of the quad, in percent of the picture", () => {
    expect(boxStyle([0.1, 0.2, 0.5, 0.2, 0.5, 0.3, 0.1, 0.3])).toEqual({ left: "10.00%", top: "20.00%", width: "40.00%", height: "10.00%" });
    // A turned line: the box of its corners.
    expect(boxStyle([0.2, 0.1, 0.4, 0.2, 0.3, 0.4, 0.1, 0.3])).toEqual({ left: "10.00%", top: "10.00%", width: "30.00%", height: "30.00%" });
    expect(boxStyle([0.1, 0.2])).toBeNull();
    expect(boxStyle([0, 0, 0, 0, Number.NaN, 0, 0, 0])).toBeNull();
  });

  it("drops lines and codes with a bad quad instead of failing", () => {
    const q = [0.1, 0.1, 0.2, 0.1, 0.2, 0.2, 0.1, 0.2];
    expect(lineBoxes([{ t: "a", s: 1, q, w: [] }, { t: "b", s: 1, q: [1], w: [] }]).map((b) => b.text)).toEqual(["a"]);
    expect(qrBoxes([{ t: "x", q }, { t: "y", q: [] }])).toHaveLength(1);
  });
});

describe("text panel words", () => {
  it("copies the text as read", () => {
    expect(copyText(text({ text: "  line one\nline two \n" }))).toBe("line one\nline two");
  });

  it("says why there is nothing", () => {
    expect(textStatus(null)).toBeNull();
    expect(textStatus(text({ status: "pending" }))).toMatch(/not been read yet/);
    expect(textStatus(text({ text: "  " }))).toMatch(/No text found/);
    expect(textStatus(text({ text: "hi" }))).toBeNull();
    expect(textStatus(text({ qr: [{ t: "https://x.org", q: [] }] }))).toBeNull();
  });
});
