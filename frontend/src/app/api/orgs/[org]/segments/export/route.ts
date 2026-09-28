import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

const exportSchema = z.object({
  q: z.string().max(200).default(""),
  limit: z.number().int().min(1).max(10_000).default(1000),
});

interface GoldenPage {
  items: Record<string, unknown>[];
  snapshot: number;
  next_cursor: string | null;
}

const COLUMNS = [
  "entity_id",
  "given_name",
  "family_name",
  "email",
  "phone_e164",
  "addr_number",
  "addr_street",
  "addr_unit",
  "addr_city",
  "addr_region",
  "addr_postal",
  "birth_date",
] as const;

function csvCell(value: unknown): string {
  if (value === null || value === undefined) return "";
  const text =
    typeof value === "string" || typeof value === "number" || typeof value === "boolean"
      ? String(value)
      : JSON.stringify(value);
  return /[",\n]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text;
}

/**
 * Segment CSV export: the BFF pages the snapshot-pinned read API and streams
 * one consistent file — every page carries the first page's snapshot, so a
 * concurrent run cannot shear the export (the §7.19 export job's synchronous
 * little sibling; the scheduled variant arrives with the parameterized job).
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const parsed = exportSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) throw new BffFailure(422, "validation", "q and limit are required");
    const { q, limit } = parsed.data;

    const lines = [COLUMNS.join(",")];
    let cursor: string | null = null;
    let snapshot: number | null = null;
    while (lines.length - 1 < limit) {
      const searchParams = new URLSearchParams({
        limit: String(Math.min(200, limit - (lines.length - 1))),
      });
      if (q) searchParams.set("q", q);
      if (cursor) searchParams.set("cursor", cursor);
      const page: GoldenPage = await forward<GoldenPage>(access, `/v1/orgs/${org}/golden-records`, {
        searchParams,
      });
      snapshot ??= page.snapshot;
      for (const item of page.items) {
        lines.push(COLUMNS.map((column) => csvCell(item[column])).join(","));
      }
      if (!page.next_cursor) break;
      cursor = page.next_cursor;
    }

    return new NextResponse(lines.join("\n") + "\n", {
      headers: {
        "content-type": "text/csv; charset=utf-8",
        "content-disposition": `attachment; filename="segment-${org}.csv"`,
        "x-snapshot": String(snapshot ?? ""),
      },
    });
  });
}
