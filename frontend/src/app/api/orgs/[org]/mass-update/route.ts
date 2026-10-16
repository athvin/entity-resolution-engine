import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

const ATTRS = [
  "given_name",
  "family_name",
  "email",
  "phone_e164",
  "birth_date",
  "addr_number",
  "addr_street",
  "addr_unit",
  "addr_city",
  "addr_region",
  "addr_postal",
] as const;

const schema = z.object({
  items: z
    .array(
      z.object({
        entity_id: z.string().min(1),
        attribute: z.enum(ATTRS),
        value: z.string().nullable().optional(),
        clear: z.boolean().optional(),
      }),
    )
    .min(1)
    .max(200),
  apply_now: z.boolean().optional(),
});

export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "steward");
    const parsed = schema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(
        422,
        "validation",
        "items[{entity_id, attribute, value?|clear?}] required",
      );
    }
    const result = await forward<unknown>(access, `/v1/orgs/${org}/mass-update`, {
      method: "POST",
      role: "steward",
      body: parsed.data,
    });
    return NextResponse.json(result);
  });
}
