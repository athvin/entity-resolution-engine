import {
  Bullets,
  CtaBand,
  Page,
  PageIntro,
  pageMetadata,
  Related,
  Section,
} from "@/components/marketing/pages/page-kit";

export const dynamic = "force-static";

export const metadata = pageMetadata({
  path: "/security",
  title: "Security and data handling",
  description:
    "How DupeZero keeps each customer's records separate, encrypts them, controls who can act on them, and records what was done.",
});

export default function SecurityPage() {
  return (
    <Page testId="page-security">
      <PageIntro
        eyebrow="Security"
        title="How we look after your records."
        lead="You are trusting us with customer data. This page says what we do with it, plainly, and what we cannot yet say."
        primary={{ href: "/contact", label: "Ask a security question" }}
      />

      <Section id="separation" title="Your data is kept apart.">
        <Bullets
          items={[
            "Each customer's records are stored in a separate database and a separate storage location.",
            "A request for another organisation's data is refused as if the data did not exist.",
          ]}
        />
      </Section>

      <Section id="encryption" title="Encrypted in transit and at rest.">
        <Bullets
          items={[
            "Connections to DupeZero use TLS, and plain HTTP is redirected to HTTPS.",
            "Stored records, the application database and backups are encrypted.",
            "Keys you give us, such as an AI provider key, are encrypted before they are stored, and are never shown back.",
          ]}
        />
      </Section>

      <Section id="access" title="Only the right people can act.">
        <Bullets
          items={[
            "Three roles in each organisation: viewers read, stewards review and correct, admins manage rules and members.",
            "Passwords are stored as strong one-way hashes. Password reset does not reveal whether an account exists.",
            "Sign-in and password reset are rate-limited to slow down guessing.",
            "Webhook calls are signed, and webhook addresses are checked so they cannot point inside our network.",
          ]}
        />
      </Section>

      <Section id="audit" title="A record of what was done.">
        <Bullets
          items={[
            "Admins can see an audit log of actions in their organisation, kept for at least 400 days.",
            "Every golden record keeps the history of how it was formed and changed.",
            "AI features are off unless your organisation turns them on, and an AI recommendation never acts by itself.",
          ]}
        />
      </Section>

      <Section
        id="not-yet"
        title="What we do not claim"
        intro={
          <>
            <p>
              We hold no security certification today, and this page does not imply one. Single
              sign-on and multi-factor sign-in are not available yet.
            </p>
          </>
        }
      />

      <Section id="processors" title="Who processes data for us.">
        <Bullets
          items={[
            "Amazon Web Services hosts DupeZero, stores its data and backups, and sends its email.",
            "Anthropic provides the model behind the optional AI features. It receives data only if your organisation turns AI on, and you can use your own Anthropic key.",
          ]}
        />
      </Section>

      <Section id="retention" title="Retention and removal.">
        <Bullets
          items={[
            "Your organisation's records are kept while it uses DupeZero.",
            "Audit log entries are kept for at least 400 days.",
            "To have records removed, write to us and we will confirm what will be removed and when.",
          ]}
        />
      </Section>

      <Section
        id="report"
        title="Report a security issue."
        intro={
          <p>
            Use our contact page and choose Security review. Tell us what you found and how to
            reproduce it, and please give us time to fix it before you share it.
          </p>
        }
      />

      <CtaBand
        title="Need more detail for a review?"
        body="Send us your questionnaire and we will answer what we can verify."
        primary={{ href: "/contact", label: "Contact us" }}
      />
      <Related
        links={[
          { href: "/legal/privacy", label: "Privacy" },
          { href: "/features/review", label: "Review and roles" },
          { href: "/faq", label: "FAQ" },
        ]}
      />
    </Page>
  );
}
