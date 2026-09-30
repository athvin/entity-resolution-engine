import { expect, test } from "@playwright/test";

import { login, ORG, USERS } from "./helpers";

/** The duplicates uplift (design §5.3): groups with sort/filter, and the
 * probability-band browse over match_scores with per-pair evidence. */

test.describe("duplicates", () => {
  test("groups tab sorts and filters by source", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/duplicates`);
    await expect(page.getByTestId("duplicate-groups")).toBeVisible();

    await expect(page.getByTestId("groups-sort")).toHaveValue("members");
    await page.getByTestId("groups-sort").selectOption("name");

    // Chips render one per contributing source; pressing one narrows, pressing
    // again clears.
    const chip = page.getByTestId("source-chip-crm");
    await chip.click();
    await expect(chip).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("duplicate-groups")).toBeVisible();
    await chip.click();
    await expect(chip).toHaveAttribute("aria-pressed", "false");
  });

  test("by-score tab narrows the band and shows the evidence", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/duplicates`);
    await page.getByTestId("tab-scores").click();
    await expect(page.getByTestId("score-band-browser")).toBeVisible();

    // The acme fixture holds 30 pairs between 0.95 and 0.99.
    await expect(page.getByTestId("score-rows").locator("> div")).toHaveCount(30);

    // Narrowing to [0.95, 0.96) keeps exactly the 0.95 pairs.
    await page.getByTestId("band-low").fill("0.95");
    await page.getByTestId("band-high").fill("0.96");
    await expect(page.getByTestId("score-rows").locator("> div")).toHaveCount(6);

    await page.getByTestId("score-row-crm:C-5000").click();
    await expect(page.getByTestId("waterfall-chart")).toBeVisible();
    // Viewers see the flag action disabled, never hidden.
    await expect(page.getByTestId("flag-not-a-match")).toBeDisabled();
  });

  test("a steward merges two entities ad hoc from entity detail", async ({ page }) => {
    await login(page, USERS.steward);
    // Entity 0 (Ada Lovelace) has two source records in the mock's shape.
    await page.goto("/selectall-dev/records/01jm0e000000");
    await expect(page.getByTestId("entity-view")).toBeVisible();

    await page.getByTestId("merge-with-button").click();
    await page.getByTestId("merge-search").fill("Grace");
    await page.getByTestId("merge-candidate-01jm0e000001").click();

    // The field-by-field compare renders before anything is written.
    await expect(page.getByTestId("merge-compare")).toContainText("Given name");
    await expect(page.getByTestId("merge-compare")).toContainText("Ada");
    await expect(page.getByTestId("merge-compare")).toContainText("Grace");

    const [response] = await Promise.all([
      page.waitForResponse(
        (r) => r.url().includes("/assertions") && r.request().method() === "POST",
      ),
      page.getByTestId("merge-confirm").click(),
    ]);
    expect(response.status()).toBe(201);
    await expect(page.getByTestId("merge-note")).toContainText("always-rule written");
  });

  test("a steward flags a scored pair as not-a-match", async ({ page }) => {
    await login(page, USERS.steward);
    await page.goto("/selectall-dev/duplicates");
    await page.getByTestId("tab-scores").click();
    await expect(page.getByTestId("score-rows").locator("> div")).toHaveCount(10);

    await page.getByTestId("score-row-crm:S-6000").click();
    const [response] = await Promise.all([
      page.waitForResponse(
        (r) => r.url().includes("/assertions") && r.request().method() === "POST",
      ),
      page.getByTestId("flag-not-a-match").click(),
    ]);
    expect(response.status()).toBe(201);
    await expect(page.getByTestId("score-write-note")).toContainText("never-rule written");
  });
});
