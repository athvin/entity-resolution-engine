import { expect, test } from "@playwright/test";

import { login, ORG, USERS } from "./helpers";

// A single-member, stable entity. Enrichment never mutates mock state (apply is
// a canned mass-update), so it is safe to run on the read fixtures and alongside
// other read-only entity tests.
const ENTITY = "01jm0e000001";

test.describe("AI enrichment (steward)", () => {
  test("suggests fixes, drops no-ops, and accepts one as a durable override", async ({ page }) => {
    await login(page, USERS.steward);
    await page.goto(`/${ORG}/records/${ENTITY}`);

    const panel = page.getByTestId("enrich-panel");
    await expect(panel).toBeVisible();
    await page.getByTestId("enrich-open").click();

    // The empty-field fill survives; its "before" is the em dash.
    const suggestion = page.getByTestId("enrich-suggestion-addr_unit");
    await expect(suggestion).toBeVisible({ timeout: 15_000 });
    await expect(page.getByTestId("enrich-value-addr_unit")).toHaveText("Suite 410");
    await expect(page.getByTestId("enrich-current-addr_unit")).toHaveText("—");

    // The birth_date suggestion matched the current value — the BFF drops no-ops,
    // so it never reaches the UI.
    await expect(page.getByTestId("enrich-suggestion-birth_date")).toHaveCount(0);

    await page.getByTestId("enrich-apply-addr_unit").click();
    await expect(page.getByTestId("enrich-applied-addr_unit")).toContainText("accepted");
  });

  test("viewers see no enrichment controls", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/records/${ENTITY}`);
    await expect(page.getByTestId("entity-golden")).toBeVisible();
    await expect(page.getByTestId("enrich-panel")).toHaveCount(0);
  });
});
