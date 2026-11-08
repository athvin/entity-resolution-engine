import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import { DS, login, USERS } from "./helpers";

/**
 * The editable review merge (spec unit 2.8), end to end through the BFF
 * against the mock control plane: a review whose side A already sits in a
 * two-member entity, so the dialog's group is THREE records — the shape the
 * pair grid cannot show.
 *
 * `merge-dev` is this journey's own org: merging resolves the review, and no
 * other spec may watch it disappear.
 *
 * What "lands exactly as shown" means at this tier: the bundle the dialog
 * POSTs carries exactly the records, master and pinned values the dialog
 * displayed. The lake-level landing (the staged bundle becomes the merged
 * entity's golden record) is the Rust `pg_merge_group` e2e's assertion.
 */

const MERGE_ORG = "merge-dev";

test.describe("review merge dialog", () => {
  test.beforeEach(async ({ page, isMobile }) => {
    test.skip(isMobile, "the dialog opens from the desktop decision panel");
    await login(page, USERS.steward);
    await page.goto(`/${MERGE_ORG}/${DS}/reviews`);
    await expect(page.getByTestId("decision-panel")).toBeVisible();
  });

  test("a 3-record group merges with copied and edited values, exactly as shown", async ({
    page,
  }) => {
    await page.getByTestId("open-merge-dialog").click();
    const dialog = page.getByTestId("merge-group-dialog");
    await expect(dialog).toBeVisible();

    // THREE columns: the pair plus rec_a's entity sibling — and the master is
    // the review's left side, marked by inversion, not hue.
    await expect(page.getByTestId("merge-master-mark")).toHaveText("master");
    const headers = dialog.locator("th");
    await expect(headers.nth(1)).toContainText("crm:G-1");
    await expect(headers.nth(2)).toContainText("archive:G-1");
    await expect(headers.nth(3)).toContainText("billing:G-9");

    // Click-to-copy: archive's "Robert" replaces the master's "Bob".
    await page.getByTestId("merge-copy-given_name-archive:G-1").click();
    await expect(page.getByTestId("merge-master-given_name")).toHaveText("Robert");

    // Click-again-to-revert, then re-stage: both directions land.
    await page.getByTestId("merge-copy-given_name-archive:G-1").click();
    await expect(page.getByTestId("merge-master-given_name")).toHaveText("Bob");
    await page.getByTestId("merge-copy-given_name-archive:G-1").click();
    await expect(page.getByTestId("merge-master-given_name")).toHaveText("Robert");

    // Inline edit on the master's email.
    await page.getByTestId("merge-edit-open-email").click();
    await page.getByTestId("merge-edit-email").fill("merged@example.test");
    await page.getByTestId("merge-edit-email").press("Enter");
    await expect(page.getByTestId("merge-master-email")).toHaveText("merged@example.test");

    // The submit says what it carries.
    await expect(page.getByTestId("merge-group-confirm")).toContainText("2 pinned values");

    const request = page.waitForRequest(
      (req) =>
        req.url().includes(`/api/orgs/${MERGE_ORG}/datasets/${DS}/merges`) &&
        req.method() === "POST",
    );
    await page.getByTestId("merge-group-confirm").click();
    const posted = (await request).postDataJSON() as {
      records: string[];
      master: string;
      overrides: { attribute: string; value?: string; clear?: boolean }[];
      review_id: string;
    };

    // The bundle is exactly what the dialog showed: all three records, the
    // chosen master, the two pins, and the review it resolves.
    expect(posted.records).toEqual(["crm:G-1", "archive:G-1", "billing:G-9"]);
    expect(posted.master).toBe("crm:G-1");
    expect(posted.overrides).toEqual([
      { attribute: "given_name", value: "Robert" },
      { attribute: "email", value: "merged@example.test" },
    ]);
    expect(posted.review_id).toBe("01jmmergerev00000000000001");

    // The dialog closes and the review leaves the queue. merge-dev carries
    // THREE same-shape reviews — one per test in this spec, since a merge
    // consumes one — so the inbox does not empty: the next copy slides into
    // the decision panel, indistinguishable by content. The staged banner is
    // the honest account of when it lands; its count is unpinned because the
    // shared mock accumulates across this spec's merges.
    await expect(dialog).not.toBeVisible();
    await expect(page.getByTestId("decision-panel")).toBeVisible();
    await expect(page.getByTestId("staged-banner")).toContainText("decisions queued");
  });

  test("master reassignment moves the column and re-keys the bundle", async ({ page }) => {
    await page.getByTestId("open-merge-dialog").click();
    await expect(page.getByTestId("merge-group-dialog")).toBeVisible();

    await page.getByTestId("merge-make-master-billing:G-9").click();
    const headers = page.getByTestId("merge-group-dialog").locator("th");
    await expect(headers.nth(1)).toContainText("billing:G-9");

    // Copy the crm side's given name into the NEW master.
    await page.getByTestId("merge-copy-given_name-crm:G-1").click();

    const request = page.waitForRequest(
      (req) =>
        req.url().includes(`/api/orgs/${MERGE_ORG}/datasets/${DS}/merges`) &&
        req.method() === "POST",
    );
    await page.getByTestId("merge-group-confirm").click();
    const posted = (await request).postDataJSON() as {
      records: string[];
      master: string;
      overrides: { attribute: string; value?: string }[];
    };
    expect(posted.master).toBe("billing:G-9");
    expect(posted.records).toEqual(["crm:G-1", "archive:G-1", "billing:G-9"]);
    expect(posted.overrides).toEqual([{ attribute: "given_name", value: "Bob" }]);
  });

  test("the open dialog passes the axe bar the reviews screen is held to", async ({ page }) => {
    await page.getByTestId("open-merge-dialog").click();
    await expect(page.getByTestId("merge-group-table")).toBeVisible();
    const results = await new AxeBuilder({ page })
      .withTags(["wcag2a", "wcag2aa"])
      .include('[data-testid="merge-group-dialog"]')
      .analyze();
    expect(
      results.violations.filter((v) => v.impact === "serious" || v.impact === "critical"),
    ).toEqual([]);
  });
});
