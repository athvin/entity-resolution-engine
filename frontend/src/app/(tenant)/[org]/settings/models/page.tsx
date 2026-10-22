import type { Metadata } from "next";

import { ModelsContent } from "@/components/settings/models-content";

export const metadata: Metadata = { title: "Models" };

export default async function ModelsPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <ModelsContent org={org} />;
}
