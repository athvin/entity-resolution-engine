import type { Metadata } from "next";

import { PRICING } from "@/components/marketing/content";
import { PricingCalculator } from "@/components/marketing/pricing/pricing-calculator";
import "@/components/marketing/pricing/pricing.css";

/** Static like every marketing page: the calculator runs entirely in the browser. */
export const dynamic = "force-static";

export const metadata: Metadata = {
  title: "Pricing",
  description: PRICING.metaDescription,
  alternates: { canonical: "/pricing" },
  openGraph: {
    title: "DupeZero pricing",
    description: PRICING.metaDescription,
    url: "/pricing",
  },
};

export default function PricingPage() {
  return (
    <div className="pricing-page" data-testid="pricing-page">
      <header className="pricing-intro">
        <p className="story-eyebrow">{PRICING.eyebrow}</p>
        <h1>{PRICING.heading}</h1>
        <p className="story-muted">{PRICING.intro}</p>
      </header>
      <PricingCalculator />
    </div>
  );
}
