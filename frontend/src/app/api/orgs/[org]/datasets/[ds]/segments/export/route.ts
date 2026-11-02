import { NextResponse } from "next/server";
import { z } from "zod";

import { csvFilename, csvRow } from "@/lib/bff/csv";
import { EXPORT_REQUIREMENTS, exportSummary, isExportable } from "@/lib/bff/exportable";
import { forwardGoldenPage } from "@/lib/bff/golden";
import { BffFailure, requireDatasetAccess, respond } from "@/lib/bff/proxy";

const exportSchema = z.object({
  q: z.string().max(200).default(""),
  limit: z.number().int().min(1).max(10_000).default(1000),
  /** F4. `any` is the default because it is the behaviour every existing caller
   * had — a new option must not silently change what a saved export produces. */
  require: z.enum(EXPORT_REQUIREMENTS).default("any"),
});

const COLUMNS = [
  "id",
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

/**
 * Segment CSV export: the BFF pages the snapshot-pinned read API and streams
 * one consistent file — every page carries the first page's snapshot, so a
 * concurrent run cannot shear the export (the §7.19 export job's synchronous
 * little sibling; the scheduled variant arrives with the parameterized job).
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const parsed = exportSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) throw new BffFailure(422, "validation", "q and limit are required");
    const { q, limit, require: requirement } = parsed.data;

    const lines = [COLUMNS.join(",")];
    let cursor: string | null = null;
    let snapshot: number | null = null;
    let skipped = 0;
    // F4 made the loop able to run long: it pages until it has `limit`
    // EXPORTABLE rows, so a requirement almost nothing satisfies would walk the
    // whole corpus one page at a time. Bounded to the pages a full unfiltered
    // export would have taken, plus headroom for the skipping — past that the
    // honest answer is a short file with the summary saying why, not a request
    // that takes minutes.
    const maxPages = Math.ceil(limit / 200) + 25;
    let pages = 0;
    while (lines.length - 1 < limit && pages < maxPages) {
      pages += 1;
      const searchParams = new URLSearchParams({
        limit: String(Math.min(200, limit - (lines.length - 1))),
      });
      if (q) searchParams.set("q", q);
      if (cursor) searchParams.set("cursor", cursor);
      const page = await forwardGoldenPage(
        access,
        `/v1/orgs/${org}/datasets/${ds}/golden-records`,
        {
          searchParams,
        },
      );
      snapshot ??= page.snapshot;
      for (const item of page.items) {
        const row = item as unknown as Record<string, unknown>;
        // F4: filtered HERE rather than upstream, because the read API has no
        // predicate for "has an email" — and adding one would be a declared
        // lake read for a question the exporter can answer from the page it
        // already holds.
        if (!isExportable(row, requirement)) {
          skipped += 1;
          continue;
        }
        lines.push(csvRow(COLUMNS.map((column) => row[column])));
      }
      if (!page.nextCursor) break;
      cursor = page.nextCursor;
    }

    return new NextResponse(lines.join("\n") + "\n", {
      headers: {
        "content-type": "text/csv; charset=utf-8",
        "content-disposition": `attachment; filename="${csvFilename(`segment-${org}`)}.csv"`,
        "x-snapshot": String(snapshot ?? ""),
        // The count LEFT OUT, reported rather than inferred: an export that
        // silently returns 7,000 of 10,000 rows looks like a broken query, and
        // the reader's next move is to re-run it rather than trust it.
        "x-export-summary": exportSummary(lines.length - 1, skipped, requirement),
        "x-export-skipped": String(skipped),
      },
    });
  });
}
