import { expect, test } from "@playwright/test";

import { login, ORG, USERS } from "./helpers";

// Every third mock entity has two members; index 0 is multi-member and its
// name is deterministic (Ada Lovelace, index 0).
const MULTI_MEMBER_ENTITY = "01jm0e000000";

test.describe("golden records", () => {
  test.beforeEach(async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/records`);
  });

  test("browses the snapshot-pinned grid and paginates by cursor", async ({ page }) => {
    await expect(page.getByTestId("snapshot-indicator")).toContainText("snapshot 42");
    const list = page.getByTestId("records-list");
    await expect(list.getByText("ada.lovelace0@example.test")).toBeVisible();
    // Page two (items 100+) arrives after scrolling; index 200 lives there.
    await list.evaluate((element) => {
      element.scrollTop = element.scrollHeight;
    });
    await expect(list.getByText("ada.hopper200@example.test")).toBeVisible({ timeout: 10_000 });
  });

  test("search narrows server-side", async ({ page }) => {
    await page.getByTestId("records-search").fill("perlman");
    await expect(
      page
        .getByTestId("records-list")
        .getByText(/perlman\d+@example\.test/i, { exact: false })
        .first(),
    ).toBeVisible();
    await page.getByTestId("records-search").fill("no-such-person-xyz");
    await expect(page.getByTestId("records-empty")).toBeVisible();
  });

  test("entity detail explains provenance, members and history", async ({ page }) => {
    await page.goto(`/${ORG}/records/${MULTI_MEMBER_ENTITY}`);
    await expect(page.getByTestId("entity-name")).toContainText("Ada Lovelace");
    await expect(page.getByTestId("entity-view")).toContainText("2 source records");
    await expect(page.getByTestId("lineage-email")).toContainText("from crm:C-5000");
    await expect(page.getByTestId("lineage-email")).toContainText("source_priority");
    await expect(page.getByTestId("entity-members")).toContainText("webforms");
    await expect(page.getByTestId("entity-timeline")).toContainText("member added");
  });

  test("an unknown entity is a designed state, not an error page", async ({ page }) => {
    await page.goto(`/${ORG}/records/does-not-exist`);
    await expect(page.getByText("No entity does-not-exist in this workspace.")).toBeVisible();
  });

  test("records grid visual baseline @visual", async ({ page }) => {
    await expect(
      page.getByTestId("records-list").getByText("ada.lovelace0@example.test"),
    ).toBeVisible();
    await expect(page).toHaveScreenshot("records.png");
  });

  test("entity detail visual baseline @visual", async ({ page }) => {
    await page.goto(`/${ORG}/records/${MULTI_MEMBER_ENTITY}`);
    await expect(page.getByTestId("entity-timeline")).toContainText("member added");
    await expect(page).toHaveScreenshot("entity-detail.png", { fullPage: true });
  });
});
