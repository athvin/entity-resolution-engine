import { z } from "zod";

/**
 * A tag name, validated at the BFF boundary.
 *
 * Trimmed before the length check, so `"   "` is empty rather than three
 * characters — er-server trims too, and a name that differs only by whitespace
 * would otherwise collide with an existing one and surface as a 409 the caller
 * cannot explain.
 */
export const tagSchema = z.object({
  name: z.string().trim().min(1).max(100),
  description: z.string().max(1000).optional(),
});

/**
 * Applying or removing a tag: which subject, addressed the only way er-server
 * accepts.
 *
 * `subject_type` is a closed set in both places. It is closed for the reason
 * `webhook.events` was closed: an unconstrained value is accepted, written, and
 * then matches nothing on read — a silence nobody can diagnose, because the row
 * exists and no filter looks for it.
 */
export const tagApplicationSchema = z.object({
  subject_type: z.enum(["entity", "review"]),
  subject_id: z.string().trim().min(1).max(200),
});

export type TagApplication = z.infer<typeof tagApplicationSchema>;
export type TagInput = z.infer<typeof tagSchema>;

/** Parse a tag-creation body, or say which field is wrong. */
export function parseTagInput(
  body: unknown,
): { ok: true; value: TagInput } | { ok: false; reason: string } {
  return explain(tagSchema.safeParse(body), "a tag name of 1 to 100 characters");
}

/** Parse an apply/unapply body, or say which field is wrong. */
export function parseTagApplication(
  body: unknown,
): { ok: true; value: TagApplication } | { ok: false; reason: string } {
  return explain(
    tagApplicationSchema.safeParse(body),
    "subject_type of entity or review, and a subject_id",
  );
}

/**
 * Turn a parse result into an outcome the route can map to a status.
 *
 * Returns rather than throws so the route owns the status code, and so these
 * are testable without constructing a `Request`.
 */
function explain<T>(
  // Structural rather than zod's own result type: v4 renamed
  // `SafeParseReturnType`, and a shape this small should not pin a major.
  parsed: { success: true; data: T } | { success: false; error: z.ZodError },
  fallback: string,
): { ok: true; value: T } | { ok: false; reason: string } {
  if (parsed.success) {
    return { ok: true, value: parsed.data };
  }
  const first = parsed.error.issues[0];
  return {
    ok: false,
    reason: first === undefined ? fallback : `${first.path.join(".") || "body"}: ${first.message}`,
  };
}
