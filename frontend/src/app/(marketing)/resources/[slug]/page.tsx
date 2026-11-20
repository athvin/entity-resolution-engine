import Link from "next/link";
import { notFound } from "next/navigation";

import { ARTICLES } from "@/components/marketing/pages/resources-content";
import {
  Bullets,
  Page,
  PageIntro,
  pageMetadata,
  Section,
  Todo,
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
    unlisted: true,
  });
}

export default async function ArticlePage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const article = ARTICLES.find((a) => a.slug === slug);
  if (!article) notFound();
  return (
    <Page testId="page-article">
      <p className="page-draft" role="note">
        <Todo>write this article</Todo> This guide is an outline. The full article is on its way.
      </p>
      <PageIntro eyebrow="Guide" title={article.title} lead={article.summary} />
      {article.outline.map((part) => (
        <Section key={part.heading} title={part.heading}>
          <Bullets items={part.points} />
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
