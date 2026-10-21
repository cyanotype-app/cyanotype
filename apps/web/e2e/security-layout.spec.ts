// Layout of the two-factor pages (sign-in code step, Settings > Security in every state, the recovery-code display, the
// admin page) and of the API key form with scopes, at 320-1280 px and in dark mode: nothing may scroll sideways. Set
// SHOT_DIR to also save a screenshot of each state.
import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

const SHOT_DIR = process.env.SHOT_DIR;
const RUNS: { width: number; scheme: "light" | "dark" }[] = [
  ...[320, 375, 414, 1024, 1280].map((width) => ({ width, scheme: "light" as const })),
  { width: 360, scheme: "dark" },
];
const LONG_EMAIL = "an.extraordinarily.long.address.for.a.guest@a-very-long-domain-name-for-photos.example.org";
const SECRET = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
const CODES = Array.from({ length: 10 }, (_, i) => `ab${i}d-efgh-2345-ijk${i}`);

type Status = { available: boolean; enabled: boolean; recovery_codes_remaining: number; required: boolean };
let status: Status = { available: true, enabled: false, recovery_codes_remaining: 0, required: false };
let enrolRequired = false;
let loginChallenge = true;
let keyConfigured = true;

async function mock(page: Page) {
  const json = (body: unknown, code = 200) => ({ status: code, contentType: "application/json", body: JSON.stringify(body) });
  await page.route(
    (u) => /^\/(auth|assets)(\/|$)/.test(u.pathname),
    async (route) => {
      const p = new URL(route.request().url()).pathname;
      const m = route.request().method();
      if (p === "/auth/me")
        return route.fulfill(
          json({ id: "u1", email: "someone.with.a.long.address@example-photo-library.org", role: "admin", two_factor: status.enabled, mfa_enrol_required: enrolRequired }),
        );
      if (p === "/auth/registration") return route.fulfill(json({ mode: "invite", first_account: false, open: false, invite_required: true }));
      if (p === "/auth/login") return route.fulfill(json(loginChallenge ? { mfa_required: true, challenge: "chal" } : { id: "u1" }));
      if (p === "/auth/2fa" && m === "GET") return route.fulfill(json(status));
      if (p === "/auth/2fa/enrol")
        return route.fulfill(
          json({ secret: SECRET, otpauth_uri: `otpauth://totp/Cyanotype:${LONG_EMAIL}?secret=${SECRET}&issuer=Cyanotype`, issuer: "Cyanotype", account: LONG_EMAIL }),
        );
      if (p === "/auth/2fa/confirm") return route.fulfill(json({ enabled: true, recovery_codes: CODES }));
      if (p === "/auth/admin/2fa")
        return route.fulfill(
          json({
            key_configured: keyConfigured,
            required: keyConfigured,
            enforced: keyConfigured,
            users: [
              { id: "u1", email: LONG_EMAIL, role: "admin", enabled: true, since: "2026-01-01T00:00:00Z", recovery_codes_remaining: 10 },
              { id: "u2", email: LONG_EMAIL.replace("guest", "member"), role: "member", enabled: true, since: "2026-01-02T00:00:00Z", recovery_codes_remaining: 2 },
              { id: "u3", email: "no.second.factor@example.org", role: "member", enabled: false, since: null, recovery_codes_remaining: 0 },
            ],
          }),
        );
      if (p === "/auth/api-keys" && m === "GET")
        return route.fulfill(
          json({
            items: [
              { id: "k1", name: "nightly import on a machine with a very long name", prefix: "cyk_abcdef", created_at: "2026-06-01T00:00:00Z", last_used_at: null, scope: "import", expires_at: "2027-06-01T00:00:00Z", expired: false },
              { id: "k2", name: "old", prefix: "cyk_ghijkl", created_at: "2025-06-01T00:00:00Z", last_used_at: "2026-01-01T00:00:00Z", scope: "full", expires_at: null, expired: false },
              { id: "k3", name: "expired", prefix: "cyk_mnopqr", created_at: "2025-06-01T00:00:00Z", last_used_at: null, scope: "read", expires_at: "2026-01-01T00:00:00Z", expired: true },
            ],
          }),
        );
      if (p.startsWith("/assets/")) return route.fulfill(json({ items: [], next_cursor: null, total: 0 }));
      return route.fulfill(json({}, 204));
    },
  );
}

