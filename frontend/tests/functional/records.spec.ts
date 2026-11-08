import { expect, test } from "@playwright/test";

import { DS, login, ORG, USERS } from "./helpers";
import AxeBuilder from "@axe-core/playwright";

// Every third mock entity has two members; index 0 is multi-member and its
// name is deterministic (Ada Lovelace, index 0).
const MULTI_MEMBER_ENTITY = "01jm0e000000";

test.describe("golden records", () => {
  test.beforeEach(async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/${DS}/records`);
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
    await page.goto(`/${ORG}/${DS}/records/${entity}`);

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
    await page.goto(`/${ORG}/${DS}/records/${MULTI_MEMBER_ENTITY}`);
    await expect(page.getByTestId("entity-links")).toBeVisible();
    // Seeing that an entity belongs to a household is reading; creating the
    // link is a steward's judgement.
    await expect(page.getByTestId("entity-link-target")).toHaveCount(0);
    await expect(page.getByTestId("entity-link-add")).toHaveCount(0);
  });

  test("entity detail explains provenance, members and history", async ({ page }) => {
    await page.goto(`/${ORG}/${DS}/records/${MULTI_MEMBER_ENTITY}`);
    await expect(page.getByTestId("entity-name")).toContainText("Ada Lovelace");
    await expect(page.getByTestId("entity-view")).toContainText("2 source records");
    await expect(page.getByTestId("lineage-email")).toContainText("from crm:C-5000");
    await expect(page.getByTestId("lineage-email")).toContainText("source_priority");
    await expect(page.getByTestId("entity-members")).toContainText("webforms");
    await expect(page.getByTestId("entity-timeline")).toContainText("member added");
  });

  test("an unknown entity is a designed state, not an error page", async ({ page }) => {
    await page.goto(`/${ORG}/${DS}/records/does-not-exist`);
    await expect(page.getByText("No entity does-not-exist in this workspace.")).toBeVisible();
  });

  test("records grid visual baseline @visual", async ({ page }) => {
    await expect(
      page.getByTestId("records-list").getByText("ada.lovelace0@example.test"),
    ).toBeVisible();
    await expect(page).toHaveScreenshot("records.png");
  });

  test("entity detail visual baseline @visual", async ({ page }) => {
    await page.goto(`/${ORG}/${DS}/records/${MULTI_MEMBER_ENTITY}`);
    await expect(page.getByTestId("entity-timeline")).toContainText("member added");
    await expect(page).toHaveScreenshot("entity-detail.png", { fullPage: true });
  });
});

test.describe("mass update (steward)", () => {
  test.beforeEach(async ({ page }) => {
    await login(page, USERS.steward);
    await page.goto(`/${ORG}/${DS}/records`);
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

/**
 * Manual group merge (spec 2.10): arbitrary records the matcher never grouped,
 * selected in the browser and merged as a group. What this tier proves is the
 * bundle: the union of BOTH entities' member records, mastered on the chosen
 * survivor's first member, with NO review_id — its absence is the audit
 * trail's manual mark. The lake-level landing (and its equality with a
 * detected merge) is the Rust `pg_merge_group` e2e's assertion.
 *
 * Mock entities are index-keyed: `01jm0e000000` (index 0) holds
 * crm:C-5000 + webforms:W-9000; `01jm0e000001` (index 1) holds crm:C-5001.
 */
test.describe("manual merge of arbitrary records", () => {
  // The merge POST bumps the org's stagedCount, so the journey runs on
  // merge-dev — the same reason merge-dialog.spec.ts does — because acme-dev's
  // "3 decisions queued" banner is asserted verbatim by reviews.spec.ts. The
  // viewer test mutates nothing and stays: viewer membership exists only on
  // acme-dev.
  const MERGE_ORG = "merge-dev";

  test("a viewer is offered no selection at all", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/${DS}/records`);
    await expect(page.getByTestId("records-list")).toBeVisible();
    await expect(page.getByTestId("merge-select-01jm0e000000")).toHaveCount(0);
  });

  test("two selected records merge with the survivor's member as master", async ({ page }) => {
    await login(page, USERS.steward);
    await page.goto(`/${MERGE_ORG}/${DS}/records`);

    await page.getByTestId("merge-select-01jm0e000000").check();
    await page.getByTestId("merge-select-01jm0e000001").check();
    await expect(page.getByTestId("merge-selection-count")).toHaveText("2 selected");
    await page.getByTestId("merge-selected").click();

    const dialog = page.getByTestId("manual-merge-dialog");
    await expect(dialog).toBeVisible();
    // Both sides listed, with member counts; the first is the default survivor.
    await expect(page.getByTestId("manual-merge-side-01jm0e000000")).toContainText("2 records");
    await expect(page.getByTestId("manual-merge-side-01jm0e000001")).toContainText("1 record");

    // Switch the survivor to the second entity — the master follows.
    await page.getByTestId("manual-merge-side-01jm0e000001").click();

    const request = page.waitForRequest(
      (req) =>
        req.url().includes(`/api/orgs/${MERGE_ORG}/datasets/${DS}/merges`) &&
        req.method() === "POST",
    );
    await page.getByTestId("manual-merge-confirm").click();
    const posted = (await request).postDataJSON() as Record<string, unknown>;

    expect(posted.records).toEqual(["crm:C-5000", "webforms:W-9000", "crm:C-5001"]);
    expect(posted.master).toBe("crm:C-5001");
    expect(posted).not.toHaveProperty("review_id");
    expect(posted.apply_now).toBe(true);

    await expect(page.getByTestId("manual-merge-done")).toContainText("merge staged");
    // The selection resets with the merge handed off.
    await expect(page.getByTestId("merge-selection-bar")).toHaveCount(0);
  });

  test("the open dialog passes the axe bar the records screen is held to", async ({ page }) => {
    // Same trap a11y.spec.ts documents: axe samples the dialog mid
    // `animate-lift-in`, when the composited layer is still semi-transparent,
    // and reports colour-contrast violations against markup that is compliant
    // once the animation completes. Reduced motion clamps it to its end state.
    await page.emulateMedia({ reducedMotion: "reduce" });
    await login(page, USERS.steward);
    await page.goto(`/${MERGE_ORG}/${DS}/records`);
    await page.getByTestId("merge-select-01jm0e000000").check();
    await page.getByTestId("merge-select-01jm0e000001").check();
    await page.getByTestId("merge-selected").click();
    await expect(page.getByTestId("manual-merge-sides")).toBeVisible();
    const results = await new AxeBuilder({ page })
      .withTags(["wcag2a", "wcag2aa"])
      .include('[data-testid="manual-merge-dialog"]')
      .analyze();
    expect(
      results.violations.filter((v) => v.impact === "serious" || v.impact === "critical"),
    ).toEqual([]);
  });
});
