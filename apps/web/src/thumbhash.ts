import { thumbHashToDataURL } from "thumbhash";

/** Blurred placeholder (PNG data URL) from the API's base64 ThumbHash; undefined if it cannot be decoded. */
export function placeholderUrl(b64: string | null): string | undefined {
  if (!b64) return undefined;
  try {
    const bin = atob(b64);
    const bytes = Uint8Array.from(bin, (c) => c.charCodeAt(0));
    return thumbHashToDataURL(bytes);
  } catch {
    return undefined;
  }
}
