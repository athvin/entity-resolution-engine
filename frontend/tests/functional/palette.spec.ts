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
    await input.fill("campaigns");
    await expect(page.getByRole("option", { name: /Campaigns/ })).toContainText("M4");
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
