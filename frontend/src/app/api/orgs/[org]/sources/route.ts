import { NextResponse } from "next/server";
import { parse } from "yaml";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

interface ConfigVersion {
  version: number;
  yaml: string;
}

interface SourceInfo {
  name: string;
  priority_rank: number | null;
  drop_subdir: string | null;
}

/** The org's configured sources, read from the active config's `sources:` block. */
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const config = await forward<ConfigVersion>(access, `/v1/orgs/${org}/config`);
    let sources: SourceInfo[] = [];
    try {
      const parsed: unknown = parse(config.yaml);
      if (typeof parsed === "object" && parsed !== null && "sources" in parsed) {
        const block = (parsed as { sources: Record<string, unknown> }).sources;
        sources = Object.entries(block).map(([name, spec]) => {
          const record =
            typeof spec === "object" && spec !== null ? (spec as Record<string, unknown>) : {};
          return {
            name,
            priority_rank: typeof record.priority_rank === "number" ? record.priority_rank : null,
            drop_subdir: typeof record.drop_subdir === "string" ? record.drop_subdir : null,
          };
        });
      }
    } catch {
      // Unparseable config is the server's problem to report elsewhere; the
      // sources page degrades to an empty list rather than a 500.
    }
    return NextResponse.json({ config_version: config.version, sources });
  });
}
