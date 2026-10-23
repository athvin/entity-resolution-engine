import { expect, test } from "@playwright/test";

import { labelingOrg, login, ORG, USERS } from "./helpers";

/**
 * The labelling surface (E10c-2).
 *
 * E10b built the labels table and the uncertainty-sampled queue and shipped no
 * UI at all — `grep -r labeling frontend/src` returned nothing but a generated
 * binding. So a tenant could not record a verdict, which meant `m_from_labels`
 * had nothing to train on and the per-version evaluation built in E10c-1 would
 * have read "no label can be scored against this version" forever.
 *
 * **Assertions here are relative, not absolute.** All four tests share one org
 * per device project and the mock is stateful: recording a verdict removes that
 * pair from the queue for every later test, exactly as the server does. A test
 * asserting "the head is 0.940" therefore passes or fails on which test ran
 * first, which is the latent-flake shape this tier has been bitten by before.
 * So each test reads the current state, acts, and asserts what *changed*.
 *
 * The uncertainty ORDERING is asserted in the DuckLake tier, where there are
 * real scores to order — the same split `pg_labels.rs` records.
 */
const SCORE = /^0\.\d{3}$/;

test.describe("labelling", () => {
  test("asks about a scored pair and explains why that pair", async ({ page }, testInfo) => {
    const org = labelingOrg(testInfo.project.name);
    await login(page, USERS.steward);
    await page.goto(`/${org}/labeling`);

    const queue = page.getByTestId("labeling-queue");
    // A bare probability is a defect on this product: the score is rendered to
    // three places and next to the threshold that explains the ask.
    await expect(queue.getByTestId("labeling-probability")).toHaveText(SCORE);
    await expect(queue).toContainText("auto-merge threshold");

    // The two records' values, so the question is answerable at all. Without
    // this the screen asks a human to judge two opaque keys.
    await expect(page.getByTestId("field-compare-grid")).toBeVisible();

    // The verdict is recorded against the model that produced the score, which
    // is what makes it usable by per-version evaluation.
    await expect(queue).toContainText("v0001");
  });

  test("a verdict moves the queue on and is counted", async ({ page }, testInfo) => {
    const org = labelingOrg(testInfo.project.name);
    await login(page, USERS.steward);
    await page.goto(`/${org}/labeling`);

    const score = page.getByTestId("labeling-probability");
    await expect(score).toHaveText(SCORE);
    const before = await score.textContent();

    await page.getByTestId("labeling-match").click();

    // A different pair, because the server excludes already-labelled pairs and
    // the refetch is what advances. This is the assertion that fails if the
    // component also advances an index of its own — it would skip a pair.
    await expect(score).not.toHaveText(before ?? "");
    await expect(score).toHaveText(SCORE);

    // Coverage is split by verdict and origin: one total hides a set that is
    // almost all `no_match` and teaches the model very little.
    const coverage = page.getByTestId("labeling-coverage");
    await expect(coverage).toContainText("Match");
    await expect(coverage).toContainText("From a human");
  });

  test("skipping advances without recording anything", async ({ page }, testInfo) => {
    const org = labelingOrg(testInfo.project.name);
    await login(page, USERS.steward);
    await page.goto(`/${org}/labeling`);

    const score = page.getByTestId("labeling-probability");
    const coverage = page.getByTestId("labeling-coverage");
    await expect(score).toHaveText(SCORE);
    const scoreBefore = await score.textContent();
    const coverageBefore = await coverage.textContent();

    await page.getByTestId("labeling-skip").click();

    // Moved on, but nothing written: a labeller who is unsure must be able to
    // pass without training the model on a guess.
    await expect(score).not.toHaveText(scoreBefore ?? "");
    await expect(coverage).toHaveText(coverageBefore ?? "");
  });

  test("a viewer sees what the matcher is unsure about but cannot answer", async ({ page }) => {
    // The shared read-only org, not a per-project one: the test writes nothing
    // so it cannot race, and the fixtures grant a VIEWER membership only in
    // `ORG` — the per-project orgs get admin and steward alone, so a viewer
    // there gets a 404 from the tenant layout rather than a readonly page.
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/labeling`);

    // The readonly note must appear and the buttons must never, including
    // during the window before the session resolves — `effectiveRole` returns
    // null while loading, so a permission check written as `!== "viewer"`
    // offered a viewer the buttons until it settled.
    await expect(page.getByTestId("labeling-readonly")).toBeVisible();
    await expect(page.getByTestId("labeling-match")).toHaveCount(0);
    await expect(page.getByTestId("labeling-no-match")).toHaveCount(0);
    // Reading the queue is not a change, so it is still shown.
    await expect(page.getByTestId("labeling-probability")).toHaveText(SCORE);
  });
});
