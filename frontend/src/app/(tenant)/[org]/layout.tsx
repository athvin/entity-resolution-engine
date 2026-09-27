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

  // An active impersonation pins the whole app to that tenant.
  if (identity.impersonating && identity.impersonating.org !== org) {
    redirect(`/${identity.impersonating.org}/dashboard`);
  }
  const membership = identity.memberships.find((m) => m.org === org);
  // Non-members learn nothing about which orgs exist; super admins pass.
  if (!membership && !identity.user.isSuperAdmin) notFound();

  const impersonation = identity.impersonating
    ? {
        org: identity.impersonating.org,
        role: identity.impersonating.role,
        expiresAt: identity.impersonating.expiresAt.toISOString(),
      }
    : null;
  // While impersonating, the switcher shows exactly what the tenant would see.
  const memberships = impersonation
    ? [{ org: impersonation.org, role: impersonation.role }]
    : identity.memberships;

  return (
    <AppShell
      org={org}
      user={identity.user}
      memberships={memberships}
      impersonation={impersonation}
    >
      {children}
    </AppShell>
  );
}
