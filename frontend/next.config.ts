import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  reactStrictMode: true,
  // The BFF is the only network boundary; the browser never talks to erserver directly.
  poweredByHeader: false,
};

export default nextConfig;
