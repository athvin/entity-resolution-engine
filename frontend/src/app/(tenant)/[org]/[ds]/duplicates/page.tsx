import type { Metadata } from "next";

import { DuplicatesContent } from "@/components/duplicates/duplicates-content";

export const metadata: Metadata = { title: "Duplicates" };

export default async function DuplicatesPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <DuplicatesContent org={org} />;
}
