import type { MetadataRoute } from "next";

import { MARKETING_PATHS, canonicalUrl } from "@/lib/site/routes";

/**
 * At `src/app/`, not inside `(marketing)` — see the note in `robots.ts`.
 *
 * Driven entirely off `MARKETING_PATHS`, which means the authenticated surface
 * is excluded by construction rather than by a disallow list that someone has
 * to remember to extend. Adding a marketing page adds a sitemap entry and a
 * `PUBLIC_PATHS` entry from the same edit.
 *
 * URLs are absolute from `CANONICAL_ORIGIN` rather than relative, so this does
 * not depend on `metadataBase` resolution.
 *
 * No `lastModified`. A build timestamp would change every deploy and tell a
 * crawler the content changed when it did not, which is worse than omitting the
 * field; when a page genuinely has an edit date worth publishing, it should come
 * from the content rather than the clock.
 */
export default function sitemap(): MetadataRoute.Sitemap {
  return MARKETING_PATHS.map((path) => ({
    url: canonicalUrl(path),
    changeFrequency: "monthly" as const,
    priority: path === "/" ? 1 : 0.7,
  }));
}
