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
 * Written from the product's verified data handling (docs/feature-inventory.md):
 * the one session cookie, no analytics, AWS for hosting and email, Anthropic
 * only when an organisation turns AI on, and no self-serve deletion. Not
 * reviewed by counsel, so the page stays unlisted and noindex.
 */
export const metadata = pageMetadata({
  path: "/legal/privacy",
  title: "Privacy policy",
  description:
    "What information DupeZero collects, why, who processes it on our behalf, and how to reach us about it.",
  unlisted: true,
});

export default function PrivacyPage() {
  return (
    <Page testId="page-legal-privacy">
      <PageIntro
        eyebrow="Legal · Last updated 20 November 2026"
        title="Privacy policy"
        lead="This policy explains what information DupeZero collects, why we collect it, who helps us process it, and what you can ask us to do with it."
      />

      <Section id="roles" title="Two kinds of information">
        <Bullets
          items={[
            "Account information about the people who use DupeZero. We decide how this is used, and this policy covers it.",
            "Customer records your organisation sends to DupeZero to deduplicate. Your organisation decides what is sent and how it is used; we process those records on its behalf and only to provide the service.",
          ]}
        />
      </Section>

      <Section id="collect" title="What we collect">
        <Bullets
          items={[
            "Your name, work email address and the organisation and role you were invited to.",
            "Your password, stored only as a one-way hash that cannot be turned back into the password.",
            "A record of actions taken in your organisation, such as sign-ins, reviews and rule changes, kept in an audit log your organisation's admins can see.",
            "The customer records, field mappings, rules and review decisions your organisation creates.",
            "Messages you send us. The contact form opens your own email app, so we receive only what you choose to send.",
          ]}
        />
      </Section>

      <Section id="cookies" title="Cookies">
        <p className="page-section-intro">
          DupeZero sets one cookie, which keeps you signed in. It is needed for the service to work.
          Our website uses no advertising or analytics cookies and no third-party tracking.
        </p>
      </Section>

      <Section id="use" title="How we use it">
        <Bullets
          items={[
            "To provide DupeZero: to sign you in, resolve your organisation's records and show the results.",
            "To keep the service secure, including limiting repeated sign-in attempts and recording actions in the audit log.",
            "To send email the service needs, such as invitations and password resets.",
            "To answer your messages.",
          ]}
        />
        <p className="page-section-intro">
          We do not sell personal information, and we do not use your organisation's customer
          records for any purpose other than providing the service to your organisation.
        </p>
      </Section>

      <Section id="processors" title="Who processes it for us">
        <Bullets
          items={[
            "Amazon Web Services hosts DupeZero, stores its data and backups, and sends its email.",
            "Anthropic provides the AI model behind DupeZero's optional AI features. It receives data only if your organisation turns AI on, and your organisation can use its own Anthropic key.",
          ]}
        />
      </Section>

      <Section id="retention" title="How long we keep it">
        <p className="page-section-intro">
          Account information and your organisation's records are kept while your organisation uses
          DupeZero. Audit log entries are kept for at least 400 days. To have information removed,
          write to us and we will confirm what will be removed and when.
        </p>
      </Section>

      <Section id="rights" title="Your choices">
        <p className="page-section-intro">
          You can ask us for a copy of your account information, or to correct or remove it. If your
          question is about customer records your organisation sent us, we will work with your
          organisation, because it decides how those records are used.
        </p>
      </Section>

      <Section id="contact" title="Contact">
        <p className="page-section-intro">
          Questions about this policy go through our{" "}
          <Link href="/contact" className="story-text-link">
            contact page
          </Link>
          . If we change this policy, we will update the date at the top of this page.
        </p>
      </Section>
    </Page>
  );
}
