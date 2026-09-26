import { expect, test } from "@playwright/test";

import { login, ORG, USERS } from "./helpers";

test.describe("dashboard", () => {
  test.beforeEach(async ({ page }) => {
    await login(page, USERS.steward);
    await page.goto(`/${ORG}/dashboard`);
  });

  test("renders live tiles from the control plane", async ({ page }) => {
    await expect(page.getByTestId("tile-records")).toContainText("800");
    await expect(page.getByTestId("tile-entities")).toContainText("512");
    await expect(page.getByTestId("tile-duplicate-groups")).toContainText("118");
    await expect(page.getByTestId("tile-records-in-groups")).toContainText("406");
    await expect(page.getByTestId("tile-open-reviews")).toContainText("14");
    await expect(page.getByTestId("snapshot-indicator")).toContainText("snapshot 42");
  });

  test("shows the last run, the active job, and honest nudges", async ({ page }) => {
    await expect(page.getByTestId("last-run-strip")).toContainText(
      "incremental — succeeded in 72s",
    );
    await expect(page.getByTestId("active-job-strip")).toContainText("run_all_incremental");
    await expect(page.getByTestId("attention-nudges")).toContainText("14 reviews waiting");
    await expect(page.getByTestId("attention-nudges")).toContainText(
      "last job failed (transient_io)",
    );
  });

  test("navigation matches the form factor", async ({ page, isMobile }) => {
    if (isMobile) {
      await expect(page.getByTestId("bottom-tabs")).toBeVisible();
      await expect(page.getByTestId("nav-rail")).toBeHidden();
    } else {
      await expect(page.getByTestId("nav-rail")).toBeVisible();
      await expect(page.getByTestId("bottom-tabs")).toBeHidden();
    }
  });

  test("dashboard visual baseline @visual", async ({ page }) => {
    await expect(page.getByTestId("tile-records")).toContainText("800");
    await expect(page.getByTestId("attention-nudges")).toBeVisible();
    await expect(page).toHaveScreenshot("dashboard.png", { fullPage: true });
  });
});
