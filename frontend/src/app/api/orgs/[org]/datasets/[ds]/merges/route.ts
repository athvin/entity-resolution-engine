import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

// The merge dialog's bundle (spec 2.8). The server revalidates every rule —
// this schema is the early answer with the field names in it.
const overrideSchema = z
  .object({
    attribute: z.string().min(1),
    value: z.string().optional(),
    clear: z.boolean().optional(),
  })
  .refine((o) => o.clear === true || typeof o.value === "string", {
    message: "an override needs a value or clear: true",
  });

const mergeSchema = z
  .object({
    records: z.array(z.string().min(1)).min(2).max(50),
    master: z.string().min(1),
    overrides: z.array(overrideSchema).max(11).optional(),
    review_id: z.string().min(1).optional(),
    apply_now: z.boolean().optional(),
  })
  .refine((body) => body.records.includes(body.master), {
    message: "master must be one of records",
  })
  .refine((body) => new Set(body.records).size === body.records.length, {
    message: "records must be distinct",
  });

export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "steward");
    const parsed = mergeSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(
        422,
        "validation",
        parsed.error.issues[0]?.message ?? "invalid merge bundle",
      );
    }
    const result = await forward<unknown>(access, `/v1/orgs/${org}/datasets/${ds}/merges`, {
      method: "POST",
      role: "steward",
      body: parsed.data,
    });
    return NextResponse.json(result);
  });
}
