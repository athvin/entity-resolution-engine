import Link from "next/link";

import { CANONICAL_ORIGIN, canonicalUrl } from "@/lib/site/routes";

/**
 * Search and AI-crawler plumbing for the marketing site: the share image,
 * JSON-LD, breadcrumbs and per-page "last updated" dates.
 *
 * Rule for everything here: structured data repeats what the page visibly
 * says. No ratings, reviews, awards or certifications, and no description that
 * is not on the page. `tests/component/seo.test.tsx` checks the shapes.
 */

/** `src/app/opengraph-image.png`, served at the site root. */
export const SHARE_IMAGE = {
  url: "/opengraph-image.png",
  width: 1200,
  height: 630,
  alt: "DupeZero — Entity resolution you can defend. Resolve. Review. Reconcile. Any source, any destination.",
} as const;

/**
 * When each page's content last changed, ISO dates. Feeds the sitemap's
 * `lastmod` and the visible "Last updated" lines, so the two cannot disagree.
 * Update a page's date when its content changes, not on every deploy: a date
 * that moves without the content moving tells crawlers something false.
 * Blog posts carry their own dates in blog-content.ts.
 */
export const PAGE_UPDATED: Readonly<Record<string, string>> = {
  "/": "2026-11-20",
  "/product": "2026-11-20",
  "/features/matching": "2026-11-20",
  "/features/review": "2026-11-20",
  "/features/golden-records": "2026-11-20",
  "/solutions": "2026-11-20",
  "/integrations": "2026-11-20",
  "/security": "2026-11-20",
  "/pricing": "2026-11-20",
  "/faq": "2026-11-20",
  "/resources": "2026-11-20",
  "/resources/causes-of-salesforce-duplicates": "2026-11-20",
  "/resources/preparing-for-a-bulk-merge": "2026-11-20",
  "/resources/matching-rules-explained": "2026-11-20",
  "/blog": "2026-11-20",
  "/about": "2026-11-20",
  "/contact": "2026-11-20",
  "/docs": "2026-11-20",
};

/** Solution pages share one template and one date. */
export const SOLUTION_PAGES_UPDATED = "2026-11-20";

/** "2026-11-20" → "20 November 2026", in UTC so static HTML never shifts a day. */
export function formatUpdated(iso: string): string {
  return new Intl.DateTimeFormat("en-GB", {
    day: "numeric",
    month: "long",
    year: "numeric",
    timeZone: "UTC",
  }).format(new Date(`${iso}T00:00:00Z`));
}

/** The Organization every other node points at. */
export const ORGANIZATION_ID = `${CANONICAL_ORIGIN}/#organization`;
export const SOFTWARE_ID = `${CANONICAL_ORIGIN}/#software`;

/**
 * One JSON-LD block. A text child, never `dangerouslySetInnerHTML`: React 19
 * leaves quotes unescaped in a <script> text child and encodes `</script`, so
 * this is valid JSON that cannot break out of the tag (see structured-data.tsx
 * and tests/component/structured-data.test.tsx, which pin both properties).
 */
export function JsonLd({ data }: { data: Record<string, unknown> }) {
  return (
    <script type="application/ld+json">
      {JSON.stringify({ "@context": "https://schema.org", ...data })}
    </script>
  );
}

export interface Crumb {
  href: string;
  label: string;
}

/** Top-level pages, and the section page each deeper page sits under. */
const SECTION_LABELS: Readonly<Record<string, string>> = {
  "/product": "Product",
  "/solutions": "Solutions",
  "/resources": "Resources",
  "/blog": "Blog",
};

function sectionFor(path: string): Crumb | null {
  if (path.startsWith("/features/") || path === "/integrations")
    return { href: "/product", label: "Product" };
  for (const href of ["/solutions", "/resources", "/blog"]) {
    if (path.startsWith(`${href}/`)) return { href, label: SECTION_LABELS[href] ?? href };
  }
  return null;
}

/** Home › [section ›] page. The page itself is last and is not a link. */
export function breadcrumbTrail(path: string, label: string): Crumb[] {
  const section = sectionFor(path);
  return [{ href: "/", label: "Home" }, ...(section ? [section] : []), { href: path, label }];
}

/**
 * Visible breadcrumbs plus the BreadcrumbList that describes exactly them.
 * Rendered on every interior page, above the page's eyebrow.
 */
export function Breadcrumbs({ path, label }: { path: string; label: string }) {
  const trail = breadcrumbTrail(path, label);
  return (
    <>
      <nav aria-label="Breadcrumb" className="page-breadcrumbs">
        <ol>
          {trail.map((c, i) =>
            i === trail.length - 1 ? (
              <li key={c.href} aria-current="page">
                {c.label}
              </li>
            ) : (
              <li key={c.href}>
                <Link href={c.href}>{c.label}</Link>
              </li>
            ),
          )}
        </ol>
      </nav>
      <JsonLd
        data={{
          "@type": "BreadcrumbList",
          itemListElement: trail.map((c, i) => ({
            "@type": "ListItem",
            position: i + 1,
            name: c.label,
            item: canonicalUrl(c.href),
          })),
        }}
      />
    </>
  );
}

/** A visible "Last updated" line for pages whose date matters to readers. */
export function LastUpdated({ iso }: { iso: string }) {
  return (
    <p className="page-updated story-muted">
      Last updated <time dateTime={iso}>{formatUpdated(iso)}</time>
    </p>
  );
}
