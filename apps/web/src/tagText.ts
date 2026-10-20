// Pure helpers of tags: the rules for a name (the server's, so a bad name is caught before it is sent), the list in an address,
// autocomplete, and the words of the filter chip and the toasts. Unit tested.
import type { Tag } from "./api";

export const MAX_TAG_CHARS = 100;
/** Most tags one filter or one bulk request names (the server's limits). */
export const MAX_TAGS_AT_ONCE = 20;

/** The name as the server will keep it: trimmed, runs of white space folded to one space. */
export const tidy = (name: string) => name.split(/\s+/).filter(Boolean).join(" ");

/** Why `name` cannot be a tag, or null when it can. Commas are the separator while typing, so a name never has one. */
export function nameProblem(name: string): string | null {
  const n = tidy(name);
  if (!n) return "A tag needs a name.";
  if ([...n].length > MAX_TAG_CHARS) return `A tag name is at most ${MAX_TAG_CHARS} characters.`;
  if (n.includes(",")) return "A tag name cannot contain a comma.";
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f]/.test(n)) return "A tag name cannot contain control characters.";
  return null;
}

/** `a, b ,c` or `a,b,A` to the distinct names (ignoring case, first spelling kept), in order. */
export function tagListOf(value: string | null | undefined): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const raw of (value ?? "").split(",")) {
    const n = tidy(raw);
    if (n && !seen.has(n.toLowerCase()) && nameProblem(n) === null) {
      seen.add(n.toLowerCase());
      out.push(n);
    }
  }
  return out.slice(0, MAX_TAGS_AT_ONCE);
}

/** Add `name` to a list of names unless it is there already (any case). Returns the same list when nothing changes. */
export function withName(list: readonly string[], name: string): readonly string[] {
  const n = tidy(name);
  if (!n || nameProblem(n) !== null || list.some((x) => x.toLowerCase() === n.toLowerCase()) || list.length >= MAX_TAGS_AT_ONCE) return list;
  return [...list, n];
}

/** Tags whose name contains what is typed (any case), most used first, never one already chosen; at most `limit`. */
export function suggest(typed: string, tags: readonly Tag[], chosen: readonly string[], limit = 8): Tag[] {
  const t = tidy(typed).toLowerCase();
  const taken = new Set(chosen.map((c) => c.toLowerCase()));
  return tags
    .filter((x) => !taken.has(x.name.toLowerCase()) && (t === "" || x.name.toLowerCase().includes(t)))
    .sort((a, b) => {
      // A name that starts with the text first, then the more used, then alphabetical.
      const sa = a.name.toLowerCase().startsWith(t) ? 0 : 1;
      const sb = b.name.toLowerCase().startsWith(t) ? 0 : 1;
      return sa - sb || b.count - a.count || a.name.localeCompare(b.name);
    })
    .slice(0, limit);
}

/** The filter chip: "Tag: beach", "Tags: beach and sunset", "Tags: beach or sunset". */
export function tagFilterLabel(tags: string, mode?: "any" | "all"): string {
  const list = tagListOf(tags);
  if (list.length <= 1) return `Tag: ${list[0] ?? tags}`;
  return `Tags: ${list.join(mode === "any" ? " or " : " and ")}`;
}

/** "3 photos" / "1 photo" for a tag's count. */
export const tagPhotos = (n: number) => `${n.toLocaleString("en-US")} ${n === 1 ? "photo" : "photos"}`;

/** The toast after a bulk change. */
export function taggedMessage(photos: number, added: readonly string[], removed: readonly string[]): string {
  const photoWord = `${photos.toLocaleString("en-US")} ${photos === 1 ? "photo" : "photos"}`;
  const names = (l: readonly string[]) => (l.length <= 2 ? l.join(" and ") : `${l.length} tags`);
  const parts = [];
  if (added.length > 0) parts.push(`added ${names(added)}`);
  if (removed.length > 0) parts.push(`removed ${names(removed)}`);
  return `${parts.join(", ")} on ${photoWord}`.replace(/^./, (c) => c.toUpperCase());
}

/** Where a tag's photos are listed. */
export const tagSearchHref = (name: string) => `/search?${new URLSearchParams({ tags: name })}`;
