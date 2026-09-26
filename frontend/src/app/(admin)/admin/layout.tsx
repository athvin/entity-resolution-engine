import { notFound, redirect } from "next/navigation";

import { AdminShell } from "@/components/admin/admin-shell";
import { identityFromCookies } from "@/lib/auth/session";

export default async function AdminLayout({ children }: { children: React.ReactNode }) {
  const identity = await identityFromCookies();
  if (!identity) redirect("/login?next=%2Fadmin%2Ftenants");
  // The console does not exist for non-operators.
  if (!identity.user.isSuperAdmin) notFound();
  // While impersonating, the super admin IS the tenant; exit first.
  if (identity.impersonating) redirect(`/${identity.impersonating.org}/dashboard`);
  return <AdminShell user={identity.user}>{children}</AdminShell>;
}
