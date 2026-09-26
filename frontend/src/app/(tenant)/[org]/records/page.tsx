import type { Metadata } from "next";

import { RecordsBrowser } from "@/components/records/records-browser";

export const metadata: Metadata = { title: "Records" };

export default async function RecordsPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <RecordsBrowser org={org} />;
}
