// Selection state for photo grids, as pure functions so the range / select-all rules are easy to test.
// `anchor` is the last item clicked without Shift; Shift+click selects everything between it and the target.
// Nothing here knows what the selection is used for (trash today, albums later).
import { useCallback, useMemo, useState } from "react";

export interface Selection {
  readonly ids: ReadonlySet<string>;
  readonly anchor: string | null;
}

export const emptySelection: Selection = { ids: new Set(), anchor: null };

/** Add or remove one item; it becomes the anchor for a following range. */
export function toggle(sel: Selection, id: string): Selection {
  const ids = new Set(sel.ids);
  if (!ids.delete(id)) ids.add(id);
  return { ids, anchor: id };
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
  for (const x of order.slice(Math.min(from, to), Math.max(from, to) + 1)) ids.add(x);
  return { ids, anchor: id };
}

/** Select all of `order` (the loaded items). */
export function selectAll(order: readonly string[]): Selection {
  return { ids: new Set(order), anchor: order.at(-1) ?? null };
}

/** Forget selected ids that are no longer in `order` (e.g. after they were trashed). Returns `sel` when unchanged. */
export function prune(sel: Selection, order: readonly string[]): Selection {
  if (sel.ids.size === 0) return sel;
  const present = new Set(order);
  const ids = [...sel.ids].filter((id) => present.has(id));
  if (ids.length === sel.ids.size) return sel;
  return { ids: new Set(ids), anchor: sel.anchor !== null && present.has(sel.anchor) ? sel.anchor : null };
}

/** `order` filtered to the selected ids, so actions get them in grid order. */
export function selectedIds(sel: Selection, order: readonly string[]): string[] {
  return order.filter((id) => sel.ids.has(id));
}

export interface SelectionApi {
  /** Selection mode: tiles select instead of opening. */
  selecting: boolean;
  /** Selected ids (always a subset of the current `order`). */
  selected: ReadonlySet<string>;
  /** Selected ids in grid order. */
  ids: string[];
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

/** Selection mode and selection over the ids currently shown (`order`). */
export function useSelection(order: readonly string[]): SelectionApi {
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
  const selectAllLoaded = useCallback(() => setSel(selectAll(order)), [order]);
  const clear = useCallback(() => setSel(emptySelection), []);
  const escape = useCallback(() => {
    if (sel.ids.size > 0) setSel(emptySelection);
    else setSelecting(false);
  }, [sel.ids.size, setSelecting]);

  return {
    selecting,
    selected: sel.ids,
    ids: selectedIds(sel, order),
    select,
    begin,
    setSelecting,
    selectAll: selectAllLoaded,
    clear,
    escape,
  };
}
