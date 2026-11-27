import { expect, test, type Page } from "@playwright/test";

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

test.describe("the hero's opening scene", () => {
  const playStates = (page: Page) =>
    page
      .locator("#hero-scene-stage")
      .evaluate((stage) =>
        stage.getAnimations({ subtree: true }).map((animation) => animation.playState),
      );

  // It plays once, in under five seconds (WCAG 2.2.2 then asks for no pause
  // control), holds its payoff, and Replay runs it again from the top.
  test("plays once, holds the golden record, and replays", async ({ page }) => {
    await page.goto("/");
    const scene = page.getByTestId("hero-scene");
    // On a short phone the scene sits below the first screen and waits to be
    // seen (scene-playback.tsx); bring it into view the way a reader would.
    await scene.scrollIntoViewIfNeeded();
    await expect(scene).toBeVisible();
    expect((await playStates(page)).length).toBeGreaterThan(0);
    await expect
      .poll(async () => (await playStates(page)).every((state) => state === "finished"), {
        timeout: 7_000,
      })
      .toBe(true);
    const longest = await page
      .locator("#hero-scene-stage")
      .evaluate((stage) =>
        Math.max(
          ...stage
            .getAnimations({ subtree: true })
            .map((animation) => Number(animation.effect?.getComputedTiming().endTime ?? 0)),
        ),
      );
    expect(longest).toBeLessThanOrEqual(5_000);
    await expect(scene.getByText(/Golden record · E-1042/)).toBeVisible();

    await page.getByTestId("hero-scene-stage-replay").click();
    expect(await playStates(page)).toContain("running");
  });

  test("with reduced motion it shows the payoff and nothing moves", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/");
    await expect(page.getByTestId("hero-scene").getByText(/Golden record · E-1042/)).toBeVisible();
    expect(await playStates(page)).toEqual([]);
    await expect(page.getByTestId("hero-scene-stage-replay")).toBeHidden();
  });

  // Beside the headline on wide screens, between the intro and the foot on
  // phones: it must never sit on a word of the copy, and the way into the
  // story stays on the first screen of a phone.
  test("never covers the copy, and the flow link stays on a phone's first screen", async ({
    page,
    isMobile,
  }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    for (const width of isMobile ? [0] : [1440, 1280, 1024, 820]) {
      if (width) await page.setViewportSize({ width, height: 900 });
      await page.goto("/");
      const overlaps = await page.evaluate(() => {
        const stage = document.querySelector("#hero-scene-stage")?.getBoundingClientRect();
        if (!stage) return ["no stage"];
        const hits: string[] = [];
        for (const selector of [".story-hero h1", ".story-intro", ".story-hero-foot"]) {
          const node = document.querySelector(selector);
          if (!node) continue;
          // The glyphs themselves: a range over each TEXT node. A range over
          // the element would also report its block-level children's boxes,
          // which run the full width of the hero.
          const rects: DOMRect[] = [];
          const walker = document.createTreeWalker(node, NodeFilter.SHOW_TEXT);
          for (let text = walker.nextNode(); text; text = walker.nextNode()) {
            if (!text.textContent?.trim()) continue;
            const range = document.createRange();
            range.selectNodeContents(text);
            rects.push(...range.getClientRects());
          }
          for (const rect of rects) {
            const disjoint =
              rect.right <= stage.left ||
              stage.right <= rect.left ||
              rect.bottom <= stage.top ||
              stage.bottom <= rect.top;
            if (!disjoint) hits.push(selector);
          }
        }
        return hits;
      });
      expect(overlaps, `width ${String(width)}`).toEqual([]);
    }
    if (isMobile) {
      const link = await page.locator("[data-explore-flow]").boundingBox();
      const viewport = page.viewportSize();
      expect(link && viewport ? link.y + link.height : Infinity).toBeLessThanOrEqual(
        viewport?.height ?? 0,
      );
    }
  });
});

