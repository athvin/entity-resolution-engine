import { expect, test, type Page } from "@playwright/test";

import { login, triageOrg, USERS } from "./helpers";

/** The M3 acceptance gate: a 20-item triage entirely by keyboard on desktop
 * and entirely by swipe on the mobile projects — each project against its own
 * private queue, so parallel runs never race. */

async function openInbox(page: Page, projectName: string) {
  await login(page, USERS.steward);
  await page.goto(`/${triageOrg(projectName)}/reviews`);
}

test.describe("steward triage", () => {
  test("20 reviews by keyboard alone", async ({ page, isMobile }, testInfo) => {
    test.skip(isMobile, "keyboard triage is the desktop path; swipe is below");
    await openInbox(page, testInfo.project.name);
    await expect(page.getByTestId("decision-panel")).toBeVisible();

    for (let index = 0; index < 20; index += 1) {
      const active = await page
        .getByTestId("review-list")
        .locator("[aria-current=true]")
        .textContent();
      // m / n / x in rotation — all three verdicts get exercised.
      const key = ["m", "n", "x"][index % 3] ?? "m";
      await page.keyboard.press(key);
      // The resolved row leaves the list (optimistic removal).
      await expect(async () => {
        const current = await page
          .getByTestId("review-list")
          .locator("[aria-current=true]")
          .textContent();
        expect(current).not.toBe(active);
      }).toPass({ timeout: 5_000 });
    }
    await expect(page.getByTestId("last-resolution-chip")).toBeVisible();
  });

  test("j and k move the cursor without resolving", async ({ page, isMobile }, testInfo) => {
    test.skip(isMobile, "cursor keys are desktop-only");
    await openInbox(page, testInfo.project.name);
    const current = () =>
      page.getByTestId("review-list").locator("[aria-current=true]").textContent();
    const first = await current();
    await page.keyboard.press("j");
    const second = await current();
    expect(second).not.toBe(first);
    await page.keyboard.press("k");
    expect(await current()).toBe(first);
  });

  test("20 reviews by swipe alone", async ({ page, isMobile }, testInfo) => {
    test.skip(!isMobile, "swipe is the phone path");
    // Twenty sequential gestures, each followed by a poll for the next card:
    // the cost is twenty round trips through the mobile layout, and the card
    // carries the whole field-compare grid. The default 30s is Playwright's
    // generic one rather than a budget chosen for this shape -- it runs in
    // ~11s locally and WebKit on a loaded CI runner is several times slower,
    // which left no headroom and timed out mid-loop on all three attempts.
    // Raised rather than shortened: what this test is for is that twenty
    // swipes each advance the queue, not that they are quick.
    test.setTimeout(120_000);
    await openInbox(page, testInfo.project.name);

    for (let index = 0; index < 20; index += 1) {
      const card = page.getByTestId("swipe-card");
      await expect(card).toBeVisible();
      const before = await card.textContent();
      const box = await card.boundingBox();
      if (!box) throw new Error("swipe card has no box");
      const startX = box.x + box.width / 2;
      const startY = box.y + Math.min(60, box.height / 4);
      const direction = index % 2 === 0 ? 1 : -1; // alternate match / no-match
      await page.mouse.move(startX, startY);
      await page.mouse.down();
      await page.mouse.move(startX + direction * 80, startY, { steps: 4 });
      await page.mouse.move(startX + direction * 170, startY, { steps: 6 });
      await page.mouse.up();
      await expect(async () => {
        const after = await page.getByTestId("swipe-card").textContent();
        expect(after).not.toBe(before);
      }).toPass({ timeout: 5_000 });
    }
    await expect(page.getByTestId("last-resolution-chip")).toBeVisible();
  });

  test("select-all walks the whole filter and typed-confirm gates the batch", async ({
    page,
    isMobile,
  }) => {
    test.skip(isMobile, "bulk selection is the desktop grid path");
    await login(page, USERS.steward);
    await page.goto("/selectall-dev/reviews");
    await expect(page.getByTestId("review-list")).toBeVisible();

    // Header checkbox selects every LOADED row; the banner offers the rest.
    await page.getByTestId("select-all-loaded").check();
    await expect(page.getByTestId("bulk-bar")).toContainText("50 selected");
    await expect(page.getByTestId("select-all-banner")).toBeVisible();

    await page.getByTestId("select-whole-filter").click();
    await expect(page.getByTestId("bulk-bar")).toContainText("130 selected");

    // Past the 100-item threshold the cost is retyped before it is paid.
    await page.getByTestId("bulk-match").click();
    const dialog = page.getByRole("dialog", { name: "Resolve 130 reviews" });
    await expect(dialog).toBeVisible();
    const confirm = dialog.getByRole("button", { name: "Match all" });
    await expect(confirm).toBeDisabled();
    await dialog.getByRole("textbox").fill("130");
    const [response] = await Promise.all([
      page.waitForResponse((r) => r.url().includes("/reviews/bulk-resolve")),
      confirm.click(),
    ]);
    const body = (await response.json()) as { failed: number };
    expect(body.failed).toBe(0);
    await expect(page.getByTestId("inbox-empty")).toBeVisible({ timeout: 10_000 });
  });

  test("bulk resolve lands as one batch with one reconcile job", async ({
    page,
    isMobile,
  }, testInfo) => {
    test.skip(isMobile, "bulk selection is the desktop grid path");
    await openInbox(page, testInfo.project.name);
    const boxes = page.getByTestId("review-list").locator("input[type=checkbox]");
    // Rows 30+: far from the queue head the concurrent keyboard-20 test consumes.
    for (let index = 30; index < 33; index += 1) {
      await boxes.nth(index).check();
    }
    await expect(page.getByTestId("bulk-bar")).toContainText("3 selected");
    const [response] = await Promise.all([
      page.waitForResponse((r) => r.url().includes("/reviews/bulk-resolve")),
      page.getByTestId("bulk-match").click(),
    ]);
    const body = (await response.json()) as { failed: number; apply_job?: string };
    expect(body.failed).toBe(0);
    expect(body.apply_job).toBeTruthy();
    await expect(page.getByTestId("bulk-bar")).toHaveCount(0);
  });

  /**
   * V6, and it belongs in the MUTATING tier even though it writes only to the
   * BFF's own Postgres: asking stores a row, and the rendering specs run
   * against a shared read-only tenant whose baselines a stored recommendation
   * would move.
   *
   * This is the one test that crosses the whole seam — badge → BFF route →
   * review detail → record pair → the scripted model → the store → back to the
   * badge. The unit tier can prove the parse and the store in isolation; only
   * this proves the route assembles the right request from two upstream reads.
   */
  test("asks the model to adjudicate a pair and shows the answer as a suggestion", async ({
    page,
    isMobile,
  }, testInfo) => {
    test.skip(isMobile, "the recommendation sits in the desktop decision panel");
    await openInbox(page, testInfo.project.name);
    await expect(page.getByTestId("decision-panel")).toBeVisible();

    // Nothing has been asked for this pair yet, which is a different state from
    // "asked and got nothing".
    await expect(page.getByTestId("recommendation-request")).toBeVisible();

    const [response] = await Promise.all([
      page.waitForResponse(
        (r) => r.url().includes("/recommend") && r.request().method() === "POST",
      ),
      page.getByTestId("recommendation-request").click(),
    ]);
    expect(response.status()).toBe(200);

    const badge = page.getByTestId("recommendation");
    await expect(badge).toBeVisible();
    // The verdict in plain words, not the wire value.
    await expect(page.getByTestId("recommendation-verdict")).toContainText("same entity");
    await expect(page.getByTestId("recommendation-confidence")).toContainText("86%");
    await expect(page.getByTestId("recommendation-reasoning")).toContainText("Same email");
    // The advisory framing is the point of the whole surface: it must never
    // read as a decision already taken.
    await expect(badge).toContainText("AI suggests");
    await expect(badge).toContainText("your resolve below is what counts");
    // And the three resolve buttons are still the only way to act.
    await expect(page.getByTestId("resolve-match")).toBeEnabled();
  });
});
