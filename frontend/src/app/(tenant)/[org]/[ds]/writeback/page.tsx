import type { Metadata } from "next";

import { WritebackContent } from "@/components/writeback/writeback-content";

export const metadata: Metadata = { title: "Writeback" };

export default async function WritebackPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <WritebackContent org={org} />;
}
