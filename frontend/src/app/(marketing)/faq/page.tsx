import Link from "next/link";

import { FAQ_ITEMS } from "@/components/marketing/pages/faq-content";
import {
  CtaBand,
  DEMO,
  Page,
  PageIntro,
  pageMetadata,
  Section,
} from "@/components/marketing/pages/page-kit";

export const dynamic = "force-static";

export const metadata = pageMetadata({
  path: "/faq",
  title: "FAQ",
  description:
    "Answers about DupeZero: how it works with Salesforce, what it deduplicates, how merges are reviewed and undone, AI features, access and pricing.",
});

/**
 * FAQPage JSON-LD from the same array the page renders. A module-level literal
 * with no props, rendered as a text child; see structured-data.tsx for why that
 * is safe without the HTML-injection sink.
 */
const FAQ_STRUCTURED_DATA = {
  "@context": "https://schema.org",
  "@type": "FAQPage",
  mainEntity: FAQ_ITEMS.map((item) => ({
    "@type": "Question",
    name: item.q,
    acceptedAnswer: { "@type": "Answer", text: item.a },
  })),
};

export default function FaqPage() {
  return (
    <Page testId="page-faq" crumb={{ path: "/faq", label: "FAQ" }}>
      <script type="application/ld+json">{JSON.stringify(FAQ_STRUCTURED_DATA)}</script>
      <PageIntro
        eyebrow="FAQ"
        title="Questions, answered plainly."
        lead="What DupeZero does, what it does not do yet, and how it handles your data."
      />
      <Section id="questions" title="Common questions">
        <dl className="page-faq">
          {FAQ_ITEMS.map((item) => (
            <div key={item.q}>
              <dt>{item.q}</dt>
              <dd className="story-muted">
                {item.a}
                {item.link && (
                  <>
                    {" "}
                    <Link href={item.link.href} className="story-text-link">
                      {item.link.label}
                    </Link>
                  </>
                )}
              </dd>
            </div>
          ))}
        </dl>
      </Section>
      <CtaBand
        title="Have a question we did not answer?"
        body="Ask us. We will tell you what DupeZero does today."
        primary={{ href: "/contact", label: "Contact us" }}
        secondary={DEMO}
      />
    </Page>
  );
}
