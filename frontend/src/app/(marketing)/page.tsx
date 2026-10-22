import type { Metadata } from "next";

import { ClosingCta } from "@/components/marketing/cta";
import { Faq } from "@/components/marketing/faq";
import { Hero } from "@/components/marketing/hero";
import { HowItWorks } from "@/components/marketing/how-it-works";
import { Problem } from "@/components/marketing/problem";
import { Proof } from "@/components/marketing/proof";
import { StructuredData } from "@/components/marketing/structured-data";
import { HERO } from "@/components/marketing/content";

/**
 * `force-static`, not `ensureStatic`.
 *
 * Next 16.4's `ensureStatic` export is the natural way to *assert* that a route
 * must stay static, but it only works with `cacheComponents` enabled and the
 * **build fails** without it — `next.config.ts` does not set it. The legacy
 * segment config is what applies here.
 *
 * This is more than a hint. `force-static` makes `cookies()`, `headers()` and
 * `useSearchParams()` return empty values, which turns "this page has no
 * dynamic data" from an intention into a structural property: a future edit
 * that reaches for the session cannot silently make the landing page
 * server-rendered per request.
 */
export const dynamic = "force-static";

/**
 * `title.absolute` escapes the root layout's `%s · DupeZero` template. The
 * landing page's title is the positioning sentence and has to be the whole of
 * it — "Home · DupeZero" would waste the one line a search result shows.
 */
export const metadata: Metadata = {
  title: { absolute: HERO.title },
  description: HERO.subhead,
  alternates: { canonical: "/" },
  openGraph: { title: HERO.title, description: HERO.subhead, url: "/" },
};

export default function LandingPage() {
  return (
    <>
      <StructuredData />
      <Hero />
      <Problem />
      <HowItWorks />
      <Proof />
      <Faq />
      <ClosingCta />
    </>
  );
}
