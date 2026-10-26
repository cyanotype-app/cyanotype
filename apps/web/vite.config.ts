/// <reference types="vitest/config" />
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// The API lives on the same origin as the app (the gateway), so the dev server proxies it.
// `/assets` is the API's route, so Vite's own hashed files go to `/static` instead.
const gateway = process.env.GATEWAY_URL ?? "http://localhost:3000";

// E2E_CSP=1 (set by playwright.config.ts) serves `vite preview` with the gateway's real Content-Security-Policy
// (infra/caddy/csp.caddy, via e2e/csp.ts) so the browser tests fail on anything the policy would block in production.
// Loaded only then: the Docker build has no infra/ directory.
const previewHeaders = async (): Promise<Record<string, string> | undefined> =>
  process.env.E2E_CSP ? { "Content-Security-Policy": (await import("./e2e/csp")).spaPolicy() } : undefined;

export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],
  build: { assetsDir: "static", sourcemap: false },
  preview: { headers: await previewHeaders() },
  server: {
    port: 5173,
    proxy: {
      "/auth": gateway,
      "/assets": gateway,
      "/healthz": gateway,
    },
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    css: false,
    exclude: ["e2e/**", "node_modules/**", "dist/**"],
  },
}));
