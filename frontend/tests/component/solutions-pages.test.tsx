import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import IndustryPage, {
  generateMetadata as industryMeta,
  generateStaticParams as industryParams,
} from "@/app/(marketing)/solutions/industries/[slug]/page";
import SolutionsPage from "@/app/(marketing)/solutions/page";
import RolePage, {
  generateMetadata as roleMeta,
  generateStaticParams as roleParams,
} from "@/app/(marketing)/solutions/roles/[slug]/page";
import { MarketingNav } from "@/components/marketing/marketing-nav";
import AboutPage from "@/app/(marketing)/about/page";
import {
  ABOUT_SCENE,
  INDUSTRIES,
  INDUSTRY_BASE,
  ROLE_BASE,
  ROLES,
  SOLUTIONS_SCENE,
} from "@/components/marketing/pages/solutions-content";
import { MARKETING_PATHS } from "@/lib/site/routes";

const pathname = vi.hoisted(() => ({ current: "/solutions/roles/marketing" }));
vi.mock("next/navigation", () => ({
  usePathname: () => pathname.current,
  notFound: () => {
    throw new Error("notFound");
  },
}));

const FEATURE_PAGES = new Set(MARKETING_PATHS);

describe("the solutions content", () => {
  it("names exactly the roles and industries the menu promises", () => {
    expect(ROLES.map((r) => r.name)).toEqual([
      "Salesforce admins",
      "Revenue operations",
      "Marketing",
      "IT and Security",
    ]);
    expect(INDUSTRIES.map((i) => i.name)).toEqual([
      "Technology",
      "Nonprofit",
      "Financial Services",
      "Healthcare & Life Sciences",
      "Education",
      "Real Estate",
      "Manufacturing",
    ]);
  });

  it("has a public, indexed route for every page and no route without a page", () => {
    const content = [
      ...ROLES.map((r) => `${ROLE_BASE}/${r.slug}`),
      ...INDUSTRIES.map((i) => `${INDUSTRY_BASE}/${i.slug}`),
    ];
    const routes = MARKETING_PATHS.filter((p) => p.startsWith("/solutions/"));
    expect([...routes].sort()).toEqual([...content].sort());
  });

  it("backs every 'how it helps' line with a published page", () => {
    for (const s of [...ROLES, ...INDUSTRIES]) {
      for (const h of s.helps) expect(FEATURE_PAGES, `${s.slug}: ${h.title}`).toContain(h.href);
    }
  });

  it("makes no compliance claim on any industry page", () => {
    for (const i of INDUSTRIES) {
      expect(i.note.body).toContain("holds no security or industry compliance certification");
      const text = JSON.stringify(i);
      expect(text).not.toMatch(/HIPAA|SOC ?2|ISO ?27001|GDPR compliant|PCI|FERPA|certified/i);
    }
  });
});

describe("the role and industry pages", () => {
  const cases = [
    ...ROLES.map((s) => ["role", s.slug, RolePage, roleMeta, ROLE_BASE] as const),
    ...INDUSTRIES.map(
      (s) => ["industry", s.slug, IndustryPage, industryMeta, INDUSTRY_BASE] as const,
    ),
  ];

  it.each(cases)("%s %s renders with its own metadata", async (_, slug, Page, meta, base) => {
    const params = Promise.resolve({ slug });
    const m = await meta({ params });
    expect(m.alternates?.canonical).toBe(`${base}/${slug}`);
    expect(m.robots).toBeUndefined();
    render(await Page({ params }));
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    for (const title of ["Where duplicates hurt", "How DupeZero helps", "Use cases"]) {
      expect(screen.getByRole("heading", { level: 2, name: title })).toBeInTheDocument();
    }
    const related = screen.getByRole("navigation", { name: "Related pages" });
    expect(within(related).queryByRole("link", { name: /^$/ })).toBeNull();
  });

  it("generates one static page per entry and 404s the rest", async () => {
    expect(roleParams()).toHaveLength(ROLES.length);
    expect(industryParams()).toHaveLength(INDUSTRIES.length);
    const params = Promise.resolve({ slug: "nope" });
    expect(await roleMeta({ params })).toEqual({});
    expect(await industryMeta({ params })).toEqual({});
    await expect(RolePage({ params })).rejects.toThrow("notFound");
    await expect(IndustryPage({ params })).rejects.toThrow("notFound");
  });

  it("asks for written confirmation before regulated data is sent, with no TODOs left", async () => {
    render(
      await IndustryPage({ params: Promise.resolve({ slug: "healthcare-and-life-sciences" }) }),
    );
    expect(
      screen.getByText(/Do not send patient health information to DupeZero/),
    ).toBeInTheDocument();
    for (const s of [...ROLES, ...INDUSTRIES])
      expect(JSON.stringify(s), s.slug).not.toMatch(/TODO/);
    expect(JSON.stringify(INDUSTRIES)).toMatch(/confirm in writing whether we can hold them/);
  });

  it("does not describe DupeZero as working only from CSV exports", () => {
    // Owner-confirmed 2026-11-20: data arrives through the Salesforce
    // AppExchange app and API connections, with CSV upload as one option.
    for (const s of [...ROLES, ...INDUSTRIES]) {
      const text = JSON.stringify(s);
      expect(text, s.slug).not.toMatch(/works from CSV|does not connect|Upload an? \w+ export/i);
    }
  });

  it("links every role and industry from the overview", () => {
    render(<SolutionsPage />);
    for (const r of ROLES) {
      expect(screen.getByRole("link", { name: `DupeZero for ${r.name}` })).toHaveAttribute(
        "href",
        `${ROLE_BASE}/${r.slug}`,
      );
    }
    for (const i of INDUSTRIES) {
      expect(screen.getByRole("link", { name: `DupeZero for ${i.name}` })).toHaveAttribute(
        "href",
        `${INDUSTRY_BASE}/${i.slug}`,
      );
    }
  });
});

