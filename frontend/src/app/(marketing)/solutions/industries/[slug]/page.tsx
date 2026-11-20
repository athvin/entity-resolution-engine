import { notFound } from "next/navigation";

import { SolutionPage, solutionMetadata } from "@/components/marketing/pages/solution-page";
import { INDUSTRY_BASE, INDUSTRIES } from "@/components/marketing/pages/solutions-content";

export const dynamic = "force-static";
export const dynamicParams = false;

export function generateStaticParams() {
  return INDUSTRIES.map((s) => ({ slug: s.slug }));
}

export async function generateMetadata({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const solution = INDUSTRIES.find((s) => s.slug === slug);
  return solution ? solutionMetadata(solution, INDUSTRY_BASE) : {};
}

export default async function IndustryPage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const solution = INDUSTRIES.find((s) => s.slug === slug);
  if (!solution) notFound();
  return <SolutionPage solution={solution} base={INDUSTRY_BASE} siblings={INDUSTRIES} />;
}
