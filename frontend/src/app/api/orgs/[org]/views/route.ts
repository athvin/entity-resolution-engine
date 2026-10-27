import { and, asc, eq } from "drizzle-orm";
import { NextResponse } from "next/server";
import { ulid } from "ulidx";

import { BffFailure, requireOrgAccess, respond } from "@/lib/bff/proxy";
import { db, schema } from "@/lib/db";
import { isEmptyDefinition, parseCreateView, VIEW_SURFACES } from "@/lib/views/define";

/**
 * The saved views for one surface (O12a).
 *
 * Viewer-readable: a view is a shortcut, and the people who most benefit from
 * someone else's saved filter are the ones who would not think to build it.
 * Shared across the org rather than owned — `created_by` records whose it was
 * without hiding it from anyone else.
 */
export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    await requireOrgAccess(org, "viewer");
    const surface = new URL(request.url).searchParams.get("surface") ?? "";
    if (!(VIEW_SURFACES as readonly string[]).includes(surface)) {
      throw new BffFailure(422, "validation", `surface must be one of ${VIEW_SURFACES.join(", ")}`);
    }
    const database = await db();
    const rows = await database
      .select()
      .from(schema.savedViews)
      .where(and(eq(schema.savedViews.org, org), eq(schema.savedViews.surface, surface)))
      .orderBy(asc(schema.savedViews.name));
    return NextResponse.json(rows);
  });
}

/**
 * Save a view. Steward+, because a shared view is something colleagues will
 * act on, and a viewer cannot act on the surface it filters anyway.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "steward");
    const parsed = parseCreateView(await request.json().catch(() => null));
    if (!parsed.ok) {
      throw new BffFailure(422, "validation", parsed.reason);
    }
    if (isEmptyDefinition(parsed.value.definition)) {
      // Almost always Save pressed before the filter was set. The name makes it
      // look deliberate to whoever finds it later, so it is refused by name.
      throw new BffFailure(
        422,
        "validation",
        "this view filters nothing — set a filter before saving it",
      );
    }

    const database = await db();
    const existing = await database
      .select({ id: schema.savedViews.id })
      .from(schema.savedViews)
      .where(
        and(
          eq(schema.savedViews.org, org),
          eq(schema.savedViews.surface, parsed.value.surface),
          eq(schema.savedViews.name, parsed.value.name),
        ),
      )
      .limit(1);
    if (existing.length > 0) {
      // Pre-checked rather than left to the unique index, so the message names
      // the collision; the index still backstops two stewards racing.
      throw new BffFailure(
        409,
        "conflict",
        `a view named "${parsed.value.name}" already exists on this screen`,
      );
    }

    const [row] = await database
      .insert(schema.savedViews)
      .values({
        id: ulid(),
        org,
        surface: parsed.value.surface,
        name: parsed.value.name,
        definition: parsed.value.definition,
        createdBy: access.identity.user.email,
      })
      .returning();
    return NextResponse.json(row, { status: 201 });
  });
}
