import { redirect } from "next/navigation";

import { identityFromCookies } from "@/lib/auth/session";
import { db, schema } from "@/lib/db";

export default async function Home() {
  const identity = await identityFromCookies();
  if (!identity) redirect("/login");
  const first = identity.memberships[0];
  if (first) redirect(`/${first.org}/dashboard`);
  if (identity.user.isSuperAdmin) {
    const database = await db();
    const registered = await database.select().from(schema.orgsRegistry).limit(1);
    if (registered[0]) redirect(`/${registered[0].org}/dashboard`);
  }
  redirect("/login?error=no-workspace");
}
