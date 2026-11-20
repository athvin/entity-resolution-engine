/**
 * The marketing site's navigation: the header's links and the footer's columns.
 *
 * Hand-maintained alongside `MARKETING_PATHS` in `src/lib/site/routes.ts`,
 * which makes a page reachable and indexed but links it nowhere.
 * `tests/component/marketing-pages.test.tsx` checks that every listed path
 * appears here and that every link here points at a public path.
 */

import { INDUSTRIES, INDUSTRY_BASE, ROLE_BASE, ROLES } from "./pages/solutions-content";

export interface SiteLink {
  href: string;
  label: string;
}

export interface LinkGroup {
  heading: string;
  links: readonly SiteLink[];
}

/** A header item: a plain link, or a dropdown of grouped links. */
export interface NavItem extends SiteLink {
  groups?: readonly LinkGroup[];
}

export const SOLUTION_GROUPS: readonly LinkGroup[] = [
  {
    heading: "By role",
    links: ROLES.map((r) => ({ href: `${ROLE_BASE}/${r.slug}`, label: r.name })),
  },
  {
    heading: "By industry",
    links: INDUSTRIES.map((i) => ({ href: `${INDUSTRY_BASE}/${i.slug}`, label: i.name })),
  },
];

export const HEADER_LINKS: readonly NavItem[] = [
  { href: "/product", label: "Product" },
  { href: "/solutions", label: "Solutions", groups: SOLUTION_GROUPS },
  { href: "/how-it-works", label: "How it works" },
  { href: "/pricing", label: "Pricing" },
  { href: "/docs", label: "Docs" },
  { href: "/contact", label: "Contact" },
];

export const FOOTER_COLUMNS: readonly LinkGroup[] = [
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
    heading: "Solutions",
    links: [
      { href: "/solutions", label: "All solutions" },
      ...ROLES.map((r) => ({ href: `${ROLE_BASE}/${r.slug}`, label: r.name })),
    ],
  },
  {
    heading: "Learn",
    links: [
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

/** Every href the header and footer link to, dropdowns included. */
export function allLinkedHrefs(): Set<string> {
  const hrefs = new Set<string>();
  for (const item of HEADER_LINKS) {
    hrefs.add(item.href);
    for (const g of item.groups ?? []) for (const l of g.links) hrefs.add(l.href);
  }
  for (const col of FOOTER_COLUMNS) for (const l of col.links) hrefs.add(l.href);
  return hrefs;
}

/** True when `href` is the current page or a section containing it. */
export function isCurrent(href: string, pathname: string): boolean {
  if (href === "/") return pathname === "/";
  return pathname === href || pathname.startsWith(`${href}/`);
}
