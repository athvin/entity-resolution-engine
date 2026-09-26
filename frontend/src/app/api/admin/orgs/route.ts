import { inArray } from "drizzle-orm";
import { NextResponse } from "next/server";
import { z } from "zod";

import { mintServiceKeys, operatorFetch, requireSuperAdmin, writeAudit } from "@/lib/bff/admin";
import { BffFailure, respond } from "@/lib/bff/proxy";
import { db, schema } from "@/lib/db";

interface OrgRow {
  name: string;
  state: string;
  active_config_version: number | null;
  created_at: string;
}

export async function GET(): Promise<NextResponse> {
  return respond(async () => {
    const identity = await requireSuperAdmin();
    const orgs = await operatorFetch<OrgRow[]>(identity, "/v1/orgs");
    const database = await db();
    const names = orgs.map((org) => org.name);
    const registry = names.length
      ? await database
          .select()
          .from(schema.orgsRegistry)
          .where(inArray(schema.orgsRegistry.org, names))
      : [];
    const credentials = names.length
      ? await database
          .select({ org: schema.orgCredentials.org })
          .from(schema.orgCredentials)
          .where(inArray(schema.orgCredentials.org, names))
      : [];
    const registered = new Map(registry.map((row) => [row.org, row]));
    const vaulted = new Set(credentials.map((row) => row.org));
    return NextResponse.json(
      orgs.map((org) => ({
        ...org,
        display_name: registered.get(org.name)?.displayName ?? org.name,
        registered: registered.has(org.name),
        has_credentials: vaulted.has(org.name),
      })),
    );
  });
}

const provisionSchema = z.object({
  name: z
    .string()
    .min(1)
    .max(128)
    .regex(/^[a-z0-9][a-z0-9_-]*$/, "lowercase letters, digits, - and _ only"),
  display_name: z.string().min(1).max(200).optional(),
});

interface ProvisionOut {
  name: string;
  state: string;
  tenant?: string;
  job_id?: string;
  admin_key?: string;
  admin_key_id?: string;
}

export async function POST(request: Request): Promise<NextResponse> {
  return respond(async () => {
    const identity = await requireSuperAdmin();
    const parsed = provisionSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", parsed.error.issues[0]?.message ?? "invalid name");
    }
    const { name, display_name } = parsed.data;

    const created = await operatorFetch<ProvisionOut>(identity, "/v1/orgs", {
      method: "POST",
      body: { name },
    });

    // Our own service keys go straight into the vault; the tenant's one-time
    // admin key passes through to the operator exactly once and is never stored.
    await mintServiceKeys(identity, name);
    const database = await db();
    await database
      .insert(schema.orgsRegistry)
      .values({
        org: name,
        displayName: display_name ?? name,
        provisionedByUser: identity.user.id,
        provisionJobId: created.job_id ?? null,
      })
      .onConflictDoNothing();
    await writeAudit(identity, "org.provision", { job_id: created.job_id ?? null }, name);

    return NextResponse.json(created, { status: 201 });
  });
}
