import type { Metadata } from "next";

import { ApiKeysContent } from "@/components/settings/api-keys-content";

export const metadata: Metadata = { title: "API keys" };

export default async function ApiKeysPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <ApiKeysContent org={org} />;
}
