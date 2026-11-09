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
    await page.locator("#flow-dataset").selectOption("1");
    await expect(flow.locator('[data-node="source-6"]')).toHaveCount(0);
    await page.locator("#flow-dataset").selectOption("0");
    await expect(flow.getByRole("button", { name: /ZoomInfo/ })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await page.locator("#flow-volume").fill("4");
    await expect(page.locator("#usage-volume")).toHaveValue("4");
    await expect(page.getByTestId("marketing-usage")).toContainText("10,000,000");
    await page.locator("#usage-dataset").selectOption("1");
    await expect(page.locator("#flow-dataset")).toHaveValue("1");
    await expect(page.getByTestId("usage-flow")).toContainText("Operations");
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
    await expect(bridges.locator("[data-flow-path]")).toHaveCount(5);
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

  test("audit holds its clock while paused and retains the corrected result", async ({ page }) => {
    await page.goto("/#audit");
    const audit = page.getByTestId("marketing-audit");
    await expect(audit).toHaveAttribute("data-running", "true");
    await audit.getByRole("button", { name: "Pause audit" }).click();
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
