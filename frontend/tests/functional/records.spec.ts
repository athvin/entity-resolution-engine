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

  // E11b. One entity per device project: the three projects share one mock
  // server, and this spec CREATES links on the entity it views — so a shared
  // entity would have desktop's link in mobile's list and the counts would
  // depend on which project ran first.
  const LINK_ENTITY: Record<string, string> = {
    "desktop-chromium": "01jm0e000012",
    "mobile-safari": "01jm0e000015",
    "mobile-chrome": "01jm0e000018",
  };

  test("links two entities without merging them (E11b)", async ({ page }, testInfo) => {
    const entity = LINK_ENTITY[testInfo.project.name] ?? "01jm0e000012";
    await login(page, USERS.steward);
    await page.goto(`/${ORG}/records/${entity}`);

    const panel = page.getByTestId("entity-links");
    await expect(panel).toBeVisible();
    await expect(page.getByTestId("entity-links-empty")).toBeVisible();

    await page.getByTestId("entity-link-target").fill("01jm0e000099");
    await page.getByTestId("entity-link-kind").selectOption("household");
    await page.getByTestId("entity-link-add").click();

    // The link lands WITHOUT merging: the panel names the relationship and the
    // other entity, and both keep their own records.
    const list = page.getByTestId("entity-links-list");
    await expect(list).toContainText("Same household");
    await expect(list).toContainText("01jm0e000099");
    // The viewed entity is the parent, so the sentence is "contains" — the
    // same row read from the other end says "belongs to", and a panel that
    // said one thing for both would be wrong half the time.
    await expect(list).toContainText("contains");
    // The entity is still its own entity: linking is not merging, so the
    // members list is untouched.
    await expect(page.getByTestId("entity-members")).toBeVisible();

    // Removing it leaves nothing behind.
    await list.getByRole("button", { name: /Remove the Same household link/ }).click();
    await expect(page.getByTestId("entity-links-empty")).toBeVisible();
  });

  test("a viewer reads links but is offered no controls (E11b)", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/records/${MULTI_MEMBER_ENTITY}`);
    await expect(page.getByTestId("entity-links")).toBeVisible();
    // Seeing that an entity belongs to a household is reading; creating the
    // link is a steward's judgement.
    await expect(page.getByTestId("entity-link-target")).toHaveCount(0);
    await expect(page.getByTestId("entity-link-add")).toHaveCount(0);
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

test.describe("mass update (steward)", () => {
  test.beforeEach(async ({ page }) => {
    await login(page, USERS.steward);
    await page.goto(`/${ORG}/records`);
  });

  test("sets a golden field across matching records with a typed confirm", async ({ page }) => {
    await page.getByTestId("mass-edit-open").click();
    await page.getByTestId("mass-edit-field").selectOption("email");
    await page.getByTestId("mass-edit-value").fill("bulk@example.test");
    await page.getByTestId("mass-edit-review").click();

    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    // The confirm restates the blast radius; type that exact count to proceed.
    const description = (await dialog.textContent()) ?? "";
    const count = /on (\d+) record/.exec(description)?.[1] ?? "";
    expect(count).not.toBe("");
    await dialog.getByRole("textbox").fill(count);
    await dialog.getByRole("button", { name: "Set field" }).click();

    await expect(page.getByTestId("mass-edit-done")).toContainText("Updated email");
  });
});
