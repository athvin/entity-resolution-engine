import {
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
import { ExampleMapping } from "@/components/marketing/pages/examples";

export const dynamic = "force-static";

export const metadata = pageMetadata({
  path: "/how-it-works",
  title: "How it works",
  description:
    "From a CSV export to a clean set of golden records: how a DupeZero cleanup runs, step by step.",
});

export default function HowItWorksPage() {
  return (
    <Page testId="page-how-it-works">
      <PageIntro
        eyebrow="How it works"
        title="From an export to a clean record, step by step."
        lead="A DupeZero cleanup starts with a file you already know how to make and ends with records you can explain. Here is every step, in order."
        primary={DEMO}
      />

      <Section id="steps" title="Seven steps">
        <Steps
          items={[
            {
              title: "Export your records",
              body: "Export the Leads, Contacts or Accounts you want to clean as a CSV file, with the fields you want compared: names, emails, phone numbers, addresses, company names and websites.",
            },
            {
              title: "Create a dataset",
              body: "A dataset holds one kind of record, people or companies, with its own sources, rules and results. Keep separate cleanups in separate datasets.",
            },
            {
              title: "Upload and map the columns",
              body: "Upload the file as a source and tell DupeZero which column is which. If your organisation has turned on AI features, it can suggest the mapping for you to check.",
            },
            {
              title: "Set the matching rules",
              body: "Choose the fields to compare and how strictly, any fields that must agree, and two thresholds: the score above which records join automatically, and the band that goes to a person.",
            },
            {
              title: "Run",
              body: "Runs work in the background. Follow their progress, cancel or resume them, and schedule them to repeat. After the first full run, later runs can process only what is new or changed.",
            },
            {
              title: "Review the close calls",
              body: "Pairs between your thresholds wait in a review queue with the evidence for and against each match. Accept, reject, or merge by hand; a wrong merge can be split apart later.",
            },
            {
              title: "Export the result",
              body: "Download golden records as CSV, up to 10,000 per download, or have DupeZero call your systems by webhook when runs finish and records change.",
            },
          ]}
        />
        <ExampleMapping label="Example of mapping a new source: each column in your data is assigned a DupeZero field" />
      </Section>

      <Section
        id="next-run"
        title="The next run remembers."
        intro={
          <p>
            Decisions you make in review are kept. If you record that two records are not the same,
            or always are, that rule holds on every later run and survives retraining of the
            matching model.
          </p>
        }
      >
        <Note title="Applying the result in Salesforce">
          DupeZero does not write to Salesforce. Use the exported records in your own update or
          merge process.
        </Note>
      </Section>

      <CtaBand
        title="Walk through it with your data."
        body="Bring an export. We will run it with you."
        primary={DEMO}
        secondary={{ href: "/features/matching", label: "How matching works" }}
      />
      <Related
        links={[
          { href: "/product", label: "Product overview" },
          { href: "/features/review", label: "Review" },
          { href: "/integrations", label: "Integrations" },
          { href: "/resources", label: "Resources" },
        ]}
      />
    </Page>
  );
}
