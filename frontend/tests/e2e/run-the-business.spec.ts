import { readFileSync } from "node:fs";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";

import { snap } from "./helpers";

/**
 * The M5 slice of the full-stack story: the operator freezes a real tenant and
 * the freeze is enforced by the engine's own guards, then the audit trail shows
 * the whole episode with a person attached. Requires `make frontend-dev` +
 * `make frontend-seed`.
 */

function org(): string {
  const file = path.join(__dirname, "..", "..", "dev", ".state", "expected.json");
  return (JSON.parse(readFileSync(file, "utf8")) as { org: string }).org;
}

async function login(page: Page, email: string): Promise<void> {
  await page.goto("/login");
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password").fill("password-123!");
  await page.getByTestId("login-submit").click();
  await page.waitForURL(/\/(dashboard|admin)/);
}

test.describe.serial("run the business", () => {
  test("the operator suspends the tenant and the engine refuses work", async ({ page }) => {
    await login(page, "root@er.dev");
    await page.goto(`/admin/tenants/${org()}`);
    await expect(page.getByTestId("tenant-detail")).toContainText("active");
    await snap(page, "tenant-before-suspend");

    await page.getByTestId("suspend-org").click();
    await expect(page.getByTestId("tenant-detail")).toContainText("suspended");
    await snap(page, "tenant-suspended");

    // The refusal comes from the same control-plane guard production uses,
    // through the tenant's own BFF path — not from UI state.
    const refused = await page.request.post(`/api/orgs/${org()}/jobs/submit`, {
      data: { kind: "train", intent: "e2e-lifecycle-freeze-check" },
    });
    expect(refused.status()).toBe(409);

    await page.getByTestId("resume-org").click();
    await expect(page.getByTestId("tenant-detail")).toContainText("active");
    await expect(page.getByTestId("suspend-org")).toBeVisible();
    await snap(page, "tenant-resumed");
  });

  test("the audit trail shows the episode with the person attached", async ({ page }) => {
    await login(page, "admin@acme.dev");
    await page.goto(`/${org()}/settings/audit`);
    const rows = page.getByTestId("audit-rows");
    await expect(rows).toContainText("org.suspend");
    await expect(rows).toContainText("org.resume");
    await expect(rows).toContainText("root@er.dev");
    await snap(page, "tenant-audit-trail");
  });

  test("stewards cannot read the audit trail", async ({ page }) => {
    await login(page, "steward@acme.dev");
    await page.goto(`/${org()}/settings/audit`);
    await expect(page.getByTestId("audit-forbidden")).toContainText("admin-only");
  });

  test("the operator's global feed merges control plane and app tiers", async ({ page }) => {
    await login(page, "root@er.dev");
    await page.goto("/admin/audit");
    await expect(page.getByTestId("audit-control-plane")).toContainText("org.suspend");
    await expect(page.getByTestId("audit-app")).toContainText("auth.login");
    await snap(page, "global-audit");
  });
});
