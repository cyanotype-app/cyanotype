// The pages outside the app's frame (sign in, register, reset password, a public share link in every state) and the
// utility pages Recently deleted and Hidden, at phone and desktop widths, light and dark. Nothing may stick out sideways. Set
// SHOT_DIR to save the screenshots.
import { expect, test } from "@playwright/test";
import { json, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

const LONG_TITLE = "A long album title " + "with_an_unbroken_word_that_goes_on_".repeat(3);
const LONG_EMAIL = "an.extraordinarily.long.address.for.a.guest@a-very-long-domain-name-for-photos.example.org";
let unlocked = false;
let hiddenUnlocked = false;

const items = Array.from({ length: 14 }, (_, i) => ({
  id: `p${i + 1}`,
  width: 400,
  height: 300,
  sort_time: new Date(Date.UTC(2026, 0, 30 - i)).toISOString(),
  thumbhash: null,
  thumb_url: `/fixture-thumbs/${i + 1}.svg`,
  preview_url: `/fixture-thumbs/${i + 1}.svg`,
  downloads: [
    { id: `p${i + 1}`, label: "JPEG", size_bytes: 3_000_000 },
    { id: `r${i + 1}`, label: "RAW", size_bytes: 25_000_000 },
  ],
  download_pending: false,
}));

const trashed = Array.from({ length: 9 }, (_, i) => ({
  id: `t${i + 1}`,
  original_name: `IMG_${1000 + i}.jpg`,
  content_type: "image/jpeg",
  size_bytes: 1000,
  width: 400,
  height: 300,
  sort_time: new Date(Date.UTC(2026, 0, 30 - i)).toISOString(),
  thumbhash: null,
  thumb_url: `/fixture-thumbs/${i + 1}.svg`,
  deleted_at: new Date(Date.now() - 2 * 86_400_000).toISOString(),
  purge_at: new Date(Date.now() + 28 * 86_400_000).toISOString(),
}));

const handlers: Handler[] = [
  (route, url, method) => {
    const p = url.pathname;
    const ok = (body: unknown, status = 200) => {
      void route.fulfill(json(body, status));
      return true;
    };
    if (p === "/assets/trash" && method === "GET") return ok({ items: trashed, next_cursor: null, total: trashed.length, retention_days: 30 });
    const m = /^\/assets\/public\/s\/([^/]+)(\/.*)?$/.exec(p);
    if (!m) return false;
    const [, token, rest = ""] = m;
    if (token === "gone") return ok({ error: "not found" }, 404);
    const locked = token === "locked" && !unlocked;
    if (rest === "")
      return ok(
        locked
          ? { needs_password: true, unlocked: false }
          : { needs_password: token === "locked", unlocked: true, kind: "album", title: LONG_TITLE, item_count: items.length, allow_download: true, include_originals: true, expires_at: new Date(Date.now() + 3 * 86_400_000).toISOString() },
      );
    if (rest === "/unlock") {
      const right = (route.request().postDataJSON() as { password: string }).password === "right";
      if (right) unlocked = true;
      void route.fulfill(right ? { status: 204 } : json({ error: "unauthorized" }, 401));
      return true;
    }
    if (rest === "/items") return ok({ items, next_cursor: null });
    return false;
  },
];

for (const { width, scheme } of RUNS) {
  test.describe(`signed-out ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("sign in, register, reset, share links, recently deleted and hidden", async ({ page }) => {
      test.setTimeout(120_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("signedout", width, scheme), problems);
      unlocked = false;
      hiddenUnlocked = false;
      await setupApp(page, { count: 12, handlers });
      await page.route("**/auth/hidden", (r) => r.fulfill(json({ unlocked: hiddenUnlocked, expires_at: null })));
      await page.route("**/auth/registration", (r) => r.fulfill(json({ mode: "invite", first_account: false, open: false, invite_required: true })));
      await page.route("**/auth/reset-password/check*", (r) => r.fulfill(json({ valid: true, email: LONG_EMAIL })));
      await page.route("**/auth/reset-password/check", (r) => r.fulfill(json({ valid: true, email: LONG_EMAIL })));
      await page.route("**/fixture-thumbs/*.svg", (r) =>
        r.fulfill({ status: 200, contentType: "image/svg+xml", body: '<svg xmlns="http://www.w3.org/2000/svg" width="300" height="300"><rect width="300" height="300" fill="#2f7cb5"/></svg>' }),
      );

      // Signed out
      let signedOut = true;
      await page.route("**/auth/me", (r) =>
        signedOut ? r.fulfill({ status: 401, contentType: "application/json", body: '{"error":"no"}' }) : r.fallback(),
      );
      await page.goto("/login");
      await page.getByRole("button", { name: "Sign in" }).waitFor();
      await check("login");
      await page.goto("/register");
      await page.getByLabel("Invite link or code").waitFor();
      await check("register");
      await page.goto("/reset-password?token=" + "0123456789abcdef".repeat(4));
      await page.getByLabel("New password").waitFor();
      await check("reset");

      await page.goto("/s/open");
      await page.getByRole("button", { name: /Open photo 1 of/ }).waitFor();
      await check("share");
      await page.getByRole("button", { name: /Open photo 1 of/ }).click();
      await page.getByRole("dialog", { name: "Photo" }).getByRole("button", { name: "Download" }).click();
      await page.getByRole("link", { name: /Download RAW/ }).waitFor();
      await check("share-viewer");
      await page.keyboard.press("Escape");
      await page.goto("/s/locked");
      await page.getByRole("heading", { name: "This link is protected" }).waitFor();
      await check("share-password");
      await page.getByLabel("Password").fill("wrong");
      await page.getByRole("button", { name: "View photos" }).click();
      await page.getByText("That password is not right.").waitFor();
      await check("share-password-error");
      await page.goto("/s/gone");
      await page.getByRole("heading", { name: "This link is not available" }).waitFor();
      await check("share-gone");
      signedOut = false;

      // Signed in: Recently deleted (and its confirmation) and Hidden
      await page.goto("/trash");
      await page.getByRole("button", { name: /Select IMG_1000/ }).waitFor();
      await check("trash");
      await page.getByRole("button", { name: /Select IMG_1001/ }).click();
      await page.getByRole("button", { name: "Delete forever" }).click();
      await page.getByRole("alertdialog").waitFor();
      await page.waitForTimeout(350);
      await check("trash-confirm");
      await page.getByRole("button", { name: "Cancel" }).click();
      await page.goto("/hidden");
      await page.getByLabel("Password").waitFor();
      await check("hidden");

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
