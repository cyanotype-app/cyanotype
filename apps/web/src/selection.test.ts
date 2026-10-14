import { describe, expect, it } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { emptySelection, prune, selectAll, selectRange, selectedIds, toggle, useSelection } from "./selection";

const order = ["a", "b", "c", "d", "e"];
const ids = (s: { ids: ReadonlySet<string> }) => [...s.ids].sort();

describe("selection", () => {
  it("toggles items and remembers the last one as the anchor", () => {
    let s = toggle(emptySelection, "b");
    expect([ids(s), s.anchor]).toEqual([["b"], "b"]);
    s = toggle(s, "d");
    expect([ids(s), s.anchor]).toEqual([["b", "d"], "d"]);
    s = toggle(s, "b");
    expect([ids(s), s.anchor]).toEqual([["d"], "b"]);
  });

  it("selects a range from the anchor in either direction, keeping the rest", () => {
    let s = toggle(emptySelection, "b");
    s = selectRange(s, order, "d");
    expect(ids(s)).toEqual(["b", "c", "d"]);
    expect(s.anchor).toBe("d");
    // Backwards from the new anchor, extending what was there.
    s = selectRange(s, order, "a");
    expect(ids(s)).toEqual(["a", "b", "c", "d"]);
    // A range from an anchor, with an unrelated item already selected.
    let t = toggle(toggle(emptySelection, "e"), "a");
    t = selectRange(t, order, "c");
    expect(ids(t)).toEqual(["a", "b", "c", "e"]);
  });

  it("falls back to a toggle when there is no usable anchor", () => {
    expect(ids(selectRange(emptySelection, order, "c"))).toEqual(["c"]);
    const stale = { ids: new Set(["zzz"]), anchor: "zzz" };
    expect(ids(selectRange(stale, order, "c"))).toEqual(["c", "zzz"]);
    expect(ids(selectRange(toggle(emptySelection, "a"), order, "nope"))).toEqual(["a"]);
  });

  it("selects all loaded items", () => {
    expect(ids(selectAll(order))).toEqual(order);
    expect(selectAll([]).ids.size).toBe(0);
  });

  it("prunes ids that are gone and returns the same object when nothing changed", () => {
    const s = selectAll(order);
    expect(prune(s, order)).toBe(s);
    const p = prune(s, ["a", "c"]);
    expect(ids(p)).toEqual(["a", "c"]);
    expect(p.anchor).toBeNull();
    expect(prune(emptySelection, [])).toBe(emptySelection);
  });

  it("lists selected ids in grid order", () => {
    const s = toggle(toggle(emptySelection, "d"), "b");
    expect(selectedIds(s, order)).toEqual(["b", "d"]);
  });
});

describe("useSelection", () => {
  it("handles click, shift-click, select all, clear and escape", () => {
    const { result } = renderHook(() => useSelection(order));
    expect(result.current.selecting).toBe(false);

    act(() => result.current.setSelecting(true));
    act(() => result.current.select("b"));
    act(() => result.current.select("d", true));
    expect(result.current.ids).toEqual(["b", "c", "d"]);

    act(() => result.current.escape()); // first Escape clears the selection...
    expect([result.current.ids, result.current.selecting]).toEqual([[], true]);
    act(() => result.current.escape()); // ...the next one leaves selection mode
    expect(result.current.selecting).toBe(false);

    act(() => result.current.begin("c")); // long-press
    expect([result.current.ids, result.current.selecting]).toEqual([["c"], true]);
    act(() => result.current.selectAll());
    expect(result.current.ids).toEqual(order);
    act(() => result.current.clear());
    expect(result.current.ids).toEqual([]);
  });

  it("drops items that disappear from the list", () => {
    let current = order;
    const { result, rerender } = renderHook(() => useSelection(current));
    act(() => result.current.selectAll());
    current = ["a", "e"];
    rerender();
    expect(result.current.ids).toEqual(["a", "e"]);
  });

  it("leaving selection mode clears the selection", () => {
    const { result } = renderHook(() => useSelection(order));
    act(() => result.current.begin("a"));
    act(() => result.current.setSelecting(false));
    act(() => result.current.setSelecting(true));
    expect(result.current.ids).toEqual([]);
  });
});
