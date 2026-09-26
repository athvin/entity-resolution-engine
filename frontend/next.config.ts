import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  reactStrictMode: true,
  // The BFF is the only network boundary; the browser never talks to erserver directly.
  poweredByHeader: false,
  // Native/node-API packages the server bundle must require at runtime, not inline.
  serverExternalPackages: ["@node-rs/argon2", "@electric-sql/pglite", "postgres"],
};

export default nextConfig;
