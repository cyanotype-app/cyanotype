// Stacks in the timeline: two or more photos (a burst the worker found, or ones the owner stacked) shown as one tile, the
// cover. Pure helpers for what a tile stands for when it is selected, what the viewer shows while stepping through a stack, and
// the words of the controls. Kept apart from the components so the rules are unit tested.
import type { AssetListItem, Target } from "./api";
import { isScope } from "./api";

/** How many photos a tile stands for (1 for a photo on its own). */
export const photosIn = (it: AssetListItem): number => it.stack?.size ?? 1;

/** The ids a tile stands for, its own first: its stack's photos while the stack is collapsed to it, else the photo alone. */
export function tileIds(it: AssetListItem): string[] {
  const members = it.stack?.members;
  return members && members.length > 0 ? [it.id, ...members.filter((m) => m !== it.id)] : [it.id];
}

/** Explicit ids to the ids of every photo behind them: a collapsed stack's tile stands for all of its photos. */
export function expandIds(items: readonly AssetListItem[], ids: readonly string[]): string[] {
  const byId = new Map(items.map((it) => [it.id, it]));
  const out = new Set<string>();
  for (const id of ids) {
    const it = byId.get(id);
    for (const m of it ? tileIds(it) : [id]) out.add(m);
  }
  return [...out];
}

/** A bulk target with the tiles' stacks expanded (explicit ids, or the ids a "select all" leaves out). */
export function expandTarget(target: Target, items: readonly AssetListItem[]): Target {
  return isScope(target) ? { ...target, except: expandIds(items, target.except) } : expandIds(items, target);
}

/** How many photos the selected tiles hide inside their stacks (the selection bar says so). */
export function hiddenInStacks(items: readonly AssetListItem[], selected: ReadonlySet<string>): number {
  let n = 0;
  for (const it of items) if (selected.has(it.id)) n += tileIds(it).length - 1;
  return n;
}

/** The note under the selection count, or null. */
export function selectionNote(hidden: number): string | null {
  return hidden > 0 ? `plus ${hidden.toLocaleString("en-US")} more in stacks` : null;
}

/** The photo as a photo on its own (no stack), so that an action on it means this one photo. */
export function alone(it: AssetListItem): AssetListItem {
  if (!it.stack) return it;
  const rest = { ...it };
  delete rest.stack;
  return rest;
}

/** Position of `id` in the stack's photos, or 0 (the first) when it is not there. */
export function positionOf(items: readonly AssetListItem[], id: string | null): number {
  const i = id === null ? -1 : items.findIndex((it) => it.id === id);
  return i < 0 ? 0 : i;
}

/** The position `delta` steps from `at`, stopping at both ends. */
export const step = (at: number, delta: number, length: number): number => Math.min(Math.max(at + delta, 0), Math.max(length - 1, 0));

/** The viewer's photos with the one at `at` replaced by the stack member on show (the stack's other photos are not in the timeline). */
export function substitute(run: readonly AssetListItem[], at: number, member: AssetListItem | undefined): AssetListItem[] {
  if (!member || at < 0 || at >= run.length) return [...run];
  const out = [...run];
  out[at] = member;
  return out;
}

export const stackLabel = (size: number): string => `Stack of ${size.toLocaleString("en-US")} photos`;

export const positionLabel = (position: number, size: number): string => `Photo ${position + 1} of ${size} in this stack`;

export const STACK_MESSAGES = {
  stacked: (n: number) => `${n.toLocaleString("en-US")} photos stacked`,
  cover: "Cover changed",
  removed: "Taken out of the stack",
  dissolved: "Stack removed",
  failed: "Couldn’t change the stack.",
  /** The selection bar's button is for two or more tiles, or a stack's tile alone. */
  needTwo: "Pick two or more photos to stack",
} as const;

/** Can the selected tiles be stacked? Two or more photos in all, no "select all" (it can hold more than a stack may), and at most `max`. */
export function canStack(items: readonly AssetListItem[], selected: ReadonlySet<string>, all: boolean, max = 100): boolean {
  if (all) return false;
  let n = 0;
  for (const it of items) if (selected.has(it.id)) n += tileIds(it).length;
  return n >= 2 && n <= max;
}
