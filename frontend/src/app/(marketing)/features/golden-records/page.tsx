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
import { ProductScreenshot } from "@/components/marketing/pages/examples";
import { ProvenanceExplorer } from "@/components/marketing/provenance-explorer";
import { SceneBeats, ScrollScene } from "@/components/marketing/scene/scroll-scene";
import { SurvivorshipStage } from "@/components/marketing/scene/survivorship-stage";
import { WayBack } from "@/components/marketing/scene/way-back";

export const dynamic = "force-static";

export const metadata = pageMetadata({
  path: "/features/golden-records",
  title: "Golden records",
  description:
    "One record per person or company, built by field rules you choose, traceable to every source record behind it, and written back to the systems it came from.",
});

/**
 * The scene's beats: the problem, then one field per beat in the order the
 * rules are listed below. Every rule named is one of those rules.
 */
const ASSEMBLED_BEATS = [
  {
    title: "Three records, three answers.",
    body: "Salesforce, a CSV upload and the API each hold Meridian Supply, and they disagree on the email, the phone, the name and the address.",
  },
  {
    title: "Email: the most recent value.",
    body: "The API record changed most recently, so its email wins. The others stay in the history.",
  },
  {
    title: "Phone: the most common value.",
    body: "Two of the three hold the same number once it is cleaned, so that number wins.",
  },
  {
    title: "Name: the source you trust most.",
    body: "You trust Salesforce for the company name, so its spelling wins and the others stay linked.",
  },
  {
    title: "Address: fields that belong together.",
    body: (
      <>
        Number, street and suite come from one record, the most complete. Every value now says where
        it came from.{" "}
        <a className="story-text-link" href="#rules">
          All the rules
        </a>
      </>
    ),
  },
];

export default function GoldenRecordsPage() {
  return (
    <Page
      testId="page-golden-records"
      crumb={{ path: "/features/golden-records", label: "Golden records" }}
    >
      <PageIntro
        morph="/features/golden-records"
        eyebrow="Feature / Golden records"
        title="One record you can trust, built from the ones you have."
        lead="Finding duplicates is half the job. The other half is deciding which email, phone number and address the surviving record should carry. DupeZero does that field by field, by rules you set."
        primary={DEMO}
        secondary={{ href: "/integrations", label: "See integrations" }}
      />

      <Section
        id="assembled"
        eyebrow="Watch one being built"
        title="One golden record, value by value."
        intro={<p>Scroll through how each field picks its value. The records are illustrative.</p>}
      >
        <ScrollScene
          name="survivorship"
          beats={ASSEMBLED_BEATS.length}
          stage={<SurvivorshipStage />}
        >
          <SceneBeats beats={ASSEMBLED_BEATS} />
        </ScrollScene>
      </Section>

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
      >
        <ProvenanceExplorer />
        <ProductScreenshot screen="golden" />
      </Section>

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
      >
        <WayBack />
      </Section>

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
