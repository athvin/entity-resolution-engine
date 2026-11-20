import {
  CardGrid,
  CtaBand,
  Note,
  Page,
  PageIntro,
  pageMetadata,
  Related,
  Section,
} from "@/components/marketing/pages/page-kit";

export const dynamic = "force-static";

export const metadata = pageMetadata({
  path: "/integrations",
  title: "Integrations",
  description:
    "How records get into DupeZero and how clean records get back to your systems: the Salesforce AppExchange app, API connections, CSV and signed webhooks.",
});

export default function IntegrationsPage() {
  return (
    <Page testId="page-integrations">
      <PageIntro
        eyebrow="Integrations"
        title="Records in, clean records back."
        lead="DupeZero connects to the systems that hold your records, resolves the duplicates, and writes the clean records back, so every system ends up with the same data."
        primary={{ href: "/contact", label: "Ask about your systems" }}
        secondary={{ href: "/docs", label: "Read the developer docs" }}
      />

      <Section id="in" title="Bringing records in">
        <CardGrid
          items={[
            {
              title: "Salesforce",
              body: "Install the DupeZero app from the Salesforce AppExchange to connect your org. DupeZero pulls the objects and fields you map.",
            },
            {
              title: "Other systems, by API",
              body: "Connect other systems through their APIs, and each one becomes a source in a dataset.",
            },
            {
              title: "CSV upload",
              body: "Upload a CSV file as a source and map its columns to fields, for a one-off list or a system without an API.",
            },
          ]}
        />
      </Section>

      <Section id="out" title="Sending clean records back">
        <CardGrid
          items={[
            {
              title: "Back to the source systems",
              body: "Once records are resolved, DupeZero writes the clean records back to the systems they came from: your Salesforce org through the app, and your other connected systems through their APIs.",
            },
            {
              title: "CSV download",
              body: "Download a filtered set of golden records as CSV, up to 10,000 per download.",
            },
            {
              title: "Webhooks",
              body: "DupeZero calls a URL you register when events happen: a run finishes or fails, a configuration is published, a review is resolved, or a record is created, merged or split. Each call is signed so you can check it came from us.",
            },
          ]}
        />
      </Section>

      <Section id="salesforce" title="Salesforce">
        <Note title="How Salesforce fits">
          The DupeZero app connects your org. DupeZero pulls the records in, you review the close
          calls, and the clean records are written back to your org, so Salesforce and your other
          systems hold the same deduplicated data.
        </Note>
      </Section>

      <CtaBand
        title="Have a system we have not listed?"
        body="Tell us what you use and how records leave it."
        primary={{ href: "/contact", label: "Contact us" }}
      />
      <Related
        links={[
          { href: "/product#how-it-works", label: "How it works" },
          { href: "/features/golden-records", label: "Golden records" },
          { href: "/docs", label: "Developer docs" },
        ]}
      />
    </Page>
  );
}
