import type { MetadataRoute } from "next";

import { POSTS } from "@/components/marketing/pages/blog-content";
import { PAGE_UPDATED, SOLUTION_PAGES_UPDATED } from "@/components/marketing/seo";
import { MARKETING_PATHS, canonicalUrl } from "@/lib/site/routes";

/** When the page's content last changed: blog dates, then PAGE_UPDATED, then the solution template. */
function lastModifiedFor(path: string): string | undefined {
  const post = POSTS.find((p) => path === `/blog/${p.slug}`);
  if (post) return post.date;
  if (PAGE_UPDATED[path]) return PAGE_UPDATED[path];
  if (path.startsWith("/solutions/")) return SOLUTION_PAGES_UPDATED;
  return undefined;
}

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
 * `lastModified` is the content's own date (seo.tsx PAGE_UPDATED, blog post
 * dates), never the build time: a timestamp that moves on every deploy tells
 * crawlers the content changed when it did not.
 */
export default function sitemap(): MetadataRoute.Sitemap {
  return MARKETING_PATHS.map((path) => ({
    url: canonicalUrl(path),
    ...(lastModifiedFor(path) ? { lastModified: lastModifiedFor(path) } : {}),
    changeFrequency: "monthly" as const,
    priority: path === "/" ? 1 : 0.7,
  }));
}
