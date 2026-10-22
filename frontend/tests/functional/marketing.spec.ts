import { expect, test } from "@playwright/test";

/**
 * The public marketing surface.
 *
 * Every test here runs with **no session**, which is the property that matters:
 * this is the only part of the app a visitor sees before signing in, and the
 * proxy's default is to redirect everything to /login.
 *
 * Visual baselines are scoped to each section rather than `fullPage`. A long
 * landing page screenshotted whole would rewrite all three device baselines on
 * any change anywhere, and the diff would be thousands of pixels tall with no
 * indication of what moved. Per-section means a copy edit in the FAQ touches
 * the FAQ baseline only.
 *
 * Reminder for whoever regenerates them: `playwright.config.ts` sets
 * `ignoreSnapshots` off Linux, so a macOS run reports green having compared
 * nothing. Baselines come from `make frontend-baselines` or CI.
 */

test.describe("marketing", () => {
  test("the landing page renders every section anonymously", async ({ page }) => {
    const response = await page.goto("/");
    expect(response?.status()).toBe(200);

    for (const section of [
      "marketing-header",
      "marketing-hero",
      "marketing-problem",
      "marketing-how-it-works",
      "marketing-proof",
      "marketing-faq",
      "marketing-cta",
      "marketing-footer",
    ]) {
      await expect(page.getByTestId(section)).toBeVisible();
    }
  });

  test("every call to action points at the portal entry", async ({ page }) => {
    await page.goto("/");
    for (const cta of ["marketing-cta-header", "marketing-cta-hero", "marketing-cta-closing"]) {
      await expect(page.getByTestId(cta)).toHaveAttribute("href", "/app");
    }
  });

  test("the portal CTA sends a signed-out visitor to login and back", async ({ page }) => {
    await page.goto("/");
    await page.getByTestId("marketing-cta-hero").click();
    // The proxy's cookie gate catches /app before its database query runs, and
    // preserves the destination so login returns the visitor to it.
    await expect(page).toHaveURL(/\/login\?next=%2Fapp$/);
  });

  test("the FAQ opens without JavaScript state", async ({ page }) => {
    await page.goto("/");
    const first = page.getByTestId("marketing-faq").locator("details").first();
    await expect(first).not.toHaveAttribute("open", "");
    await first.locator("summary").click();
    await expect(first).toHaveAttribute("open", "");
  });

  test("the page carries Organization and SoftwareApplication JSON-LD", async ({ page }) => {
    await page.goto("/");
    const raw = await page.locator('script[type="application/ld+json"]').textContent();
    expect(raw).toBeTruthy();
    // The point of the assertion: React's handling of a <script> text child
    // leaves this parseable. If a React upgrade changes that, JSON-LD silently
    // stops working for every crawler and only this catches it.
    const parsed = JSON.parse(raw ?? "") as { "@graph": { "@type": string }[] };
    expect(parsed["@graph"].map((node) => node["@type"]).sort()).toEqual([
      "Organization",
      "SoftwareApplication",
    ]);
  });

  test("robots.txt is served as text and is not redirected", async ({ page }) => {
    const response = await page.goto("/robots.txt");
    expect(response?.status()).toBe(200);
    expect(response?.headers()["content-type"]).toContain("text/plain");
    const body = await response?.text();
    // The proxy's matcher excludes only .svg/.png/.ico/.webp, so without an
    // explicit PUBLIC_PATHS entry this would be a 307 to /login.
    expect(body).toContain("User-Agent: *");
    expect(body).toContain("Disallow: /app");
  });

  test("sitemap.xml is served as XML and lists only public paths", async ({ page }) => {
    const response = await page.goto("/sitemap.xml");
    expect(response?.status()).toBe(200);
    expect(response?.headers()["content-type"]).toContain("xml");
    const body = (await response?.text()) ?? "";
    expect(body).toContain("https://dupezero.com/");
    // Driven off MARKETING_PATHS, so the authenticated surface is excluded by
    // construction rather than by a disallow list someone has to maintain.
    for (const authenticated of ["/app", "/admin", "/login", "/dashboard"]) {
      expect(body).not.toContain(`https://dupezero.com${authenticated}`);
    }
  });

  test("the landing page carries the enforced security headers", async ({ page }) => {
    // security-headers.spec.ts deliberately checks a static page, a dynamic
    // page and an API route, because `headers()` can miss a serving path. `/`
    // changed category from dynamic to static in this change, so it is worth
    // one assertion of its own.
    const response = await page.goto("/");
    const headers = response?.headers() ?? {};
    expect(headers["x-content-type-options"]).toBe("nosniff");
    expect(headers["referrer-policy"]).toBeTruthy();
  });

  test("landing hero visual baseline @visual", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByTestId("marketing-hero")).toBeVisible();
    await expect(page.getByTestId("marketing-hero")).toHaveScreenshot("marketing-hero.png");
  });

  test("landing how-it-works visual baseline @visual", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByTestId("marketing-how-it-works")).toBeVisible();
    await expect(page.getByTestId("marketing-how-it-works")).toHaveScreenshot(
      "marketing-how-it-works.png",
    );
  });

  test("landing closing cta visual baseline @visual", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByTestId("marketing-cta")).toBeVisible();
    await expect(page.getByTestId("marketing-cta")).toHaveScreenshot("marketing-cta.png");
  });
});
