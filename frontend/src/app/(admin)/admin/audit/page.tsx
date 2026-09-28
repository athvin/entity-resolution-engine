import type { Metadata } from "next";

import { GlobalAuditFeed } from "@/components/admin/global-audit";

export const metadata: Metadata = { title: "Audit" };

export default function AdminAuditPage() {
  return <GlobalAuditFeed />;
}
