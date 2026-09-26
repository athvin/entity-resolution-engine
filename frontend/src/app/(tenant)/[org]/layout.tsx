import { notFound, redirect } from "next/navigation";

import { AppShell } from "@/components/shell/app-shell";
import { identityFromCookies } from "@/lib/auth/session";

export default async function OrgLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ org: string }>;
}) {
  const { org } = await params;
  const identity = await identityFromCookies();
  if (!identity) redirect(`/login?next=${encodeURIComponent(`/${org}/dashboard`)}`);
  const membership = identity.memberships.find((m) => m.org === org);
  // Non-members learn nothing about which orgs exist; super admins pass (M2
  // wraps this in explicit impersonation).
  if (!membership && !identity.user.isSuperAdmin) notFound();
  return (
    <AppShell org={org} user={identity.user} memberships={identity.memberships}>
      {children}
    </AppShell>
  );
}
