/**
 * Search and AI-crawler plumbing: sitemap, robots policy, per-page metadata,
 * breadcrumbs and JSON-LD. Structured data must repeat what the page shows and
 * never claim ratings, reviews or prices the page does not display.
 */
import { render } from "@testing-library/react";
import type { Metadata } from "next";
import { describe, expect, it, vi } from "vitest";

import AboutPage, { metadata as aboutMeta } from "@/app/(marketing)/about/page";
import BlogPostPage, {
  generateMetadata as postMeta,
  generateStaticParams as blogParams,
} from "@/app/(marketing)/blog/[slug]/page";
import BlogPage, { metadata as blogMeta } from "@/app/(marketing)/blog/page";
import ContactPage, { metadata as contactMeta } from "@/app/(marketing)/contact/page";
import FaqPage, { metadata as faqMeta } from "@/app/(marketing)/faq/page";
import GoldenPage, { metadata as goldenMeta } from "@/app/(marketing)/features/golden-records/page";
import MatchingPage, { metadata as matchingMeta } from "@/app/(marketing)/features/matching/page";
import ReviewPage, { metadata as reviewMeta } from "@/app/(marketing)/features/review/page";
import IntegrationsPage, {
  metadata as integrationsMeta,
} from "@/app/(marketing)/integrations/page";
import PricingPage, { metadata as pricingMeta } from "@/app/(marketing)/pricing/page";
import ProductPage, { metadata as productMeta } from "@/app/(marketing)/product/page";
import ArticlePage, {
  generateMetadata as articleMeta,
  generateStaticParams as articleParams,
} from "@/app/(marketing)/resources/[slug]/page";
import ResourcesPage, { metadata as resourcesMeta } from "@/app/(marketing)/resources/page";
import SecurityPage, { metadata as securityMeta } from "@/app/(marketing)/security/page";
import SolutionsPage, { metadata as solutionsMeta } from "@/app/(marketing)/solutions/page";
import NotFound from "@/app/not-found";
import robots from "@/app/robots";
import sitemap from "@/app/sitemap";
import { breadcrumbTrail, SHARE_IMAGE } from "@/components/marketing/seo";
import { PRICING_CONFIG } from "@/lib/pricing/config";
import { CANONICAL_ORIGIN, MARKETING_PATHS, MARKETING_UNLISTED_PATHS } from "@/lib/site/routes";

vi.mock("next/navigation", () => ({
  usePathname: () => "/product",
  notFound: () => {
    throw new Error("notFound");
  },
}));

const PAGES: [string, () => React.ReactElement, Metadata][] = [
  ["/product", ProductPage, productMeta],
  ["/features/matching", MatchingPage, matchingMeta],
  ["/features/review", ReviewPage, reviewMeta],
  ["/features/golden-records", GoldenPage, goldenMeta],
  ["/solutions", SolutionsPage, solutionsMeta],
  ["/integrations", IntegrationsPage, integrationsMeta],
  ["/security", SecurityPage, securityMeta],
  ["/pricing", PricingPage, pricingMeta],
  ["/faq", FaqPage, faqMeta],
  ["/resources", ResourcesPage, resourcesMeta],
  ["/blog", BlogPage, blogMeta],
  ["/about", AboutPage, aboutMeta],
  ["/contact", ContactPage, contactMeta],
];

/** Every JSON-LD block in the rendered page, parsed. */
function jsonLd(): Record<string, unknown>[] {
  return [...document.querySelectorAll('script[type="application/ld+json"]')].map(
    (s) => JSON.parse(s.textContent) as Record<string, unknown>,
  );
}

/** Trust signals we have no source for: rating and review nodes, awards, credentials. */
const FORBIDDEN_TYPES =
  /"aggregateRating"|"@type":"(Review|AggregateRating)"|"review":|"award"|"hasCredential"/;

describe("sitemap.xml", () => {
  const entries = sitemap();

  it("lists exactly the indexable marketing pages, at canonical URLs", () => {
    expect(entries.map((e) => e.url).sort()).toEqual(
      MARKETING_PATHS.map((p) => `${CANONICAL_ORIGIN}${p === "/" ? "/" : p}`).sort(),
    );
    for (const p of MARKETING_UNLISTED_PATHS) {
      expect(entries.some((e) => e.url.endsWith(p))).toBe(false);
    }
  });

  it("gives every page a real content date, never a build timestamp", () => {
    for (const e of entries) {
      expect(e.lastModified, e.url).toMatch(/^\d{4}-\d{2}-\d{2}$/);
    }
  });
});

describe("robots.txt", () => {
  const { rules, sitemap: map } = robots();
  const groups = Array.isArray(rules) ? rules : [rules];
  const groupFor = (bot: string) =>
    groups.find((g) => (Array.isArray(g.userAgent) ? g.userAgent : [g.userAgent]).includes(bot));

  it("allows search engines and answer-retrieval bots, with the private paths closed", () => {
    for (const bot of [
      "Googlebot",
      "Bingbot",
      "OAI-SearchBot",
      "ChatGPT-User",
      "PerplexityBot",
      "Claude-SearchBot",
    ]) {
      const g = groupFor(bot);
      expect(g?.allow, bot).toBe("/");
      expect(g?.disallow, bot).toContain("/app");
      expect(g?.disallow, bot).toContain("/api/");
    }
  });

  it("states the training-crawler policy in its own group", () => {
    for (const bot of ["GPTBot", "ClaudeBot", "Google-Extended", "CCBot"]) {
      expect(groupFor(bot), bot).toBeDefined();
    }
  });

  it("sets no crawl delay and points at the sitemap", () => {
    for (const g of groups) expect(g.crawlDelay).toBeUndefined();
    expect(map).toBe(`${CANONICAL_ORIGIN}/sitemap.xml`);
  });
});

