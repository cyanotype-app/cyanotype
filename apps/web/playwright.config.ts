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
    launchOptions: {
      ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}),
      // Headless Chromium has no GPU: the map's WebGL runs on the software rasteriser.
      args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"],
    },
  },
  webServer: {
    command: `pnpm exec vite build && pnpm exec vite preview --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: true,
    timeout: 120_000,
  },
});
