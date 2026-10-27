import { z } from "zod";

/**
 * Saved views (O12a), the validated vocabulary.
 *
 * A view is a named filter a human returns to. The definition is always an
 * INSPECTABLE structured filter — never a prompt and never raw SQL — for the
 * same reason `segments.definition` is: the thing a person saves has to be the
 * thing they can read back and edit, and anything else makes the saved state a
 * black box they stop trusting.
 */

/** The screens a view can belong to. Closed, because a records filter is
 * meaningless on the duplicates queue and offering it is a dead control. */
export const VIEW_SURFACES = ["records", "duplicates", "reviews"] as const;
export type ViewSurface = (typeof VIEW_SURFACES)[number];

/**
 * What a view may filter on, per surface.
 *
 * Deliberately limited to what each screen can already evaluate TODAY. A view
 * carrying a filter nothing applies is worse than no view: it restores a state
 * the reader believes is filtered and is not, which is the silent-wrongness
 * shape this program keeps finding.
 */
export const definitionSchema = z.object({
  /** Server-side search, where the surface has one. */
  q: z.string().trim().max(200).default(""),
  /** Review/duplicate band bounds, 0–1, where the surface has them. */
  min: z.number().min(0).max(1).nullable().default(null),
  max: z.number().min(0).max(1).nullable().default(null),
  /** A review reason or duplicate state, where the surface filters by one. */
  reason: z.string().trim().max(100).nullable().default(null),
});

export type ViewDefinition = z.infer<typeof definitionSchema>;

export const createViewSchema = z.object({
  surface: z.enum(VIEW_SURFACES),
  name: z.string().trim().min(1).max(80),
  definition: definitionSchema,
});

export type CreateViewInput = z.infer<typeof createViewSchema>;

export function parseCreateView(
  body: unknown,
): { ok: true; value: CreateViewInput } | { ok: false; reason: string } {
  const parsed = createViewSchema.safeParse(body);
  if (parsed.success) {
    const { min, max } = parsed.data.definition;
    // A band whose floor is above its ceiling matches nothing, and the UI shows
    // an empty list rather than an error — so it is refused here, where the
    // reader can still see what they typed.
    if (min !== null && max !== null && min > max) {
      return { ok: false, reason: "definition: the band's minimum is above its maximum" };
    }
    return { ok: true, value: parsed.data };
  }
  const first = parsed.error.issues[0];
  return {
    ok: false,
    reason:
      first === undefined
        ? "surface, name and definition are required"
        : `${first.path.join(".") || "body"}: ${first.message}`,
  };
}

/**
 * Is this view's filter actually filtering anything?
 *
 * A view that narrows nothing is a bookmark for "everything", which is the
 * default state the screen already opens in. Saving one is always a mistake —
 * usually someone pressing Save before setting the filter — and the name they
 * chose makes it look deliberate to whoever finds it later.
 */
export function isEmptyDefinition(definition: ViewDefinition): boolean {
  return (
    definition.q === "" &&
    definition.min === null &&
    definition.max === null &&
    definition.reason === null
  );
}
