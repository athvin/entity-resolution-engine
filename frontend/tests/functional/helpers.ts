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
/** Reserved for tests that mutate mock state; never pixel-compared. */
export const ORG_MUTABLE = "mutable-dev";

/** One private triage org per device project, so parallel projects never race. */
export function triageOrg(projectName: string): string {
  const byProject: Record<string, string> = {
    "desktop-chromium": "mutable-dev",
    "mobile-safari": "mutable-ios",
    "mobile-chrome": "mutable-android",
  };
  const org = byProject[projectName];
  if (!org) throw new Error(`no triage org for project ${projectName}`);
  return org;
}

/** The wizard journey's own org per project: its activate path PUBLISHES a
 * sources change, which would flip the shared triage org's active config out
 * from under the config-studio tier assertions. */
export function wizardOrg(projectName: string): string {
  const byProject: Record<string, string> = {
    "desktop-chromium": "wizard-dev",
    "mobile-safari": "wizard-ios",
    "mobile-chrome": "wizard-android",
  };
  const org = byProject[projectName];
  if (!org) throw new Error(`no wizard org for project ${projectName}`);
  return org;
}

/** Sign in through the real BFF; page.request shares the page's cookie jar. */
export async function login(page: Page, email: string): Promise<void> {
  const response = await page.request.post("/api/auth/login", {
    data: { email, password: PASSWORD },
  });
  if (!response.ok()) {
    throw new Error(`login as ${email} failed: ${String(response.status())}`);
  }
}
