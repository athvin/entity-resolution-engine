import Link from "next/link";
import { notFound } from "next/navigation";

import { BLOG_BASE, POSTS } from "@/components/marketing/pages/blog-content";
import {
  Actions,
  Bullets,
  CtaBand,
  DEMO,
  Page,
  PageIntro,
  pageMetadata,
  Section,
} from "@/components/marketing/pages/page-kit";
import { formatPostDate } from "@/components/marketing/pages/post-date";
import { JsonLd, ORGANIZATION_ID, SHARE_IMAGE } from "@/components/marketing/seo";
import { canonicalUrl } from "@/lib/site/routes";

export const dynamic = "force-static";
export const dynamicParams = false;

export function generateStaticParams() {
  return POSTS.map((p) => ({ slug: p.slug }));
}

export async function generateMetadata({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const post = POSTS.find((p) => p.slug === slug);
  if (!post) return {};
  return pageMetadata({
    path: `${BLOG_BASE}/${post.slug}`,
    title: post.title,
    description: post.summary,
  });
}

export default async function BlogPostPage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const post = POSTS.find((p) => p.slug === slug);
  if (!post) notFound();
  return (
    <Page testId="page-blog-post" crumb={{ path: `${BLOG_BASE}/${post.slug}`, label: post.title }}>
      <JsonLd
        data={{
          "@type": "BlogPosting",
          headline: post.title,
          description: post.summary,
          datePublished: post.date,
          dateModified: post.date,
          // The byline the page shows: the team, as an organisation, not a person.
          author: { "@type": "Organization", name: "DupeZero team", url: canonicalUrl("/about") },
          publisher: { "@id": ORGANIZATION_ID },
          image: canonicalUrl(SHARE_IMAGE.url),
          mainEntityOfPage: canonicalUrl(`${BLOG_BASE}/${post.slug}`),
        }}
      />
      <PageIntro
        eyebrow={`Blog · ${formatPostDate(post.date)} · DupeZero team`}
        title={post.title}
        lead={post.summary}
      />
      {post.sections.map((part) => (
        <Section
          key={part.heading}
          title={part.heading}
          intro={part.paragraphs.map((p) => (
            <p key={p}>{p}</p>
          ))}
        >
          {part.points && <Bullets items={part.points} />}
        </Section>
      ))}
      {post.cta && <Actions primary={post.cta} />}
      <CtaBand
        title="Talk to us about your duplicates."
        body="Tell us where they hurt and we will show you that part first."
        primary={DEMO}
      />
      <p className="page-related">
        <Link href={BLOG_BASE} className="story-text-link">
          ← All posts
        </Link>
      </p>
    </Page>
  );
}
