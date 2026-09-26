import { defineConfig, devices } from "@playwright/test";

/**
 * The fast tier: Playwright against the mock erserver (tests/mock-erserver).
 * Carries all functional coverage, all visual baselines, and all three device
 * projects. No Docker, no real backend. The full-stack tier lives in
 * playwright.e2e.config.ts.
 *
 * Visual baselines are Linux-only (generated in CI's image or via
 * `make frontend-baselines`); on other OSes snapshot comparison is skipped so
 * local runs never fight font rendering.
 */
const artifactsDir = process.env.PLAYWRIGHT_ARTIFACTS_DIR;

export default defineConfig({
  testDir: "./tests/functional",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  ignoreSnapshots: process.platform !== "linux",
  outputDir: artifactsDir ? `${artifactsDir}/test-results` : "./test-results",
  reporter: [
    ["list"],
    [
      "html",
      {
        outputFolder: artifactsDir ? `${artifactsDir}/playwright-report` : "./playwright-report",
        open: "never",
      },
    ],
  ],
  expect: {
    toHaveScreenshot: { maxDiffPixelRatio: 0.01, animations: "disabled" },
  },
  use: {
    baseURL: "http://localhost:3100",
    trace: "on-first-retry",
    video: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    {
      name: "desktop-chromium",
      use: { ...devices["Desktop Chrome"], viewport: { width: 1440, height: 900 } },
    },
    { name: "mobile-safari", use: { ...devices["iPhone 15"] } },
    { name: "mobile-chrome", use: { ...devices["Pixel 7"] } },
  ],
  webServer: [
    {
      command: "node tests/mock-erserver/server.mjs",
      port: 8010,
      reuseExistingServer: !process.env.CI,
    },
    {
      command: "node_modules/.bin/next start --port 3100",
      port: 3100,
      reuseExistingServer: !process.env.CI,
      env: {
        ERSERVER_BASE_URL: "http://localhost:8010",
        ERSERVER_OPERATOR_TOKEN: "mock-operator-token",
        ERWEB_DATABASE_URL: "postgresql://mock:mock@localhost:1/mock",
        ERWEB_SESSION_SECRET: "mock-session-secret-mock-session-secret",
        ERWEB_CREDENTIAL_KEY: "3q2+7wEirykuXBXRO26AY9nbZAqAnDzws76jd2GxwkE=",
      },
    },
  ],
});
