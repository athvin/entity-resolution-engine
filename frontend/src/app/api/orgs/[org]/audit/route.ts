import { NextResponse } from "next/server";

import { csvFilename, csvRow } from "@/lib/bff/csv";
import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

const PASSTHROUGH = ["action", "actor", "since", "until", "before_id", "limit"] as const;

/** The columns the CSV export carries, in the order the server's own export uses. */
const CSV_COLUMNS = [
  "id",
  "at",
  "actor",
  "org",
  "dataset",
  "action",
  "trace_id",
  "detail",
] as const;

/**
 * The tenant-facing audit feed (design §7.20) — admin-gated, like the server.
 *
 * `format=csv` downloads the same page. Rendered here rather than proxied —
 * `forward()` parses JSON, the same reason the merge-plans route re-renders —
 * through the shared csv helpers, whose cells are formula-guarded.
 */
export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "admin");
    const incoming = new URL(request.url).searchParams;
    const searchParams = new URLSearchParams();
    for (const key of PASSTHROUGH) {
      const value = incoming.get(key);
      if (value !== null) searchParams.set(key, value);
    }
    const rows = await forward<Record<string, unknown>[]>(access, `/v1/orgs/${org}/audit`, {
      role: "admin",
      searchParams,
    });
    if (incoming.get("format") === "csv") {
      const lines = [
        csvRow(CSV_COLUMNS),
        ...rows.map((row) =>
          csvRow(
            CSV_COLUMNS.map((column) =>
              typeof row[column] === "object" && row[column] !== null
                ? JSON.stringify(row[column])
                : row[column],
            ),
          ),
        ),
      ];
      return new NextResponse(lines.join("\n") + "\n", {
        headers: {
          "content-type": "text/csv; charset=utf-8",
          "content-disposition": `attachment; filename="${csvFilename(`audit-${org}`)}"`,
        },
      });
    }
    return NextResponse.json(rows);
  });
}
