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

test.describe("/features/golden-records", () => {
  const scrubTo = (page: Page, progress: number) =>
    page.evaluate((p) => {
      const track = document.querySelector('[data-scene="survivorship"]');
      if (!track) throw new Error("no survivorship scene");
      const box = track.getBoundingClientRect();
      const top = box.top + window.scrollY;
      const start = top - 64;
      const end = top + box.height - window.innerHeight;
      window.scrollTo({ top: Math.round(start + (end - start) * p), behavior: "instant" });
    }, progress);
  const opacity = (page: Page, selector: string) =>
    page
      .locator(`[data-scene="survivorship"] ${selector}`)
      .first()
      .evaluate((node) => Number(getComputedStyle(node).opacity));

  // The golden record fills one field per beat: empty at the start, the email
  // (the first rule) a quarter of the way in, everything at the end.
  test("fills the golden record one field per beat", async ({ page }) => {
    await page.goto("/features/golden-records");
    await scrubTo(page, 0);
    await expect.poll(() => opacity(page, ".surv-sources")).toBeGreaterThan(0.95);
    expect(await opacity(page, '.surv-slot[data-step="1"] .surv-slot-value')).toBeLessThan(0.05);

    await scrubTo(page, 0.25);
    await expect.poll(() => opacity(page, '.surv-candidates[data-step="1"]')).toBeGreaterThan(0.95);
    await expect
      .poll(() => opacity(page, '.surv-slot[data-step="1"] .surv-slot-value'))
      .toBeGreaterThan(0.95);
    expect(await opacity(page, '.surv-slot[data-step="4"] .surv-slot-value')).toBeLessThan(0.05);

    await scrubTo(page, 1);
    await expect
      .poll(() => opacity(page, '.surv-slot[data-step="4"] .surv-slot-value'))
      .toBeGreaterThan(0.95);
    expect(await opacity(page, ".surv-sources")).toBeLessThan(0.05);
  });

  test("with reduced motion it is a static storyboard", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/features/golden-records");
    const scene = page.locator('[data-scene="survivorship"]');
    for (const moment of ["sources", "email", "phone", "name", "address"])
      await expect(scene.locator(`[data-moment="${moment}"]`)).toBeVisible();
    expect(
      await page
        .getByTestId("scene-survivorship-stage")
        .evaluate((stage) => stage.getAnimations({ subtree: true }).length),
    ).toBe(0);
  });

  test("survivorship scene poster visual baseline @visual", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/features/golden-records");
    await page.addStyleTag({
      content: '[data-testid="marketing-header"] { visibility: hidden; }',
    });
    await expect(page.getByTestId("scene-survivorship")).toHaveScreenshot(
      "marketing-survivorship-scene.png",
    );
  });

  // The trace explorer names every source holding a value: two for the phone.
  test("the field trace names the sources behind a value", async ({ page }) => {
    await page.goto("/features/golden-records");
    const trace = page.getByTestId("marketing-provenance");
    await trace.getByRole("button", { name: "Phone" }).click();
    const result = page.getByTestId("provenance-result");
    await expect(result).toContainText("+1 415 555 0142");
    await expect(result).toContainText("Salesforce, CSV upload");
    await expect(result).toContainText("the most common value");
  });

  // The way back out plays as it scrolls into view and ends with every
  // destination and event on screen.
  test("the way back out arrives as it scrolls into view", async ({ page }) => {
    await page.goto("/features/golden-records");
    const figure = page.getByTestId("way-back");
    const shown = () =>
      figure.evaluate((node) =>
        [...node.querySelectorAll(".way-back-destinations li, .way-back-events span")].map((item) =>
          Number(getComputedStyle(item).opacity),
        ),
      );
    expect(Math.max(...(await shown()))).toBeLessThan(0.05);
    await figure.evaluate((node) => {
      node.scrollIntoView({ block: "center" });
    });
    await expect.poll(async () => Math.min(...(await shown()))).toBeGreaterThan(0.95);
  });
});

