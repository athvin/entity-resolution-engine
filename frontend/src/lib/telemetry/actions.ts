/**
 * The closed vocabulary of interactions worth measuring.
 *
 * ## Why this is a closed set and not a free string
 *
 * Two independent reasons, and the first is the one that forced the file to
 * exist.
 *
 * **It is the PII boundary.** An action name travels to the span store, so it
 * has to be a value no author can accidentally fill with a record. The first
 * version of the resolver fell back to `data-testid` and then `aria-label` on
 * the theory that an author-written attribute is a safe one. In this app it is
 * not: `members-list.tsx` writes `data-testid={`member-remove-${member.email}`}`
 * and `aria-label={`Remove ${member.email}`}`, and `entity-view.tsx` writes
 * `aria-label={`Split out ${member.record_key}`}`. Both are *correct* — a screen
 * reader user needs to know which member they are removing, and a test needs to
 * address one row — so neither can be changed to suit telemetry. Accessibility
 * and telemetry want opposite things from the same attribute, and accessibility
 * wins. The only safe source is an attribute that exists solely for this, whose
 * legal values are enumerated here.
 *
 * **It bounds cardinality by construction.** An interaction name reaching a
 * dashboard dimension must come from a finite set, and a set defined by "whatever
 * authors typed" is not finite.
 *
 * ## Both directions are enforced
 *
 * `scripts/check-er-actions.mjs` fails the frontend gate unless every member
 * here appears in at least one `data-er-action=` attribute or `recordAction(…)`
 * call, and every such attribute or call names a member. This copies the
 * reasoning already written in `lib/usage/record.ts` — *"the catalog had to be
 * made true, not just written"* — because a vocabulary nothing emits is worse
 * than no vocabulary: it reads like coverage.
 *
 * ## Naming
 *
 * `area.thing` or `area.thing.variant`, lowercase, dot-separated. The area is
 * the product surface rather than the component, so moving a button between
 * components does not rename the metric.
 */

/**
 * Every interaction the app may report.
 *
 * Append-only in practice: a renamed action breaks continuity in the span store,
 * which is the same reason entity lint codes are append-only. Removing one is
 * fine when the control itself goes away.
 */
export const UI_ACTIONS = [
  // --- the pipeline ------------------------------------------------------
  /** Start a resolution run from the runs page. The highest-value click here:
   *  it is the one that spends money, and it begins the trace that ends in a
   *  SQL statement. */
  "run.start",

  // --- stewardship: one pair at a time ----------------------------------
  // The review inbox is keyboard-first — the UI advertises `m`, `n` and `x`
  // with `<kbd>` hints — so these are reported from BOTH the button's
  // `data-er-action` and the key handler's `recordAction`. A click-only
  // listener would record nothing for the steward who clears a queue from the
  // keyboard, which is the steward who clears the most.
  "review.decide.match",
  "review.decide.no_match",
  "review.decide.dismiss",

  // --- stewardship: in bulk ---------------------------------------------
  // Separate from the single-pair actions on purpose. A bulk resolve is a
  // different cost and a different risk, and averaging it together with
  // single decisions hides both.
  /** The merge dialog's submit: a pair decision PLUS the group edit (master
   *  choice and field pins) riding with it. */
  "review.merge.confirm",

  "review.bulk.match",
  "review.bulk.no_match",
  "review.bulk.recommend",

  // --- training labels --------------------------------------------------
  // Also keyboard-first (`m`, `n`, `s`).
  "label.match",
  "label.no_match",
  /** Recorded even though it writes nothing, because the rate of it is the
   *  signal: a queue that is mostly skipped is a queue of unanswerable pairs. */
  "label.skip",

  // --- records ----------------------------------------------------------
  "records.merge.confirm",

  // --- configuration ----------------------------------------------------
  "config.publish",

  // --- tenancy ----------------------------------------------------------
  "dataset.create",
] as const;

/**
 * The header a declared action travels on, browser -> BFF -> erserver.
 *
 * Separate from `traceparent` because it answers a different question. The
 * traceparent makes the backend work part of the click's TRACE; this makes the
 * backend span able to say WHICH control caused it, without a reader having to
 * walk up to the root span to find out. That is the difference between "this
 * SQL belongs to some click" and "this SQL is what the Merge button does".
 *
 * Spoofable, exactly as `x-acting-user` and `traceparent` are: any credential
 * holder can send any value. Tolerable for the same reason — an action name is
 * a correlation handle and never an authorization input, and nothing downstream
 * makes a decision on it. erserver shape-validates it before recording so a
 * hostile value cannot blow up span cardinality or carry a payload.
 */
export const ACTION_HEADER = "x-er-action";

/** A name drawn from [`UI_ACTIONS`]. */
export type UiAction = (typeof UI_ACTIONS)[number];

/** Set form, for the O(1) membership test the resolver and the beacon do. */
const MEMBERS: ReadonlySet<string> = new Set(UI_ACTIONS);

/**
 * Whether `value` is a declared action.
 *
 * Used on both sides of the wire: the browser refuses to mint an interaction for
 * an undeclared name, and `POST /api/telemetry` refuses to record one. The
 * server-side check is not redundant — the browser is not trusted, and the
 * gate's guarantee covers only the markup in this repo.
 */
export function isUiAction(value: string): value is UiAction {
  return MEMBERS.has(value);
}
