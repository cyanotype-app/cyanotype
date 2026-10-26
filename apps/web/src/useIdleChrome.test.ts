import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDLE_MS, useIdleChrome } from "./useIdleChrome";

const root = { current: null };

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("useIdleChrome", () => {
  it("shows at first, fades after a quiet spell, and comes back on any interaction", () => {
    const { result } = renderHook(() => useIdleChrome(root, false));
    expect(result.current.visible).toBe(true);
    act(() => void vi.advanceTimersByTime(IDLE_MS + 10));
    expect(result.current.visible).toBe(false);
    act(() => result.current.show());
    expect(result.current.visible).toBe(true);
    // ... and the quiet spell starts over from the interaction.
    act(() => void vi.advanceTimersByTime(IDLE_MS - 100));
    expect(result.current.visible).toBe(true);
    act(() => void vi.advanceTimersByTime(200));
    expect(result.current.visible).toBe(false);
  });

  it("a tap toggles it", () => {
    const { result } = renderHook(() => useIdleChrome(root, false));
    act(() => result.current.toggle());
    expect(result.current.visible).toBe(false);
    act(() => result.current.toggle());
    expect(result.current.visible).toBe(true);
  });

  it("stays while held (a panel is open) and while a menu is open", () => {
    const { result, rerender } = renderHook(({ hold }) => useIdleChrome(root, hold), { initialProps: { hold: true } });
    act(() => void vi.advanceTimersByTime(IDLE_MS * 3));
    expect(result.current.visible).toBe(true);
    rerender({ hold: false });
    const menu = document.createElement("div");
    menu.setAttribute("role", "menu");
    document.body.append(menu);
    act(() => void vi.advanceTimersByTime(IDLE_MS * 2));
    expect(result.current.visible).toBe(true);
    menu.remove();
    act(() => void vi.advanceTimersByTime(IDLE_MS + 10));
    expect(result.current.visible).toBe(false);
  });
});
