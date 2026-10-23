import { z } from "zod";

/**
 * The shape of a recorded verdict, validated at the BFF boundary.
 *
 * Validated rather than passed through for two reasons beyond tidiness. A 422
 * raised here names the field, where an upstream `detail` string would have to
 * be parsed by the UI to say anything useful. And `score_at_label` is the one
 * field whose *range* matters rather than its type: it is the probability the
 * labeller was shown, so a value outside [0,1] is not a probability and would
 * make every precision figure computed from it meaningless, silently.
 *
 * `model_version` and `score_at_label` are `nullish` rather than required
 * because a pair scored before the model registry existed genuinely has
 * neither. A label without them is still valid training signal; it simply
 * cannot be measured by per-version evaluation.
 */
export const labelSchema = z.object({
  rec_a_key: z.string().min(1).max(200),
  rec_b_key: z.string().min(1).max(200),
  verdict: z.enum(["match", "no_match"]),
  model_version: z.string().min(1).max(100).nullish(),
  score_at_label: z.number().min(0).max(1).nullish(),
  note: z.string().max(2000).optional(),
});

export type LabelBody = z.infer<typeof labelSchema>;

/**
 * Parse a verdict request body, or say why it is not one.
 *
 * Returns a result rather than throwing so the route decides the status code,
 * and so this is testable without a `Request`.
 */
export function parseLabelInput(
  body: unknown,
): { ok: true; value: LabelBody } | { ok: false; reason: string } {
  const parsed = labelSchema.safeParse(body);
  if (parsed.success) {
    // A pair cannot be compared against itself, and the engine would never have
    // scored such a pair — so a verdict on one describes nothing.
    if (parsed.data.rec_a_key === parsed.data.rec_b_key) {
      return { ok: false, reason: "a record cannot be labelled against itself" };
    }
    return { ok: true, value: parsed.data };
  }
  const first = parsed.error.issues[0];
  return {
    ok: false,
    reason:
      first === undefined
        ? "a pair and a verdict of match or no_match"
        : `${first.path.join(".") || "body"}: ${first.message}`,
  };
}
