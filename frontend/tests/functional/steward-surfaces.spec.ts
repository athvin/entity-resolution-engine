import { expect, test } from "@playwright/test";

import { login, ORG, ORG_MUTABLE, USERS } from "./helpers";

// Which multi-member acme entity each project unmerges — one per project so
// parallel runs never race on the same entity (every third index is multi).
const UNMERGE_ENTITY: Record<string, string> = {
  "desktop-chromium": "01jm0e000003",
  "mobile-safari": "01jm0e000006",
  "mobile-chrome": "01jm0e000009",
};

test.describe("sources & imports", () => {
  test("lists configured sources from the active config", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/sources`);
    for (const source of ["crm", "billing", "webforms"]) {
      await expect(page.getByTestId(`source-${source}`)).toBeVisible();
    }
    await expect(page.getByTestId("sources-page")).toContainText("config v1");
  });

  test("receipts answer 'did my file do anything?' honestly", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/sources`);
    const receipts = page.getByTestId("receipts");
    await expect(receipts).toContainText("webforms");
    await expect(receipts).toContainText("12"); // new
    await expect(receipts).toContainText("identical delivery, not a failure");
  });

  test("a steward uploads a file and the import queues", async ({ page }) => {
    await login(page, USERS.steward);
    await page.goto(`/${ORG_MUTABLE}/sources`);
    await page.getByTestId("source-crm-file").setInputFiles({
      name: "delta.csv",
      mimeType: "text/csv",
      buffer: Buffer.from("id,email\n1,a@b.c\n"),
    });
    await expect(page.getByTestId("source-crm-status")).toContainText("queued — job");
  });

  test("viewers get a read-only sources page", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/sources`);
    await expect(page.getByTestId("source-crm")).toContainText("importing needs the steward role");
    await expect(page.getByTestId("source-crm-upload")).toHaveCount(0);
  });

  test("sources visual baseline @visual", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/sources`);
    await expect(page.getByTestId("receipts")).toContainText("webforms");
    await expect(page).toHaveScreenshot("sources.png", { fullPage: true });
  });
});

test.describe("schedules", () => {
  test("renders the ledger with the config-owned row locked", async ({ page }) => {
    await login(page, USERS.admin);
    await page.goto(`/${ORG}/runs/schedules`);
    await expect(page.getByTestId("schedule-run_all_incremental")).toContainText("0 6 * * *");
    const systemRow = page.getByTestId("schedule-correct");
    await expect(systemRow).toContainText("config (correction pass)");
    await expect(page.getByTestId("schedule-toggle-correct")).toBeDisabled();
  });

  test("an admin creates, disables and deletes a schedule", async ({ page, isMobile }) => {
    test.skip(isMobile, "state-mutating flow runs once, on the desktop project");
    await login(page, USERS.admin);
    await page.goto(`/${ORG_MUTABLE}/runs/schedules`);
    await page.getByTestId("schedule-kind").selectOption("run_all_full");
    // The raw escape hatch: whatever the API accepts stays reachable.
    await page.getByTestId("cron-preset-custom").click();
    await page.getByTestId("cron-custom").fill("30 4 * * *");
    await expect(page.getByTestId("cron-preview")).toContainText("runs next:");
    await page.getByTestId("schedule-create").click();
    const row = page.getByTestId("schedule-run_all_full");
    await expect(row).toContainText("30 4 * * *");

    const toggle = page.getByTestId("schedule-toggle-run_all_full");
    await expect(toggle).toHaveAttribute("aria-checked", "true");
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-checked", "false");

    await row.getByRole("button", { name: /Delete/ }).click();
    await expect(page.getByTestId("schedule-run_all_full")).toHaveCount(0);
  });

  test("the cron builder composes presets and previews the next fires", async ({ page }) => {
    await login(page, USERS.admin);
    await page.goto(`/${ORG_MUTABLE}/runs/schedules`);
    // Daily is the default shape, and the composed expression is always visible.
    await expect(page.getByTestId("cron-value")).toHaveText("0 6 * * *");
    await expect(page.getByTestId("cron-preview")).toContainText("UTC");

    await page.getByTestId("cron-preset-weekly").click();
    await expect(page.getByTestId("cron-value")).toHaveText("0 6 * * 0");
    await page.getByLabel("Day of week").selectOption("3");
    await expect(page.getByTestId("cron-value")).toHaveText("0 6 * * 3");

    await page.getByTestId("cron-preset-hourly").click();
    await expect(page.getByTestId("cron-value")).toHaveText("0 * * * *");

    // An unparseable expression says so instead of pretending to schedule.
    await page.getByTestId("cron-preset-custom").click();
    await page.getByTestId("cron-custom").fill("not cron");
    await expect(page.getByTestId("cron-preview")).toContainText("not a valid 5-field cron");
  });

  test("stewards see schedules but cannot edit", async ({ page }) => {
    await login(page, USERS.steward);
    await page.goto(`/${ORG}/runs/schedules`);
    await expect(page.getByTestId("schedules-table")).toContainText("run_all_incremental");
    await expect(page.getByTestId("new-schedule")).toHaveCount(0);
    await expect(page.getByTestId("schedule-toggle-run_all_incremental")).toBeDisabled();
  });
});

test.describe("unmerge", () => {
  test("a steward splits a record out of a merged entity", async ({ page }, testInfo) => {
    const entityId = UNMERGE_ENTITY[testInfo.project.name];
    if (!entityId) throw new Error(`no unmerge fixture for ${testInfo.project.name}`);
    await login(page, USERS.steward);
    await page.goto(`/${ORG}/records/${entityId}`);
    await expect(page.getByTestId("entity-view")).toContainText("2 source records");

    const webformsMember = page.locator("[data-testid^=mark-webforms]");
    await webformsMember.check();
    await page.getByTestId("unmerge-button").click();
    await expect(page.getByTestId("unmerge-status")).toContainText(
      "unmerge applied — 1 never-rule",
    );
    // The invalidated refetch shows the member gone (the mock applies instantly;
    // the real engine shows it after the reconcile the message references).
    await expect(page.getByTestId("entity-members")).not.toContainText("webforms", {
      timeout: 10_000,
    });
  });

  test("viewers see no unmerge controls", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/records/01jm0e000000`);
    await expect(page.getByTestId("entity-members")).toBeVisible();
    await expect(page.getByTestId("unmerge-button")).toHaveCount(0);
  });
});
