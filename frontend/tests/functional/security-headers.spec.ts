import { expect, test, type APIResponse } from "@playwright/test";

import { login, ORG, USERS } from "./helpers";

/**
 * The response security headers from next.config.ts.
 *
 * They are asserted on a STATIC page, a DYNAMIC page and an API route, because
 * those are served by three different paths through Next and a `headers()` entry
 * that silently misses one of them is the likely failure. `/login` is
 * prerendered, the tenant dashboard is server-rendered on demand, and
 * `/api/auth/session` is a route handler.
 */

/** key → expected value. Lower-cased: Playwright normalizes header names. */
const ENFORCED: Record<string, string> = {
  "x-frame-options": "DENY",
  "x-content-type-options": "nosniff",
  "referrer-policy": "strict-origin-when-cross-origin",
  "strict-transport-security": "max-age=63072000; includeSubDomains",
  "permissions-policy": "camera=(), microphone=(), geolocation=()",
};

function expectEnforced(headers: Record<string, string>, where: string): void {
  for (const [key, value] of Object.entries(ENFORCED)) {
    expect(headers[key], `${key} on ${where}`).toBe(value);
  }
}

test("every enforced security header is present on a static page", async ({ page }) => {
  const response = await page.goto("/login");
  if (!response) throw new Error("no response for /login");
  expectEnforced(response.headers(), "/login (static)");
});

test("every enforced security header is present on a dynamic page", async ({ page }) => {
  await login(page, USERS.admin);
  const response = await page.goto(`/${ORG}/dashboard`);
  if (!response) throw new Error("no response for the dashboard");
  expectEnforced(response.headers(), `/${ORG}/dashboard (dynamic)`);
});

test("every enforced security header is present on a BFF route", async ({ page }) => {
  await login(page, USERS.admin);
  const response: APIResponse = await page.request.get("/api/auth/session");
  expect(response.ok()).toBe(true);
  expectEnforced(response.headers(), "/api/auth/session (route handler)");
});

test("the CSP ships report-only, with a reporting endpoint and no external origins", async ({
  page,
}) => {
  const response = await page.goto("/login");
  if (!response) throw new Error("no response for /login");
  const headers = response.headers();

  // Report-only for now: a strict script-src needs a per-request nonce, which
  // Next can only inject into dynamically rendered pages — and /login is static.
  const csp = headers["content-security-policy-report-only"];
  expect(csp, "CSP-Report-Only must be set").toBeDefined();

  // Enforcing CSP must stay ABSENT until the nonce work lands; if it ever
  // appears by accident, the static pages are the thing that breaks.
  expect(headers["content-security-policy"]).toBeUndefined();

  expect(csp).toContain("default-src 'self'");
  expect(csp).toContain("frame-ancestors 'none'");
  expect(csp).toContain("object-src 'none'");
  // The BFF is the only network boundary — no external origin is permitted.
  expect(csp).toContain("connect-src 'self'");
  // A report-only policy with nowhere to report is just a header.
  expect(csp).toContain("report-uri /api/csp-report");
  expect(headers["reporting-endpoints"]).toContain("/api/csp-report");
});

test("the CSP report sink accepts a report without a session", async ({ request }) => {
  // Reports from the login page arrive before any session exists, so this
  // endpoint is in PUBLIC_PATHS. Those are the reports most worth having.
  const response = await request.post("/api/csp-report", {
    headers: { "content-type": "application/csp-report" },
    data: JSON.stringify({
      "csp-report": { "document-uri": "/login", "violated-directive": "script-src" },
    }),
  });
  expect(response.status()).toBe(204);
});

test("the CSP report sink refuses an oversized body", async ({ request }) => {
  const response = await request.post("/api/csp-report", {
    headers: { "content-type": "application/csp-report" },
    data: "x".repeat(20 * 1024),
  });
  expect(response.status()).toBe(413);
});
