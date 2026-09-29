import { NextResponse } from "next/server";
import { z } from "zod";

import { operatorFetch, requireSuperAdmin, writeAudit } from "@/lib/bff/admin";
import { BffFailure, respond } from "@/lib/bff/proxy";

const resourcesSchema = z
  .object({
    duckdb_threads: z.number().int().min(1).max(32).optional(),
    duckdb_memory_limit: z
      .string()
      .regex(/^[1-9][0-9]*[MG]B$/, "duckdb_memory_limit must look like 6GB or 512MB")
      .optional(),
  })
  .refine((body) => body.duckdb_threads !== undefined || body.duckdb_memory_limit !== undefined, {
    message: "provide duckdb_threads and/or duckdb_memory_limit",
  });

interface Resources {
  duckdb_threads: number | null;
  duckdb_memory_limit: string | null;
}

export async function PATCH(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const identity = await requireSuperAdmin();
    const { org } = await params;
    const parsed = resourcesSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", parsed.error.issues[0]?.message ?? "invalid body");
    }
    const result = await operatorFetch<Resources>(identity, `/v1/orgs/${org}/resources`, {
      method: "PATCH",
      body: parsed.data,
    });
    await writeAudit(identity, "org.resources", parsed.data, org);
    return NextResponse.json(result);
  });
}