test.describe("the #dedupe set piece", () => {
  /** Scroll the dedupe track to `progress` of its pinned range. */
  const scrubTo = (page: Page, progress: number) =>
    page.evaluate((p) => {
      const track = document.querySelector('[data-scene="dedupe"]');
      if (!track) throw new Error("no dedupe scene");
      const box = track.getBoundingClientRect();
      const top = box.top + window.scrollY;
      const start = top - 64;
      const end = top + box.height - window.innerHeight;
      window.scrollTo({ top: Math.round(start + (end - start) * p), behavior: "instant" });
    }, progress);
  const opacity = (page: Page, selector: string) =>
    page
      .locator(`[data-scene="dedupe"] ${selector}`)
      .first()
      .evaluate((node) => Number(getComputedStyle(node).opacity));

  // The reader's scroll is the playhead: at the start the three records are
  // the picture, a third of the way in the evidence is, and at the end only
  // the golden record is left. Read from computed styles, so the timeline,
  // not a class flip, is what is being tested.
  test("scrubs from three records to one golden record", async ({ page }) => {
    await page.goto("/");
    await scrubTo(page, 0);
    await expect.poll(() => opacity(page, ".dedupe-records")).toBeGreaterThan(0.95);
    expect(await opacity(page, ".dedupe-golden")).toBeLessThan(0.05);

    await scrubTo(page, 0.4);
    await expect.poll(() => opacity(page, ".dedupe-evidence")).toBeGreaterThan(0.95);

    await scrubTo(page, 1);
    await expect.poll(() => opacity(page, ".dedupe-golden")).toBeGreaterThan(0.95);
    expect(await opacity(page, ".dedupe-records")).toBeLessThan(0.05);
    // The stage stays pinned under the header while the track passes.
    const stage = await page.getByTestId("scene-dedupe-stage").boundingBox();
    expect(stage && stage.y).toBeGreaterThanOrEqual(63);
  });

  // Without motion the four moments are a storyboard: all visible, nothing
  // animated, nothing pinned. This is also what the baseline and axe see.
  test("with reduced motion it is a static storyboard", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/");
    const scene = page.locator('[data-scene="dedupe"]');
    for (const moment of ["standardize", "match", "review", "reconcile"])
      await expect(scene.locator(`[data-moment="${moment}"]`)).toBeVisible();
    expect(
      await page
        .getByTestId("scene-dedupe-stage")
        .evaluate((stage) => [
          stage.getAnimations({ subtree: true }).length,
          getComputedStyle(stage).position,
        ]),
    ).toEqual([0, "relative"]);
  });

  // The chapter button walks the set piece one beat at a time, and each glide
  // lands on the frame the scene draws for that beat.
  test("the next-chapter button steps through its beats", async ({ page }) => {
    await page.goto("/");
    // Arrive at the dataset chapter the way the button would leave a reader:
    // its top under the header.
    await page.locator("#dataset").evaluate((node) => {
      window.scrollTo({
        top: node.getBoundingClientRect().top + window.scrollY - 64,
        behavior: "instant",
      });
    });
    const next = page.getByTestId("marketing-next-chapter");
    for (const label of ["Dedupe", "Standardize", "Match"]) {
      await expect(next).toHaveAccessibleName(`Next chapter: ${label}`);
      await next.click();
      await expect(next).not.toHaveAccessibleName(`Next chapter: ${label}`);
    }
    // Landed on Match: its card sits in the middle of the band the beats are
    // read in (below the pinned stage on a phone, beside it on a wide screen),
    // and the stage shows that beat's frame, the evidence.
    const offset = await page.locator("#dedupe-match").evaluate((card) => {
      const stage = document
        .querySelector('[data-scene="dedupe"] .scene-stage-column')
        ?.getBoundingClientRect();
      const box = card.getBoundingClientRect();
      const stacked = stage !== undefined && stage.bottom <= box.top + 1;
      const bandTop = stacked ? stage.bottom : 64;
      return Math.abs(box.top + box.height / 2 - (bandTop + window.innerHeight) / 2);
    });
    expect(offset).toBeLessThan(48);
    await expect.poll(() => opacity(page, ".dedupe-evidence")).toBeGreaterThan(0.9);
    await expect(next).toHaveAccessibleName("Next chapter: Review");
  });
});

