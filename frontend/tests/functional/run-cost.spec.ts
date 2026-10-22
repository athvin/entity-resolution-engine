import { expect, test } from "@playwright/test";

import { login, ORG, USERS } from "./helpers";

/**
 * E6c: the blocking estimator (E6a) and the config's segment catalog (E6b)
 * reach the UI. Both endpoints existed with no caller at all — an operator
 * could not see what a run would cost before starting it, which is the whole
 * answer to the incumbent's three-hour cap.
 */
test.describe("run cost", () => {
  test("the estimate is on demand, not on page load", async ({ page }) => {
    await login(page, USERS.steward);

    // Fail the estimate request if anything fetches it before being asked: the
    // query opens the org's lake and counts pairs, so an automatic fetch would
    // make a cheap page expensive to open.
    let calls = 0;
    await page.route(`**/api/orgs/${ORG}/blocking-estimate*`, (route) => {
      calls += 1;
      return route.fallback();
    });

    await page.goto(`/${ORG}/runs`);
    await expect(page.getByTestId("run-cost")).toBeVisible();
    expect(calls).toBe(0);

    await page.getByTestId("run-cost-estimate").click();
    await expect(page.getByTestId("run-cost-candidate")).toContainText("1,842");
    expect(calls).toBe(1);
  });

  test("shows the pair count, what blocking avoided, and each rule's marginal", async ({
    page,
  }) => {
    await login(page, USERS.steward);
    await page.goto(`/${ORG}/runs`);
    await page.getByTestId("run-cost-estimate").click();

    const panel = page.getByTestId("run-cost");
    // 1,842 of 124,750 possible — the number that decides the run's cost, and
    // the share the rules avoid.
    await expect(page.getByTestId("run-cost-candidate")).toContainText("1,842");
    await expect(panel).toContainText("124,750");
    await expect(page.getByTestId("run-cost-avoided")).toContainText("98.5%");

    // The marginal column is what says whether a rule is pulling its weight:
    // the third rule adds 127 pairs where the first adds 1,203.
    await expect(panel).toContainText("1,203");
    await expect(panel).toContainText("127");

    // An oversized block dominates a run on its own, so it is named.
    await expect(page.getByTestId("run-cost-blocks")).toContainText("406 pairs");
  });

  test("an org with no model says so instead of erroring", async ({ page }) => {
    await login(page, USERS.steward);
    // `fresh-dev` has no model; the endpoint 409s and that is a designed state.
    await page.goto("/fresh-dev/runs");
    await page.getByTestId("run-cost-estimate").click();
    await expect(page.getByTestId("run-cost-unavailable")).toContainText("no model yet");
    await expect(page.getByTestId("run-cost-error")).toHaveCount(0);
  });

  test("a viewer is not offered a run's cost", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/runs`);
    await expect(page.getByTestId("jobs-table")).toBeVisible();
    // The panel rides the steward's run-trigger surface; a viewer cannot start
    // a run, so costing one is not their question.
    await expect(page.getByTestId("run-cost")).toHaveCount(0);
  });

  test("a run can be scoped to a declared segment", async ({ page, isMobile }) => {
    test.skip(isMobile, "state-mutating flow runs once, on the desktop project");
    // Its own org: starting a run takes the org's single writer slot, so
    // sharing one with the Runs page's Run-now test would leave whichever ran
    // second looking at a correctly disabled button.
    const org = "segment-dev";
    await login(page, USERS.steward);
    await page.goto(`/${org}/runs`);

    // The names come from the published config; the predicate rides the option
    // title so an operator can see what the slice covers.
    const picker = page.getByTestId("run-segment");
    await expect(picker).toContainText("Whole corpus");
    await expect(picker).toContainText("west");

    // The job carries the segment NAME. The predicate is resolved server-side
    // against the org's own config, so no SQL travels in a request.
    const submitted = page.waitForRequest(
      (request) => request.url().includes(`/api/orgs/${org}/jobs`) && request.method() === "POST",
    );
    await picker.selectOption("west");
    await page.getByTestId("run-now").click();
    const request = await submitted;
    expect(JSON.parse(request.postData() ?? "{}")).toMatchObject({
      kind: "run_all_incremental",
      params: { segment: "west" },
    });
  });
});