test.describe("/product", () => {
  const scrubTo = (page: Page, progress: number) =>
    page.evaluate((p) => {
      const track = document.querySelector('[data-scene="workflow"]');
      if (!track) throw new Error("no workflow scene");
      const box = track.getBoundingClientRect();
      const top = box.top + window.scrollY;
      const start = top - 64;
      const end = top + box.height - window.innerHeight;
      window.scrollTo({ top: Math.round(start + (end - start) * p), behavior: "instant" });
    }, progress);
  const opacity = (page: Page, selector: string) =>
    page
      .locator(`[data-scene="workflow"] ${selector}`)
      .first()
      .evaluate((node) => Number(getComputedStyle(node).opacity));
  const panel = (step: number) => `.flow-panel[data-step="${String(step)}"]`;

  // The seven steps are the beats, and the window shows one step at a time:
  // the dataset first, the rules at the midpoint, the record going back out
  // at the end.
  test("the workflow window shows one step per beat", async ({ page }) => {
    await page.goto("/product");
    await expect(page.locator('[data-scene="workflow"] .scene-beat')).toHaveCount(7);

    await scrubTo(page, 0);
    await expect.poll(() => opacity(page, panel(0))).toBeGreaterThan(0.95);
    expect(await opacity(page, panel(6))).toBeLessThan(0.05);

    await scrubTo(page, 0.5);
    await expect.poll(() => opacity(page, panel(3))).toBeGreaterThan(0.95);
    for (const step of [0, 2, 4, 6]) expect(await opacity(page, panel(step))).toBeLessThan(0.05);

    await scrubTo(page, 1);
    await expect.poll(() => opacity(page, panel(6))).toBeGreaterThan(0.95);
    expect(await opacity(page, panel(5))).toBeLessThan(0.05);
  });

  // On a wide stage a rail frames the current step and checks off the
  // finished ones, so the window never reads as empty between cuts.
  test("the step rail follows the window", async ({ page, isMobile }) => {
    test.skip(isMobile, "a phone's stage is too narrow for the rail; each panel's title names it");
    await page.goto("/product");
    const marker = page.locator('[data-scene="workflow"] .flow-rail-marker');
    await expect(marker).toBeVisible();
    const row = () =>
      marker.evaluate(
        (node) =>
          new DOMMatrixReadOnly(getComputedStyle(node).transform).m42 /
          node.getBoundingClientRect().height,
      );
    const check = (step: number) =>
      opacity(page, `.flow-rail li[data-step="${String(step)}"] .flow-rail-check`);

    await scrubTo(page, 0);
    await expect.poll(row).toBeCloseTo(0, 1);
    expect(await check(0)).toBeLessThan(0.05);

    await scrubTo(page, 0.5);
    await expect.poll(row).toBeCloseTo(3, 1);
    expect(await check(2)).toBeGreaterThan(0.95);
    expect(await check(3)).toBeLessThan(0.05);

    await scrubTo(page, 1);
    await expect.poll(row).toBeCloseTo(6, 1);
    expect(await check(5)).toBeGreaterThan(0.95);
  });

  test("with reduced motion the workflow is a static storyboard", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/product");
    const scene = page.locator('[data-scene="workflow"]');
    for (const moment of ["dataset", "sources", "mapping", "rules", "run", "review", "out"])
      await expect(scene.locator(`[data-moment="${moment}"]`)).toBeVisible();
    await expect(scene.locator(".flow-rail")).toBeHidden();
    expect(
      await page
        .getByTestId("scene-workflow-stage")
        .evaluate((stage) => stage.getAnimations({ subtree: true }).length),
    ).toBe(0);
  });

  test("workflow scene poster visual baseline @visual", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/product");
    await page.addStyleTag({
      content: '[data-testid="marketing-header"] { visibility: hidden; }',
    });
    await expect(page.getByTestId("scene-workflow")).toHaveScreenshot(
      "marketing-workflow-scene.png",
    );
  });

  // The problem figure deals one company's three records out from under the
  // middle one. Mid-deal, every record is already opaque, so no record's text
  // shows through another's; by the time the figure is centred, all three
  // are at rest.
  test("the problem figure deals three records out, never translucent", async ({ page }) => {
    await page.goto("/product");
    const figure = page.getByTestId("split-figure");
    const records = () =>
      figure.evaluate((node) =>
        [...node.querySelectorAll(".split-record")].map((record) => {
          const style = getComputedStyle(record);
          const matrix = new DOMMatrixReadOnly(style.transform);
          return {
            opacity: Number(style.opacity),
            moved: Math.hypot(matrix.m41, matrix.m42),
          };
        }),
      );
    await figure.evaluate((node) => {
      const top = node.getBoundingClientRect().top + window.scrollY;
      const span = window.innerHeight + node.getBoundingClientRect().height;
      window.scrollTo({
        top: Math.round(top - window.innerHeight + 0.3 * span),
        behavior: "instant",
      });
    });
    // Wait for the mid-deal frame itself: the middle record arrived, record 0
    // still displaced. Below the fold the records start in their before-state,
    // displaced too, so displacement alone passed on a frame the timeline had
    // not reached yet (WebKit, about one run in two in CI's shard).
    await expect
      .poll(async () => {
        const [first, middle] = await records();
        return Boolean(
          first && middle && middle.opacity === 1 && middle.moved < 0.5 && first.moved > 20,
        );
      })
      .toBe(true);
    for (const record of await records()) expect(record.opacity).toBe(1);

    await figure.evaluate((node) => {
      node.scrollIntoView({ block: "center" });
    });
    await expect
      .poll(async () => Math.max(...(await records()).map((r) => r.moved)))
      .toBeLessThan(0.5);
  });
});

