import Link from "next/link";

import {
  Bullets,
  Page,
  PageIntro,
  pageMetadata,
  Section,
} from "@/components/marketing/pages/page-kit";

export const dynamic = "force-static";

/**
 * Plain-language terms written from how the product works today (accounts by
 * invitation, suspension rather than deletion, AI off by default). Not
 * reviewed by counsel, so the page stays unlisted and noindex.
 */
export const metadata = pageMetadata({
  path: "/legal/terms",
  title: "Terms of service",
  description:
    "The terms for using DupeZero: accounts, your data, acceptable use, AI features and fees.",
  unlisted: true,
});

export default function TermsPage() {
  return (
    <Page testId="page-legal-terms">
      <PageIntro
        eyebrow="Legal · Last updated 20 November 2026"
        title="Terms of service"
        lead="These terms apply when you or your organisation use DupeZero. If your organisation has signed a separate agreement with us, that agreement applies where it differs from these terms."
      />

      <Section id="accounts" title="Accounts">
        <Bullets
          items={[
            "Accounts are created by invitation. Your organisation's admins decide who is invited and which role each person has.",
            "Keep your password to yourself. You are responsible for what is done with your account.",
            "Tell us straight away if you think someone else has used your account.",
          ]}
        />
      </Section>

      <Section id="data" title="Your data">
        <Bullets
          items={[
            "Your organisation keeps ownership of the records it sends to DupeZero and the results DupeZero produces from them.",
            "Your organisation must have the right to send us the records it sends, and must not send data we have not agreed in writing to hold.",
            "We use your organisation's records only to provide the service to it, as described in our privacy policy.",
          ]}
        />
      </Section>

      <Section id="use" title="Acceptable use">
        <Bullets
          items={[
            "Do not use DupeZero for anything unlawful, or to process data you are not allowed to process.",
            "Do not try to reach another organisation's data, test our security without our written permission, or disrupt the service.",
            "Do not resell access to DupeZero without our agreement.",
          ]}
        />
        <p className="page-section-intro">
          We may suspend an organisation that breaks these rules. We will tell you why, unless the
          law prevents it.
        </p>
      </Section>

      <Section id="ai" title="AI features">
        <p className="page-section-intro">
          AI features are off until your organisation turns them on. An AI recommendation is a
          suggestion for a person to accept or reject; DupeZero never merges records on an AI
          recommendation alone. Check suggestions before relying on them.
        </p>
      </Section>

      <Section id="fees" title="Fees">
        <p className="page-section-intro">
          DupeZero is priced per Salesforce org, with tiers by the number of records included and
          every feature in every tier. The fees for your organisation are the ones in your order
          with us; current tiers are on our{" "}
          <Link href="/pricing" className="story-text-link">
            pricing page
          </Link>
          .
        </p>
      </Section>

      <Section id="changes" title="Changes to the service and to these terms">
        <p className="page-section-intro">
          We improve DupeZero continually, and features may change. If we change these terms in a
          way that matters, we will tell your organisation's admins before the change takes effect
          and update the date at the top of this page.
        </p>
      </Section>

      <Section id="ending" title="Ending your use">
        <p className="page-section-intro">
          Your organisation can stop using DupeZero at any time. Write to us if you want your
          organisation's records removed, and we will confirm what will be removed and when.
        </p>
      </Section>

      <Section id="contact" title="Contact">
        <p className="page-section-intro">
          Questions about these terms go through our{" "}
          <Link href="/contact" className="story-text-link">
            contact page
          </Link>
          .
        </p>
      </Section>
    </Page>
  );
}
