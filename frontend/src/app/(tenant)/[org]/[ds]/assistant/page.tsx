import type { Metadata } from "next";

import { AssistantChat } from "@/components/assistant/assistant-chat";

export const metadata: Metadata = { title: "Assistant" };

export default async function AssistantPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <AssistantChat org={org} />;
}
