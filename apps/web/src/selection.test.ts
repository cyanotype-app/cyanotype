import { describe, expect, it } from "vitest";
import { renderHook, act } from "@testing-library/react";
import {
  emptySelection,
  everything,
  prune,
  selectAll,
  selectRange,
  selectedCount,
  selectedIds,
  targetOf,
  toggle,
  toggleMany,
  useSelection,
} from "./selection";

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

describe("selecting a group (a collapsed burst)", () => {
  it("selects all of it, then deselects all of it, and completes a partly selected group", () => {
    let s = toggleMany(emptySelection, ["a", "b", "c"]);
    expect(ids(s)).toEqual(["a", "b", "c"]);
    s = toggleMany(s, ["a", "b", "c"]);
    expect(ids(s)).toEqual([]);
    s = toggle(emptySelection, "b");
    s = toggleMany(s, ["a", "b", "c"]);
    expect(ids(s)).toEqual(["a", "b", "c"]);
  });

  it("works on the exceptions of an everything-selected view", () => {
    let s = toggleMany(everything, ["a", "b"]);
    expect(ids(s)).toEqual(["a", "b"]);
    expect(selectedCount(s, 5)).toBe(3);
    s = toggleMany(s, ["a", "b"]);
    expect(ids(s)).toEqual([]);
  });

  it("is what a click and a long press on a collapsed burst do, through the hook", () => {
    const { result } = renderHook(() => useSelection(order));
    act(() => result.current.beginMany(["a", "b"]));
    expect([result.current.selecting, [...result.current.selected].sort(), result.current.count]).toEqual([true, ["a", "b"], 2]);
    act(() => result.current.selectMany(["c", "d"]));
    expect(result.current.count).toBe(4);
    act(() => result.current.selectMany(["a", "b"]));
    expect([...result.current.selected].sort()).toEqual(["c", "d"]);
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

describe("select all over the whole view", () => {
  it("counts from the view total minus the deselected ids", () => {
    expect(selectedCount(everything, 48_213)).toBe(48_213);
    const s = toggle(toggle(everything, "b"), "c"); // deselect two
    expect(selectedCount(s, 48_213)).toBe(48_211);
    expect(selectedIds(s, order)).toEqual(["a", "d", "e"]);
    expect(selectedCount(toggle(s, "b"), 48_213)).toBe(48_212); // select one back
    expect(selectedCount(selectAll(order), 99)).toBe(5); // explicit selections count what they hold
  });

  it("shift-click selects a range again (takes it out of the exceptions)", () => {
    let s = toggle(toggle(toggle(everything, "a"), "b"), "d"); // a, b, d deselected
    s = { ...s, anchor: "a" };
    s = selectRange(s, order, "c");
    expect([...s.ids]).toEqual(["d"]);
  });

  it("is never pruned (the exceptions may be photos that are not loaded)", () => {
    const s = toggle(everything, "zzz");
    expect(prune(s, order)).toBe(s);
  });

  it("builds explicit ids, or the scope with the exceptions", () => {
    expect(targetOf(toggle(emptySelection, "b"), order, "library")).toEqual(["b"]);
    expect(targetOf(toggle(everything, "b"), order, "favorites")).toEqual({ scope: "favorites", except: ["b"] });
    expect(targetOf(everything, order, { album: "x" })).toEqual({ scope: { album: "x" }, except: [] });
  });

  it("the hook selects everything with a total, and only the loaded ones without", () => {
    const { result } = renderHook(() => useSelection(order, 500));
    act(() => result.current.setSelecting(true));
    act(() => result.current.selectAll());
    expect([result.current.all, result.current.count, result.current.ids]).toEqual([true, 500, order]);
    expect(result.current.target("trash")).toEqual({ scope: "trash", except: [] });
    act(() => result.current.select("c")); // deselect one
    expect(result.current.count).toBe(499);
    expect(result.current.selected.has("c")).toBe(false);
    expect(result.current.target("trash")).toEqual({ scope: "trash", except: ["c"] });
    act(() => result.current.escape());
    expect([result.current.all, result.current.count]).toEqual([false, 0]);

    const loadedOnly = renderHook(() => useSelection(order));
    act(() => loadedOnly.result.current.selectAll());
    expect([loadedOnly.result.current.all, loadedOnly.result.current.count]).toEqual([false, 5]);
  });
});
