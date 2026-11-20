import {
  CardGrid,
  CtaBand,
  DEMO,
  Note,
  Page,
  PageIntro,
  pageMetadata,
  Related,
  Section,
  Steps,
} from "@/components/marketing/pages/page-kit";
import { ExampleReviewPair } from "@/components/marketing/pages/examples";

export const dynamic = "force-static";

export const metadata = pageMetadata({
  path: "/product",
  title: "Product overview",
  description:
    "DupeZero finds records that describe the same person or company, shows the evidence behind each match, and gives you one trusted record to work from.",
});

export default function ProductPage() {
  return (
    <Page testId="page-product">
      <PageIntro
        eyebrow="Product"
        title="One record for every customer, and the reasons behind it."
        lead="DupeZero finds the records that describe the same person or company, shows you why it thinks so, and builds one trusted record from them. It is made for the people who look after CRM data: Salesforce admins and revenue operations teams."
        primary={DEMO}
        secondary={{ href: "/how-it-works", label: "See how it works" }}
      />

      <Section
        id="problem"
        title="Duplicates cost more than storage."
        intro={
          <p>
            When one customer lives in three records, their calls, emails and opportunities are
            split between them. Two reps work the same lead. Reports count one company twice.
            Campaigns reach the same person more than once, or miss them because the record that was
            filtered is the incomplete one.
          </p>
        }
      />

      <Section id="what" title="What DupeZero does">
        <CardGrid
          items={[
            {
              title: "Matching",
              body: "Tidies names, emails, phone numbers and addresses, then compares the fields you choose, with rules you control and a model that learns from your data.",
              href: "/features/matching",
            },
            {
              title: "Review",
              body: "Sends the uncertain pairs to a queue, with the evidence for and against each match. A person makes the call.",
              href: "/features/review",
            },
            {
              title: "Golden records",
              body: "Builds one record per person or company, with rules for which value wins, and keeps every source record it came from.",
              href: "/features/golden-records",
            },
            {
              title: "Integrations",
              body: "Brings records in from CSV files and sends results out as CSV downloads and signed webhooks.",
              href: "/integrations",
            },
          ]}
        />
      </Section>

      <Section id="flow" title="How a cleanup runs">
        <Steps
          items={[
            {
              title: "Upload",
              body: "Export your records as CSV and upload them to a dataset of people or companies.",
            },
            {
              title: "Match",
              body: "Set the fields to compare and how confident a match must be before records join automatically.",
            },
            {
              title: "Review",
              body: "Decide the close calls in a queue that shows the evidence for each pair.",
            },
            {
              title: "Export",
              body: "Download the golden records, or let your systems hear about changes by webhook.",
            },
          ]}
        />
        <ExampleReviewPair label="The DupeZero review queue, showing a pair of records and the fields that agree and disagree" />
      </Section>

      <Section id="know" title="Before you start">
        <Note title="How data moves today">
          DupeZero works from CSV files. You export records from Salesforce, or any other system,
          and download the results as CSV. It does not connect to your Salesforce org or change
          records there, so applying the result in Salesforce is a step your team runs.
        </Note>
      </Section>

      <CtaBand
        title="See it with your own records."
        body="We will walk through a cleanup with a sample of your data."
        primary={DEMO}
        secondary={{ href: "/pricing", label: "See pricing" }}
      />
      <Related
        links={[
          { href: "/how-it-works", label: "How it works" },
          { href: "/solutions", label: "Solutions by role" },
          { href: "/security", label: "Security and data handling" },
          { href: "/faq", label: "FAQ" },
        ]}
      />
    </Page>
  );
}
