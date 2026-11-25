import { expect, test } from "@playwright/test";

import { apiKeyOrg, login, USERS } from "./helpers";

/**
 * Self-service API keys (#866). Before this, a key came only from the operator,
 * so "connect your own system" began with a support ticket. The flow a customer
 * admin needs is mint → copy → list → revoke, and the plaintext must exist on
 * screen exactly once.
 */
test.describe("api keys", () => {
  test("an admin mints a key, sees it once, and revokes it", async ({ page }, testInfo) => {
    const org = apiKeyOrg(testInfo.project.name);
    // A stub clipboard, as in the docs spec: the real one needs a permission
    // grant WebKit does not offer, and the assertion is about what was copied.
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
    await login(page, USERS.admin);
    await page.goto(`/${org}/settings/api-keys`);

    const list = page.getByTestId("api-keys-list");
    await expect(list.getByTestId("api-key-row")).toHaveCount(4);
    // The app's own three service keys say what they are and cannot be
    // revoked here; the one customer key can.
    await expect(list.getByTestId("api-key-service")).toHaveCount(3);

    await page.getByTestId("api-key-role").selectOption("steward");
    await page.getByTestId("api-key-create").click();
    const plaintext = page.getByTestId("api-key-plaintext");
    await expect(plaintext).toContainText(`erk_mock-${org}-steward`);
    await expect(page.getByTestId("api-key-secret")).toContainText("Shown once");

    await page.getByTestId("api-key-copy").click();
    await expect(page.getByTestId("api-key-copy")).toContainText("Copied");
    const shown = await plaintext.textContent();
    await expect(page.locator("html")).toHaveAttribute("data-copied", shown ?? "");

    // The list gained a row and carries no plaintext at all.
    await expect(list.getByTestId("api-key-row")).toHaveCount(5);
    await expect(list).not.toContainText("erk_");

    // Revoke the key just minted. Its id is the newest non-service row.
    const revoke = list.locator('[data-testid^="api-key-revoke-"]').first();
    await revoke.click();
    await expect(list).toContainText("revoked");

    // A reload is a fresh read: the plaintext is gone for good.
    await page.reload();
    await expect(page.getByTestId("api-keys-list")).toBeVisible();
    await expect(page.getByTestId("api-key-secret")).toHaveCount(0);
  });

  test("the app's own credential cannot be revoked through the BFF", async ({ page }, testInfo) => {
    const org = apiKeyOrg(testInfo.project.name);
    await login(page, USERS.admin);
    // No button is offered, so this is the route-level refusal behind it.
    const resp = await page.request.delete(`/api/orgs/${org}/api-keys/mock-${org}-admin`);
    expect(resp.status()).toBe(409);
    expect(await resp.text()).toContain("sign the app out");
  });

  test("a non-admin is told the page is admin-only", async ({ page }, testInfo) => {
    const org = apiKeyOrg(testInfo.project.name);
    // The steward: a member of these per-project orgs (the viewer is not),
    // and below admin, which is the case this asserts.
    await login(page, USERS.steward);
    await page.goto(`/${org}/settings/api-keys`);
    await expect(page.getByTestId("api-keys-admin-only")).toBeVisible();
    // ...and the BFF agrees: membership is not enough to list keys.
    const resp = await page.request.get(`/api/orgs/${org}/api-keys`);
    expect(resp.status()).toBe(403);
  });
});
