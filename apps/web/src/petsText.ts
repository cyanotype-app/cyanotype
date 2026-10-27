// Pure helpers of Pets: the words (kinds, counts, progress), reading a kind out of a URL, and which kinds the page offers. Unit tested.
import type { NamedPet, PetAdded, PetKind, PetsSummary, PhotoPet, SkipReason } from "./api";
import { MAX_FILTER_PEOPLE, nameProblem as personNameProblem, peopleIdsOf, pendingLabel } from "./peopleText";

const n = (v: number) => v.toLocaleString("en-US");

/** The kinds a filter or a rule can be set to, in the order they are offered. */
export const PET_KINDS: readonly PetKind[] = ["any", "dog", "cat"];

/** What a kind is called as a filter, a chip and a rule: `any` is "Pets" (a dog or a cat). */
export const PET_LABEL: Record<PetKind, string> = { any: "Pets", dog: "Dogs", cat: "Cats" };

/** What a kind is called in the Pets page's switch: `any` is "All". */
export const PET_TAB: Record<PetKind, string> = { any: "All", dog: "Dogs", cat: "Cats" };

/** A kind out of a URL parameter, or undefined when it is not one (never passed to the server). */
export function petKindOf(v: string | null | undefined): PetKind | undefined {
  return v === "any" || v === "dog" || v === "cat" ? v : undefined;
}

/** "3 photos" / "1 photo". */
export const petPhotos = (count: number) => `${n(count)} ${count === 1 ? "photo" : "photos"}`;

/** How many photos a kind has in the summary. */
export function petCount(s: PetsSummary, kind: PetKind): number {
  return kind === "any" ? s.any : (s.kinds.find((k) => k.kind === kind)?.photos ?? 0);
}

/** The switch of the Pets page: "All", then each kind that has photos (a kind with none is not offered). */
export function offeredKinds(s: PetsSummary): PetKind[] {
  return PET_KINDS.filter((k) => k === "any" || petCount(s, k) > 0);
}

/** The accessible name of a kind's card: "Dogs, 12 photos". */
export const petCardLabel = (kind: PetKind, photos: number) => `${PET_LABEL[kind]}, ${petPhotos(photos)}`;

/** Is the scan of the library still going? (`total` counts the photos that can be scanned.) */
export const scanning = (s: Pick<PetsSummary, "scanned" | "total">) => s.total > 0 && s.scanned < s.total;

/** 0..99 while the scan is going (100 only when it is done), null when there is nothing to scan. */
export function scanPercent(s: Pick<PetsSummary, "scanned" | "total">): number | null {
  if (s.total <= 0) return null;
  if (s.scanned >= s.total) return 100;
  return Math.max(0, Math.min(99, Math.floor((s.scanned / s.total) * 100)));
}

/** "Looked at 1,200 of 5,000 photos": the progress line of the backfill. */
export const scanLine = (s: Pick<PetsSummary, "scanned" | "total">) => `Looked at ${n(s.scanned)} of ${n(s.total)} photos`;

/** A smart album's Pets rule in words: "Pets: dogs or cats", "Pets: dogs", "Pets: cats". */
export const petRuleLabel = (kind: PetKind) => `Pets: ${kind === "any" ? "dogs or cats" : kind === "dog" ? "dogs" : "cats"}`;

// Named pets (docs/pets.md, "Named pets"): the words and the small rules around a pet the owner made.

/** Most named pets one search or smart-album rule can name together (the server's limit, the same as people). */
export const MAX_FILTER_PETS = MAX_FILTER_PEOPLE;
export const NO_NAMED_PETS = "Name a pet on the Pets page to filter by it.";
export const FIND_A_PET = "Find a pet";
export const PET_FILTER_NOTE = "Photos with all of the chosen pets in them.";

/** "Dog" / "Cat", for a label. */
export const KIND_TITLE: Record<"dog" | "cat", string> = { dog: "Dog", cat: "Cat" };

