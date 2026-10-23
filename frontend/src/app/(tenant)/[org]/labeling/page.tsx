import type { Metadata } from "next";

import { LabelingContent } from "@/components/labeling/labeling-content";

export const metadata: Metadata = { title: "Labelling" };

export default async function LabelingPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <LabelingContent org={org} />;
}
