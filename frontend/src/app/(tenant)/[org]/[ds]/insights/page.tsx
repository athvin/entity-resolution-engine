import type { Metadata } from "next";

import { InsightsContent } from "@/components/insights/insights-content";

export const metadata: Metadata = { title: "Insights" };

export default async function InsightsPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <InsightsContent org={org} />;
}
