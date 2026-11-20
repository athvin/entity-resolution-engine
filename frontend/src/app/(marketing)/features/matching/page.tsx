import {
  Bullets,
  CardGrid,
  CtaBand,
  DEMO,
  Note,
  Page,
  PageIntro,
  pageMetadata,
  Related,
  Section,
} from "@/components/marketing/pages/page-kit";

export const dynamic = "force-static";

export const metadata = pageMetadata({
  path: "/features/matching",
  title: "Matching",
  description:
    "How DupeZero decides two records are the same person or company: value cleanup, configurable comparisons, hard rules, and a model trained on your data.",
});

export default function MatchingPage() {
  return (
    <Page testId="page-matching">
      <PageIntro
        eyebrow="Feature / Matching"
        title="Matching rules you can read, and change."
        lead="Duplicates rarely match exactly. One record says Bob, the other Robert; one email has capitals, the other a plus tag. DupeZero cleans the values first, then compares them the way you tell it to."
        primary={DEMO}
        secondary={{ href: "/features/review", label: "See how review works" }}
      />

      <Section
        id="cleanup"
        title="Clean the values before comparing them."
        intro={
          <p>Each record keeps its original values. The cleaned copies are used to compare.</p>
        }
      >
        <Bullets
          items={[
            "Emails are lower-cased, plus tags are removed, and placeholder addresses are ignored.",
            "Phone numbers are put in one international format.",
            "Names are normalised, and common nicknames are recognised, so Bob can match Robert.",
            "Addresses are split into parts, such as number and street, so they can be compared part by part.",
            "Words you choose to ignore, such as legal suffixes in company names, are dropped.",
          ]}
        />
      </Section>

      <Section id="compare" title="Compare each field the way it deserves.">
        <CardGrid
          items={[
            {
              title: "Exact and near spelling",
              body: "Exact agreement, similar spelling, or a set number of typos. Useful for names and company names.",
            },
            {
              title: "Dates and numbers",
              body: "Dates within a number of days, or with the day and month swapped. Numbers within an amount or a percentage.",
            },
            {
              title: "Places",
              body: "Address parts compared one by one, or two locations within a set distance.",
            },
            {
              title: "Emails",
              body: "Compare the whole address, or just the part before or after the @.",
            },
          ]}
        />
      </Section>

      <Section
        id="rules"
        title="Set rules no score can override."
        intro={
          <p>
            Some decisions should not be left to a score. Require a field to agree before two
            records can match, or rule a pair out whatever else agrees.
          </p>
        }
      />

      <Section
        id="learning"
        title="Let your data set the weights."
        intro={
          <>
            <p>
              Not every agreement means the same. Two records sharing an email says more than two
              sharing a city. DupeZero trains a statistical model on your records to learn how much
              each field tells it, and you can improve it by labelling example pairs as the same or
              different.
            </p>
            <p>
              Rare values count for more: a shared uncommon surname is stronger evidence than a
              shared common one.
            </p>
          </>
        }
      />

      <Section
        id="thresholds"
        title="Decide what happens at each score."
        intro={
          <p>
            Set the score above which records join automatically and the band below it that waits
            for a person. Everything under that band stays separate.
          </p>
        }
      >
        <Note title="Limits worth knowing">
          Phone numbers are standardised for the United States and Canada; other numbers are left
          out of phone comparisons. Matching by how a name sounds is not supported.
        </Note>
      </Section>

      <CtaBand
        title="Try your rules on your records."
        body="We will set up matching with a sample of your data."
        primary={DEMO}
      />
      <Related
        links={[
          { href: "/features/review", label: "Review" },
          { href: "/features/golden-records", label: "Golden records" },
          { href: "/resources/matching-rules-explained", label: "Matching rules, explained" },
        ]}
      />
    </Page>
  );
}
