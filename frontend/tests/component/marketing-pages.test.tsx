import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { Metadata } from "next";
import { beforeEach, describe, expect, it, vi } from "vitest";

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
import PrivacyPage, { metadata as privacyMeta } from "@/app/(marketing)/legal/privacy/page";
import TermsPage, { metadata as termsMeta } from "@/app/(marketing)/legal/terms/page";
import ProductPage, { metadata as productMeta } from "@/app/(marketing)/product/page";
import ArticlePage, {
  generateMetadata as articleMeta,
  generateStaticParams,
} from "@/app/(marketing)/resources/[slug]/page";
import ResourcesPage, { metadata as resourcesMeta } from "@/app/(marketing)/resources/page";
import SecurityPage, { metadata as securityMeta } from "@/app/(marketing)/security/page";
import SolutionsPage, { metadata as solutionsMeta } from "@/app/(marketing)/solutions/page";
import { MarketingFooter } from "@/components/marketing/marketing-footer";
import { MarketingHeader } from "@/components/marketing/marketing-header";
import { MarketingNav } from "@/components/marketing/marketing-nav";
import { POSTS } from "@/components/marketing/pages/blog-content";
import { validateContact } from "@/components/marketing/pages/contact-form";
import { formatPostDate } from "@/components/marketing/pages/post-date";
import { FAQ_ITEMS } from "@/components/marketing/pages/faq-content";
import {
  allLinkedHrefs,
  FOOTER_COLUMNS,
  HEADER_LINKS,
  isCurrent,
} from "@/components/marketing/site-links";
import {
  MARKETING_PATHS,
  MARKETING_PUBLIC_PATHS,
  MARKETING_UNLISTED_PATHS,
} from "@/lib/site/routes";

