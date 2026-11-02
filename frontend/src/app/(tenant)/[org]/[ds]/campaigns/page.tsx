import type { Metadata } from "next";

import { CampaignsContent } from "@/components/campaigns/campaigns-content";

export const metadata: Metadata = { title: "Campaigns" };

export default async function CampaignsPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <CampaignsContent org={org} />;
}
