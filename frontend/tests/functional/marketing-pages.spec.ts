import { expect, test } from "@playwright/test";

import { FOOTER_COLUMNS, HEADER_LINKS } from "../../src/components/marketing/site-links";
import { MARKETING_PATHS, MARKETING_UNLISTED_PATHS } from "../../src/lib/site/routes";

/**
 * The content pages: each renders anonymously, is reachable from the header,
 * and keeps the indexed/unlisted split the sitemap depends on. Runs on every
 * project, so the small-screen Menu path is covered on the phone projects.
 */
test.describe("marketing content pages", () => {
  for (const path of [...MARKETING_PATHS, ...MARKETING_UNLISTED_PATHS]) {
    test(`${path} renders anonymously with one h1`, async ({ page }) => {
      const response = await page.goto(path);
      expect(response?.status()).toBe(200);
      await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
      const robots = page.locator('meta[name="robots"]');
      if (MARKETING_UNLISTED_PATHS.includes(path)) {
        await expect(robots).toHaveAttribute("content", /noindex/);
      } else {
        await expect(robots).toHaveCount(0);
      }
    });
  }

  test("every header link navigates and is marked current", async ({ page }) => {
    for (const { href, label } of HEADER_LINKS.filter((l) => !l.groups)) {
      await page.goto("/");
      const menu = page.getByTestId("marketing-menu-button");
      if (await menu.isVisible()) await menu.click();
      const nav = page.getByRole("navigation", { name: "Main navigation" });
      await nav.getByRole("link", { name: label, exact: true }).click();
      await expect(page).toHaveURL(new RegExp(`${href}$`));
      if (await menu.isVisible()) await menu.click();
      await expect(nav.getByRole("link", { name: label, exact: true })).toHaveAttribute(
        "aria-current",
        "page",
      );
    }
  });

  test("the old How it works URL lands on the Product page's walkthrough", async ({ page }) => {
    const response = await page.request.get("/how-it-works", { maxRedirects: 0 });
    expect(response.status()).toBe(308);
    expect(response.headers().location).toBe("/product#how-it-works");
    await page.goto("/how-it-works");
    await expect(page).toHaveURL(/\/product#how-it-works$/);
    await expect(page.locator("#how-it-works")).toBeVisible();
  });

  // One test per link: a single test walking all of them outran the 30s budget.
  for (const item of HEADER_LINKS.filter((l) => l.groups)) {
    const slug = item.label.toLowerCase();
    for (const link of (item.groups ?? []).flatMap((g) => g.links)) {
      test(`the ${item.label} dropdown opens by click and reaches ${link.label}`, async ({
        page,
      }) => {
        await page.goto("/");
        const menu = page.getByTestId("marketing-menu-button");
        const mobile = await menu.isVisible();
        if (mobile) await menu.click();
        await page.getByTestId(mobile ? `marketing-menu-${slug}` : `marketing-nav-${slug}`).click();
        const panel = page.getByTestId(
          mobile ? `marketing-menu-${slug}-panel` : `marketing-nav-${slug}-panel`,
        );
        await panel.getByRole("link", { name: link.label, exact: true }).click();
        await expect(page).toHaveURL(new RegExp(`${link.href}$`));
        await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      });
    }
  }

  test("the sitemap lists indexed pages only", async ({ request }) => {
    const xml = await (await request.get("/sitemap.xml")).text();
    for (const path of MARKETING_PATHS.filter((p) => p !== "/")) {
      expect(xml).toContain(`https://dupezero.com${path}</loc>`);
    }
    for (const path of MARKETING_UNLISTED_PATHS) expect(xml).not.toContain(path);
  });

  test("footer links all resolve, and nothing is wider than a 320px phone", async ({ page }) => {
    await page.setViewportSize({ width: 320, height: 720 });
    // One visit per page: /pricing#roi is a section of /pricing, and a same-document
    // hash navigation returns no response.
    for (const href of new Set(
      FOOTER_COLUMNS.flatMap((c) => c.links.map((l) => l.href.split("#")[0] ?? l.href)),
    )) {
      const response = await page.goto(href);
      expect(response?.status(), href).toBe(200);
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth),
        href,
      ).toBeLessThanOrEqual(320);
    }
  });
});
