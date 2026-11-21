import { notFound } from "next/navigation";

import { SolutionPage, solutionMetadata } from "@/components/marketing/pages/solution-page";
import { ROLE_BASE, ROLES } from "@/components/marketing/pages/solutions-content";

export const dynamic = "force-static";
export const dynamicParams = false;

export function generateStaticParams() {
  return ROLES.map((s) => ({ slug: s.slug }));
}

export async function generateMetadata({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const solution = ROLES.find((s) => s.slug === slug);
  return solution ? solutionMetadata(solution, ROLE_BASE) : {};
}

export default async function RolePage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const solution = ROLES.find((s) => s.slug === slug);
  if (!solution) notFound();
  return <SolutionPage solution={solution} base={ROLE_BASE} siblings={ROLES} />;
}
