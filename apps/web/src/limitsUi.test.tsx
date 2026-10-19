// Limits in the UI: the upload queue stopping on a quota or daily cap refusal, the tray saying why, the Storage page's meters
// and warnings at 80 % and 95 %, and the admin's limit editing.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, type AdminAccounts, type CheckResponse, type MyUsage } from "./api";
import { Accounts } from "./pages/admin/Accounts";
import { Storage } from "./pages/Storage";
import { UploadQueue, type QueueDeps } from "./upload/queue";
import { UploadProvider, useUploads } from "./upload/UploadProvider";
import { UploadTray } from "./upload/UploadTray";

const GB = 1024 ** 3;
const file = (name: string, size = 10) => new File([new Uint8Array(size)], name, { lastModified: 0 });
const sha = "cd".repeat(32);
const upload = (): CheckResponse => ({ status: "upload", upload_id: "u", part_size: 100, parts: [{ number: 1, url: "/p/1" }], uploaded: [] });

function queueWith(check: QueueDeps["api"]["checkUpload"]) {
  const api = {
    checkUpload: vi.fn(check),
    putPart: vi.fn(async () => ({ etag: '"e"' })),
    completeUpload: vi.fn(async () => ({ asset_id: "a1" })),
  };
  return { api, q: new UploadQueue({ api, hash: async () => sha, delay: async () => {}, fileConcurrency: 1 }) };
}
const settle = (q: UploadQueue) => vi.waitFor(() => expect(q.active).toBe(false));

const usage = (over: Partial<MyUsage> = {}): MyUsage => ({
  originals_bytes: 3 * GB,
  trash_bytes: GB,
  stored_bytes: 4 * GB,
  derived_bytes: 0,
  derived_complete: true,
  photo_count: 10,
  raw_count: 0,
  file_count: 10,
  trash_count: 1,
  uploads: { count_24h: 1, bytes_24h: GB / 2, count_30d: 1, bytes_30d: GB / 2 },
  limits: { quota_bytes: 5 * GB, daily_upload_bytes: GB, max_file_bytes: 200 * 1024 ** 2 },
  daily: { used_bytes: GB / 2, oldest_rolls_out_at: new Date(Date.now() + 3 * 3_600_000).toISOString() },
  ...over,
});

afterEach(() => vi.restoreAllMocks());

describe("upload queue and limits", () => {
  it("stops the files not started when the quota refuses one, and says why once", async () => {
    const msg = "Not enough storage: this file needs 10 B and your account has 0 B left of 5 B.";
    const { api, q } = queueWith(async () => {
      throw new ApiError(413, msg, "quota_exceeded", { code: "quota_exceeded" });
    });
    q.add([file("a.jpg"), file("b.jpg"), file("c.jpg")]);
    await settle(q);
    const items = q.getSnapshot();
    expect(items.map((i) => i.status)).toEqual(["failed", "stopped", "stopped"]);
    expect(api.checkUpload).toHaveBeenCalledTimes(1); // the others were never sent to the server
    expect(items.every((i) => i.block?.code === "quota_exceeded" && i.error === msg)).toBe(true);
    const s = q.summary();
    expect((s.failed, s.stopped, s.blocked?.code)).toBe("quota_exceeded");
    expect(s.active).toBe(false);
  });

  it("keeps the daily cap's resume time", async () => {
    const at = "2030-01-02T03:04:05Z";
    const { q } = queueWith(async () => {
      throw new ApiError(429, "Daily upload limit reached.", "daily_cap_exceeded", { retry_at: at });
    });
    q.add([file("a.jpg"), file("b.jpg")]);
    await settle(q);
    expect(q.summary().blocked).toEqual({ code: "daily_cap_exceeded", message: "Daily upload limit reached.", retryAt: at });
  });

  it("lets a single oversized file fail alone and carries on", async () => {
    let n = 0;
    const { q } = queueWith(async () => {
      if (n++ === 0) throw new ApiError(413, "This file is 10 B and the largest file your account may upload is 5 B.", "file_too_large");
      return upload();
    });
    q.add([file("big.jpg"), file("small.jpg")]);
    await settle(q);
    expect(q.getSnapshot().map((i) => i.status)).toEqual(["failed", "done"]);
    expect(q.summary().blocked).toBeNull();
  });

  it("queues the stopped files again on retryBlocked", async () => {
    let refuse = true;
    const { api, q } = queueWith(async () => {
      if (refuse) throw new ApiError(429, "cap", "daily_cap_exceeded", { retry_at: "2030-01-02T03:04:05Z" });
      return upload();
    });
    q.add([file("a.jpg"), file("b.jpg")]);
    await settle(q);
    refuse = false;
    q.retryBlocked();
    await settle(q);
    expect(q.getSnapshot().map((i) => i.status)).toEqual(["done", "done"]);
    expect(api.checkUpload).toHaveBeenCalledTimes(3);
    expect(q.summary().blocked).toBeNull();
  });

  it("does not treat an ordinary 429 or a 413 without a code as a stop", async () => {
    const { q } = queueWith(async () => {
      throw new ApiError(429, "slow down");
    });
    q.add([file("a.jpg"), file("b.jpg")]);
    await settle(q);
    expect(q.getSnapshot().map((i) => i.status)).toEqual(["failed", "failed"]);
    expect(q.summary().blocked).toBeNull();
  });
});

