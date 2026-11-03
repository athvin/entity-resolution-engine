import { expect, test } from "@playwright/test";

import { DS, login, ORG, USERS } from "./helpers";

/**
 * The dataset profile: what is in this corpus and what it will cost to match.
 *
 * Nothing in the product answered either question before this. The new-source
 * wizard profiles the first 200 rows in the browser and the file is never
 * profiled again after it lands; the engine never looked inside
 * `raw_records.payload` at all. On the matching side, blocking decides the
 * entire cost of a run and the config validator checked only that a column
 * EXISTS — a rule keyed on a 97%-empty column passed validation and destroyed
 * recall silently.
 *
 * What this tier can and cannot check is worth stating, because the line is
 * exactly where this repo's recurring false green lives. The mock serves ONE
 * shape for every caller: it sees an API key, not a role. So the five
 * value-bearing fields are never redacted here, and any assertion about a
 * viewer not seeing them would pass against a mock that redacts nothing.
 * That half is checked in `tests/component/profile-redaction.test.tsx`, which
 * passes the nulled shape directly. This file checks the journey and the
 * numbers.
 */
test.describe("profile", () => {
  test("shows each column's shape, and its distribution on expansion", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/${DS}/profile`);

    await expect(page.getByTestId("column-table")).toBeVisible();

    // Blank is reported separately from null, always: ingest stores every
    // delivered cell as a JSON string, so an empty cell is `''` and not NULL.
    // A fill rate that counted `''` as filled would read 93.5% here.
    const email = page.getByTestId("column-row-email");
    await expect(email).toContainText("92.0%");
    await expect(email).toContainText("12");

    // The distinct count is EXACT when the profile computed one. The sketch is
    // marked with a tilde when it is all there is, because
    // `approx_count_distinct` is a HyperLogLog and is wrong in both directions
    // — on this repo's own 1000-row fixture it returns 978 for a genuinely
    // unique column, so an unmarked figure would be fabricated exactness.
    await expect(email).toContainText("724");

    // The distribution lives in the detail, with its count beside its share:
    // "one value dominates" is actionable only once you know it is 248 rows.
    await page.getByRole("button", { name: /postcode/ }).click();
    const detail = page.getByTestId("column-detail-postcode");
    await expect(detail).toContainText("94103");
    await expect(detail).toContainText("31.0%");
    await expect(detail).toContainText("248");
  });

  test("findings lead, grouped by what the severity means", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/${DS}/profile`);

    // Findings are the primary output, not a footnote — the best decision in
    // ydata-profiling is that its Alerts section comes before the per-variable
    // detail, because "what is wrong with this" is the question.
    await expect(page.getByTestId("profile-findings")).toBeVisible();

    // The heading says what the level MEANS rather than naming it, because the
    // palette is unavailable and "error" alone decodes to nothing. And the
    // wording must not claim anything is blocked: a profile is descriptive.
    await expect(page.getByTestId("profile-findings-error")).toContainText(
      "will hurt matching if left as it is",
    );
    await expect(page.getByTestId("finding-P004")).toContainText("placeholder values");

    // P010 is `info` and exists so the most actionable line is present whether
    // or not anything is wrong: a panel that only ever reports problems is one
    // people learn to skip.
    await expect(page.getByTestId("finding-P010")).toContainText("most selective usable blocking");
  });

  test("the blocking table shows the cost AND what the cost loses", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/${DS}/profile`);

    // Tier 1 measures no keys, so the table is absent and says which tier
    // would measure them — rather than rendering an empty table, which reads
    // as "measured, found nothing".
    await expect(page.getByTestId("blocking-keys-empty")).toContainText("tier 2");

    // A steward can queue the tier that measures them.
    await login(page, USERS.steward);
    await page.goto(`/${ORG}/${DS}/profile`);
    await expect(page.getByTestId("profile-run-tier2")).toBeVisible();
  });

  test("a corpus nobody has profiled says 'not yet', not zero", async ({ page }) => {
    // STEWARD, not viewer: `fresh-dev` is a TRIAGE_ORG, and those grant admin
    // and steward only. A viewer there is a non-member, which the BFF answers
    // with 404 so they learn nothing — and the fixtures warn in place that it
    // "reads as a missing page". The test would have failed for membership
    // reasons while appearing to be about the empty state.
    await login(page, USERS.steward);
    // `fresh-dev` has nothing computed. The screen must say the measurement has
    // not been taken — the ThresholdEditor lesson, where an empty array
    // rendered as a confident "0 of 0 pairs": an answer given before the
    // question could be answered.
    await page.goto(`/fresh-dev/${DS}/profile`);
    await expect(page.getByTestId("profile-absent")).toContainText("Not profiled yet");

    // The tier checklist is present with nothing running, so the two tiers and
    // what each measures are readable BEFORE committing to either.
    await expect(page.getByTestId("profile-tier-2")).toContainText("blocking key");
    await expect(page.getByTestId("profile-tier-1")).toHaveAttribute("data-state", "idle");
  });

  test("a viewer is told why they cannot run one, rather than shown a button that 403s", async ({
    page,
  }) => {
    // In `acme-dev`, where a viewer membership actually exists.
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/${DS}/profile`);

    await expect(page.getByTestId("column-table")).toBeVisible();
    // Running a profile writes three lake tables and takes the dataset's one
    // writer slot, so it is steward+. The affordance is absent rather than
    // present-and-failing.
    await expect(page.getByTestId("profile-run-tier2")).toHaveCount(0);
  });

  test("the switcher moves between the three corpora", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/${DS}/profile`);

    // One screen, three subjects — the mechanics are identical, so a golden
    // profile is the raw profile with a different FROM clause.
    await page.getByTestId("profile-subject-golden").click();
    await expect(page.getByTestId("profile-subject-golden")).toHaveAttribute(
      "aria-selected",
      "true",
    );
    await expect(page.getByTestId("column-table")).toBeVisible();

    // The raw subject is per source, because two sources landing in one
    // dataset have different columns — so it needs a source before it can ask.
    await page.getByTestId("profile-subject-raw").click();
    await expect(page.getByTestId("profile-source")).toBeVisible();
  });
});
