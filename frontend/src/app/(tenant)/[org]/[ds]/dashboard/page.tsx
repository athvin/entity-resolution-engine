import type { Metadata } from "next";

import { DashboardContent } from "@/components/dashboard/dashboard-content";

export const metadata: Metadata = { title: "Dashboard" };

export default async function DashboardPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <DashboardContent org={org} />;
}
