import "server-only";
import { z } from "zod";

/**
 * Config promotion between orgs (O9), the validated request shapes.
 *
 * The dev→prod story: a tenant's config is already versioned and diffable
 * *within* an org; this adds the cross-environment half — read a version from
 * one org, see it against another's, and copy it in as a draft.
 *
 * **The authorization rule is the whole feature.** A promotion names TWO orgs,
 * and the obvious mistake is to check the one in the URL. Both are checked, at
 * admin, independently — so a person who administers staging but merely views
 * production cannot read production's config through the compare endpoint, nor
 * push staging's into it. `requireOrgAccess` is called twice rather than once
 * with a list, because that is the call the authz gate recognises and the one
 * every other route uses.
 */

/** A target org name, bounded the same way the slug guard bounds one. */
const orgName = z.string().trim().min(1).max(64);

export const compareSchema = z.object({
  /** The org whose config is being compared against the one in the URL. */
  against: orgName,
  /** Which version of `against` to read; absent means its published one. */
  version: z.number().int().min(1).optional(),
});

export const importSchema = z.object({
  /** The org to copy FROM. The URL names the org copied INTO. */
  from: orgName,
  version: z.number().int().min(1).optional(),
});

export type CompareInput = z.infer<typeof compareSchema>;
export type ImportInput = z.infer<typeof importSchema>;

function explain<T>(
  parsed: { success: true; data: T } | { success: false; error: z.ZodError },
  fallback: string,
): { ok: true; value: T } | { ok: false; reason: string } {
  if (parsed.success) return { ok: true, value: parsed.data };
  const first = parsed.error.issues[0];
  return {
    ok: false,
    reason: first === undefined ? fallback : `${first.path.join(".") || "body"}: ${first.message}`,
  };
}

export function parseCompare(
  body: unknown,
): { ok: true; value: CompareInput } | { ok: false; reason: string } {
  return explain(compareSchema.safeParse(body), "an `against` org is required");
}

export function parseImport(
  body: unknown,
): { ok: true; value: ImportInput } | { ok: false; reason: string } {
  return explain(importSchema.safeParse(body), "a `from` org is required");
}

/**
 * Is this promotion self-referential?
 *
 * Copying an org's config into itself is not harmful — it would create a draft
 * identical to the published version — but it is always a mistake, and the
 * draft it leaves behind looks like a pending change nobody made. Refused by
 * name so the reader learns what they actually did.
 */
export function isSameOrg(a: string, b: string): boolean {
  return a.trim().toLowerCase() === b.trim().toLowerCase();
}
