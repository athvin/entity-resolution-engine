import { expect, test } from "@playwright/test";

import { DS, login, ORG, PASSWORD, USERS } from "./helpers";

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
    await expect(page).toHaveURL(new RegExp(`/${ORG}/${DS}/dashboard$`));
    await expect(page.getByTestId("dashboard")).toBeVisible();
  });

  test("sign-in reaches the portal even when client navigation is stalled", async ({ page }) => {
    await page.goto("/login?next=%2Fapp");
    // Hold only the portal's client-router request. A document navigation can
    // still reach it with the newly issued session cookie.
    await page.route("**/app?*", (route) => {
      if (route.request().headers()["rsc"] === "1") return;
      return route.continue();
    });
    await page.getByLabel("Email").fill(USERS.steward);
    await page.getByLabel("Password").fill(PASSWORD);
    const response = page.waitForResponse("**/api/auth/login");
    await page.getByTestId("login-submit").click();
    expect((await response).status()).toBe(200);
    await expect(page).toHaveURL(new RegExp(`/${ORG}/${DS}/dashboard$`));
    await expect(page.getByTestId("dashboard")).toBeVisible();
  });

  test("a stalled sign-in times out and lets the user retry", async ({ page }) => {
    await page.goto("/login");
    await page.clock.install();
    let attempts = 0;
    await page.route("**/api/auth/login", (route) => {
      attempts += 1;
      if (attempts === 1) return;
      return route.continue();
    });
    await page.getByLabel("Email").fill(USERS.steward);
    await page.getByLabel("Password").fill(PASSWORD);
    const request = page.waitForRequest("**/api/auth/login");
    await page.getByTestId("login-submit").click();
    await request;
    await expect(page.getByTestId("login-submit")).toHaveText("Signing in…");
    await page.clock.fastForward(15_001);
    await expect(page.getByTestId("login-error")).toHaveText("sign-in timed out — try again");
    await expect(page.getByTestId("login-submit")).toBeEnabled();
    await page.clock.resume();
    await page.getByTestId("login-submit").click();
    await expect(page).toHaveURL(new RegExp(`/${ORG}/${DS}/dashboard$`));
  });

  test("signing out ends the session and re-locks the app", async ({ page }) => {
    await login(page, USERS.steward);
    await page.goto(`/${ORG}/${DS}/dashboard`);
    await page.getByTestId("user-menu").click();
    await page.getByTestId("sign-out").click();
    await expect(page).toHaveURL(/\/login$/);
    await page.goto(`/${ORG}/${DS}/dashboard`);
    await expect(page).toHaveURL(/\/login\?next=/);
  });

  test("a session cookie alone is not enough for another org's data", async ({ page }) => {
    await login(page, USERS.steward);
    const response = await page.request.get("/api/orgs/some-other-org/datasets/default/metrics");
    expect(response.status()).toBe(404);
  });
});
