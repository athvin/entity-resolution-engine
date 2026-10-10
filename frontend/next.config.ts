import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  reactStrictMode: true,
  // The BFF is the only network boundary; the browser never talks to erserver directly.
  poweredByHeader: false,
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
