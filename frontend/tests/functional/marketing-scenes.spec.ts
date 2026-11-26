import { expect, test } from "@playwright/test";

import { CHAPTERS } from "../../src/components/marketing/chapters";

test.describe("marketing scenes and cuts", () => {
  // The next-chapter button walks CHAPTERS, and a section the list forgets is
  // skipped without a sound. So the list is held to the page: every story
  // section is a chapter, and every chapter is on the page, in page order.
  test("the chapter list names every story section, in page order", async ({ page }) => {
    await page.goto("/");
    const sections = await page
      .locator(".marketing-story section[id]")
      .evaluateAll((nodes) => nodes.map((node) => node.id));
    const ids: string[] = CHAPTERS.map((chapter) => chapter.id);
    expect(sections.length).toBeGreaterThan(0);
    for (const id of sections) expect(ids).toContain(id);

    const tops = await page.evaluate(
      (chapterIds) =>
        chapterIds.map((id) => {
          const node = document.getElementById(id);
          return node ? node.getBoundingClientRect().top + window.scrollY : null;
        }),
      ids,
    );
    expect(tops).not.toContain(null);
    expect(tops).toEqual([...tops].sort((a, b) => (a ?? 0) - (b ?? 0)));
  });

  // A cut between pages (cut.tsx) is an animation over a navigation that has
  // already happened: the new page must be there, interactive, and error-free.
  test("moving between marketing pages lands on the new page", async ({ page, isMobile }) => {
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto("/product");
    const link = page.locator('main a[href="/features/matching"]').first();
    await link.scrollIntoViewIfNeeded();
    await link.click();
    await expect(page).toHaveURL(/\/features\/matching$/);
    await expect(page.getByTestId("page-matching")).toBeVisible();
    await expect(page.getByRole("heading", { level: 1 })).toHaveText(/Matching rules/);
    // The header stays usable straight after the cut.
    if (!isMobile) {
      await page.getByTestId("marketing-header").getByRole("link", { name: "Pricing" }).click();
      await expect(page).toHaveURL(/\/pricing$/);
      await expect(page.getByTestId("pricing-page")).toBeVisible();
    }
    expect(errors).toEqual([]);
  });

  test("with reduced motion the cut is instant", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/product");
    const link = page.locator('main a[href="/features/review"]').first();
    await link.scrollIntoViewIfNeeded();
    await link.click();
    await expect(page.getByTestId("page-review")).toBeVisible();
    // No cut animation is left running once the page is there. Matched by
    // animation name only: reading `effect.pseudoElement` off a finished
    // view-transition animation is exactly the kind of access that has to
    // survive its pseudo-element being torn down.
    const running = await page.evaluate(
      () =>
        document
          .getAnimations()
          .filter(
            (animation) =>
              animation instanceof CSSAnimation &&
              animation.animationName.startsWith("marketing-cut") &&
              animation.playState === "running",
          ).length,
    );
    expect(running).toBe(0);
  });
});
