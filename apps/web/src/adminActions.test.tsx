// The admin portal's account actions, health card, audit log, the reset-password page and the sessions page.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, type AdminAccount, type AuditPage as AuditPageData, type Health } from "./api";
import { AccountActions } from "./components/AccountActions";
import { AuditPage } from "./pages/admin/AuditPage";
import { HealthPage } from "./pages/admin/HealthPage";
import { ResetPassword } from "./pages/ResetPassword";
import { Sessions } from "./pages/Sessions";

const account = (over: Partial<AdminAccount> = {}): AdminAccount => ({
  id: "u2",
  email: "sam@example.com",
  role: "member",
  created_at: "2026-01-01T00:00:00Z",
  last_active: null,
  originals_bytes: 0,
  trash_bytes: 0,
  derived_bytes: 0,
  photo_count: 0,
  raw_count: 0,
  file_count: 0,
  trash_count: 0,
  derived_files: 0,
  uploads_24h_count: 0,
  uploads_24h_bytes: 0,
  uploads_30d_count: 0,
  uploads_30d_bytes: 0,
  albums: 0,
  shares: 0,
  api_keys: 0,
  ...over,
});

afterEach(() => vi.restoreAllMocks());

describe("AccountActions", () => {
  const setup = (a: AdminAccount, meId = "u1") => {
    const onChanged = vi.fn(async () => {});
    render(<AccountActions account={a} meId={meId} onChanged={onChanged} />);
    return onChanged;
  };
  const open = async (user: ReturnType<typeof userEvent.setup>) => user.click(screen.getByText("Manage"));

  it("lists the account's API keys with their last use and revokes one", async () => {
    const key = {
      id: "k1",
      name: "cyanotype CLI on test-laptop",
      prefix: "cyk_abcdef",
      scope: "import" as const,
      created_at: "2026-01-01T00:00:00Z",
      last_used_at: null,
      expires_at: null,
    };
    const list = vi
      .spyOn(api, "accountApiKeys")
      .mockResolvedValueOnce({ items: [key] })
      .mockResolvedValue({ items: [] });
    const revoke = vi.spyOn(api, "revokeAccountApiKey").mockResolvedValue(undefined);
    vi.spyOn(window, "confirm").mockReturnValue(true);
    const user = userEvent.setup();
    const changed = setup(account());
    await open(user);
    await user.click(screen.getByRole("button", { name: "API keys" }));
    expect(await screen.findByText("cyanotype CLI on test-laptop")).toBeInTheDocument();
    expect(screen.getByText(/last used never/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Revoke cyanotype CLI on test-laptop" }));
    await waitFor(() => expect(revoke).toHaveBeenCalledWith("u2", "k1"));
    expect(await screen.findByText("No API keys.")).toBeInTheDocument();
    expect(list).toHaveBeenCalledTimes(2);
    expect(changed).toHaveBeenCalled();
  });

  it("disables and reloads", async () => {
    const disable = vi.spyOn(api, "disableAccount").mockResolvedValue({ disabled: true, changed: true });
    const user = userEvent.setup();
    const changed = setup(account());
    await open(user);
    await user.click(screen.getByRole("button", { name: "Disable account" }));
    expect(disable).toHaveBeenCalledWith("u2");
    expect(await screen.findByText(/Disabled\. It is signed out/)).toBeInTheDocument();
    expect(changed).toHaveBeenCalled();
  });

  it("enables a disabled account", async () => {
    const enable = vi.spyOn(api, "enableAccount").mockResolvedValue({ disabled: false, changed: true });
    const user = userEvent.setup();
    setup(account({ disabled_at: "2026-02-01T00:00:00Z" }));
    await open(user);
    await user.click(screen.getByRole("button", { name: "Enable account" }));
    expect(enable).toHaveBeenCalledWith("u2");
    expect(await screen.findByText(/Enabled\. It can sign in again/)).toBeInTheDocument();
  });

  it("promotes and demotes", async () => {
    const role = vi.spyOn(api, "setAccountRole").mockResolvedValue({ role: "admin", changed: true });
    const user = userEvent.setup();
    setup(account());
    await open(user);
    await user.click(screen.getByRole("button", { name: "Make admin" }));
    expect(role).toHaveBeenCalledWith("u2", "admin");
    expect(await screen.findByText("Now an administrator.")).toBeInTheDocument();
  });

  it("shows the server's refusal, such as the last administrator", async () => {
    vi.spyOn(api, "setAccountRole").mockRejectedValue(new ApiError(409, "this is the last administrator: demoting it would leave nobody able to manage the server"));
    const user = userEvent.setup();
    setup(account({ id: "u1", role: "admin" }));
    await open(user);
    await user.click(screen.getByRole("button", { name: "Remove admin" }));
    expect(await screen.findByText(/last administrator/)).toBeInTheDocument();
  });

  it("offers neither disable nor delete for the admin's own account", async () => {
    const user = userEvent.setup();
    setup(account({ id: "u1", role: "admin" }), "u1");
    await open(user);
    expect(screen.queryByRole("button", { name: "Disable account" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Delete account/ })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Reset password" })).toBeInTheDocument();
  });

  it("makes a one-time reset link, shows it once and copies it", async () => {
    vi.spyOn(api, "accountResetLink").mockResolvedValue({
      token: "TOK",
      path: "/reset-password?token=TOK",
      email: "sam@example.com",
      expires_at: "2099-01-01T00:00:00Z",
    });
    const user = userEvent.setup();
    const write = vi.spyOn(navigator.clipboard, "writeText");
    setup(account());
    await open(user);
    await user.click(screen.getByRole("button", { name: "Reset password" }));
    await user.click(screen.getByRole("button", { name: "Make a reset link" }));
    const link = `${window.location.origin}/reset-password?token=TOK`;
    expect(await screen.findByText(link)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Copy link" }));
    expect(write).toHaveBeenCalledWith(link);
    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
  });

  it("lists the sessions and signs the account out everywhere", async () => {
    const list = vi
      .spyOn(api, "accountSessions")
      .mockResolvedValueOnce({ items: [{ id: "a", created_at: "2026-01-01T00:00:00Z", expires_at: "2026-02-01T00:00:00Z" }, { id: "b", created_at: "2026-01-02T00:00:00Z", expires_at: "2026-02-02T00:00:00Z" }] })
      .mockResolvedValue({ items: [] });
    const revoke = vi.spyOn(api, "revokeAccountSessions").mockResolvedValue({ revoked: 2 });
    const user = userEvent.setup();
    setup(account());
    await open(user);
    await user.click(screen.getByRole("button", { name: "Sessions" }));
    expect(await screen.findAllByText(/Signed in/)).toHaveLength(2);
    await user.click(screen.getByRole("button", { name: "Sign out everywhere" }));
    expect(revoke).toHaveBeenCalledWith("u2");
    expect(await screen.findByText("No active sessions.")).toBeInTheDocument();
    expect(list).toHaveBeenCalledTimes(2);
  });

  it("deletes only after the email is typed (two steps) and then reloads", async () => {
    const del = vi.spyOn(api, "deleteAccount").mockResolvedValue({ deleting: true, already_requested: false });
    const user = userEvent.setup();
    const changed = setup(account());
    await open(user);
    // Step one: the button only opens the confirmation.
    await user.click(screen.getByRole("button", { name: "Delete account…" }));
    expect(del).not.toHaveBeenCalled();
    const go = screen.getByRole("button", { name: "Delete account permanently" });
    expect(go).toBeDisabled();
    // Step two: the email, which must match (case aside, whitespace aside).
    const box = screen.getByLabelText("Type the email address to confirm");
    await user.type(box, "sam@example.co");
    expect(go).toBeDisabled();
    await user.type(box, "m ");
    expect(go).toBeEnabled();
    await user.clear(box);
    await user.type(box, "SAM@example.com");
    await user.click(go);
    expect(del).toHaveBeenCalledWith("u2", "SAM@example.com");
    await waitFor(() => expect(changed).toHaveBeenCalled());
  });

  it("shows a deletion in progress and a failed one with a way to retry", async () => {
    const { unmount } = render(<AccountActions account={account({ deletion: "running" })} onChanged={async () => {}} />);
    expect(screen.getByText(/Being deleted/)).toBeInTheDocument();
    expect(screen.queryByText("Manage")).not.toBeInTheDocument();
    unmount();
    const user = userEvent.setup();
    render(<AccountActions account={account({ deletion: "failed", deletion_error: "jobs still running" })} onChanged={async () => {}} />);
    await user.click(screen.getByText("Manage"));
    expect(screen.getByText(/failed: jobs still running/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Delete account…" }));
    expect(screen.getByRole("button", { name: "Delete account permanently" })).toBeInTheDocument();
  });
});

describe("HealthPage", () => {
  // The integrity card has its own tests (integrity.test.tsx); here it just has nothing to show.
  beforeEach(() => {
    vi.spyOn(api, "integrity").mockResolvedValue({
      last_run: null,
      open: { missing_original: 0, untracked_object: 0, checksum_mismatch: 0 },
      acknowledged: 0,
      coverage: { assets: 0, verified: 0, oldest_verified_at: null },
      task: null,
    });
    vi.spyOn(api, "integrityFindings").mockResolvedValue({ items: [], next_before: null });
  });
  const health = (over: Partial<Health> = {}): Health => ({
    version: { assets: "sha-abc1234", workers: [{ host: "w1", version: "sha-abc1234", reporting: true }] },
    queue: [
      { kind: "thumbnail", queued: 40, running: 2, failed: 3, oldest_queued_age_secs: 125 },
      { kind: "metadata", queued: 0, running: 0, failed: 1, oldest_queued_age_secs: 0 },
    ],
    stalled_seconds: 0,
    failed_jobs: [{ id: 1, kind: "thumbnail", attempts: 5, last_error: "decode refused: too big", finished_at: "2026-06-01T00:00:00Z" }],
    workers: [{ host: "w1", reporting: true, at: "2026-06-01T00:00:00Z", version: "sha-abc1234", rss_bytes: 300 * 1024 ** 2, budget_used_bytes: 0, budget_total_bytes: 640 * 1024 ** 2 }],
    backup: { prefix: "backups/postgres", count: 14, last_at: new Date(Date.now() - 6 * 3_600_000).toISOString(), last_size_bytes: 3 * 1024 ** 3 },
    storage: { database_bytes: 50 * 1024 ** 2, stored_bytes: 800 * 1024 ** 3, derived_bytes: 30 * 1024 ** 3, bucket_check: null },
    ...over,
  });

  it("shows the queue by kind, the failures, the worker, the backup, the sizes and the version", async () => {
    vi.spyOn(api, "health").mockResolvedValue(health());
    render(<HealthPage />);
    expect(await screen.findByText(/40 jobs waiting, 2 running, 4 failed/)).toBeInTheDocument();
    expect(screen.getByText(/oldest waiting 2 min/)).toBeInTheDocument();
    expect(screen.getByText("decode refused: too big")).toBeInTheDocument();
    expect(screen.getByText("300 MB resident · decode budget 0 B of 640 MB")).toBeInTheDocument();
    expect(screen.getByText(/Last backup 6 hours ago, 3\.0 GB\. 14 kept\./)).toBeInTheDocument();
    expect(screen.getByText("50 MB")).toBeInTheDocument();
    expect(screen.getByText(/Run the bucket check/)).toBeInTheDocument();
    expect(screen.getAllByText("sha-abc1234").length).toBeGreaterThanOrEqual(2);
  });

  it("retries every failed job, or one kind, and says how many", async () => {
    vi.spyOn(api, "health").mockResolvedValue(health());
    const retry = vi.spyOn(api, "retryAllFailedJobs").mockResolvedValueOnce({ requeued: 4 }).mockResolvedValueOnce({ requeued: 3 });
    const user = userEvent.setup();
    render(<HealthPage />);
    await user.click(await screen.findByRole("button", { name: "Retry all failed jobs" }));
    expect(retry).toHaveBeenLastCalledWith(undefined);
    expect(await screen.findByText("4 jobs put back in the queue.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Retry thumbnail (3)" }));
    expect(retry).toHaveBeenLastCalledWith("thumbnail");
    expect(await screen.findByText("3 jobs put back in the queue.")).toBeInTheDocument();
  });

  it("warns about a stalled queue, an old backup, a silent worker and a bucket it could not list", async () => {
    vi.spyOn(api, "health").mockResolvedValue(
      health({
        stalled_seconds: 900,
        workers: [{ host: "w1", reporting: false, at: new Date(Date.now() - 3_600_000).toISOString(), version: null, rss_bytes: null, budget_used_bytes: null, budget_total_bytes: null }],
        backup: { prefix: "backups/postgres", error: "could not list the bucket's backups" },
      }),
    );
    render(<HealthPage />);
    expect(await screen.findByText(/No job has finished for 15 min/)).toBeInTheDocument();
    expect(screen.getByText(/not reporting/)).toBeInTheDocument();
    expect(screen.getByText("could not list the bucket's backups")).toBeInTheDocument();
  });

  it("is calm when everything is idle, and shows a finished bucket check", async () => {
    vi.spyOn(api, "health").mockResolvedValue(
      health({
        queue: [],
        failed_jobs: [],
        workers: [],
        storage: { database_bytes: 1024, stored_bytes: 0, derived_bytes: 0, bucket_check: { at: new Date().toISOString(), current_bytes: 5 * 1024 ** 3, noncurrent_bytes: 1024 ** 3, objects: 10 } },
      }),
    );
    render(<HealthPage />);
    expect(await screen.findByText("Nothing waiting.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Retry/ })).not.toBeInTheDocument();
    expect(screen.getByText(/No worker has reported yet/)).toBeInTheDocument();
    expect(screen.getByText(/5\.0 GB \+ 1\.0 GB old versions/)).toBeInTheDocument();
  });

  it("shows the error for a refused request", async () => {
    vi.spyOn(api, "health").mockRejectedValue(new ApiError(403, "administrators only"));
    render(<HealthPage />);
    expect(await screen.findByText("administrators only")).toBeInTheDocument();
  });
});

describe("AuditPage", () => {
  const page = (n: number, from = 1, next: number | null = null): AuditPageData => ({
    items: Array.from({ length: n }, (_, i) => ({
      id: 100 - from - i,
      at: "2026-06-01T10:00:00Z",
      actor_id: "u1",
      actor_email: "admin@example.com",
      action: i % 2 ? "invite.create" : "account.disable",
      target_type: "user",
      target_id: "u2",
      detail: { email: "sam@example.com" },
    })),
    next_before: next,
    actions: ["account.disable", "invite.create"],
  });

  it("lists entries as sentences with who and about whom", async () => {
    vi.spyOn(api, "auditLog").mockResolvedValue(page(2));
    render(<AuditPage />);
    const list = await screen.findByRole("list");
    expect(within(list).getByText("Disabled an account")).toBeInTheDocument();
    expect(within(list).getByText("Created an invite")).toBeInTheDocument();
    expect(within(list).getAllByText("by admin@example.com")).toHaveLength(2);
    expect(within(list).getAllByText("sam@example.com")).toHaveLength(2);
  });

  it("filters by action, person and dates, and clears", async () => {
    const log = vi.spyOn(api, "auditLog").mockResolvedValue(page(1));
    const user = userEvent.setup();
    render(<AuditPage />);
    await screen.findByRole("list");
    await user.selectOptions(screen.getByLabelText("Action"), "account.disable");
    await user.type(screen.getByLabelText("Done by"), "admin@");
    await user.type(screen.getByLabelText("About"), "sam");
    await user.type(screen.getByLabelText("From"), "2026-06-01");
    await user.click(screen.getByRole("button", { name: "Filter" }));
    expect(log).toHaveBeenLastCalledWith({ action: "account.disable", actor: "admin@", target: "sam", from: "2026-06-01", to: "", before: undefined, limit: 50 });
    await user.click(screen.getByRole("button", { name: "Clear" }));
    expect(log).toHaveBeenLastCalledWith({ action: "", actor: "", target: "", from: "", to: "", before: undefined, limit: 50 });
    expect(screen.getByLabelText("Done by")).toHaveValue("");
  });

  it("loads older entries with the cursor and keeps the filter", async () => {
    const log = vi.spyOn(api, "auditLog").mockResolvedValueOnce(page(2, 1, 98)).mockResolvedValueOnce(page(1, 3, null));
    const user = userEvent.setup();
    render(<AuditPage />);
    await user.click(await screen.findByRole("button", { name: "Load more" }));
    expect(log).toHaveBeenLastCalledWith(expect.objectContaining({ before: 98, limit: 50 }));
    await waitFor(() => expect(screen.getAllByText(/by admin@example.com/)).toHaveLength(3));
    expect(screen.queryByRole("button", { name: "Load more" })).not.toBeInTheDocument();
  });

  it("says when nothing matches and shows an error", async () => {
    vi.spyOn(api, "auditLog").mockResolvedValueOnce({ items: [], next_before: null, actions: [] });
    const { unmount } = render(<AuditPage />);
    expect(await screen.findByText("Nothing matches.")).toBeInTheDocument();
    unmount();
    vi.spyOn(api, "auditLog").mockRejectedValue(new ApiError(403, "administrators only"));
    render(<AuditPage />);
    expect(await screen.findByText("administrators only")).toBeInTheDocument();
  });
});

describe("ResetPassword page", () => {
  const renderAt = (url: string) =>
    render(
      <MemoryRouter initialEntries={[url]}>
        <Routes>
          <Route path="/reset-password" element={<ResetPassword />} />
          <Route path="/login" element={<p>login page</p>} />
        </Routes>
      </MemoryRouter>,
    );

  it("checks the link, sets the password once and points to sign-in", async () => {
    const check = vi.spyOn(api, "resetCheck").mockResolvedValue({ valid: true, email: "sam@example.com" });
    const reset = vi.spyOn(api, "resetPassword").mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderAt("/reset-password?token=TOK");
    expect(await screen.findByText("For sam@example.com")).toBeInTheDocument();
    expect(check).toHaveBeenCalledWith("TOK");
    await user.type(screen.getByLabelText("New password"), "a brand new passphrase");
    await user.type(screen.getByLabelText("Again"), "a brand new passphrase");
    await user.click(screen.getByRole("button", { name: "Set the password" }));
    expect(reset).toHaveBeenCalledWith("TOK", "a brand new passphrase");
    expect(await screen.findByText(/signed out everywhere/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Sign in with the new password" })).toHaveAttribute("href", "/login");
  });

  it("refuses mismatched or short passwords without calling the server", async () => {
    vi.spyOn(api, "resetCheck").mockResolvedValue({ valid: true, email: "sam@example.com" });
    const reset = vi.spyOn(api, "resetPassword");
    const user = userEvent.setup();
    renderAt("/reset-password?token=TOK");
    await screen.findByText("For sam@example.com");
    await user.type(screen.getByLabelText("New password"), "a brand new passphrase");
    await user.type(screen.getByLabelText("Again"), "something else entirely");
    await user.click(screen.getByRole("button", { name: "Set the password" }));
    expect(await screen.findByText("The two passwords are not the same.")).toBeInTheDocument();
    expect(reset).not.toHaveBeenCalled();
  });

  it.each([
    ["used", /already used/],
    ["expired", /has expired/],
    ["unknown", /not valid/],
    ["disabled", /account is disabled/],
  ] as const)("says so up front when the link is %s", async (reason, text) => {
    vi.spyOn(api, "resetCheck").mockResolvedValue({ valid: false, reason });
    renderAt("/reset-password?token=TOK");
    expect(await screen.findByText(text)).toBeInTheDocument();
    expect(screen.queryByLabelText("New password")).not.toBeInTheDocument();
  });

  it("explains a page opened without a token, and shows a server refusal at submit", async () => {
    const { unmount } = renderAt("/reset-password");
    expect(screen.getByText(/not valid/)).toBeInTheDocument();
    unmount();
    vi.spyOn(api, "resetCheck").mockResolvedValue({ valid: true, email: "sam@example.com" });
    vi.spyOn(api, "resetPassword").mockRejectedValue(new ApiError(403, "this reset link is not valid, was already used or has expired"));
    const user = userEvent.setup();
    renderAt("/reset-password?token=TOK");
    await screen.findByText("For sam@example.com");
    await user.type(screen.getByLabelText("New password"), "a brand new passphrase");
    await user.type(screen.getByLabelText("Again"), "a brand new passphrase");
    await user.click(screen.getByRole("button", { name: "Set the password" }));
    expect(await screen.findByText(/was already used or has expired/)).toBeInTheDocument();
  });
});

describe("Sessions page", () => {
  const items = [
    { id: "a", created_at: "2026-01-01T00:00:00Z", expires_at: "2026-02-01T00:00:00Z", current: true },
    { id: "b", created_at: "2026-01-02T00:00:00Z", expires_at: "2026-02-02T00:00:00Z", current: false },
  ];

  it("marks this browser, ends one session and the others", async () => {
    const list = vi.spyOn(api, "mySessions").mockResolvedValue({ items });
    const one = vi.spyOn(api, "revokeSession").mockResolvedValue(undefined);
    const others = vi.spyOn(api, "revokeOtherSessions").mockResolvedValue({ revoked: 1 });
    const user = userEvent.setup();
    render(<Sessions />);
    expect(await screen.findByText("This browser")).toBeInTheDocument();
    const row = screen.getByText("Another browser").closest("li")!;
    await user.click(within(row).getByRole("button", { name: "Sign out" }));
    expect(one).toHaveBeenCalledWith("b");
    await user.click(await screen.findByRole("button", { name: "Sign out the others" }));
    expect(others).toHaveBeenCalled();
    expect(await screen.findByText("Signed out everywhere else.")).toBeInTheDocument();
    expect(list.mock.calls.length).toBeGreaterThanOrEqual(3);
  });

  it("signs out everywhere", async () => {
    vi.spyOn(api, "mySessions").mockResolvedValue({ items });
    const all = vi.spyOn(api, "revokeAllSessions").mockResolvedValue({ revoked: 2 });
    const user = userEvent.setup();
    render(<Sessions />);
    await user.click(await screen.findByRole("button", { name: "Sign out everywhere" }));
    expect(all).toHaveBeenCalled();
  });

  it("shows no 'sign out the others' when this is the only session", async () => {
    vi.spyOn(api, "mySessions").mockResolvedValue({ items: [items[0]!] });
    render(<Sessions />);
    await screen.findByText("This browser");
    expect(screen.queryByRole("button", { name: "Sign out the others" })).not.toBeInTheDocument();
  });
});
