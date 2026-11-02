import { NextResponse } from "next/server";

import { BffFailure, datasetPath, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";
import { parseTagApplication } from "@/lib/bff/tags";

/**
 * Remove a tag from a subject. Steward+, matching apply.
 *
 * Unlike retiring a tag this is a real delete upstream — nothing references a
 * tagging, so there is no foreign key to fail on. The history companion keeps
 * the row image, so who removed it stays answerable.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string; tagId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds, tagId } = await params;
    const access = await requireDatasetAccess(org, ds, "steward");
    const parsed = parseTagApplication(await request.json());
    if (!parsed.ok) {
      throw new BffFailure(422, "validation", "subject_type of entity or review, and a subject_id");
    }
    await forward<unknown>(
      access,
      datasetPath(access, `/tags/${encodeURIComponent(tagId)}/unapply`),
      {
        method: "POST",
        role: "steward",
        body: parsed.value,
      },
    );
    return NextResponse.json({ removed: true });
  });
}
