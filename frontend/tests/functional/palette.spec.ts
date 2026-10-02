import { expect, test } from "@playwright/test";

import { login, ORG, USERS } from "./helpers";

test.describe("command palette", () => {
  test.beforeEach(async ({ page }) => {
    await login(page, USERS.admin);
    await page.goto(`/${ORG}/dashboard`);
  });

  test("opens with the keyboard on desktop and lists pages", async ({ page, isMobile }) => {
    test.skip(isMobile, "phones have no command key; the search button path is tested below");
    await page.keyboard.press("ControlOrMeta+k");
    const input = page.getByPlaceholder("Search records, pages, commands…");
    await expect(input).toBeVisible();
    await input.fill("records");
    await page.getByRole("option", { name: "Records", exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`/${ORG}/records$`));
  });

  test("opens from the search button", async ({ page }) => {
    await page.getByTestId("open-palette").click();
    await expect(page.getByPlaceholder("Search records, pages, commands…")).toBeVisible();
  });

  test("switches theme from the palette", async ({ page }) => {
    await page.getByTestId("open-palette").click();
    await page.getByRole("option", { name: "Dark theme" }).click();
    await expect(page.locator("html")).toHaveClass(/dark/);
  });

  test("finds a golden record by name and jumps to it", async ({ page }) => {
    await page.getByTestId("open-palette").click();
    await page.getByPlaceholder("Search records, pages, commands…").fill("radia");
    const option = page.getByRole("option", { name: /Radia Lovelace/ }).first();
    await expect(option).toBeVisible();
    await option.click();
    await expect(page).toHaveURL(new RegExp(`/${ORG}/records/01jm0e`));
    await expect(page.getByTestId("entity-name")).toContainText("Radia Lovelace");
  });
});

test.describe("palette — runs", () => {
  test("a steward starts a run from the palette and lands on its job", async ({
    page,
    isMobile,
  }) => {
    test.skip(isMobile, "state-mutating flow runs once, on the desktop project");
    await login(page, USERS.steward);
    // A private org: starting a run occupies the workspace's single writer
    // slot, so sharing one with the Runs page's Run-now test would disable
    // whichever button ran second.
    await page.goto("/palette-dev/dashboard");
    await page.getByTestId("open-palette").click();
    await page.getByPlaceholder("Search records, pages, commands…").fill("run now");

    // All three kinds answer to "run now" — the Runs page's split button and
    // this list are one shared vocabulary.
    await expect(page.getByRole("option", { name: /Incremental run/ })).toBeVisible();
    await expect(page.getByRole("option", { name: /Correction pass/ })).toBeVisible();
    await page.getByRole("option", { name: /Full re-resolution/ }).click();

    await page.waitForURL(/\/runs\/[^/]+$/);
    await expect(page.getByTestId("job-detail")).toContainText("run_all_full");
  });

  test("viewers get no run commands", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/dashboard`);
    await page.getByTestId("open-palette").click();
    await page.getByPlaceholder("Search records, pages, commands…").fill("run now");
    // Starting a run is a steward action, so it is absent rather than offered
    // and refused — the same rule the Runs page follows.
    await expect(page.getByRole("option", { name: /Incremental run/ })).toHaveCount(0);
  });
});

test.describe("palette — operator", () => {
  test("a super admin can view-as straight from the palette", async ({ page }) => {
    await login(page, USERS.superAdmin);
    await page.goto(`/${ORG}/dashboard`);
    await page.getByTestId("open-palette").click();
    await page.getByPlaceholder("Search records, pages, commands…").fill(`view as ${ORG}`);
    await page.getByRole("option", { name: `View ${ORG} as admin` }).click();
    await expect(page.getByTestId("impersonation-banner")).toContainText(`Viewing ${ORG} as admin`);
    await page.getByTestId("impersonation-exit").click();
    await expect(page).toHaveURL(/\/admin\/tenants$/);
  });
});
