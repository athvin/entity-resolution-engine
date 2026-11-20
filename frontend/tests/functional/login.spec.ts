import { expect, test } from "@playwright/test";

test.describe("login screen", () => {
  // `/` is the public landing page now, so the invariant this protects moved
  // with the dispatcher to `/app`: the signed-in entry point must not be
  // reachable without a session, and must carry the visitor back after login.
  test("the portal entry redirects to /login when no session cookie is present", async ({
    page,
  }) => {
    await page.goto("/app");
    await expect(page).toHaveURL(/\/login\?next=%2Fapp$/);
  });

  test("the landing page is reachable with no session", async ({ page }) => {
    const response = await page.goto("/");
    expect(response?.status()).toBe(200);
    await expect(page).toHaveURL(/\/$/);
    await expect(page.getByTestId("marketing-hero")).toBeVisible();
  });

  test("renders the sign-in form", async ({ page }) => {
    await page.goto("/login");
    await expect(page.getByTestId("login-card")).toBeVisible();
    await expect(page.getByLabel("Email")).toBeVisible();
    await expect(page.getByLabel("Password")).toBeVisible();
    await expect(page.getByTestId("login-submit")).toBeVisible();
  });

  test("the logo goes back to the website's home page", async ({ page }) => {
    await page.goto("/login");
    const home = page.getByRole("link", { name: "DupeZero home" });
    await expect(home).toHaveAttribute("href", "/");
    const box = await home.boundingBox();
    expect(box?.height).toBeGreaterThanOrEqual(44);
    await home.click();
    await expect(page).toHaveURL(/\/$/);
    await expect(page.getByTestId("marketing-hero")).toBeVisible();
  });

  test("deep links carry a next parameter through the redirect", async ({ page }) => {
    await page.goto("/acme/default/dashboard");
    await expect(page).toHaveURL(/\/login\?next=%2Facme%2Fdefault%2Fdashboard$/);
  });

  test("login page visual baseline @visual", async ({ page }) => {
    await page.goto("/login");
    await expect(page.getByTestId("login-card")).toBeVisible();
    await expect(page).toHaveScreenshot("login.png", { fullPage: true });
  });
});
