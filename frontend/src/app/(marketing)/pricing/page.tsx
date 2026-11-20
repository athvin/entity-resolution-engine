import type { Metadata } from "next";

import { PRICING } from "@/components/marketing/content";
import {
  Breadcrumbs,
  JsonLd,
  LastUpdated,
  ORGANIZATION_ID,
  PAGE_UPDATED,
  SHARE_IMAGE,
  SOFTWARE_ID,
} from "@/components/marketing/seo";
import { PRICING_CONFIG } from "@/lib/pricing/config";
import { canonicalUrl } from "@/lib/site/routes";
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
    images: [SHARE_IMAGE],
  },
  twitter: {
    card: "summary_large_image",
    title: "DupeZero pricing",
    description: PRICING.metaDescription,
    images: [SHARE_IMAGE.url],
  },
};

/**
 * The tiers' annual list prices, as the page's own table shows them (cents to
 * dollars). Built from PRICING_CONFIG, so the offer cannot drift from the page.
 */
const annual = PRICING_CONFIG.tiers.map((t) => t.annualCents / 100);

export default function PricingPage() {
  return (
    <div className="pricing-page" data-testid="pricing-page">
      <Breadcrumbs path="/pricing" label="Pricing" />
      <JsonLd
        data={{
          "@type": "SoftwareApplication",
          "@id": SOFTWARE_ID,
          name: "DupeZero",
          applicationCategory: "BusinessApplication",
          operatingSystem: "Web",
          url: canonicalUrl("/pricing"),
          publisher: { "@id": ORGANIZATION_ID },
          offers: {
            "@type": "AggregateOffer",
            priceCurrency: PRICING_CONFIG.currency,
            lowPrice: Math.min(...annual),
            highPrice: Math.max(...annual),
            offerCount: annual.length,
            url: canonicalUrl("/pricing"),
          },
        }}
      />
      <header className="pricing-intro">
        <p className="story-eyebrow">{PRICING.eyebrow}</p>
        <h1>{PRICING.heading}</h1>
        <p className="story-muted">{PRICING.intro}</p>
        <LastUpdated iso={PAGE_UPDATED["/pricing"] ?? ""} />
      </header>
      <PricingCalculator />
    </div>
  );
}
