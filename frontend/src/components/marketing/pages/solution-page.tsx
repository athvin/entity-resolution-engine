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
} from "./page-kit";
import type { Solution } from "./solutions-content";

/** Metadata for one role or industry page. */
export function solutionMetadata(solution: Solution, base: string) {
  return pageMetadata({
    path: `${base}/${solution.slug}`,
    title: solution.name,
    description: solution.metaDescription,
  });
}

/**
 * One role or industry page: where duplicates hurt, how DupeZero helps (each
 * line linking to the feature page that backs it), use cases, and the limit
 * that matters most for this reader.
 */
export function SolutionPage({
  solution,
  base,
  siblings,
}: {
  solution: Solution;
  base: string;
  siblings: readonly Solution[];
}) {
  return (
    <Page testId={`page-solution-${solution.slug}`}>
      <PageIntro
        eyebrow={`Solutions / ${solution.name}`}
        title={solution.title}
        lead={solution.lead}
        primary={DEMO}
        secondary={{ href: "/solutions", label: "All solutions" }}
      />

      <Section id="hurts" title="Where duplicates hurt">
        <Bullets items={solution.hurts} />
      </Section>

      <Section id="helps" title="How DupeZero helps">
        <CardGrid
          items={solution.helps.map((h) => ({
            title: h.title,
            body: h.body,
            href: h.href,
            linkLabel: "How it works",
          }))}
        />
      </Section>

      <Section id="use-cases" title="Use cases">
        <CardGrid items={solution.useCases} />
      </Section>

      <Section id="note" title="Worth knowing">
        <Note title={solution.note.title}>{solution.note.body}</Note>
      </Section>

      <CtaBand
        title="See DupeZero with your records."
        body="We will walk through a cleanup with a sample of your data."
        primary={DEMO}
        secondary={{ href: "/pricing", label: "See pricing" }}
      />
      <Related
        links={siblings
          .filter((s) => s.slug !== solution.slug)
          .map((s) => ({ href: `${base}/${s.slug}`, label: s.name }))}
      />
    </Page>
  );
}
