import { redirect } from "next/navigation";

import { identityFromCookies } from "@/lib/auth/session";
import { db, schema } from "@/lib/db";

/**
 * The signed-in entry point, and the single destination of every marketing CTA.
 *
 * This used to be `/`, which is now the landing page. The move is not cosmetic:
 * `/app` is deliberately absent from the proxy's PUBLIC_PATHS, so a signed-out
 * visitor is turned away by the cheap cookie check and arrives back here via
 * `?next=/app` after logging in — which means neither this page's database
 * query nor `identityFromCookies()` runs for anonymous traffic, as it did when
 * this was the root route.
 */
export default async function PortalEntry() {
  const identity = await identityFromCookies();
  if (!identity) redirect("/login");
  const first = identity.memberships[0];
  if (first) redirect(`/${first.org}/dashboard`);
  if (identity.user.isSuperAdmin) {
    const database = await db();
    const registered = await database.select().from(schema.orgsRegistry).limit(1);
    redirect(registered[0] ? `/${registered[0].org}/dashboard` : "/admin");
  }
  redirect("/login?error=no-workspace");
}
