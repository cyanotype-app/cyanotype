// Approving a command-line login: /cli/authorize (the loopback flow) and /activate (the device flow) at phone and desktop
// widths, with a long device name and a long account name: nothing may scroll sideways. Also, under the gateway's real
// Content-Security-Policy (the preview serves it), pressing Allow really navigates to the command line's listener on
// 127.0.0.1 (a navigation is not a form submission, so `form-action 'self'` does not stop it). Set SHOT_DIR to save screenshots.
import { expect, test, type Page } from "@playwright/test";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { json, makeCheck, RUNS, setupApp, shotPrefix, viewportOf, type Handler } from "./kit";

const DEVICE = "a-really-long-hostname-for-a-machine-in-the-spare-room-with-no-spaces.example.internal";
const CHALLENGE = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

const authorizeUrl = (port: number, device = DEVICE) =>
  `/cli/authorize?redirect_uri=${encodeURIComponent(`http://127.0.0.1:${port}/callback`)}&state=st-1&code_challenge=${CHALLENGE}&device=${encodeURIComponent(device)}&scope=import`;

function handlers(calls: string[], port: () => number): Handler[] {
  return [
    (route, url, method) => {
      const p = url.pathname;
      if (!p.startsWith("/auth/cli/") || method !== "POST") return false;
      const body = JSON.parse(route.request().postData() ?? "{}") as Record<string, string>;
      calls.push(`${p} ${JSON.stringify(body)}`);
      if (p === "/auth/cli/authorize/check") void route.fulfill(json({ device: DEVICE.slice(0, 48), scope: "import" }));
      else if (p === "/auth/cli/authorize/approve")
        void route.fulfill(json({ redirect: `http://127.0.0.1:${port()}/callback?code=abc123&state=${body.state}` }));
      else if (p === "/auth/cli/authorize/deny")
        void route.fulfill(json({ redirect: `http://127.0.0.1:${port()}/callback?error=access_denied&state=${body.state}` }));
      else if (p === "/auth/cli/device/lookup") {
        if (body.user_code === "WDJB-MJHT") void route.fulfill(json({ device: DEVICE.slice(0, 48), scope: "read", expires_in: 540 }));
        else void route.fulfill(json({ error: "That code was not recognised, or it has expired. Run `cyanotype login` again to get a new one.", code: "unknown_code" }, 404));
      } else if (p === "/auth/cli/device/approve") void route.fulfill(json({ approved: true, device: DEVICE.slice(0, 48) }));
      else if (p === "/auth/cli/device/deny") void route.fulfill(json({ approved: false, device: DEVICE.slice(0, 48) }));
      else void route.fulfill(json({ error: "not mocked" }, 404));
      return true;
    },
  ];
}

/** Answers /auth/cli/* (the library mock only covers /assets and /auth/me). */
async function mockCli(page: Page, calls: string[], port: () => number) {
  const handle = handlers(calls, port)[0]!;
  await page.route(
    (u) => u.pathname.startsWith("/auth/cli/"),
    (route) => void handle(route, new URL(route.request().url()), route.request().method()),
  );
}

