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
import { ExampleMapping, ExampleReviewPair } from "@/components/marketing/pages/examples";
import { CONNECT_NOTE } from "@/components/marketing/pages/solutions-content";

export const dynamic = "force-static";

/**
 * The product overview and the step-by-step walkthrough, on one page. The old
 * /how-it-works URL redirects to #how-it-works here (next.config.ts).
 */
export const metadata = pageMetadata({
  path: "/product",
  title: "Product",
  description:
    "What DupeZero does and how it works, step by step: connect your systems, resolve the duplicates, review the close calls, and send clean records back to every system.",
});

export default function ProductPage() {
  return (
    <Page testId="page-product">
      <PageIntro
        eyebrow="Product"
        title="One record for every customer, and the reasons behind it."
        lead="DupeZero finds the records that describe the same person or company, shows you why it thinks so, builds one trusted record from them, and sends that clean record back to the systems it came from. It is made for the people who look after CRM data: Salesforce admins and revenue operations teams."
        primary={DEMO}
        secondary={{ href: "#how-it-works", label: "See how it works" }}
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
              body: "Brings records in from your Salesforce org and other connected systems, writes the clean records back to them, and also offers CSV downloads and signed webhooks.",
              href: "/integrations",
            },
          ]}
        />
      </Section>

      <Section
        id="how-it-works"
        eyebrow="How it works"
        title="From your records to a clean one, step by step."
      >
        <Steps
          items={[
            {
              title: "Create a dataset",
              body: "A dataset holds one kind of record, people or companies, with its own sources, rules and results. Keep separate cleanups in separate datasets.",
            },
            {
              title: "Bring your records in",
              body: "Connect your Salesforce org with the DupeZero app, connect another system through its API, or upload a CSV file. Each one becomes a source in the dataset.",
            },
            {
              title: "Map the fields",
              body: "Tell DupeZero which field is which: names, emails, phone numbers, addresses, company names and websites. If your organisation has turned on AI features, it can suggest the mapping for you to check.",
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
              title: "Send clean data back",
              body: "DupeZero writes the resolved records back to the systems they came from, so your Salesforce org and your other connected systems all hold the same clean data. You can also download golden records as CSV, or have DupeZero call your systems by webhook when runs finish and records change.",
            },
          ]}
        />
        <ExampleMapping label="Example of mapping a new source: each field in your data is assigned a DupeZero field" />
        <ExampleReviewPair label="Example of a pair in the review queue, showing the fields that agree and disagree" />
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
        <Note title={CONNECT_NOTE.title}>{CONNECT_NOTE.body}</Note>
      </Section>

      <CtaBand
        title="See it with your own records."
        body="We will walk through a cleanup with a sample of your data."
        primary={DEMO}
        secondary={{ href: "/pricing", label: "See pricing" }}
      />
      <Related
        links={[
          { href: "/features/matching", label: "How matching works" },
          { href: "/solutions", label: "Solutions by role" },
          { href: "/security", label: "Security and data handling" },
          { href: "/faq", label: "FAQ" },
        ]}
      />
    </Page>
  );
}
