import { describe, expect, it } from "vitest";

import {
  CANONICAL_ORIGIN,
  MARKETING_PATHS,
  MARKETING_PUBLIC_PATHS,
  PORTAL_ENTRY_PATH,
  canonicalUrl,
} from "@/lib/site/routes";

/**
 * `src/lib/**` is the only tree the coverage floor measures, and coverage runs
 * with Vitest's `all` — so a module here that no unit test imports reports 0%
 * and drags the floor down (see `src/lib/nav.ts`, which sits at zero for
 * exactly that reason). This file exists so the route constants land at 100%
 * rather than spending budget.
 *
 * The assertions are not filler. Each one pins a property something else
 * depends on: the proxy spreads these into PUBLIC_PATHS, the sitemap maps them
 * to absolute URLs, and `metadataBase` parses the origin at build time.
 */
describe("marketing route constants", () => {
  it("exposes an https origin that parses as a URL", () => {
    // `new URL(CANONICAL_ORIGIN)` runs in the marketing layout's metadataBase.
    // A malformed value there is a build failure, not a runtime one.
    const url = new URL(CANONICAL_ORIGIN);
    expect(url.protocol).toBe("https:");
    expect(CANONICAL_ORIGIN.endsWith("/")).toBe(false);
  });

  it("declares every marketing path as a root-relative path", () => {
    expect(MARKETING_PATHS.length).toBeGreaterThan(0);
    for (const path of MARKETING_PATHS) {
      expect(path.startsWith("/")).toBe(true);
      // A protocol-relative value here would widen the proxy's PUBLIC_PATHS to
      // match an off-origin prefix.
      expect(path.startsWith("//")).toBe(false);
    }
  });

  it("has no duplicate public paths", () => {
    // A duplicate is harmless in the proxy but silently doubles a sitemap entry.
    expect(new Set(MARKETING_PUBLIC_PATHS).size).toBe(MARKETING_PUBLIC_PATHS.length);
  });

  it("makes the two metadata routes public", () => {
    // The proxy matcher excludes only .svg/.png/.ico/.webp, so without these
    // entries every crawler gets a 307 to /login and the site is unindexable.
    expect(MARKETING_PUBLIC_PATHS).toContain("/robots.txt");
    expect(MARKETING_PUBLIC_PATHS).toContain("/sitemap.xml");
  });

  it("keeps the portal entry OUT of the public paths", () => {
    // This is the load-bearing one. /app is the signed-in dispatcher: if it
    // were public, the proxy would stop bouncing anonymous visitors and that
    // page's database query would run for them.
    expect(MARKETING_PUBLIC_PATHS).not.toContain(PORTAL_ENTRY_PATH);
    expect(PORTAL_ENTRY_PATH.startsWith("/")).toBe(true);
  });

  it("builds absolute URLs against the canonical origin", () => {
    expect(canonicalUrl("/")).toBe(`${CANONICAL_ORIGIN}/`);
    expect(canonicalUrl("/sitemap.xml")).toBe(`${CANONICAL_ORIGIN}/sitemap.xml`);
  });
});
