import { expect, test } from "@playwright/test";

import { login, ORG, PASSWORD, USERS } from "./helpers";

/** An org the fixtures do not contain. A tenant user must not be able to tell
 * this apart from one that exists but is not theirs. */
const FOREIGN_ORG = "globex-dev";

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

/**
 * Tenant isolation, at the HTTP boundary.
 *
 * `requireOrgAccess` answers 404 (not 403) for an org you are not a member of,
 * deliberately: a tenant user must learn nothing about which orgs exist. These
 * assert that at the BFF, not in a unit test, because the thing worth locking is
 * the status the network actually returns.
 */
test("a tenant admin cannot read another org, and learns nothing about it", async ({ page }) => {
  await login(page, USERS.admin);

  for (const path of [
    `/api/orgs/${FOREIGN_ORG}/metrics`,
    `/api/orgs/${FOREIGN_ORG}/golden-records`,
    `/api/orgs/${FOREIGN_ORG}/reviews`,
    `/api/orgs/${FOREIGN_ORG}/members`,
  ]) {
    const response = await page.request.get(path);
    expect(response.status(), path).toBe(404);
    // 404 and not 403: the code itself must not become an existence oracle.
    expect(response.status(), path).not.toBe(403);
  }
});

test("a tenant admin cannot reach the operator console", async ({ page }) => {
  await login(page, USERS.admin);
  // requireSuperAdmin throws 404 for the same reason.
  const response = await page.request.get("/api/admin/orgs");
  expect(response.status()).toBe(404);
});

/**
 * Role enforcement. The fixtures give acme-dev a viewer, a steward and an admin;
 * the role rank is viewer < steward < admin (src/lib/bff/proxy.ts).
 */
test("a viewer cannot perform steward or admin writes", async ({ page }) => {
  await login(page, USERS.viewer);

  // A read the viewer IS allowed, first — so a blanket failure would show up as
  // this assertion rather than as a false pass below.
  const allowed = await page.request.get(`/api/orgs/${ORG}/metrics`);
  expect(allowed.ok()).toBe(true);

  const forbidden = [
    { path: `/api/orgs/${ORG}/jobs/submit`, data: { kind: "run_all_incremental" } },
    { path: `/api/orgs/${ORG}/reviews/bulk-resolve`, data: { ids: ["r1"], decision: "merge" } },
    { path: `/api/orgs/${ORG}/members`, data: { email: "x@example.com", role: "admin" } },
  ];
  for (const { path, data } of forbidden) {
    const response = await page.request.post(path, { data });
    // 403: the viewer may see this org, so hiding it would be the wrong answer —
    // what they may not do is write to it.
    expect(response.status(), path).toBe(403);
  }
});

/**
 * The org path parameter is interpolated raw into the upstream URL, and
 * `new URL()` collapses `..` segments. requireOrgSlug rejects anything that is
 * not a slug before it can re-point that request.
 */
test("a traversal-shaped org name cannot re-point the upstream request", async ({ page }) => {
  await login(page, USERS.admin);
  for (const org of ["..", "..%2f..%2fvictim", "acme%2f..%2fvictim", "Acme-Dev"]) {
    const response = await page.request.get(`/api/orgs/${org}/metrics`);
    expect([404, 400], `org=${org}`).toContain(response.status());
  }
});

/**
 * The open redirect: `?next=` reached `router.push()` validated only against
 * `startsWith("/login")`, so an absolute URL sent a freshly authenticated user
 * off-origin — the moment they are least likely to check the address bar.
 */
test("a hostile ?next= cannot redirect off-origin after login", async ({ page, baseURL }) => {
  const origin = new URL(baseURL ?? "http://localhost:3100").origin;

  for (const hostile of ["https://evil.example/phish", "//evil.example/phish"]) {
    await page.goto(`/login?next=${encodeURIComponent(hostile)}`);
    await page.getByLabel("Email").fill(USERS.admin);
    await page.getByLabel("Password").fill(PASSWORD);
    await page.getByTestId("login-submit").click();

    // Land somewhere, then assert it is still us.
    await page.waitForURL((url) => !url.pathname.startsWith("/login"), { timeout: 15_000 });
    expect(new URL(page.url()).origin, `next=${hostile}`).toBe(origin);
    expect(page.url(), `next=${hostile}`).not.toContain("evil.example");

    await page.request.post("/api/auth/logout");
  }
});

test("a legitimate ?next= deep link still works", async ({ page }) => {
  // The fix must not break the flow the proxy and the tenant layout depend on.
  await page.goto(`/login?next=${encodeURIComponent(`/${ORG}/records`)}`);
  await page.getByLabel("Email").fill(USERS.admin);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByTestId("login-submit").click();
  await page.waitForURL(new RegExp(`/${ORG}/records`), { timeout: 15_000 });
  expect(new URL(page.url()).pathname).toBe(`/${ORG}/records`);
});
