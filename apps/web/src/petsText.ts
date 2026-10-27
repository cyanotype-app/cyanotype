// Pure helpers of Pets: the words (kinds, counts, progress), reading a kind out of a URL, and which kinds the page offers. Unit tested.
import type { PetKind, PetsSummary } from "./api";

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
