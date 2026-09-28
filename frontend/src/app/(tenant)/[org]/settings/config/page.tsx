import type { Metadata } from "next";

import { ConfigStudio } from "@/components/config/config-studio";

export const metadata: Metadata = { title: "Config studio" };

export default async function ConfigPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <ConfigStudio org={org} />;
}
