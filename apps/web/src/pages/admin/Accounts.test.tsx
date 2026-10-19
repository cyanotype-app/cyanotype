import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, type AdminAccount, type AdminAccounts, type AdminTask, type BucketReport } from "../../api";
import { Accounts } from "./Accounts";

const GB = 1024 ** 3;
const account = (n: number, over: Partial<AdminAccount> = {}): AdminAccount => ({
  id: `u${n}`,
  email: `user${n}@example.com`,
  role: n === 1 ? "admin" : "member",
  created_at: "2026-01-01T00:00:00Z",
  last_active: null,
  originals_bytes: GB,
  trash_bytes: 0,
  derived_bytes: 0,
  photo_count: 10,
  raw_count: 0,
  file_count: 10,
  trash_count: 0,
  derived_files: 10,
  uploads_24h_count: 0,
  uploads_24h_bytes: 0,
  uploads_30d_count: 0,
  uploads_30d_bytes: 0,
  albums: 0,
  shares: 0,
  api_keys: 0,
  ...over,
});
const report = (over: Partial<BucketReport> = {}): BucketReport => ({
  finished_at: "2026-06-01T00:00:00Z",
  versioned: true,
  objects: 100,
  current_bytes: 6 * GB,
  originals_bytes: 5 * GB,
  derived_bytes: GB,
  manifests_bytes: 0,
  edits_bytes: 0,
  other_bytes: 0,
  noncurrent_versions: 3,
  noncurrent_bytes: 2 * GB,
  delete_markers: 0,
  db_originals_bytes: 5 * GB,
  db_derived_bytes: GB,
  db_derived_files: 100,
  db_files: 100,
  unknown_account_bytes: 0,
  mismatched: [],
  ...over,
});
const task = (state: AdminTask["state"], result: BucketReport | null = null, error: string | null = null): AdminTask<BucketReport> => ({
  id: 1,
  kind: "bucket_check",
  state,
  result,
  error,
  created_at: "2026-06-01T00:00:00Z",
  started_at: null,
  finished_at: state === "done" || state === "failed" ? "2026-06-01T00:05:00Z" : null,
});
const data = (over: Partial<AdminAccounts> = {}): AdminAccounts => ({
  items: [account(1), account(2, { trash_bytes: 5 * 1024 ** 2, trash_count: 3, albums: 4, shares: 2, api_keys: 1, raw_count: 7 })],
  totals: {
    accounts: 2,
    originals_bytes: 2 * GB,
    trash_bytes: 5 * 1024 ** 2,
    derived_bytes: 100 * 1024 ** 2,
    derived_complete: true,
    photo_count: 20,
    raw_count: 7,
    uploads_24h_count: 5,
    uploads_24h_bytes: GB,
    uploads_30d_count: 50,
    uploads_30d_bytes: 10 * GB,
  },
  reconcile: { at: "2026-06-01T00:00:00Z", users_checked: 2, drifted_users: 0, drift_bytes: 0 },
  bucket_check: null,
  ...over,
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe("Accounts page", () => {
  it("lists every account with counts and sizes, and the totals", async () => {
    vi.spyOn(api, "adminAccounts").mockResolvedValue(data());
    render(<Accounts />);
    expect(await screen.findByText("user2@example.com")).toBeInTheDocument();
    const totals = screen.getByLabelText("Server totals");
    expect(within(totals).getByText("Accounts")).toBeInTheDocument();
    expect(within(totals).getByText("10 GB")).toBeInTheDocument(); // uploaded in 30 days
    const cards = screen.getAllByRole("listitem").filter((li) => li.classList.contains("account"));
    expect(cards).toHaveLength(2);
    expect(within(cards[0]!).getByText("admin")).toBeInTheDocument();
    expect(within(cards[1]!).getByText("member")).toBeInTheDocument();
    expect(within(cards[1]!).getByText("7 RAW")).toBeInTheDocument();
    expect(within(cards[1]!).getByText("5.0 MB")).toBeInTheDocument();
    expect(screen.getAllByText(/last active never/)).toHaveLength(2);
    expect(screen.getByText(/all matched/)).toBeInTheDocument();
  });

  it("says when the nightly check found drift", async () => {
    vi.spyOn(api, "adminAccounts").mockResolvedValue(
      data({ reconcile: { at: "2026-06-01T00:00:00Z", users_checked: 2, drifted_users: 1, drift_bytes: 10 } }),
    );
    render(<Accounts />);
    expect(await screen.findByText(/1 account had drifted and was corrected/)).toBeInTheDocument();
  });

  it("starts a bucket check, polls while it runs, and shows the result and differences", async () => {
    vi.spyOn(api, "adminAccounts").mockResolvedValue(data());
    const start = vi.spyOn(api, "startBucketCheck").mockResolvedValue({ task: task("queued"), already_running: false });
    const poll = vi
      .spyOn(api, "bucketCheck")
      .mockResolvedValueOnce({ task: task("running") })
      .mockResolvedValue({
        task: task("done", report({ mismatched: [{ user_id: "u2", email: "user2@example.com", bucket_originals_bytes: 0, db_originals_bytes: 2048 }] })),
      });
    const user = userEvent.setup();
    render(<Accounts />);
    await user.click(await screen.findByRole("button", { name: "Check bucket size" }));
    expect(start).toHaveBeenCalled();
    expect(await screen.findByText(/Waiting for the worker/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Checking…" })).toBeDisabled();
    // Polls every 3 s.
    await waitFor(() => expect(poll).toHaveBeenCalledTimes(1), { timeout: 5000 });
    expect(await screen.findByText(/Listing the bucket/)).toBeInTheDocument();
    expect(await screen.findByText(/Finished/, undefined, { timeout: 5000 })).toBeInTheDocument();
    expect(screen.getByText("Differences found.")).toBeInTheDocument();
    expect(screen.getByText("user2@example.com: bucket 0 B, database 2.0 KB")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Check bucket size" })).toBeEnabled();
  }, 15000);

  it("shows a finished check that matches, and a failed one", async () => {
    vi.spyOn(api, "adminAccounts").mockResolvedValue(data({ bucket_check: task("done", report()) }));
    const { unmount } = render(<Accounts />);
    expect(await screen.findByText(/match the database for every account/)).toBeInTheDocument();
    expect(within(screen.getByRole("region", { name: "Bucket check" })).getByText("Old versions kept")).toBeInTheDocument();
    unmount();
    vi.spyOn(api, "adminAccounts").mockResolvedValue(data({ bucket_check: task("failed", null, "storage is down") }));
    render(<Accounts />);
    expect(await screen.findByText("Failed: storage is down")).toBeInTheDocument();
  });

  it("shows an error for a member's 403 and no data", async () => {
    vi.spyOn(api, "adminAccounts").mockRejectedValue(new Error("administrators only"));
    render(<Accounts />);
    expect(await screen.findByText("administrators only")).toBeInTheDocument();
    expect(screen.queryByLabelText("Server totals")).not.toBeInTheDocument();
  });
});
