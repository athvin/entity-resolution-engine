import Link from "next/link";
import { notFound } from "next/navigation";

import { ARTICLES } from "@/components/marketing/pages/resources-content";
import {
  JsonLd,
  ORGANIZATION_ID,
  PAGE_UPDATED,
  SHARE_IMAGE,
  formatUpdated,
} from "@/components/marketing/seo";
import { canonicalUrl } from "@/lib/site/routes";
import {
  Bullets,
  Page,
  PageIntro,
  pageMetadata,
  Section,
} from "@/components/marketing/pages/page-kit";

export const dynamic = "force-static";
export const dynamicParams = false;

export function generateStaticParams() {
  return ARTICLES.map((a) => ({ slug: a.slug }));
}

export async function generateMetadata({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const article = ARTICLES.find((a) => a.slug === slug);
  if (!article) return {};
  return pageMetadata({
    path: `/resources/${article.slug}`,
    title: article.title,
    description: article.summary,
  });
}

export default async function ArticlePage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const article = ARTICLES.find((a) => a.slug === slug);
  if (!article) notFound();
  const updated = PAGE_UPDATED[`/resources/${article.slug}`] ?? "";
  return (
    <Page
      testId="page-article"
      crumb={{ path: `/resources/${article.slug}`, label: article.title }}
    >
      <JsonLd
        data={{
          "@type": "Article",
          headline: article.title,
          description: article.summary,
          datePublished: updated,
          dateModified: updated,
          author: { "@type": "Organization", name: "DupeZero team", url: canonicalUrl("/about") },
          publisher: { "@id": ORGANIZATION_ID },
          image: canonicalUrl(SHARE_IMAGE.url),
          mainEntityOfPage: canonicalUrl(`/resources/${article.slug}`),
        }}
      />
      <PageIntro
        eyebrow={`Guide · Last updated ${formatUpdated(updated)} · DupeZero team`}
        title={article.title}
        lead={article.summary}
      />
      {article.sections.map((part) => (
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
      <p className="page-related">
        <Link href="/resources" className="story-text-link">
          ← All guides
        </Link>
      </p>
    </Page>
  );
}
