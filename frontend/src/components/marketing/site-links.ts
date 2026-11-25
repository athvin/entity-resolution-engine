/**
 * The marketing site's navigation: the header's links and the footer's columns.
 *
 * Hand-maintained alongside `MARKETING_PATHS` in `src/lib/site/routes.ts`,
 * which makes a page reachable and indexed but links it nowhere.
 * `tests/component/marketing-pages.test.tsx` checks that every listed path
 * appears here and that every link here points at a public path.
 */

import { INDUSTRIES, INDUSTRY_BASE, ROLE_BASE, ROLES } from "./pages/solutions-content";

/** Where every demo button goes: the contact form, until a booking tool exists. */
export const DEMO_PATH = "/contact";

export interface SiteLink {
  href: string;
  label: string;
}

export interface LinkGroup {
  /** Shown above the group's links; omit it for a dropdown with one plain list. */
  heading?: string | undefined;
  links: readonly SiteLink[];
}

/** A footer column always has its heading: it names the column's nav landmark. */
export interface FooterColumn extends LinkGroup {
  heading: string;
}

/** A header item: a plain link, or a dropdown of grouped links. */
export interface NavItem extends SiteLink {
  groups?: readonly LinkGroup[];
  /** The dropdown's first link, to `href`. Defaults to "All <label>". */
  overviewLabel?: string | undefined;
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

/** The savings (ROI) calculator: a section of /pricing, not its own page. */
export const ROI_CALCULATOR_HREF = "/pricing#roi";

export const RESOURCE_GROUPS: readonly LinkGroup[] = [
  {
    heading: "Read",
    links: [
      { href: "/docs", label: "Docs" },
      { href: "/blog", label: "Blog" },
    ],
  },
  {
    heading: "Calculate",
    links: [{ href: ROI_CALCULATOR_HREF, label: "ROI calculator" }],
  },
];

export const PRODUCT_GROUPS: readonly LinkGroup[] = [
  {
    links: [
      { href: "/features/matching", label: "Matching" },
      { href: "/features/review", label: "Review" },
      { href: "/features/golden-records", label: "Golden Records" },
      { href: "/integrations", label: "Integrations" },
    ],
  },
];

export const HEADER_LINKS: readonly NavItem[] = [
  { href: "/product", label: "Product", groups: PRODUCT_GROUPS, overviewLabel: "Overview" },
  { href: "/solutions", label: "Solutions", groups: SOLUTION_GROUPS },
  { href: "/resources", label: "Resources", groups: RESOURCE_GROUPS },
  { href: "/pricing", label: "Pricing" },
  { href: "/contact", label: "Contact" },
];

export const FOOTER_COLUMNS: readonly FooterColumn[] = [
  {
    heading: "Product",
    links: [
      { href: "/product", label: "Overview" },
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
      { href: "/blog", label: "Blog" },
      { href: "/faq", label: "FAQ" },
      { href: "/docs", label: "Developer docs" },
      { href: "/docs/api", label: "API reference" },
      { href: "/pricing", label: "Pricing" },
      { href: ROI_CALCULATOR_HREF, label: "ROI calculator" },
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
  // A link to a section of a page (the ROI calculator) is never "the current page".
  if (href.includes("#")) return false;
  if (href === "/") return pathname === "/";
  return pathname === href || pathname.startsWith(`${href}/`);
}
