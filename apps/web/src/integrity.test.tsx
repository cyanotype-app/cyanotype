// Admin > Health > Storage integrity: the card, its actions (the destructive one asks for a typed word) and the wording helpers.
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, type IntegrityFinding, type IntegritySummary } from "./api";
import { actionsFor, coverageLine, findingTitle, runLine } from "./integrityText";
import { IntegrityCard } from "./pages/admin/IntegrityCard";

const finding = (over: Partial<IntegrityFinding> = {}): IntegrityFinding => ({
  id: 1,
  kind: "missing_original",
  state: "open",
  owner_id: "u1",
  owner_email: "alpha@example.com",
  asset_id: "a1",
  key: "originals/u1/ab/abcdef.jpg",
  detail: { previous_versions: 1 },
  resolution: null,
  first_seen_at: "2026-06-01T00:00:00Z",
  last_seen_at: "2026-06-01T00:00:00Z",
  resolved_at: null,
  ...over,
});

const summary = (over: Partial<IntegritySummary> = {}): IntegritySummary => ({
  last_run: {
    id: 1,
    trigger: "schedule",
    state: "done",
    started_at: new Date(Date.now() - 3_600_000).toISOString(),
    finished_at: new Date(Date.now() - 3_500_000).toISOString(),
    error: null,
    objects_listed: 120,
    assets_checked: 118,
    assets_verified: 20,
    bytes_hashed: 2 * 1024 ** 3,
    missing_originals: 1,
    untracked_objects: 1,
    checksum_mismatches: 1,
    derived_requeued: 2,
    findings_resolved: 0,
  },
  open: { missing_original: 1, untracked_object: 1, checksum_mismatch: 1 },
  acknowledged: 0,
  coverage: { assets: 118, verified: 60, oldest_verified_at: "2026-05-01T00:00:00Z" },
  task: null,
  ...over,
});

afterEach(() => vi.restoreAllMocks());

