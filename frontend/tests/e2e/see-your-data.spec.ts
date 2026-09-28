import { readFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";

import { snap } from "./helpers";

/**
 * The M2 slice of the full-stack story: browse the real corpus, open an entity
 * with provenance, watch runs — then the operator provisions a brand-new tenant
 * through the UI and impersonates it. Requires `make frontend-dev` + seed.
 */
interface Expected {
  org: string;
  metrics: { records: number; entities: number };
}

function expected(): Expected {
  const file = path.join(__dirname, "..", "..", "dev", ".state", "expected.json");
  return JSON.parse(readFileSync(file, "utf8")) as Expected;
}

async function loginAs(page: import("@playwright/test").Page, email: string) {
  await page.goto("/login");
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password").fill("password-123!");
  await page.getByTestId("login-submit").click();
  await page.waitForURL(/\/dashboard$/); // the cookie is live once the redirect lands
}

test("a steward browses real golden records with provenance and history", async ({ page }) => {
  const { org } = expected();
  await loginAs(page, "steward@acme.dev");
  await expect(page).toHaveURL(new RegExp(`/${org}/dashboard$`));

  await page.goto(`/${org}/records`);
  await expect(page.getByTestId("snapshot-indicator")).toBeVisible();
  const rows = page.getByTestId("records-list").locator("a");
  await expect(rows.first()).toBeVisible();
  await snap(page, "records-real");

  // Open the first record; the seeded corpus resolves ~800 records into ~300
  // entities, so provenance and members render from real lineage.
  await rows.first().click();
  await expect(page.getByTestId("entity-golden")).toBeVisible();
  await expect(page.getByTestId("entity-members")).toBeVisible();
  await expect(page.getByTestId("entity-timeline")).toBeVisible();
  await snap(page, "entity-real");

  await page.goto(`/${org}/runs`);
  // Assert on the job kind every journey keeps producing, not the seed's
  // one-time train — the tenant is long-lived and each e2e pass appends jobs,
  // so train eventually scrolls out of the table's window.
  await expect(page.getByTestId("jobs-table")).toContainText("run_all");
  await expect(page.getByTestId("runs-table")).toContainText("succeeded");
  await snap(page, "runs-real");
});

test("the operator provisions a tenant through the UI and impersonates it", async ({ page }) => {
  test.setTimeout(240_000); // real provisioning creates a database and inits a lake
  const name = `e2e${Date.now().toString(36)}`;

  await loginAs(page, "root@er.dev");
  await page.goto("/admin/tenants/new");
  await page.getByTestId("tenant-name").fill(name);
  await page.getByTestId("provision-submit").click();

  await expect(page.getByTestId("one-time-admin-key")).toContainText("erk_");
  await snap(page, "provision-onetime-key");
  await expect(page.getByTestId("provision-progress")).toContainText("active", {
    timeout: 180_000,
  });
  await snap(page, "provision-active");

  // Straight into the tenant's shoes: fresh workspace, honest empty states.
  await page.goto("/admin/tenants");
  await page.getByTestId(`view-as-${name}`).click();
  await page.getByTestId(`view-as-${name}-admin`).click();
  await expect(page.getByTestId("impersonation-banner")).toContainText(`Viewing ${name} as admin`);
  await expect(page.getByTestId("dashboard")).toBeVisible();
  await snap(page, "impersonated-fresh-tenant");

  await page.getByTestId("impersonation-exit").click();
  await expect(page).toHaveURL(/\/admin\/tenants$/);
});
