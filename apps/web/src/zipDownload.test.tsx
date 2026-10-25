import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, MAX_ZIP_IDS, zipApi, zipBody } from "./api";
import { ZipButton } from "./components/ZipButton";
import type { PublicZipPlan } from "./shareApi";
import { followLink, planShareZip, sharePlanMessage, submitZip, zipStartedMessage } from "./zipDownload";

afterEach(() => vi.restoreAllMocks());

const plan = (over: Partial<PublicZipPlan> = {}): PublicZipPlan => ({
  files: 3,
  bytes: 3 * 1024 * 1024,
  pending: 0,
  skipped: 0,
  name: "Alpha.zip",
  max_files: 1000,
  max_bytes: 4 << 30,
  ...over,
});

describe("zipBody", () => {
  it("sends ids, or a scope with its exceptions and no ids, plus the file name when there is one", () => {
    expect(zipBody(["a", "b"])).toEqual({ ids: ["a", "b"] });
    expect(zipBody({ scope: { album: "x" }, except: ["c"] }, "Alpha trip")).toEqual({
      scope: { album: "x" },
      except: ["c"],
      name: "Alpha trip",
    });
    expect(zipBody({ scope: "library", except: [] })).toEqual({ scope: "library", except: [] });
  });
});

describe("messages", () => {
  it("say how many files and how big, and what a link leaves out", () => {
    expect(zipStartedMessage({ files: 1, bytes: 2048 })).toBe("Downloading 1 file (2.0 KB) as a ZIP. Your browser shows the progress.");
    expect(zipStartedMessage({ files: 4000, bytes: 5 * 1024 ** 3 })).toContain("4,000 files (5.0 GB)");
    expect(sharePlanMessage({ files: 2, bytes: 10, skipped: 1 })).toContain("1 photo is not available to download.");
    expect(sharePlanMessage({ files: 2, bytes: 10, skipped: 3 })).toContain("3 photos are not available to download.");
    expect(sharePlanMessage({ files: 2, bytes: 10, skipped: 0 })).not.toContain("not available");
  });
});

describe("submitZip", () => {
  it("posts the selection from a hidden form (the browser streams the answer) and leaves nothing behind", () => {
    let seen: { method: string; action: string; selection: string } | null = null;
    vi.spyOn(HTMLFormElement.prototype, "submit").mockImplementation(function (this: HTMLFormElement) {
      seen = {
        method: this.method,
        action: new URL(this.action).pathname,
        selection: (this.elements.namedItem("selection") as HTMLInputElement).value,
      };
      expect(document.body.contains(this)).toBe(true);
    });
    submitZip({ ids: ["a"], name: "Alpha" });
    expect(seen).toEqual({ method: "post", action: "/assets/zip", selection: JSON.stringify({ ids: ["a"], name: "Alpha" }) });
    expect(document.querySelector("form")).toBeNull();
  });
});

describe("followLink", () => {
  it("clicks a temporary download link", () => {
    let href = "";
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      href = this.getAttribute("href") ?? "";
      expect(this.hasAttribute("download")).toBe(true);
    });
    followLink("/assets/public/s/tok/zip");
    expect(href).toBe("/assets/public/s/tok/zip");
    expect(document.querySelector("a")).toBeNull();
  });
});

describe("planShareZip", () => {
  it("asks again while download copies are being made, then returns the plan", async () => {
    const ask = vi
      .fn<() => Promise<PublicZipPlan>>()
      .mockResolvedValueOnce(plan({ pending: 2 }))
      .mockResolvedValueOnce(plan({ pending: 1 }))
      .mockResolvedValueOnce(plan());
    const waiting: number[] = [];
    const sleep = vi.fn().mockResolvedValue(undefined);
    const got = await planShareZip(ask, { sleep, onWaiting: (n) => waiting.push(n) });
    expect(got.pending).toBe(0);
    expect(ask).toHaveBeenCalledTimes(3);
    expect(waiting).toEqual([2, 1]);
    expect(sleep).toHaveBeenCalledTimes(2);
  });

  it("gives up with a message instead of waiting for ever", async () => {
    const ask = vi.fn().mockResolvedValue(plan({ pending: 1 }));
    await expect(planShareZip(ask, { sleep: async () => undefined, tries: 3 })).rejects.toThrow(/still being prepared/);
    expect(ask).toHaveBeenCalledTimes(3);
  });

  it("passes the server's refusal on", async () => {
    const ask = vi.fn().mockRejectedValue(new ApiError(413, "this link has too many photos for one download"));
    await expect(planShareZip(ask)).rejects.toThrow("too many photos");
  });
});

describe("ZipButton", () => {
  const setup = (target: Parameters<typeof ZipButton>[0]["target"], name?: string) => {
    const submit = vi.fn();
    const onStarted = vi.fn();
    const onError = vi.fn();
    render(<ZipButton target={target} name={name} onStarted={onStarted} onError={onError} submit={submit} />);
    return { submit, onStarted, onError };
  };

  it("checks the plan, then starts the download of the same request", async () => {
    const planCall = vi.spyOn(zipApi, "plan").mockResolvedValue({ files: 2, bytes: 4096, name: "Trip.zip" });
    const { submit, onStarted, onError } = setup({ scope: { album: "al" }, except: ["x"] }, "Trip");
    await userEvent.setup().click(screen.getByRole("button", { name: "Download" }));
    const body = { scope: { album: "al" }, except: ["x"], name: "Trip" };
    await waitFor(() => expect(submit).toHaveBeenCalledWith(body));
    expect(planCall).toHaveBeenCalledWith(body);
    expect(onStarted).toHaveBeenCalledWith("Downloading 2 files (4.0 KB) as a ZIP. Your browser shows the progress.");
    expect(onError).not.toHaveBeenCalled();
    // Ready for another go.
    expect(screen.getByRole("button", { name: "Download" })).toBeEnabled();
  });

  it("does not start a download the server refused", async () => {
    vi.spyOn(zipApi, "plan").mockRejectedValue(new ApiError(413, "an archive holds at most 50000 files; download it in parts"));
    const { submit, onStarted, onError } = setup(["a"]);
    await userEvent.setup().click(screen.getByRole("button", { name: "Download" }));
    await waitFor(() => expect(onError).toHaveBeenCalled());
    expect((onError.mock.calls[0]![0] as Error).message).toContain("in parts");
    expect(submit).not.toHaveBeenCalled();
    expect(onStarted).not.toHaveBeenCalled();
  });

  it("turns away a hand-picked selection the server would refuse, without asking", async () => {
    const planCall = vi.spyOn(zipApi, "plan");
    const { submit, onError } = setup(Array.from({ length: MAX_ZIP_IDS + 1 }, (_, i) => `id-${i}`));
    await userEvent.setup().click(screen.getByRole("button", { name: "Download" }));
    expect((onError.mock.calls[0]![0] as Error).message).toContain("Select all");
    expect(planCall).not.toHaveBeenCalled();
    expect(submit).not.toHaveBeenCalled();
  });

  it("is disabled while it prepares", async () => {
    let finish: (v: { files: number; bytes: number; name: string }) => void = () => undefined;
    vi.spyOn(zipApi, "plan").mockReturnValue(new Promise((r) => (finish = r)));
    setup(["a"]);
    await userEvent.setup().click(screen.getByRole("button", { name: "Download" }));
    expect(screen.getByRole("button", { name: "Preparing…" })).toBeDisabled();
    finish({ files: 1, bytes: 1, name: "x.zip" });
    expect(await screen.findByRole("button", { name: "Download" })).toBeEnabled();
  });
});
