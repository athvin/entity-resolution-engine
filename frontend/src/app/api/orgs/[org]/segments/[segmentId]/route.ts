import { and, eq } from "drizzle-orm";
import { NextResponse } from "next/server";

import { requireOrgAccess, respond } from "@/lib/bff/proxy";
import { db, schema } from "@/lib/db";

export async function DELETE(
  _request: Request,
  { params }: { params: Promise<{ org: string; segmentId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, segmentId } = await params;
    await requireOrgAccess(org, "steward");
    const database = await db();
    await database
      .delete(schema.segments)
      .where(and(eq(schema.segments.org, org), eq(schema.segments.id, segmentId)));
    return NextResponse.json({ deleted: true });
  });
}
