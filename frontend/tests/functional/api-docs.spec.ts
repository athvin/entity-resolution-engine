import { expect, test } from "@playwright/test";

/**
 * /docs/api (#867): the public API reference, readable signed-out.
 *
 * Owner decisions on #867, asserted here rather than trusted: the reference is
 * rendered in-house (no stock viewer, so no colour-coded methods), and the
 * first call is a copyable request built from environment variables, never a
 * live call carrying a customer's key.
 */
test.describe("api reference", () => {
  test("renders for a signed-out visitor, without a login redirect", async ({ page }) => {
    const response = await page.goto("/docs/api");
    expect(response?.status()).toBe(200);
    expect(page.url()).toMatch(/\/docs\/api$/);
    await expect(page.getByRole("heading", { level: 1, name: "API reference." })).toBeVisible();
    await expect(page.getByTestId("api-operation").first()).toBeAttached();
  });

  test("the connect flow ends in a first call the visitor copies", async ({ page }) => {
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
    await page.goto("/docs/api");
    await expect(page.locator("#connect")).toContainText("Settings → API keys");
    await page.getByRole("button", { name: "Copy First call" }).click();
    const copied = await page.locator("html").getAttribute("data-copied");
    // Authorize → first call: the bearer header, then the org read-back. The
    // key is a variable: the page never holds a credential.
    expect(copied).toContain("Authorization: Bearer $ER_API_KEY");
    expect(copied).toContain('"$ER_API_ORIGIN/v1/orgs/$ER_ORG"');
    expect(copied).not.toMatch(/erk_/);
  });

  test("an operation documents its role, its typed response and its request", async ({ page }) => {
    await page.goto("/docs/api");
    const list = page.locator("#get_orgs_by_org_api_keys");
    await list.locator("summary").click();
    await expect(list).toContainText("admin+");
    await expect(list.getByRole("link", { name: "ApiKeyList" })).toHaveAttribute(
      "href",
      "#schema-ApiKeyList",
    );
    await expect(list).toContainText('curl -X GET "$ER_API_ORIGIN/v1/orgs/$ER_ORG/api-keys"');
    // The cursor and limit a paged read takes are documented (#865).
    const reviews = page.locator("#get_orgs_by_org_datasets_by_ds_reviews");
    await reviews.locator("summary").click();
    await expect(reviews).toContainText("Query parameters");
    await expect(reviews).toContainText("next_cursor");
  });

  test("the document itself is public JSON with no operator route", async ({ request }) => {
    const resp = await request.get("/api/openapi", { maxRedirects: 0 });
    expect(resp.status()).toBe(200);
    const doc = (await resp.json()) as {
      paths: Record<string, Record<string, { security?: Record<string, string[]>[] }>>;
    };
    const roles = Object.values(doc.paths).flatMap((item) =>
      Object.values(item).flatMap((op) => (op.security ?? []).flatMap((s) => s.bearerAuth ?? [])),
    );
    expect(roles.length).toBeGreaterThan(50);
    expect(roles).not.toContain("operator");
  });
});
