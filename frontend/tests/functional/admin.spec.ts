import { expect, test } from "@playwright/test";

import { login, ORG, USERS } from "./helpers";

test.describe("operator console", () => {
  test("only super admins can see it", async ({ page }) => {
    await login(page, USERS.admin);
    const response = await page.request.get("/api/admin/orgs");
    expect(response.status()).toBe(404);
    await page.goto("/admin/tenants");
    await expect(page.getByTestId("tenants-list")).toHaveCount(0);
  });

  test("lists tenants with state and vault status", async ({ page }) => {
    await login(page, USERS.superAdmin);
    await page.goto("/admin/tenants");
    const acme = page.getByTestId(`tenant-${ORG}`);
    await expect(acme).toContainText("Acme (dev)");
    await expect(acme).toContainText("active");
    await expect(acme).toContainText("vaulted");
  });

  test("provisions a tenant end to end: one-time key, live state, vaulted keys", async ({
    page,
    browserName,
    isMobile,
  }) => {
    const name = `nova-${browserName}${isMobile ? "-m" : ""}`;
    await login(page, USERS.superAdmin);
    await page.goto("/admin/tenants/new");
    await page.getByTestId("tenant-name").fill(name);
    await page.getByTestId("provision-submit").click();

    // The one-time admin key is displayed exactly once, for handoff.
    await expect(page.getByTestId("one-time-admin-key")).toContainText("erk_onetime");
    // provisioning → active as the detail poll observes the control plane.
    await expect(page.getByTestId("provision-progress")).toContainText("active", {
      timeout: 15_000,
    });

    await page.getByTestId("open-new-tenant").click();
    await expect(page.getByTestId("tenant-detail")).toContainText(
      "vaulted (viewer / steward / admin)",
    );
  });

  test("global queue shows every org's jobs", async ({ page }) => {
    await login(page, USERS.superAdmin);
    await page.goto("/admin/queue");
    await expect(page.getByTestId("global-queue")).toContainText(ORG);
    await expect(page.getByTestId("global-queue")).toContainText("train");
  });

  test("operator raises a tenant's DuckDB threads for a faster pipeline", async ({
    page,
    browserName,
    isMobile,
  }) => {
    // A per-project org: the three projects run this mutation concurrently against
    // one shared mock, so a fixed org would race (and the seeded orgs feed the
    // @visual baseline). This one is provisioned fresh at the seeded 2 threads / 4GB.
    const org = `res-${browserName}${isMobile ? "-m" : ""}`;
    await login(page, USERS.superAdmin);
    await page.goto("/admin/tenants/new");
    await page.getByTestId("tenant-name").fill(org);
    await page.getByTestId("provision-submit").click();
    await expect(page.getByTestId("provision-progress")).toContainText("active", {
      timeout: 15_000,
    });
    await page.goto(`/admin/tenants/${org}`);

    const card = page.getByTestId("resources-card");
    await expect(card.getByTestId("duckdb-threads")).toHaveValue("2");

    await card.getByTestId("duckdb-threads").fill("6");
    await card.getByTestId("duckdb-memory").fill("6GB");
    await card.getByTestId("save-resources").click();

    // The control-plane echo drives the cache; the inputs reflect the persisted values.
    await expect(card.getByTestId("save-resources")).toBeDisabled();
    await expect(card.getByTestId("duckdb-threads")).toHaveValue("6");
    await expect(card.getByTestId("duckdb-memory")).toHaveValue("6GB");

    // Client-side validation blocks an out-of-range value before any request.
    await card.getByTestId("duckdb-threads").fill("99");
    await expect(card.getByTestId("save-resources")).toBeDisabled();
    await expect(card).toContainText("whole number 1–32");
  });

  test("tenant detail visual baseline @visual", async ({ page }) => {
    await login(page, USERS.superAdmin);
    await page.goto(`/admin/tenants/${ORG}`);
    await expect(page.getByTestId("tenant-detail")).toContainText("vaulted");
    await expect(page).toHaveScreenshot("admin-tenant-detail.png", { fullPage: true });
  });
});
