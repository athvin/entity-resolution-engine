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
    "How records get into and out of DupeZero today: CSV upload, CSV download and signed webhooks.",
});

export default function IntegrationsPage() {
  return (
    <Page testId="page-integrations">
      <PageIntro
        eyebrow="Integrations"
        title="What connects today."
        lead="This page lists only the ways in and out that work now. If you need something that is not here, ask us."
        primary={{ href: "/contact", label: "Ask about your systems" }}
        secondary={{ href: "/docs", label: "Read the developer docs" }}
      />

      <Section id="in" title="Bringing records in">
        <CardGrid
          items={[
            {
              title: "CSV upload",
              body: "Upload a CSV file as a source in a dataset and map its columns to fields. Any system that can export CSV, Salesforce included, can feed DupeZero this way.",
            },
          ]}
        />
      </Section>

      <Section id="out" title="Sending results out">
        <CardGrid
          items={[
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
        <Note title="How Salesforce fits today">
          Export from Salesforce as CSV, resolve in DupeZero, and apply the result with your own
          import or merge tools. DupeZero does not connect to your org or change records in it.
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
