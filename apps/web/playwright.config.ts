import { defineConfig } from "@playwright/test";

// Layout check for phone widths (`pnpm test:mobile`). Not part of CI: it needs a Chromium that Playwright can
// launch (`pnpm exec playwright install chromium`, or point CHROMIUM_PATH at an existing binary).
const PORT = process.env.E2E_PORT ?? "4173";
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  reporter: "list",
  use: {
    baseURL: `http://localhost:${PORT}`,
    launchOptions: { ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) },
  },
  projects: [
    // The layout check includes the map page, whose WebGL needs the software rasteriser in headless Chromium (no GPU).
    // Only this spec gets those flags: they change scrolling and paint timing, which made the timeline spec's
    // scrubber-jump assertions flaky (a year tap landing ~33,000 px off).
    {
      name: "layout",
      testMatch: /(mobile-overflow|security-layout)\.spec\.ts/,
      use: {
        launchOptions: {
          ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}),
          args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"],
        },
      },
    },
    { name: "timeline", testIgnore: /(mobile-overflow|security-layout)\.spec\.ts/ },
  ],
  webServer: {
    command: `pnpm exec vite build && pnpm exec vite preview --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: true,
    timeout: 120_000,
  },
});
