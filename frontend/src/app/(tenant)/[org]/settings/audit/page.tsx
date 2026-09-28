import type { Metadata } from "next";

import { TenantAuditFeed } from "@/components/audit/tenant-audit";

export const metadata: Metadata = { title: "Audit" };

export default async function TenantAuditPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <TenantAuditFeed org={org} />;
}