test.describe("/integrations", () => {
  const scrubTo = (page: Page, progress: number) =>
    page.evaluate((p) => {
      const track = document.querySelector('[data-scene="round-trip"]');
      if (!track) throw new Error("no round-trip scene");
      const box = track.getBoundingClientRect();
      const top = box.top + window.scrollY;
      const start = top - 64;
      const end = top + box.height - window.innerHeight;
      window.scrollTo({ top: Math.round(start + (end - start) * p), behavior: "instant" });
    }, progress);
  const opacity = (page: Page, selector: string) =>
    page
      .locator(`[data-scene="round-trip"] ${selector}`)
      .first()
      .evaluate((node) => Number(getComputedStyle(node).opacity));

  // The round trip builds up a beat at a time: the ways in from the first
  // frame, the core by the second beat, the ways out by the third, the
  // Salesforce loop last.
  test("the round trip arrives one beat at a time", async ({ page }) => {
    await page.goto("/integrations");
    await expect(page.locator('[data-scene="round-trip"] .scene-beat')).toHaveCount(4);

    await scrubTo(page, 0);
    await expect.poll(() => opacity(page, ".trip-in li")).toBeGreaterThan(0.95);
    expect(await opacity(page, ".trip-core")).toBeLessThan(0.05);
    expect(await opacity(page, ".trip-out li")).toBeLessThan(0.05);

    await scrubTo(page, 2 / 3);
    await expect.poll(() => opacity(page, '.trip-out li[data-index="3"]')).toBeGreaterThan(0.95);
    expect(await opacity(page, ".trip-core")).toBeGreaterThan(0.95);
    expect(await opacity(page, ".trip-loop")).toBeLessThan(0.05);

    await scrubTo(page, 1);
    await expect.poll(() => opacity(page, ".trip-loop")).toBeGreaterThan(0.95);
  });

  test("with reduced motion the round trip is a static storyboard", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/integrations");
    const scene = page.locator('[data-scene="round-trip"]');
    for (const moment of ["in", "contract", "out", "round-trip"])
      await expect(scene.locator(`[data-moment="${moment}"]`)).toBeVisible();
    expect(
      await page
        .getByTestId("scene-round-trip-stage")
        .evaluate((stage) => stage.getAnimations({ subtree: true }).length),
    ).toBe(0);
  });

  test("round-trip scene poster visual baseline @visual", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/integrations");
    await page.addStyleTag({
      content: '[data-testid="marketing-header"] { visibility: hidden; }',
    });
    await expect(page.getByTestId("scene-round-trip")).toHaveScreenshot(
      "marketing-round-trip-scene.png",
    );
  });
});

