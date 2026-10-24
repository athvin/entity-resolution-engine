import { NextResponse } from "next/server";

import { resolveAiConfig } from "@/lib/ai/config";
import { adjudicateReview } from "@/lib/adjudicate/run";
import { parseBulkAdjudicate } from "@/lib/bff/adjudicate-bulk";
import { BffFailure, requireOrgAccess, respond } from "@/lib/bff/proxy";

/** One pair's outcome. A failure is per-item, never fatal to the batch. */
interface BulkOutcome {
  review_id: string;
  ok: boolean;
  verdict?: string;
  error?: string;
}

/**
 * How many pairs are in flight at once.
 *
 * Sequential would make ten pairs a minute-long request; unbounded would open
 * ten concurrent model calls and ten pairs of upstream reads against one
 * tenant's control plane. Three is the middle: the wall time is roughly a
 * third, and the load on er-server stays comparable to three people using the
 * inbox at once, which it already handles.
 */
const CONCURRENCY = 3;

/**
 * Adjudicate several gray-band pairs in one request (V6's bulk half).
 *
 * **Every item is independent.** One pair that cannot be judged — an
 * entity-level escalation, a record tombstoned since the queue read, a model
 * reply that did not parse — is reported against its own id and the rest
 * proceed. A batch that aborted on the first failure would be useless on
 * exactly the queues worth running it on, since those are the ones with
 * irregular rows in them.
 *
 * The response is therefore **always 200 with per-item outcomes**, not a status
 * code for the batch: "7 of 10 answered" has no HTTP status, and picking one
 * would misreport the other seven.
 *
 * Steward+, like the single route, for the same reason: a suggestion precedes a
 * write and sits behind the same role as the write it precedes. The cost cap
 * lives in `parseBulkAdjudicate`.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "steward");
    const parsed = parseBulkAdjudicate(await request.json().catch(() => null));
    if (!parsed.ok) {
      throw new BffFailure(422, "validation", parsed.reason);
    }

    // Resolved ONCE for the batch rather than per item: it is a database read
    // plus a decrypt, and it cannot change mid-request. Checking it up front
    // also means an AI-off tenant gets one clean 503 instead of ten identical
    // per-item errors.
    const ai = await resolveAiConfig(org, "assistant");
    if (!ai.ok) {
      throw new BffFailure(503, "internal", `adjudication is unavailable: ${ai.reason}`);
    }

    const queue = [...parsed.value.review_ids];
    const outcomes: BulkOutcome[] = [];
    const worker = async () => {
      for (;;) {
        const reviewId = queue.shift();
        if (reviewId === undefined) return;
        try {
          const stored = await adjudicateReview(access, org, reviewId, ai);
          outcomes.push({ review_id: reviewId, ok: true, verdict: stored.verdict });
        } catch (error) {
          outcomes.push({
            review_id: reviewId,
            ok: false,
            // `BffFailure.message` is the explanation written for a human at
            // the point of refusal; anything else is reported generically
            // rather than leaking an upstream error's internals.
            error:
              error instanceof BffFailure ? error.message : "this pair could not be adjudicated",
          });
        }
      }
    };
    await Promise.all(Array.from({ length: Math.min(CONCURRENCY, queue.length) }, worker));

    return NextResponse.json({
      // Ordered by the request rather than by completion, so the caller can
      // match outcomes to what it asked for without sorting.
      outcomes: parsed.value.review_ids.map(
        (id) =>
          outcomes.find((outcome) => outcome.review_id === id) ?? {
            review_id: id,
            ok: false,
            error: "this pair could not be adjudicated",
          },
      ),
      answered: outcomes.filter((outcome) => outcome.ok).length,
      asked: parsed.value.review_ids.length,
    });
  });
}
