// Pure helpers of People: the words (names, counts, toasts), the rules for a name (the server's), which people are shown where, and
// the arithmetic of cropping a face out of a photo's preview with CSS. Unit tested.
import type { FaceBox, Person } from "./api";

export const MAX_NAME_CHARS = 100;
export const UNNAMED = "Unnamed person";
export const ADD_A_NAME = "Add a name";

/** The name as the server will keep it: trimmed. */
export const tidyName = (name: string) => name.trim();

/** Why `name` cannot be a person's name, or null when it can. Blank is fine: it clears the name. */
export function nameProblem(name: string): string | null {
  const n = tidyName(name);
  if ([...n].length > MAX_NAME_CHARS) return `A name is at most ${MAX_NAME_CHARS} characters.`;
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f]/.test(n)) return "A name cannot contain control characters.";
  return null;
}

/** What a person is called on screen. */
export const personLabel = (p: { name: string | null }) => p.name?.trim() || UNNAMED;

const n = (v: number) => v.toLocaleString("en-US");

/** "3 photos" / "1 photo". */
export const photosWord = (count: number) => `${n(count)} ${count === 1 ? "photo" : "photos"}`;

/** "12 people" / "1 person". */
export const peopleWord = (count: number) => `${n(count)} ${count === 1 ? "person" : "people"}`;

/** Has the owner named the person? A blank name is no name. */
export const isNamed = (p: { name: string | null }) => (p.name?.trim() ?? "") !== "";

/**
 * The People page's three sections, never mixed: the named people, the unnamed ones (to name), and the ones the owner hid (a
 * collapsed section, named or not). Order is kept.
 */
export function splitPeople(people: readonly Person[]): { named: Person[]; unnamed: Person[]; hidden: Person[] } {
  const shown = people.filter((p) => !p.hidden);
  return { named: shown.filter(isNamed), unnamed: shown.filter((p) => !isNamed(p)), hidden: people.filter((p) => p.hidden) };
}

/** "3 photos to confirm": the pending "Is this X?" suggestions of a person (the badge on their card). */
export const pendingLabel = (count: number) => `${photosWord(count)} to confirm`;

/** What a card is called for assistive technology: the person, how many photos, and what waits for an answer. */
export const cardLabel = (p: Person) => ((p.pending ?? 0) > 0 ? `${openPersonLabel(p)}, ${pendingLabel(p.pending ?? 0)}` : openPersonLabel(p));

/** The badge's text: the number, capped so it stays small. */
export const badgeText = (count: number) => (count > 99 ? "99+" : String(count));

/** The question over a suggested face. */
export const suggestionQuestion = (p: { name: string | null }) => (p.name?.trim() ? `Is this ${p.name.trim()}?` : "Is this the same person?");

/** The toast after "Not this person". */
export const removedMessage = (photos: number, p: { name: string | null }) => `Took ${photosWord(photos)} out of ${personLabel(p)}`;

/** The toast after a merge. */
export const mergedMessage = (into: { name: string | null }, faces: number) => `Merged into ${personLabel(into)} (${n(faces)} ${faces === 1 ? "face" : "faces"})`;

/** The toasts after a change to several people at once. */
export const bulkMessage = {
  hide: (count: number) => `${peopleWord(count)} hidden from the People page and from photos`,
  show: (count: number) => `${peopleWord(count)} shown again`,
  delete: (count: number) => `${peopleWord(count)} deleted. The photos stay in your library.`,
  merge: (count: number, into: { name: string | null }) => `Merged ${peopleWord(count)} into ${personLabel(into)}`,
};

/** The accessible name of a person's card and chip. */
export const openPersonLabel = (p: Person) => `${personLabel(p)}, ${photosWord(p.photos)}`;

/**
 * Where to put the preview image inside a square, overflow-hidden box so the box shows the face with `margin` times its larger
 * side around it. `box` is fractions of the preview, `natW` x `natH` its size in pixels. The image is as wide as `width` of the box
 * and sits `left` and `top` from its corner (all percentages of the box, which is square). The crop stays inside the picture when
 * it can, and is centred on it when it is bigger.
 */
export function cropStyle(box: FaceBox, natW: number, natH: number, margin = 1.7): { width: string; left: string; top: string } {
  const [x, y, w, h] = box;
  const side = Math.max(w * natW, h * natH, 1) * margin;
  const place = (centre: number, extent: number) => {
    if (side >= extent) return (extent - side) / 2;
    return Math.min(Math.max(centre - side / 2, 0), extent - side);
  };
  const x0 = place((x + w / 2) * natW, natW);
  const y0 = place((y + h / 2) * natH, natH);
  const pct = (v: number) => `${Math.round(v * 1e4) / 1e2}%`;
  return { width: pct(natW / side), left: pct(-x0 / side), top: pct(-y0 / side) };
}

/** Most people one search or smart-album rule can name together (the server's limit). */
export const MAX_FILTER_PEOPLE = 10;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** The ids in a `people` URL parameter: valid ids only, each once, at most [`MAX_FILTER_PEOPLE`]. Anything else is dropped. */
export function peopleIdsOf(v: string | null | undefined): string[] {
  const out: string[] = [];
  for (const part of (v ?? "").split(",")) {
    const id = part.trim().toLowerCase();
    if (UUID.test(id) && !out.includes(id)) out.push(id);
  }
  return out.slice(0, MAX_FILTER_PEOPLE);
}

/** The people a filter can choose: named and not hidden, in the order given (most photos first). */
export const filterablePeople = (people: readonly Person[]): Person[] => people.filter((p) => !p.hidden && (p.name?.trim() ?? "") !== "");

/** The picker's list for what is typed: the chosen people first, then the others whose name contains it (case-insensitive). */
export function matchingPeople(people: readonly Person[], typed: string, chosen: readonly string[]): Person[] {
  const t = typed.trim().toLowerCase();
  const hit = (p: Person) => t === "" || (p.name ?? "").toLowerCase().includes(t);
  return [...people.filter((p) => chosen.includes(p.id)), ...people.filter((p) => !chosen.includes(p.id) && hit(p))];
}

/** "Person A", "Person A and Person B", "Person A, Person B and Person C": the chip of a people filter. A person who is gone says so. */
export function peopleFilterLabel(ids: readonly string[], people: readonly Pick<Person, "id" | "name">[]): string {
  const names = ids.map((id) => people.find((p) => p.id === id)?.name?.trim() || "Unknown person");
  return names.length <= 2 ? names.join(" and ") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

export const PEOPLE_FILTER_NOTE = "Photos with all of the chosen people in them.";
export const NO_NAMED_PEOPLE = "Name a person on the People page to filter by them.";
export const FIND_A_PERSON = "Find a person";

/** The note under "Save as album": the title is what anyone the album is shared with sees. */
export const SAVE_ALBUM_NOTE =
  "A copy of these photos as they are now. The album has its own title, which people you share it with can see, so it does not start as a name.";

/** "People: 2 people together" for a smart-album rule (names are not kept in the rule). */
export const peopleRuleLabel = (ids: readonly string[]) => (ids.length === 1 ? "People: 1 person" : `People: ${ids.length} people together`);