test.describe("/docs walkthrough", () => {
  const scrubTo = (page: Page, progress: number) =>
    page.evaluate((p) => {
      const track = document.querySelector('[data-scene="api-storyboard"]');
      if (!track) throw new Error("no walkthrough scene");
      const box = track.getBoundingClientRect();
      const top = box.top + window.scrollY;
      const start = top - 64;
      const end = top + box.height - window.innerHeight;
      window.scrollTo({ top: Math.round(start + (end - start) * p), behavior: "instant" });
    }, progress);
  const answered = (page: Page, exchange: number) =>
    page
      .locator(
        `[data-scene="api-storyboard"] .sb-exchange[data-ex="${String(exchange)}"] .sb-response`,
      )
      .evaluate((node) => Number(getComputedStyle(node).opacity));
  // How much of a request is typed: its cover has moved that share of its width.
  const typed = (page: Page, exchange: number) =>
    page
      .locator(
        `[data-scene="api-storyboard"] .sb-exchange[data-ex="${String(exchange)}"] .sb-cover`,
      )
      .evaluate(
        (node) =>
          new DOMMatrixReadOnly(getComputedStyle(node).transform).m41 /
          node.getBoundingClientRect().width,
      );

  // One call after another: each beat's calls are typed and answered by its
  // centre, and the next beat's are not started.
  test("types one call after another, answered by each beat's centre", async ({ page }) => {
    await page.goto("/docs");
    await expect(page.locator('[data-scene="api-storyboard"] .scene-beat')).toHaveCount(6);

    await scrubTo(page, 0);
    await expect.poll(() => answered(page, 0)).toBeGreaterThan(0.95);
    expect(await answered(page, 1)).toBeLessThan(0.05);
    expect(await typed(page, 1)).toBeLessThan(0.05);

    await scrubTo(page, 0.4);
    await expect.poll(() => answered(page, 2)).toBeGreaterThan(0.95);
    expect(await typed(page, 2)).toBeGreaterThan(0.95);
    expect(await typed(page, 3)).toBeLessThan(0.05);

    await scrubTo(page, 1);
    await expect.poll(() => answered(page, 8)).toBeGreaterThan(0.95);
    const scrolled = await page
      .locator('[data-scene="api-storyboard"] .sb-transcript')
      .evaluate((node) => new DOMMatrixReadOnly(getComputedStyle(node).transform).m42);
    expect(scrolled).toBeLessThan(0);
  });

  // Each value a response returns is outlined again where the next call uses
  // it: the key in the header, then the name, version, job and entity in the
  // path.
  test("outlines each value where it is returned and where it is used", async ({ page }) => {
    await page.goto("/docs");
    const marks = await page
      .locator('[data-scene="api-storyboard"] .sb-carry')
      .evaluateAll((nodes) => nodes.map((node) => node.textContent.replaceAll('"', "")));
    expect(marks).toHaveLength(10);
    for (let index = 0; index < marks.length; index += 2) {
      expect(marks[index + 1], `carry ${String(index / 2)}`).toBe(marks[index]);
    }
  });

  test("with reduced motion the walkthrough is a static transcript", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/docs");
    const exchanges = page.locator('[data-scene="api-storyboard"] .sb-exchange');
    await expect(exchanges).toHaveCount(9);
    for (const exchange of await exchanges.all()) await expect(exchange).toBeVisible();
    expect(
      await page
        .getByTestId("scene-api-storyboard-stage")
        .evaluate((stage) => stage.getAnimations({ subtree: true }).length),
    ).toBe(0);
  });

  test("walkthrough poster visual baseline @visual", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/docs");
    await page.addStyleTag({
      content: '[data-testid="marketing-header"] { visibility: hidden !important; }',
    });
    await expect(page.getByTestId("scene-api-storyboard")).toHaveScreenshot(
      "marketing-api-storyboard.png",
    );
  });
});

