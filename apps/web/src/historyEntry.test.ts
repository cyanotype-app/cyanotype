import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createHistoryEntry } from "./historyEntry";

const settle = () => new Promise((r) => setTimeout(r, 30));

describe("createHistoryEntry", () => {
  beforeEach(() => {
    window.history.replaceState(null, "", "/start");
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("pushes once and marks the state", () => {
    const e = createHistoryEntry("cyInfo");
    const before = window.history.length;
    e.push();
    e.push();
    expect(window.history.length).toBe(before + 1);
    expect(e.has()).toBe(true);
  });

  it("pops once however many closes arrive before popstate", async () => {
    const e = createHistoryEntry("cyInfo");
    e.push();
    const back = vi.spyOn(window.history, "back");
    const a = e.pop();
    const b = e.pop();
    const c = e.pop();
    expect(back).toHaveBeenCalledTimes(1);
    expect(e.has()).toBe(false);
    await Promise.all([a, b, c]);
    expect(window.location.pathname).toBe("/start");
    expect(e.has()).toBe(false);
  });

  it("does nothing when its entry is not current", async () => {
    const e = createHistoryEntry("cyInfo");
    const back = vi.spyOn(window.history, "back");
    await e.pop();
    expect(back).not.toHaveBeenCalled();
    e.push();
    await e.pop();
    await settle();
    await e.pop();
    expect(back).toHaveBeenCalledTimes(1);
  });

  it("can be pushed and popped again after a pop", async () => {
    const e = createHistoryEntry("cyInfo");
    e.push();
    await e.pop();
    e.push();
    expect(e.has()).toBe(true);
    await e.pop();
    expect(e.has()).toBe(false);
  });

  it("stops waiting if the browser never reports the pop", async () => {
    const e = createHistoryEntry("cyInfo");
    e.push();
    vi.useFakeTimers();
    vi.spyOn(window.history, "back").mockImplementation(() => {});
    let done = false;
    void e.pop().then(() => (done = true));
    await vi.advanceTimersByTimeAsync(1100);
    expect(done).toBe(true);
  });
});
