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
    const input = page.getByPlaceholder("Type a page, workspace or command…");
    await expect(input).toBeVisible();
    await input.fill("records");
    await expect(page.getByRole("option", { name: /Records/ })).toContainText("M2");
    await input.fill("dash");
    await page.getByRole("option", { name: /Dashboard/ }).click();
    await expect(page).toHaveURL(new RegExp(`/${ORG}/dashboard$`));
  });

  test("opens from the search button", async ({ page }) => {
    await page.getByTestId("open-palette").click();
    await expect(page.getByPlaceholder("Type a page, workspace or command…")).toBeVisible();
  });

  test("switches theme from the palette", async ({ page }) => {
    await page.getByTestId("open-palette").click();
    await page.getByRole("option", { name: "Dark theme" }).click();
    await expect(page.locator("html")).toHaveClass(/dark/);
  });
});
