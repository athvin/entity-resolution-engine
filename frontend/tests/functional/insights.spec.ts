import { expect, test } from "@playwright/test";

import { login, ORG, USERS } from "./helpers";

/**
 * Resolution metrics over time (O7b-1). O7a started retaining the snapshots
 * because every dashboard number had always been computed LIVE — the product
 * could say what is true now and nothing about the direction of travel, and
 * "is the duplicate rate going up" is the question an operator actually has.
 * `GET /insights` then had no caller at all: no BFF route, no page.
 */
test.describe("insights", () => {
  test("shows the newest value, its trend, and the change since the previous run", async ({
    page,
  }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/insights`);

    // The rate is counted by RECORD, not by group — O7a's first judgement,
    // because counting only groups calls a five-record cluster and a pair the
    // same amount of duplication, which is how a worsening corpus looks flat.
    const rate = page.getByTestId("insight-duplicate-rate");
    await expect(rate).toContainText("46.2%");
    await expect(rate).toContainText("Counted by record rather than by group");

    // The signed change, because the sign IS the information and this product
    // has no hue to carry it — and in the SAME unit the value is shown in.
    // Reporting the raw fraction rendered a jump from 36.4% to 46.2% as "+0.1",
    // which reads as a tenth of a point rather than nearly ten.
    await expect(rate).toContainText("+9.8 points since the previous run");

    // `largest_group` is kept separately on purpose: a sudden jump there is the
    // signature of an over-merge, which an average cluster size hides.
    await expect(page.getByTestId("insight-largest-group")).toContainText("over-merge");
  });

  test("a rate it cannot compute is a gap, not a zero", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/insights`);

    // The oldest snapshot in the fixture has an empty corpus, so its rate is
    // null. O7a sends null rather than 0.0 precisely so nothing draws a
    // reassuring flat line for a corpus with no records in it — the sparkline
    // has to render that as a gap.
    const spark = page.getByTestId("insight-duplicate-rate-spark");
    await expect(spark).toBeVisible();
    await expect(spark.getByTestId("insight-duplicate-rate-spark-gap-0")).toBeVisible();
    // ...and the two known points are bars, oldest first.
    await expect(spark.getByTestId("insight-duplicate-rate-spark-bar-1")).toBeVisible();
    await expect(spark.getByTestId("insight-duplicate-rate-spark-bar-2")).toBeVisible();

    // The table shows the same null as an em dash rather than 0.0%.
    const rows = page.getByTestId("insight-row");
    await expect(rows).toHaveCount(3);
    await expect(rows.last()).toContainText("—");
  });

  test("says which feed the duplicates come from, worst first", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/insights`);

    // The corpus-wide rate says a tenant HAS a problem; this says which feed to
    // go and fix, which is the difference between knowing and acting.
    const card = page.getByTestId("insight-sources");
    await expect(card).toContainText("which feed to go and fix");
    await expect(card.getByTestId("insight-source-webforms")).toContainText("3 of 4 records");
    await expect(card.getByTestId("insight-source-webforms")).toContainText("75.0%");

    // Worst first: the answer should be the first row, not alphabetical.
    const names = await card.getByTestId(/^insight-source-[a-z]+$/).allTextContents();
    expect(names[0]).toContain("webforms");
    expect(names[names.length - 1]).toContain("billing");

    // The group count is its own fact beside the record count — a source can
    // account for few duplicate RECORDS while appearing in many groups.
    await expect(card.getByTestId("insight-source-billing")).toContainText("1 group");
    await expect(card.getByTestId("insight-source-crm")).toContainText("2 groups");

    // Bars, not a pie: these quantities are compared, not apportioned.
    await expect(card.getByTestId("insight-source-bar-webforms")).toBeVisible();

    // The per-source counts must SUM to the corpus-wide row. The server
    // assembles both from one group computation precisely so they cannot
    // disagree, and asserting it here keeps the fixture honest about that —
    // a mock whose totals drifted would make a broken writer look correct,
    // which is this program's most-repeated defect.
    const rows = await card.getByTestId(/^insight-source-[a-z]+$/).allTextContents();
    const summed = rows.reduce((total, row) => {
      const m = /of ([\d,]+) records/.exec(row);
      return total + (m?.[1] ? Number(m[1].replaceAll(",", "")) : 0);
    }, 0);
    const newestRecords = await page
      .getByTestId("insight-row")
      .first()
      .locator("td")
      .nth(1)
      .innerText();
    expect(summed).toBe(Number(newestRecords.replaceAll(",", "")));
  });

  test("an org with no runs says so rather than drawing an empty frame", async ({ page }) => {
    await login(page, USERS.steward);
    await page.goto("/mutable-dev/insights");
    await expect(page.getByTestId("insights-series")).toContainText("No snapshots yet");
    await expect(page.getByTestId("insight-duplicate-rate")).toContainText("No runs yet");
    await expect(page.getByTestId("insight-sources")).toContainText("No attribution yet");
  });

  test("insights visual baseline @visual", async ({ page }) => {
    // Worth a pixel baseline specifically because this screen is CHARTS: the
    // sparklines and the attribution bars are hand-rolled DOM whose geometry no
    // other assertion covers, and a bar that silently rendered at the wrong
    // width would read as a different corpus rather than as a bug.
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/insights`);
    await expect(page.getByTestId("insight-sources")).toContainText("webforms");
    await expect(page.getByTestId("insights-series")).toBeVisible();
    await expect(page).toHaveScreenshot("insights.png", { fullPage: true });
  });

  test("the nav reaches it on both form factors", async ({ page, isMobile }) => {
    // The capability shipped headless; a page nothing links to is only
    // marginally more reachable than no page. Both navigations are checked
    // because they are different lists: the rail renders every item, while the
    // phone's tab bar holds four and pushes the rest into "More" — so a new
    // screen can be reachable on a desktop and unreachable on a phone.
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/dashboard`);
    if (isMobile) {
      await page.getByTestId("bottom-tabs").getByRole("button", { name: "More" }).click();
      // The overflow entries are `DropdownMenuItem asChild`, so the anchor
      // carries role=menuitem rather than link.
      await page.getByRole("menuitem", { name: "Insights" }).click();
    } else {
      await page.getByTestId("nav-rail").getByRole("link", { name: "Insights" }).click();
    }
    await expect(page.getByTestId("insights-page")).toBeVisible();
  });
});