describe("the Solutions dropdown", () => {
  beforeEach(() => {
    pathname.current = "/solutions/roles/marketing";
  });

  it("opens by click, lists every page, and marks the current one", () => {
    render(<MarketingNav />);
    const button = screen.getByTestId("marketing-nav-solutions");
    const panel = screen.getByTestId("marketing-nav-solutions-panel");
    expect(button).toHaveAttribute("aria-expanded", "false");
    expect(button).toHaveClass("font-semibold");
    expect(panel).not.toBeVisible();

    fireEvent.click(button);
    expect(button).toHaveAttribute("aria-expanded", "true");
    expect(panel).toBeVisible();
    expect(within(panel).getAllByRole("link")).toHaveLength(1 + ROLES.length + INDUSTRIES.length);
    expect(within(panel).getByRole("link", { name: "Marketing" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    // The overview link is current only on /solutions itself.
    expect(within(panel).getByRole("link", { name: "All solutions" })).not.toHaveAttribute(
      "aria-current",
    );
  });

  it("closes on Escape with focus back on its button, and on an outside click", () => {
    render(<MarketingNav />);
    const button = screen.getByTestId("marketing-nav-solutions");
    fireEvent.click(button);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(button).toHaveAttribute("aria-expanded", "false");
    expect(button).toHaveFocus();

    fireEvent.click(button);
    fireEvent.click(document.body);
    expect(button).toHaveAttribute("aria-expanded", "false");
  });

  it("nests inside the small-screen menu, and Escape closes it before the menu", () => {
    pathname.current = "/solutions";
    render(<MarketingNav />);
    fireEvent.click(screen.getByTestId("marketing-menu-button"));
    const nested = screen.getByTestId("marketing-menu-solutions");
    const panel = screen.getByTestId("marketing-menu-solutions-panel");
    expect(panel).not.toBeVisible();

    fireEvent.click(nested);
    expect(panel).toBeVisible();
    expect(within(panel).getByRole("link", { name: "All solutions" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(within(panel).getByRole("link", { name: "Healthcare & Life Sciences" })).toBeVisible();

    fireEvent.keyDown(document, { key: "Escape" });
    expect(panel).not.toBeVisible();
    expect(nested).toHaveFocus();
    expect(screen.getByTestId("marketing-menu-panel")).toBeVisible();

    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.getByTestId("marketing-menu-panel")).not.toBeVisible();
    expect(screen.getByTestId("marketing-menu-button")).toHaveFocus();
  });
});

describe("the resolution scenes", () => {
  const scenes = [
    ...[...ROLES, ...INDUSTRIES].map((solution) => [solution.slug, solution.scene] as const),
    ["solutions", SOLUTIONS_SCENE] as const,
    ["about", ABOUT_SCENE] as const,
  ];

  it("gives every role, industry and overview page three records resolving into one", () => {
    expect(scenes).toHaveLength(ROLES.length + INDUSTRIES.length + 2);
    for (const [slug, scene] of scenes) {
      expect(scene.records, slug).toHaveLength(3);
      expect(scene.resolved.name, slug).not.toBe("");
      expect(scene.sentence, slug).toMatch(/\.$/);
    }
  });

  // People and companies only, fictional: `.example` domains and 555-01xx
  // numbers, and nothing a page about regulated industries must not show.
  it("shows only fictional people and companies, never sensitive fields", () => {
    for (const [slug, scene] of scenes) {
      expect(JSON.stringify(scene), slug).not.toMatch(
        /patient|student|diagnos|medical record|social security|\bssn\b|date of birth|account number|grade/i,
      );
      for (const value of [...scene.records.map((record) => record.value), scene.resolved.value]) {
        if (value.includes("@")) expect(value, slug).toMatch(/@[a-z0-9.-]+\.example$/);
        if (/^\(\d{3}\)/.test(value)) expect(value, slug).toMatch(/^\(\d{3}\) 555-01\d\d$/);
        if (/^[a-z0-9.-]+\.[a-z]+$/.test(value)) expect(value, slug).toMatch(/\.example$/);
      }
    }
  });

  it("renders each page's scene, and /solutions' and /about's", async () => {
    for (const solution of ROLES) {
      const { unmount } = render(
        await RolePage({ params: Promise.resolve({ slug: solution.slug }) }),
      );
      expect(screen.getByTestId("resolution-figure")).toHaveTextContent(
        solution.scene.resolved.name,
      );
      unmount();
    }
    for (const solution of INDUSTRIES) {
      const { unmount } = render(
        await IndustryPage({ params: Promise.resolve({ slug: solution.slug }) }),
      );
      expect(screen.getByTestId("resolution-figure")).toHaveTextContent(solution.scene.sentence);
      unmount();
    }
    for (const [Page, scene] of [
      [SolutionsPage, SOLUTIONS_SCENE],
      [AboutPage, ABOUT_SCENE],
    ] as const) {
      const { unmount } = render(<Page />);
      expect(screen.getByTestId("resolution-figure")).toHaveTextContent(scene.sentence);
      unmount();
    }
  });
});
