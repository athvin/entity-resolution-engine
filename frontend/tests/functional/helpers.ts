import { expect, type Page } from "@playwright/test";

/** Fixture identities installed by ERWEB_TEST_FIXTURES=1 (see src/lib/db/test-fixtures.ts). */
export const USERS = {
  superAdmin: "root@er.dev",
  admin: "admin@acme.dev",
  steward: "steward@acme.dev",
  viewer: "viewer@acme.dev",
} as const;

export const PASSWORD = "password-123!";
export const ORG = "acme-dev";

/** The dataset every journey runs inside: the mock mirrors the backend sweep,
 * so each fixture org carries exactly one dataset named `default`. */
export const DS = "default";

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

/** The registry's own org per project: activating a version ARCHIVES the one it
 * replaces, so a project that activates would otherwise flip the state another
 * project is asserting about — all three projects share one mock server. */
export function modelOrg(projectName: string): string {
  const byProject: Record<string, string> = {
    "desktop-chromium": "models-dev",
    "mobile-safari": "models-ios",
    "mobile-chrome": "models-android",
  };
  const org = byProject[projectName];
  if (!org) throw new Error(`no models org for project ${projectName}`);
  return org;
}

/** Tagging's own org per project: creating a tag changes the vocabulary every
 * other test on that org is offered, and all three projects share one mock
 * server. Same reason the registry, webhooks and labelling each get three. */
export function tagOrg(projectName: string): string {
  const byProject: Record<string, string> = {
    "desktop-chromium": "tags-dev",
    "mobile-safari": "tags-ios",
    "mobile-chrome": "tags-android",
  };
  const org = byProject[projectName];
  if (!org) throw new Error(`no tag org for project ${projectName}`);
  return org;
}

/** Labelling's own org per project: recording a verdict REMOVES the pair from
 * the queue (the server excludes already-labelled pairs), so a project that
 * labels would shrink the queue another project is asserting the length of. */
export function labelingOrg(projectName: string): string {
  const byProject: Record<string, string> = {
    "desktop-chromium": "labeling-dev",
    "mobile-safari": "labeling-ios",
    "mobile-chrome": "labeling-android",
  };
  const org = byProject[projectName];
  if (!org) throw new Error(`no labelling org for project ${projectName}`);
  return org;
}

/** The multi-dataset org per project: the manage screen creates, suspends and
 * resumes datasets, and all three projects share one mock server, so a
 * project that suspends would freeze a dataset another is asserting is live. */
export function datasetsOrg(projectName: string): string {
  const byProject: Record<string, string> = {
    "desktop-chromium": "datasets-dev",
    "mobile-safari": "datasets-ios",
    "mobile-chrome": "datasets-android",
  };
  const org = byProject[projectName];
  if (!org) throw new Error(`no datasets org for project ${projectName}`);
  return org;
}

/** The webhook page's own org per project: subscribing and removing both
 * mutate the list, and all three projects share one mock server, so a project
 * that removes would delete the row another is asserting about. */
export function webhookOrg(projectName: string): string {
  const byProject: Record<string, string> = {
    "desktop-chromium": "webhooks-dev",
    "mobile-safari": "webhooks-ios",
    "mobile-chrome": "webhooks-android",
  };
  const org = byProject[projectName];
  if (!org) throw new Error(`no webhooks org for project ${projectName}`);
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

/**
 * Fill a CONTROLLED input across the hydration boundary.
 *
 * A fill that lands BEFORE React hydrates deadlocks the input on React 19,
 * and retrying the same fill cannot escape it. Mechanism, reconstructed from
 * the workers=2 campaign traces (grow.spec's assistant test, mobile-safari —
 * the one engine slow enough to lose the race under CPU contention):
 *
 *  1. `fill(text)` pre-hydration: the text sits in the DOM, no listener yet.
 *  2. Hydration completes. React 19 PRESERVES user-entered DOM text rather
 *     than wiping it to the rendered value — and primes its internal value
 *     tracker with that DOM text, while component state stays "".
 *  3. Every later `fill(text)` sets the SAME string, so the tracker sees no
 *     change, no `onChange` fires, state stays empty, and a submit gated on
 *     it (`disabled={!input.trim()}`) never enables. The trace shows exactly
 *     this terminal state: text visible in the input, button disabled, page
 *     JS demonstrably alive (route prefetches, session fetch).
 *
 * The clear-then-fill pair breaks the deadlock: the "" -> text transition
 * always differs from the tracker, so once hydration has happened one retry
 * fires `onChange` and the gated control enables — which is the condition
 * only reachable through React state, hence the inner assertion.
 */
export async function hydratedFill(
  page: Page,
  inputTestId: string,
  text: string,
  enablesTestId: string,
): Promise<void> {
  await expect(async () => {
    const box = page.getByTestId(inputTestId);
    await box.fill("");
    await box.fill(text);
    await expect(page.getByTestId(enablesTestId)).toBeEnabled({ timeout: 500 });
  }).toPass({ timeout: 15_000 });
}
