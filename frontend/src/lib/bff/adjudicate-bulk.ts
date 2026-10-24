import { z } from "zod";

/**
 * How many pairs one bulk request may adjudicate.
 *
 * **This is the cost cap**, and it is a count rather than a token budget
 * because a count is the number a steward can see before pressing the button —
 * "ask about 10 pairs" is a decision someone can make; "spend 40k tokens" is
 * not, and the UI could only estimate it anyway.
 *
 * Ten rather than a round fifty: each pair costs two upstream reads plus a
 * model round trip, so fifty would park a request for minutes and risk a
 * platform timeout that leaves the caller unable to tell which pairs were
 * stored. A steward wanting more presses the button again, and the second
 * press is cheap because stored recommendations are not re-asked.
 */
export const MAX_BULK = 10;

export const bulkAdjudicateSchema = z.object({
  review_ids: z.array(z.string().trim().min(1).max(200)).min(1).max(MAX_BULK),
});

export type BulkAdjudicateInput = z.infer<typeof bulkAdjudicateSchema>;

/**
 * Parse a bulk-adjudication body, or say what is wrong with it.
 *
 * The cap is enforced here rather than by truncating the list. Silently
 * dropping the tail would report success for pairs that were never asked
 * about — the caller sees "done" and a blank badge, which reads as "the model
 * had no opinion" rather than "we did not ask".
 */
export function parseBulkAdjudicate(
  body: unknown,
): { ok: true; value: BulkAdjudicateInput } | { ok: false; reason: string } {
  const parsed = bulkAdjudicateSchema.safeParse(body);
  if (parsed.success) {
    // De-duplicated after validation so a repeated id does not spend a second
    // model call on the same pair — and so the count the caller sees back
    // matches the number of pairs actually judged.
    return { ok: true, value: { review_ids: [...new Set(parsed.data.review_ids)] } };
  }
  const first = parsed.error.issues[0];
  return {
    ok: false,
    reason:
      first === undefined
        ? `review_ids must be 1 to ${String(MAX_BULK)} review ids`
        : `${first.path.join(".") || "body"}: ${first.message}`,
  };
}