test.describe("/features/matching", () => {
  const scrubTo = (page: Page, progress: number) =>
    page.evaluate((p) => {
      const track = document.querySelector('[data-scene="matching"]');
      if (!track) throw new Error("no matching scene");
      const box = track.getBoundingClientRect();
      const top = box.top + window.scrollY;
      const start = top - 64;
      const end = top + box.height - window.innerHeight;
      window.scrollTo({ top: Math.round(start + (end - start) * p), behavior: "instant" });
    }, progress);
  const opacity = (page: Page, selector: string) =>
    page
      .locator(`[data-scene="matching"] ${selector}`)
      .first()
      .evaluate((node) => Number(getComputedStyle(node).opacity));

  // Five beats: the rule is up halfway through, and the score is the last
  // frame, with the rule and evidence gone so it has the frame to itself.
  test("scrubs one pair from its values to its decision", async ({ page }) => {
    await page.goto("/features/matching");
    await scrubTo(page, 0);
    expect(await opacity(page, ".match-decision")).toBeLessThan(0.05);
    await scrubTo(page, 0.5);
    await expect.poll(() => opacity(page, ".match-rule")).toBeGreaterThan(0.95);
    await scrubTo(page, 1);
    await expect.poll(() => opacity(page, ".match-decision")).toBeGreaterThan(0.95);
    expect(await opacity(page, ".match-rule")).toBeLessThan(0.05);
  });

  // The poster: what reduced motion, the axe sweep and this baseline see.
  test("matching scene poster visual baseline @visual", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/features/matching");
    await page.addStyleTag({
      content: '[data-testid="marketing-header"] { visibility: hidden; }',
    });
    await expect(page.getByTestId("scene-matching")).toHaveScreenshot(
      "marketing-matching-scene.png",
    );
  });

  test("with reduced motion the scene is a static storyboard", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/features/matching");
    const scene = page.locator('[data-scene="matching"]');
    for (const moment of ["clean", "rules", "compare", "decide"])
      await expect(scene.locator(`[data-moment="${moment}"]`)).toBeVisible();
    expect(
      await page
        .getByTestId("scene-matching-stage")
        .evaluate((stage) => stage.getAnimations({ subtree: true }).length),
    ).toBe(0);
  });

  // The explorer opens on the scene's pair and moves with the reader's choices,
  // including the case a rule settles whatever the score says.
  test("Try a pair moves the decision, and a rule can overrule the score", async ({ page }) => {
    await page.goto("/features/matching");
    const explorer = page.getByTestId("try-a-pair");
    const verdict = page.getByTestId("try-a-pair-verdict");
    await expect(verdict).toContainText("0.93");
    await expect(verdict).toContainText("a person sees this pair");

    await explorer
      .getByRole("group", { name: "Phone" })
      .getByText("exact", { exact: true })
      .click();
    await expect(verdict).toContainText("join without a person");

    await explorer.getByRole("group", { name: "Account ID" }).getByText("different").click();
    await expect(verdict).toContainText("a rule no score can override");

    await explorer.getByRole("button", { name: "Back to the Meridian pair" }).click();
    await expect(verdict).toContainText("0.93");
  });
});

test.describe("/features/review", () => {
  // The visitor makes the close call: a match joins the pair into E-1042, an
  // unmerge splits it back out with a never-match rule, and the history keeps
  // both steps; "Not a match" keeps them apart from the start.
  test("the reviewer matches, unmerges and keeps a correction", async ({ page }) => {
    await page.goto("/features/review");
    const reviewer = page.getByTestId("reviewer");
    const outcome = page.getByTestId("reviewer-outcome");
    await expect(outcome).toContainText("0.93");

    await reviewer.getByRole("button", { name: "Ask AI for a recommendation" }).click();
    await expect(reviewer.getByText("Recommends: same company.")).toBeVisible();

    await reviewer.getByRole("button", { name: "Match", exact: true }).click();
    await expect(outcome).toContainText("E-1042 now holds all three source records");
    await expect(
      reviewer.getByRole("list", { name: "Entities after reconciliation" }),
    ).toContainText("Salesforce · CSV upload · API");

    await reviewer.getByRole("button", { name: "Unmerge the API record" }).click();
    await expect(outcome).toContainText("the correction stays");
    const history = reviewer.getByRole("list", { name: "History" });
    await expect(history.getByRole("listitem")).toHaveCount(2);
    await expect(history).toContainText("never-match rule");

    await reviewer.getByRole("button", { name: "Put the pair back in the queue" }).click();
    await reviewer.getByRole("button", { name: "Not a match" }).click();
    await expect(outcome).toContainText("stay separate, in this run and every one after it");
  });
});
