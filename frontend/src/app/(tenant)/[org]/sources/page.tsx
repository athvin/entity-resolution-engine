import type { Metadata } from "next";

import { SourcesContent } from "@/components/sources/sources-content";

export const metadata: Metadata = { title: "Sources" };

export default async function SourcesPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <SourcesContent org={org} />;
}
