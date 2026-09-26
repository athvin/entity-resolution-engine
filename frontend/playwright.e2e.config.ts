import { defineConfig, devices } from "@playwright/test";

/**
 * The full-stack tier ("actually works end to end"): Playwright against the real
 * seeded dev stack. Gated on FRONTEND_E2E=1 (mirrors the server's ERSERVER_E2E
 * convention) and never runs on the PR path — `make frontend-e2e` is the
 * documented release gate. Requires `make frontend-dev` + `make frontend-seed`.
 *
 * No visual comparison here (pixels depend on live data); every journey step
 * saves a full-page screenshot via the snap() helper instead.
 */
const artifactsDir = process.env.PLAYWRIGHT_ARTIFACTS_DIR;

export default defineConfig({
  testDir: "./tests/e2e",
  globalSetup: "./tests/e2e/global-setup.ts",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 300_000,
  ignoreSnapshots: true,
  outputDir: artifactsDir ? `${artifactsDir}/e2e-results` : "./test-results",
  reporter: [
    ["list"],
    [
      "html",
      {
        outputFolder: artifactsDir ? `${artifactsDir}/e2e-report` : "./playwright-report",
        open: "never",
      },
    ],
  ],
  use: {
    baseURL: "http://localhost:3200",
    trace: "on-first-retry",
    video: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    {
      name: "desktop-chromium",
      use: { ...devices["Desktop Chrome"], viewport: { width: 1440, height: 900 } },
    },
    { name: "mobile-chrome", use: { ...devices["Pixel 7"] } },
  ],
  webServer: {
    command: "node_modules/.bin/next start --port 3200",
    port: 3200,
    reuseExistingServer: true,
    env: {
      ERSERVER_BASE_URL: "http://localhost:8000",
      ERSERVER_OPERATOR_TOKEN: process.env.ERSERVER_OPERATOR_TOKEN ?? "dev-operator-token",
      ERWEB_DATABASE_URL:
        process.env.ERWEB_DATABASE_URL ?? "postgresql://postgres:er@localhost:5433/postgres",
      ERWEB_SESSION_SECRET:
        process.env.ERWEB_SESSION_SECRET ?? "dev-session-secret-dev-session-secret",
      ERWEB_CREDENTIAL_KEY:
        process.env.ERWEB_CREDENTIAL_KEY ?? "3q2+7wEirykuXBXRO26AY9nbZAqAnDzws76jd2GxwkE=",
      ERWEB_INSECURE_COOKIES: "1", // plain-HTTP localhost
    },
  },
});
