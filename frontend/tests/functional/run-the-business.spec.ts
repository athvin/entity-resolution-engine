import { expect, test } from "@playwright/test";

import { login, ORG, USERS } from "./helpers";

test.describe("tenant lifecycle", () => {
  test("the operator suspends and resumes a tenant; the freeze is real", async ({
    page,
    isMobile,
  }) => {
    test.skip(isMobile, "state-mutating flow runs once, on desktop");
    await login(page, USERS.superAdmin);
    // mutable-dev is the desktop mutation org; its state round-trips here.
    await page.goto("/admin/tenants/mutable-dev");
    await page.getByTestId("suspend-org").click();
    await expect(page.getByTestId("tenant-detail")).toContainText("suspended");
    await expect(page.getByTestId("resume-org")).toBeVisible();

    // A suspended org refuses work through the same guards production uses.
    const refused = await page.request.post("/api/orgs/mutable-dev/jobs/submit", {
      data: { kind: "train", intent: "lifecycle-check-1" },
    });
    expect(refused.status()).toBe(409);

    await page.getByTestId("resume-org").click();
    await expect(page.getByTestId("suspend-org")).toBeVisible();
    // Purge stays honest: visible, disabled, explained.
    await expect(page.getByTestId("lifecycle-card").getByText("Purge")).toBeDisabled();
  });
});

test.describe("audit feeds", () => {
  test("the tenant trail shows person-attributed mutations to admins only", async ({ page }) => {
    await login(page, USERS.admin);
    await page.goto(`/${ORG}/settings/audit`);
    const rows = page.getByTestId("audit-rows");
    await expect(rows).toContainText("steward.resolve_review");
    await expect(rows).toContainText("user:steward@acme.dev");
    await expect(rows).toContainText("schedule.create");
  });

  test("stewards get the honest admin-only state, not an error page", async ({ page }) => {
    await login(page, USERS.steward);
    await page.goto(`/${ORG}/settings/audit`);
    await expect(page.getByTestId("audit-forbidden")).toContainText("admin-only");
  });

  test("the operator's global feed merges both trails", async ({ page }) => {
    await login(page, USERS.superAdmin);
    await page.goto("/admin/audit");
    await expect(page.getByTestId("audit-control-plane")).toContainText("steward.resolve_review");
    // The app tier carries at least this session's login row.
    await expect(page.getByTestId("audit-app")).toContainText("auth.login");
  });
});

test.describe("fresh-tenant onboarding", () => {
  test("the empty dashboard is a checklist, and precondition failures are explained", async ({
    page,
    isMobile,
  }) => {
    test.skip(isMobile, "state-mutating flow runs once, on desktop");
    await login(page, USERS.steward);
    await page.goto("/fresh-dev/dashboard");
    const checklist = page.getByTestId("onboarding-checklist");
    await expect(checklist).toContainText("Three steps to your first golden records");
    await expect(page.getByTestId("precondition-note")).toContainText(
      "2 imports delivered and waiting on the model — they were not lost",
    );

    // Deliveries exist, so training is armed; the run stays gated until it lands.
    await expect(page.getByTestId("onboarding-run")).toBeDisabled();
    await page.getByTestId("onboarding-train").click();
    await expect(page.getByTestId("onboarding-run")).toBeEnabled({ timeout: 10_000 });
  });
});
