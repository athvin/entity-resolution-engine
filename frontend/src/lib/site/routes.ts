/**
 * The public marketing surface: its routes and its canonical origin.
 *
 * No `server-only` import. `src/proxy.ts` spreads MARKETING_PATHS into its
 * PUBLIC_PATHS list and runs on the edge runtime, so this module must stay free
 * of node imports — the same constraint `src/lib/auth/constants.ts` carries.
 *
 * Pure data by design. This file lives under `src/lib/**`, which is the only
 * tree the per-PR coverage floor measures, and coverage runs with Vitest's
 * `all` — so an uncovered function here spends the function budget. Presentation
 * data belongs in `src/components/marketing/content.ts` instead, outside the
 * glob. `tests/unit/site-routes.test.ts` keeps this file at full coverage.
 */

/**
 * Where the marketing site lives on the public internet.
 *
 * Hardcoded, deliberately, and NOT an environment variable:
 *
 * 1. A canonical URL is one value for the whole internet, forever — that is
 *    what "canonical" means. Making it per-environment is the bug, not the
 *    feature: a developer namespace must never advertise itself as canonical,
 *    which is what `robots.ts` uses ERWEB_PUBLIC_BASE_URL to detect.
 * 2. `metadataBase` is mandatory the moment a page declares a canonical tag or
 *    an OG image, and `next build` runs in the Docker image with no runtime
 *    environment at all (see frontend/Dockerfile). A build-time variable would
 *    bake whichever value happened to be set, and one image serves many
 *    hostnames.
 * 3. This is not a NEXT_PUBLIC_* workaround. That prefix is banned because it
 *    inlines a value into the client bundle (.semgrep.yml, no-next-public-env).
 *    Site metadata is rendered by server components into <head> and reaches the
 *    browser as HTML, never as JavaScript — so no public variable is needed and
 *    none should be added.
 */
export const CANONICAL_ORIGIN = "https://dupezero.com";

/**
 * Every path on the marketing surface, in nav order.
 *
 * Single source of truth for three consumers that would otherwise drift:
 * `src/proxy.ts` (reachable without a session), `src/app/sitemap.ts` (what
 * crawlers are told about), and the marketing nav itself. Adding a page means
 * adding it here and nowhere else.
 */
/*
 * Annotated `readonly string[]` rather than left to infer `as const`. With the
 * literal tuple type, `strictTypeChecked` proves `path === "/"` always true in
 * `sitemap.ts` and rejects the comparison — correct about today's single entry,
 * and wrong about what this list is. It is a list of paths that happens to have
 * one element, not a one-element tuple.
 */
export const MARKETING_PATHS: readonly string[] = ["/"];

/**
 * Paths the proxy must let through unauthenticated for the marketing surface to
 * work at all.
 *
 * `/robots.txt` and `/sitemap.xml` are the non-obvious entries. The proxy's
 * matcher excludes only `.svg|.png|.ico|.webp`, so a `.txt` or `.xml` response
 * is matched like any page — without these, every crawler gets a 307 to
 * `/login` and the site is unindexable. (A static `opengraph-image.png` needs no
 * entry; it ends in `.png`. A generated `opengraph-image.tsx` would, because
 * Next serves it extensionless — one of several reasons the image is a
 * committed file.)
 */
export const MARKETING_PUBLIC_PATHS: readonly string[] = [
  ...MARKETING_PATHS,
  "/robots.txt",
  "/sitemap.xml",
];

/**
 * Where the marketing CTA sends someone.
 *
 * Deliberately not a dashboard URL. This route is the signed-in dispatcher
 * (`src/app/app/page.tsx`): it resolves the viewer's first membership, or sends
 * a super admin to the registry, or falls back to `/login?error=no-workspace`.
 * Because it is NOT in PUBLIC_PATHS, a signed-out visitor is bounced by the
 * proxy's cookie check before that page's database query runs, and arrives back
 * here after login via `?next=`.
 *
 * The upshot is one CTA that is correct for both states with zero client
 * JavaScript on the landing page — no session fetch, no flash of the wrong
 * label.
 */
export const PORTAL_ENTRY_PATH = "/app";

/** Absolute URL for a marketing path, for sitemap entries and JSON-LD. */
export function canonicalUrl(path: string): string {
  return new URL(path, CANONICAL_ORIGIN).toString();
}
