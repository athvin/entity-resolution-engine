import type { Page } from "@playwright/test";

/** Fixture identities installed by ERWEB_TEST_FIXTURES=1 (see src/lib/db/test-fixtures.ts). */
export const USERS = {
  superAdmin: "root@er.dev",
  admin: "admin@acme.dev",
  steward: "steward@acme.dev",
  viewer: "viewer@acme.dev",
} as const;

export const PASSWORD = "password-123!";
export const ORG = "acme-dev";

/** Sign in through the real BFF; page.request shares the page's cookie jar. */
export async function login(page: Page, email: string): Promise<void> {
  const response = await page.request.post("/api/auth/login", {
    data: { email, password: PASSWORD },
  });
  if (!response.ok()) {
    throw new Error(`login as ${email} failed: ${String(response.status())}`);
  }
}
