import { expect, test } from "@playwright/test";

test.describe("login screen", () => {
  test("root redirects to /login when no session cookie is present", async ({ page }) => {
    await page.goto("/");
    await expect(page).toHaveURL(/\/login$/);
  });

  test("renders the sign-in form", async ({ page }) => {
    await page.goto("/login");
    await expect(page.getByTestId("login-card")).toBeVisible();
    await expect(page.getByLabel("Email")).toBeVisible();
    await expect(page.getByLabel("Password")).toBeVisible();
    await expect(page.getByTestId("login-submit")).toBeVisible();
  });

  test("deep links carry a next parameter through the redirect", async ({ page }) => {
    await page.goto("/acme/dashboard");
    await expect(page).toHaveURL(/\/login\?next=%2Facme%2Fdashboard$/);
  });

  test("login page visual baseline @visual", async ({ page }) => {
    await page.goto("/login");
    await expect(page.getByTestId("login-card")).toBeVisible();
    await expect(page).toHaveScreenshot("login.png", { fullPage: true });
  });
});
