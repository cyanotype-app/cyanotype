// Selection state for photo grids, as pure functions so the range / select-all rules are easy to test.
// `anchor` is the last item clicked without Shift; Shift+click selects everything between it and the target.
// Nothing here knows what the selection is used for (trash today, albums later).
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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

/** Add or remove several items as one (a collapsed burst): all of them go out when all are in, else all come in. */
export function toggleMany(sel: Selection, ids: readonly string[]): Selection {
  const picked = (id: string) => sel.ids.has(id) !== (sel.all === true);
  const everySelected = ids.length > 0 && ids.every(picked);
  const next = new Set(sel.ids);
  // In `all` mode the ids are the exceptions: selecting takes them out, deselecting puts them in.
  const remove = sel.all === true ? !everySelected : everySelected;
  for (const id of ids) {
    if (remove) next.delete(id);
    else next.add(id);
  }
  return { ids: next, anchor: ids[0] ?? sel.anchor, all: sel.all };
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
  /** Click / tap on a group of items (a collapsed burst): selects or deselects all of them together. */
  selectMany: (ids: readonly string[]) => void;
  /** Enter selection mode with `id` selected (long-press). */
  begin: (id: string) => void;
  /** Enter selection mode with these selected (long-press on a collapsed burst). */
  beginMany: (ids: readonly string[]) => void;
  /**
   * A long press on a tile (touch): selection mode starts (or goes on) with the tile selected, and `dragOver` then selects
   * everything from it to the tile under the finger, in either direction, until `dragEnd`.
   */
  dragStart: (id: string) => void;
  dragOver: (id: string) => void;
  dragEnd: () => void;
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
  const selectMany = useCallback((ids: readonly string[]) => setSel((s) => toggleMany(s, ids)), []);
  const begin = useCallback((id: string) => {
    setSelectingState(true);
    setSel({ ids: new Set([id]), anchor: id });
  }, []);
  const beginMany = useCallback((ids: readonly string[]) => {
    setSelectingState(true);
    setSel({ ids: new Set(ids), anchor: ids[0] ?? null });
  }, []);
  // The latest state for the drag, which runs between renders.
  const live = useRef({ raw, order });
  useEffect(() => {
    live.current = { raw, order };
  });
  const drag = useRef<{ base: Selection; anchor: string } | null>(null);
  const dragStart = useCallback((id: string) => {
    const base = live.current.raw;
    drag.current = { base, anchor: id };
    setSelectingState(true);
    const ids = new Set(base.ids);
    if (base.all) ids.delete(id);
    else ids.add(id);
    setSel({ ids, anchor: id, all: base.all });
  }, []);
  const dragOver = useCallback((id: string) => {
    const d = drag.current;
    if (!d) return;
    const { order: o } = live.current;
    const a = o.indexOf(d.anchor);
    const b = o.indexOf(id);
    if (a < 0 || b < 0) return;
    const ids = new Set(d.base.ids);
    for (const x of o.slice(Math.min(a, b), Math.max(a, b) + 1)) {
      if (d.base.all) ids.delete(x);
      else ids.add(x);
    }
    setSel({ ids, anchor: d.anchor, all: d.base.all });
  }, []);
  const dragEnd = useCallback(() => {
    drag.current = null;
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
    selectMany,
    begin,
    beginMany,
    dragStart,
    dragOver,
    dragEnd,
    setSelecting,
    selectAll: selectAllInView,
    clear,
    escape,
  };
}
