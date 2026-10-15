/// <reference types="vitest/config" />
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// The API lives on the same origin as the app (the gateway), so the dev server proxies it.
// `/assets` is the API's route, so Vite's own hashed files go to `/static` instead.
const gateway = process.env.GATEWAY_URL ?? "http://localhost:3000";

export default defineConfig({
  plugins: [react()],
  build: { assetsDir: "static", sourcemap: false },
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
});
