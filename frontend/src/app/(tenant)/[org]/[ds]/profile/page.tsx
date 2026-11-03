import type { Metadata } from "next";

import { ProfileContent } from "@/components/profile/profile-content";

export const metadata: Metadata = { title: "Profile" };

export default async function ProfilePage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <ProfileContent org={org} />;
}
