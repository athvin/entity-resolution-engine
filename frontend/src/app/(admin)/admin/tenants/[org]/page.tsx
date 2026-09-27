import type { Metadata } from "next";

import { TenantDetail } from "@/components/admin/tenant-detail";

export const metadata: Metadata = { title: "Tenant" };

export default async function TenantPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <TenantDetail org={org} />;
}
