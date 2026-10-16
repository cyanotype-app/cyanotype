// Selection state for photo grids, as pure functions so the range / select-all rules are easy to test.
// `anchor` is the last item clicked without Shift; Shift+click selects everything between it and the target.
// Nothing here knows what the selection is used for (trash today, albums later).
import { useCallback, useMemo, useState } from "react";
import type { Scope, Target } from "./api";

export interface Selection {
  /** The selected ids; in `all` mode the ids that were deselected from "everything in the view". */
  readonly ids: ReadonlySet<string>;
  readonly anchor: string | null;
  /** "Select all" over the whole view (not only what is loaded): everything except `ids` is selected. */
  readonly all?: boolean;
}

export const emptySelection: Selection = { ids: new Set(), anchor: null };

/** Add or remove one item; it becomes the anchor for a following range. */
export function toggle(sel: Selection, id: string): Selection {
  const ids = new Set(sel.ids);
  if (!ids.delete(id)) ids.add(id);
  return { ids, anchor: id, all: sel.all };
}

/**
 * Select every item from the anchor to `id` (inclusive, in `order`), keeping what was already selected.
 * Without a usable anchor (none yet, or no longer in `order`) it behaves like a plain toggle.
 */
export function selectRange(sel: Selection, order: readonly string[], id: string): Selection {
  const to = order.indexOf(id);
  const from = sel.anchor === null ? -1 : order.indexOf(sel.anchor);
  if (to < 0) return sel; // not a loaded item
  if (from < 0) return toggle(sel, id);
  const ids = new Set(sel.ids);
  // In `all` mode ids are the exceptions, so selecting a range takes them out.
  for (const x of order.slice(Math.min(from, to), Math.max(from, to) + 1)) {
    if (sel.all) ids.delete(x);
    else ids.add(x);
  }
  return { ids, anchor: id, all: sel.all };
}

/** Select all of `order` (the loaded items). */
export function selectAll(order: readonly string[]): Selection {
  return { ids: new Set(order), anchor: order.at(-1) ?? null };
}

/** Select everything in the view, loaded or not. */
export const everything: Selection = { ids: new Set(), anchor: null, all: true };

/** Forget selected ids that are no longer in `order` (e.g. after they were trashed). Returns `sel` when unchanged. */
export function prune(sel: Selection, order: readonly string[]): Selection {
  if (sel.all || sel.ids.size === 0) return sel;
  const present = new Set(order);
  const ids = [...sel.ids].filter((id) => present.has(id));
  if (ids.length === sel.ids.size) return sel;
  return { ids: new Set(ids), anchor: sel.anchor !== null && present.has(sel.anchor) ? sel.anchor : null };
}

/** `order` filtered to the selected ids, so actions get them in grid order. */
export function selectedIds(sel: Selection, order: readonly string[]): string[] {
  return order.filter((id) => sel.ids.has(id) !== (sel.all === true));
}

/** How many items are selected, given how many the view holds in all. */
export function selectedCount(sel: Selection, total: number): number {
  return sel.all ? Math.max(0, total - sel.ids.size) : sel.ids.size;
}

/** What a bulk call should send: explicit ids, or the view's scope minus the deselected items. */
export function targetOf(sel: Selection, order: readonly string[], scope: Scope): Target {
  return sel.all ? { scope, except: [...sel.ids] } : selectedIds(sel, order);
}

export interface SelectionApi {
  /** Selection mode: tiles select instead of opening. */
  selecting: boolean;
  /** Selected loaded ids (in `all` mode: every loaded id that was not deselected). */
  selected: ReadonlySet<string>;
  /** Selected loaded ids in grid order. */
  ids: string[];
  /** "Select all" covers the whole view, not only the loaded photos. */
  all: boolean;
  /** How many are selected: exact, from the view's total, in `all` mode. */
  count: number;
  /** What to send to a bulk endpoint for this selection. */
  target: (scope: Scope) => Target;
  /** Click / tap on an item. With `shift`, selects the range from the last clicked item. */
  select: (id: string, shift?: boolean) => void;
  /** Enter selection mode with `id` selected (long-press). */
  begin: (id: string) => void;
  setSelecting: (on: boolean) => void;
  selectAll: () => void;
  clear: () => void;
  /** Escape: clear the selection; when it is already empty, leave selection mode. */
  escape: () => void;
}

/**
 * Selection mode and selection over the ids currently shown (`order`). `total` is how many items the whole view
 * holds; "Select all" then selects all of them (the server resolves the ones that are not loaded). Without it,
 * "Select all" covers the loaded items only.
 */
export function useSelection(order: readonly string[], total?: number): SelectionApi {
  const [selecting, setSelectingState] = useState(false);
  const [raw, setSel] = useState<Selection>(emptySelection);
  // Items that disappear (trashed, restored, rolled back) drop out of the selection.
  const sel = useMemo(() => prune(raw, order), [raw, order]);

  const setSelecting = useCallback((on: boolean) => {
    setSelectingState(on);
    if (!on) setSel(emptySelection);
  }, []);
  const select = useCallback(
    (id: string, shift = false) => setSel((s) => (shift ? selectRange(s, order, id) : toggle(s, id))),
    [order],
  );
  const begin = useCallback((id: string) => {
    setSelectingState(true);
    setSel({ ids: new Set([id]), anchor: id });
  }, []);
  const hasTotal = total !== undefined;
  const selectAllInView = useCallback(
    () => setSel(hasTotal ? { ...everything, anchor: order.at(-1) ?? null } : selectAll(order)),
    [order, hasTotal],
  );
  const clear = useCallback(() => setSel(emptySelection), []);
  const count = selectedCount(sel, total ?? order.length);
  const escape = useCallback(() => {
    if (count > 0) setSel(emptySelection);
    else setSelecting(false);
  }, [count, setSelecting]);
  const ids = useMemo(() => selectedIds(sel, order), [sel, order]);
  const selected = useMemo(() => (sel.all ? new Set(ids) : sel.ids), [sel, ids]);
  const target = useCallback((scope: Scope) => targetOf(sel, order, scope), [sel, order]);

  return {
    selecting,
    selected,
    ids,
    all: sel.all === true,
    count,
    target,
    select,
    begin,
    setSelecting,
    selectAll: selectAllInView,
    clear,
    escape,
  };
}
