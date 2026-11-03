import type { Metadata } from "next";

import { NewDatasetWizard } from "@/components/datasets/new-dataset-wizard";

export const metadata: Metadata = { title: "New dataset" };

export default async function NewDatasetPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <NewDatasetWizard org={org} />;
}
