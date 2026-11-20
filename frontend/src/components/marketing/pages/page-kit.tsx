import type { Metadata } from "next";
import Link from "next/link";

import "./pages.css";
import { DEMO_PATH } from "../site-links";

import { Button } from "@/components/ui/button";

/**
 * Building blocks for the marketing site's content pages (product, features,
 * solutions, security and the rest). Server components only, so every page
 * prerenders to static HTML like the rest of the `(marketing)` group.
 *
 * Copy lives in each page file. Where a page would otherwise need a fact we do
 * not have (a founding year, a customer quote, a certification), the copy is
 * written so it does not need one, rather than inventing it.
 */

/** Title, description, canonical URL and Open Graph for one page. */
export function pageMetadata({
  path,
  title,
  description,
  unlisted = false,
}: {
  path: string;
  title: string;
  description: string;
  /** Placeholder pages: reachable, but asked not to be indexed. */
  unlisted?: boolean;
}): Metadata {
  return {
    title,
    description,
    alternates: { canonical: path },
    openGraph: { title: `${title} · DupeZero`, description, url: path },
    ...(unlisted ? { robots: { index: false, follow: true } } : {}),
  };
}

export interface Action {
  href: string;
  label: string;
}

function ActionLink({ action, variant }: { action: Action; variant: "default" | "outline" }) {
  // mailto and absolute links are not routes, so they get a plain anchor.
  const internal = action.href.startsWith("/");
  return (
    <Button asChild size="lg" variant={variant}>
      {internal ? (
        <Link href={action.href}>{action.label}</Link>
      ) : (
        <a href={action.href}>{action.label}</a>
      )}
    </Button>
  );
}

export function Actions({
  primary,
  secondary,
}: {
  primary: Action;
  secondary?: Action | undefined;
}) {
  return (
    <div className="page-actions">
      <ActionLink action={primary} variant="default" />
      {secondary && <ActionLink action={secondary} variant="outline" />}
    </div>
  );
}

export function PageIntro({
  eyebrow,
  title,
  lead,
  primary,
  secondary,
}: {
  eyebrow: string;
  title: string;
  lead: string;
  primary?: Action;
  secondary?: Action | undefined;
}) {
  return (
    <header className="page-intro">
      <p className="story-eyebrow">{eyebrow}</p>
      <h1>{title}</h1>
      <p className="page-lead story-muted">{lead}</p>
      {primary && <Actions primary={primary} secondary={secondary} />}
    </header>
  );
}

export function Section({
  id,
  eyebrow,
  title,
  intro,
  children,
}: {
  id?: string;
  eyebrow?: string;
  title: string;
  intro?: React.ReactNode;
  children?: React.ReactNode;
}) {
  const headingId = id ? `${id}-title` : undefined;
  return (
    <section id={id} className="page-section" aria-labelledby={headingId}>
      {eyebrow && <p className="story-eyebrow">{eyebrow}</p>}
      <h2 id={headingId}>{title}</h2>
      {intro && <div className="page-section-intro story-muted">{intro}</div>}
      {children}
    </section>
  );
}

export interface Card {
  title: string;
  body: React.ReactNode;
  href?: string;
  linkLabel?: string;
}

export function CardGrid({ items }: { items: readonly Card[] }) {
  return (
    <ul className="page-cards">
      {items.map((c) => (
        <li key={c.title}>
          <h3>{c.title}</h3>
          <div className="story-muted">{c.body}</div>
          {c.href && (
            <Link href={c.href} className="story-text-link page-card-link">
              {c.linkLabel ?? `More about ${c.title.toLowerCase()}`}
              <span aria-hidden="true"> →</span>
            </Link>
          )}
        </li>
      ))}
    </ul>
  );
}

export function Steps({ items }: { items: readonly { title: string; body: React.ReactNode }[] }) {
  return (
    <ol className="page-steps">
      {items.map((s, i) => (
        <li key={s.title}>
          <span className="page-step-number" aria-hidden="true">
            {String(i + 1).padStart(2, "0")}
          </span>
          <div>
            <h3>{s.title}</h3>
            <div className="story-muted">{s.body}</div>
          </div>
        </li>
      ))}
    </ol>
  );
}

export function Bullets({ items }: { items: readonly React.ReactNode[] }) {
  return (
    <ul className="page-bullets">
      {items.map((b, i) => (
        <li key={i}>{b}</li>
      ))}
    </ul>
  );
}

/** A plainly stated limit or caveat, so a feature is never implied to do more. */
export function Note({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <aside className="page-note" aria-label={title}>
      <p className="page-note-title">{title}</p>
      <div className="story-muted">{children}</div>
    </aside>
  );
}

/** Visible marker for content the owner still has to supply. */
export function CtaBand({
  title,
  body,
  primary,
  secondary,
}: {
  title: string;
  body: string;
  primary: Action;
  secondary?: Action | undefined;
}) {
  return (
    <section className="page-cta" aria-labelledby="page-cta-title">
      <div>
        <h2 id="page-cta-title">{title}</h2>
        <p className="story-muted">{body}</p>
      </div>
      <Actions primary={primary} secondary={secondary} />
    </section>
  );
}

/** Related pages, for internal linking at the foot of a page. */
export function Related({ links }: { links: readonly Action[] }) {
  return (
    <nav className="page-related" aria-label="Related pages">
      <p className="page-note-title">Related</p>
      <ul>
        {links.map((l) => (
          <li key={l.href}>
            <Link href={l.href} className="story-text-link">
              {l.label}
            </Link>
          </li>
        ))}
      </ul>
    </nav>
  );
}

export function Page({ children, testId }: { children: React.ReactNode; testId: string }) {
  return (
    <div className="page" data-testid={testId}>
      {children}
    </div>
  );
}

/** The demo call to action every page ends on. */
export const DEMO: Action = { href: DEMO_PATH, label: "Request a demo" };
