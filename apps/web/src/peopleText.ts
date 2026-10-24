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

/** The People page's two lists: the people to show, and the ones the owner hid (a collapsed section). Order is kept. */
export function splitPeople(people: readonly Person[]): { shown: Person[]; hidden: Person[] } {
  return { shown: people.filter((p) => !p.hidden), hidden: people.filter((p) => p.hidden) };
}

/** The question over a suggested face. */
export const suggestionQuestion = (p: { name: string | null }) => (p.name?.trim() ? `Is this ${p.name.trim()}?` : "Is this the same person?");

/** The toast after "Not this person". */
export const removedMessage = (photos: number, p: { name: string | null }) => `Took ${photosWord(photos)} out of ${personLabel(p)}`;

/** The toast after a merge. */
export const mergedMessage = (into: { name: string | null }, faces: number) => `Merged into ${personLabel(into)} (${n(faces)} ${faces === 1 ? "face" : "faces"})`;

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
