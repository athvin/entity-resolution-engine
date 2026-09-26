import { expect, test } from "@playwright/test";

import { login, ORG, USERS } from "./helpers";

/**
 * The M1 acceptance gate from the build plan: no erserver credential may ever
 * reach the browser. The mock fixtures store keys shaped `erk_mock_<role>_secret`
 * and the mock operator token is `mock-operator-token`; if either substring shows
 * up in ANY response the browser receives — HTML, JSON, JS bundle — the BFF
 * boundary is broken.
 */
const FORBIDDEN = ["erk_", "mock-operator-token"];

test("no erserver credential ever reaches the browser", async ({ page }) => {
  const leaks: string[] = [];
  page.on("response", (response) => {
    const type = response.headers()["content-type"] ?? "";
    if (!/json|html|javascript|text/.test(type)) return;
    void response
      .text()
      .then((body) => {
        for (const needle of FORBIDDEN) {
          if (body.includes(needle)) {
            leaks.push(`${needle} in ${response.url()}`);
          }
        }
      })
      .catch(() => undefined);
  });

  await login(page, USERS.admin);
  await page.goto(`/${ORG}/dashboard`);
  await expect(page.getByTestId("tile-records")).toContainText("800");

  // Exercise the session endpoint the shell uses, plus every dashboard read.
  const session = await page.request.get("/api/auth/session");
  expect((await session.text()).includes("erk_")).toBe(false);

  await page.waitForLoadState("networkidle");
  expect(leaks).toEqual([]);
});
