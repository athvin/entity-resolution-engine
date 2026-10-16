import { expect, test } from "@playwright/test";

// The password-reset flow is unauthenticated (the token in the URL is the
// credential), so these run without a session. No @visual baselines yet — this
// spec asserts behaviour only.
test.describe("password reset", () => {
  test("request page is reachable without a session", async ({ page }) => {
    await page.goto("/reset");
    await expect(page).toHaveURL(/\/reset$/);
    await expect(page.getByTestId("reset-request-card")).toBeVisible();
    await expect(page.getByTestId("reset-email")).toBeVisible();
    await expect(page.getByTestId("reset-request-submit")).toBeVisible();
  });

  test("the login page links to it", async ({ page }) => {
    await page.goto("/login");
    await page.getByTestId("login-forgot").click();
    await expect(page).toHaveURL(/\/reset$/);
    await expect(page.getByTestId("reset-request-card")).toBeVisible();
  });

  test("requesting a link is enumeration-safe (same confirmation for any address)", async ({
    page,
  }) => {
    await page.goto("/reset");
    // An address with no account: the BFF returns 202 without sending anything,
    // and the page shows the same copy it would for a real account.
    await page.getByTestId("reset-email").fill("nobody@example.com");
    await page.getByTestId("reset-request-submit").click();
    await expect(page.getByTestId("reset-request-sent")).toBeVisible();
    await expect(page.getByTestId("reset-request-sent")).toContainText("on its way");
  });

  test("confirm page is reachable without a session and rejects an invalid token", async ({
    page,
  }) => {
    await page.goto("/reset/not-a-real-token");
    await expect(page.getByTestId("reset-confirm-card")).toBeVisible();
    await page.getByTestId("reset-password").fill("a-valid-length-password");
    await page.getByTestId("reset-confirm").fill("a-valid-length-password");
    await page.getByTestId("reset-confirm-submit").click();
    await expect(page.getByTestId("reset-error")).toBeVisible();
    await expect(page.getByTestId("reset-error")).toContainText("no longer valid");
  });

  test("confirm page catches a client-side password mismatch before any request", async ({
    page,
  }) => {
    await page.goto("/reset/not-a-real-token");
    await page.getByTestId("reset-password").fill("first-password-123");
    await page.getByTestId("reset-confirm").fill("second-password-456");
    await page.getByTestId("reset-confirm-submit").click();
    await expect(page.getByTestId("reset-error")).toContainText("don't match");
  });
});