test.describe("/pricing", () => {
  const share = (page: Page, selector: string) =>
    page
      .locator(selector)
      .first()
      .evaluate((node) => {
        const style = getComputedStyle(node);
        const track = node.parentElement?.getBoundingClientRect().width ?? 0;
        return {
          scale: new DOMMatrixReadOnly(style.transform).a,
          fullWidth: Math.abs(node.getBoundingClientRect().width / (track || 1)),
          layoutWidth: Number.parseFloat(style.width) / (track || 1),
        };
      });

  // A bar is the track's full width, scaled to its share, so an input change
  // eases it by transform instead of resizing it.
  test("the bars scale to their share, never resize", async ({ page }) => {
    await page.goto("/pricing");
    const bar = '[data-testid="pricing-option-monthly"] .pricing-bar';
    await expect.poll(async () => (await share(page, bar)).scale).toBeCloseTo(1, 2);
    expect((await share(page, bar)).layoutWidth).toBeCloseTo(1, 2);
    const cheapest = '[data-testid="pricing-option-threeYear"] .pricing-bar';
    await expect.poll(async () => (await share(page, cheapest)).scale).toBeLessThan(1);
    expect((await share(page, cheapest)).layoutWidth).toBeCloseTo(1, 2);
  });

  // The headline price rolls through the kit's Roll and settles on the new
  // figure; screen readers get the plain value at once.
  test("the headline price rolls to its new value", async ({ page }) => {
    await page.goto("/pricing");
    const headline = page.locator(".pricing-list-price strong");
    await expect(headline.locator(".ss-roll")).toHaveCount(1);
    const records = page.getByTestId("pricing-records");
    await records.fill("200,000");
    await expect(headline.locator(".sr-only")).toHaveText("$1,875");
    await expect(headline.locator("[aria-hidden='true']")).toHaveText("$1,875");
  });

  // The figure sits in the first screen, so it plays itself once when first
  // seen and Replay runs it again: three systems' bars start stacked and
  // finish end to end in one bar, with the total above and the marker on its
  // tier.
  test("the systems' records slide into one total on one tier", async ({ page }) => {
    await page.goto("/pricing");
    const figure = page.getByTestId("records-pool");
    const state = () =>
      figure.evaluate((node) => ({
        moved: [...node.querySelectorAll(".pool-seg")].map((seg) => {
          const matrix = new DOMMatrixReadOnly(getComputedStyle(seg).transform);
          return Math.hypot(matrix.m41, matrix.m42);
        }),
        total: Number(getComputedStyle(node.querySelector(".pool-total") as Element).opacity),
      }));
    await figure.scrollIntoViewIfNeeded();
    await expect.poll(async () => Math.max(...(await state()).moved)).toBeLessThan(0.5);
    expect((await state()).total).toBeGreaterThan(0.95);
    await expect(figure.locator('.pool-tiers li[data-on="true"]')).toHaveText("Professional");

    await page.getByTestId("records-pool-stage-replay").click();
    await expect
      .poll(async () => Math.min(...(await state()).moved.slice(0, 2)))
      .toBeGreaterThan(20);
    await expect.poll(async () => Math.max(...(await state()).moved)).toBeLessThan(0.5);
    expect((await state()).total).toBeGreaterThan(0.95);
  });

  test("with reduced motion the figure is its finished picture", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/pricing");
    const figure = page.getByTestId("records-pool");
    expect(await figure.evaluate((node) => node.getAnimations({ subtree: true }).length)).toBe(0);
    await expect(figure.locator(".pool-total")).toBeVisible();
  });

  test("records figure poster visual baseline @visual", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/pricing");
    await page.addStyleTag({
      content: '[data-testid="marketing-header"] { visibility: hidden !important; }',
    });
    await expect(page.getByTestId("records-pool")).toHaveScreenshot("marketing-records-pool.png");
  });
});

test.describe("solutions, role, industry and /about scenes", () => {
  const golden = (page: Page) =>
    page
      .getByTestId("resolution-figure")
      .locator(".resolve-golden")
      .evaluate((node) => Number(getComputedStyle(node).opacity));

  // Under each page intro, in the first screen: it plays itself once when
  // seen, and Replay runs it again, from the three records alone to the one
  // they resolve into.
  test("three records resolve into one, and Replay runs it again", async ({ page }) => {
    await page.goto("/solutions/industries/manufacturing");
    const figure = page.getByTestId("resolution-figure");
    await figure.scrollIntoViewIfNeeded();
    await expect.poll(() => golden(page)).toBeGreaterThan(0.95);
    await expect(figure.locator(".resolve-golden")).toContainText("Kestrel Fasteners");
    await page.getByTestId("resolve-manufacturing-stage-replay").click();
    await expect.poll(() => golden(page)).toBeLessThan(0.05);
    await expect.poll(() => golden(page)).toBeGreaterThan(0.95);
  });

  test("with reduced motion every scene is its finished picture", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    for (const path of ["/solutions", "/about", "/solutions/roles/marketing"]) {
      await page.goto(path);
      const figure = page.getByTestId("resolution-figure");
      await expect(figure.locator(".resolve-golden")).toBeVisible();
      expect(
        await figure.evaluate((node) => node.getAnimations({ subtree: true }).length),
        path,
      ).toBe(0);
    }
  });

  test("resolution figure poster visual baseline @visual", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/solutions/industries/manufacturing");
    await page.addStyleTag({
      content: '[data-testid="marketing-header"] { visibility: hidden !important; }',
    });
    await expect(page.getByTestId("resolution-figure")).toHaveScreenshot(
      "marketing-resolution-figure.png",
    );
  });
});
