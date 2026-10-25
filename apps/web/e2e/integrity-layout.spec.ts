// Admin > Health > Storage integrity at phone and desktop widths, light and dark: the card with all three kinds of finding (long
// keys and addresses), the typed confirmation for deleting an object, and a check in progress. Set SHOT_DIR to save screenshots.
import { expect, test } from "@playwright/test";
import { json, makeCheck, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

const LONG_KEY = "originals/0f8fad5b-d9cb-469f-a165-70867728950e/ab/abababababababababababababababababababababababababababababababab.jpeg2000";
const LONG_EMAIL = "someone.with.a.really.long.address.for.layout.checks@example-photo-library.organisation.example";
const base = {
  state: "open",
  owner_id: "u2",
  owner_email: LONG_EMAIL,
  first_seen_at: "2026-06-01T00:00:00Z",
  last_seen_at: "2026-06-01T00:00:00Z",
  resolution: null,
  resolved_at: null,
};
const findings = [
  { ...base, id: 3, kind: "missing_original", asset_id: "a1", key: LONG_KEY, detail: { previous_versions: 2, size_bytes: 123456 } },
  { ...base, id: 2, kind: "untracked_object", asset_id: null, key: LONG_KEY.replace("ab/", "cd/"), detail: { reason: "no_row", size_bytes: 9876543, owner_known: true } },
  {
    ...base,
    id: 1,
    kind: "checksum_mismatch",
    asset_id: "a2",
    key: LONG_KEY.replace("ab/", "ef/"),
    detail: { reason: "hash", expected_sha256: "a".repeat(64), actual_sha256: "b".repeat(64), previous_versions: 1 },
  },
];
const run = {
  id: 7,
  trigger: "schedule",
  state: "done",
  started_at: "2026-06-01T03:00:00Z",
  finished_at: "2026-06-01T03:20:00Z",
  error: null,
  objects_listed: 12345,
  assets_checked: 12000,
  assets_verified: 400,
  bytes_hashed: 2147483648,
  missing_originals: 1,
  untracked_objects: 1,
  checksum_mismatches: 1,
  derived_requeued: 3,
  findings_resolved: 0,
};
const summary = (task: unknown = null) => ({
  last_run: run,
  open: { missing_original: 1, untracked_object: 1, checksum_mismatch: 1 },
  acknowledged: 2,
  coverage: { assets: 12000, verified: 7000, oldest_verified_at: "2026-05-01T00:00:00Z" },
  task,
});
const health = {
  version: { assets: "sha-abc1234", workers: [] },
  queue: [],
  stalled_seconds: 0,
  failed_jobs: [],
  workers: [],
  backup: { prefix: "backups/postgres", count: 0, last_at: null, last_size_bytes: null },
  storage: { database_bytes: 1, stored_bytes: 1, derived_bytes: 1, bucket_check: null },
};

for (const width of [320, 375, 414, 1280]) {
  for (const scheme of ["light", "dark"] as const) {
    test.describe(`integrity ${width}px ${scheme}`, () => {
      test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

      test("the card, the delete confirmation and a running check without overflow", async ({ page }) => {
        test.setTimeout(90_000);
        const problems: string[] = [];
        const check = makeCheck(page, shotPrefix("integrity", width, scheme), problems);
        let task: unknown = null;
        const handlers: Handler[] = [
          (route, url) => {
            const p = url.pathname;
            if (p === "/assets/admin/health") {
              void route.fulfill(json(health));
              return true;
            }
            if (p === "/assets/admin/integrity") {
              void route.fulfill(json(summary(task)));
              return true;
            }
            if (p === "/assets/admin/integrity/findings") {
              void route.fulfill(json({ items: findings, next_before: 1 }));
              return true;
            }
            if (p === "/assets/admin/integrity/run") {
              task = { state: "running", created_at: "2026-06-02T00:00:00Z", started_at: "2026-06-02T00:00:01Z" };
              {
              void route.fulfill(json({ task: { state: "queued" }, already_running: false }, 202));
              return true;
            }
            }
            return false;
          },
        ];
        await setupApp(page, { handlers, role: "admin" });

        await page.goto("/admin/health");
        await page.getByRole("heading", { name: "Storage integrity" }).waitFor();
        await page.getByRole("button", { name: "Restore as photo" }).waitFor();
        await check("card");
        await page.getByRole("button", { name: "Delete object" }).click();
        await page.getByLabel("Type delete to confirm").fill("del");
        await expect(page.getByRole("button", { name: "Delete for good" })).toBeDisabled();
        await page.getByRole("button", { name: "Delete for good" }).scrollIntoViewIfNeeded();
        await check("delete-confirm");
        await page.getByRole("button", { name: "Cancel" }).click();
        await page.getByRole("button", { name: "Run check now" }).click();
        await page.getByRole("button", { name: "Checking…" }).waitFor();
        await check("running");

        expect(problems, problems.join("\n")).toEqual([]);
      });
    });
  }
}
