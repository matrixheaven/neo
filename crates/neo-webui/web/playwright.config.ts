import { defineConfig } from "@playwright/test";

const PORT = Number(process.env.MOCK_PORT ?? 47921);

export default defineConfig({
  testDir: "./test/browser",
  testMatch: "screenshots.spec.ts",
  timeout: 30_000,
  retries: 0,
  workers: 1,
  reporter: [["list"]],
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
  },
  webServer: {
    command: "node test/browser/mock-server.mjs",
    port: PORT,
    reuseExistingServer: false,
    timeout: 15_000,
  },
});