describe("IntegrityCard", () => {
  it("shows the last run, the counts per kind and the findings", async () => {
    vi.spyOn(api, "integrity").mockResolvedValue(summary());
    vi.spyOn(api, "integrityFindings").mockResolvedValue({ items: [finding()], next_before: null });
    render(<IntegrityCard />);
    expect(await screen.findByText(/120 objects listed, 118 photos compared, 2\.0 GB re-read/)).toBeInTheDocument();
    expect(screen.getByText(/2 thumbnails queued to be made again/)).toBeInTheDocument();
    expect(screen.getByText(/Checksums read for 60 of 118 photos \(50%\)/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Missing originals (1)" })).toBeInTheDocument();
    expect(screen.getByText("The original is gone from the bucket.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Restore from a previous version" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Move to trash" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Mark lost" })).toBeInTheDocument();
  });

  it("queues a check and says it is running", async () => {
    const sum = vi.spyOn(api, "integrity").mockResolvedValueOnce(summary({ last_run: null })).mockResolvedValue(
      summary({ last_run: null, task: { state: "queued", created_at: "2026-06-01T00:00:00Z", started_at: null } }),
    );
    vi.spyOn(api, "integrityFindings").mockResolvedValue({ items: [], next_before: null });
    const run = vi.spyOn(api, "runIntegrityCheck").mockResolvedValue({ task: { state: "queued" }, already_running: false });
    const user = userEvent.setup();
    render(<IntegrityCard />);
    await user.click(await screen.findByRole("button", { name: "Run check now" }));
    expect(run).toHaveBeenCalled();
    expect(await screen.findByRole("button", { name: "Checking…" })).toBeDisabled();
    expect(sum).toHaveBeenCalledTimes(2);
  });

  it("filters by kind and asks for the next page", async () => {
    vi.spyOn(api, "integrity").mockResolvedValue(summary());
    const list = vi
      .spyOn(api, "integrityFindings")
      .mockResolvedValueOnce({ items: [finding({ id: 9 })], next_before: 9 })
      .mockResolvedValue({ items: [finding({ id: 8, key: "originals/u1/cd/second.jpg" })], next_before: null });
    const user = userEvent.setup();
    render(<IntegrityCard />);
    await user.click(await screen.findByRole("button", { name: "Show more" }));
    expect(list).toHaveBeenLastCalledWith({ kind: undefined, before: 9 });
    expect(await screen.findByText("originals/u1/cd/second.jpg")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Untracked objects (1)" }));
    await waitFor(() => expect(list).toHaveBeenLastCalledWith({ kind: "untracked_object", before: undefined }));
  });

  it("restores a version and refreshes", async () => {
    vi.spyOn(api, "integrity").mockResolvedValue(summary());
    vi.spyOn(api, "integrityFindings").mockResolvedValue({ items: [finding()], next_before: null });
    const act = vi.spyOn(api, "integrityAct").mockResolvedValue({ changed: true, finding: finding({ state: "resolved" }) });
    const user = userEvent.setup();
    render(<IntegrityCard />);
    await user.click(await screen.findByRole("button", { name: "Restore from a previous version" }));
    expect(act).toHaveBeenCalledWith(1, "restore_version", undefined);
    expect(await screen.findByText("Restored from a previous version.")).toBeInTheDocument();
  });

  it("shows the server's refusal", async () => {
    vi.spyOn(api, "integrity").mockResolvedValue(summary());
    vi.spyOn(api, "integrityFindings").mockResolvedValue({ items: [finding()], next_before: null });
    vi.spyOn(api, "integrityAct").mockRejectedValue(new Error("no earlier version of the object has the stored checksum"));
    const user = userEvent.setup();
    render(<IntegrityCard />);
    await user.click(await screen.findByRole("button", { name: "Restore from a previous version" }));
    expect(await screen.findByText(/no earlier version of the object/)).toBeInTheDocument();
  });

  it("deletes an object only after the word is typed", async () => {
    const orphan = finding({ id: 5, kind: "untracked_object", asset_id: null, detail: { reason: "no_row", size_bytes: 2048, owner_known: true } });
    vi.spyOn(api, "integrity").mockResolvedValue(summary());
    vi.spyOn(api, "integrityFindings").mockResolvedValue({ items: [orphan], next_before: null });
    const act = vi.spyOn(api, "integrityAct").mockResolvedValue({ changed: true, finding: finding({ state: "resolved" }) });
    const user = userEvent.setup();
    render(<IntegrityCard />);
    await user.click(await screen.findByRole("button", { name: "Delete object" }));
    expect(act).not.toHaveBeenCalled();
    const confirm = screen.getByRole("button", { name: "Delete for good" });
    expect(confirm).toBeDisabled();
    await user.type(screen.getByLabelText("Type delete to confirm"), "dele");
    expect(confirm).toBeDisabled();
    await user.type(screen.getByLabelText("Type delete to confirm"), "te");
    await user.click(confirm);
    expect(act).toHaveBeenCalledWith(5, "delete_object", "delete");
    expect(await screen.findByText("Object deleted.")).toBeInTheDocument();
  });

  it("offers a restore as photo only for an object with an owner and no row", async () => {
    const f = (detail: IntegrityFinding["detail"]) => finding({ kind: "untracked_object", asset_id: null, detail });
    const labels = (x: IntegrityFinding) => actionsFor(x).map((a) => a.label);
    expect(labels(f({ reason: "no_row", owner_known: true }))).toContain("Restore as photo");
    expect(labels(f({ reason: "no_row", owner_known: false }))).not.toContain("Restore as photo");
    expect(labels(f({ reason: "duplicate_content" }))).not.toContain("Restore as photo");
    expect(labels(finding({ detail: { previous_versions: 0 } }))).not.toContain("Restore from a previous version");
    expect(labels(finding({ detail: { previous_versions: null } }))).toContain("Restore from a previous version");
  });
});

describe("integrity wording", () => {
  it("describes each kind of finding", () => {
    const m = finding({ kind: "checksum_mismatch", detail: { reason: "size", size_bytes: 1024, actual_bytes: 512 } });
    expect(findingTitle(m)).toBe("The file is 512 B, the library says 1.0 KB.");
    expect(findingTitle(finding({ kind: "checksum_mismatch", detail: { reason: "hash" } }))).toMatch(/no longer match/);
    expect(findingTitle(finding({ kind: "untracked_object", detail: { reason: "unrecognised_key" } }))).toMatch(/not an original's/);
  });

  it("reads a run and the coverage", () => {
    expect(runLine(null)).toMatch(/has not run yet/);
    expect(runLine(summary({ last_run: { ...summary().last_run!, state: "failed", error: "list originals/ failed" } }).last_run)).toMatch(/stopped .*list originals\/ failed/);
    expect(coverageLine({ assets: 0, verified: 0, oldest_verified_at: null })).toBe("No photos yet.");
  });
});
