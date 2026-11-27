import {
  CardGrid,
  CtaBand,
  DEMO,
  Page,
  PageIntro,
  pageMetadata,
  Related,
  Section,
} from "@/components/marketing/pages/page-kit";
import { Reviewer } from "@/components/marketing/scene/reviewer";

export const dynamic = "force-static";

export const metadata = pageMetadata({
  path: "/features/review",
  title: "Review",
  description:
    "Review uncertain matches with the evidence in front of you, merge or split by hand, and record decisions that hold on every later run.",
});

export default function ReviewPage() {
  return (
    <Page testId="page-review" crumb={{ path: "/features/review", label: "Review" }}>
      <PageIntro
        morph="/features/review"
        eyebrow="Feature / Review"
        title="The close calls go to a person, with the evidence."
        lead="Automatic merging is only safe for the clear cases. For the rest, DupeZero puts each pair in front of someone who knows the data, along with the reasons it might or might not be a match."
        primary={DEMO}
      />

      <Section
        id="decide"
        eyebrow="Try it"
        title="Make the call yourself."
        intro={
          <p>
            One pair from the review band, with its evidence. Decide it, then change your mind and
            watch the correction stick. The records are illustrative.
          </p>
        }
      >
        <Reviewer />
      </Section>

      <Section id="queue" title="A queue for the uncertain pairs.">
        <CardGrid
          items={[
            {
              title: "See why",
              body: "Each pair shows which fields agreed, which disagreed, and how much each one counted toward the score.",
            },
            {
              title: "Decide quickly",
              body: "Resolve pairs one at a time, or many at once when they share an obvious answer.",
            },
            {
              title: "Ask for a second opinion",
              body: "If your organisation turns on AI features, ask an AI model for a recommendation with its reasons. It only suggests; a person still decides.",
            },
          ]}
        />
      </Section>

      <Section id="fix" title="Fix a mistake without losing the history.">
        <CardGrid
          items={[
            {
              title: "Merge by hand",
              body: "Group records yourself when you know they belong together.",
            },
            {
              title: "Unmerge",
              body: "Split records out of a group that should not have formed. The history of the change is kept.",
            },
            {
              title: "Rules that stick",
              body: "Record that two records are always, or never, the same. The rule holds on every later run, and survives retraining.",
            },
          ]}
        />
      </Section>

      <Section
        id="roles"
        title="The right people make the call."
        intro={
          <p>
            Viewers can look at records and evidence. Stewards review, merge and correct. Admins
            change the rules and manage the organisation.
          </p>
        }
      />

      <CtaBand
        title="Review a real queue."
        body="See what your duplicates look like with the evidence beside them."
        primary={DEMO}
      />
      <Related
        links={[
          { href: "/features/matching", label: "Matching" },
          { href: "/features/golden-records", label: "Golden records" },
          { href: "/resources/preparing-for-a-bulk-merge", label: "Preparing for a bulk merge" },
        ]}
      />
    </Page>
  );
}
