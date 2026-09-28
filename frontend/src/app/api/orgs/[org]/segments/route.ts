import { asc, and, eq } from "drizzle-orm";
import { NextResponse } from "next/server";
import { ulid } from "ulidx";
import { z } from "zod";

import { BffFailure, requireOrgAccess, respond } from "@/lib/bff/proxy";
import { db, schema } from "@/lib/db";

export const segmentDefinitionSchema = z.object({
  /** Server-side search over name/email — what the read API evaluates today. */
  q: z.string().max(200).default(""),
  /** Rows the export/preview will fetch at most (pages of 200 under the hood). */
  limit: z.number().int().min(1).max(10_000).default(1000),
});

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    await requireOrgAccess(org, "viewer");
    const database = await db();
    const rows = await database
      .select()
      .from(schema.segments)
      .where(eq(schema.segments.org, org))
      .orderBy(asc(schema.segments.name));
    return NextResponse.json(rows);
  });
}

const createSchema = z.object({
  name: z.string().min(1).max(120),
  definition: segmentDefinitionSchema,
});

export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "steward");
    const parsed = createSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "name and definition {q, limit} are required");
    }
    const database = await db();
    const existing = await database
      .select({ id: schema.segments.id })
      .from(schema.segments)
      .where(and(eq(schema.segments.org, org), eq(schema.segments.name, parsed.data.name)))
      .limit(1);
    if (existing.length > 0) {
      throw new BffFailure(422, "validation", `a segment named ${parsed.data.name} exists`);
    }
    const row = {
      id: ulid().toLowerCase(),
      org,
      name: parsed.data.name,
      definition: parsed.data.definition,
      createdBy: access.identity.user.email,
    };
    await database.insert(schema.segments).values(row);
    return NextResponse.json(row, { status: 201 });
  });
}
