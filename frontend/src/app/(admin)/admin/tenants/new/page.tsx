import type { Metadata } from "next";

import { NewTenant } from "@/components/admin/new-tenant";

export const metadata: Metadata = { title: "New tenant" };

export default function NewTenantPage() {
  return <NewTenant />;
}
