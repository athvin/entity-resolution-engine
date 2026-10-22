import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/**
 * `model_version_label` is REQUIRED, not derived: it must equal the
 * `model_version` the frozen term-frequency snapshot is keyed by, and a guess
 * that disagrees changes every pair's score instead of failing — the TF lookup
 * joins on it and silently returns no term frequencies on a miss.
 */
const uploadSchema = z.object({
  model_json: z
    .string()
    .min(1)
    .max(8 * 1024 * 1024),
  model_version_label: z.string().min(1).max(100),
  name: z.string().min(1).max(100).optional(),
  tf_snapshot_id: z.string().min(1).max(100).optional(),
});

/** Mint a new version. Admin-only, and it lands as a `draft`. */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "admin");
    const parsed = uploadSchema.safeParse(await request.json());
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "model_json and model_version_label are required");
    }
    const created = await forward<unknown>(access, `/v1/orgs/${org}/models/versions`, {
      method: "POST",
      role: "admin",
      body: parsed.data,
    });
    return NextResponse.json(created, { status: 201 });
  });
}
