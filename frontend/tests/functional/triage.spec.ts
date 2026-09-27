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
});
