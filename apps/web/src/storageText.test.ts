import { describe, expect, it } from "vitest";
import type { AdminTask, BucketReport } from "./api";
import { ago, barShares, bucketTaskLine, countLabel, derivedLabel, mismatchLine, reconcileLine } from "./storageText";

const NOW = Date.parse("2026-06-10T12:00:00Z");

describe("countLabel", () => {
  it("pluralises and groups thousands", () => {
    expect(countLabel(1, "photo")).toBe("1 photo");
    expect(countLabel(0, "photo")).toBe("0 photos");
    expect(countLabel(12345, "file")).toBe("12,345 files");
    expect(countLabel(2, "library", "libraries")).toBe("2 libraries");
  });
});

describe("barShares", () => {
  it("splits the bar by bytes and keeps a small part visible", () => {
    expect(barShares(0, 0)).toEqual({ photos: 0, trash: 0 });
    expect(barShares(300, 100)).toEqual({ photos: 75, trash: 25 });
    expect(barShares(100, 0)).toEqual({ photos: 100, trash: 0 });
    expect(barShares(0, 100)).toEqual({ photos: 0, trash: 100 });
    // 1 byte of trash in a terabyte still shows a sliver.
    expect(barShares(1e12, 1)).toEqual({ photos: 99, trash: 1 });
    for (const [a, b] of [[7, 3], [1, 1e9], [5, 5]]) {
      const s = barShares(a!, b!);
      expect(s.photos + s.trash).toBe(100);
    }
  });
});

describe("ago", () => {
  it("is coarse and honest", () => {
    expect(ago(null, NOW)).toBe("never");
    expect(ago("2026-06-10T11:59:40Z", NOW)).toBe("just now");
    expect(ago("2026-06-10T11:55:00Z", NOW)).toBe("5 minutes ago");
    expect(ago("2026-06-10T11:00:00Z", NOW)).toBe("1 hour ago");
    expect(ago("2026-06-07T12:00:00Z", NOW)).toBe("3 days ago");
    expect(ago("2026-01-02T12:00:00Z", NOW)).toMatch(/2026|1\/2|2\/1/);
  });
});

describe("derivedLabel", () => {
  it("is a lower bound until everything is measured", () => {
    expect(derivedLabel(2048, true)).toBe("2.0 KB");
    expect(derivedLabel(2048, false)).toBe("at least 2.0 KB");
  });
});

describe("reconcileLine", () => {
  it("reports the nightly check", () => {
    expect(reconcileLine(null, NOW)).toMatch(/has not run yet/);
    expect(reconcileLine({ at: "2026-06-10T09:00:00Z", users_checked: 3, drifted_users: 0 }, NOW)).toBe(
      "Numbers checked against the files 3 hours ago: all matched.",
    );
    expect(reconcileLine({ at: "2026-06-10T09:00:00Z", users_checked: 3, drifted_users: 1 }, NOW)).toBe(
      "Numbers checked against the files 3 hours ago: 1 account had drifted and was corrected.",
    );
    expect(reconcileLine({ at: "2026-06-10T09:00:00Z", users_checked: 3, drifted_users: 2 }, NOW)).toMatch(/2 accounts had drifted and were corrected/);
  });
});

describe("bucketTaskLine", () => {
  const task = (over: Partial<AdminTask<BucketReport>>): AdminTask<BucketReport> => ({
    id: 1,
    kind: "bucket_check",
    state: "queued",
    result: null,
    error: null,
    created_at: "2026-06-10T11:00:00Z",
    started_at: null,
    finished_at: null,
    ...over,
  });
  it("says where the check is", () => {
    expect(bucketTaskLine(null, NOW)).toBe("Not run yet.");
    expect(bucketTaskLine(task({}), NOW)).toMatch(/Waiting/);
    expect(bucketTaskLine(task({ state: "running" }), NOW)).toMatch(/Listing/);
    expect(bucketTaskLine(task({ state: "failed", error: "storage is down" }), NOW)).toBe("Failed: storage is down");
    expect(bucketTaskLine(task({ state: "done", finished_at: "2026-06-10T11:30:00Z" }), NOW)).toBe("Finished 30 minutes ago.");
  });
});

describe("mismatchLine", () => {
  it("names the account and both sizes", () => {
    expect(mismatchLine({ user_id: "u", email: "a@b.co", bucket_originals_bytes: 0, db_originals_bytes: 2048 })).toBe(
      "a@b.co: bucket 0 B, database 2.0 KB",
    );
  });
});
