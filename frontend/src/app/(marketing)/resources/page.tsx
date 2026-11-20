import { BLOG_BASE, POSTS } from "@/components/marketing/pages/blog-content";
import { ARTICLES } from "@/components/marketing/pages/resources-content";
import { ROI_CALCULATOR_HREF } from "@/components/marketing/site-links";
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
    "Guides, blog posts, developer docs and an ROI calculator for the people who look after Salesforce and CRM data.",
});

export default function ResourcesPage() {
  return (
    <Page testId="page-resources">
      <PageIntro
        eyebrow="Resources"
        title="Guides to cleaner records."
        lead="Practical reading for the people who look after CRM data: where duplicates come from, how to merge safely, and how matching decides."
      />
      <Section id="articles" title="Guides">
        <CardGrid
          items={ARTICLES.map((a) => ({
            title: a.title,
            body: a.summary,
            href: `/resources/${a.slug}`,
            linkLabel: "Read the guide",
          }))}
        />
      </Section>
      <Section id="more" title="More resources">
        <CardGrid
          items={[
            {
              title: "Blog",
              body: POSTS[0]
                ? `Latest: ${POSTS[0].title}.`
                : "Notes from the DupeZero team on cleaner customer data.",
              href: BLOG_BASE,
              linkLabel: "Read the blog",
            },
            {
              title: "Docs",
              body: "How to set up datasets, sources, matching and results in DupeZero.",
              href: "/docs",
              linkLabel: "Open the docs",
            },
            {
              title: "ROI calculator",
              body: "Estimate what duplicates cost your team, and what a clean-up could recover.",
              href: ROI_CALCULATOR_HREF,
              linkLabel: "Try the calculator",
            },
          ]}
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
