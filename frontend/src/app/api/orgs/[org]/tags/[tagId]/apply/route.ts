import { NextResponse } from "next/server";

import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";
import { parseTagApplication } from "@/lib/bff/tags";

/**
 * Apply a tag to a subject. Steward+, because application is role-restricted:
 * tagging is how work is assigned, so who claimed it matters.
 *
 * Idempotent upstream — the unique index makes a repeat a no-op rather than a
 * second row — so the UI can fire this without first checking.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; tagId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, tagId } = await params;
    const access = await requireOrgAccess(org, "steward");
    const parsed = parseTagApplication(await request.json());
    if (!parsed.ok) {
      throw new BffFailure(422, "validation", "subject_type of entity or review, and a subject_id");
    }
    await forward<unknown>(access, `/v1/orgs/${org}/tags/${encodeURIComponent(tagId)}/apply`, {
      method: "POST",
      // `role` defaults to viewer AND selects the stored key.
      role: "steward",
      body: parsed.value,
    });
    return NextResponse.json({ applied: true });
  });
}
