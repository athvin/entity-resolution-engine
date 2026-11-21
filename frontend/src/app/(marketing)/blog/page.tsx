import { BLOG_BASE, POSTS } from "@/components/marketing/pages/blog-content";
import {
  CardGrid,
  CtaBand,
  DEMO,
  Page,
  PageIntro,
  pageMetadata,
  Section,
} from "@/components/marketing/pages/page-kit";
import { formatPostDate } from "@/components/marketing/pages/post-date";

export const dynamic = "force-static";

export const metadata = pageMetadata({
  path: BLOG_BASE,
  title: "Blog",
  description:
    "Notes from the DupeZero team on duplicate records, matching you can explain, and keeping customer data clean.",
});

export default function BlogPage() {
  return (
    <Page testId="page-blog" crumb={{ path: "/blog", label: "Blog" }}>
      <PageIntro
        eyebrow="Blog"
        title="Notes on cleaner customer data."
        lead="How we think about duplicates, matching and the people who decide. Short, practical, and specific to the work."
      />
      <Section id="posts" title="Latest posts">
        <CardGrid
          items={POSTS.map((p) => ({
            title: p.title,
            body: `${formatPostDate(p.date)} · ${p.summary}`,
            href: `${BLOG_BASE}/${p.slug}`,
            linkLabel: "Read the post",
          }))}
        />
      </Section>
      <CtaBand
        title="See it with your own records."
        body="We will walk you through DupeZero using the duplicates you actually have."
        primary={DEMO}
      />
    </Page>
  );
}
