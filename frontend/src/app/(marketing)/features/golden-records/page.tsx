import {
  Bullets,
  CtaBand,
  DEMO,
  Page,
  PageIntro,
  pageMetadata,
  Related,
  Section,
} from "@/components/marketing/pages/page-kit";

export const dynamic = "force-static";

export const metadata = pageMetadata({
  path: "/features/golden-records",
  title: "Golden records",
  description:
    "One record per person or company, built by field rules you choose, traceable to every source record behind it, and written back to the systems it came from.",
});

export default function GoldenRecordsPage() {
  return (
    <Page testId="page-golden-records">
      <PageIntro
        eyebrow="Feature / Golden records"
        title="One record you can trust, built from the ones you have."
        lead="Finding duplicates is half the job. The other half is deciding which email, phone number and address the surviving record should carry. DupeZero does that field by field, by rules you set."
        primary={DEMO}
        secondary={{ href: "/integrations", label: "See integrations" }}
      />

      <Section
        id="rules"
        title="Choose which value wins."
        intro={<p>Each field can follow its own rule. Among them:</p>}
      >
        <Bullets
          items={[
            "The most recent value, or the most common one.",
            "The most complete value, or the longest or shortest.",
            "The value from the source you trust most.",
            "The highest or lowest number.",
            "Every value combined, for fields that hold lists.",
            "Fields that belong together, such as the parts of an address, taken from the same record.",
          ]}
        />
      </Section>

      <Section
        id="trace"
        title="Every record keeps its sources."
        intro={
          <p>
            Open a golden record to see each source record behind it and its history of changes:
            when records joined and when one was split out. The original values stay attached.
          </p>
        }
      />

      <Section
        id="out"
        title="Clean records go back where they came from."
        intro={
          <>
            <p>
              Golden records are written back to the systems they came from, so your Salesforce org
              and your other connected systems all hold the same deduplicated data.
            </p>
            <p>
              You can also download a filtered set of golden records as CSV, up to 10,000 per
              download, or have DupeZero notify your systems by signed webhook when records are
              created, merged or split.
            </p>
          </>
        }
      />

      <CtaBand
        title="See your golden records."
        body="We will build them from a sample of your data."
        primary={DEMO}
      />
      <Related
        links={[
          { href: "/features/matching", label: "Matching" },
          { href: "/features/review", label: "Review" },
          { href: "/integrations", label: "Integrations" },
        ]}
      />
    </Page>
  );
}
