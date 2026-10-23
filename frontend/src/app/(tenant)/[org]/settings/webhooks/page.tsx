import type { Metadata } from "next";

import { WebhooksContent } from "@/components/settings/webhooks-content";

export const metadata: Metadata = { title: "Webhooks" };

export default async function WebhooksPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <WebhooksContent org={org} />;
}
