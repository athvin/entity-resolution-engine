import { expect, test } from "@playwright/test";

import { login, ORG, PASSWORD, USERS } from "./helpers";

test.describe("authentication", () => {
  test("rejects a wrong password with one generic message", async ({ page }) => {
    await page.goto("/login");
    await page.getByLabel("Email").fill(USERS.steward);
    await page.getByLabel("Password").fill("wrong-password");
    await page.getByTestId("login-submit").click();
    await expect(page.getByTestId("login-error")).toHaveText("invalid email or password");
    await expect(page).toHaveURL(/\/login/);
  });

  test("signs a steward in through the form and lands on the dashboard", async ({ page }) => {
    await page.goto("/login");
    await page.getByLabel("Email").fill(USERS.steward);
    await page.getByLabel("Password").fill(PASSWORD);
    await page.getByTestId("login-submit").click();
    await expect(page).toHaveURL(new RegExp(`/${ORG}/dashboard$`));
    await expect(page.getByTestId("dashboard")).toBeVisible();
  });

  test("signing out ends the session and re-locks the app", async ({ page }) => {
    await login(page, USERS.steward);
    await page.goto(`/${ORG}/dashboard`);
    await page.getByTestId("user-menu").click();
    await page.getByTestId("sign-out").click();
    await expect(page).toHaveURL(/\/login$/);
    await page.goto(`/${ORG}/dashboard`);
    await expect(page).toHaveURL(/\/login\?next=/);
  });

  test("a session cookie alone is not enough for another org's data", async ({ page }) => {
    await login(page, USERS.steward);
    const response = await page.request.get("/api/orgs/some-other-org/metrics");
    expect(response.status()).toBe(404);
  });
});
