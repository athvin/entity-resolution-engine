import { and, eq } from "drizzle-orm";
import { NextResponse } from "next/server";

import { BffFailure, requireDatasetAccess, respond } from "@/lib/bff/proxy";
import { db, schema } from "@/lib/db";

/**
 * Delete a saved view (O12a).
 *
 * Steward+, matching creation. The org predicate is in the WHERE clause rather
 * than checked after the read: deleting by id alone would let a steward in one
 * tenant remove another tenant's view by guessing a ULID, and the 404 below
 * makes a cross-tenant id indistinguishable from a missing one.
 */
export async function DELETE(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string; viewId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds, viewId } = await params;
    await requireDatasetAccess(org, ds, "steward");
    const database = await db();
    const deleted = await database
      .delete(schema.savedViews)
      .where(and(eq(schema.savedViews.org, org), eq(schema.savedViews.id, viewId)))
      .returning({ id: schema.savedViews.id });
    if (deleted.length === 0) {
      throw new BffFailure(404, "not_found", "not found");
    }
    return NextResponse.json({ deleted: viewId });
  });
}