for (const { width, scheme } of RUNS) {
  test.describe(`cli login ${width}px ${scheme}`, () => {
    test.use({ colorScheme: scheme, viewport: viewportOf(width), hasTouch: width < 600 });

    test("the authorize and activate pages fit", async ({ page }) => {
      test.setTimeout(60_000);
      const problems: string[] = [];
      const check = makeCheck(page, shotPrefix("cli-login", width, scheme), problems);
      const calls: string[] = [];
      await setupApp(page);
      await mockCli(page, calls, () => 1);

      // The loopback page: the question, the device, the access, and the two buttons.
      await page.goto(authorizeUrl(53682));
      await expect(page.getByText(/Allow cyanotype CLI on a-really-long-hostname/)).toBeVisible();
      await expect(page.getByRole("button", { name: "Allow" })).toBeVisible();
      await expect(page.getByRole("button", { name: "Deny" })).toBeVisible();
      await expect(page.getByText("Import only")).toBeVisible();
      await check("authorize");

      // A link that is not a login link says so.
      await page.goto("/cli/authorize");
      await expect(page.getByText(/not a login link/)).toBeVisible();
      await check("authorize-invalid");

      // The device page: the code box, a wrong code, the question for a right one, the result.
      await page.goto("/activate");
      const code = page.getByLabel("Code");
      await code.fill("bcdfghjk");
      await expect(code).toHaveValue("BCDF-GHJK");
      await check("activate-enter");
      await page.getByRole("button", { name: "Continue" }).click();
      await expect(page.getByText(/was not recognised/)).toBeVisible();
      await check("activate-unknown");
      await code.fill("wdjbmjht");
      await page.getByRole("button", { name: "Continue" }).click();
      await expect(page.getByText(/Allow cyanotype CLI on a-really-long-hostname/)).toBeVisible();
      await expect(page.getByText("Read only")).toBeVisible();
      await check("activate-ask");
      await page.getByRole("button", { name: "Allow" }).click();
      await expect(page.getByText(/is signing in; you can close this tab/)).toBeVisible();
      await check("activate-done");

      // The code travels in request bodies, never in an address.
      expect(page.url()).not.toContain("WDJB");
      expect(calls.some((c) => c.startsWith("/auth/cli/device/approve") && c.includes("WDJB-MJHT"))).toBe(true);
      expect(problems, problems.join("\n")).toEqual([]);
    });
  });
}

test.describe("cli login redirect", () => {
  test.use({ viewport: viewportOf(1280) });

  /** A stand-in for the command line's listener: remembers what the browser requested. */
  async function listener() {
    const seen: string[] = [];
    const server = createServer((req, res) => {
      seen.push(req.url ?? "");
      res.setHeader("content-type", "text/html");
      res.end("<h1>You can close this tab</h1>");
    });
    await new Promise<void>((ok) => server.listen(0, "127.0.0.1", ok));
    return { seen, port: (server.address() as AddressInfo).port, close: () => server.close() };
  }

  test("Allow navigates to the loopback listener with the code, under the gateway's CSP", async ({ page }) => {
    const l = await listener();
    const calls: string[] = [];
    await setupApp(page);
    await mockCli(page, calls, () => l.port);
    await page.goto(authorizeUrl(l.port, "test-laptop"));
    await page.getByRole("button", { name: "Allow" }).click();
    await expect.poll(() => l.seen.length, { timeout: 10_000 }).toBeGreaterThan(0);
    expect(l.seen[0]).toBe("/callback?code=abc123&state=st-1");
    await expect(page.getByText("You can close this tab")).toBeVisible();
    expect(calls.map((c) => c.split(" ")[0])).toEqual(["/auth/cli/authorize/check", "/auth/cli/authorize/approve"]);
    l.close();
  });

  test("Deny navigates to the listener with an error and no code", async ({ page }) => {
    const l = await listener();
    await setupApp(page);
    await mockCli(page, [], () => l.port);
    await page.goto(authorizeUrl(l.port, "test-laptop"));
    await page.getByRole("button", { name: "Deny" }).click();
    await expect.poll(() => l.seen.length, { timeout: 10_000 }).toBeGreaterThan(0);
    expect(l.seen[0]).toBe("/callback?error=access_denied&state=st-1");
    l.close();
  });

  test("a visitor who is signed out signs in and comes back to the same request", async ({ page }) => {
    const l = await listener();
    let signedIn = false;
    await setupApp(page);
    await mockCli(page, [], () => l.port);
    await page.route("**/auth/me", (r) =>
      r.fulfill(signedIn ? json({ id: "u1", email: "alpha@example.com", role: "member" }) : json({ error: "unauthorized" }, 401)),
    );
    await page.route("**/auth/registration", (r) => r.fulfill(json({ mode: "invite", first_account: false, open: false, invite_required: true })));
    await page.route("**/auth/login", (r) => {
      signedIn = true;
      return r.fulfill(json({ id: "u1" }));
    });
    await page.goto(authorizeUrl(l.port, "test-laptop"));
    await page.getByLabel("Email").fill("alpha@example.com");
    await page.getByLabel("Password").fill("correct horse battery");
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByText(/Allow cyanotype CLI on a-really-long-hostname/)).toBeVisible();
    expect(page.url()).toContain("state=st-1");
    l.close();
  });
});
