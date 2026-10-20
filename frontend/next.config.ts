import type { NextConfig } from "next";

/**
 * Response security headers for every path.
 *
 * Set here rather than in src/proxy.ts because `headers()` applies to static
 * responses too: `/login` and `/reset` are prerendered (see the build output),
 * and the proxy's matcher deliberately excludes static assets.
 *
 * The CSP is REPORT-ONLY for now, and the policy below is the realistic one
 * rather than the strict one, on purpose. A strict script-src needs a per-request
 * nonce, which Next can only inject into DYNAMICALLY rendered pages — the login
 * and reset pages are static, so a nonce policy would either break them or force
 * them dynamic. Starting report-only with 'unsafe-inline' for Next's own
 * bootstrap means the reports we get are about genuinely unexpected origins
 * (an injected script, an unplanned external connect) instead of drowning in
 * our own inline tags. Tightening script-src via nonces is the follow-up; see
 * docs/frontend-testing.md.
 */
const CSP_REPORT_ONLY = [
  "default-src 'self'",
  // Next's inline bootstrap + React hydration payload. Nonce-able only once the
  // static pages become dynamic — the follow-up above.
  "script-src 'self' 'unsafe-inline'",
  // Tailwind's injected styles and next-themes' inline theme flash-guard.
  "style-src 'self' 'unsafe-inline'",
  "img-src 'self' data: blob:",
  "font-src 'self' data:",
  // The BFF is the only network boundary — there is deliberately no external
  // origin the browser is allowed to reach. This directive is the one most
  // worth watching in the report feed.
  "connect-src 'self'",
  "frame-ancestors 'none'",
  "object-src 'none'",
  "base-uri 'self'",
  "form-action 'self'",
  "report-uri /api/csp-report",
  "report-to csp-endpoint",
].join("; ");

const SECURITY_HEADERS = [
  // Clickjacking. frame-ancestors in the CSP is the modern equivalent, but that
  // is report-only today, so this is what actually enforces it.
  { key: "X-Frame-Options", value: "DENY" },
  { key: "X-Content-Type-Options", value: "nosniff" },
  { key: "Referrer-Policy", value: "strict-origin-when-cross-origin" },
  // Two years, subdomains included. Inert over plain HTTP — RFC 6797 §8.1 makes
  // a UA ignore this header on a non-secure transport — so the dev stack and the
  // Playwright tiers (both http://localhost) are unaffected by it.
  { key: "Strict-Transport-Security", value: "max-age=63072000; includeSubDomains" },
  // This app asks for none of these; say so rather than leaving it to the default.
  { key: "Permissions-Policy", value: "camera=(), microphone=(), geolocation=()" },
  { key: "Reporting-Endpoints", value: 'csp-endpoint="/api/csp-report"' },
  { key: "Content-Security-Policy-Report-Only", value: CSP_REPORT_ONLY },
];

const nextConfig: NextConfig = {
  reactStrictMode: true,
  // The BFF is the only network boundary; the browser never talks to erserver directly.
  poweredByHeader: false,
  headers() {
    return Promise.resolve([{ source: "/:path*", headers: SECURITY_HEADERS }]);
  },
  // Native/node-API packages the server bundle must require at runtime, not inline.
  // drizzle-orm is external for the standalone image's sake: inlined, it would be
  // absent from the traced node_modules, and scripts/db-migrate.mjs -- the er-web
  // image's migration entrypoint (infrastructure.md §12) -- could not import it.
  serverExternalPackages: ["@node-rs/argon2", "@electric-sql/pglite", "postgres", "drizzle-orm"],
};

// The container build (frontend/Dockerfile) sets this to emit the self-contained
// `.next/standalone` server the er-web image runs (infrastructure.md §11). Gated on
// an env var rather than set unconditionally so the dev stack and the CI/Playwright
// `next start` tiers keep today's exact behavior, warning-free.
if (process.env.ER_WEB_STANDALONE === "1") {
  nextConfig.output = "standalone";
}

export default nextConfig;