describe("per-page metadata", () => {
  it("gives every page a title, description, self-canonical and share image", async () => {
    const all: [string, Metadata][] = PAGES.map(([p, , m]) => [p, m]);
    for (const { slug } of blogParams()) {
      all.push([`/blog/${slug}`, await postMeta({ params: Promise.resolve({ slug }) })]);
    }
    for (const { slug } of articleParams()) {
      all.push([`/resources/${slug}`, await articleMeta({ params: Promise.resolve({ slug }) })]);
    }
    for (const [path, m] of all) {
      expect(m.title, path).toBeTruthy();
      expect(m.description, path).toBeTruthy();
      expect(m.alternates?.canonical, path).toBe(path);
      const og = m.openGraph?.images;
      expect(JSON.stringify(og), path).toContain(SHARE_IMAGE.url);
      expect(JSON.stringify(m.twitter), path).toContain(SHARE_IMAGE.url);
    }
  });
});

describe("structured data", () => {
  it("is valid JSON on every page, with breadcrumbs matching the visible trail", () => {
    for (const [path, Page] of PAGES) {
      const { unmount } = render(<Page />);
      const blocks = jsonLd();
      expect(JSON.stringify(blocks), path).not.toMatch(FORBIDDEN_TYPES);
      const crumbs = blocks.find((b) => b["@type"] === "BreadcrumbList") as
        { itemListElement: { name: string; item: string }[] } | undefined;
      expect(crumbs, path).toBeDefined();
      const visible = [...document.querySelectorAll('nav[aria-label="Breadcrumb"] li')].map(
        (li) => li.textContent,
      );
      expect(
        crumbs?.itemListElement.map((i) => i.name),
        path,
      ).toEqual(visible);
      expect(crumbs?.itemListElement.at(-1)?.item, path).toBe(`${CANONICAL_ORIGIN}${path}`);
      unmount();
    }
  });

  it("nests features and integrations under Product, posts under Blog", () => {
    expect(breadcrumbTrail("/features/review", "Review").map((c) => c.label)).toEqual([
      "Home",
      "Product",
      "Review",
    ]);
    expect(breadcrumbTrail("/blog/x", "X").map((c) => c.href)).toEqual(["/", "/blog", "/blog/x"]);
    expect(breadcrumbTrail("/pricing", "Pricing").map((c) => c.href)).toEqual(["/", "/pricing"]);
  });

  it("describes the product with the description the page shows", () => {
    render(<ProductPage />);
    const app = jsonLd().find((b) => b["@type"] === "SoftwareApplication") as
      { description: string; offers?: unknown } | undefined;
    expect(app).toBeDefined();
    expect(document.body.textContent).toContain(app?.description);
    // No prices on /product, so no offer there.
    expect(app?.offers).toBeUndefined();
  });

  it("offers only the annual prices the pricing table shows", () => {
    render(<PricingPage />);
    const app = jsonLd().find((b) => b["@type"] === "SoftwareApplication") as {
      offers: { lowPrice: number; highPrice: number; priceCurrency: string };
    };
    const prices = PRICING_CONFIG.tiers.map((t) => t.annualCents / 100);
    expect(app.offers.lowPrice).toBe(Math.min(...prices));
    expect(app.offers.highPrice).toBe(Math.max(...prices));
    expect(app.offers.priceCurrency).toBe("USD");
    const text = document.body.textContent;
    for (const p of [app.offers.lowPrice, app.offers.highPrice]) {
      expect(text).toContain(`$${p.toLocaleString("en-US")}`);
    }
  });

  it("dates blog posts and guides with the date the page shows", async () => {
    for (const { slug } of blogParams()) {
      const { unmount } = render(await BlogPostPage({ params: Promise.resolve({ slug }) }));
      const post = jsonLd().find((b) => b["@type"] === "BlogPosting") as {
        datePublished: string;
        headline: string;
      };
      expect(post.datePublished).toMatch(/^\d{4}-\d{2}-\d{2}$/);
      expect(document.querySelector("h1")?.textContent).toBe(post.headline);
      unmount();
    }
    for (const { slug } of articleParams()) {
      const { unmount } = render(await ArticlePage({ params: Promise.resolve({ slug }) }));
      const article = jsonLd().find((b) => b["@type"] === "Article") as { dateModified: string };
      expect(article.dateModified).toMatch(/^\d{4}-\d{2}-\d{2}$/);
      expect(document.body.textContent).toContain("Last updated");
      unmount();
    }
  });
});

describe("the 404 page", () => {
  it("links to the key pages", () => {
    const { getByTestId } = render(<NotFound />);
    const hrefs = [...getByTestId("not-found-links").querySelectorAll("a")].map((a) =>
      a.getAttribute("href"),
    );
    expect(hrefs).toEqual(
      expect.arrayContaining(["/", "/product", "/pricing", "/docs", "/contact"]),
    );
  });
});
