import { ARTICLES } from "@/components/marketing/pages/resources-content";
import {
  CardGrid,
  CtaBand,
  DEMO,
  Page,
  PageIntro,
  pageMetadata,
  Section,
} from "@/components/marketing/pages/page-kit";

export const dynamic = "force-static";

export const metadata = pageMetadata({
  path: "/resources",
  title: "Resources",
  description:
    "Guides for Salesforce admins and revenue operations on duplicate records: where they come from, preparing to merge, and how matching rules work.",
});

export default function ResourcesPage() {
  return (
    <Page testId="page-resources">
      <PageIntro
        eyebrow="Resources"
        title="Guides to cleaner records."
        lead="Practical reading for the people who look after CRM data. These guides are being written; each one shows its outline in the meantime."
      />
      <Section id="articles" title="Guides">
        <CardGrid
          items={ARTICLES.map((a) => ({
            title: a.title,
            body: a.summary,
            href: `/resources/${a.slug}`,
            linkLabel: "Read the outline",
          }))}
        />
      </Section>
      <CtaBand
        title="Rather talk it through?"
        body="We will look at your duplicates with you."
        primary={DEMO}
      />
    </Page>
  );
}