function Adder({ files }: { files: File[] }) {
  const { queue } = useUploads();
  return (
    <button type="button" onClick={() => queue.add(files)}>
      add
    </button>
  );
}

describe("UploadTray", () => {
  it("shows the reason, when uploads resume, and offers to try again", async () => {
    vi.spyOn(api, "usage").mockResolvedValue(usage());
    const retryAt = new Date(Date.now() + 5 * 3_600_000).toISOString();
    const deps: QueueDeps = {
      api: {
        checkUpload: vi.fn(async () => {
          throw new ApiError(429, "Daily upload limit reached: 1.0 GB of 1.0 GB used in the last 24 hours.", "daily_cap_exceeded", {
            retry_at: retryAt,
          });
        }),
        putPart: vi.fn(),
        completeUpload: vi.fn(),
      },
      hash: async () => sha,
      delay: async () => {},
      fileConcurrency: 1,
    };
    const user = userEvent.setup();
    render(
      <UploadProvider deps={deps}>
        <Adder files={[file("a.jpg"), file("b.jpg")]} />
        <UploadTray />
      </UploadProvider>,
    );
    await user.click(screen.getByRole("button", { name: "add" }));
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(/Daily upload limit reached\. Uploads can resume at/)).toBeInTheDocument();
    expect(within(alert).getByText(/1\.0 GB of 1\.0 GB used/)).toBeInTheDocument();
    expect(screen.getByText(/1 failed · 1 not started/)).toBeInTheDocument();
    expect(screen.getAllByText("Not started")).toHaveLength(1);
    expect(screen.getByText("Uploads stopped")).toBeInTheDocument();
    // The failed file has no plain Retry (it is stopped by the limit); Try again covers it.
    expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument();
    expect(await screen.findByLabelText("Your limits")).toHaveTextContent("Storage 4.0 GB of 5.0 GB");
    await user.click(within(alert).getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(deps.api.checkUpload).toHaveBeenCalledTimes(2));
  });

  it("warns at 80 % of the storage while a batch runs", async () => {
    vi.spyOn(api, "usage").mockResolvedValue(usage());
    const deps: QueueDeps = {
      api: { checkUpload: vi.fn(async () => upload()), putPart: vi.fn(async () => ({ etag: '"e"' })), completeUpload: vi.fn(async () => ({ asset_id: "a" })) },
      hash: async () => sha,
      delay: async () => {},
    };
    const user = userEvent.setup();
    render(
      <UploadProvider deps={deps}>
        <Adder files={[file("a.jpg")]} />
        <UploadTray />
      </UploadProvider>,
    );
    await user.click(screen.getByRole("button", { name: "add" }));
    expect(await screen.findByText("You have used 80% of your storage.")).toBeInTheDocument();
  });
});

