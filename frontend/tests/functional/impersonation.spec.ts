import { expect, test } from "@playwright/test";

import { login, ORG, ORG_MUTABLE, USERS } from "./helpers";

const FAILED_JOB = "01jm000000000000000000000b";

test.describe("impersonation — view as tenant", () => {
  test.beforeEach(async ({ page }) => {
    await login(page, USERS.superAdmin);
  });

  test("viewing as viewer really is a viewer, everywhere", async ({ page }) => {
    await page.goto("/admin/tenants");
    await page.getByTestId(`view-as-${ORG}`).click();
    await page.getByTestId(`view-as-${ORG}-viewer`).click();

    // Lands on the tenant dashboard with the unmissable banner.
    await expect(page).toHaveURL(new RegExp(`/${ORG}/dashboard$`));
    await expect(page.getByTestId("impersonation-banner")).toContainText(
      `Viewing ${ORG} as viewer`,
    );

    // The impersonated role is enforced: no steward controls on a failed job.
    await page.goto(`/${ORG}/runs/${FAILED_JOB}`);
    await expect(page.getByTestId("job-error")).toBeVisible();
    await expect(page.getByTestId("job-resume")).toHaveCount(0);

    // Every other org answers 404 — "view as tenant" sees exactly what they see.
    const other = await page.request.get(`/api/orgs/${ORG_MUTABLE}/metrics`);
    expect(other.status()).toBe(404);

    // The console itself bounces back to the impersonated tenant.
    await page.goto("/admin/tenants");
    await expect(page).toHaveURL(new RegExp(`/${ORG}/dashboard$`));

    // Exit returns to the console with full powers restored.
    await page.getByTestId("impersonation-exit").click();
    await expect(page).toHaveURL(/\/admin\/tenants$/);
    const restored = await page.request.get(`/api/orgs/${ORG_MUTABLE}/metrics`);
    expect(restored.status()).toBe(200);
  });

  test("impersonation is audited both ways", async ({ page }) => {
    await page.goto("/admin/tenants");
    await page.getByTestId(`view-as-${ORG}`).click();
    await page.getByTestId(`view-as-${ORG}-admin`).click();
    await expect(page.getByTestId("impersonation-banner")).toBeVisible();
    await expect(page.getByTestId("impersonation-banner")).toContainText(
      "actions are audited under both identities",
    );
    await page.getByTestId("impersonation-exit").click();
    await expect(page).toHaveURL(/\/admin\/tenants$/);
  });
});
