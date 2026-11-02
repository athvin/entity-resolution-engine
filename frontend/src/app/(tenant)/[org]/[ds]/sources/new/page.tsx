import type { Metadata } from "next";

import { NewSourceWizard } from "@/components/sources/new-source-wizard";

export const metadata: Metadata = { title: "New source" };

export default async function NewSourcePage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <NewSourceWizard org={org} />;
}
