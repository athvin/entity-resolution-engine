import { expect, test } from "@playwright/test";

import { login, ORG, USERS } from "./helpers";

// acme-dev review fixtures are render-only: resolving them would shift the
// dashboard's "14 reviews waiting" and the visual baselines under parallel
// projects. Everything that mutates runs against mutable-dev (see triage spec).
test.describe("review inbox — rendering", () => {
  test.beforeEach(async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/reviews`);
  });

  test("shows the queue, the staged banner, and the evidence waterfall", async ({
    page,
    isMobile,
  }) => {
    await expect(page.getByTestId("staged-banner")).toContainText("3 decisions queued");
    if (isMobile) {
      await expect(page.getByTestId("swipe-card")).toBeVisible();
      await expect(page.getByTestId("swipe-card")).toContainText("crm:C-1000");
    } else {
      await expect(page.getByTestId("review-list")).toContainText("crm:C-1000");
      await expect(page.getByTestId("decision-panel")).toContainText("82.0%");
    }
    const chart = page.getByTestId("waterfall-chart").first();
    await expect(chart).toContainText("email: agree — supports the match (+4.1)");
    await expect(chart).toContainText("birth date: disagree — argues against (-2.3)");
    await expect(chart).toContainText("total match weight");
  });

  test("explains why each pair is here and filters by reason", async ({ page, isMobile }) => {
    test.skip(isMobile, "the filter control is identical; keep the matrix lean");
    await expect(page.getByTestId("decision-panel")).toContainText(
      "scored between your review threshold and auto-merge",
    );
    await page.getByTestId("reason-filter").selectOption("never_unsatisfiable");
    await expect(page.getByTestId("review-list")).toContainText("crm:C-1013");
    await expect(page.getByTestId("review-list")).not.toContainText("crm:C-1000");
  });

  test("viewers cannot act", async ({ page, isMobile }) => {
    if (isMobile) {
      const bar = page.getByTestId("mobile-action-bar");
      await expect(bar.getByRole("button", { name: "Match" })).toBeDisabled();
    } else {
      await expect(page.getByTestId("resolve-match")).toBeDisabled();
      await expect(page.getByTestId("review-list").locator("input[type=checkbox]")).toHaveCount(0);
    }
  });

  test("assertion library lists rules and the contradiction inspector names the cycle", async ({
    page,
  }) => {
    await page.getByTestId("tab-assertions").click();
    await expect(page.getByTestId("contradictions")).toContainText("1 rule conflict");
    await expect(page.getByTestId("contradictions")).toContainText("crm:C-1005");
    await expect(page.getByTestId("assertion-library")).toContainText("unmerge:01jm0e000000");
    // Viewers see no retract buttons.
    await expect(page.getByTestId("retract-01jmassert0000000000000003")).toHaveCount(0);
    // Retracted rows appear only when asked for.
    await expect(page.getByTestId("assertion-01jmassert0000000000000001")).toHaveCount(0);
    await page.getByText("show retracted").click();
    await expect(page.getByTestId("assertion-01jmassert0000000000000001")).toContainText(
      "retracted",
    );
  });

  test("review inbox visual baseline @visual", async ({ page }) => {
    await expect(page.getByTestId("staged-banner")).toBeVisible();
    await expect(page.getByTestId("waterfall-chart").first()).toContainText("total match weight");
    await expect(page).toHaveScreenshot("reviews.png", { fullPage: true });
  });
});
