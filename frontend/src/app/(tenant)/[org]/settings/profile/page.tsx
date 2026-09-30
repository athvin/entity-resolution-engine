import type { Metadata } from "next";

import { ProfileForm } from "@/components/settings/profile-form";

export const metadata: Metadata = { title: "Profile" };

export default function ProfilePage() {
  return <ProfileForm />;
}