const pathname = vi.hoisted(() => ({ current: "/product" }));
vi.mock("next/navigation", () => ({
  usePathname: () => pathname.current,
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
  ["/faq", FaqPage, faqMeta],
  ["/resources", ResourcesPage, resourcesMeta],
  ["/about", AboutPage, aboutMeta],
  ["/contact", ContactPage, contactMeta],
];

describe("the content pages", () => {
  it.each(PAGES)("%s renders one h1 and declares its own canonical URL", (path, Page, meta) => {
    render(<Page />);
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(meta.alternates?.canonical).toBe(path);
    expect(meta.openGraph?.url).toBe(path);
    expect(meta.description).toBeTruthy();
    expect(meta.robots).toBeUndefined();
    expect(MARKETING_PATHS).toContain(path);
  });

  it("gives every page a unique title and description", () => {
    const titles = PAGES.map(([, , m]) => m.title as string);
    const descriptions = PAGES.map(([, , m]) => String(m.description));
    expect(new Set(titles).size).toBe(titles.length);
    expect(new Set(descriptions).size).toBe(descriptions.length);
  });

  it("keeps the unreviewed legal pages noindex and unlisted, with their text filled in", () => {
    for (const [path, Page, meta] of [
      ["/legal/privacy", PrivacyPage, privacyMeta],
      ["/legal/terms", TermsPage, termsMeta],
    ] as const) {
      const { unmount } = render(<Page />);
      expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
      expect(document.body).toHaveTextContent("Last updated 20 November 2026");
      expect(document.body).not.toHaveTextContent("TODO");
      expect(meta.robots).toEqual({ index: false, follow: true });
      expect(MARKETING_UNLISTED_PATHS).toContain(path);
      unmount();
    }
  });

  it("publishes every article in full at an indexed path", async () => {
    for (const { slug } of generateStaticParams()) {
      const params = Promise.resolve({ slug });
      const meta = await articleMeta({ params });
      expect(meta.robots).toBeUndefined();
      expect(MARKETING_PATHS).toContain(`/resources/${slug}`);
      const { unmount } = render(await ArticlePage({ params }));
      expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
      expect(screen.getAllByRole("heading", { level: 2 }).length).toBeGreaterThanOrEqual(4);
      expect(document.body).not.toHaveTextContent("TODO");
      unmount();
    }
    expect(await articleMeta({ params: Promise.resolve({ slug: "nope" }) })).toEqual({});
    await expect(ArticlePage({ params: Promise.resolve({ slug: "nope" }) })).rejects.toThrow(
      "notFound",
    );
  });

  it("publishes FAQ structured data matching the visible questions", () => {
    const { container } = render(<FaqPage />);
    const script = container.querySelector('script[type="application/ld+json"]');
    const data = JSON.parse(script?.textContent ?? "{}") as {
      mainEntity: { name: string; acceptedAnswer: { text: string } }[];
    };
    expect(data.mainEntity.map((q) => q.name)).toEqual(FAQ_ITEMS.map((i) => i.q));
    for (const item of FAQ_ITEMS) {
      expect(screen.getByText(item.q)).toBeInTheDocument();
      expect(screen.getByText(item.a, { exact: false })).toBeInTheDocument();
    }
  });
});

describe("navigation", () => {
  beforeEach(() => {
    pathname.current = "/product";
  });

  it("links every marketing page from the header or the footer", () => {
    const linked = allLinkedHrefs();
    // Articles are linked from the /resources index rather than the header or footer.
    // Articles and posts are linked from their index pages, not the header or footer.
    const fromIndex = (p: string) => p.startsWith("/resources/") || p.startsWith("/blog/");
    for (const path of MARKETING_PATHS.filter((p) => p !== "/" && !fromIndex(p))) {
      expect(linked, path).toContain(path);
    }
    for (const href of linked) expect(MARKETING_PUBLIC_PATHS).toContain(href.split("#")[0]);
  });

  it("keeps the header to five items, with How it works inside Product", () => {
    render(<MarketingNav />);
    const nav = screen.getAllByRole("navigation", { name: "Main navigation" })[0];
    if (!nav) throw new Error("no main navigation");
    expect(within(nav).queryByRole("link", { name: "How it works" })).toBeNull();
    expect(HEADER_LINKS.map((l) => l.label)).toEqual([
      "Product",
      "Solutions",
      "Pricing",
      "Resources",
      "Contact",
    ]);
  });

  it("puts the step-by-step walkthrough on the Product page", () => {
    render(<ProductPage />);
    expect(document.getElementById("how-it-works")).not.toBeNull();
    expect(screen.getByRole("link", { name: "See how it works" })).toHaveAttribute(
      "href",
      "#how-it-works",
    );
    expect(document.body).not.toHaveTextContent(/works from CSV|does not connect/i);
  });

  it("groups Docs, Blog and the ROI calculator under Resources", () => {
    pathname.current = "/blog";
    render(<MarketingNav />);
    const button = screen.getByTestId("marketing-nav-resources");
    const panel = screen.getByTestId("marketing-nav-resources-panel");
    expect(panel).not.toBeVisible();
    // The current page is inside the dropdown, so its button carries the weight.
    expect(button.className).toContain("font-semibold");
    fireEvent.click(button);
    expect(panel).toBeVisible();
    expect(within(panel).getByRole("link", { name: "Docs" })).toHaveAttribute("href", "/docs");
    expect(within(panel).getByRole("link", { name: "Blog" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(within(panel).getByRole("link", { name: "ROI calculator" })).toHaveAttribute(
      "href",
      "/pricing#roi",
    );
    expect(within(panel).getByRole("link", { name: "All resources" })).toHaveAttribute(
      "href",
      "/resources",
    );
    // Escape returns focus to this dropdown's button, not the Solutions one.
    fireEvent.keyDown(document, { key: "Escape" });
    expect(panel).not.toBeVisible();
    expect(button).toHaveFocus();
  });

  it("marks the current page, including a section's sub-pages", () => {
    expect(isCurrent("/product", "/product")).toBe(true);
    expect(isCurrent("/resources", "/resources/matching-rules-explained")).toBe(true);
    expect(isCurrent("/", "/product")).toBe(false);
    expect(isCurrent("/", "/")).toBe(true);
    expect(isCurrent("/pricing", "/pricingx")).toBe(false);
    expect(isCurrent("/pricing#roi", "/pricing")).toBe(false);

    render(<MarketingNav />);
    expect(screen.getByTestId("marketing-nav-product")).toHaveAttribute("aria-current", "page");
    expect(screen.getByTestId("marketing-nav-docs")).not.toHaveAttribute("aria-current");
  });

  it("opens and closes the small-screen menu", () => {
    render(<MarketingNav />);
    const button = screen.getByTestId("marketing-menu-button");
    const panel = screen.getByTestId("marketing-menu-panel");
    expect(button).toHaveAttribute("aria-expanded", "false");
    expect(panel).not.toBeVisible();

    fireEvent.click(button);
    expect(button).toHaveAttribute("aria-expanded", "true");
    expect(panel).toBeVisible();
    expect(within(panel).getByRole("link", { name: "Product" })).toHaveAttribute(
      "aria-current",
      "page",
    );

    fireEvent.keyDown(document, { key: "Escape" });
    expect(button).toHaveAttribute("aria-expanded", "false");
    expect(button).toHaveFocus();

    fireEvent.click(button);
    fireEvent.click(document.body);
    expect(button).toHaveAttribute("aria-expanded", "false");
  });

  it("offers Login and Book a demo in the header, and no theme control", () => {
    render(<MarketingHeader />);
    expect(screen.getByTestId("marketing-cta-header")).toHaveTextContent("Login");
    expect(screen.getByTestId("marketing-cta-header")).toHaveAttribute("href", "/app");
    expect(screen.getByTestId("marketing-cta-demo")).toHaveTextContent("Book a demo");
    expect(screen.getByTestId("marketing-cta-demo")).toHaveAttribute("href", "/contact");
    expect(screen.queryByRole("button", { name: /mode|theme/i })).toBeNull();
    // Below 640px the header's Login button is hidden; the menu carries it.
    expect(screen.getByTestId("marketing-menu-login")).toHaveAttribute("href", "/app");
  });

  it("renders the footer's columns and keeps Sign in", () => {
    render(<MarketingFooter />);
    const footer = screen.getByTestId("marketing-footer");
    for (const col of FOOTER_COLUMNS) {
      const nav = within(footer).getByRole("navigation", { name: `${col.heading} links` });
      expect(within(nav).getAllByRole("link")).toHaveLength(col.links.length);
    }
    expect(within(footer).getByRole("link", { name: "Sign in" })).toHaveAttribute("href", "/app");
  });
});

describe("the contact form", () => {
  it("validates name, email and message length", () => {
    expect(validateContact({ name: "", email: "", message: "" })).toEqual({
      name: "Enter your name.",
      email: "Enter your work email.",
    });
    expect(validateContact({ name: "A", email: "not-an-email", message: "" }).email).toMatch(
      /like name@company.com/,
    );
    expect(validateContact({ name: "A", email: "a@b.co", message: "x".repeat(2_001) })).toEqual({
      message: "Keep the message under 2,000 characters.",
    });
    expect(validateContact({ name: "A", email: "a@b.co", message: "hi" })).toEqual({});
  });

  it("shows errors inline and focuses the first one", () => {
    render(<ContactPage />);
    fireEvent.click(screen.getByTestId("contact-submit"));
    const name = screen.getByTestId("contact-name");
    expect(name).toHaveAttribute("aria-invalid", "true");
    expect(name).toHaveFocus();
    expect(screen.getByText("Enter your work email.")).toBeInTheDocument();
    expect(screen.getByTestId("contact-status")).toHaveTextContent("");
  });

  it("hands a valid message to the email app and says so", () => {
    render(<ContactPage />);
    fireEvent.change(screen.getByTestId("contact-name"), { target: { value: "Sam" } });
    fireEvent.change(screen.getByTestId("contact-email"), { target: { value: "sam@co.com" } });
    fireEvent.click(screen.getByLabelText("Security review"));
    fireEvent.click(screen.getByTestId("contact-submit"));
    expect(screen.getByTestId("contact-status")).toHaveTextContent("write to sales@dupezero.com");
  });
});

describe("the blog", () => {
  it("lists every post and renders each one in full at an indexed path", async () => {
    render(<BlogPage />);
    for (const post of POSTS) {
      expect(screen.getByText(post.title)).toBeInTheDocument();
    }
    expect(blogMeta.alternates?.canonical).toBe("/blog");
    cleanup();

    for (const { slug } of blogParams()) {
      expect(MARKETING_PATHS).toContain(`/blog/${slug}`);
      const params = Promise.resolve({ slug });
      const meta = await postMeta({ params });
      expect(meta.robots).toBeUndefined();
      const { unmount } = render(await BlogPostPage({ params }));
      expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
      expect(document.body).toHaveTextContent("20 November 2026");
      expect(document.body).not.toHaveTextContent("TODO");
      unmount();
    }
    await expect(BlogPostPage({ params: Promise.resolve({ slug: "nope" }) })).rejects.toThrow(
      "notFound",
    );
  });

  it("formats post dates in UTC", () => {
    expect(formatPostDate("2026-01-01")).toBe("1 January 2026");
  });
});
