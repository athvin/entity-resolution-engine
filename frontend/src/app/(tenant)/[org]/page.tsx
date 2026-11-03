import type { Metadata } from "next";

import { DatasetDispatcher } from "@/components/datasets/dataset-dispatcher";

export const metadata: Metadata = { title: "Workspace" };

/**
 * The org home. It renders nothing of its own: a workspace is a set of
 * datasets, and this decides which one you meant. See
 * [`DatasetDispatcher`] for the order it tries.
 */
export default async function OrgHome({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <DatasetDispatcher org={org} />;
}