describe("Storage page limits", () => {
  const renderPage = () =>
    render(
      <MemoryRouter>
        <Storage />
      </MemoryRouter>,
    );

  it("shows each limit with its meter and the 80 % warning", async () => {
    vi.spyOn(api, "usage").mockResolvedValue(usage());
    renderPage();
    const storage = await screen.findByRole("progressbar", { name: "Storage used" });
    expect(storage).toHaveAttribute("aria-valuenow", "80");
    expect(screen.getByText(/4\.0 GB of 5\.0 GB/)).toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Uploaded in the last 24 hours" })).toHaveAttribute("aria-valuenow", "50");
    expect(screen.getByText("200 MB")).toBeInTheDocument(); // the largest file
    expect(screen.getByText("You have used 80% of your storage.")).toBeInTheDocument();
    expect(screen.getByText(/frees up at/)).toBeInTheDocument();
  });

  it("is blunter at 95 % and when full", async () => {
    vi.spyOn(api, "usage").mockResolvedValue(usage({ stored_bytes: 4.8 * GB }));
    const { unmount } = renderPage();
    expect(await screen.findByText(/almost full: 96% used/)).toBeInTheDocument();
    unmount();
    vi.spyOn(api, "usage").mockResolvedValue(usage({ stored_bytes: 5 * GB, daily: { used_bytes: GB, oldest_rolls_out_at: null } }));
    renderPage();
    expect(await screen.findByText(/Your storage is full/)).toBeInTheDocument();
    expect(screen.getByText(/reached your daily upload limit/)).toBeInTheDocument();
  });

  it("says no limit, and shows no warning, for an account without limits", async () => {
    vi.spyOn(api, "usage").mockResolvedValue(usage({ limits: { quota_bytes: null, daily_upload_bytes: null, max_file_bytes: null } }));
    renderPage();
    await screen.findByRole("heading", { name: "Your limits" });
    expect(screen.getAllByText("No limit")).toHaveLength(3);
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
    expect(screen.queryByText(/You have used/)).not.toBeInTheDocument();
  });

  it("copes with a server that sends no limits", async () => {
    vi.spyOn(api, "usage").mockResolvedValue(usage({ limits: undefined, daily: undefined }));
    renderPage();
    await screen.findByRole("heading", { name: "Your limits" });
    expect(screen.getAllByText("No limit")).toHaveLength(3);
  });
});

