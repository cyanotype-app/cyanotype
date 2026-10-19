import { describe, expect, it } from "vitest";
import type { Health } from "./api";
import { actionLabel, detailLines } from "./auditText";
import { backupStatus, duration, failedKinds, memoryLine, queueTotals } from "./healthText";

const NOW = Date.parse("2026-06-10T12:00:00Z");
const GB = 1024 ** 3;

describe("actionLabel and detailLines", () => {
  it("has a sentence for each known action and the name for an unknown one", () => {
    expect(actionLabel("account.disable")).toBe("Disabled an account");
    expect(actionLabel("invite.use")).toBe("Registered with an invite");
    expect(actionLabel("something.new")).toBe("something.new");
  });
  it("lists who it was about, role changes, limits before and after, and counts", () => {
    expect(detailLines({ detail: { email: "sam@example.com", from: "member", to: "admin" } })).toEqual(["sam@example.com", "member → admin"]);
    expect(
      detailLines({
        detail: {
          email: "sam@example.com",
          before: { quota_bytes: null, daily_upload_bytes: null, max_file_bytes: null },
          after: { quota_bytes: 5 * GB, daily_upload_bytes: null, max_file_bytes: null },
        },
      }),
    ).toEqual(["sam@example.com", "was: No limits", "now: 5.0 GB storage"]);
    expect(detailLines({ detail: { kind: "thumbnail", requeued: 3, note: null } })).toEqual(["kind: thumbnail", "requeued: 3"]);
    expect(detailLines({ detail: { sessions_ended: 0 } })).toEqual(["sessions ended: 0"]);
    expect(detailLines({ detail: {} })).toEqual([]);
  });
});

describe("duration", () => {
  it("is coarse", () => {
    expect(duration(0)).toBe("0 s");
    expect(duration(45)).toBe("45 s");
    expect(duration(180)).toBe("3 min");
    expect(duration(2 * 3600 + 5 * 60)).toBe("2 h 5 min");
    expect(duration(3 * 3600)).toBe("3 h");
    expect(duration(3 * 86400 + 5)).toBe("3 d");
  });
});

describe("backupStatus", () => {
  const b = (over: Partial<Health["backup"]> = {}): Health["backup"] => ({
    prefix: "backups/postgres",
    count: 14,
    last_at: "2026-06-10T03:17:00Z",
    last_size_bytes: 3 * GB,
    ...over,
  });
  it("is fine for last night's dump", () => {
    expect(backupStatus(b(), NOW)).toEqual({ level: "ok", text: "Last backup 8 hours ago, 3.0 GB. 14 kept." });
  });
  it("warns after a missed night", () => {
    expect(backupStatus(b({ last_at: "2026-06-08T03:17:00Z" }), NOW).level).toBe("warn");
  });
  it("is bad when there is none or the bucket could not be listed", () => {
    expect(backupStatus(b({ last_at: null, count: 0 }), NOW)).toEqual({ level: "bad", text: "No backup found under backups/postgres/." });
    expect(backupStatus({ prefix: "p", error: "could not list" }, NOW)).toEqual({ level: "bad", text: "could not list" });
  });
});

describe("worker and queue helpers", () => {
  it("words a worker's memory", () => {
    const w = { host: "w", reporting: true, at: "", version: null, rss_bytes: 300 * 1024 ** 2, budget_used_bytes: 0, budget_total_bytes: 640 * 1024 ** 2 };
    expect(memoryLine(w)).toBe("300 MB resident · decode budget 0 B of 640 MB");
    expect(memoryLine({ ...w, rss_bytes: null, budget_total_bytes: null })).toBe("no numbers yet");
  });
  it("totals the queue and names the kinds with failures", () => {
    const q = [
      { kind: "thumbnail", queued: 5, running: 1, failed: 2, oldest_queued_age_secs: 10 },
      { kind: "metadata", queued: 1, running: 0, failed: 0, oldest_queued_age_secs: 0 },
    ];
    expect(queueTotals(q)).toEqual({ queued: 6, running: 1, failed: 2 });
    expect(failedKinds(q)).toEqual([{ kind: "thumbnail", failed: 2 }]);
  });
});