/** Why `name` cannot be a pet's name, or null when it can. A pet is always named: blank is a problem. */
export function petNameProblem(name: string): string | null {
  if (name.trim() === "") return "Give your pet a name.";
  return personNameProblem(name);
}

/** The ids in a `pet` URL parameter: valid ids only, each once, at most [`MAX_FILTER_PETS`] (the same rules as people). */
export const petIdsOf = peopleIdsOf;

/** What a named pet's card is called for assistive technology: the name, how many photos, and what waits for an answer. */
export const namedPetLabel = (p: Pick<NamedPet, "name" | "photos" | "pending">) =>
  `${p.name}, ${petPhotos(p.photos)}${p.pending > 0 ? `, ${pendingLabel(p.pending)}` : ""}`;

/** The question over a proposed animal. */
export const petQuestion = (p: Pick<NamedPet, "name">) => `Is this ${p.name}?`;

/** The pets a filter or a rule can choose: not hidden, in the order given (most photos first). */
export const filterablePets = (pets: readonly NamedPet[]): NamedPet[] => pets.filter((p) => !p.hidden);

/** "Pet A", "Pet A and Pet B", "Pet A, Pet B and Pet C": the chip of a pet filter. A pet that is gone says so. */
export function petFilterLabel(ids: readonly string[], pets: readonly { id: string; name: string }[]): string {
  const names = ids.map((id) => pets.find((p) => p.id === id)?.name.trim() || "Unknown pet");
  return names.length <= 2 ? names.join(" and ") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** A smart-album rule in words: "Pet is Name", "Pet is Name and Name" (or, without the names, "Pet: 2 pets together"). */
export function petIdsRuleLabel(ids: readonly string[], pets: readonly { id: string; name: string }[] = []): string {
  if (pets.length > 0 && ids.length > 0 && ids.every((id) => pets.some((p) => p.id === id))) return `Pet is ${petFilterLabel(ids, pets)}`;
  return ids.length === 1 ? "Pet: 1 pet" : `Pet: ${ids.length} pets together`;
}

/** The named pets the Info panel lists for a photo: each once, however many of its animals are in it. */
export function petsNamed(animals: readonly PhotoPet[] | undefined): { id: string; name: string }[] {
  const out: { id: string; name: string }[] = [];
  for (const a of animals ?? []) if (a.pet && !out.some((p) => p.id === a.pet!.id)) out.push(a.pet);
  return out;
}

/** The toast after a pet was made or photos were added: how many photos joined. */
export const addedToPetMessage = (name: string, photos: number, made: boolean) =>
  made ? (photos === 0 ? `Made ${name}` : `Made ${name} with ${petPhotos(photos)}`) : `Added ${petPhotos(photos)} to ${name}`;

/** The toast after "Not this pet". */
export const removedFromPetMessage = (photos: number, name: string) => `Took ${petPhotos(photos)} out of ${name}`;

/** The toast after a merge. */
export const mergedPetMessage = (into: string) => `Merged into ${into}`;

/** Why a photo was left out, in plain words, one line per reason ("2 photos have no dog that was found"). */
export function skippedLines(skipped: readonly PetAdded["skipped"][number][], pet: { name: string; kind: "dog" | "cat" }): string[] {
  const count = (r: SkipReason) => skipped.filter((s) => s.reason === r).length;
  const lines: string[] = [];
  const none = count("no_animal");
  if (none > 0) lines.push(`${petPhotos(none)} ${none === 1 ? "has" : "have"} no ${pet.kind} that was found`);
  const unseen = count("not_scanned");
  if (unseen > 0) lines.push(`${petPhotos(unseen)} ${unseen === 1 ? "has" : "have"} not been looked through for pets yet`);
  const already = count("already");
  if (already > 0) lines.push(`${petPhotos(already)} ${already === 1 ? "is" : "are"} already in ${pet.name}`);
  const other = count("in_other_pet");
  if (other > 0) lines.push(`${petPhotos(other)} ${other === 1 ? "belongs" : "belong"} to another pet already`);
  return lines;
}
