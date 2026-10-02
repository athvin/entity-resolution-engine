import { NextResponse } from "next/server";

import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";
import {
  DEFAULT_MASTER_ELECTION_POLICY,
  isMasterElectionPolicy,
} from "@/lib/domain/master-election";

interface MergePlan {
  entity_id: string;
  master_key: string;
  member_count: number;
  member_records: string[];
  golden_given_name: string | null;
  golden_family_name: string | null;
  golden_email: string | null;
  golden_phone_e164: string | null;
}

/** The server's own CSV header, in its order — the file must not depend on which
 * of the two producers a caller reached. */
const COLUMNS = [
  "entity_id",
  "master_key",
  "member_count",
  "member_records",
  "golden_given_name",
  "golden_family_name",
  "golden_email",
  "golden_phone_e164",
] as const;

/** Every value a merge plan can carry. Narrower than `unknown` on purpose: the
 * shape is known here, so a field that stopped being one of these should be a
 * type error rather than an `[object Object]` in someone's spreadsheet. */
type PlanValue = MergePlan[(typeof COLUMNS)[number]];

function csvCell(value: PlanValue): string {
  if (value === null) return "";
  // `member_records` is the one list, and `;` is the separator the server's own
  // CSV writer uses for it.
  const text = Array.isArray(value) ? value.join(";") : String(value);
  return /[",\n]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text;
}

/**
 * The pre-merge report (Cloudingo parity): every entity the engine would merge,
 * which member it would keep as master, and the golden values that survive.
 *
 * Asks erserver for JSON and renders the CSV here rather than proxying its
 * `format=csv` bytes, because `forward()` parses JSON — the same shape
 * `segments/export` uses. The column list is duplicated from the server's
 * writer deliberately: a reader comparing the two files should get the same
 * header, and a drift here is a visible diff rather than a silent reordering.
 *
 * `policy` is validated against the mirrored vocabulary before the call so a
 * typo is a 422 from us with the list in it, not a round trip. The server
 * validates again — it is the authority, and this is only the early answer.
 */
export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const url = new URL(request.url);

    const policy = url.searchParams.get("policy") ?? DEFAULT_MASTER_ELECTION_POLICY;
    if (!isMasterElectionPolicy(policy)) {
      throw new BffFailure(422, "validation", `unknown master-election policy ${policy}`);
    }
    const since = url.searchParams.get("since");
    const format = url.searchParams.get("format") ?? "csv";
    if (format !== "csv" && format !== "json") {
      throw new BffFailure(422, "validation", "format must be csv or json");
    }

    const searchParams = new URLSearchParams({ policy, format: "json" });
    if (since) searchParams.set("since", since);
    const { items } = await forward<{ items: MergePlan[] }>(access, `/v1/orgs/${org}/merge-plans`, {
      searchParams,
    });

    if (format === "json") {
      return NextResponse.json({ items, policy });
    }

    const lines = [COLUMNS.join(",")];
    for (const plan of items) {
      lines.push(COLUMNS.map((column) => csvCell(plan[column])).join(","));
    }
    return new NextResponse(lines.join("\n") + "\n", {
      headers: {
        "content-type": "text/csv; charset=utf-8",
        // The policy is in the filename because the choice is invisible in the
        // rows: two reports of one lake differ only in `master_key`.
        "content-disposition": `attachment; filename="pre-merge-${org}-${policy}.csv"`,
      },
    });
  });
}
