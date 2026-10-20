import type { PhotoText, PhotoTextLine, PhotoTextQr } from "./api";

/** A QR payload that may be offered as a link the person clicks: a plain `http(s)` address, nothing else. Never opened by the app. */
export function safeLink(payload: string): string | null {
  const t = payload.trim();
  if (t.length === 0 || t.length > 2048 || [...t].some((c) => c.trim() === "" || c.charCodeAt(0) < 0x20 || c.charCodeAt(0) === 0x7f)) return null;
  try {
    const u = new URL(t);
    return u.protocol === "http:" || u.protocol === "https:" ? u.href : null;
  } catch {
    return null;
  }
}

/** What a QR payload is, in words for the list ("Web address", "Wi-Fi network", "Text"). */
export function qrKind(payload: string): string {
  if (safeLink(payload)) return "Web address";
  const p = payload.trim().toUpperCase();
  if (p.startsWith("WIFI:")) return "Wi-Fi network";
  if (p.startsWith("MAILTO:")) return "Email address";
  if (p.startsWith("TEL:") || p.startsWith("SMSTO:") || p.startsWith("SMS:")) return "Phone number";
  if (p.startsWith("BEGIN:VCARD") || p.startsWith("MECARD:")) return "Contact";
  if (p.startsWith("GEO:")) return "Location";
  return "Text";
}

/** The shown host of a link, so a long address is recognisable at a glance ("example.org"). */
export function linkHost(href: string): string {
  try {
    return new URL(href).host;
  } catch {
    return href;
  }
}

/** The text for "Copy": the lines as read, one per row. */
export const copyText = (t: PhotoText): string => t.text.trim();

/** A box on the photo, as percentages of the picture, from a normalised quad (x0,y0 ... x3,y3): the bounding box of its corners. */
export function boxStyle(q: number[]): { left: string; top: string; width: string; height: string } | null {
  if (q.length !== 8 || q.some((v) => !Number.isFinite(v))) return null;
  const xs = [q[0]!, q[2]!, q[4]!, q[6]!];
  const ys = [q[1]!, q[3]!, q[5]!, q[7]!];
  const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
  const pct = (v: number) => `${(v * 100).toFixed(2)}%`;
  return { left: pct(x0), top: pct(y0), width: pct(x1 - x0), height: pct(y1 - y0) };
}

/** Highlight boxes: one per line (words are drawn when the viewer is zoomed in on one line; lines are what a person selects). */
export function lineBoxes(lines: PhotoTextLine[]): { key: string; style: NonNullable<ReturnType<typeof boxStyle>>; text: string }[] {
  return lines.flatMap((l, i) => {
    const style = boxStyle(l.q);
    return style ? [{ key: `l${i}`, style, text: l.t }] : [];
  });
}

export function qrBoxes(codes: PhotoTextQr[]): { key: string; style: NonNullable<ReturnType<typeof boxStyle>> }[] {
  return codes.flatMap((c, i) => {
    const style = boxStyle(c.q);
    return style ? [{ key: `q${i}`, style }] : [];
  });
}

/** One sentence for the empty states. */
export function textStatus(t: PhotoText | null): string | null {
  if (!t) return null;
  if (t.status === "pending") return "The text in this photo has not been read yet. It is read in the background, after the photo is processed.";
  if (t.text.trim() === "" && t.qr.length === 0) return "No text found in this photo.";
  return null;
}
