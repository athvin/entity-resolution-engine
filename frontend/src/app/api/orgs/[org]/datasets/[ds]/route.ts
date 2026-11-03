import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, datasetPath, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

/** Only the label is editable. The routing `name` is in every URL, bookmark
 * and saved view, so renaming it would break them silently. */
const patchSchema = z.object({
  display_name: z.string().trim().min(1).max(200).nullable(),
});

/**
 * One dataset's detail — what the `[ds]` layout's guard renders its
 * provisioning / suspended states from. A name the org does not have is
 * erserver's 404, passed through as-is.
 */
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const dataset = await forward<unknown>(access, datasetPath(access, ""));
    return NextResponse.json(dataset);
  });
}

/**
 * Rename a dataset's label. Admin: the name appears in the switcher every
 * member of the org reads.
 */
export async function PATCH(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "admin");
    const parsed = patchSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "display_name must be a label or null to clear it");
    }
    const updated = await forward<unknown>(access, datasetPath(access, ""), {
      method: "PATCH",
      role: "admin",
      body: parsed.data,
    });
    return NextResponse.json(updated);
  });
}
