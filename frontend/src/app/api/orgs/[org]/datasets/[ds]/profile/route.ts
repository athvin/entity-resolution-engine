import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

/**
 * Validated here as well as upstream, because a 422 that names the field beats
 * a forwarded one: the server's parser is shared with the dispatcher and
 * phrases its refusals for a job, not a form.
 *
 * `source` is required for `raw` and meaningless otherwise — a raw profile is
 * per-source, since two sources landing in one dataset have different columns.
 */
const profileSchema = z
  .object({
    subject: z.enum(["raw", "standardized", "golden"]),
    source: z.string().min(1).optional(),
    tier: z.union([z.literal(1), z.literal(2)]),
  })
  .refine((v) => v.subject !== "raw" || (v.source ?? "") !== "", {
    message: "source is required when subject is raw",
  });

/**
 * Query keys forwarded verbatim. An allowlist rather than a passthrough of the
 * incoming string: `subject` and `source` reach a lake read, and forwarding
 * whatever arrived would let a caller append parameters this route never
 * reasoned about.
 */
const PASSTHROUGH = ["subject", "source", "tier"] as const;

/**
 * What is in this corpus: every column's shape, the deterministic findings, and
 * what each column would cost as a blocking key.
 *
 * Cheap, unlike `blocking-estimate` — it reads three lake tables a profile job
 * already wrote rather than counting pairs, so it is safe to fetch on page
 * load. A corpus nobody has profiled comes back `{"profiled": false}`, which is
 * an answer and not a failure: the POST below is the affordance.
 *
 * Five fields of each column are nulled for a viewer and populated for a
 * steward (real values — see `er_server::read::VALUE_BEARING_COLUMN_FIELDS`).
 * That decision is made upstream, by role, and this route must not try to
 * improve on it: the session's role is already what `requireDatasetAccess`
 * resolved, and er-server re-derives it from the forwarded principal.
 */
export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const incoming = new URL(request.url).searchParams;
    const searchParams = new URLSearchParams();
    for (const key of PASSTHROUGH) {
      const value = incoming.get(key);
      if (value !== null) searchParams.set(key, value);
    }
    const profile = await forward<unknown>(access, `/v1/orgs/${org}/datasets/${ds}/profile`, {
      searchParams,
    });
    return NextResponse.json(profile);
  });
}

/**
 * Queue a profile. Steward+, because it writes three lake tables and so takes
 * the dataset's one writer slot.
 *
 * Returns the job id upstream sent rather than waiting: a deep tier walks the
 * whole corpus, and the caller follows it with the same job polling every other
 * queued kind uses.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "steward");
    const parsed = profileSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(
        422,
        "validation",
        parsed.error.issues[0]?.message ?? "subject and tier are required",
      );
    }
    const queued = await forward<unknown>(access, `/v1/orgs/${org}/datasets/${ds}/profile`, {
      method: "POST",
      role: "steward",
      body: parsed.data,
    });
    // 202 upstream, 202 here: the work has not happened yet, and a 200 would
    // say it had.
    return NextResponse.json(queued, { status: 202 });
  });
}