function makeCheck(page: Page, prefix: string, problems: string[]) {
  return async (name: string) => {
    await page.waitForTimeout(150);
    const r = await page.evaluate(() => {
      const w = window.innerWidth;
      const bad: string[] = [];
      for (const el of Array.from(document.querySelectorAll<HTMLElement>("body *"))) {
        const b = el.getBoundingClientRect();
        if (b.width === 0) continue;
        if (b.right > w + 0.5 || b.left < -0.5) {
          bad.push(`${el.tagName.toLowerCase()}.${String(el.className).split(" ")[0]} "${(el.textContent ?? "").trim().slice(0, 24)}" [${Math.round(b.left)}..${Math.round(b.right)}]`);
        }
      }
      return { scroll: document.documentElement.scrollWidth, w, bad: bad.slice(0, 6) };
    });
    if (r.scroll > r.w || r.bad.length) problems.push(`${name}: scrollWidth ${r.scroll} > ${r.w}; offenders: ${r.bad.join("; ")}`);
    if (SHOT_DIR) await page.screenshot({ path: join(SHOT_DIR, `${prefix}-${name}.png`), fullPage: true });
  };
}

for (const { width, scheme } of RUNS) {
  test.describe(`${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: { width, height: width === 1280 ? 800 : 740 }, hasTouch: width < 600 });

    test("two-factor pages fit", async ({ page }) => {
      test.setTimeout(60_000);
      if (SHOT_DIR) mkdirSync(SHOT_DIR, { recursive: true });
      const problems: string[] = [];
      const check = makeCheck(page, `${width}${scheme === "dark" ? "-dark" : ""}-2fa`, problems);

      // Sign-in: the password step, then the code step, then the recovery-code variant.
      status = { available: true, enabled: false, recovery_codes_remaining: 0, required: false };
      enrolRequired = false;
      loginChallenge = true;
      keyConfigured = true;
      await mock(page);
      // Routes registered later win: the first /auth/me answers "not signed in", the rest fall through to the mock.
      await page.route("**/auth/me", (r) => r.fulfill({ status: 401, contentType: "application/json", body: '{"error":"unauthorized"}' }), { times: 1 });
      await page.goto("/login");
      await page.getByLabel("Email").fill(LONG_EMAIL);
      await page.getByLabel("Password").fill("correct horse battery");
      await page.getByRole("button", { name: "Sign in" }).click();
      await page.getByLabel(/Code from your authenticator app/).waitFor();
      await check("login-code");
      await page.getByRole("button", { name: /Use a recovery code/ }).click();
      await page.getByLabel("Recovery code").fill("abcd-efgh-2345-ijkl");
      await check("login-recovery");

      // Settings > Security: off, the setup, the codes shown once, on, and the forms behind the buttons.
      await page.goto("/settings/security");
      await page.getByRole("button", { name: /Set up two-factor/ }).click();
      await page.getByRole("img", { name: /QR code/ }).waitFor();
      await check("security-enrol");
      await page.getByLabel(/6-digit code/).fill("123456");
      status = { ...status, enabled: true, recovery_codes_remaining: 10 };
      await page.getByRole("button", { name: "Turn on" }).click();
      await page.getByText("They will not be shown", { exact: false }).or(page.getByText("will not be shown again")).first().waitFor();
      await check("security-recovery-codes");
      await page.getByLabel(/I have saved these codes/).check();
      await page.getByRole("button", { name: "Done", exact: true }).click();
      await page.getByText(/10 of 10 recovery codes left/).waitFor();
      await check("security-on");
      await page.getByRole("button", { name: "Turn off" }).click();
      await check("security-turn-off");
      await page.getByRole("button", { name: "Cancel" }).click();
      await page.getByRole("button", { name: "New recovery codes" }).click();
      await check("security-new-codes");

      status = { available: false, enabled: false, recovery_codes_remaining: 0, required: false };
      await page.goto("/settings/security");
      await page.getByText(/not available on this server yet/).waitFor();
      await check("security-unavailable");

      // An account that must enrol: sent to the page, with the reason.
      status = { available: true, enabled: false, recovery_codes_remaining: 0, required: true };
      enrolRequired = true;
      await page.goto("/library");
      await page.getByText(/requires two-factor authentication/).waitFor();
      await check("security-required");
      enrolRequired = false;

      // Admin.
      await page.goto("/admin/security");
      await page.getByText(/have it on/).waitFor();
      await check("admin-security");
      keyConfigured = false;
      await page.goto("/admin/security");
      await page.getByText(/switched off on this server/).waitFor();
      await check("admin-security-no-key");

      // API keys with scopes.
      await page.goto("/settings/api-keys");
      await page.getByText(/Import only · Expires/).waitFor();
      await check("api-keys-scopes");

      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}
