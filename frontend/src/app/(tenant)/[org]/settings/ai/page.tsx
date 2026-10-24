import type { Metadata } from "next";

import { AiContent } from "@/components/settings/ai-content";

export const metadata: Metadata = { title: "AI" };

export default async function AiSettingsPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <AiContent org={org} />;
}
