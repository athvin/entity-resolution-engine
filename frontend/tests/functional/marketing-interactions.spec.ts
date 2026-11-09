import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

test.describe("interactive marketing", () => {
  test("the quiet entrance links to the flow and directly into the audit", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByTestId("marketing-hero").locator("svg.story-wires")).toHaveCount(0);
    await page.getByRole("link", { name: "Explore the flow" }).click();
    await expect(page).toHaveURL(/#flow$/);
    await page.goto("/");
    await page.getByRole("link", { name: "See how audit works", exact: true }).click();
    await expect(page).toHaveURL(/#audit$/);
    await expect(page.getByTestId("marketing-audit")).toHaveAttribute("data-running", "true");
  });

  test("source choices persist per dataset and stay synchronized with usage", async ({ page }) => {
    await page.goto("/#flow");
    const flow = page.getByTestId("marketing-flow");
    await flow.getByRole("button", { name: /ZoomInfo/ }).click();
    await expect(flow.locator('[data-node="source-6"]')).toBeVisible();
    // WebKit native selects ignore min-height; measure the actual touch target.
    for (const id of ["#flow-dataset", "#usage-dataset"]) {
      expect(
        await page.locator(id).evaluate((el) => el.getBoundingClientRect().height),
      ).toBeGreaterThanOrEqual(44);
    }
    await page.locator("#flow-dataset").selectOption("1");
    await expect(flow.locator('[data-node="source-6"]')).toHaveCount(0);
    await page.locator("#flow-dataset").selectOption("0");
    await expect(flow.getByRole("button", { name: /ZoomInfo/ })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await page.locator("#flow-volume").fill("100");
    await expect(page.locator("#usage-volume")).toHaveValue("100");
    await expect(page.getByTestId("marketing-usage")).toContainText("10,000,000");
    await page.locator("#usage-dataset").selectOption("1");
    await expect(page.locator("#flow-dataset")).toHaveValue("1");
    await expect(page.getByTestId("usage-flow")).toContainText("Operations");
  });

  test("volume scrubs continuously on a quadratic scale with keyboard control", async ({
    page,
  }) => {
    await page.goto("/#flow");
    const slider = page.locator("#flow-volume");
    await slider.fill("50");
    await expect(slider).toHaveAttribute("aria-valuetext", "2,500,750 records per run");
    await expect(page.locator("#usage-volume")).toHaveValue("50");
    await slider.fill("50.25");
    await expect(slider).toHaveAttribute("aria-valuetext", "2,525,810 records per run");
    await slider.focus();
    await page.keyboard.press("ArrowRight");
    expect(Number(await slider.inputValue())).toBeGreaterThan(50.25);
    await page.keyboard.press("Home");
    await expect(slider).toHaveAttribute("aria-valuetext", "1,000 records per run");
    await page.keyboard.press("End");
    await expect(slider).toHaveAttribute("aria-valuetext", "10,000,000 records per run");
    await page.locator("#usage-volume").fill("25");
    await expect(slider).toHaveValue("25");
    await expect(slider).toHaveAttribute("aria-valuetext", "625,938 records per run");
  });

  test("dark is the default and the theme toggle remembers an explicit choice", async ({
    page,
  }) => {
    await page.emulateMedia({ colorScheme: "light" });
    await page.goto("/");
    await expect(page.locator("html")).toHaveClass("dark");
    await page.getByRole("button", { name: "Switch to light mode" }).click();
    await expect(page.locator("html")).toHaveClass("light");
    await page.reload();
    await expect(page.locator("html")).toHaveClass("light");
    await page.getByRole("link", { name: "Docs", exact: true }).click();
    await expect(page.locator("html")).toHaveClass("light");
    await page.getByRole("button", { name: "Switch to dark mode" }).click();
    await expect(page.locator("html")).toHaveClass("dark");
  });

  test("a partial swipe settles onto the next chapter", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByRole("button", { name: "Switch to light mode" })).toBeEnabled();
    await page.locator("#flow").evaluate((node) => {
      const destination = node.getBoundingClientRect().top + scrollY - 64;
      window.scrollTo({ top: destination * 0.65, behavior: "instant" });
    });
    await expect
      .poll(() =>
        page.locator("#flow").evaluate((node) => Math.abs(node.getBoundingClientRect().top - 64)),
      )
      .toBeLessThan(2);
  });

  test("a short touch swipe settles on the processing diagram", async ({
    page,
    browserName,
    isMobile,
  }) => {
    test.skip(
      browserName !== "chromium" || !isMobile,
      "CDP dispatches real touch input in Chromium",
    );
    await page.setViewportSize({ width: 375, height: 667 });
    await page.goto("/#dedupe");
    await expect(page.getByRole("button", { name: "Pause pipeline" })).toBeEnabled();
    const input = await page.context().newCDPSession(page);
    await input.send("Input.dispatchTouchEvent", {
      type: "touchStart",
      touchPoints: [{ x: 180, y: 560 }],
    });
    for (let step = 1; step <= 8; step++) {
      await input.send("Input.dispatchTouchEvent", {
        type: "touchMove",
        touchPoints: [{ x: 180, y: 560 - step * 20 }],
      });
      await page.waitForTimeout(16);
    }
    await input.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
    await expect
      .poll(() =>
        page
          .getByTestId("trail-dedupe")
          .evaluate((node) => Math.abs(node.getBoundingClientRect().top - 64)),
      )
      .toBeLessThan(2);
  });

  test("narrow phones fit the viewport and visibly move records", async ({ page }) => {
    await page.setViewportSize({ width: 320, height: 568 });
    await page.goto("/#dedupe");
    await expect(page.getByRole("button", { name: "Pause pipeline" })).toBeEnabled();
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(320);
    const diagram = page.getByTestId("trail-dedupe");
    await diagram.evaluate((node) => {
      node.scrollIntoView();
    });
    const packet = diagram.locator(".story-wires:not(.story-transmission) [data-packet]").first();
    const position = () =>
      packet.evaluate((node) => {
        const rect = node.getBoundingClientRect();
        const parent = node.closest(".trail-diagram")?.getBoundingClientRect();
        return { x: rect.x - (parent?.x ?? 0), y: rect.y - (parent?.y ?? 0), width: rect.width };
      });
    await expect
      .poll(() => packet.evaluate((node) => node.getAnimations()[0]?.playState))
      .toBe("running");
    const before = await position();
    expect(before.width).toBeGreaterThan(5);
    await expect
      .poll(async () => {
        const after = await position();
        return Math.hypot(after.x - before.x, after.y - before.y);
      })
      .toBeGreaterThan(3);
  });

  test("tapping the theme control preserves the current chapter", async ({ page, isMobile }) => {
    await page.goto("/#dedupe");
    const toggle = page.getByRole("button", { name: "Switch to light mode" });
    await expect(toggle).toBeEnabled();
    await expect
      .poll(() =>
        page.locator("#dedupe").evaluate((node) => Math.abs(node.getBoundingClientRect().top - 64)),
      )
      .toBeLessThan(2);
    const scroll = await page.evaluate(() => scrollY);
    const box = await toggle.boundingBox();
    if (!box) throw new Error("Theme toggle is not rendered");
    // Locator.click scrolls a sticky header into view before pressing it. Use
    // a real pointer location so the test measures the visitor's tap instead.
    if (isMobile) await page.touchscreen.tap(box.x + box.width / 2, box.y + box.height / 2);
    else await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
    await expect(page.locator("html")).toHaveClass("light");
    await page.waitForTimeout(500);
    expect(Math.abs((await page.evaluate(() => scrollY)) - scroll)).toBeLessThan(2);
  });

  test("the footer stays reachable after scrolling and opening the FAQ", async ({
    page,
    isMobile,
  }) => {
    await page.goto("/");
    const bottomGap = () =>
      page.evaluate(() => document.documentElement.scrollHeight - innerHeight - scrollY);
    await page.evaluate(() => {
      window.scrollTo({ top: document.documentElement.scrollHeight, behavior: "instant" });
    });
    await expect.poll(bottomGap).toBeLessThan(3);
    await page.waitForTimeout(800);
    expect(await bottomGap()).toBeLessThan(3);
    await page.getByTestId("marketing-faq").locator("summary").last().click();
    await page.evaluate(() => {
      window.scrollTo({ top: document.documentElement.scrollHeight, behavior: "instant" });
    });
    if (!isMobile) await page.mouse.wheel(0, 600);
    await page.waitForTimeout(800);
    expect(await bottomGap()).toBeLessThan(3);
    await expect(
      page.getByTestId("marketing-footer").getByRole("link", { name: "Sign in" }),
    ).toBeInViewport();
  });

  test("removing every source stops the stream and explains the next action", async ({ page }) => {
    await page.goto("/#flow");
    const flow = page.getByTestId("marketing-flow");
    for (const name of ["Google Sheets", "Salesforce", "S3 files", "Shopify"]) {
      await flow.getByRole("button", { name: new RegExp(name) }).click();
    }
    await expect(flow).toContainText("Choose a source above to start the flow.");
    await expect(page.getByTestId("pipeline-demo").locator("[data-packet]")).toHaveCount(0);
    await expect(page.getByTestId("trail-bridges").locator("[data-packet]")).toHaveCount(0);
    await expect(page.getByTestId("usage-flow")).toContainText("Choose a source to start");
    await flow.getByRole("button", { name: /Custom API/ }).click();
    await expect(flow.locator('[data-node="source-5"]')).toBeVisible();
  });

  test("connectors meet their nodes after source changes and resizing", async ({ page }) => {
    await page.goto("/#flow");
    await page
      .getByTestId("marketing-flow")
      .getByRole("button", { name: /ZoomInfo/ })
      .click();
    const aligned = () =>
      page.getByTestId("trail-ingress").evaluate((root) => {
        const graph = root.getBoundingClientRect();
        return Array.from(
          root.querySelectorAll<SVGPathElement>('[data-source^="source-"] [data-flow-path]'),
        ).every((path) => {
          const id = path.closest("[data-source]")?.getAttribute("data-source");
          const node = root.querySelector(`[data-node="${id ?? ""}"]`)?.getBoundingClientRect();
          if (!node) return false;
          const p = path.getPointAtLength(0);
          return (
            Math.abs(p.x - (node.x + node.width / 2 - graph.x)) < 1 &&
            Math.abs(p.y - (node.bottom - graph.y)) < 1
          );
        });
      });
    await expect.poll(aligned).toBe(true);
    await page.setViewportSize({ width: 760, height: 900 });
    await expect.poll(aligned).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
      true,
    );
  });

  test("the trail crosses section boundaries and every next step is reachable", async ({
    page,
  }) => {
    await page.goto("/#flow");
    const bridges = page.getByTestId("trail-bridges");
    await expect(
      bridges.locator(":scope > .story-wires:not(.story-transmission) [data-flow-path]"),
    ).toHaveCount(5);
    for (const [id, next] of [
      ["flow", "dataset"],
      ["dataset", "dedupe"],
      ["dedupe", "egress"],
      ["egress", "audit"],
    ]) {
      await page.locator(`#${id ?? ""} .story-next`).click();
      await expect(page).toHaveURL(new RegExp(`#${next ?? ""}$`));
      await expect
        .poll(() =>
          page
            .locator(`#${next ?? ""}`)
            .evaluate((e) => Math.abs(e.getBoundingClientRect().top - 64)),
        )
        .toBeLessThan(3);
    }
    await page.locator("#usage .story-next").click();
    await expect(page).toHaveURL(/#questions$/);
    await page.getByTestId("marketing-faq").locator("summary").first().click();
    await expect(page.getByTestId("marketing-faq").locator("details").first()).toHaveAttribute(
      "open",
      "",
    );
  });

  test("pipeline playback pauses and resumes without resetting", async ({ page }) => {
    await page.goto("/#flow");
    const demo = page.getByTestId("pipeline-demo");
    await page.locator("#dedupe").scrollIntoViewIfNeeded();
    const packet = page.getByTestId("trail-dedupe").locator("[data-packet]").first();
    const clock = () => packet.evaluate((e) => Number(e.getAnimations()[0]?.currentTime ?? 0));
    await expect.poll(clock).toBeGreaterThan(100);
    await demo.getByRole("button", { name: "Pause pipeline" }).click();
    await packet.evaluate(async (e) => {
      await e.getAnimations()[0]?.ready;
    });
    const stopped = await clock();
    await page.waitForTimeout(250);
    expect(Math.abs((await clock()) - stopped)).toBeLessThan(40);
    await demo.getByRole("button", { name: "Resume pipeline" }).click();
    await expect.poll(clock).toBeGreaterThan(stopped + 100);
  });

  test("offscreen trails stop and transmitted light follows the same record", async ({ page }) => {
    await page.goto("/#dedupe");
    const normal = page
      .getByTestId("trail-dedupe")
      .locator(".story-wires:not(.story-transmission) [data-packet]")
      .first();
    const glow = page
      .getByTestId("trail-dedupe")
      .locator(".story-transmission [data-packet]")
      .first();
    await expect
      .poll(() => normal.evaluate((node) => Number(node.getAnimations()[0]?.currentTime ?? 0)))
      .toBeGreaterThan(100);
    const offscreen = page.getByTestId("trail-bridges").locator("[data-packet]").first();
    await expect
      .poll(() => offscreen.evaluate((node) => node.getAnimations()[0]?.playState))
      .toBe("paused");
    await page.getByRole("button", { name: "Pause pipeline" }).click();
    await normal.evaluate(async (node) => {
      await node.getAnimations()[0]?.ready;
    });
    const recordTime = await normal.evaluate((node) =>
      Number(node.getAnimations()[0]?.currentTime),
    );
    const lightTime = await glow.evaluate((node) => Number(node.getAnimations()[0]?.currentTime));
    expect(Math.abs(recordTime - lightTime)).toBeLessThan(1);
    // Read both boxes in one frame: anchor scrolling can still be settling
    // even though packet playback is paused.
    const separation = await normal.evaluate((node) => {
      const echo = node
        .closest(".trail-diagram")
        ?.querySelector(".story-transmission [data-packet]");
      if (!echo) throw new Error("Transmitted light is missing");
      const record = node.getBoundingClientRect();
      const light = echo.getBoundingClientRect();
      return {
        x: Math.abs(record.x + record.width / 2 - light.x - light.width / 2),
        y: Math.abs(record.y + record.height / 2 - light.y - light.height / 2),
      };
    });
    expect(separation.x).toBeLessThan(1);
    expect(separation.y).toBeLessThan(1);
  });

  test("audit holds its clock while paused and retains the corrected result", async ({ page }) => {
    await page.goto("/#audit");
    const audit = page.getByTestId("marketing-audit");
    await expect(audit).toHaveAttribute("data-running", "true");
    await audit.getByRole("button", { name: "Pause audit" }).click();
    await expect(audit.getByRole("button", { name: "Resume audit" })).toBeVisible();
    await expect(page).toHaveURL(/#audit$/);
    const clock = () =>
      audit
        .locator(".story-audit-clock")
        .evaluate((e) => Number(e.getAnimations()[0]?.currentTime ?? 0));
    await audit.locator(".story-audit-clock").evaluate(async (e) => {
      await e.getAnimations()[0]?.ready;
    });
    const time = await clock();
    await page.waitForTimeout(250);
    expect(Math.abs((await clock()) - time)).toBeLessThan(40);
    await expect(page.locator("#flow-dataset")).toBeDisabled();
    await audit.getByRole("button", { name: "Resume audit" }).click();
    await expect(audit).toHaveAttribute("data-phase", "4", { timeout: 20000 });
    await expect(audit.getByRole("status")).toContainText("Two entities. Correction recorded.");
    await expect(page.locator("#flow-dataset")).toBeEnabled();
    await audit.getByRole("button", { name: "Replay audit" }).click();
    await expect(audit).toHaveAttribute("data-phase", "0");
  });

  test("record evidence is keyboard accessible @visual", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/#flow");
    const inspect = page.getByRole("button", { name: "Inspect a record" });
    await expect(inspect).toBeEnabled();
    await inspect.focus();
    await page.keyboard.press("Enter");
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText("ACCT-042");
    await expect(dialog).toHaveScreenshot("marketing-record-decision.png");
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
  });

  test("reduced motion preserves the explanation and has no packet animations", async ({
    page,
  }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/#audit");
    await expect(page.getByTestId("marketing-audit")).toHaveAttribute("data-phase", "4");
    expect(
      await page
        .locator("[data-packet]")
        .evaluateAll((nodes) => nodes.flatMap((n) => n.getAnimations()).length),
    ).toBe(0);
    await expect(page.getByTestId("marketing-audit")).toContainText("Two entities.");
  });

  test("the story and docs remain meaningful without JavaScript", async ({ browser }) => {
    const context = await browser.newContext({ javaScriptEnabled: false });
    const page = await context.newPage();
    await page.goto("/");
    await expect(page.getByRole("heading", { name: /Your organization/ })).toBeVisible();
    await page.getByTestId("marketing-faq").locator("summary").first().click();
    await expect(page.getByTestId("marketing-faq").locator("details").first()).toHaveAttribute(
      "open",
      "",
    );
    await page.goto("/docs");
    await page.locator('[data-testid="docs-operation"]').first().locator("summary").click();
    await expect(page.locator('[data-testid="docs-operation"]').first()).toHaveAttribute(
      "open",
      "",
    );
    await context.close();
  });

  for (const theme of ["light", "dark"]) {
    test(`story and docs have no serious accessibility violations in ${theme}`, async ({
      page,
    }) => {
      await page.addInitScript((value) => {
        localStorage.setItem("theme", value);
      }, theme);
      await page.emulateMedia({ reducedMotion: "reduce" });
      for (const route of ["/", "/docs"]) {
        await page.goto(route);
        const result = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
        expect(
          result.violations.filter((v) => v.impact === "serious" || v.impact === "critical"),
        ).toEqual([]);
      }
    });
  }

  test("audit poster visual baseline @visual", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/#audit");
    await expect(page.getByTestId("marketing-audit")).toHaveAttribute("data-phase", "4");
    await page.addStyleTag({ content: '[data-testid="marketing-header"] { visibility: hidden; }' });
    await expect(page.getByTestId("marketing-audit")).toHaveScreenshot("marketing-audit.png");
  });
});
