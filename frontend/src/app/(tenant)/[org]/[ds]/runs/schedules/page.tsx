import type { Metadata } from "next";

import { SchedulesContent } from "@/components/schedules/schedules-content";

export const metadata: Metadata = { title: "Schedules" };

export default async function SchedulesPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <SchedulesContent org={org} />;
}
