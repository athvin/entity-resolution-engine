import { z } from "zod";

/**
 * The AI-settings write, validated at the BFF boundary.
 *
 * `.nullable().optional()` on both mutable fields is the whole point: this
 * endpoint has to distinguish three states per field, and collapsing any two of
 * them loses a tenant's key or their model choice.
 *
 * | body | meaning |
 * |---|---|
 * | field absent | leave the stored value alone |
 * | `null` | clear it — back to the deployment default |
 * | a string | set it |
 *
 * A PUT that treated absence as "clear" would delete a working key every time
 * an admin flipped the enable toggle, which is the kind of data loss that looks
 * like the key "just stopped working".
 */
export const aiSettingsSchema = z.object({
  enabled: z.boolean(),
  /** No allowlist — see `parseAiSettings` for why a closed set is wrong here. */
  model: z.string().trim().min(1).max(200).nullable().optional(),
  /** Anthropic keys are long; the bound is a sanity check, not a format claim. */
  api_key: z.string().trim().min(8).max(500).nullable().optional(),
});

export type AiSettingsInput = z.infer<typeof aiSettingsSchema>;

/**
 * Parse an AI-settings body, or say which field is wrong.
 *
 * **Why `model` is not a closed set**, unlike `subject_type` and
 * `webhook.events`: those are closed because an unconstrained value is
 * accepted, written, and then matches nothing on read — a silence nobody can
 * diagnose. A wrong model id is not silent. It is a loud 404 from the model
 * API on the very next call, and the PUT handler makes it louder still by
 * verifying the model before it saves. An allowlist here would instead have to
 * track every model id across both the public Anthropic API and whatever
 * Bedrock inference profiles a deployment is pointed at, and would reject a
 * newly released model that works.
 */
export function parseAiSettings(
  body: unknown,
): { ok: true; value: AiSettingsInput } | { ok: false; reason: string } {
  const parsed = aiSettingsSchema.safeParse(body);
  if (parsed.success) {
    return { ok: true, value: parsed.data };
  }
  const first = parsed.error.issues[0];
  return {
    ok: false,
    reason:
      first === undefined
        ? "enabled is required; model and api_key are optional strings or null"
        : `${first.path.join(".") || "body"}: ${first.message}`,
  };
}