describe("Accounts page limit editing", () => {
  const account = (n: number, over: Record<string, unknown> = {}) => ({
    id: `u${n}`,
    email: `user${n}@example.com`,
    role: "member" as const,
    created_at: "2026-01-01T00:00:00Z",
    last_active: null,
    originals_bytes: 4 * GB,
    trash_bytes: GB,
    derived_bytes: 0,
    photo_count: 1,
    raw_count: 0,
    file_count: 1,
    trash_count: 0,
    derived_files: 1,
    uploads_24h_count: 0,
    uploads_24h_bytes: 0,
    uploads_30d_count: 0,
    uploads_30d_bytes: 0,
    albums: 0,
    shares: 0,
    api_keys: 0,
    quota_bytes: null,
    daily_upload_bytes: null,
    max_file_bytes: null,
    ...over,
  });
  const data = (over: Partial<AdminAccounts> = {}): AdminAccounts => ({
    items: [account(1, { quota_bytes: 5 * GB }), account(2)],
    totals: {
      accounts: 2, originals_bytes: 0, trash_bytes: 0, derived_bytes: 0, derived_complete: true, photo_count: 0, raw_count: 0,
      uploads_24h_count: 0, uploads_24h_bytes: 0, uploads_30d_count: 0, uploads_30d_bytes: 0,
    },
    reconcile: null,
    bucket_check: null,
    default_limits: { quota_bytes: null, daily_upload_bytes: null, max_file_bytes: null },
    ...over,
  });

  it("shows each account's limits and how much of the storage it uses", async () => {
    vi.spyOn(api, "adminAccounts").mockResolvedValue(data());
    render(<Accounts />);
    await screen.findByText("user1@example.com");
    expect(screen.getByText("5.0 GB storage · 100% of storage used")).toBeInTheDocument();
    expect(screen.getAllByText("No limits")).toHaveLength(2); // the defaults panel and the second account
  });

  it("edits an account's limits in GB (empty is none) and reloads", async () => {
    const list = vi.spyOn(api, "adminAccounts").mockResolvedValue(data());
    const save = vi.spyOn(api, "setAccountLimits").mockResolvedValue({ quota_bytes: 10 * GB, daily_upload_bytes: null, max_file_bytes: GB / 2 });
    const user = userEvent.setup();
    render(<Accounts />);
    await user.click(await screen.findByRole("button", { name: "Edit limits of user2@example.com" }));
    await user.type(screen.getByLabelText("Storage (GB)"), "10");
    await user.type(screen.getByLabelText("Largest file (GB)"), "0.5");
    await user.click(screen.getByRole("button", { name: "Save limits" }));
    expect(save).toHaveBeenCalledWith("u2", { quota_bytes: 10 * GB, daily_upload_bytes: null, max_file_bytes: GB / 2 });
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
  });

  it("starts from the current values, can clear them, and refuses nonsense without asking the server", async () => {
    vi.spyOn(api, "adminAccounts").mockResolvedValue(data());
    const save = vi.spyOn(api, "setAccountLimits").mockResolvedValue({ quota_bytes: null, daily_upload_bytes: null, max_file_bytes: null });
    const user = userEvent.setup();
    render(<Accounts />);
    await user.click(await screen.findByRole("button", { name: "Edit limits of user1@example.com" }));
    const quota = screen.getByLabelText("Storage (GB)");
    expect(quota).toHaveValue("5");
    await user.clear(quota);
    await user.type(screen.getByLabelText("Uploads per day (GB)"), "lots");
    await user.click(screen.getByRole("button", { name: "Save limits" }));
    expect(await screen.findByText(/greater than zero/)).toBeInTheDocument();
    expect(save).not.toHaveBeenCalled();
    await user.clear(screen.getByLabelText("Uploads per day (GB)"));
    await user.click(screen.getByRole("button", { name: "Save limits" }));
    expect(save).toHaveBeenCalledWith("u1", { quota_bytes: null, daily_upload_bytes: null, max_file_bytes: null });
  });

  it("shows a server refusal in the form", async () => {
    vi.spyOn(api, "adminAccounts").mockResolvedValue(data());
    vi.spyOn(api, "setAccountLimits").mockRejectedValue(new ApiError(403, "administrators only"));
    const user = userEvent.setup();
    render(<Accounts />);
    await user.click(await screen.findByRole("button", { name: "Edit limits of user2@example.com" }));
    await user.type(screen.getByLabelText("Storage (GB)"), "1");
    await user.click(screen.getByRole("button", { name: "Save limits" }));
    expect(await screen.findByText("administrators only")).toBeInTheDocument();
  });

  it("edits the defaults for new accounts", async () => {
    vi.spyOn(api, "adminAccounts").mockResolvedValue(
      data({ default_limits: { quota_bytes: 20 * GB, daily_upload_bytes: null, max_file_bytes: null } }),
    );
    const save = vi.spyOn(api, "setDefaultLimits").mockResolvedValue({ quota_bytes: 20 * GB, daily_upload_bytes: GB, max_file_bytes: null });
    const user = userEvent.setup();
    render(<Accounts />);
    const box = await screen.findByRole("region", { name: "Limits for new accounts" });
    expect(within(box).getByText(/20 GB storage/)).toBeInTheDocument();
    await user.click(within(box).getByRole("button", { name: "Edit defaults" }));
    await user.type(within(box).getByLabelText("Uploads per day (GB)"), "1");
    await user.click(within(box).getByRole("button", { name: "Save defaults" }));
    expect(save).toHaveBeenCalledWith({ quota_bytes: 20 * GB, daily_upload_bytes: GB, max_file_bytes: null });
  });
});
