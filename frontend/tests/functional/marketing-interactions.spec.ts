import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

test.describe("interactive marketing", () => {
  test("sources can feed multiple datasets without reframing the page", async ({ page }) => {
    await page.goto("/");
    const demo = page.getByTestId("pipeline-demo");
    await expect(
      demo.getByText("An animated pipeline · fictional sample data", { exact: true }),
    ).toHaveCount(0);
    await expect(demo.getByRole("button", { name: "Organizations", exact: true })).toHaveCount(0);
    await expect(demo.locator(".dz-source-node")).toHaveCount(6);
    const choices = demo.getByRole("group", { name: "Example dataset" });
    await choices.scrollIntoViewIfNeeded();
    const geometry = () =>
      demo.evaluate((e) => {
        const r = e.getBoundingClientRect();
        return { scroll: window.scrollY, height: r.height, width: r.width };
      });
    const before = await geometry();
    const activeSources = () =>
      demo
        .locator('.dz-wide [data-source][data-active="true"]')
        .evaluateAll((nodes) => nodes.map((n) => n.getAttribute("data-source")));
    await choices.getByRole("button", { name: "Operations", exact: true }).click();
    await expect.poll(activeSources).toEqual(["Google Sheets", "SFTP", "S3 files"]);
    expect(await geometry()).toEqual(before);
    await choices.getByRole("button", { name: "Shared index", exact: true }).click();
    await expect
      .poll(activeSources)
      .toEqual(["Google Sheets", "Salesforce", "SFTP", "S3 files", "Shopify", "Custom API"]);
    expect(await geometry()).toEqual(before);
    await choices.getByRole("button", { name: "Commerce", exact: true }).click();
    await expect
      .poll(activeSources)
      .toEqual(["Google Sheets", "Salesforce", "S3 files", "Shopify"]);
    expect(await geometry()).toEqual(before);
  });

  test("dense, faster streams pause and resume in place", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "no-preference" });
    await page.goto("/");
    const demo = page.getByTestId("pipeline-demo");
    const clock = () =>
      demo
        .locator("[data-packet]")
        .first()
        .evaluate((e) => Number(e.getAnimations()[0]?.currentTime ?? 0));
    await expect.poll(clock).toBeGreaterThan(100);
    await demo.getByRole("button", { name: "Pause pipeline" }).click();
    await expect
      .poll(() =>
        demo
          .locator("[data-packet]")
          .first()
          .evaluate((e) => e.getAnimations()[0]?.playState),
      )
      .toBe("paused");
    // WebKit exposes playState="paused" before its pending pause task settles.
    await demo
      .locator("[data-packet]")
      .first()
      .evaluate(async (e) => {
        await e.getAnimations()[0]?.ready;
      });
    const stopped = await clock();
    await page.waitForTimeout(300);
    expect(Math.abs((await clock()) - stopped)).toBeLessThan(40);
    await demo.getByRole("button", { name: "Resume pipeline" }).click();
    await expect.poll(clock).toBeGreaterThan(stopped + 100);
    expect(await demo.locator(".dz-wide [data-packet]").count()).toBeGreaterThan(100);
    expect(
      await demo
        .locator("[data-packet]")
        .first()
        .evaluate((e) => e.getAnimations()[0]?.effect?.getTiming().duration),
    ).toBe(6000);
  });

  test("a moving record opens its decision by double click or touch", async ({
    page,
    isMobile,
  }) => {
    await page.emulateMedia({ reducedMotion: "no-preference" });
    await page.goto("/");
    const demo = page.getByTestId("pipeline-demo");
    await demo.locator(".dz-resolve-node").scrollIntoViewIfNeeded();
    const findRecord = () =>
      page.evaluate(() => {
        for (const node of document.querySelectorAll<SVGGElement>(
          '[data-testid="pipeline-demo"] [data-packet][data-kind="matched"][role="button"]',
        )) {
          const r = node.getBoundingClientRect();
          if (
            Number(getComputedStyle(node).opacity) > 0.8 &&
            r.width > 0 &&
            r.left >= 0 &&
            r.right < innerWidth &&
            r.top > 80 &&
            r.bottom < innerHeight - 20 &&
            document
              .elementFromPoint(r.x + r.width / 2, r.y + r.height / 2)
              ?.closest("[data-packet]") === node
          )
            return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
        }
        return null;
      });
    await expect.poll(findRecord).not.toBeNull();
    const point = await findRecord();
    if (!point) throw new Error("No visible record to inspect");
    if (isMobile) await page.touchscreen.tap(point.x, point.y);
    else {
      await page.mouse.move(point.x, point.y);
      await page.mouse.dblclick(point.x, point.y);
    }
    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText("example decision");
    await expect(dialog.getByText("Before", { exact: true })).toBeVisible();
    await expect(dialog.getByText("After", { exact: true })).toBeVisible();
    await dialog.getByRole("button", { name: "Close", exact: true }).click();
    await expect(dialog).toHaveCount(0);
    await expect(demo.getByRole("button", { name: "Inspect a record", exact: true })).toBeFocused();
  });

  test("auditing diverts a consolidated record and returns two separate records", async ({
    page,
  }) => {
    await page.emulateMedia({ reducedMotion: "no-preference" });
    await page.goto("/");
    const demo = page.getByTestId("pipeline-demo");
    await demo.getByRole("button", { name: "Inspect a record", exact: true }).click();
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "Audit & unmerge", exact: true })
      .click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(demo).toHaveAttribute("data-audit", "auditing");
    await expect(demo).toHaveAttribute("data-audit", "returning", { timeout: 4000 });
    await expect(
      demo.locator('.dz-wide .dz-audit-path [data-kind="alone"][data-packet]'),
    ).toHaveCount(2);
    await expect(demo).toHaveAttribute("data-audit", "complete", { timeout: 5000 });
    await expect(demo.getByRole("status")).toContainText("correction is retained");
  });

  test("review policies gate reconciliation and an unmerge separates the entity", async ({
    page,
  }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/");
    const review = page.getByTestId("marketing-review-flow");
    await review.getByRole("button", { name: /Human review/ }).click();
    await expect(review.locator("figure")).toHaveAttribute("data-status", "pending");
    await expect(review.getByTestId("review-flow-status")).toContainText(
      "Your decision comes next",
    );
    await review.getByRole("button", { name: "Approve sample match" }).click();
    await expect(review.locator("figure")).toHaveAttribute("data-status", "merged");
    await review.getByRole("button", { name: "Unmerge sample record" }).click();
    await expect(review.getByTestId("review-flow-status")).toContainText("never-match assertion");
    await expect(review.locator(".dz-review-result")).toHaveAttribute("data-split", "true");
    await expect(review.locator(".dz-result-heading")).toHaveText("2 distinct entities");
    await review.getByRole("button", { name: "Reset example" }).click();
    await expect(review.locator("figure")).toHaveAttribute("data-status", "merged");
    await review.getByRole("button", { name: /AI-assisted review/ }).click();
    await expect(review.locator("figure")).toHaveAttribute("data-status", "pending");
    await expect(review.getByTestId("review-flow-status")).toContainText("AI recommends");
    await review.getByRole("button", { name: "Accept recommendation" }).click();
    await expect(review.locator("figure")).toHaveAttribute("data-status", "merged");
    await review.getByRole("button", { name: /Use my config/ }).click();
    await expect(review.locator("figure")).toHaveAttribute("data-status", "merged");
  });

  test("field selection changes the value, provenance, and rule", async ({ page }) => {
    await page.goto("/");
    const explorer = page.getByTestId("marketing-provenance");
    await explorer.getByRole("button", { name: "Domain", exact: true }).click();
    const result = page.getByTestId("provenance-result");
    await expect(result).toContainText("northstar.example");
    await expect(result).toContainText("Database");
    await expect(result).toContainText("First non-empty");
    await explorer.getByRole("button", { name: "Address", exact: true }).focus();
    await page.keyboard.press("Enter");
    await expect(result).toContainText("18 Mercer Street");
    await expect(result).toContainText("Spreadsheet");
  });

  test("reduced motion shows complete records without running packet animations", async ({
    page,
  }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/");
    await expect(page.getByRole("button", { name: "Pause pipeline" })).toBeDisabled();
    const demo = page.getByTestId("pipeline-demo");
    await expect(demo).toHaveAttribute("data-still", "true");
    await demo.getByRole("button", { name: "Inspect alone record", exact: true }).click();
    await expect(page.getByRole("dialog")).toContainText("No match is also a valid result");
    await expect(
      page.getByRole("dialog").getByRole("button", { name: "Audit & unmerge", exact: true }),
    ).toBeDisabled();
    expect(
      await page
        .locator("[data-packet]")
        .evaluateAll((elements) => elements.every((e) => e.getAnimations().length === 0)),
    ).toBe(true);
  });

  test("the static graph, copy, FAQ and app link work without JavaScript", async ({ browser }) => {
    const context = await browser.newContext({ javaScriptEnabled: false });
    try {
      const page = await context.newPage();
      await page.goto("http://localhost:3100/");
      await expect(page.getByRole("heading", { level: 1 })).toContainText("you can defend.");
      await expect(page.getByTestId("flow-caption")).toContainText("Commerce");
      await expect(page.locator(".dz-golden-node")).toContainText("Golden records");
      await page.getByTestId("marketing-faq").locator("summary").first().click();
      await expect(page.getByTestId("marketing-faq").locator("details").first()).toHaveAttribute(
        "open",
        "",
      );
      await expect(page.getByTestId("marketing-cta-hero")).toHaveAttribute("href", "/app");
    } finally {
      await context.close();
    }
  });

  for (const scheme of ["light", "dark"] as const) {
    test(`interactive controls remain accessible in ${scheme}, including mobile`, async ({
      page,
    }) => {
      await page.emulateMedia({ reducedMotion: "reduce", colorScheme: scheme });
      await page.goto("/");
      await page
        .getByRole("group", { name: "Example dataset" })
        .getByRole("button", { name: "Operations", exact: true })
        .click();
      await page.getByRole("button", { name: /AI-assisted review/ }).click();
      const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
      expect(
        results.violations.filter((v) => v.impact === "serious" || v.impact === "critical"),
      ).toEqual([]);
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
      ).toBe(true);
    });
  }

  test("record decision is keyboard accessible @visual", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/");
    const trigger = page.getByRole("button", { name: "Inspect matched record", exact: true });
    await expect(trigger).toBeEnabled();
    await trigger.focus();
    await page.keyboard.press("Enter");
    const dialog = page.getByRole("dialog");
    await expect(dialog).toHaveScreenshot("marketing-record-decision.png");
    const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
    expect(
      results.violations.filter((v) => v.impact === "serious" || v.impact === "critical"),
    ).toEqual([]);
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
  });

  test("field provenance visual baseline @visual", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/");
    await expect(page.getByTestId("marketing-provenance")).toHaveScreenshot(
      "marketing-provenance.png",
    );
  });

  test("review and unmerge visual baseline @visual", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/");
    const review = page.getByTestId("marketing-review-flow");
    await review.getByRole("button", { name: "Unmerge sample record" }).click();
    await expect(review).toHaveScreenshot("marketing-unmerge.png");
  });
});
