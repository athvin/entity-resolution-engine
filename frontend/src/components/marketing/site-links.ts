/**
 * The marketing site's navigation: the header's links and the footer's columns.
 *
 * Hand-maintained alongside `MARKETING_PATHS` in `src/lib/site/routes.ts`,
 * which makes a page reachable and indexed but links it nowhere.
 * `tests/component/marketing-pages.test.tsx` checks that every listed path
 * appears here and that every link here points at a public path.
 */

export interface SiteLink {
  href: string;
  label: string;
}

export const HEADER_LINKS: readonly SiteLink[] = [
  { href: "/product", label: "Product" },
  { href: "/how-it-works", label: "How it works" },
  { href: "/pricing", label: "Pricing" },
  { href: "/docs", label: "Docs" },
  { href: "/contact", label: "Contact" },
];

export const FOOTER_COLUMNS: readonly { heading: string; links: readonly SiteLink[] }[] = [
  {
    heading: "Product",
    links: [
      { href: "/product", label: "Overview" },
      { href: "/how-it-works", label: "How it works" },
      { href: "/features/matching", label: "Matching" },
      { href: "/features/review", label: "Review" },
      { href: "/features/golden-records", label: "Golden records" },
      { href: "/integrations", label: "Integrations" },
    ],
  },
  {
    heading: "Learn",
    links: [
      { href: "/solutions", label: "Solutions by role" },
      { href: "/resources", label: "Resources" },
      { href: "/faq", label: "FAQ" },
      { href: "/docs", label: "Developer docs" },
      { href: "/pricing", label: "Pricing" },
    ],
  },
  {
    heading: "Company",
    links: [
      { href: "/about", label: "About" },
      { href: "/contact", label: "Contact" },
      { href: "/security", label: "Security" },
      { href: "/legal/privacy", label: "Privacy" },
      { href: "/legal/terms", label: "Terms" },
    ],
  },
];

/** True when `href` is the current page or a section containing it. */
export function isCurrent(href: string, pathname: string): boolean {
  if (href === "/") return pathname === "/";
  return pathname === href || pathname.startsWith(`${href}/`);
}
