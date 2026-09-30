import type { Metadata } from "next";

import { MembersList } from "@/components/settings/members-list";

export const metadata: Metadata = { title: "Members" };

export default async function MembersPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <MembersList org={org} />;
}
