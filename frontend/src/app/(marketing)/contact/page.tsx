import { ContactForm } from "@/components/marketing/pages/contact-form";
import { Page, PageIntro, pageMetadata, Section } from "@/components/marketing/pages/page-kit";

export const dynamic = "force-static";

export const metadata = pageMetadata({
  path: "/contact",
  title: "Contact and demo requests",
  description:
    "Request a DupeZero demo or ask a question about deduplicating your Salesforce records.",
});

export default function ContactPage() {
  return (
    <Page testId="page-contact">
      <PageIntro
        eyebrow="Contact"
        title="Request a demo, or ask us anything."
        lead="Tell us a little about your records and what you want to fix. We will reply by email."
      />
      <Section id="form" title="Send us a message">
        <ContactForm />
      </Section>
    </Page>
  );
}
