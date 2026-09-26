import type { Metadata } from "next";

import { TenantsList } from "@/components/admin/tenants-list";

export const metadata: Metadata = { title: "Tenants" };

export default function TenantsPage() {
  return <TenantsList />;
}
