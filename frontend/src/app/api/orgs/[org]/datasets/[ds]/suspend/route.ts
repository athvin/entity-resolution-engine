import { NextResponse } from "next/server";

import { datasetPath, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

/**
 * Freeze one dataset. Admin.
 *
 * Suspending a dataset stops its work without touching the org's other
 * datasets — the per-dataset answer to the org-wide freeze. Every
 * dataset-scoped route then answers 409 naming this dataset, which the BFF
 * surfaces as `dataset_not_active` rather than the org's own refusal.
 */
export async function POST(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "admin");
    const result = await forward<unknown>(access, datasetPath(access, "/suspend"), {
      method: "POST",
      role: "admin",
    });
    return NextResponse.json(result);
  });
}
