import { defineConfig } from "@playwright/test";

// Layout check for phone widths (`pnpm test:mobile`). Not part of CI: it needs a Chromium that Playwright can
// launch (`pnpm exec playwright install chromium`, or point CHROMIUM_PATH at an existing binary).
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  reporter: "list",
  use: {
    baseURL: "http://localhost:4173",
    launchOptions: process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {},
  },
  webServer: {
    command: "pnpm exec vite build && pnpm exec vite preview --port 4173 --strictPort",
    url: "http://localhost:4173",
    reuseExistingServer: true,
    timeout: 120_000,
  },
});
