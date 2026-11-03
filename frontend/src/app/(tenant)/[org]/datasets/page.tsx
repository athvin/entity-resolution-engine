import type { Metadata } from "next";

import { DatasetsContent } from "@/components/datasets/datasets-content";

export const metadata: Metadata = { title: "Datasets" };

export default async function DatasetsPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <DatasetsContent org={org} />;
}
