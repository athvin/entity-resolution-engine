import { expect, test } from "@playwright/test";
import operations from "../../src/components/marketing/docs/operations.json";

test.describe("marketing developer docs", () => {
  test("public reference covers the manifest and the customer workflow", async ({ page }) => {
    const response = await page.goto("/docs");
    expect(response?.status()).toBe(200);
    await expect(page.getByRole("heading", { name: "Build with DupeZero." })).toBeVisible();
    await expect(page.getByTestId("docs-operation")).toHaveCount(operations.length);
    for (const id of [
      "organizations",
      "authentication",
      "configuration",
      "ingestion",
      "jobs",
      "results",
      "corrections",
      "errors",
      "schemas",
    ])
      await expect(page.locator(`#${id}`)).toBeAttached();
    await expect(page.locator("#schemas")).toContainText("no operation definitions");
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
      true,
    );
  });
  test("examples copy through an explicit action", async ({ page }) => {
    await page.addInitScript(() => {
      Object.defineProperty(navigator, "clipboard", {
        value: {
          writeText: (text: string) => {
            document.documentElement.dataset.copied = text;
            return Promise.resolve();
          },
        },
      });
    });
    await page.goto("/docs");
    await page.getByRole("button", { name: "Copy Client environment" }).click();
    await expect(page.locator("html")).toHaveAttribute("data-copied", /ER_API_ORIGIN/);
    await expect(page.getByRole("button", { name: "Copy Client environment" })).toContainText(
      "Copied",
    );
  });
  test("sitemap and navigation expose docs without a session", async ({ page }) => {
    await page.goto("/");
    await page
      .getByRole("navigation", { name: "Main navigation" })
      .getByRole("link", { name: "Docs" })
      .click();
    await expect(page).toHaveURL(/\/docs$/);
    const sitemap = await page.request.get("/sitemap.xml");
    expect(await sitemap.text()).toContain("https://dupezero.com/docs");
  });
  test("developer docs opening visual baseline @visual", async ({ page }) => {
    await page.goto("/docs");
    await expect(page.locator("#overview")).toHaveScreenshot("developer-docs-overview.png");
  });
});
