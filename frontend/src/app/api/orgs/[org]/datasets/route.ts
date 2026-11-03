import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, DATASET_SLUG, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/** The slug rules erserver enforces, checked here so the message names the
 * rule rather than arriving as a bare upstream 422. `datasets` and `settings`
 * are refused because they are static segments under `/{org}/`. */
const RESERVED = new Set(["datasets", "settings"]);

const createSchema = z.object({
  name: z.string().trim().min(1).max(64),
  display_name: z.string().trim().min(1).max(200).optional(),
  entity_type: z.enum(["person", "account"]).optional(),
});

/**
 * The org's datasets — the switcher's source of truth.
 *
 * Viewer, because knowing which resolution domains exist is the price of
 * rendering navigation at all; everything a dataset CONTAINS sits behind the
 * dataset-scoped routes and their own checks.
 */
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    // Bare array upstream (`list_datasets_route` returns `Json(json!(out))`),
    // like webhooks — checked against the handler rather than assumed.
    const datasets = await forward<unknown[]>(access, `/v1/orgs/${org}/datasets`);
    return NextResponse.json(datasets);
  });
}

/**
 * Create a dataset. Admin, because a dataset is a whole resolution domain —
 * its own lake, models and spend — not a view onto an existing one.
 *
 * Answers 202, not 201: erserver returns the `provisioning` row plus the
 * `job_id` doing the slow half (the lake's first attach, the starter config,
 * the flip to active). The wizard polls the detail endpoint from there.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "admin");
    const parsed = createSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "a name is required");
    }
    const { name } = parsed.data;
    if (!DATASET_SLUG.test(name)) {
      throw new BffFailure(
        422,
        "validation",
        "a dataset name is lowercase letters, digits, dashes and underscores, starting with a letter or digit",
      );
    }
    if (RESERVED.has(name)) {
      throw new BffFailure(422, "validation", `"${name}" is reserved — pick another name`);
    }
    const created = await forward<unknown>(access, `/v1/orgs/${org}/datasets`, {
      method: "POST",
      // `role` defaults to viewer AND selects the stored key, so omitting it
      // sends a viewer credential to an admin-only endpoint.
      role: "admin",
      body: parsed.data,
    });
    return NextResponse.json(created, { status: 202 });
  });
}
