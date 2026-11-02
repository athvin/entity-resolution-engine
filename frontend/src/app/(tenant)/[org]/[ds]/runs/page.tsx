import type { Metadata } from "next";

import { RunsContent } from "@/components/runs/runs-content";

export const metadata: Metadata = { title: "Runs" };

export default async function RunsPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <RunsContent org={org} />;
}
