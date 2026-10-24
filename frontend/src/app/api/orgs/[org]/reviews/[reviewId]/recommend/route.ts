import { NextResponse } from "next/server";

import { resolveAiConfig } from "@/lib/ai/config";
import { adjudicateReview } from "@/lib/adjudicate/run";
import { BffFailure, requireOrgAccess, respond } from "@/lib/bff/proxy";

/**
 * Ask the model to adjudicate one gray-band review pair (V6).
 *
 * **It recommends and never resolves.** Acting on the answer is the steward's
 * existing `POST .../resolve`, so every applied recommendation is audited like
 * any human decision and no second merge path exists. There is deliberately no
 * auto-apply in v1 — not a disabled flag, which would be dead config, but
 * genuinely absent until someone asks for it.
 *
 * Steward+, matching the enrich route: a suggestion precedes a write, so it
 * sits behind the same role as the write it precedes.
 *
 * The pipeline itself is `adjudicateReview`, shared with the bulk route so the
 * two cannot drift.
 */
export async function POST(
  _request: Request,
  { params }: { params: Promise<{ org: string; reviewId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, reviewId } = await params;
    const access = await requireOrgAccess(org, "steward");

    // The ASSISTANT purpose, not enrich. Both defaults are the same model
    // today, so this is about which knob moves it later: adjudication is a
    // judgement about identity that feeds a merge, which is the highest-stakes
    // model output in the product, and it should ride the deployment's primary
    // model rather than whatever was chosen for bulk field cleanup. A tenant's
    // own override (V5) applies to both surfaces regardless.
    const ai = await resolveAiConfig(org, "assistant");
    if (!ai.ok) {
      throw new BffFailure(503, "internal", `adjudication is unavailable: ${ai.reason}`);
    }

    return NextResponse.json(await adjudicateReview(access, org, reviewId, ai));
  });
}
